//! The flush thread: the worker that takes the delta map to the disk.
//!
//! Responsibility: own the one-slot request mailbox and the thread that serves it, so the
//! write transaction never runs on the host thread. The flush itself -- the drain, the
//! transaction and the report -- lives in the parent module; what lives here is the
//! hand-off.
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

use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::super::{Durability, Inner, lock};

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
/// It holds a weak handle to the store, so it can never keep one alive: the last `UserDb`
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
pub(in crate::user_db) struct FlushThread {
    mailbox: Arc<Mailbox>,
    /// Kept so that the thread is not detached silently; it is never joined.
    _handle: JoinHandle<()>,
}

impl FlushThread {
    /// Starts the thread, or reports that one could not be started.
    pub(in crate::user_db) fn start(inner: &Arc<Inner>) -> Option<Self> {
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
    pub(in crate::user_db) fn request(&self) {
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
    pub(in crate::user_db) fn pause(&self, timeout: Duration) -> bool {
        self.wait_until_idle(timeout, true)
    }

    /// Releases the pause and wakes the thread.
    pub(in crate::user_db) fn resume(&self) {
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
    pub(in crate::user_db) fn settle(&self, timeout: Duration) -> bool {
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
