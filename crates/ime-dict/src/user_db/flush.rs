//! The write path: one flush, and the batching policy it reports against.
//!
//! Responsibility: take the pending delta and the pending removals, merge them into the
//! store in one write transaction, and report how the flush went. It also owns the policy
//! that widens the batching window when the disk turns out to be slow.
//!
//! Boundaries: this module never decides *when* to flush -- [`Inner::commit_inner`] does,
//! from the batch size, the ceiling and the interval -- and it never reads a frequency. What
//! it owns is the transaction and the two ways a flush can end: the in-memory counts adopt
//! what was written, or the store degrades to read-only and the cache is rolled back.
//!
//! Splitting this out of the store keeps the store's own file about the hot path: a reader
//! looking for why a keystroke costs what it costs reads the record path, and a reader
//! looking for what reaches the disk reads this file.

use super::*;

/// The deltas one flush takes out of the pending state.
type Drained = Vec<(Box<str>, Pending)>;

/// The tombstones one flush takes out of the pending state.
type Removed = Vec<Box<str>>;

impl Inner {
    /// Flushes everything the pending state holds, with `durability`.
    ///
    /// The trigger is the caller's: this runs when the batch size, the ceiling or the
    /// interval says a flush is due, and when the owner is shutting down. A store that has
    /// already degraded answers without touching the disk, which is what keeps a read-only
    /// store usable for the rest of the session.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] -- the `data/readonly-mode` code -- when the write
    /// fails, after the store has degraded to read-only.
    pub(super) fn flush(&self, durability: Durability) -> Result<CommitReport, ImeError> {
        if self.readonly.load(Ordering::Acquire) {
            return Ok(CommitReport::default());
        }
        let (drained, removed): (Drained, Removed) = {
            let mut pending = lock(&self.pending);
            if pending.entries.is_empty() && pending.removed.is_empty() {
                return Ok(CommitReport::default());
            }
            pending.last_flush_nanos = self.clock.now_nanos();
            (
                pending.entries.drain().collect(),
                pending.removed.drain().collect(),
            )
        };
        let started = self.clock.now_nanos();
        match self.write_batch(&drained, &removed, durability) {
            Ok(()) => {
                let elapsed_us = self.clock.now_nanos().saturating_sub(started) / 1_000;
                Ok(self.report(drained.len(), elapsed_us))
            }
            Err(reason) => {
                self.degrade(&drained);
                Err(ImeError::DataReadonly { reason })
            }
        }
    }

    /// Writes one batch of deltas and one batch of removals, merging with what is stored.
    ///
    /// The error is the rendered `redb` failure rather than a typed one: it is carried to
    /// the caller as the reason of the read-only diagnostic, which the frozen error model
    /// spells as a string.
    ///
    /// A key's creation time is stamped the first time the key reaches the file, from the
    /// delta's own last-use stamp: the store has no earlier moment to remember, and the
    /// stamp is written once and never overwritten, so a later flush cannot move it.
    pub(super) fn write_batch(
        &self,
        drained: &[(Box<str>, Pending)],
        removed: &[Box<str>],
        durability: Durability,
    ) -> Result<(), String> {
        #[cfg(test)]
        if take_injected_failure() {
            return Err("injected flush failure".to_string());
        }
        let mut txn = self.db.begin_write().map_err(|error| error.to_string())?;
        {
            let mut words = txn
                .open_table(USER_WORDS)
                .map_err(|error| error.to_string())?;
            let mut meta = txn
                .open_table(USER_META)
                .map_err(|error| error.to_string())?;
            for key in removed {
                words
                    .remove(key.as_ref())
                    .map_err(|error| error.to_string())?;
                meta.remove(key.as_ref())
                    .map_err(|error| error.to_string())?;
            }
            for (key, delta) in drained {
                let previous = words
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .map_or((0, 0), |guard| guard.value());
                let merged = (
                    previous.0.saturating_add(delta.count),
                    previous.1.max(delta.last_used_ms),
                );
                words
                    .insert(key.as_ref(), merged)
                    .map_err(|error| error.to_string())?;
                let stamped = meta
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .is_some();
                if !stamped {
                    meta.insert(key.as_ref(), (delta.last_used_ms, 0))
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        txn.set_durability(durability);
        txn.commit().map_err(|error| error.to_string())?;
        // The in-memory map adopts what was just written, so a later read answers from
        // memory rather than going back to the file. The failure path above returns before
        // this point, and it must: the transaction rolled back, so the map is still right.
        self.adopt(drained);
        for key in removed {
            self.forget_committed(key);
            lock(&self.cache).remove(key);
        }
        Ok(())
    }

    /// Enters read-only mode after a failed flush.
    ///
    /// The drained deltas never reached the disk, so the cache -- which counts committed
    /// plus pending -- is rolled back to the last committed values. The drained removals go
    /// the same way: the transaction rolled back, so the words they named are still in the
    /// file, and the tombstones are gone with them. Reads keep working: a store that cannot
    /// learn must still answer, or the decoder would lose the history it already has.
    ///
    /// The in-memory counts are deliberately left untouched and stay hydrated: they only
    /// ever hold what reached the file, the failed transaction wrote nothing, so they are
    /// still the truth -- and a store that can no longer be written is the one store that
    /// most needs its reads to stay in memory.
    pub(super) fn degrade(&self, drained: &[(Box<str>, Pending)]) {
        self.readonly.store(true, Ordering::Release);
        let mut cache = lock(&self.cache);
        for (key, delta) in drained {
            if let Some(value) = cache.get(key) {
                cache.insert(key, value.saturating_sub(delta.count));
            }
        }
    }

    /// Reports a flush and relaxes the batching policy when the disk is slow.
    ///
    /// A run of flushes past [`SLOW_COMMIT_MS`] costs more than the budget allows, so the
    /// store batches harder instead of blocking the host thread more often. One slow flush
    /// is not enough to act on -- see [`SLOW_COMMIT_STREAK`] -- and a fast flush resets the
    /// run, so a single hiccup cannot move the policy. The relaxation itself is one-way: a
    /// store that has been slow keeps the wider window rather than oscillating between the
    /// two policies.
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
}
