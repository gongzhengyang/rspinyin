//! The queue shapes the boundary contract fixes.
//!
//! Two shapes cover every channel. A [`LatestSlot`] holds one value and keeps
//! the newest, which is what makes a `Frame` or a `Theme` collapse: a frame is a
//! complete snapshot rather than a delta, so overwriting an unread one loses no
//! information. A [`RingQueue`] is the bounded FIFO that keeps `Show` and `Hide`
//! in order, and [`CollapsingQueue`] adds the sender-side staging that the
//! contract's "spin, then collapse to the newest" overflow rule needs.
//!
//! The shapes are not interchangeable, and the contract is explicit about which
//! channel gets which: collapsing a `Show`/`Hide` pair would leave the candidate
//! window in the wrong visibility state, while refusing a `Frame` would leave it
//! drawing stale candidates.
//!
//! # Why a mutex and not a lock-free ring
//!
//! A hand-rolled lock-free ring would need raw pointers, which this crate's lint
//! policy denies, or an exercise in atomic subtlety that the standard library
//! has already solved. Both queues are therefore built on `std::sync::Mutex`,
//! which is sound here because the lock is never held across a blocking call:
//! the critical sections are a `VecDeque` push or pop and nothing else. Neither
//! queue allocates after construction -- the deque is reserved to its capacity
//! up front -- so a post allocates only the value the caller brought.

use std::collections::VecDeque;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::yield_now;
use std::time::{Duration, Instant};

/// How many failed attempts pass between two `yield_now` calls.
///
/// Spinning alone would starve the consumer on a single-core machine and
/// yielding alone would spend the whole budget in syscalls; alternating keeps
/// the common multi-core wait short and still lets the other thread run.
const YIELD_EVERY: u32 = 32;

/// A single slot that keeps the newest value and counts what it replaced.
///
/// # Concurrency
///
/// `Send` and `Sync` whenever `T` is `Send`. Both methods are lock-based and
/// return within themselves; neither blocks on another thread.
#[derive(Debug)]
pub struct LatestSlot<T> {
    slot: Mutex<Option<T>>,
    coalesced: AtomicU64,
}

impl<T> LatestSlot<T> {
    /// Creates an empty slot.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            coalesced: AtomicU64::new(0),
        }
    }

    /// Stores `value`, replacing and dropping whatever was there.
    ///
    /// Returns `true` when an older value was replaced, which is the signal that
    /// the write was a coalescing rather than a plain store.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn put(&self, value: T) -> bool {
        let replaced = self.lock().replace(value).is_some();
        if replaced {
            self.coalesced.fetch_add(1, Ordering::Relaxed);
        }
        replaced
    }

    /// Removes and returns the stored value, leaving the slot empty.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take(&self) -> Option<T> {
        self.lock().take()
    }

    /// Whether a value is waiting.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn is_empty(&self) -> bool {
        self.lock().is_none()
    }

    /// How many values were replaced before being read.
    ///
    /// This is the probe counter behind `ui.frame.coalesced`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn coalesced(&self) -> u64 {
        self.coalesced.load(Ordering::Relaxed)
    }

    /// Locks the slot, recovering the value if a thread panicked while holding
    /// the lock: an `Option` is never left inconsistent, so refusing to hand it
    /// back would turn one thread's panic into every thread's failure.
    fn lock(&self) -> MutexGuard<'_, Option<T>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A bounded FIFO that never allocates once it is full.
///
/// # Concurrency
///
/// `Send` and `Sync` whenever `T` is `Send`. [`RingQueue::try_push`] and
/// [`RingQueue::pop`] never wait; [`RingQueue::push_within`] waits for room but
/// only for the budget it is given, and the lock itself is released between
/// attempts so the consumer can always make progress.
#[derive(Debug)]
pub struct RingQueue<T> {
    inner: Mutex<VecDeque<T>>,
    capacity: usize,
    rejected: AtomicU64,
}

impl<T> RingQueue<T> {
    /// Creates a queue holding at most `capacity` values.
    ///
    /// A capacity of zero is raised to one: a queue that can never hold
    /// anything would silently drop every value it is given, and the callers
    /// that pass a capacity from configuration must not be able to turn the
    /// control channel into a no-op.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            // Reserved up front so that a push within the capacity never
            // allocates: the post path runs on the host thread, where an
            // allocator call is latency the user can feel.
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            rejected: AtomicU64::new(0),
        }
    }

    /// The largest number of values the queue holds.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many values are waiting.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the queue holds no values.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Enqueues `value`, or hands it straight back when the queue is full.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let mut queue = self.lock();
        if queue.len() >= self.capacity {
            return Err(value);
        }
        queue.push_back(value);
        Ok(())
    }

    /// Enqueues `value`, waiting up to `budget` for room.
    ///
    /// # Errors
    ///
    /// Returns the value back when the queue is still full once `budget` has
    /// elapsed, and counts the refusal in [`RingQueue::rejected`]. A zero budget
    /// makes this exactly one attempt.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn push_within(&self, mut value: T, budget: Duration) -> Result<(), T> {
        let start = Instant::now();
        let mut spins: u32 = 0;
        loop {
            match self.try_push(value) {
                Ok(()) => return Ok(()),
                Err(returned) => {
                    if Instant::now().saturating_duration_since(start) >= budget {
                        self.rejected.fetch_add(1, Ordering::Relaxed);
                        return Err(returned);
                    }
                    value = returned;
                }
            }
            spins += 1;
            if spins >= YIELD_EVERY {
                spins = 0;
                yield_now();
            } else {
                spin_loop();
            }
        }
    }

    /// Removes and returns the oldest value.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn pop(&self) -> Option<T> {
        self.lock().pop_front()
    }

    /// How many pushes gave up because the queue stayed full.
    ///
    /// This is the raw count behind the probe of whichever channel owns the
    /// queue; a channel with a policy of its own, such as the never-dropped
    /// click queue, publishes its own counter on top of it.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// Locks the queue, recovering the values if a thread panicked while holding
    /// the lock; a `VecDeque` is never left inconsistent by a panic in the
    /// caller, so the queue stays usable.
    fn lock(&self) -> MutexGuard<'_, VecDeque<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A bounded FIFO that collapses to the newest value instead of refusing it.
///
/// This is the overflow behaviour the contract fixes for the ordered control
/// channels: the sender spins for its budget, and when the queue is still full
/// it keeps the newest value in a staging slot rather than losing it. The
/// staging slot is flushed ahead of anything newer on the next push, so the
/// order the consumer observes is always the order of the values that survived
/// -- dropping the older ones is what keeps a `Show`/`Hide` pair meaningful,
/// because collapsing a pair would leave the window in the wrong state.
///
/// # Concurrency
///
/// `Send` and `Sync` whenever `T` is `Send`. [`CollapsingQueue::push`] may wait
/// for the queue's budget, and the lock on the staging slot is held across that
/// wait; the queue's own lock is not, so the consumer is never blocked by a
/// sender that is spinning.
#[derive(Debug)]
pub struct CollapsingQueue<T> {
    queue: RingQueue<T>,
    staged: Mutex<Option<T>>,
    collapsed: AtomicU64,
    budget: Duration,
}

impl<T> CollapsingQueue<T> {
    /// Creates a queue holding at most `capacity` values that waits `budget` for
    /// room before it starts collapsing.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(capacity: usize, budget: Duration) -> Self {
        Self {
            queue: RingQueue::new(capacity),
            staged: Mutex::new(None),
            collapsed: AtomicU64::new(0),
            budget,
        }
    }

    /// Enqueues `value`, or stages it as the newest value to send next.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn push(&self, value: T) {
        let mut staged = self.lock();
        if let Some(previous) = staged.take() {
            if self.queue.push_within(previous, self.budget).is_err() {
                // Both the staged value and this one are stuck. The contract
                // keeps the newest, so the staged one is the value that
                // disappears and the one that gets counted.
                *staged = Some(value);
                self.collapsed.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        if let Err(value) = self.queue.push_within(value, self.budget) {
            // Staging is not a drop: the value is still on its way, and the
            // counter records that it had to take the slow path.
            *staged = Some(value);
            self.collapsed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Removes and returns the oldest value, including a staged one.
    ///
    /// A staged value is only handed to the queue by a later push, so a queue
    /// that is drained and then left alone would strand it; taking it once the
    /// queue itself is empty is what keeps it from being lost. Order is still
    /// preserved, because the staged value is by construction newer than
    /// everything that has already been consumed and older than everything that
    /// has not been staged yet.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn pop(&self) -> Option<T> {
        if let Some(value) = self.queue.pop() {
            return Some(value);
        }
        self.lock().take()
    }

    /// How many pushes had to be staged because the queue was full.
    ///
    /// This is the probe counter behind `ui.control.dropped`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn collapsed(&self) -> u64 {
        self.collapsed.load(Ordering::Relaxed)
    }

    /// How many values are waiting in the queue itself, excluding a staged one.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue itself holds no values, ignoring a staged one.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Locks the staging slot, recovering the value if a thread panicked while
    /// holding the lock.
    fn lock(&self) -> MutexGuard<'_, Option<T>> {
        self.staged.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latest_slot_put_replaces_the_older_value() {
        let slot: LatestSlot<u32> = LatestSlot::new();
        assert!(!slot.put(1), "the first store replaces nothing");
        assert!(slot.put(2), "the second store replaces the first");
        assert!(slot.put(3));
        assert_eq!(slot.coalesced(), 2);
        assert_eq!(slot.take(), Some(3), "the newest value survives");
    }

    #[test]
    fn test_latest_slot_take_empties_the_slot() {
        let slot: LatestSlot<u32> = LatestSlot::new();
        assert!(slot.is_empty());
        slot.put(7);
        assert!(!slot.is_empty());
        assert_eq!(slot.take(), Some(7));
        assert_eq!(slot.take(), None);
        assert!(slot.is_empty());
    }

    #[test]
    fn test_ring_queue_try_push_beyond_capacity_returns_the_value() {
        let queue: RingQueue<u32> = RingQueue::new(2);
        assert_eq!(queue.capacity(), 2);
        assert_eq!(queue.try_push(1), Ok(()));
        assert_eq!(queue.try_push(2), Ok(()));
        assert_eq!(queue.try_push(3), Err(3), "a full queue hands it back");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(1), "a FIFO keeps the oldest first");
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn test_ring_queue_zero_capacity_is_raised_to_one() {
        let queue: RingQueue<u32> = RingQueue::new(0);
        assert_eq!(queue.capacity(), 1);
        assert_eq!(queue.try_push(1), Ok(()));
        assert_eq!(queue.try_push(2), Err(2));
    }

    #[test]
    fn test_ring_queue_push_within_gives_up_and_counts_the_refusal() {
        let queue: RingQueue<u32> = RingQueue::new(1);
        assert_eq!(queue.try_push(1), Ok(()));
        let refused = queue.push_within(2, Duration::ZERO);
        assert_eq!(refused, Err(2), "a zero budget means a single attempt");
        assert_eq!(queue.rejected(), 1);
        // Room again: the wait path succeeds and counts nothing.
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.push_within(3, Duration::from_millis(1)), Ok(()));
        assert_eq!(queue.rejected(), 1);
        assert_eq!(queue.pop(), Some(3));
    }

    #[test]
    fn test_collapsing_queue_keeps_order_while_there_is_room() {
        let queue: CollapsingQueue<u32> = CollapsingQueue::new(4, Duration::ZERO);
        queue.push(1);
        queue.push(2);
        queue.push(3);
        assert_eq!(queue.collapsed(), 0);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn test_collapsing_queue_collapses_to_the_newest_when_full() {
        let queue: CollapsingQueue<u32> = CollapsingQueue::new(1, Duration::ZERO);
        queue.push(1);
        queue.push(2);
        assert_eq!(queue.collapsed(), 1, "the second push had to be staged");
        // 3 arrives while 2 is staged; the staged value is older, so 2 is the
        // one that disappears and the counter records the replacement.
        queue.push(3);
        assert_eq!(queue.collapsed(), 2);
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(3), "the newest value is the one kept");
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn test_collapsing_queue_delivers_a_staged_value_without_a_further_push() {
        let queue: CollapsingQueue<u32> = CollapsingQueue::new(1, Duration::ZERO);
        queue.push(1);
        queue.push(2);
        assert_eq!(queue.pop(), Some(1));
        // Nothing else is pushed, so only draining can save the staged value.
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn test_collapsing_queue_keeps_order_across_a_full_and_drained_cycle() {
        let queue: CollapsingQueue<u32> = CollapsingQueue::new(1, Duration::ZERO);
        queue.push(1);
        queue.push(2);
        queue.push(3);
        // The staged value is newer than everything already consumed, so it may
        // only appear after the queue itself is empty.
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(3));
        queue.push(4);
        queue.push(5);
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);
    }
}
