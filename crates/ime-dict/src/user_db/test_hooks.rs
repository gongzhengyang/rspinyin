//! The store's test-only seams: injected failures, injected migration failures, the
//! store-read counter, and the read-only views of internal state.
//!
//! Responsibility: be the one place a test reaches into the store without a real refusal
//! to stand on. The degradation paths -- a failed flush, a failed migration -- are
//! exercised deterministically by arming a failure here rather than by building a disk
//! that refuses writes, which no test may assume and no write through an already-open
//! descriptor respects anyway. The hot path's whole claim is that a lookup on a loaded
//! store opens no read transaction, and a count of zero cannot be asserted without a
//! counter; the alternative is timing a call, which no test in this workspace may do.
//! The accessors expose internal counts for the same reason: the counts are
//! implementation details, and production code reads the state they shadow through the
//! public surface instead.
//!
//! Boundaries: nothing here runs in a production build -- the module is compiled only
//! under `cfg(test)` -- and nothing here decides anything. Each seam is one flag, one
//! counter, or one lock-and-read.

use std::cell::Cell;

use super::*;

thread_local! {
    /// Test-only: makes the next flush fail.
    ///
    /// The degradation path has to be exercised deterministically, and a real refusal
    /// needs a read-only filesystem or a full disk -- neither of which a test may assume,
    /// and neither of which a write through an already-open descriptor respects anyway.
    static INJECTED_FAILURE: Cell<bool> = const { Cell::new(false) };

    /// Test-only: makes the next schema migration fail.
    ///
    /// The v1-read-only degradation has the same shape as the flush one -- a write
    /// transaction that fails is what both degrade on -- and a test reaches it the same
    /// way: by arming the failure instead of building a disk that refuses writes.
    static INJECTED_MIGRATION_FAILURE: Cell<bool> = const { Cell::new(false) };

    /// Test-only: counts the store reads this thread has opened.
    ///
    /// The hot path's whole claim is that a lookup on a loaded store opens no read
    /// transaction, and a count of zero cannot be asserted without a counter -- the
    /// alternative is to time the call, which no test in this workspace may do.
    static STORE_READS: Cell<u64> = const { Cell::new(0) };
}

/// Counts one store read. Test-only.
#[cfg(test)]
pub(super) fn note_store_read() {
    STORE_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
}

/// Store reads this thread has opened so far. Test-only.
#[cfg(test)]
pub(super) fn store_reads() -> u64 {
    STORE_READS.with(|reads| reads.get())
}

/// Arms the next flush to fail. Test-only.
///
/// The crate-wide visibility is the coin table's tests' door: they arm a flush failure
/// the same way the store's own tests do, and both go through this one hook.
#[cfg(test)]
pub(crate) fn inject_failure() {
    INJECTED_FAILURE.with(|flag| flag.set(true));
}

/// Consumes an armed failure. Test-only.
#[cfg(test)]
pub(super) fn take_injected_failure() -> bool {
    INJECTED_FAILURE.with(|flag| flag.replace(false))
}

/// Arms the next schema migration to fail. Test-only.
#[cfg(test)]
pub(super) fn inject_migration_failure() {
    INJECTED_MIGRATION_FAILURE.with(|flag| flag.set(true));
}

/// Consumes an armed migration failure. Test-only.
#[cfg(test)]
pub(super) fn take_injected_migration_failure() -> bool {
    INJECTED_MIGRATION_FAILURE.with(|flag| flag.replace(false))
}

/// The read-only views of the store's internal state that a test asserts through.
///
/// Test-only, and compiled only with the module above: production code reads the same
/// facts through [`UserDb::record_count`], [`UserFreqSource::freq`] and the
/// [`UserDb::is_hydrated`] report, which is what keeps these names out of the public
/// surface.
impl UserDb {
    /// How many records are waiting to be flushed.
    ///
    /// Test-only: the count is an implementation detail, and production code reads it
    /// through [`UserDb::record_count`] after a flush.
    pub(super) fn pending_len(&self) -> usize {
        lock(&self.inner.pending).entries.len()
    }

    /// How many removals are waiting to be flushed.
    ///
    /// Test-only, and the counterpart of [`UserDb::pending_len`]: a tombstone is the one
    /// other thing the pending state holds.
    pub(super) fn pending_removed_len(&self) -> usize {
        lock(&self.inner.pending).removed.len()
    }

    /// How many penalty rows are waiting to be flushed.
    ///
    /// Test-only, and the counterpart of [`UserDb::pending_removed_len`]: a penalty change
    /// queues the row's absolute value the way a tombstone queues a removal, and a test
    /// that pins the batching of the negative-feedback writes reads the queue through here.
    pub(super) fn pending_penalties_len(&self) -> usize {
        lock(&self.inner.pending).penalties.len()
    }

    /// The penalty level one key currently answers with.
    ///
    /// Test-only: the level is an implementation detail, and production code reads its
    /// effect through [`UserFreqSource::freq`], which is the shift applied.
    pub(super) fn penalty_of(&self, key: &str) -> u8 {
        lock(&self.inner.penalties).get(key).copied().unwrap_or(0)
    }

    /// How many keys the in-memory counts hold, or `None` when the store was not loaded.
    ///
    /// Test-only: the count is an implementation detail, and production code reads the
    /// store's size through [`UserDb::record_count`].
    pub(super) fn committed_len(&self) -> Option<usize> {
        if !self.inner.is_hydrated {
            return None;
        }
        Some(lock(&self.inner.committed).len())
    }

    /// Waits until the flush thread has served everything it was asked to.
    ///
    /// Test-only: the production paths that need this are `commit` and `final_commit`, and
    /// both flush on the caller's own thread. A test that asserts what a trigger wrote cannot
    /// otherwise tell "not yet" from "never", and the bound is generous because a flush of a
    /// batch is bounded by one write transaction -- a thread that misses it is broken rather
    /// than slow, which is what the assertion says.
    pub(super) fn wait_for_flush(&self) {
        if let Some(writer) = self.inner.writer.get().and_then(|set| set.as_ref()) {
            assert!(
                writer.settle(std::time::Duration::from_secs(5)),
                "the flush thread did not settle"
            );
        }
    }

    /// Whether the store was opened with the fallback path's cache.
    ///
    /// Test-only: it is what turns "the cache exists only for a store that was not loaded"
    /// from a code-review item into an assertion.
    pub(super) fn has_miss_cache(&self) -> bool {
        self.inner.cache.is_some()
    }
}
