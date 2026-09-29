//! Tests for the frame-rate channel.
//!
//! Every test here runs with no display server and no clock of its own: the timestamps are
//! handed to the log, so a sequence of sixty commits at a sixtieth of a second is asserted
//! in microseconds rather than by sleeping for a second. The decorator's own tests drive a
//! backend that records what it was asked to do, which is how the acceptance criterion that
//! the wrapper changes nothing is checked rather than argued.

use std::time::{Duration, Instant};

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};

use super::*;

/// The interval a sixtieth of a second is, in the microseconds the tests step by.
const FRAME: Duration = Duration::from_micros(16_667);

/// A failure count that never runs out, for a backend that is starved for the whole run.
const STARVED: u32 = u32::MAX;

/// One operation a backend was asked to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// A buffer was asked for.
    Acquire,
    /// A commit was made, with this many damage rectangles.
    Commit(usize),
    /// An input region was set, with this many rectangles.
    SetInputRegion(usize),
    /// The surface was shown or hidden.
    SetVisible(bool),
    /// A frame callback was requested.
    RequestFrame,
    /// Pending events were polled.
    PollEvents,
}

/// A backend that answers from memory and records every call.
///
/// It is what makes the "the decorator changes nothing" criterion checkable: the same script
/// is run against a bare double and against a wrapped one, and the two call logs have to be
/// identical.
struct RecordingBackend {
    /// Logical width, reported by `geometry`.
    width: u32,
    /// Logical height, reported by `geometry`.
    height: u32,
    /// Device pixel ratio, reported by `geometry`.
    scale: f32,
    /// The one draw buffer it hands out.
    buffer: Vec<u8>,
    /// What it was asked to do, in order.
    calls: Vec<Call>,
    /// Whether the surface is currently mapped.
    visible: bool,
    /// The interactive region it was last given.
    region: Vec<RectI>,
    /// The events `poll_events` delivers.
    events: Vec<SurfaceEvent>,
    /// How many of the next acquisitions answer `NoFreeBuffer`.
    ///
    /// A count rather than a flag: the miss a case has to place is the one in the middle of a
    /// run, since what it shows is the commits on either side of it still being recorded.
    acquire_failures: u32,
    /// Whether committing answers `Disconnected`.
    commit_fails: bool,
    /// What `request_frame` answers.
    frame_token: Option<FrameToken>,
}

impl RecordingBackend {
    /// A backend of the candidate window's own size, with nothing asked of it yet.
    fn new() -> Self {
        Self {
            width: 600,
            height: 140,
            scale: 2.0,
            buffer: vec![0; 600 * 140 * 4],
            calls: Vec::new(),
            visible: false,
            region: Vec::new(),
            events: Vec::new(),
            acquire_failures: 0,
            commit_fails: false,
            frame_token: None,
        }
    }
}

impl SurfaceBackend for RecordingBackend {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        self.calls.push(Call::Acquire);
        if self.acquire_failures > 0 {
            self.acquire_failures -= 1;
            return Err(PlatformError::NoFreeBuffer);
        }
        Ok(PixelBufferMut {
            stride: self.width as usize * 4,
            width: self.width,
            height: self.height,
            data: &mut self.buffer,
        })
    }

    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
        self.calls.push(Call::Commit(damage.len()));
        if self.commit_fails {
            return Err(PlatformError::Disconnected);
        }
        Ok(())
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        self.calls.push(Call::SetInputRegion(rects.len()));
        self.region = rects.to_vec();
        Ok(())
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        self.calls.push(Call::SetVisible(visible));
        self.visible = visible;
        Ok(())
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        self.calls.push(Call::RequestFrame);
        self.frame_token
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        self.calls.push(Call::PollEvents);
        out.extend(self.events.iter().copied());
        Ok(())
    }

    fn geometry(&self) -> (u32, u32, f32) {
        (self.width, self.height, self.scale)
    }

    fn backend_id(&self) -> &'static str {
        "recording"
    }
}

/// A log of `count` commits spaced `interval` apart, starting at `base`.
fn spaced(base: Instant, count: u32, interval: Duration) -> CommitLog {
    let mut log = CommitLog::new();
    for index in 0..count {
        log.push(base + interval * index, 100, None);
    }
    log
}

/// The rectangle the tests damage.
fn cell() -> RectI {
    RectI {
        x: 8,
        y: 8,
        w: 32,
        h: 32,
    }
}

#[test]
fn test_sixty_evenly_spaced_commits_report_sixty_fps_with_no_drops() {
    let log = spaced(Instant::now(), 60, FRAME);
    let stats = log.frame_stats(60.0);
    assert_eq!(stats.frames, 60);
    assert!(
        (stats.fps - 60.0).abs() < 0.5,
        "sixty commits a frame apart are sixty frames a second: {stats:?}"
    );
    assert_eq!(stats.dropped, 0, "no gap is late against a 16.67ms period");
    assert!(
        (stats.span_ms - 983.35).abs() < 1.0,
        "fifty-nine intervals of a sixtieth of a second: {stats:?}"
    );
    assert!(
        (stats.interval_p50_ms - 16.667).abs() < 0.01,
        "every interval is the frame time: {stats:?}"
    );
    assert!((stats.interval_p99_ms - 16.667).abs() < 0.01);
    assert_eq!(stats.acquire_misses, 0);
    assert_eq!(stats.target_hz, 60.0);
}

#[test]
fn test_three_fifty_millisecond_gaps_count_as_three_dropped_frames() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    let mut clock = base;
    log.push(clock, 0, None);
    for _ in 0..3 {
        for _ in 0..10 {
            clock += FRAME;
            log.push(clock, 0, None);
        }
        // Fifty milliseconds is three frames' worth at sixty a second, and the criterion
        // counts the gap once.
        clock += Duration::from_millis(50);
        log.push(clock, 0, None);
    }
    for _ in 0..10 {
        clock += FRAME;
        log.push(clock, 0, None);
    }
    let stats = log.frame_stats(60.0);
    assert_eq!(
        stats.dropped, 3,
        "one gap past 1.5 times the period is one dropped frame: {stats:?}"
    );
}

#[test]
fn test_a_gap_counts_once_however_long_it_is() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    log.push(base, 0, None);
    log.push(base + Duration::from_millis(100), 0, None);
    let stats = log.frame_stats(60.0);
    assert_eq!(
        stats.dropped, 1,
        "the count is of missed deadlines, not of frames never drawn: {stats:?}"
    );
    assert!((stats.worst_interval_ms - 100.0).abs() < 0.01, "{stats:?}");
}

#[test]
fn test_a_gap_at_the_drop_threshold_is_not_a_drop() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    log.push(base, 0, None);
    // A sixtieth of a second times one and a half is the threshold, and the comparison is
    // strict: a gap of exactly the threshold made its deadline and a gap a microsecond longer
    // missed it. The threshold is pinned at 25ms exactly rather than approached, because that
    // is where `1000.0 / 60.0` in `f32` and one and a half of it land -- a shade under a
    // sixtieth of a second, multiplied by one and a half, rounds back to 25.
    log.push(base + Duration::from_millis(25), 0, None);
    let stats = log.frame_stats(60.0);
    assert_eq!(
        stats.period_ms * stats.drop_factor,
        25.0,
        "the criterion this case rests on is 25ms: {stats:?}"
    );
    assert_eq!(stats.dropped, 0, "the comparison is `>`: {stats:?}");

    log.push(
        base + Duration::from_millis(50) + Duration::from_micros(1),
        0,
        None,
    );
    let stats = log.frame_stats(60.0);
    assert_eq!(
        stats.dropped, 1,
        "a microsecond past the threshold is a missed deadline: {stats:?}"
    );
    assert!(
        (stats.worst_interval_ms - 25.001).abs() < 0.01,
        "the interval that missed it is the one that was a microsecond late: {stats:?}"
    );
}

#[test]
fn test_percentiles_follow_the_nearest_rank_rule() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    log.push(base, 0, None);
    for millis in 1..=100u64 {
        // Cumulative, so the intervals are one through a hundred milliseconds rather than
        // a hundred intervals of one. The percentiles are what this case is about, and a
        // fixture whose every interval is the same length would pin nothing.
        let at = millis * (millis + 1) / 2;
        log.push(base + Duration::from_millis(at), 0, None);
    }
    let stats = log.frame_stats(60.0);
    assert_eq!(stats.frames, 101);
    // Every interval from one to a hundred milliseconds, so the nearest rank of the
    // hundredth percentile is the interval at that rank rather than an average of two.
    assert!(
        (stats.interval_p50_ms - 50.0).abs() < 0.01,
        "the fiftieth interval is the median: {stats:?}"
    );
    assert!((stats.interval_p95_ms - 95.0).abs() < 0.01, "{stats:?}");
    assert!((stats.interval_p99_ms - 99.0).abs() < 0.01, "{stats:?}");
    assert!((stats.worst_interval_ms - 100.0).abs() < 0.01, "{stats:?}");
    assert!((stats.span_ms - 5050.0).abs() < 1.0, "{stats:?}");
    assert_eq!(
        stats.dropped, 75,
        "every interval past 25ms is a missed deadline: {stats:?}"
    );
}

#[test]
fn test_stats_of_a_log_with_no_interval_are_zero() {
    let empty = CommitLog::new().frame_stats(60.0);
    assert_eq!(empty.frames, 0);
    assert_eq!(empty.fps, 0.0, "there is no interval to divide by");
    assert_eq!(empty.dropped, 0);
    assert_eq!(empty.interval_p99_ms, 0.0);
    assert_eq!(empty.span_ms, 0.0);

    let mut single = CommitLog::new();
    single.push(Instant::now(), 0, None);
    let stats = single.frame_stats(60.0);
    assert_eq!(stats.frames, 1);
    assert_eq!(stats.fps, 0.0, "one commit spans no interval at all");
    assert_eq!(stats.dropped, 0);
}

#[test]
fn test_the_window_keeps_only_the_newest_commits() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    for index in 0..(WINDOW as u64 + 10) {
        log.push(base + FRAME * index as u32, index, None);
    }
    assert_eq!(log.frames(), WINDOW, "the window is bounded");
    let oldest = log.records().next().expect("the window is not empty");
    let newest = log.last().expect("the window is not empty");
    assert_eq!(oldest.damage_px, 10, "the ten oldest commits aged out");
    assert_eq!(newest.damage_px, WINDOW as u64 + 9);
    assert_eq!(
        log.records().count(),
        WINDOW,
        "the iterator walks the window, not the history"
    );
}

#[test]
fn test_a_window_that_is_exactly_full_keeps_every_commit() {
    // The boundary of the window: a log holding exactly `WINDOW` commits is not over it, so
    // nothing is evicted and the statistics cover all of them.
    let log = spaced(Instant::now(), WINDOW as u32, FRAME);
    assert_eq!(log.frames(), WINDOW, "the boundary is `> WINDOW`");
    let stats = log.frame_stats(60.0);
    assert_eq!(stats.frames, WINDOW as u32);
    assert_eq!(stats.dropped, 0);
    assert!(
        (stats.fps - 60.0).abs() < 0.5,
        "a full window is still a rate: {stats:?}"
    );
    assert!(
        (stats.span_ms - 9_983.53).abs() < 2.0,
        "five hundred and ninety-nine intervals of a sixtieth of a second: {stats:?}"
    );
}

#[test]
fn test_the_statistics_cover_the_window_and_not_the_evicted_history() {
    let base = Instant::now();
    let mut log = CommitLog::new();
    // A commit a second before the rest. While it is in the window its gap is the longest
    // interval there, so a statistic that still saw it would report a drop and a rate far
    // below the one the run reached.
    log.push(base, 0, None);
    for index in 0..(WINDOW as u32) {
        log.push(base + Duration::from_secs(1) + FRAME * index, 0, None);
    }
    assert_eq!(log.frames(), WINDOW, "the oldest commit aged out");
    let stats = log.frame_stats(60.0);
    assert_eq!(
        stats.dropped, 0,
        "the second-long gap left the window with its commit: {stats:?}"
    );
    assert!(
        (stats.fps - 60.0).abs() < 0.5,
        "the rate is the one the retained window was drawn at: {stats:?}"
    );
    assert!(
        (stats.span_ms - 9_983.53).abs() < 2.0,
        "the span is the window's, not the history's: {stats:?}"
    );
}

#[test]
fn test_clearing_the_log_keeps_or_drops_the_miss_count_as_asked() {
    let mut log = CommitLog::new();
    log.push(Instant::now(), 0, None);
    log.count_acquire_miss();
    log.clear();
    assert_eq!(log.frames(), 0);
    assert!(log.is_empty());
    assert_eq!(
        log.acquire_misses(),
        1,
        "a case that wants a fresh window keeps the run's own misses"
    );
    log.reset();
    assert_eq!(log.acquire_misses(), 0);
}

#[test]
fn test_an_unusable_target_is_replaced_by_the_pacing_the_platform_uses() {
    for unusable in [0.0, -60.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            normalize_target(unusable),
            DEFAULT_TARGET_HZ,
            "a target nobody can measure against is replaced: {unusable}"
        );
    }
    assert_eq!(normalize_target(144.0), 144.0, "a real rate is kept");
    let log = spaced(Instant::now(), 10, Duration::from_micros(6_944));
    let stats = log.frame_stats(f32::NAN);
    assert_eq!(stats.target_hz, DEFAULT_TARGET_HZ);
    assert!(
        (stats.period_ms - 1000.0 / DEFAULT_TARGET_HZ).abs() < 0.01,
        "{stats:?}"
    );
}

#[test]
fn test_a_policy_states_the_rate_a_run_is_judged_against() {
    // The same sixty commits, judged against the tier they were paced at and against a 144Hz
    // one. The rate the run reached does not move -- it is what was measured -- and the
    // verdict does, which is the whole reason the criterion is a value the case states.
    let log = spaced(Instant::now(), 60, FRAME);
    let at_60 = log.frame_stats_with(&FrameRatePolicy::DEFAULT);
    let at_144 = log.frame_stats_with(&FrameRatePolicy::at(144.0));
    assert_eq!(at_60.target_hz, DEFAULT_TARGET_HZ);
    assert_eq!(at_60.drop_factor, DROP_FACTOR);
    assert_eq!(
        at_60.dropped, 0,
        "a sixtieth of a second is the period the run was paced at: {at_60:?}"
    );
    assert_eq!(at_144.target_hz, 144.0);
    assert!(
        (at_144.period_ms - 1000.0 / 144.0).abs() < 0.001,
        "the period follows the rate: {at_144:?}"
    );
    assert_eq!(
        at_144.dropped, 59,
        "every interval is past a 144Hz deadline: {at_144:?}"
    );
    assert!(
        (at_144.fps - at_60.fps).abs() < 0.01,
        "the measured rate is the run's, not the criterion's: {at_144:?} {at_60:?}"
    );
    assert_eq!(at_144.frames, at_60.frames);

    // The factor is how a criterion other than the design's is stated: a hundred-millisecond
    // gap is a missed deadline against one and a half periods and inside eight of them.
    let base = Instant::now();
    let mut gapped = CommitLog::new();
    gapped.push(base, 0, None);
    gapped.push(base + Duration::from_millis(100), 0, None);
    let lenient = FrameRatePolicy {
        target_hz: 60.0,
        drop_factor: 8.0,
    };
    assert_eq!(gapped.frame_stats(60.0).dropped, 1);
    let lenient_stats = gapped.frame_stats_with(&lenient);
    assert_eq!(
        lenient_stats.dropped, 0,
        "a hundred milliseconds is inside eight periods: {lenient_stats:?}"
    );
    assert_eq!(gapped.frame_stats_with(&lenient).drop_factor, 8.0);
}

#[test]
fn test_an_unusable_drop_factor_is_replaced_by_the_designs_own() {
    for unusable in [0.0, 0.5, -1.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            normalize_drop_factor(unusable),
            DROP_FACTOR,
            "a factor that cannot judge a frame is replaced: {unusable}"
        );
    }
    assert_eq!(
        normalize_drop_factor(1.0),
        1.0,
        "a factor of one is strict, not unusable"
    );
    assert_eq!(normalize_drop_factor(2.0), 2.0);

    // A factor that is not a number makes every comparison false, so the run would be
    // reported with no drops at all; the criterion is replaced rather than reported.
    let base = Instant::now();
    let mut log = CommitLog::new();
    log.push(base, 0, None);
    log.push(base + Duration::from_millis(100), 0, None);
    let unusable = FrameRatePolicy {
        target_hz: 60.0,
        drop_factor: f32::NAN,
    };
    let stats = log.frame_stats_with(&unusable);
    assert_eq!(stats.drop_factor, DROP_FACTOR);
    assert_eq!(
        stats.dropped, 1,
        "the replaced criterion still counts the gap: {stats:?}"
    );
    assert_eq!(
        log.frame_stats_with(&FrameRatePolicy::at(f32::NAN))
            .target_hz,
        DEFAULT_TARGET_HZ,
        "both halves of a policy are normalised"
    );
}

#[test]
fn test_damage_px_sums_the_rectangles_and_saturates() {
    assert_eq!(damage_px(&[]), 0);
    assert_eq!(damage_px(&[cell()]), 1024);
    assert_eq!(damage_px(&[cell(), cell()]), 2048);
    assert_eq!(
        damage_px(&[RectI {
            x: 0,
            y: 0,
            w: 0,
            h: 100
        }]),
        0,
        "a degenerate rectangle damages nothing"
    );
    let huge = RectI {
        x: 0,
        y: 0,
        w: u32::MAX,
        h: u32::MAX,
    };
    assert_eq!(damage_px(&[huge, huge]), u64::MAX, "the sum saturates");
}

#[test]
fn test_the_decorator_does_not_change_what_the_backend_does() {
    let region = [cell()];
    // Both sides are driven through the same script, and the script asks for a frame callback
    // so that the one call the decorator has state for -- the token it holds until the next
    // commit -- is compared rather than left to the tests that cover it on its own.
    let callback = Some(FrameToken(3));

    let mut plain = RecordingBackend::new();
    plain.frame_token = callback;
    plain.set_visible(true).expect("the surface is shown");
    plain
        .set_input_region(&region)
        .expect("the region is applied");
    {
        let buffer = plain.acquire_buffer().expect("a buffer is free");
        let first = buffer.data.first_mut().expect("a draw buffer has a byte");
        *first = 7;
    }
    let plain_token = plain.request_frame();
    plain.commit(&region).expect("the frame is committed");
    let mut plain_events = Vec::new();
    plain
        .poll_events(&mut plain_events)
        .expect("the queue is polled");
    let plain_geometry = plain.geometry();
    let plain_id = plain.backend_id();

    let mut inner_backend = RecordingBackend::new();
    inner_backend.frame_token = callback;
    let mut wrapped = InstrumentedBackend::new(inner_backend);
    wrapped.set_visible(true).expect("the surface is shown");
    wrapped
        .set_input_region(&region)
        .expect("the region is applied");
    {
        let buffer = wrapped.acquire_buffer().expect("a buffer is free");
        let first = buffer.data.first_mut().expect("a draw buffer has a byte");
        *first = 7;
    }
    let wrapped_token = wrapped.request_frame();
    wrapped.commit(&region).expect("the frame is committed");
    let mut wrapped_events = Vec::new();
    wrapped
        .poll_events(&mut wrapped_events)
        .expect("the queue is polled");
    let wrapped_geometry = wrapped.geometry();
    let wrapped_id = wrapped.backend_id();
    let recorded_token = wrapped.log().last().map(|record| record.frame_token);
    let inner = wrapped.into_inner();

    assert_eq!(
        inner.calls, plain.calls,
        "the same calls, in the same order"
    );
    assert_eq!(inner.visible, plain.visible);
    assert_eq!(inner.region, plain.region);
    assert_eq!(
        inner.buffer[0], plain.buffer[0],
        "the same buffer was written"
    );
    assert_eq!(wrapped_events, plain_events);
    assert_eq!(wrapped_geometry, plain_geometry);
    assert_eq!(wrapped_id, plain_id);
    assert_eq!(
        wrapped_token, plain_token,
        "a frame callback the backend answers is handed back unchanged"
    );
    assert_eq!(
        recorded_token,
        Some(callback),
        "and the record names the callback the commit answered"
    );
    assert_eq!(
        inner.calls,
        vec![
            Call::SetVisible(true),
            Call::SetInputRegion(1),
            Call::Acquire,
            Call::RequestFrame,
            Call::Commit(1),
            Call::PollEvents,
        ]
    );
}

#[test]
fn test_acquire_misses_are_counted_and_the_error_reaches_the_caller() {
    let mut starved = RecordingBackend::new();
    starved.acquire_failures = STARVED;
    let mut wrapped = InstrumentedBackend::new(starved);
    for expected in 1..=3 {
        let refused = wrapped.acquire_buffer();
        assert!(
            matches!(refused, Err(PlatformError::NoFreeBuffer)),
            "a starved buffer is reported, never swallowed"
        );
        assert_eq!(wrapped.acquire_misses(), expected);
    }
    assert_eq!(
        wrapped.frame_stats(60.0).acquire_misses,
        3,
        "the count reaches the report"
    );

    let mut healthy = InstrumentedBackend::new(RecordingBackend::new());
    assert!(healthy.acquire_buffer().is_ok());
    assert_eq!(healthy.acquire_misses(), 0, "a free buffer is not a miss");
    assert_eq!(healthy.log().frames(), 0, "acquiring is not committing");
}

#[test]
fn test_a_starved_buffer_does_not_interrupt_the_sampling() {
    // One miss in the middle of a run: the caller skips the frame it could not draw, which is
    // the design's own degradation, and the commits on either side of the skip are recorded
    // all the same. A harness that stopped counting at the first starved buffer would report
    // a shorter run than the one that happened.
    let mut starved_once = RecordingBackend::new();
    starved_once.acquire_failures = 1;
    let mut wrapped = InstrumentedBackend::new(starved_once);
    assert!(
        matches!(wrapped.acquire_buffer(), Err(PlatformError::NoFreeBuffer)),
        "the first buffer is not free"
    );
    for _ in 0..3 {
        {
            let buffer = wrapped.acquire_buffer().expect("the buffer is free again");
            assert_eq!(buffer.width, 600, "the frame is drawable");
        }
        wrapped.commit(&[cell()]).expect("the frame goes out");
    }

    let stats = wrapped.frame_stats(60.0);
    assert_eq!(
        stats.frames, 3,
        "the commits made after the miss are recorded all the same: {stats:?}"
    );
    assert_eq!(stats.acquire_misses, 1, "the miss is counted");
    assert_eq!(wrapped.log().frames(), 3);
    // The rate is not asserted here: the decorator stamps its records from the real clock, so
    // the interval arithmetic is asserted on the log, where the timestamps are handed in.
    let inner = wrapped.into_inner();
    assert_eq!(
        inner.calls,
        vec![
            Call::Acquire,
            Call::Acquire,
            Call::Commit(1),
            Call::Acquire,
            Call::Commit(1),
            Call::Acquire,
            Call::Commit(1),
        ],
        "the run continued through the miss"
    );
}

#[test]
fn test_sixty_commits_through_the_decorator_are_sixty_frames() {
    // The decorator driven the way the UI thread drives it, one acquire and one commit a
    // frame. The count is what this asserts; the rate a case reports for such a run is
    // asserted where the timestamps are the fixture's, because these come from the clock.
    let mut wrapped = InstrumentedBackend::new(RecordingBackend::new());
    for _ in 0..60 {
        {
            let buffer = wrapped.acquire_buffer().expect("a buffer is free");
            assert_eq!(buffer.stride, 600 * 4);
        }
        wrapped.commit(&[cell()]).expect("the frame goes out");
    }
    let stats = wrapped.frame_stats(60.0);
    assert_eq!(stats.frames, 60, "every commit reached the log");
    assert_eq!(stats.acquire_misses, 0, "nothing was starved");
    assert_eq!(wrapped.log().records().count(), 60);
    assert!(stats.span_ms > 0.0, "the run spans its commits");
}

#[test]
fn test_a_commit_is_recorded_with_the_token_its_frame_was_requested_for() {
    let mut with_callback = RecordingBackend::new();
    with_callback.frame_token = Some(FrameToken(7));
    let mut wrapped = InstrumentedBackend::new(with_callback);
    assert_eq!(wrapped.request_frame(), Some(FrameToken(7)));
    wrapped
        .commit(&[cell(), cell()])
        .expect("the frame goes out");
    wrapped.commit(&[]).expect("the second frame goes out");
    let records: Vec<CommitRecord> = wrapped.log().records().copied().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].frame_token,
        Some(FrameToken(7)),
        "the token belongs to the frame that answered it"
    );
    assert_eq!(
        records[1].frame_token, None,
        "a commit with no callback requested carries no token"
    );
    assert_eq!(records[0].damage_px, 2048, "the damage set is summed");
    assert_eq!(records[1].damage_px, 0);

    // The X11 tier asks for no callback at all, and the records say so.
    let mut wrapped = InstrumentedBackend::new(RecordingBackend::new());
    assert_eq!(wrapped.request_frame(), None);
    wrapped.commit(&[]).expect("the frame goes out");
    assert_eq!(
        wrapped.log().last().expect("a record").frame_token,
        None,
        "a backend without frame callbacks records no token"
    );
}

#[test]
fn test_a_failed_commit_reaches_the_caller_and_is_still_recorded() {
    let mut failing = RecordingBackend::new();
    failing.commit_fails = true;
    let mut wrapped = InstrumentedBackend::new(failing);
    assert!(matches!(
        wrapped.commit(&[]),
        Err(PlatformError::Disconnected)
    ));
    assert_eq!(
        wrapped.log().frames(),
        1,
        "a commit that failed still happened at that moment"
    );
}
