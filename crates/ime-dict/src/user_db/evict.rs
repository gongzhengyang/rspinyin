//! The capacity policy: keeping the store under [`USER_WORD_CAP`] records.
//!
//! Responsibility: decide when the store is too big, find the exact `tick` threshold that
//! separates the records to remove from the ones to keep, and remove them in short write
//! transactions from a thread of its own.
//!
//! Boundaries: this module owns the sweep and nothing else. It never records, never
//! flushes and never reads a frequency; it moves rows out of the words table and out of
//! the cache, and it leaves the store usable at every point in between. Splitting the
//! sweep out of the store is what keeps the store's own file about the hot path.
//!
//! The ranking is the logical tick, not the wall clock: the tick is what the decay fold
//! user-decay arithmetic also reads a record's age from, so "oldest" means the same thing to the
//! policy that evicts as to the arithmetic that decays. One consequence is that the rows
//! a schema-1 migration stamped with tick zero are all equally old -- they tie on the
//! keeping side of the threshold and only become evictable once enough later records
//! carry ticks for the rank to clear zero.

use std::collections::BinaryHeap;
use std::ops::Bound;
use std::time::Duration;

use super::*;

use super::cache::lock_cache;

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

/// One transaction's folded records and the cursor that continues the sweep.
type FoldedBatch = (Vec<(Box<str>, Stored)>, Option<Box<str>>);

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
        let _ = self.fold_forward();
        if count > USER_WORD_CAP {
            let _ = self.evict_oldest(EVICT_FRACTION);
        }
    }

    /// Folds every record on disk forward to the current logical clock.
    ///
    /// The write is split by [`EVICT_BATCH`] so the sweep shares the file in the same
    /// short transactions eviction uses. A record's tick moves with its accumulator:
    /// folding it again from the old anchor would repeat the same work, and the next
    /// fold then covers only the commits that arrived since this sweep. Failures are
    /// returned to the caller; the idle sweep treats them the way it treats eviction
    /// failures, as a reason to retry on the next idle window.
    pub(super) fn fold_forward(&self) -> Result<u64, ImeError> {
        if self.readonly.load(Ordering::Acquire) || self.v1_readonly {
            return Ok(0);
        }
        let now = self.tick.load(Ordering::Relaxed);
        let mut folded = 0u64;
        let mut cursor: Option<Box<str>> = None;
        loop {
            let (batch, last) = self.fold_batch(cursor.as_deref(), now)?;
            folded += batch.len() as u64;
            let Some(last) = last else {
                break;
            };
            cursor = Some(last);
        }
        Ok(folded)
    }

    /// Folds the next [`EVICT_BATCH`] records after `after`, in one transaction.
    ///
    /// The updated records are returned so the commit can bring the hydrated map and
    /// the fallback cache into step only after the file has accepted them. `last` is
    /// the final key examined even when that record already sat at `now`, so the
    /// cursor still advances across an already-current store.
    fn fold_batch(&self, after: Option<&str>, now: u64) -> Result<FoldedBatch, ImeError> {
        let mut txn = self
            .db
            .begin_write()
            .map_err(|error| store_error(&self.path, &error))?;
        let mut folded = Vec::new();
        let mut last: Option<Box<str>> = None;
        let mut updates: Vec<(Box<str>, u32, u64, i64)> = Vec::new();
        {
            let words = txn
                .open_table(USER_WORDS_V2)
                .map_err(|error| store_error(&self.path, &error))?;
            let range = match after {
                Some(key) => words.range::<&str>((Bound::Excluded(key), Bound::Unbounded)),
                None => words.range::<&str>(..),
            }
            .map_err(|error| store_error(&self.path, &error))?;
            for entry in range.take(EVICT_BATCH) {
                let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
                let key = key.value();
                let (commits, last_used_ms, tick, dee) = value.value();
                let folded_dee = self.decay.fold(dee_q16(dee), tick, now);
                if folded_dee != dee_q16(dee) || tick != now {
                    updates.push((Box::from(key), commits, last_used_ms, folded_dee));
                    folded.push((
                        Box::from(key),
                        Stored {
                            commits,
                            dee_q16: folded_dee,
                            tick: now,
                        },
                    ));
                }
                last = Some(Box::from(key));
            }
        }
        if !updates.is_empty() {
            let mut words = txn
                .open_table(USER_WORDS_V2)
                .map_err(|error| store_error(&self.path, &error))?;
            for (key, commits, last_used_ms, folded_dee) in updates {
                words
                    .insert(
                        key.as_ref(),
                        (commits, last_used_ms, now, dee_bits(folded_dee)),
                    )
                    .map_err(|error| store_error(&self.path, &error))?;
            }
        }
        if !folded.is_empty() {
            txn.set_durability(Durability::Eventual);
            txn.commit()
                .map_err(|error| store_error(&self.path, &error))?;
            self.adopt_folded(&folded);
        }
        Ok((folded, last))
    }

    /// Adopts records the idle fold wrote to the in-memory read paths.
    fn adopt_folded(&self, folded: &[(Box<str>, Stored)]) {
        if self.is_hydrated {
            let mut committed = lock(&self.committed);
            for (key, record) in folded {
                committed.insert(key.clone(), *record);
            }
        }
        if let Some(mut cache) = lock_cache(&self.cache) {
            for (key, record) in folded {
                cache.insert(key, self.effective_freq(record));
            }
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

    /// Returns the `rank`-th smallest `tick`, or `None` when the store holds fewer
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
    /// fewer records than it names. The pin is read from the loaded set, which every open
    /// fills -- a store past the record ceiling included -- and which the toggle updates on
    /// the keystroke itself, so a pin protects from the moment the user makes it rather
    /// than from the flush that persists it.
    fn stale_threshold(&self, rank: u64) -> Result<Option<u64>, ImeError> {
        let pins = lock(&self.pinned).clone();
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let table = txn
            .open_table(USER_WORDS_V2)
            .map_err(|error| store_error(&self.path, &error))?;
        let mut oldest: BinaryHeap<u64> = BinaryHeap::with_capacity(rank as usize);
        let mut seen = 0u64;
        for entry in table
            .iter()
            .map_err(|error| store_error(&self.path, &error))?
        {
            let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
            if pins.contains(key.value()) {
                continue;
            }
            seen += 1;
            let stamp = value.value().2;
            if (oldest.len() as u64) < rank {
                oldest.push(stamp);
            } else if let Some(worst) = oldest.peek()
                && stamp < *worst
            {
                oldest.pop();
                oldest.push(stamp);
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
    /// leaves the cursor where it was, so the batch it belongs to still ends. The pin is
    /// read from the loaded set, for the same reason [`Inner::stale_threshold`] reads it
    /// there: the set is what the toggle updates, and it is filled at every open.
    fn scan_stale(&self, after: Option<&str>, threshold: u64) -> Result<StaleBatch, ImeError> {
        let pins = lock(&self.pinned).clone();
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let table = txn
            .open_table(USER_WORDS_V2)
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
            if value.value().2 < threshold && !pins.contains(key.value()) {
                stale.push(Box::from(key.value()));
            }
            last = Some(Box::from(key.value()));
        }
        Ok((stale, last))
    }

    /// Removes `keys` from the store, from the in-memory counts and from the cache.
    ///
    /// Both memories hold values for the keys just removed, so they have to go: a later
    /// `freq` of an evicted key must answer zero, not a count the store no longer holds. The
    /// loaded counts are dropped one key at a time, so that a record arriving while the sweep
    /// runs never waits behind the whole batch; the cache is dropped in one pass instead,
    /// because the record path no longer touches it -- the delta map is the only lock a record
    /// takes -- and there is nothing left for the per-key rule to protect there. The metadata
    /// row goes with the record: it is keyed by the same key, and a row left behind would be
    /// read by the next record that takes that key, giving it a creation time and a pin
    /// that belonged to the word the user had before.
    fn delete_batch(&self, keys: &[Box<str>]) -> Result<(), ImeError> {
        let mut txn = self
            .db
            .begin_write()
            .map_err(|error| store_error(&self.path, &error))?;
        {
            let mut table = txn
                .open_table(USER_WORDS_V2)
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
        }
        if let Some(mut cache) = lock_cache(&self.cache) {
            for key in keys {
                cache.remove(key);
            }
        }
        Ok(())
    }
}
