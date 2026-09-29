//! The capacity policy: keeping the store under [`USER_WORD_CAP`] records.
//!
//! Responsibility: decide when the store is too big, find the exact `last_used_ms`
//! threshold that separates the records to remove from the ones to keep, and remove them
//! in short write transactions from a thread of its own.
//!
//! Boundaries: this module owns the sweep and nothing else. It never records, never
//! flushes and never reads a frequency; it moves rows out of `user_words` and out of the
//! cache, and it leaves the store usable at every point in between. Splitting the sweep
//! out of the store is what keeps the store's own file about the hot path.

use std::collections::BinaryHeap;
use std::ops::Bound;
use std::time::Duration;

use redb::ReadOnlyTable;

use super::*;

/// The idle sweep thread, when one could be started.
///
/// The thread holds only a weak handle, so it can never keep the store alive: the last
/// [`UserDb`] dropping is what ends it. It is deliberately not joined -- joining would
/// block shutdown behind a database transaction -- so the handle is dropped and the
/// thread notices the store is gone at its next wakeup.
pub(super) struct Sweep {
    _thread: std::thread::JoinHandle<()>,
}

/// Starts the idle sweep thread, or reports that one could not be started.
///
/// A store without a sweep still answers every query and still flushes; it only stops
/// enforcing its record cap, which is a degradation the caller never sees.
pub(super) fn start_sweep(inner: &Arc<Inner>) -> Option<Sweep> {
    let weak = Arc::downgrade(inner);
    let thread = std::thread::Builder::new()
        .name("userdb-sweep".to_string())
        .spawn(move || {
            loop {
                // One wake per idle window: the thread never spins, and the atomic the record
                // path touches costs that path nothing.
                std::thread::sleep(Duration::from_millis(EVICT_IDLE_MS));
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                inner.sweep_once();
            }
        })
        .ok()?;
    Some(Sweep { _thread: thread })
}

/// One batch of stale keys together with the cursor to resume from.
///
/// The second element is the last key the scan examined, or `None` when the table was
/// walked to the end. It is a tuple rather than a struct because it is only ever produced
/// and immediately destructured in [`Inner::sweep_once`], and the pair has no meaning
/// apart from that hand-off.
type StaleBatch = (Vec<Box<str>>, Option<Box<str>>);

impl Inner {
    /// Whether no record has arrived for at least `idle_ms`.
    fn idle_for_ms(&self, idle_ms: u64) -> bool {
        let quiet = self
            .clock
            .now_nanos()
            .saturating_sub(self.last_record_nanos.load(Ordering::Relaxed));
        quiet >= idle_ms.saturating_mul(1_000_000)
    }

    /// Runs one idle sweep: evicts the oldest records when the store is over its cap.
    ///
    /// A failed sweep is not a failed store -- the records stay and the next idle window
    /// retries -- so the outcome is dropped rather than reported.
    fn sweep_once(&self) {
        if !self.idle_for_ms(EVICT_IDLE_MS) {
            return;
        }
        let Ok(count) = self.stored_count() else {
            return;
        };
        if count > USER_WORD_CAP {
            let _ = self.evict_oldest(EVICT_FRACTION);
        }
    }

    /// Drops the oldest `percent` of the records.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when a transaction fails. A read-only store
    /// evicts nothing and reports zero.
    pub(super) fn evict_oldest(&self, percent: u8) -> Result<u64, ImeError> {
        if percent == 0 || self.readonly.load(Ordering::Acquire) {
            return Ok(0);
        }
        let count = self.stored_count()?;
        let target = (count * u64::from(percent) / 100).clamp(1, MAX_EVICT_KEYS);
        let Some(threshold) = self.stale_threshold(target + 1)? else {
            return Ok(0);
        };
        self.delete_stale(threshold, target)
    }

    /// Returns the `rank`-th smallest `last_used_ms`, or `None` when the store holds fewer
    /// records than `rank`.
    ///
    /// The caller asks for one more than the number of records it wants to remove, so that
    /// everything strictly below the returned stamp is exactly that many records: ties land
    /// on the keeping side, which is the side that cannot delete more than the policy
    /// allows. The heap holds only stamps, never keys, so its size is bounded by the
    /// eviction target rather than by the store.
    ///
    /// A pinned record takes no part in the ranking: it is exempt from eviction, so it must
    /// not be one of the records the percentile counts either, or the policy would remove
    /// fewer records than it names. The pin is read from the metadata table rather than from
    /// the loaded set, because a store too large to load holds no pins in memory at all.
    fn stale_threshold(&self, rank: u64) -> Result<Option<u64>, ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let table = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(&self.path, &error))?;
        let meta = txn
            .open_table(USER_META)
            .map_err(|error| store_error(&self.path, &error))?;
        let mut oldest: BinaryHeap<u64> = BinaryHeap::with_capacity(rank as usize);
        let mut seen = 0u64;
        for entry in table
            .iter()
            .map_err(|error| store_error(&self.path, &error))?
        {
            let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
            if is_pinned(&meta, key.value(), &self.path)? {
                continue;
            }
            seen += 1;
            let stamp = value.value().1;
            if (oldest.len() as u64) < rank {
                oldest.push(stamp);
            } else if let Some(worst) = oldest.peek() {
                if stamp < *worst {
                    oldest.pop();
                    oldest.push(stamp);
                }
            }
        }
        if seen < rank {
            return Ok(None);
        }
        Ok(oldest.peek().copied())
    }

    /// Deletes up to `target` records older than `threshold`, sweeping the key space in
    /// ascending order in short write transactions.
    ///
    /// The transactions are short on purpose: the store is shared with the host thread, and
    /// one transaction over a full store would block a flush for as long as the sweep takes.
    fn delete_stale(&self, threshold: u64, target: u64) -> Result<u64, ImeError> {
        let mut deleted = 0u64;
        let mut cursor: Option<Box<str>> = None;
        while deleted < target {
            let (stale, last) = self.scan_stale(cursor.as_deref(), threshold)?;
            let Some(last) = last else {
                break;
            };
            if !stale.is_empty() {
                self.delete_batch(&stale)?;
                deleted += stale.len() as u64;
            }
            cursor = Some(last);
        }
        Ok(deleted)
    }

    /// Examines the next [`EVICT_BATCH`] keys after `after` and returns the stale ones
    /// together with the last key examined, which is the next cursor.
    ///
    /// A pinned record is never stale however old its stamp is: the sweep passes over it and
    /// leaves the cursor where it was, so the batch it belongs to still ends.
    fn scan_stale(&self, after: Option<&str>, threshold: u64) -> Result<StaleBatch, ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let table = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(&self.path, &error))?;
        let meta = txn
            .open_table(USER_META)
            .map_err(|error| store_error(&self.path, &error))?;
        let range = match after {
            Some(key) => table.range::<&str>((Bound::Excluded(key), Bound::Unbounded)),
            None => table.range::<&str>(..),
        }
        .map_err(|error| store_error(&self.path, &error))?;
        let mut stale = Vec::new();
        let mut last: Option<Box<str>> = None;
        for entry in range.take(EVICT_BATCH) {
            let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
            if value.value().1 < threshold && !is_pinned(&meta, key.value(), &self.path)? {
                stale.push(Box::from(key.value()));
            }
            last = Some(Box::from(key.value()));
        }
        Ok((stale, last))
    }

    /// Removes `keys` from the store, from the in-memory counts and from the cache.
    ///
    /// Both memories hold totals for the keys just removed, so they have to go: a later
    /// `freq` of an evicted key must answer zero, not a count the store no longer holds.
    /// Each lock is taken once per key rather than once per batch, so that a record
    /// arriving while the sweep runs never waits behind the whole batch. The metadata row
    /// goes with the record: it is keyed by the same key, and a row left behind would be
    /// read by the next record that takes that key, giving it a creation time and a pin
    /// that belonged to the word the user had before.
    fn delete_batch(&self, keys: &[Box<str>]) -> Result<(), ImeError> {
        let mut txn = self
            .db
            .begin_write()
            .map_err(|error| store_error(&self.path, &error))?;
        {
            let mut table = txn
                .open_table(USER_WORDS)
                .map_err(|error| store_error(&self.path, &error))?;
            let mut meta = txn
                .open_table(USER_META)
                .map_err(|error| store_error(&self.path, &error))?;
            for key in keys {
                table
                    .remove(key.as_ref())
                    .map_err(|error| store_error(&self.path, &error))?;
                meta.remove(key.as_ref())
                    .map_err(|error| store_error(&self.path, &error))?;
            }
        }
        txn.set_durability(Durability::Eventual);
        txn.commit()
            .map_err(|error| store_error(&self.path, &error))?;
        for key in keys {
            self.forget_committed(key);
            lock(&self.cache).remove(key);
        }
        Ok(())
    }
}

/// Whether the metadata row of `key` says the user pinned it.
///
/// A key with no metadata row is a record an older build wrote, and it was not pinned:
/// there was no way to pin one then. Reading the table rather than the loaded set is what
/// makes the exemption work for a store too large to load, which holds no pins in memory.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the lookup fails.
fn is_pinned(
    meta: &ReadOnlyTable<&str, (u64, u64)>,
    key: &str,
    path: &Path,
) -> Result<bool, ImeError> {
    let held = meta.get(key).map_err(|error| store_error(path, &error))?;
    Ok(held.is_some_and(|guard| guard.value().1 != 0))
}
