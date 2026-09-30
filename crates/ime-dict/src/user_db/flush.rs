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
//! `record` is called from an fcitx5 callback, and a `redb` write transaction is filesystem
//! IO: `AGENTS.md` forbids the latter on that thread, and the frozen `UserFreqSource::record`
//! contract says the same in its own words -- "Implementations batch and flush asynchronously;
//! this method must return within 5us". The record path therefore adds to the delta map and
//! publishes a request, and the transaction happens here, on the store's flush thread.
//!
//! The hand-off is a one-slot request channel with merged semantics: a flush drains the whole
//! delta map, so a request that arrives while another is outstanding has nothing of its own to
//! add. Ordering survives because the thread is a single consumer, which is the guarantee the
//! same-thread flush gave -- without putting a write transaction on the fcitx5 main loop.
//!
//! # What the batching window costs
//!
//! A crash loses at most the window [`COMMIT_INTERVAL_MS`] bounds (`ASM-20`), and the hand-off
//! does not widen it: the triggers are unchanged, and the batch a trigger names reaches the
//! disk as soon as the thread picks it up rather than before the callback returns. The one
//! case that differs is a store whose thread could not be started -- see
//! [`Inner::request_flush`] -- where the record path flushes on its own thread, which is the
//! behaviour this module replaced and the reason it is a degradation and not a fallback.

use std::sync::{Condvar, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::*;

use super::cache::lock_cache;

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
        }
    }
}

/// The deltas one flush takes out of the delta map.
type Drained = Vec<(Box<str>, Delta)>;

/// The tombstones one flush takes out of the delta map.
type Removed = Vec<Box<str>>;

/// How long an explicit flush waits for the flush thread to stand aside.
///
/// The bound is what keeps a disk that has stopped answering from holding the owner's
/// shutdown path: past it the flush proceeds beside the thread, which is safe because both
/// drain the same delta map and neither can see a record twice.
pub(super) const FLUSH_SETTLE_TIMEOUT_MS: u64 = 200;

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
        let mut pending = lock(&self.pending);
        // Committing the word again takes the tombstone back: the user is asking for it to be
        // learned, which is the opposite of the removal still waiting to be flushed, and the
        // later statement wins.
        pending.removed.remove(key);
        let merged = if let Some(entry) = pending.entries.get_mut(key) {
            entry.count = entry.count.saturating_add(1);
            entry.last_used_nanos = now_nanos;
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
        let (drained, removed): (Drained, Removed) = {
            let mut pending = lock(&self.pending);
            if pending.entries.is_empty() && pending.removed.is_empty() {
                return Ok(CommitReport::default());
            }
            pending.last_flush_nanos = now_nanos;
            let mut drained: Drained = Drained::with_capacity(pending.entries.len());
            for (key, delta) in pending.entries.drain() {
                drained.push((key, delta.stamped(now_ms, now_nanos)));
            }
            (drained, pending.removed.drain().collect())
        };
        let started = self.clock.now_nanos();
        match self.write_batch(&drained, &removed, durability) {
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
    pub(super) fn write_batch(
        &self,
        drained: &[(Box<str>, Delta)],
        removed: &[Box<str>],
        durability: Durability,
    ) -> Result<(), String> {
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
                let total = previous.0.saturating_add(delta.count);
                words
                    .insert(key.as_ref(), (total, previous.1.max(delta.last_used_ms)))
                    .map_err(|error| error.to_string())?;
                if caching {
                    updated.push((key.as_ref(), total));
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
}

/// The flush thread's mailbox: a one-slot request channel with merged semantics.
///
/// The slot holds a flag rather than a queue, and that is the contract rather than an
/// optimisation: a flush drains the whole delta map, so a request that arrives while one is
/// outstanding has nothing of its own to add -- the outstanding flush will write it. A queue
/// would therefore cost a wakeup and an entry per request and buy nothing.
#[derive(Debug, Default)]
struct Mailbox {
    /// The slot and the flags that guard it.
    state: Mutex<MailboxState>,
    /// Signalled when a request is published or the thread is asked to stop.
    work: Condvar,
    /// Signalled when the thread stops serving a request.
    idle: Condvar,
}

/// What the mailbox holds.
#[derive(Debug, Default)]
struct MailboxState {
    /// Whether a flush has been asked for and not yet taken.
    requested: bool,
    /// Whether the thread is inside a flush right now.
    serving: bool,
    /// Whether the owner has taken the write path for itself.
    paused: bool,
    /// Whether the owner has asked the thread to stop.
    stopping: bool,
}

/// The store's flush thread.
///
/// It holds a weak handle to the store, so it can never keep one alive: the last [`UserDb`]
/// dropping is what ends it. It is deliberately not joined -- joining would block whatever
/// drops the store behind a database transaction, and that may be the host thread on its way
/// out -- so the handle is dropped and the thread wakes on the stop signal instead.
///
/// # Why one thread and not a pool
///
/// Ordering is part of the contract: one session's records have to reach the store in the
/// order they were committed (`ASM-11`), and a single consumer draining one slot preserves
/// that by construction. `ASM-P05` allows the store two writer threads -- this one and the
/// idle sweep -- and this is the second of them.
pub(super) struct FlushThread {
    mailbox: Arc<Mailbox>,
    /// Kept so that the thread is not detached silently; it is never joined.
    _handle: JoinHandle<()>,
}

impl FlushThread {
    /// Starts the thread, or reports that one could not be started.
    pub(super) fn start(inner: &Arc<Inner>) -> Option<Self> {
        let mailbox = Arc::new(Mailbox::default());
        let thread_mailbox = Arc::clone(&mailbox);
        let weak = Arc::downgrade(inner);
        let handle = std::thread::Builder::new()
            .name("userdb-flush".to_string())
            .spawn(move || serve(&thread_mailbox, &weak))
            .ok()?;
        Some(Self {
            mailbox,
            _handle: handle,
        })
    }

    /// Asks the thread to flush, without waiting for it.
    ///
    /// The lock is taken only when a trigger fires -- once per batch or per interval, not per
    /// keystroke -- and the thread holds it for the few instructions that read two flags, so
    /// the wait a caller can meet here is bounded by a handful of instructions.
    pub(super) fn request(&self) {
        {
            let mut state = lock(&self.mailbox.state);
            state.requested = true;
        }
        self.mailbox.work.notify_one();
    }

    /// Waits until the thread has served everything it was asked to, then holds it there.
    ///
    /// Returns whether the thread stood aside inside `timeout`. Either way the hold is taken,
    /// so the caller's own flush cannot overlap one the thread starts; a writer that did not
    /// stand aside inside the bound may still be inside the flush it was already running, and
    /// that is safe -- two flushes that drain the same delta map each write what they took,
    /// and neither can see a record twice -- while refusing to flush at shutdown is not.
    pub(super) fn pause(&self, timeout: Duration) -> bool {
        self.wait_until_idle(timeout, true)
    }

    /// Releases the pause and wakes the thread.
    pub(super) fn resume(&self) {
        {
            let mut state = lock(&self.mailbox.state);
            state.paused = false;
        }
        self.mailbox.work.notify_one();
    }

    /// Waits until no request is outstanding and none is being served.
    ///
    /// Unlike [`FlushThread::pause`] this takes no hold: the caller only observes, and a
    /// flush that starts after the observation is one the caller did not wait for. That is
    /// the shape two callers want. The store's drop uses it to give the thread a moment to
    /// step out of the file before the last reference lets go of it -- pausing there would
    /// be wrong, because a clone that appears while this one drops must find a thread that
    /// can still serve -- and a test uses it for the same observation without a hold.
    pub(super) fn settle(&self, timeout: Duration) -> bool {
        self.wait_until_idle(timeout, false)
    }

    /// Waits for the thread to be idle, and takes the write path while the lock is held.
    ///
    /// The hold is taken under the same lock that observed the thread idle, which is what
    /// makes it race-free: the thread takes a request under that lock too, so it cannot have
    /// started a flush in between. Without `hold` the wait is a plain observation, which is
    /// all a test needs.
    fn wait_until_idle(&self, timeout: Duration, hold: bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.mailbox.state);
        loop {
            if !state.serving && !state.requested {
                state.paused |= hold;
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.paused |= hold;
                return false;
            }
            state = self
                .mailbox
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for FlushThread {
    /// Stops the thread.
    ///
    /// Dropping the handle alone would detach the thread, and a thread waiting on a condition
    /// variable nobody will signal is a leak: the stop flag and the wakeup are what let it
    /// end, and it ends without being waited for.
    fn drop(&mut self) {
        {
            let mut state = lock(&self.mailbox.state);
            state.stopping = true;
        }
        self.mailbox.work.notify_all();
    }
}

/// The flush thread's loop: take a request, flush, report that it is idle again.
///
/// The store is reached through the weak handle and only for the length of one flush, so a
/// store that is dropped while a request is outstanding ends the thread instead of being
/// resurrected by it.
fn serve(mailbox: &Arc<Mailbox>, inner: &Weak<Inner>) {
    loop {
        {
            let mut state = lock(&mailbox.state);
            while !state.stopping && (state.paused || !state.requested) {
                state = mailbox
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.stopping {
                return;
            }
            state.requested = false;
            state.serving = true;
        }
        let flushed = match inner.upgrade() {
            Some(store) => {
                let _ = store.flush(Durability::Eventual);
                true
            }
            // The store is gone: there is nothing left to write and nobody left to tell.
            None => false,
        };
        let mut state = lock(&mailbox.state);
        state.serving = false;
        mailbox.idle.notify_all();
        drop(state);
        if !flushed {
            return;
        }
    }
}
