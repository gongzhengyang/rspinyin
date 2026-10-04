//! The candidate window's return channel: the thread that drains its events to the
//! engine.
//!
//! The surface collects clicks, hovers, page turns and dismissals into the UI
//! thread's event queue, and nothing else consumes that queue: before this module
//! existed the events piled up until it overflowed. The drain thread is the
//! consumer. It waits on the queue's own condition variable — a queue that never
//! posts costs it zero wakeups, so an idle candidate window adds no spinning thread
//! (`BUDGET-CPU-01`) — and hands every event to the engine addon through the outlet
//! in [`crate::ffi::transport`].
//!
//! # Thread placement
//!
//! The engine's session layer runs on Fcitx5's main loop and only there (`ASM-11` /
//! `ASM-12`), so this thread never calls it. [`run_drain`] hands each event to the
//! caller-supplied `post`, whose production body marshals the wire into the main
//! loop through the glue and answers whether the handoff was accepted. A refused
//! handoff — no outlet, or the outlet's queue full — is dropped and counted under
//! [`crate::ffi::transport::ENGINE_GONE_CODE`]; the drain keeps running, because the
//! window it serves does.
//!
//! # Lifecycle
//!
//! The queue appears with the UI thread, so [`start`] runs at the end of the
//! `ui-startup` step and [`stop`] from [`crate::addon::on_addon_destroy`], before
//! the teardown takes the outlet away. A stop sets the flag and wakes the queue with
//! a render receipt — the one event that names no engine state, which is what makes
//! it a safe wake-up letter — so a thread parked in the queue's long wait answers
//! within the stop deadline instead of the wait's own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ime_types::{ImeError, UiEvent};
use ime_ui::channel::UiEventQueue;

use crate::ffi::transport::{
    ENGINE_GONE_CODE, EVENT_OUTLET_UNAVAILABLE_CODE, arm_event_outlet, post_event,
};

/// How long the drain waits on the queue before it re-reads the stop flag.
///
/// The deadline exists so that a stop that raced the wake-up letter cannot strand
/// the thread past the addon's shutdown; a queue that never posts costs the thread
/// nothing in between, because the wait is the queue's own condition-variable wait
/// and there is no polling timer here.
const DRAIN_DEADLINE: Duration = Duration::from_secs(3600);

/// The thread's name, next to `rspinyin-ui` in `ps -T`.
const DRAIN_THREAD_NAME: &str = "rspinyin-drain";

/// How long [`stop`] waits for the thread to answer.
const DRAIN_STOP_TIMEOUT: Duration = Duration::from_millis(200);

/// Recorded when the drain thread did not stop inside its shutdown deadline.
///
/// The thread is detached rather than waited for, the same discipline the UI
/// thread's own stop applies.
const DRAIN_STOP_TIMEOUT_CODE: &str = "ui/event/drain-stop-timeout";

/// One queued drain thread and the state stopping it needs.
pub(crate) struct EventDrain {
    queue: Arc<UiEventQueue>,
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl EventDrain {
    /// Starts the production drain on `queue`.
    pub(crate) fn spawn(queue: Arc<UiEventQueue>) -> Result<Self, ImeError> {
        Self::spawn_with(queue, post_to_engine, report_engine_gone)
    }

    /// Starts a drain whose handoff and drop reporting are supplied.
    ///
    /// The split exists for the tests, which drive the loop with mock functions and
    /// no glue at all; the production body passes [`post_to_engine`] and
    /// [`report_engine_gone`].
    pub(crate) fn spawn_with(
        queue: Arc<UiEventQueue>,
        post: impl FnMut(&UiEvent) -> bool + Send + 'static,
        report_drop: impl FnMut() + Send + 'static,
    ) -> Result<Self, ImeError> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_queue = Arc::clone(&queue);
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name(DRAIN_THREAD_NAME.to_owned())
            .spawn(move || {
                run_drain(&thread_queue, post, report_drop, &thread_stop);
            })
            .map_err(|_| ImeError::UiChannelClosed)?;
        Ok(Self {
            queue,
            stop,
            handle: Mutex::new(Some(handle)),
        })
    }

    /// Asks the thread to stop and waits up to `timeout` for it to do so.
    ///
    /// Returns whether it stopped inside the deadline. The wake-up letter is a
    /// render receipt: the one event the drain never hands on, so receiving it can
    /// never produce an engine call, and a thread parked in the queue's long wait
    /// answers it at once.
    pub(crate) fn stop(self, timeout: Duration) -> bool {
        self.stop.store(true, Ordering::Release);
        self.queue.post_rendered(stop_receipt());
        let handle = match self.handle.lock() {
            Ok(mut slot) => slot.take(),
            // A panic while the handle was locked cannot leave the `Option`
            // inconsistent, so the handle is still the one to join.
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(handle) = handle else {
            return true;
        };
        join_within(handle, timeout)
    }
}

impl Drop for EventDrain {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.queue.post_rendered(stop_receipt());
        // Detached, like `UiThread::drop`: a drain that will not stop must not hold
        // up the teardown, and the flag above ends its loop at the next wake.
        if let Ok(mut slot) = self.handle.lock() {
            let _ = slot.take();
        }
    }
}

/// The drain loop: one event per pass, from the queue to `post`.
///
/// `post` answers whether the handoff was accepted; a refusal is reported once per
/// refused event through `report_drop` and the loop keeps draining, because the
/// engine going away must not turn the window's queue into a leak.
pub(crate) fn run_drain(
    queue: &UiEventQueue,
    mut post: impl FnMut(&UiEvent) -> bool,
    mut report_drop: impl FnMut(),
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Acquire) {
        // The queue's own long wait: zero wakeups while nothing posts.
        let Some(event) = queue.poll(DRAIN_DEADLINE) else {
            continue;
        };
        // A render receipt is the latency probe — and the stop letter. It names no
        // engine state, so it is never handed on.
        if matches!(event, UiEvent::Rendered { .. }) {
            continue;
        }
        if !post(&event) {
            report_drop();
        }
    }
}

/// The render receipt that ends a parked wait.
///
/// Zeroes everywhere: nothing reads a receipt's content, and the drain's skip rule
/// is what makes the shape safe to reuse as a letter.
fn stop_receipt() -> UiEvent {
    UiEvent::Rendered {
        revision: 0,
        raster: Duration::ZERO,
        presented_at_unix_nanos: 0,
    }
}

/// Waits for the drain thread for at most `timeout`, detaching past it.
///
/// `JoinHandle::join` has no timeout and the addon's teardown must never be held up
/// by a thread that will not stop, so the join happens on a helper thread and the
/// caller waits on a channel instead. This repeats the UI thread's own bounded
/// join, which lives in `ime-ui` and cannot be reached from here.
fn join_within(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (finished, done) = mpsc::channel();
    let helper = thread::Builder::new()
        .name(String::from("rspinyin-drain-join"))
        .spawn(move || {
            let _ = handle.join();
            let _ = finished.send(());
        });
    // A helper that cannot be started leaves the handle detached rather than
    // joined: the thread itself was already asked to stop, and it will.
    let Ok(helper) = helper else {
        return false;
    };
    drop(helper);
    match done.recv_timeout(timeout) {
        Ok(()) => true,
        // A disconnected channel means the helper died mid-join, which is the same
        // "no answer inside the deadline" state a timeout is.
        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => false,
    }
}

/// Hands one event to the engine addon through the glue's outlet.
fn post_to_engine(event: &UiEvent) -> bool {
    // The events belong to the session the current frame came from, which is the
    // context the engine's ingest is addressed to; the engine refuses an unknown
    // one on its own side. The mirror accessor hands back a snapshot clone, and the
    // rates here are human-scale — a click, a page turn, a throttled hover — so the
    // copy is not a hot path.
    let ic = crate::ui_impl::panel_mirror().map_or(0, |mirror| mirror.ic);
    post_event(ic, event)
}

/// The production drop report: the stable, throttled code.
fn report_engine_gone() {
    crate::ffi::emit_diagnostic(ENGINE_GONE_CODE);
}

/// The running drain, or `None` before the start-up and after the stop.
static EVENT_DRAIN: Mutex<Option<EventDrain>> = Mutex::new(None);

/// Starts the drain thread for the window the start-up just built.
///
/// Called from the addon's `ui-startup` step, so the queue it drains exists. The
/// outlet is armed first: a thread whose every post would be refused is a thread
/// that only manufactures diagnostics, so a failed arm starts nothing and is
/// recorded once. A reload without a destroy stops the previous drain first, the
/// way the UI start-up itself does.
pub(crate) fn start() {
    stop();
    if !arm_event_outlet() {
        crate::ffi::emit_diagnostic(EVENT_OUTLET_UNAVAILABLE_CODE);
        return;
    }
    let Some(queue) = crate::addon::event_queue() else {
        return;
    };
    match EventDrain::spawn(queue) {
        Ok(drain) => *lock_drain() = Some(drain),
        Err(err) => crate::ffi::emit_diagnostic(&format!("lifecycle/event-drain: {err}")),
    }
}

/// Stops the drain thread, before the addon's teardown takes the outlet away.
pub(crate) fn stop() {
    if let Some(drain) = lock_drain().take() {
        if !drain.stop(DRAIN_STOP_TIMEOUT) {
            crate::ffi::emit_diagnostic(DRAIN_STOP_TIMEOUT_CODE);
        }
    }
}

/// Borrows the drain slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The slot's contents are independent of
/// whatever that was, and the alternative — never stopping a drain again — would
/// leak the thread for the rest of the process.
fn lock_drain() -> MutexGuard<'static, Option<EventDrain>> {
    match EVENT_DRAIN.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{self, TryRecvError};
    use std::time::Instant;

    use ime_types::{DismissReason, PageDir, SelectTrigger};
    use ime_ui::channel::ChannelConfig;

    use crate::ffi::transport::{RspinyinEventWire, event_wire};

    // The anchor kind, transcribed from the engine's reader like the mirror below;
    // the numbers are the ABI. The select kind belongs to the mirror, which is what
    // the round trip is read back through.
    const EVENT_KIND_SELECT: u32 = 0;
    const EVENT_KIND_ANCHOR: u32 = 4;

    use super::*;

    /// How long a test waits for the drain to work through what it was handed.
    ///
    /// Generous by design: the bound catches a hung loop, not a slow machine.
    const SETTLE: Duration = Duration::from_secs(5);

    /// How often a test samples while waiting for the drain to act.
    const SETTLE_POLL: Duration = Duration::from_millis(5);

    fn queue() -> Arc<UiEventQueue> {
        Arc::new(UiEventQueue::new(&ChannelConfig::default()))
    }

    fn select(index: u16) -> UiEvent {
        UiEvent::Select {
            revision: 7,
            index,
            trigger: SelectTrigger::Mouse,
        }
    }

    fn hover(index: Option<u16>) -> UiEvent {
        UiEvent::Hover {
            revision: 7,
            index,
        }
    }

    fn page() -> UiEvent {
        UiEvent::Page {
            revision: 7,
            dir: PageDir::Next,
        }
    }

    fn dismiss() -> UiEvent {
        UiEvent::Dismiss {
            revision: 7,
            reason: DismissReason::Escape,
        }
    }

    /// Waits until `done` answers true or the settle bound runs out.
    fn wait_for(done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            thread::sleep(SETTLE_POLL);
        }
        done()
    }

    #[test]
    fn test_event_wire_round_trips_every_event_shape() {
        // The engine's reader is the authority for these numbers; the mirror of it
        // below is what the round trip is taken through, because the two addons
        // must not share a crate.
        let events = [
            select(3),
            select(0),
            hover(Some(4)),
            hover(None),
            page(),
            UiEvent::Page {
                revision: 7,
                dir: PageDir::Prev,
            },
            dismiss(),
            UiEvent::Dismiss {
                revision: 7,
                reason: DismissReason::OutsideClick,
            },
            UiEvent::Dismiss {
                revision: 7,
                reason: DismissReason::ScrollUpEmpty,
            },
            UiEvent::Select {
                revision: 7,
                index: 1,
                trigger: SelectTrigger::Tab,
            },
        ];
        for event in events {
            let wire = event_wire(&event).expect("a deliverable event has a wire");
            let parsed = event_from_wire(&wire).expect("the wire parses");
            assert_eq!(parsed, event, "the wire carries {event:?} intact");
            let again = event_wire(&parsed).expect("the parsed event wires again");
            assert_eq!(again, wire, "the transcription is stable under re-encoding");
        }
    }

    #[test]
    fn test_event_wire_leaves_no_wire_for_a_rendered_receipt() {
        let receipt = UiEvent::Rendered {
            revision: 1,
            raster: Duration::from_micros(200),
            presented_at_unix_nanos: 9,
        };
        assert!(
            event_wire(&receipt).is_none(),
            "a probe receipt names no engine state, so it has no wire kind"
        );
    }

    #[test]
    fn test_engine_gone_code_keeps_the_registered_spelling() {
        // The drop count is matched on this code by diagnostics and tests on both
        // sides of the channel, so its spelling is part of the contract.
        assert_eq!(ENGINE_GONE_CODE, "ui/event/engine-gone");
        assert_eq!(
            EVENT_OUTLET_UNAVAILABLE_CODE,
            "ui/event/outlet-unavailable"
        );
    }

    #[test]
    fn test_event_drain_forwards_events_in_order_until_it_is_stopped() {
        let queue = queue();
        let (sent, received) = mpsc::channel::<UiEvent>();
        let drain = EventDrain::spawn_with(Arc::clone(&queue), move |event| {
            sent.send(event.clone()).is_ok()
        }, || {})
        .expect("the drain thread starts");

        queue.post_select(select(1)).expect("the queue has room");
        queue.post_ordered(page());
        queue.post_hover(hover(Some(2)), Instant::now());

        let mut seen = Vec::new();
        let deadline = Instant::now() + SETTLE;
        while seen.len() < 3 && Instant::now() < deadline {
            if let Ok(event) = received.recv_timeout(SETTLE_POLL) {
                seen.push(event);
            }
        }
        assert_eq!(
            seen,
            vec![select(1), page(), hover(Some(2))],
            "the queue's own delivery order: the click first, then the ordered \
             event, then the hover"
        );

        assert!(
            drain.stop(SETTLE),
            "a parked drain answers its wake-up letter inside the deadline"
        );
        // The thread is gone, so nothing posted after the stop is handed on.
        queue.post_select(select(9)).expect("the queue has room");
        assert_eq!(
            received.try_recv(),
            Err(TryRecvError::Empty),
            "a stopped drain forwards nothing"
        );
    }

    #[test]
    fn test_event_drain_counts_every_event_the_outlet_refuses() {
        let queue = queue();
        let drops = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&drops);
        let drain = EventDrain::spawn_with(
            Arc::clone(&queue),
            |_event| false,
            move || {
                counter.fetch_add(1, Ordering::Relaxed);
            },
        )
        .expect("the drain thread starts");

        queue.post_select(select(1)).expect("the queue has room");
        queue.post_select(select(2)).expect("the queue has room");

        assert!(
            wait_for(|| drops.load(Ordering::Relaxed) == 2),
            "each refused handoff is reported, got {}",
            drops.load(Ordering::Relaxed)
        );
        assert!(drain.stop(SETTLE), "the drain stops when asked");
        assert_eq!(
            drops.load(Ordering::Relaxed),
            2,
            "nothing is reported past the events that were refused"
        );
    }

    #[test]
    fn test_event_drain_never_hands_on_a_rendered_receipt() {
        let queue = queue();
        let (sent, received) = mpsc::channel::<UiEvent>();
        let drain = EventDrain::spawn_with(Arc::clone(&queue), move |event| {
            sent.send(event.clone()).is_ok()
        }, || {})
        .expect("the drain thread starts");

        queue.post_rendered(stop_receipt());
        assert!(
            drain.stop(SETTLE),
            "the drain consumes the receipt and answers the stop"
        );
        assert_eq!(
            received.try_recv(),
            Err(TryRecvError::Empty),
            "a receipt names no engine state, so nothing was handed on"
        );
    }

    #[test]
    fn test_event_drain_stop_wakes_a_thread_parked_in_the_queue_wait() {
        let queue = queue();
        let drain = EventDrain::spawn_with(queue, |_event| true, || {})
            .expect("the drain thread starts");
        // The thread is parked in the queue's long wait; a lost wake-up would run
        // out the stop deadline instead of answering the letter, which is what the
        // assertion bounds.
        assert!(
            drain.stop(Duration::from_secs(10)),
            "a parked drain stops inside a deadline far below its own"
        );
    }

    #[test]
    fn test_start_without_an_outlet_starts_no_thread() {
        // The pure-Rust build's stand-in never arms, and a test process has no
        // Fcitx5 instance for the real glue to reach either, so `start` records the
        // unavailable outlet and starts nothing on either configuration.
        start();
        assert!(
            lock_drain().is_none(),
            "an outlet that cannot be armed must not grow a drain thread"
        );
    }

    #[test]
    fn test_event_drain_stop_timeout_code_is_stable() {
        assert_eq!(DRAIN_STOP_TIMEOUT_CODE, "ui/event/drain-stop-timeout");
    }

    #[test]
    fn test_the_anchor_kind_is_not_a_window_event() {
        // The caret anchor rides the same channel under its own kind, but it is
        // engine state rather than a window event: the event reader must refuse it,
        // which is what keeps a caret report from being mistaken for a click.
        let mut wire = zeroed_wire();
        wire.kind = EVENT_KIND_ANCHOR;
        assert!(
            event_from_wire(&wire).is_none(),
            "an anchor wire is not a UiEvent"
        );
        let select_wire = event_wire(&UiEvent::Select {
            revision: 1,
            index: 0,
            trigger: SelectTrigger::Mouse,
        })
        .expect("a select has a wire");
        assert_ne!(
            select_wire.kind, EVENT_KIND_ANCHOR,
            "no window event encodes as the anchor kind"
        );
    }

    /// An all-zero wire, field by field.
    ///
    /// `mem::zeroed` would need an `unsafe` block, and this crate carries none outside
    /// its `ffi` tree; the fields are plain numbers, so the explicit form says the same
    /// thing the zeroed form would.
    fn zeroed_wire() -> RspinyinEventWire {
        RspinyinEventWire {
            kind: 0,
            revision: 0,
            index: 0,
            reason: 0,
            anchor_x: 0,
            anchor_y: 0,
            anchor_w: 0,
            anchor_h: 0,
            anchor_screen: 0,
            anchor_scale: 0.0,
            anchor_placement: 0,
        }
    }

    /// The engine's wire reader, transcribed for the round-trip tests.
    ///
    /// `ime-ui-addon` must not depend on `ime-fcitx5`, so the reader's table is
    /// mirrored here instead of imported; the tests above fail when either side
    /// drifts, which is the same guarantee a shared test suite would give.
    fn event_from_wire(wire: &RspinyinEventWire) -> Option<UiEvent> {
        const EVENT_KIND_HOVER: u32 = 1;
        const EVENT_KIND_PAGE: u32 = 2;
        const EVENT_KIND_DISMISS: u32 = 3;
        const TRIGGER_MOUSE: u32 = 0;
        const TRIGGER_NUMBER_KEY: u32 = 1;
        const TRIGGER_SPACE: u32 = 2;
        const TRIGGER_ENTER: u32 = 3;
        const TRIGGER_TAB: u32 = 4;
        const HOVER_ABSENT: u32 = 0;
        const HOVER_PRESENT: u32 = 1;
        const PAGE_NEXT: u32 = 0;
        const PAGE_PREV: u32 = 1;
        const DISMISS_OUTSIDE_CLICK: u32 = 0;
        const DISMISS_ESCAPE: u32 = 1;
        const DISMISS_SCROLL_UP_EMPTY: u32 = 2;
        match wire.kind {
            EVENT_KIND_SELECT => Some(UiEvent::Select {
                revision: wire.revision,
                index: wire.index,
                trigger: match wire.reason {
                    TRIGGER_MOUSE => SelectTrigger::Mouse,
                    TRIGGER_NUMBER_KEY => SelectTrigger::NumberKey,
                    TRIGGER_SPACE => SelectTrigger::Space,
                    TRIGGER_ENTER => SelectTrigger::Enter,
                    TRIGGER_TAB => SelectTrigger::Tab,
                    _ => return None,
                },
            }),
            EVENT_KIND_HOVER => Some(UiEvent::Hover {
                revision: wire.revision,
                index: match wire.reason {
                    HOVER_ABSENT => None,
                    HOVER_PRESENT => Some(wire.index),
                    _ => return None,
                },
            }),
            EVENT_KIND_PAGE => Some(UiEvent::Page {
                revision: wire.revision,
                dir: match wire.reason {
                    PAGE_NEXT => PageDir::Next,
                    PAGE_PREV => PageDir::Prev,
                    _ => return None,
                },
            }),
            EVENT_KIND_DISMISS => Some(UiEvent::Dismiss {
                revision: wire.revision,
                reason: match wire.reason {
                    DISMISS_OUTSIDE_CLICK => DismissReason::OutsideClick,
                    DISMISS_ESCAPE => DismissReason::Escape,
                    DISMISS_SCROLL_UP_EMPTY => DismissReason::ScrollUpEmpty,
                    _ => return None,
                },
            }),
            _ => None,
        }
    }
}
