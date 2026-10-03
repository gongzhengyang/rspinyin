//! Criterion benchmark for the cross-thread wakeup, and the probe that measures it.
//!
//! The budget this case exists for is a wakeup rather than a frame: the host thread posts a
//! command and the UI thread has to leave `poll(2)` and see it, inside the percentile ceiling
//! the card states. Nothing in the loop can be timed from the outside, because the cost is the
//! scheduler's, so the measurement is taken at the two ends of one delivery:
//!
//! 1. the poster stamps `Instant::now()` and then posts the command, which is what writes the
//!    `eventfd` the loop is blocked on;
//! 2. the UI thread, in the first call the loop makes after it wakes (`UiSurface::apply`),
//!    reads the same clock and records the difference.
//!
//! One delivery per iteration, and the poster waits for the observation before it posts the
//! next one. That is what makes each sample a wakeup: the frame channel is latest-wins, so a
//! burst of posts would be coalesced into a single wakeup and the samples would describe a
//! queue instead of a wakeup. The coalescing behaviour has its own test beside the loop.
//!
//! # Two numbers, and which one the gate reads
//!
//! [`LatencyHistogram`] holds the difference above -- the quantity the acceptance criterion
//! names -- with one sample per delivery, and the benchmark asserts its population: the ten
//! thousand deliveries the criterion states, one sample each, and a ninety-ninth percentile
//! inside the histogram's window.
//!
//! The criterion case `ui/wakeup_latency` reports a slightly larger number. It times the
//! whole round trip from the poster's side, which includes the `write(2)` into the counter,
//! the allocation of the frame and the poster's own wait. That is the conservative reading of
//! the same latency, and it is the one `xtask budget --check` asserts against the threshold.
//!
//! One caveat about that gate number, recorded here because the benchmark is where it is
//! visible: criterion samples a *block* of iterations at a time and reports the spread of the
//! block means, so `mean + 3 sigma` is an estimate of the ninety-ninth percentile of the
//! blocks, not of the deliveries. The per-delivery distribution is the histogram's, and its
//! percentile is the number the acceptance criterion describes. Nothing here can change that
//! -- the iteration count is criterion's to choose -- so the two are kept side by side and
//! neither is presented as the other.
//!
//! # The pointer case
//!
//! `ui/pointer_to_pixel` measures the same one-delivery-at-a-time wait with the surface's
//! *connection* descriptor as the waker -- the descriptor a display backend reports and
//! the loop adds to its poll set. The poster stamps, wakes that descriptor, and the sample
//! is taken in the first call the loop makes afterwards ([`UiSurface::drain_events`]),
//! which is where a real surface would consume the queued input. It is the latency the
//! surface's own descriptor exists to bound: a pointer event has to wake the loop
//! directly instead of riding along with the next host post. What it deliberately does
//! not time is the raster that follows the drain -- that is the render path the frame
//! budgets cover, and folding it in would time the renderer under the name of a wakeup.
//!
//! # No display, no environment, nothing time-dependent in the assertions
//!
//! The surface here is a recording surface with no connection of its own, so the loop waits
//! on the wakeup counter alone and the benchmark needs no display server, no `$HOME` and no
//! dictionary. The frame each delivery carries is empty and fixed: what is measured is the
//! wakeup, not the payload. What the assertions depend on is the *structure* of the run --
//! ten thousand deliveries, one sample each, every delivery observed, the percentile inside
//! the window -- and never on how fast the machine is.
//!
//! # The instrument is checked before it is used
//!
//! `harness = false` means libtest never drives this target, so a `#[cfg(test)]` module here
//! would never run and would be invisible to `cargo clippy --all-targets` as well. The
//! histogram's arithmetic is therefore checked by [`assert_instrument_is_sound`] on every
//! run, with synthetic samples and no clock.
//!
//! This file measures. Comparing the measurement against the threshold is
//! `xtask budget --check`, which reads the threshold out of `docs/dev/budgets.json` and the
//! numbers out of the criterion output this run leaves in `target/criterion`.

mod histogram;

use std::error::Error;
use std::hint::spin_loop;
use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use criterion::Criterion;
use ime_types::{
    Anchor, ImeError, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId, StatusStrip,
    UiCommand, UiFrame,
};

use histogram::LatencyHistogram;
use ime_ui::channel::{UiEventQueue, Wakeup};
use ime_ui::ui_thread::{SurfaceUpdate, UiContext, UiSurface, UiThread, UiThreadConfig};

/// Deliveries the acceptance criterion states for the wakeup case.
///
/// Run once, before criterion's sampler starts, so the number the criterion names is
/// measured on every run rather than left to the sampler's own iteration count.
const WAKEUP_DELIVERIES: u64 = 10_000;

/// Deliveries the pointer case runs, the same population as the wakeup case.
///
/// One histogram sample per delivery, for the same reason: the percentile the criterion
/// is read against is the deliveries', and a shorter run would describe the sampler's
/// blocks instead of the population.
const POINTER_DELIVERIES: u64 = 10_000;

/// How long one delivery may take before the benchmark reports a failure rather than hanging.
///
/// A quarter of a second is five thousand times the budget: a delivery that takes longer than
/// this is a UI thread that has stopped, not a slow wakeup, and a benchmark that waited
/// forever for one would be reported as a hung job instead of a failed measurement.
const DELIVERY_TIMEOUT: Duration = Duration::from_millis(250);

/// Spin iterations between two `yield_now` calls while the poster waits.
///
/// Spinning alone is fastest on the multi-core machine the budget is stated for, and
/// yielding alone would spend the whole wait inside the scheduler; alternating keeps the
/// common wait short and still lets a single-core machine run the thread being waited for.
const YIELD_EVERY: u32 = 64;

/// How long the UI thread is given to stop once the measurement is over.
///
/// The shutdown budget is two hundred milliseconds; the wait is deliberately far longer, so
/// that a machine under load reports the measured count of timeouts rather than a failure of
/// the benchmark that is really a failure of the machine.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The stamps the two threads share, the run's counters, and the histogram they fill.
///
/// # Concurrency
///
/// `Send` and `Sync`. Every counter is an atomic and the histogram sits behind a mutex. The
/// poster is the only writer of the sequence and delivery counters, and the surface is the
/// only writer of the observation counter.
struct Probe {
    /// The epoch both threads measure against, so a stamp can travel as a plain number.
    epoch: Instant,
    /// The revision of the newest frame the poster has built.
    sequence: AtomicU32,
    /// How many deliveries have been posted, which is the observation count each delivery
    /// waits for before the next post is made.
    deliveries: AtomicU64,
    /// Nanoseconds since `epoch` at which the poster handed over the delivery in flight.
    posted: AtomicU64,
    /// How many deliveries the surface has observed.
    observed: AtomicU64,
    /// One sample per delivery.
    histogram: Mutex<LatencyHistogram>,
}

impl Probe {
    /// Creates an empty probe and the epoch the two threads will share.
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            sequence: AtomicU32::new(0),
            deliveries: AtomicU64::new(0),
            posted: AtomicU64::new(0),
            observed: AtomicU64::new(0),
            histogram: Mutex::new(LatencyHistogram::default()),
        }
    }

    /// Posts one frame and waits for the UI thread to observe it.
    ///
    /// The wait is bounded, so a UI thread that has stopped is reported as a failed delivery
    /// rather than allowed to hang the benchmark.
    fn deliver_one(&self, thread: &UiThread) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let expected = self.deliveries.fetch_add(1, Ordering::Relaxed) + 1;
        self.stamp_post();
        let posted = thread.send(UiCommand::Frame(frame(sequence.wrapping_add(1))));
        assert!(
            posted.is_ok(),
            "the UI thread must accept delivery {expected}: {posted:?}"
        );
        self.await_observation(expected);
    }

    /// Runs `deliveries` deliveries, one wakeup each.
    fn run(&self, thread: &UiThread, deliveries: u64) {
        for _ in 0..deliveries {
            self.deliver_one(thread);
        }
    }

    /// Posts one pointer delivery: the stamp, then the connection descriptor going ready,
    /// which is the only signal a real backend raises for queued input.
    fn deliver_pointer(&self, connection: &Wakeup) {
        let expected = self.deliveries.fetch_add(1, Ordering::Relaxed) + 1;
        self.stamp_post();
        connection
            .wake()
            .expect("the connection descriptor accepts a wake");
        self.await_observation(expected);
    }

    /// Runs `deliveries` pointer deliveries, one descriptor wake each.
    fn run_pointer(&self, connection: &Wakeup, deliveries: u64) {
        for _ in 0..deliveries {
            self.deliver_pointer(connection);
        }
    }

    /// Spins until the surface has observed delivery number `expected`.
    fn await_observation(&self, expected: u64) {
        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        let mut spins: u32 = 0;
        while self.observed() < expected {
            assert!(
                Instant::now() < deadline,
                "delivery {expected} was not observed within {:?}",
                DELIVERY_TIMEOUT
            );
            spins += 1;
            if spins >= YIELD_EVERY {
                spins = 0;
                thread::yield_now();
            } else {
                spin_loop();
            }
        }
    }

    /// Stamps a post, immediately before the command is handed over.
    ///
    /// The store is a release so that the stamp a woken reader sees is never one a later post
    /// has overwritten. The delivery loop already makes that impossible by waiting for each
    /// observation before it posts the next command; the ordering is what keeps it impossible
    /// if the loop is ever changed.
    fn stamp_post(&self) {
        let nanos = self.nanos_since_epoch(Instant::now());
        self.posted.store(nanos, Ordering::Release);
    }

    /// Records one delivery: how long the post took to reach the UI thread.
    ///
    /// The clock is read first and the sample is published last, so nothing this call does
    /// after its first line is inside the interval it measures.
    fn observe(&self) {
        let nanos = self.nanos_since_epoch(Instant::now());
        let posted = self.posted.load(Ordering::Acquire);
        let elapsed = nanos.saturating_sub(posted);
        let mut histogram = self.histogram();
        histogram.record(Duration::from_nanos(elapsed));
        drop(histogram);
        self.observed.fetch_add(1, Ordering::Release);
    }

    /// How many deliveries the surface has observed.
    fn observed(&self) -> u64 {
        self.observed.load(Ordering::Acquire)
    }

    /// How many deliveries have been posted.
    fn deliveries(&self) -> u64 {
        self.deliveries.load(Ordering::Relaxed)
    }

    /// The histogram, held for as long as the guard lives.
    ///
    /// A panic while the histogram was locked cannot leave it inconsistent -- every method on
    /// it is a counter update -- so the guard is recovered rather than propagated.
    fn histogram(&self) -> MutexGuard<'_, LatencyHistogram> {
        let slot = &self.histogram;
        slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Nanoseconds since the probe's epoch.
    fn nanos_since_epoch(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.epoch).as_nanos()).unwrap_or(u64::MAX)
    }
}

/// The surface the loop drives: it records when each delivery reaches the UI thread.
///
/// It reports no descriptor of its own, which is what makes the loop wait on the wakeup
/// counter alone and the benchmark need no display server. Everything else is a no-op: what
/// is measured is the wakeup, and work here would be work the real surface does not do on
/// this path.
struct RecordingSurface {
    probe: Arc<Probe>,
}

impl UiSurface for RecordingSurface {
    fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        None
    }

    fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError> {
        if matches!(update, SurfaceUpdate::Frame(_)) {
            self.probe.observe();
        }
        Ok(())
    }

    fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
        Ok(())
    }

    fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
        // Never animating: an idle loop is what the wakeup latency is stated for, and a
        // deadline here would turn the measurement into a frame-paced one.
        Ok(None)
    }

    fn close(&mut self) -> Result<(), ImeError> {
        Ok(())
    }
}

/// The surface the loop drives for the pointer case: it reports the connection descriptor
/// the poster wakes, and records when the drained input reaches it.
///
/// This is the half of the pointer path the surface's own descriptor unlocks -- without
/// it the loop can only notice input when it wakes for some other reason. Nothing is ever
/// posted on this path, so `apply` is a no-op: the delivery is complete when the loop has
/// left its wait and handed the surface its drain call.
struct PointerSurface {
    probe: Arc<Probe>,
    connection: Wakeup,
}

impl UiSurface for PointerSurface {
    fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        Some(self.connection.fd())
    }

    fn apply(&mut self, _update: SurfaceUpdate) -> Result<(), ImeError> {
        Ok(())
    }

    fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
        self.probe.observe();
        Ok(())
    }

    fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
        // Never animating, for the same reason the wakeup case is: a deadline would pace
        // the measurement by frames instead of by deliveries.
        Ok(None)
    }

    fn close(&mut self) -> Result<(), ImeError> {
        Ok(())
    }
}

/// Times one delivery per iteration, against the real UI thread.
fn bench_wakeup_latency(criterion: &mut Criterion, thread: &UiThread, probe: &Probe) {
    let mut group = criterion.benchmark_group("ui");
    group.bench_function("wakeup_latency", |bencher| {
        bencher.iter_custom(|iterations| {
            let start = Instant::now();
            probe.run(thread, iterations);
            start.elapsed()
        });
    });
    group.finish();
}

/// Times one pointer delivery per iteration, against the real UI thread: the connection
/// descriptor going ready, the loop leaving `poll(2)`, and the surface draining what it
/// has queued.
fn bench_pointer_to_pixel(criterion: &mut Criterion, connection: &Wakeup, probe: &Probe) {
    let mut group = criterion.benchmark_group("ui");
    group.bench_function("pointer_to_pixel", |bencher| {
        bencher.iter_custom(|iterations| {
            let start = Instant::now();
            probe.run_pointer(connection, iterations);
            start.elapsed()
        });
    });
    group.finish();
}

/// Asserts what the run's samples have to satisfy, whatever the machine did.
fn assert_samples_hold(probe: &Probe) {
    let deliveries = probe.deliveries();
    let histogram = probe.histogram();
    let samples = histogram.samples();
    assert!(
        samples >= deliveries,
        "every delivery must leave one sample: {deliveries} posted, {samples} recorded"
    );
    let window = LatencyHistogram::WINDOW;
    let p99 = histogram.p99();
    let inside = p99.is_some_and(|sample| sample <= window);
    assert!(
        inside,
        "the ninety-ninth percentile must land inside the {window:?} window, got {p99:?}"
    );
}

/// Checks the histogram's arithmetic before the run measures anything with it.
///
/// Synthetic samples and no clock, so the check is the same on every machine and on every
/// run. It is here rather than in a `#[cfg(test)]` module because `harness = false` means
/// libtest never drives this target: a test written there would never execute.
fn assert_instrument_is_sound() {
    let empty = LatencyHistogram::new();
    assert_eq!(
        empty.p99(),
        None,
        "an empty histogram has no percentile to report"
    );

    // One hundred samples from one microsecond to one hundred: the ninety-ninth percentile
    // is the ninety-ninth of them, reported as the upper edge of the bucket it fell in.
    let mut hundred = LatencyHistogram::new();
    for micros in 1..=100u64 {
        hundred.record(Duration::from_micros(micros));
    }
    assert_eq!(hundred.samples(), 100);
    assert_eq!(hundred.p99(), Some(Duration::from_micros(100)));

    // Five percent of the samples past the window move the percentile past it too, which is
    // what turns a stalled run into a failure rather than into a flattering percentile.
    let mut stalled = LatencyHistogram::new();
    for _ in 0..95 {
        stalled.record(Duration::from_micros(1));
    }
    for _ in 0..5 {
        stalled.record(LatencyHistogram::WINDOW + Duration::from_millis(1));
    }
    let stalled_p99 = stalled.p99();
    assert_eq!(stalled.samples(), 100);
    assert!(
        stalled_p99.is_some_and(|sample| sample > LatencyHistogram::WINDOW),
        "a stalled tail must be reported past the window, got {stalled_p99:?}"
    );
}

/// The frame one delivery carries.
///
/// Deliberately empty. The measurement is the wakeup, not the payload, and an empty frame
/// costs nothing beyond its own box -- no `String` and no `Vec` allocates -- so no iteration
/// pays for candidates that the measurement would then have to discount.
fn frame(revision: u32) -> Box<UiFrame> {
    Box::new(UiFrame {
        revision,
        preedit: Preedit {
            text: String::new(),
            caret: 0,
            spans: Vec::new(),
        },
        candidates: Vec::new(),
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: Anchor {
            cursor: RectI {
                x: 0,
                y: 0,
                w: 2,
                h: 20,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Below,
        },
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
        highlight: None,
    })
}

/// Runs the acceptance criterion's own measurement, then the criterion case.
fn main() -> Result<(), Box<dyn Error>> {
    assert_instrument_is_sound();

    let probe = Arc::new(Probe::new());
    let surface = Arc::clone(&probe);
    let thread = UiThread::spawn(UiThreadConfig::default(), move |_context: UiContext| {
        Ok(Box::new(RecordingSurface { probe: surface }) as Box<dyn UiSurface>)
    })?;

    // The ten thousand deliveries the acceptance criterion states, one histogram sample
    // each, before criterion's sampler adds its own.
    probe.run(&thread, WAKEUP_DELIVERIES);
    let samples = probe.histogram().samples();
    assert_eq!(
        samples, WAKEUP_DELIVERIES,
        "each delivery must leave exactly one sample, got {samples}: a different count means \
         the loop folded two posts into one wakeup"
    );

    let mut criterion = Criterion::default().configure_from_args();
    bench_wakeup_latency(&mut criterion, &thread, &probe);

    // The pointer case: the same loop, woken through the connection descriptor a display
    // backend reports instead of through the shared counter. The pre-run is the
    // acceptance measurement; criterion's sampler adds its own block afterwards.
    let pointer_probe = Arc::new(Probe::new());
    let connection = Wakeup::new().expect("an eventfd can be created");
    let poster_fd = connection.clone();
    let surface = PointerSurface {
        probe: Arc::clone(&pointer_probe),
        connection,
    };
    let pointer_thread = UiThread::spawn(UiThreadConfig::default(), move |_context: UiContext| {
        Ok(Box::new(surface) as Box<dyn UiSurface>)
    })?;
    pointer_probe.run_pointer(&poster_fd, POINTER_DELIVERIES);
    let pointer_samples = pointer_probe.histogram().samples();
    assert_eq!(
        pointer_samples, POINTER_DELIVERIES,
        "each pointer delivery must leave exactly one sample, got {pointer_samples}: a \
         different count means the loop missed a descriptor wake or drained twice"
    );
    bench_pointer_to_pixel(&mut criterion, &poster_fd, &pointer_probe);
    criterion.final_summary();

    assert_samples_hold(&probe);
    assert_samples_hold(&pointer_probe);

    let stopped = thread.shutdown(SHUTDOWN_TIMEOUT);
    assert!(stopped.is_ok(), "the UI thread must stop: {stopped:?}");
    assert_eq!(
        thread.stats().shutdown_timeouts,
        0,
        "the UI thread stopped inside its shutdown budget"
    );

    let pointer_stopped = pointer_thread.shutdown(SHUTDOWN_TIMEOUT);
    assert!(
        pointer_stopped.is_ok(),
        "the pointer thread must stop: {pointer_stopped:?}"
    );
    assert_eq!(
        pointer_thread.stats().shutdown_timeouts,
        0,
        "the pointer thread stopped inside its shutdown budget"
    );
    Ok(())
}
