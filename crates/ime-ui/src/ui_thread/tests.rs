//! Unit tests for the UI thread's lifecycle and its delivery guarantees.
//!
//! Every test here drives the real thread with a recording surface, so the
//! assertions are about what the loop actually did rather than about what the
//! code says it does. No test needs a display server: the recording surface
//! reports no descriptor of its own, and the loop is perfectly happy waiting on
//! the wakeup counter alone.

use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ime_types::{
    Anchor, HideReason, ImeError, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId,
    StatusStrip, UiCommand, UiError, UiFrame,
};

use crate::channel::UiEventQueue;

use super::{SurfaceUpdate, UiContext, UiSurface, UiThread, UiThreadConfig};

/// How long a test waits for the UI thread before it gives up.
///
/// Generous on purpose: the assertions are about ordering and counting, not
/// about how fast a loaded machine is, and a shorter bound would only make the
/// suite flaky.
const PATIENCE: Duration = Duration::from_secs(5);

/// One thing the loop did to the surface.
#[derive(Clone, Debug, PartialEq)]
enum Observed {
    /// A frame was applied, carrying its revision.
    Frame(u32),
    /// A theme was applied.
    Theme,
    /// The window was told to appear, carrying the revision.
    Show(u32),
    /// The window was told to disappear, carrying the revision.
    Hide(u32),
    /// The surface was closed.
    Close,
}

/// The surface's record of what it was told, shared with the test thread.
#[derive(Debug, Default)]
struct Recorder {
    observed: Mutex<Vec<Observed>>,
    ready: Condvar,
    renders: AtomicUsize,
    /// How long one render takes, used to make the loop too slow to keep up with
    /// a burst.
    render_delay: Duration,
    /// Whether rendering panics, used to check the thread's failure isolation.
    panics: bool,
    /// Held shut until the test opens it, or `None` for a recorder that never waits.
    ///
    /// The first render blocks here when it is set. It exists so the burst test does not
    /// have to guess how long the producer needs: a fixed render delay only keeps the loop
    /// away for as long as the machine happens to cooperate, and under `cargo nextest`'s
    /// parallel load 64 posts outlasted the 200ms window, so the collapse the test asserts
    /// had not happened yet. That is the signature of a wall-clock guess rather than a
    /// fact, and a gate makes the ordering a fact.
    gate: Option<Arc<(Mutex<bool>, Condvar)>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn with(render_delay: Duration, panics: bool) -> Arc<Self> {
        Arc::new(Self {
            render_delay,
            panics,
            ..Self::default()
        })
    }

    /// A recorder whose loop stalls in its first render until [`Recorder::open_gate`].
    fn gated() -> Arc<Self> {
        Arc::new(Self {
            gate: Some(Arc::new((Mutex::new(false), Condvar::new()))),
            ..Self::default()
        })
    }

    /// Releases the loop held by [`Recorder::gated`].
    fn open_gate(&self) {
        if let Some((open, ready)) = self.gate.as_deref() {
            *open
                .lock()
                .expect("the gate lock is never poisoned by a test") = true;
            ready.notify_all();
        }
    }

    fn record(&self, item: Observed) {
        let mut observed = self
            .observed
            .lock()
            .expect("the recorder lock is never poisoned by a test");
        observed.push(item);
        drop(observed);
        self.ready.notify_all();
    }

    /// Waits until at least `count` items have been recorded and returns them.
    fn wait_for(&self, count: usize) -> Vec<Observed> {
        let deadline = Instant::now() + PATIENCE;
        let mut observed = self
            .observed
            .lock()
            .expect("the recorder lock is never poisoned by a test");
        while observed.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, _) = self
                .ready
                .wait_timeout(observed, remaining)
                .expect("the recorder lock is never poisoned by a test");
            observed = next;
        }
        observed.clone()
    }

    fn rendered(&self) -> usize {
        self.renders.load(Ordering::Relaxed)
    }

    fn frames(&self) -> usize {
        self.observed
            .lock()
            .expect("the recorder lock is never poisoned by a test")
            .iter()
            .filter(|item| matches!(item, Observed::Frame(_)))
            .count()
    }
}

/// A surface that records what the loop does to it and needs no display.
struct RecordingSurface {
    recorder: Arc<Recorder>,
}

impl UiSurface for RecordingSurface {
    fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        // No connection of its own: the loop then waits only on the wakeup
        // counter, which is what makes this testable without a display server.
        None
    }

    fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError> {
        let observed = match update {
            SurfaceUpdate::Frame(frame) => Observed::Frame(frame.revision),
            SurfaceUpdate::Theme(_) => Observed::Theme,
            SurfaceUpdate::Show { revision, .. } => Observed::Show(revision),
            SurfaceUpdate::Hide { revision, .. } => Observed::Hide(revision),
        };
        self.recorder.record(observed);
        Ok(())
    }

    fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
        Ok(())
    }

    fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
        self.recorder.renders.fetch_add(1, Ordering::Relaxed);
        if self.recorder.panics {
            panic!("the recording surface was asked to panic");
        }
        if let Some((open, ready)) = self.recorder.gate.as_deref() {
            let mut open = open
                .lock()
                .expect("the gate lock is never poisoned by a test");
            while !*open {
                // Bounded by `PATIENCE` so a test that forgets to open the gate reports a
                // failure instead of hanging the suite.
                let (next, _) = ready
                    .wait_timeout(open, PATIENCE)
                    .expect("the gate lock is never poisoned by a test");
                open = next;
            }
        }
        if !self.recorder.render_delay.is_zero() {
            thread::sleep(self.recorder.render_delay);
        }
        // Never animating: an idle loop is exactly what the tests below check.
        Ok(None)
    }

    fn close(&mut self) -> Result<(), ImeError> {
        self.recorder.record(Observed::Close);
        Ok(())
    }
}

fn spawn(recorder: &Arc<Recorder>) -> UiThread {
    let recorder = Arc::clone(recorder);
    UiThread::spawn(UiThreadConfig::default(), move |_context: UiContext| {
        Ok(Box::new(RecordingSurface { recorder }) as Box<dyn UiSurface>)
    })
    .expect("the UI thread starts")
}

fn anchor() -> Anchor {
    Anchor {
        cursor: RectI {
            x: 10,
            y: 20,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Below,
    }
}

fn frame(revision: u32) -> Box<UiFrame> {
    Box::new(UiFrame {
        revision,
        preedit: Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: Vec::new(),
        },
        candidates: Vec::new(),
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: anchor(),
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
    })
}

/// Waits for the UI thread to exit, returning whether it did.
fn wait_until_dead(thread: &UiThread) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if !thread.is_alive() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

/// Waits until the render count stops growing and returns it.
fn settle(recorder: &Recorder) -> usize {
    let deadline = Instant::now() + PATIENCE;
    let mut last = recorder.rendered();
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
        let now = recorder.rendered();
        if now == last {
            return now;
        }
        last = now;
    }
    last
}

#[test]
fn test_ui_thread_applies_a_posted_frame() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    thread
        .send(UiCommand::Frame(frame(7)))
        .expect("the thread is alive");
    assert_eq!(recorder.wait_for(1), vec![Observed::Frame(7)]);
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_observes_show_hide_show_in_order() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    let sequence = [
        UiCommand::Show {
            revision: 1,
            anchor: anchor(),
        },
        UiCommand::Hide {
            revision: 2,
            reason: HideReason::Cancelled,
        },
        UiCommand::Show {
            revision: 3,
            anchor: anchor(),
        },
    ];
    for command in sequence {
        thread.send(command).expect("the thread is alive");
    }
    assert_eq!(
        recorder.wait_for(3),
        vec![Observed::Show(1), Observed::Hide(2), Observed::Show(3)],
        "the ordered channel delivers visibility changes in order"
    );
    assert_eq!(
        thread.stats().controls_collapsed,
        0,
        "nothing was collapsed, so nothing was reordered"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_collapses_a_frame_burst() {
    let recorder = Recorder::with(Duration::from_millis(10), false);
    let thread = spawn(&recorder);
    for revision in 0..10_000u32 {
        thread
            .send(UiCommand::Frame(frame(revision)))
            .expect("the thread is alive");
    }
    let stats = thread.stats();
    let coalesced = stats.frames_coalesced;
    assert!(
        coalesced >= 9_900,
        "almost every frame should have been coalesced, got {coalesced}"
    );
    let applied = recorder.frames();
    assert!(
        applied <= 100,
        "the loop must not have applied the whole burst, applied {applied}"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_collapses_a_show_hide_burst_to_the_newest() {
    // The loop is held in its first render until the whole burst is posted, so the queue is
    // certain to have filled and collapsed before anything drains. The previous version of
    // this test relied on a 200ms render keeping the loop away instead, which is a guess
    // about the machine rather than a fact about the code.
    let recorder = Recorder::gated();
    let thread = spawn(&recorder);
    for revision in 0..64u32 {
        let command = if revision % 2 == 0 {
            UiCommand::Show {
                revision,
                anchor: anchor(),
            }
        } else {
            UiCommand::Hide {
                revision,
                reason: HideReason::Cancelled,
            }
        };
        thread.send(command).expect("the thread is alive");
    }
    assert!(
        thread.stats().controls_collapsed >= 50,
        "the ordered queue is eight deep, so most of the burst had to collapse"
    );
    recorder.open_gate();
    // Eight commands fit in the queue and the newest one is staged behind them,
    // so the burst arrives as nine applications and the last one is the newest.
    let observed = recorder.wait_for(9);
    assert_eq!(observed.len(), 9, "the queue drained as expected");
    assert_eq!(
        observed.last(),
        Some(&Observed::Hide(63)),
        "the newest visibility change is the one that survives"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_send_does_not_wait_for_the_loop() {
    // The loop is inside a long render, so nothing it does can speed the posts
    // up: the host must be able to post while the renderer is busy.
    let recorder = Recorder::with(Duration::from_millis(200), false);
    let thread = spawn(&recorder);
    let start = Instant::now();
    for revision in 0..1_000u32 {
        thread
            .send(UiCommand::Frame(frame(revision)))
            .expect("the thread is alive");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(100),
        "posting a thousand frames took {elapsed:?}, which means it waited"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_idle_surface_is_not_rendered_again() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    thread
        .send(UiCommand::Frame(frame(1)))
        .expect("the thread is alive");
    assert_eq!(recorder.wait_for(1), vec![Observed::Frame(1)]);
    let settled = settle(&recorder);
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        recorder.rendered(),
        settled,
        "an idle loop must neither redraw nor wake on a timer"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_shutdown_is_idempotent_and_stops_the_thread() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    thread.shutdown(PATIENCE).expect("the thread stops");
    assert!(
        !thread.is_alive(),
        "the thread is gone once shutdown returns"
    );
    thread
        .shutdown(PATIENCE)
        .expect("a second shutdown is a no-op");
    assert_eq!(thread.stats().shutdown_timeouts, 0);
    assert!(
        recorder.wait_for(1).contains(&Observed::Close),
        "the surface was closed before the thread exited"
    );
}

#[test]
fn test_ui_thread_shutdown_command_stops_the_loop() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    thread
        .send(UiCommand::Shutdown)
        .expect("the thread is alive");
    assert!(
        wait_until_dead(&thread),
        "a shutdown command stops the loop without a second request"
    );
    assert!(recorder.wait_for(1).contains(&Observed::Close));
}

#[test]
fn test_ui_thread_after_a_panic_discards_commands() {
    // The surface panics on its first render, so the loop never even starts.
    let recorder = Recorder::with(Duration::ZERO, true);
    let thread = spawn(&recorder);
    assert!(wait_until_dead(&thread), "the panic ends the UI thread");
    assert_eq!(
        thread.send(UiCommand::Frame(frame(1))),
        Err(UiError::ThreadDead),
        "a dead thread refuses commands instead of queueing them"
    );
    let stats = thread.stats();
    assert!(stats.thread_dead);
    assert!(
        stats.thread_panicked,
        "the panic was recorded, not swallowed"
    );
}

#[test]
fn test_ui_thread_stats_start_clean() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    let stats = thread.stats();
    assert_eq!(stats.frames_coalesced, 0);
    assert_eq!(stats.themes_coalesced, 0);
    assert_eq!(stats.controls_collapsed, 0);
    assert_eq!(stats.select_timeouts, 0);
    assert!(!stats.thread_dead);
    assert!(!stats.thread_panicked);
    assert_eq!(stats.shutdown_timeouts, 0);
    thread.shutdown(PATIENCE).expect("the thread stops");
}

#[test]
fn test_ui_thread_poll_event_with_a_zero_timeout_never_blocks() {
    let recorder = Recorder::new();
    let thread = spawn(&recorder);
    let start = Instant::now();
    assert_eq!(thread.poll_event(Duration::ZERO), None);
    assert!(
        start.elapsed() < Duration::from_millis(50),
        "the host's own loop must be able to poll without waiting"
    );
    thread.shutdown(PATIENCE).expect("the thread stops");
}
