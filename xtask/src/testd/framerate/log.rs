//! The commit log: what the backend was asked to present, and when.
//!
//! Responsibility: hold the last few hundred commits with their timestamps and damage, and
//! turn them into the statistics a frame-rate case asserts on. Nothing here touches a
//! backend, a display server or a clock: a caller hands in the timestamps, so every number
//! this module reports is a pure function of the sequence it was given.
//!
//! # Why the window slides
//!
//! A case measures an interaction, not a session: six hundred commits is ten seconds at sixty
//! frames a second and four at a hundred and forty-four, which is the whole of what a
//! frame-rate claim is about. Keeping only the newest [`WINDOW`] commits bounds the memory one
//! measurement costs and makes the answer independent of how long the run has been going,
//! which is what lets the same assertion hold at the end of an eight-hour soak. The window is
//! a count of commits rather than a duration, so it does not assume the rate it is measuring.
//!
//! # What a dropped frame is, and what it is not
//!
//! The criterion is [`FrameRatePolicy`]: a gap longer than its `drop_factor` times its target
//! period is one dropped frame, and a gap is counted once however long it is. That number
//! answers "how many times did the run miss its deadline", which is what a budget assertion is
//! about; it deliberately does not try to answer "how many frames were never drawn", which
//! would depend on a refresh rate the X11 tier does not have.
//!
//! The criterion is a value a case hands in rather than a pair of constants inside
//! [`CommitLog::frame_stats`], for the reason the purity guard states for its own thresholds:
//! a tier paced at another rate states its own, and a module that kept a second copy of the
//! design's numbers would go on judging every run against them.
//!
//! # Why the interval percentiles are not the probe's histogram
//!
//! `ime_diag::probe::Histogram` is the production probe's distribution and it is the wrong
//! instrument here. Its buckets grow Fibonacci-style, so a frame interval of 16.667ms is
//! reported as the 17.711ms upper bound of the bucket it lands in -- an estimate that is
//! deliberately never smaller than the sample, which is right for a latency ceiling and wrong
//! for a criterion that decides at 25ms whether a frame was late. Its top bucket saturates at
//! 100ms, which is exactly the order of the gap a dropped frame leaves. And it holds counts,
//! not samples: a case that wants to say *which* interval was the worst, or that a commit
//! carried no frame callback, has nothing to read. The percentiles here are therefore the
//! nearest-rank ones over the measured values, and the histogram is not duplicated -- this
//! module records no sample until a caller hands it a timestamp.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ime_types::{FrameToken, RectI};

/// How many commits the log keeps.
pub const WINDOW: usize = 600;

/// The frame rate the X11 tier is paced at.
///
/// The design's own timer: without a compositor frame callback the UI thread drives
/// animation from a `1000/60 ms` timer, so sixty frames a second is the rate a case measures
/// against unless it names another.
pub const DEFAULT_TARGET_HZ: f32 = 60.0;

/// How much longer than the target period a gap has to be to count as a dropped frame.
///
/// The design's own factor, and the one [`FrameRatePolicy::DEFAULT`] carries: a gap past one and
/// a half periods is a frame that missed its deadline. A case that judges a run against
/// something else states its own factor through the policy rather than this constant.
pub const DROP_FACTOR: f32 = 1.5;

/// The criterion a run's commits are judged against.
///
/// The two numbers travel together because neither decides anything alone: a drop count
/// without the rate it was counted against cannot be read, and a rate without the factor says
/// nothing about how much jitter was tolerated. A case builds one with [`Self::at`] when it
/// only wants to state a rate, or with a struct literal when it wants to state both.
///
/// # The numbers are not restated from a budget
///
/// `docs/dev/budgets.json` states no frame-rate threshold: the budgets are ceilings on latency,
/// memory, CPU and size, and a frame rate is what a case measures *while* one of those is
/// asserted. The rate the X11 tier is paced at is the design's own timer, and it is stated here
/// once, as [`Self::DEFAULT`], rather than at each call site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRatePolicy {
    /// The rate the run was paced at, in frames per second.
    pub target_hz: f32,
    /// How much longer than the target period a gap has to be to count as a dropped frame.
    ///
    /// The comparison is strict, so a gap of exactly the threshold is on time.
    pub drop_factor: f32,
}

impl FrameRatePolicy {
    /// The design's own criterion: sixty frames a second, a gap past one and a half periods.
    pub const DEFAULT: Self = Self {
        target_hz: DEFAULT_TARGET_HZ,
        drop_factor: DROP_FACTOR,
    };

    /// The design's criterion at `target_hz`.
    ///
    /// The rate a case passes in is the rate the run was paced at -- the display's own where
    /// the tier learns it from a frame callback, and the UI thread's timer where it does not.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn at(target_hz: f32) -> Self {
        Self {
            target_hz,
            drop_factor: DROP_FACTOR,
        }
    }

    /// The same criterion with unusable numbers replaced by usable ones.
    ///
    /// Both replacements are the design's defaults rather than a refusal, for the reason
    /// [`normalize_target`] gives: a report that says which rate the run fell short of is
    /// worth more than one that says there was no rate to fall short of.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn normalized(self) -> Self {
        Self {
            target_hz: normalize_target(self.target_hz),
            drop_factor: normalize_drop_factor(self.drop_factor),
        }
    }
}

/// One commit, as it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    /// When the commit was started.
    ///
    /// The start rather than the end, because the interval between two commits is what a
    /// frame rate is made of and a commit's own duration belongs to it: recording the end
    /// would hide the cost of the frame that is still being presented.
    pub at: Instant,
    /// The damaged area the commit declared, summed in physical pixels.
    pub damage_px: u64,
    /// The frame callback token this commit answered, when the backend asks for one.
    pub frame_token: Option<FrameToken>,
}

/// The newest commits of one backend, with the buffer misses seen beside them.
#[derive(Clone, Debug, Default)]
pub struct CommitLog {
    /// The commits, oldest first, never more than [`WINDOW`] of them.
    records: VecDeque<CommitRecord>,
    /// How many times a buffer was not free.
    acquire_misses: u32,
}

impl CommitLog {
    /// An empty log.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one commit, dropping the oldest record once the window is full.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn push(&mut self, at: Instant, damage_px: u64, frame_token: Option<FrameToken>) {
        self.records.push_back(CommitRecord {
            at,
            damage_px,
            frame_token,
        });
        if self.records.len() > WINDOW {
            self.records.pop_front();
        }
    }

    /// Records one buffer that was not free.
    ///
    /// The count is kept even though the window is not: a miss is a property of the run, and
    /// a case that asserted on it after the commits that produced it had aged out would be
    /// asserting on nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn count_acquire_miss(&mut self) {
        self.acquire_misses = self.acquire_misses.saturating_add(1);
    }

    /// How many buffers were not free.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn acquire_misses(&self) -> u32 {
        self.acquire_misses
    }

    /// How many commits the log holds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn frames(&self) -> usize {
        self.records.len()
    }

    /// Whether the log holds no commit at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Empties the log, keeping the buffer-miss count.
    ///
    /// A case that wants a fresh measurement window clears the commits and leaves the run's
    /// own history of misses alone; [`CommitLog::reset`] clears both.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// Empties the log and the buffer-miss count.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reset(&mut self) {
        self.clear();
        self.acquire_misses = 0;
    }

    /// The commits, oldest first.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn records(&self) -> impl Iterator<Item = &CommitRecord> {
        self.records.iter()
    }

    /// The newest commit.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn last(&self) -> Option<&CommitRecord> {
        self.records.back()
    }

    /// The frame statistics of the log, against a target rate.
    ///
    /// A convenience over [`Self::frame_stats_with`] for a case that has a rate and wants the
    /// design's own drop criterion; the rate itself is normalised the same way there.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn frame_stats(&self, target_hz: f32) -> FrameStats {
        self.frame_stats_with(&FrameRatePolicy::at(target_hz))
    }

    /// The frame statistics of the log, against a criterion.
    ///
    /// The rate is the one the window was drawn at: the intervals between consecutive
    /// commits, divided into the span they cover. A log of fewer than two commits has no
    /// interval at all, so it reports a rate of zero rather than a division by an empty span.
    /// The drop count is the number of intervals past the criterion's threshold, which is what
    /// makes the rate a case measured and the verdict it reached two separate numbers.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn frame_stats_with(&self, policy: &FrameRatePolicy) -> FrameStats {
        let policy = policy.normalized();
        let target_hz = policy.target_hz;
        let period_ms = 1000.0 / target_hz;
        let mut intervals = Vec::with_capacity(self.records.len().saturating_sub(1));
        let mut previous: Option<Instant> = None;
        for record in &self.records {
            if let Some(last) = previous {
                intervals.push(elapsed_ms(record.at.saturating_duration_since(last)));
            }
            previous = Some(record.at);
        }
        let span_ms = match (self.records.front(), self.records.back()) {
            (Some(first), Some(last)) => elapsed_ms(last.at.saturating_duration_since(first.at)),
            _ => 0.0,
        };
        let threshold = period_ms * policy.drop_factor;
        let dropped = intervals
            .iter()
            .filter(|interval| **interval > threshold)
            .count() as u32;
        let fps = if span_ms > 0.0 {
            intervals.len() as f32 * 1000.0 / span_ms
        } else {
            0.0
        };
        intervals.sort_unstable_by(f32::total_cmp);
        FrameStats {
            frames: self.records.len() as u32,
            span_ms,
            fps,
            target_hz,
            period_ms,
            drop_factor: policy.drop_factor,
            dropped,
            interval_p50_ms: percentile(&intervals, 0.50),
            interval_p95_ms: percentile(&intervals, 0.95),
            interval_p99_ms: percentile(&intervals, 0.99),
            worst_interval_ms: intervals.last().copied().unwrap_or(0.0),
            acquire_misses: self.acquire_misses,
        }
    }
}

/// What a run of commits came to.
///
/// Every field is public because a case reads them one at a time and a report prints them;
/// the percentiles are the nearest-rank ones [`percentile`] documents, and `fps` is the rate
/// the commits were actually made at rather than the rate that was asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameStats {
    /// Commits the window holds.
    pub frames: u32,
    /// Time from the first commit in the window to the last, in milliseconds.
    pub span_ms: f32,
    /// The rate the commits were made at, in frames per second.
    pub fps: f32,
    /// The rate the statistics were taken against, after normalisation.
    pub target_hz: f32,
    /// The target period, `1000 / target_hz`, in milliseconds.
    pub period_ms: f32,
    /// How much longer than the period a gap had to be to count as dropped.
    ///
    /// Carried beside the count because a count is not readable without it: three dropped
    /// frames against a criterion of one and a half periods and three against a criterion of
    /// eight are different statements about the same run.
    pub drop_factor: f32,
    /// Gaps longer than [`FrameRatePolicy::drop_factor`] times the period, counted once each.
    pub dropped: u32,
    /// Median interval between two commits, in milliseconds.
    pub interval_p50_ms: f32,
    /// Ninety-fifth percentile interval, in milliseconds.
    pub interval_p95_ms: f32,
    /// Ninety-ninth percentile interval, in milliseconds.
    pub interval_p99_ms: f32,
    /// The longest interval in the window, in milliseconds.
    pub worst_interval_ms: f32,
    /// How many buffers were not free.
    pub acquire_misses: u32,
}

/// Replaces a target rate that cannot be used with the one the platform is paced at.
///
/// A rate that is zero, negative or not a number leaves the drop criterion with nothing to
/// measure against; the answer is the platform's own timer rather than a refusal, because a
/// frame-rate report that says "no target" is worth less than one that says which rate it
/// fell short of.
///
/// # Panics
///
/// Never.
pub fn normalize_target(target_hz: f32) -> f32 {
    if target_hz.is_finite() && target_hz > 0.0 {
        target_hz
    } else {
        DEFAULT_TARGET_HZ
    }
}

/// Replaces a drop factor that cannot judge a frame with the design's own.
///
/// A factor has to be finite and at least one. A factor below one would call an early frame a
/// missed deadline, and a factor that is not a number would make every comparison false and
/// report a run with no drops at all -- the one direction a drop count must not fail in, since
/// a criterion that cannot be violated is a criterion that has stopped being asserted. An
/// infinite factor is refused for the same reason: it is the same report by another route.
///
/// # Panics
///
/// Never.
pub fn normalize_drop_factor(drop_factor: f32) -> f32 {
    if drop_factor.is_finite() && drop_factor >= 1.0 {
        drop_factor
    } else {
        DROP_FACTOR
    }
}

/// The physical pixels a damage set declares.
///
/// The sum is what a case compares against the window's own area, so it saturates rather
/// than wrapping: a rectangle set that overflowed would otherwise report a small number for
/// a large repaint.
///
/// # Panics
///
/// Never.
pub fn damage_px(rects: &[RectI]) -> u64 {
    rects.iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.w).saturating_mul(u64::from(rect.h)))
    })
}

/// One interval in milliseconds, from a duration.
fn elapsed_ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1000.0
}

/// The nearest-rank percentile of a sorted interval list.
///
/// The rank is `ceil(p * n)` counted from one, which is the definition that makes the
/// ninetieth percentile of ten samples the ninth of them rather than the average of two: a
/// percentile of a latency budget has to be a value that was actually measured, or a case
/// would assert on a number no frame ever had.
///
/// `p` is rounded to `f32` before it is multiplied, and the rank is read off the product rather
/// than off an exact ratio. That is safe for the three fractions this module asks for -- the
/// rounding error of a half, a ninety-fifth and a ninety-ninth is smaller than half the spacing
/// of the product, so `ceil` names the rank the exact fraction names -- but it is a property of
/// those three, and a percentile added later is worth checking against it.
fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let count = sorted.len();
    let rank = (p.clamp(0.0, 1.0) * count as f32).ceil().max(1.0) as usize;
    sorted[rank.saturating_sub(1).min(count - 1)]
}
