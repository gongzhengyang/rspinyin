//! The write path: the delta map, the flush that drains it, and the thread that performs it.
//!
//! Responsibility: hold the commits that have been recorded but not yet written, decide when
//! they are due, merge them into the store in one write transaction, and report how the flush
//! went. It also owns the policy that widens the batching window when the disk turns out to be
//! slow.
//!
//! Boundaries: this module never reads a frequency and never answers a lookup -- that is the
//! loaded half's job -- and it never decides *what* to record. What it owns is the delta map,
//! the transaction, and the two ways a flush can end: the store adopts what was written, or it
//! degrades to read-only.
//!
//! Splitting this out of the store keeps the store's own file about the hot path: a reader
//! looking for why a keystroke costs what it costs reads the record path, and a reader looking
//! for what reaches the disk reads this file.
//!
//! # Why the flush runs on a thread
//!
//! The thread that performs the flush, and the mailbox it reads requests from, live in
//! [`thread`]; what stays here is the delta map the thread drains and the transaction it
//! runs.
//!
//! # What the batching window costs
//!
//! A crash loses at most the window [`COMMIT_INTERVAL_MS`] bounds (`ASM-20`), and the hand-off
//! does not widen it: the triggers are unchanged, and the batch a trigger names reaches the
//! disk as soon as the thread picks it up rather than before the callback returns. The one
//! case that differs is a store whose thread could not be started -- see
//! [`Inner::request_flush`] -- where the record path flushes on its own thread, which is the
//! behaviour this module replaced and the reason it is a degradation and not a fallback.

use std::time::Duration;

use super::*;

use super::cache::lock_cache;

/// The flush thread and its mailbox; see the module's own docs for the hand-off contract.
mod thread;

pub(super) use self::thread::FlushThread;

/// What one flush did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitReport {
    /// Number of records written; zero when nothing was pending. A removal is not a record
    /// and is not counted here.
    pub written: usize,
    /// Duration of the write transaction in microseconds.
    pub elapsed_us: u64,
    /// Whether this flush relaxed the batching policy because the disk was slow.
    pub relaxed: bool,
}

/// The in-memory delta of one key: what has been recorded but not yet flushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pending {
    /// Commits recorded since the last successful flush.
    pub(super) count: u32,
    /// Monotonic nanoseconds of the most recent record.
    ///
    /// Monotonic rather than wall-clock because the record path may not read the wall clock:
    /// it runs inside an fcitx5 callback, and a `SystemTime::now` is the one thing on that
    /// path that can take a lock inside the C library. The conversion to the unix
    /// milliseconds the store holds happens in [`Pending::stamped`], which only the flush and
    /// the two management paths call.
    ///
    /// The stamp is per record rather than per batch on purpose: `evict_oldest` ranks records
    /// by it, and a batch that carried the flush's own reading would make every record of one
    /// window equally old, which is a coarser ranking than the store had before.
    pub(super) last_used_nanos: u64,
    /// The logical clock value this key's most recent record was stamped with.
    ///
    /// The clock lives in an atomic the record path advances per record, so reading it here
    /// costs the same monotonic load the nanosecond stamp does. Eviction ranks by it, and the
    /// decay fold measures a record's age with it.
    pub(super) tick: u64,
}

/// One delta taken out of the delta map, stamped for the store.
///
/// The map holds a monotonic stamp and the store holds a wall-clock one; this is the value
/// the flush converts between them, so that nothing below the flush has to know which clock
/// the map was written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Delta {
    /// Commits recorded since the last successful flush.
    pub(super) count: u32,
    /// Wall-clock milliseconds of the most recent record.
    pub(super) last_used_ms: u64,
    /// The logical clock value of the most recent record.
    pub(super) tick: u64,
}

impl Pending {
    /// The delta the store's writers take: the same count, stamped in wall-clock
    /// milliseconds.
    ///
    /// `now_ms` and `now_nanos` are read together, so the age the record has accumulated is
    /// subtracted from the wall clock as it stands at the conversion. The two clocks advance
    /// at the same rate, which is what makes the subtraction meaningful: a wall-clock step
    /// between the record and the flush moves every stamp of the window by the size of the
    /// step, and the ranking the stamps feed is a ranking of relative ages, so the order
    /// survives it.
    pub(super) fn stamped(self, now_ms: u64, now_nanos: u64) -> Delta {
        let age_ms = now_nanos.saturating_sub(self.last_used_nanos) / 1_000_000;
        Delta {
            count: self.count,
            last_used_ms: now_ms.saturating_sub(age_ms),
            tick: self.tick,
        }
    }
}

/// The deltas one flush takes out of the delta map.
type Drained = Vec<(Box<str>, Delta)>;

/// The tombstones one flush takes out of the delta map.
type Removed = Vec<Box<str>>;

/// The pin changes one flush takes out of the delta map.
type Pinned = Vec<(Box<str>, bool)>;

/// The key-pin rows one flush takes out of the delta map: the input key, and the word
/// its row names -- or `None` for a row to delete.
type KeyRows = Vec<(Box<str>, Option<Box<str>>)>;

/// The coined entries one flush takes out of the delta map: the composite `(key, word)`
/// row key, the word it enrols, and the commits the window accumulated for the pair.
type Coined = Vec<(Box<str>, Box<str>, u32)>;

/// The coined rows one flush takes out as removals, by their composite row key.
type CoinedRemoved = Vec<Box<str>>;

/// How long an explicit flush waits for the flush thread to stand aside.
///
/// The bound is what keeps a disk that has stopped answering from holding the owner's
/// shutdown path: past it the flush proceeds beside the thread, which is safe because both
/// drain the same delta map and neither can see a record twice.
pub(super) const FLUSH_SETTLE_TIMEOUT_MS: u64 = 200;

/// One flush's worth of pending state, handed to [`Inner::write_batch`] as one bundle.
///
/// One struct rather than seven trailing parameters because the seven are drained from the
/// pending lock together and mean nothing apart: a call site that named them positionally
/// had four slices in a row that only the signature told apart.
pub(super) struct Batch<'a> {
    /// The deltas still to write, keyed and stamped.
    pub(super) drained: &'a [(Box<str>, Delta)],
    /// The keys whose records the batch removes.
    pub(super) removed: &'a [Box<str>],
    /// The pin toggles the batch applies.
    pub(super) pinned: &'a [(Box<str>, bool)],
    /// The key rows the batch rewrites, pinned form included.
    pub(super) key_rows: &'a [(Box<str>, Option<Box<str>>)],
    /// The coined words the batch writes, with their source and count.
    pub(super) coined: &'a [(Box<str>, Box<str>, u32)],
    /// The coined words whose records the batch removes.
    pub(super) coined_removed: &'a [Box<str>],
    /// How hard the write promises the data survived.
    pub(super) durability: Durability,
}

impl Inner {
    /// Adds one commit of `key` to the delta map, answering whether a flush is now due.
    ///
    /// This is the whole of what the record path does: one monotonic reading, one lock of the
    /// delta map, one hash lookup, and -- only for a key the map has not seen -- one
    /// allocation. The wall clock is not read at all; the delta carries a monotonic stamp and
    /// the flush converts it (see [`Pending::stamped`]).
    ///
    /// The monotonic reading cannot be deferred the way the wall-clock one is: the interval
    /// trigger is what bounds the durability window (`ASM-20`), and it is evaluated per
    /// record -- including for a key that is already pending. Skipping it for repeated keys
    /// would let a user who commits one word over and over keep that word in memory for the
    /// whole session, because neither the batch trigger nor the ceiling would ever fire.
    pub(super) fn record_delta(&self, key: &str) -> bool {
        let now_nanos = self.clock.now_nanos();
        self.last_record_nanos.store(now_nanos, Ordering::Relaxed);
        // One commit, one tick of the logical clock: the clock measures usage, so the
        // half-life a decay names in commits is a half-life in these ticks. The advance
        // is an atomic add -- no lock, no IO -- and the value this record stamped is the
        // one the fetch returns.
        let tick = self.tick.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let mut pending = lock(&self.pending);
        // Committing the word again takes the tombstone back: the user is asking for it to be
        // learned, which is the opposite of the removal still waiting to be flushed, and the
        // later statement wins.
        pending.removed.remove(key);
        let merged = if let Some(entry) = pending.entries.get_mut(key) {
            entry.count = entry.count.saturating_add(1);
            entry.last_used_nanos = now_nanos;
            entry.tick = tick;
            true
        } else {
            false
        };
        if !merged {
            // The one allocation this path makes, and only for a key the delta map has not
            // seen: an `entry(Box::from(key))` lookup would build the box on every call, hit
            // or miss, and the steady state -- the same word committed again and again -- is
            // a hit.
            let fresh = Pending {
                count: 1,
                last_used_nanos: now_nanos,
                tick,
            };
            pending.entries.insert(Box::from(key), fresh);
        }
        let held = pending.entries.len();
        let batch = self.batch.load(Ordering::Relaxed);
        let interval_ms = self.interval_ms.load(Ordering::Relaxed);
        let elapsed = now_nanos.saturating_sub(pending.last_flush_nanos);
        held >= batch
            || held >= PENDING_CAPACITY
            || elapsed >= interval_ms.saturating_mul(1_000_000)
    }

    /// Converts a delta snapshot into the wall-clock stamps the store speaks.
    ///
    /// The management and export paths read the delta map to render what the user has
    /// learned, and what they render is unix milliseconds. Both clocks are read once for the
    /// whole snapshot: every record it holds was recorded before the call, so one instant
    /// dates them all without changing the order between them -- and that order is what the
    /// enumeration is sorted by.
    pub(super) fn stamp_snapshot(
        &self,
        snapshot: HashMap<Box<str>, Pending>,
    ) -> HashMap<Box<str>, Delta> {
        let now_ms = self.clock.now_ms();
        let now_nanos = self.clock.now_nanos();
        snapshot
            .into_iter()
            .map(|(key, delta)| (key, delta.stamped(now_ms, now_nanos)))
            .collect()
    }

    /// Asks the flush thread to write the delta map, without waiting for it.
    ///
    /// The host thread's contract is that a commit never blocks it: the transaction is handed
    /// to the thread that owns the store's slow path and the call returns. The failure is not
    /// swallowed either -- it flips the store to read-only, which is what the caller polls.
    ///
    /// A store whose thread could not be started flushes here instead, on the caller's
    /// thread. That is the one case in which the record path touches the disk, and it is a
    /// degradation rather than a failure: a store without a flush thread still flushes,
    /// exactly as a store without a sweep thread still answers every query.
    pub(super) fn request_flush(&self) {
        match self.writer.get().and_then(|set| set.as_ref()) {
            Some(writer) => writer.request(),
            None => {
                let _ = self.flush(Durability::Eventual);
            }
        }
    }

    /// Flushes on the calling thread, with the writer held aside while it runs.
    ///
    /// The two explicit entry points -- `commit` and `final_commit` -- use this: both are the
    /// owner's own calls, both may wait, and both have to answer a report to their caller.
    /// Holding the writer aside is what keeps the two from being inside a transaction at
    /// once, so that the flush which returns is the flush that made the delta durable
    /// (`ASM-20`: a graceful shutdown loses nothing).
    pub(super) fn flush_now(&self, durability: Durability) -> Result<CommitReport, ImeError> {
        let writer = self.writer.get().and_then(|set| set.as_ref());
        let Some(writer) = writer else {
            return self.flush(durability);
        };
        // A writer that does not answer inside the bound is left running, and the flush below
        // proceeds beside it; see [`FlushThread::pause`] for why that is the safe direction.
        let _settled = writer.pause(Duration::from_millis(FLUSH_SETTLE_TIMEOUT_MS));
        let outcome = self.flush(durability);
        writer.resume();
        outcome
    }

    /// Flushes everything the delta map holds, with `durability`.
    ///
    /// The trigger is the caller's: this runs when the batch size, the ceiling or the interval
    /// says a flush is due, when the owner is shutting down, and when the flush thread picks
    /// up a request. A store that has already degraded answers without touching the disk,
    /// which is what keeps a read-only store usable for the rest of the session.
    ///
    /// A reader that looks between the drain and the adopt sees a record in neither half of
    /// the answer and undercounts by one window. That is deliberate: closing it would mean
    /// holding the loaded counts' lock across a write transaction, which is the lock a reader
    /// on the decode path must never wait behind, and the signal this store carries is a
    /// ranking, not a total -- `ASM-04` and `ASM-20` both allow a window to be lost, and this
    /// is a window that is merely late.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] -- the `data/readonly-mode` code -- when the write
    /// fails, after the store has degraded to read-only.
    pub(super) fn flush(&self, durability: Durability) -> Result<CommitReport, ImeError> {
        if self.readonly.load(Ordering::Acquire) {
            return Ok(CommitReport::default());
        }
        // Both readings are taken together, before the delta map is touched: the flush converts
        // each delta's monotonic stamp into a wall-clock one, and a conversion that read the
        // two clocks at different moments would date one batch from two instants.
        let now_nanos = self.clock.now_nanos();
        let now_ms = self.clock.now_ms();
        let (drained, removed, pinned, key_rows, coined, coined_removed): (
            Drained,
            Removed,
            Pinned,
            KeyRows,
            Coined,
            CoinedRemoved,
        ) = {
            let mut pending = lock(&self.pending);
            if pending.entries.is_empty()
                && pending.removed.is_empty()
                && pending.pins.is_empty()
                && pending.key_rows.is_empty()
                && pending.coined.is_empty()
                && pending.coined_removed.is_empty()
            {
                return Ok(CommitReport::default());
            }
            pending.last_flush_nanos = now_nanos;
            let mut drained: Drained = Drained::with_capacity(pending.entries.len());
            for (key, delta) in pending.entries.drain() {
                drained.push((key, delta.stamped(now_ms, now_nanos)));
            }
            let pinned = pending.pins.drain().collect();
            let key_rows = pending.key_rows.drain().collect();
            let coined = pending
                .coined
                .drain()
                .map(|(composite, (word, count))| (composite, word, count))
                .collect();
            let coined_removed = pending.coined_removed.drain().collect();
            (
                drained,
                pending.removed.drain().collect(),
                pinned,
                key_rows,
                coined,
                coined_removed,
            )
        };
        let started = self.clock.now_nanos();
        match self.write_batch(Batch {
            drained: &drained,
            removed: &removed,
            pinned: &pinned,
            key_rows: &key_rows,
            coined: &coined,
            coined_removed: &coined_removed,
            durability,
        }) {
            Ok(()) => {
                let elapsed_us = self.clock.now_nanos().saturating_sub(started) / 1_000;
                Ok(self.report(drained.len(), elapsed_us))
            }
            Err(reason) => {
                self.degrade();
                Err(ImeError::DataReadonly { reason })
            }
        }
    }

    /// Writes one batch of deltas and one batch of removals, merging with what is stored.
    ///
    /// The error is the rendered `redb` failure rather than a typed one: it is carried to the
    /// caller as the reason of the read-only diagnostic, which the frozen error model spells
    /// as a string.
    ///
    /// A key's creation time is stamped the first time the key reaches the file, from the
    /// delta's own last-use stamp: the store has no earlier moment to remember, and the stamp
    /// is written once and never overwritten, so a later flush cannot move it.
    pub(super) fn write_batch(&self, batch: Batch<'_>) -> Result<(), String> {
        let Batch {
            drained,
            removed,
            pinned,
            key_rows,
            coined,
            coined_removed,
            durability,
        } = batch;
        #[cfg(test)]
        if take_injected_failure() {
            return Err("injected flush failure".to_string());
        }
        // Collected only for a store that has a cache to bring up to date -- a loaded one has
        // none -- and bounded by the batch, so this is a batch-sized allocation at most.
        let caching = self.cache.is_some();
        let mut updated: Vec<(&str, u32)> = Vec::new();
        let mut txn = self.db.begin_write().map_err(|error| error.to_string())?;
        {
            let mut words = txn
                .open_table(USER_WORDS_V2)
                .map_err(|error| error.to_string())?;
            let mut meta = txn
                .open_table(USER_META)
                .map_err(|error| error.to_string())?;
            let mut key_pins = txn
                .open_table(USER_KEY_PINS)
                .map_err(|error| error.to_string())?;
            for key in removed {
                words
                    .remove(key.as_ref())
                    .map_err(|error| error.to_string())?;
                meta.remove(key.as_ref())
                    .map_err(|error| error.to_string())?;
            }
            // The pin's two rows land together: the flag the ranking reads, and the
            // key association the toggle reads. A key row without its flag would rank
            // nothing, and a flag without its key row could never be released by the
            // key that owns it.
            for (key, word) in key_rows {
                match word {
                    Some(word) => key_pins
                        .insert(key.as_ref(), word.as_ref())
                        .map_err(|error| error.to_string())?,
                    None => key_pins
                        .remove(key.as_ref())
                        .map_err(|error| error.to_string())?,
                };
            }
            for (key, is_pinned) in pinned {
                let previous = meta
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .map_or((0, 0), |guard| guard.value());
                meta.insert(key.as_ref(), (previous.0, u64::from(*is_pinned)))
                    .map_err(|error| error.to_string())?;
            }
            // The coined rows land in the same transaction as everything else: one flush,
            // one commit, and the durability window the frequency records live under is
            // the one the coins live under too. An insert of an entry the file already
            // holds adds the window's commits to what is stored, which is the
            // confirmation the coin path recorded.
            let mut lexicon = txn
                .open_table(USER_LEXICON)
                .map_err(|error| error.to_string())?;
            for composite in coined_removed {
                lexicon
                    .remove(composite.as_ref())
                    .map_err(|error| error.to_string())?;
            }
            for (composite, _word, count) in coined {
                let previous = lexicon
                    .get(composite.as_ref())
                    .map_err(|error| error.to_string())?
                    .map_or((0u32, 0u64), |guard| guard.value());
                lexicon
                    .insert(
                        composite.as_ref(),
                        (previous.0.saturating_add(*count), previous.1),
                    )
                    .map_err(|error| error.to_string())?;
            }
            for (key, delta) in drained {
                let previous = words
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .map_or(
                        StoredRow {
                            commits: 0,
                            last_used_ms: 0,
                            tick: 0,
                            dee_q16: 0,
                        },
                        |guard| {
                            let (commits, last_used_ms, tick, dee) = guard.value();
                            StoredRow {
                                commits,
                                last_used_ms,
                                tick,
                                dee_q16: dee_q16(dee),
                            }
                        },
                    );
                let next = self.merged_record(previous.as_stored(), *delta);
                words
                    .insert(
                        key.as_ref(),
                        (
                            next.commits,
                            previous.last_used_ms.max(delta.last_used_ms),
                            next.tick,
                            next.dee_bits(),
                        ),
                    )
                    .map_err(|error| error.to_string())?;
                if caching {
                    updated.push((key.as_ref(), self.effective_freq(&next)));
                }
                let stamped = meta
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .is_some();
                if !stamped {
                    meta.insert(key.as_ref(), (delta.last_used_ms, 0))
                        .map_err(|error| error.to_string())?;
                }
            }
            // The clock's file copy follows the batch it stamped, so a reopen resumes the
            // clock at least where the file's records left it -- a crash between this and
            // the next flush rewinds it by one window, and a rewind only makes records
            // younger, which the fold bounds the same way it bounds every age.
            let mut clock = txn
                .open_table(META_TICK)
                .map_err(|error| error.to_string())?;
            clock
                .insert(TICK_KEY, self.tick.load(Ordering::Relaxed))
                .map_err(|error| error.to_string())?;
        }
        txn.set_durability(durability);
        txn.commit().map_err(|error| error.to_string())?;
        // The in-memory map adopts what was just written, so a later read answers from
        // memory rather than going back to the file. The failure path above returns before
        // this point, and it must: the transaction rolled back, so the map is still right.
        self.adopt(drained);
        // The fallback path's cache holds what the file holds, and the file's value for these
        // keys has just changed: a resident entry has to follow, or the next read of a store
        // too large to load would answer the value the store held before the flush. The record
        // path never touches the cache -- it may not take a second lock -- so this is the only
        // place that keeps it in step.
        if let Some(mut cache) = lock_cache(&self.cache) {
            for (key, total) in updated {
                cache.refresh(key, total);
            }
            for key in removed {
                cache.remove(key);
            }
        }
        for key in removed {
            self.forget_committed(key);
        }
        Ok(())
    }

    /// Enters read-only mode after a failed flush.
    ///
    /// The drained deltas never reached the disk and are gone with the transaction that
    /// carried them, which is the window `ASM-20` allows; the reads keep working, because a
    /// store that cannot learn must still answer or the decoder would lose the history it
    /// already has.
    ///
    /// The in-memory counts are deliberately left untouched and stay hydrated: they only ever
    /// hold what reached the file, the failed transaction wrote nothing, so they are still the
    /// truth -- and a store that can no longer be written is the one store that most needs its
    /// reads to stay in memory. The cache needs no rollback for the same reason: an entry
    /// holds the file's value, and the file did not change.
    pub(super) fn degrade(&self) {
        self.readonly.store(true, Ordering::Release);
    }

    /// Reports a flush and relaxes the batching policy when the disk is slow.
    ///
    /// A run of flushes past [`SLOW_COMMIT_MS`] costs more than the budget allows, so the
    /// store batches harder instead of blocking the host thread more often. One slow flush
    /// is not enough to act on -- see [`SLOW_COMMIT_STREAK`] -- and a fast flush resets the
    /// run, so a single hiccup cannot move the policy. The relaxation itself is one-way: a
    /// store that has been slow keeps the wider window rather than oscillating between the
    /// two policies.
    ///
    /// The report is only observable through `commit` and `final_commit`, because the flush a
    /// batch trigger earns runs on the store's own thread and answers to nobody. That is where
    /// the caller reads [`CommitReport::relaxed`] from, and it is why the code the caller
    /// reports for it -- `data/commit/slow-disk` -- is emitted from there and not from the
    /// record path.
    pub(super) fn report(&self, written: usize, elapsed_us: u64) -> CommitReport {
        let streak = if elapsed_us >= SLOW_COMMIT_MS * 1_000 {
            self.slow_streak
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1)
        } else {
            self.slow_streak.store(0, Ordering::Relaxed);
            0
        };
        let mut relaxed = false;
        if streak >= SLOW_COMMIT_STREAK && self.batch.load(Ordering::Relaxed) < RELAXED_COMMIT_BATCH
        {
            self.batch.store(RELAXED_COMMIT_BATCH, Ordering::Relaxed);
            self.interval_ms
                .store(RELAXED_COMMIT_INTERVAL_MS, Ordering::Relaxed);
            relaxed = true;
        }
        CommitReport {
            written,
            elapsed_us,
            relaxed,
        }
    }

    /// Swaps the batching window in place, as [`UserDb::set_flush_window`] describes.
    pub(super) fn set_flush_window(&self, batch: usize, interval_ms: u64) {
        self.batch.store(batch.max(1), Ordering::Relaxed);
        self.interval_ms.store(interval_ms, Ordering::Relaxed);
    }
}

/// The batching-window setter the host's configuration step reaches the store through.
impl UserDb {
    /// Applies the batching window a configuration names, without reinstalling anything.
    ///
    /// The window is the record path's own trigger: a flush becomes due once `batch`
    /// distinct keys are pending or `interval_ms` of typing has passed, whichever first.
    /// Both live in the same atomics the slow-disk relaxation writes, so this is two
    /// lock-free stores and a reconfiguration that races a record cannot wedge either
    /// side -- the record in flight keeps the window it loaded, the next one sees the new
    /// policy. `batch` below one is raised to one, since a zero batch would report a
    /// flush due before any record existed.
    ///
    /// This is the parameter channel the host layer maps the `data.durability` spelling
    /// onto: the store never sees the configuration, only the window it names. The one
    /// window this call does not own is the relaxation a measured slow disk applies, which
    /// may widen what was set here; the next application of the configuration restores it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_flush_window(&self, batch: usize, interval_ms: u64) {
        self.inner.set_flush_window(batch, interval_ms);
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the batching window the record path flushes with.
    //!
    //! The fixtures live in [`super::super::tests`], which is where the store's own test
    //! infrastructure is; the assertions here are the ones that need this module's private
    //! seams -- `wait_for_flush` and the pending counters -- to tell "not yet" from "never".

    use super::super::tests::{TestClock, open_in, temp_dir};
    use super::*;

    #[test]
    fn test_set_flush_window_zero_window_flushes_every_record() {
        // The window `data.durability = "immediate"` is mapped to: one record and no
        // milliseconds, so nothing accumulates past the commit that is being recorded.
        let dir = temp_dir("flush-window-immediate");
        let db = open_in(&dir, TestClock::default());
        db.record("zuo'tian", 0);
        assert_eq!(
            db.record_count().expect("counting"),
            0,
            "the shipped window holds a single record back"
        );

        db.set_flush_window(1, 0);
        db.record("jin'tian", 0);
        db.wait_for_flush();
        assert_eq!(
            db.record_count().expect("counting"),
            2,
            "the zero window held nothing back, the record it inherited included"
        );
        assert_eq!(db.pending_len(), 0, "the delta map was drained");
    }

    #[test]
    fn test_set_flush_window_restores_the_shipped_window() {
        // The setter is how a reload re-applies the configuration in both directions: a
        // window narrowed to zero is widened back to the shipped one, which batches again.
        let dir = temp_dir("flush-window-eventual");
        let db = open_in(&dir, TestClock::default());
        db.set_flush_window(1, 0);
        db.set_flush_window(COMMIT_BATCH, COMMIT_INTERVAL_MS);

        db.record("zuo'tian", 0);
        db.wait_for_flush();
        assert_eq!(
            db.record_count().expect("counting"),
            0,
            "the shipped window holds the record back again"
        );
        assert_eq!(db.pending_len(), 1, "the record stays pending");
    }
}
