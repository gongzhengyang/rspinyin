//! The UI thread's half of the event channel.
//!
//! Events travel the other way from commands and are classified the same way:
//! by what happens when the channel is full.
//!
//! * `Select` is never dropped. A click that vanishes is a click the user has to
//!   repeat, so the UI thread waits for room, and when the wait runs out it
//!   abandons the click *loudly* -- the counter behind `ui/select/timeout` and an
//!   error the caller may report -- rather than queueing it behind a host that is
//!   not draining.
//! * `Page` and `Dismiss` are ordered and collapse to the newest, exactly like
//!   `Show` and `Hide`: the last page turn is the one the user meant.
//! * `Hover` is a latest-wins slot with a throttle, because pointer motion is
//!   high-rate and only the newest hovered candidate matters.
//! * `Rendered` is a latency probe rather than business state, so it is
//!   latest-wins as well; the newest sample is the one a probe wants.
//!
//! The consumer side drains without waiting for a zero timeout and really waits
//! for a non-zero one, which is what keeps `poll_event(Duration::ZERO)` safe to
//! call from the host's own event loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ime_types::{UiError, UiEvent};

use super::ChannelConfig;
use super::queue::{CollapsingQueue, LatestSlot, RingQueue};

/// What one hover means for the slot and for the host's notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoverDecision {
    /// The pointer is on the candidate the host already knows about.
    Ignore,
    /// New information inside the throttle window: store it, but do not spend a
    /// wakeup on it.
    Store,
    /// New information outside the window: store it and wake the host.
    StoreAndNotify,
}

/// The last hovered index and when the host was last told about it.
#[derive(Debug, Default)]
struct HoverGate {
    /// `None` until the first hover is seen. The inner option is the hovered
    /// candidate, which is itself optional because the pointer can leave the
    /// grid without leaving the window.
    index: Option<Option<u16>>,
    notified: Option<Instant>,
}

impl HoverGate {
    /// Records a hovered index and reports what should happen to it.
    ///
    /// The throttle is a pure function of the instant it is given, so the
    /// behaviour is testable without sleeping and the caller owns the clock.
    fn observe(&mut self, index: Option<u16>, now: Instant, throttle: Duration) -> HoverDecision {
        if self.index == Some(index) {
            return HoverDecision::Ignore;
        }
        self.index = Some(index);
        if self
            .notified
            .is_some_and(|last| now.saturating_duration_since(last) < throttle)
        {
            return HoverDecision::Store;
        }
        self.notified = Some(now);
        HoverDecision::StoreAndNotify
    }
}

/// The events the UI thread posts back to the host thread.
///
/// # Concurrency
///
/// `Send` and `Sync`. The posting methods never block: they either store a value
/// or wait for the bounded budget of the class they belong to. [`UiEventQueue::poll`]
/// blocks for at most the timeout it is given, and does not block at all when
/// that timeout is zero.
#[derive(Debug)]
pub struct UiEventQueue {
    select: RingQueue<UiEvent>,
    ordered: CollapsingQueue<UiEvent>,
    hover: LatestSlot<UiEvent>,
    rendered: LatestSlot<UiEvent>,
    hover_gate: Mutex<HoverGate>,
    select_timeouts: AtomicU64,
    /// Guards the notification only. The queues have their own locks, and no
    /// lock is ever taken while another is held, so the ordering is fixed and
    /// there is nothing to deadlock on.
    signal: Mutex<()>,
    ready: Condvar,
    select_spin: Duration,
    hover_throttle: Duration,
}

impl UiEventQueue {
    /// Creates the event channel with the capacities and budgets from `config`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(config: &ChannelConfig) -> Self {
        Self {
            select: RingQueue::new(config.select_capacity),
            ordered: CollapsingQueue::new(config.page_capacity, config.page_spin),
            hover: LatestSlot::new(),
            rendered: LatestSlot::new(),
            hover_gate: Mutex::new(HoverGate::default()),
            select_timeouts: AtomicU64::new(0),
            signal: Mutex::new(()),
            ready: Condvar::new(),
            select_spin: config.select_spin,
            hover_throttle: config.hover_throttle,
        }
    }

    /// Posts a click, waiting for room.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::SelectTimeout`] when the queue stayed full for the
    /// whole select budget. The click is abandoned rather than queued, and the
    /// counter behind `ui/select/timeout` records it: a click delivered late
    /// would select against a frame the engine has already replaced, which is
    /// worse than a click the user has to repeat.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn post_select(&self, event: UiEvent) -> Result<(), UiError> {
        // The wait happens before the notification lock is taken: holding it
        // across a spin would stall the host's own drain for the whole budget.
        let outcome = self.select.push_within(event, self.select_spin);
        let guard = self.lock_signal();
        match outcome {
            Ok(()) => {
                self.ready.notify_all();
                drop(guard);
                Ok(())
            }
            Err(_) => {
                self.select_timeouts.fetch_add(1, Ordering::Relaxed);
                drop(guard);
                Err(UiError::SelectTimeout)
            }
        }
    }

    /// Posts an ordered event, collapsing to the newest when the queue is full.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn post_ordered(&self, event: UiEvent) {
        self.ordered.push(event);
        let guard = self.lock_signal();
        self.ready.notify_all();
        drop(guard);
    }

    /// Posts a hover, keeping only the newest index and notifying at most once
    /// per throttle window.
    ///
    /// `now` is passed in rather than read from the clock so that the throttle
    /// is a pure function of its inputs and can be tested without sleeping.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn post_hover(&self, event: UiEvent, now: Instant) {
        let index = match &event {
            UiEvent::Hover { index, .. } => *index,
            // Only a hover belongs here; anything else is classified by the
            // caller into the queue that matches its overflow rule.
            _ => return,
        };
        let decision = {
            let mut gate = self.lock_hover_gate();
            gate.observe(index, now, self.hover_throttle)
        };
        if decision == HoverDecision::Ignore {
            return;
        }
        // The slot is latest-wins, so a throttled notification cannot lose the
        // value: the host reads the slot whenever it next polls, and the
        // notification only decides how soon that is.
        self.hover.put(event);
        if decision == HoverDecision::StoreAndNotify {
            let guard = self.lock_signal();
            self.ready.notify_all();
            drop(guard);
        }
    }

    /// Posts a render receipt.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn post_rendered(&self, event: UiEvent) {
        self.rendered.put(event);
        let guard = self.lock_signal();
        self.ready.notify_all();
        drop(guard);
    }

    /// Waits up to `timeout` for the next event.
    ///
    /// A zero timeout drains without waiting, which is the form the host's own
    /// event loop must use: an input method never blocks the thread that is
    /// handling keys. A non-zero timeout is for a caller that owns a thread of
    /// its own and has nothing else to do.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn poll(&self, timeout: Duration) -> Option<UiEvent> {
        let mut guard = self.lock_signal();
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(event) = self.try_pop() {
                return Some(event);
            }
            let remaining = match deadline {
                None => return None,
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            };
            if remaining.is_zero() {
                return None;
            }
            // Re-checking the queues while holding the notification lock is what
            // makes a concurrent post impossible to miss: the poster takes the
            // same lock before it notifies.
            let (next, _) = self
                .ready
                .wait_timeout(guard, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            guard = next;
        }
    }

    /// How many clicks were abandoned because the queue stayed full.
    ///
    /// This is the probe counter behind `ui/select/timeout`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn select_timeouts(&self) -> u64 {
        self.select_timeouts.load(Ordering::Relaxed)
    }

    /// How many ordered events had to be collapsed.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn ordered_collapsed(&self) -> u64 {
        self.ordered.collapsed()
    }

    /// Takes the next event, preferring the ones the user is waiting on.
    ///
    /// A click is the most specific thing the user can do, so it goes first; the
    /// ordered control events follow, and the throttled hover and probe samples
    /// come last because the newest of each is all that matters.
    fn try_pop(&self) -> Option<UiEvent> {
        self.select
            .pop()
            .or_else(|| self.ordered.pop())
            .or_else(|| self.hover.take())
            .or_else(|| self.rendered.take())
    }

    /// Locks the notification mutex, recovering it if a poster panicked while
    /// holding it; the mutex guards nothing but the notification, so there is no
    /// state a panic could have left inconsistent.
    fn lock_signal(&self) -> MutexGuard<'_, ()> {
        self.signal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the hover gate, recovering it if a poster panicked while holding it.
    fn lock_hover_gate(&self) -> MutexGuard<'_, HoverGate> {
        self.hover_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use ime_types::{PageDir, SelectTrigger};

    use super::*;

    fn queue() -> UiEventQueue {
        UiEventQueue::new(&ChannelConfig::default())
    }

    fn select(index: u16) -> UiEvent {
        UiEvent::Select {
            revision: 1,
            index,
            trigger: SelectTrigger::Mouse,
        }
    }

    fn hover(index: Option<u16>) -> UiEvent {
        UiEvent::Hover {
            revision: 1,
            index,
        }
    }

    fn page() -> UiEvent {
        UiEvent::Page {
            revision: 1,
            dir: PageDir::Next,
        }
    }

    #[test]
    fn test_event_queue_poll_with_zero_timeout_never_blocks() {
        let queue = queue();
        let start = Instant::now();
        assert_eq!(queue.poll(Duration::ZERO), None);
        // An empty queue with a zero timeout returns at once; the generous bound
        // only guards against an accidental wait on a loaded machine.
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn test_event_queue_select_is_never_dropped_while_the_host_drains() {
        let queue = Arc::new(queue());
        let draining = Arc::clone(&queue);
        let host = thread::spawn(move || {
            let mut seen = Vec::new();
            while seen.len() < 256 {
                if let Some(event) = draining.poll(Duration::from_millis(500)) {
                    seen.push(event);
                }
            }
            seen
        });
        for index in 0..256u16 {
            queue
                .post_select(select(index))
                .expect("the host is draining, so nothing is abandoned");
        }
        let seen = host.join().expect("the drain thread finishes");
        let expected: Vec<u16> = (0..256).collect();
        let observed: Vec<u16> = seen
            .iter()
            .map(|event| match event {
                UiEvent::Select { index, .. } => *index,
                other => panic!("only clicks were posted, got {other:?}"),
            })
            .collect();
        assert_eq!(observed, expected, "every click arrives, in order");
        assert_eq!(queue.select_timeouts(), 0);
    }

    #[test]
    fn test_event_queue_select_reports_timeout_when_nobody_drains() {
        let mut config = ChannelConfig::default();
        // A budget of zero turns the wait into a single attempt, which is what
        // makes the overflow path testable without waiting half a millisecond.
        config.select_spin = Duration::ZERO;
        let queue = UiEventQueue::new(&config);
        for index in 0..config.select_capacity {
            queue
                .post_select(select(index as u16))
                .expect("the queue has room");
        }
        let outcome = queue.post_select(select(999));
        assert_eq!(outcome, Err(UiError::SelectTimeout));
        assert_eq!(queue.select_timeouts(), 1);
        // The refused click was not queued behind the others.
        let drained = queue.poll(Duration::ZERO);
        assert_eq!(
            drained,
            Some(select(0)),
            "the queue still starts at its oldest entry"
        );
    }

    #[test]
    fn test_event_queue_hover_ignores_an_unchanged_index() {
        let queue = queue();
        let start = Instant::now();
        queue.post_hover(hover(Some(3)), start);
        assert_eq!(queue.poll(Duration::ZERO), Some(hover(Some(3))));
        queue.post_hover(hover(Some(3)), start + Duration::from_millis(100));
        assert_eq!(
            queue.poll(Duration::ZERO),
            None,
            "a repeat of the same index says nothing new"
        );
    }

    #[test]
    fn test_event_queue_hover_keeps_the_newest_index_while_throttled() {
        let queue = queue();
        let start = Instant::now();
        queue.post_hover(hover(Some(1)), start);
        // Inside the throttle window the notification is suppressed, but the
        // value still lands in the latest-wins slot and supersedes the older one.
        queue.post_hover(hover(Some(2)), start + Duration::from_millis(1));
        assert_eq!(queue.poll(Duration::ZERO), Some(hover(Some(2))));
        assert_eq!(queue.poll(Duration::ZERO), None);
    }

    #[test]
    fn test_hover_gate_throttles_notifications_but_not_stores() {
        let throttle = Duration::from_millis(16);
        let start = Instant::now();
        let mut gate = HoverGate::default();
        assert_eq!(
            gate.observe(Some(1), start, throttle),
            HoverDecision::StoreAndNotify,
            "the first hover is always announced"
        );
        assert_eq!(
            gate.observe(Some(1), start + throttle, throttle),
            HoverDecision::Ignore,
            "the same candidate is not announced twice"
        );
        assert_eq!(
            gate.observe(Some(2), start + Duration::from_millis(1), throttle),
            HoverDecision::Store,
            "a new candidate inside the window is stored but not announced"
        );
        assert_eq!(
            gate.observe(Some(3), start + throttle, throttle),
            HoverDecision::StoreAndNotify,
            "the window has elapsed, so the host is told again"
        );
        assert_eq!(
            gate.observe(None, start + throttle, throttle),
            HoverDecision::Store,
            "leaving the grid is a change, but it lands inside the window"
        );
        assert_eq!(
            gate.observe(None, start + throttle * 2, throttle),
            HoverDecision::Ignore,
            "leaving the grid twice says nothing new"
        );
    }

    #[test]
    fn test_event_queue_ordered_events_collapse_to_the_newest_when_full() {
        let mut config = ChannelConfig::default();
        config.page_capacity = 1;
        config.page_spin = Duration::ZERO;
        let queue = UiEventQueue::new(&config);
        queue.post_ordered(page());
        queue.post_ordered(UiEvent::Page {
            revision: 2,
            dir: PageDir::Prev,
        });
        let newest = queue.poll(Duration::ZERO);
        assert_eq!(newest, Some(page()), "the oldest queued event comes first");
        assert_eq!(
            queue.poll(Duration::ZERO),
            Some(UiEvent::Page {
                revision: 2,
                dir: PageDir::Prev,
            }),
            "the collapsed event is delivered rather than lost"
        );
        assert_eq!(queue.ordered_collapsed(), 1);
    }

    #[test]
    fn test_event_queue_prefers_clicks_over_hover_and_probes() {
        let queue = queue();
        let now = Instant::now();
        queue.post_hover(hover(Some(4)), now);
        queue.post_ordered(page());
        queue.post_select(select(2));
        assert_eq!(queue.poll(Duration::ZERO), Some(select(2)));
        assert_eq!(queue.poll(Duration::ZERO), Some(page()));
        assert_eq!(queue.poll(Duration::ZERO), Some(hover(Some(4))));
        assert_eq!(queue.poll(Duration::ZERO), None);
    }

    #[test]
    fn test_event_queue_rendered_receipts_are_latest_wins() {
        let queue = queue();
        let receipt = |nanos: u64| UiEvent::Rendered {
            revision: 1,
            raster: Duration::from_micros(200),
            presented_at_unix_nanos: nanos,
        };
        queue.post_rendered(receipt(1));
        queue.post_rendered(receipt(2));
        assert_eq!(queue.poll(Duration::ZERO), Some(receipt(2)));
        assert_eq!(queue.poll(Duration::ZERO), None);
    }

    #[test]
    fn test_event_queue_poll_wakes_on_a_concurrent_post() {
        let queue = Arc::new(queue());
        let posting = Arc::clone(&queue);
        let start = Instant::now();
        let host = thread::spawn(move || queue.poll(Duration::from_millis(500)));
        // Give the host a moment to start waiting, then post; a missed wakeup
        // would make this take the whole timeout.
        thread::sleep(Duration::from_millis(20));
        posting.post_select(select(1));
        let received = host.join().expect("the host thread finishes");
        assert_eq!(received, Some(select(1)));
        assert!(start.elapsed() < Duration::from_millis(400));
    }
}
