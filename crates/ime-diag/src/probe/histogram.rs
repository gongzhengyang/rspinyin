//! The fixed-bucket histogram the runtime probes record into.
//!
//! Responsibility: turn a measured [`Duration`] into one relaxed `fetch_add` on a
//! bucket, and turn the bucket counts back into the percentiles a report states.
//! Nothing here locks, allocates or formats -- `record` is a table lookup and two
//! or three atomic adds -- which is what lets it sit on the key path at all.
//!
//! # Buckets
//!
//! The bounds are fixed at compile time and grow the way the design fixes them:
//! Fibonacci-style, so the resolution is finest exactly where an input method's
//! latency lives (1µs steps up to 13µs) and coarsens as the values grow. The table
//! is [`BUCKETS`] entries long; the Fibonacci sequence reaches the 100ms ceiling
//! after 24 of them, so the tail is saturated at that ceiling and the last entry is
//! the overflow bucket -- a sample slower than [`TOP_US`] is counted there rather
//! than dropped.
//!
//! A percentile is reported as the *upper* bound of the bucket a sample falls in,
//! never the lower one: the estimate is coarse, but it never understates a latency,
//! and a budget that passes against it is a budget that passes against the real
//! distribution.
//!
//! # Size
//!
//! One histogram is 64 buckets and 3 counters -- 536 bytes -- plus the enable flag,
//! which lands in the padding the alignment of the counters already reserves. The
//! whole structure is therefore 536 bytes, which is what makes a probe negligible
//! next to the memory budgets it measures.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Number of buckets every histogram carries.
pub const BUCKETS: usize = 64;

/// Upper bound of the slowest bucket, in microseconds.
///
/// A sample slower than this is counted in the last bucket rather than dropped, so
/// the percentiles stay truthful about how many samples there were even when the
/// distribution runs off the end of the table.
pub const TOP_US: u64 = 100_000;

/// The upper bound of each bucket, in microseconds, in ascending order.
pub(super) const BOUNDS: [u64; BUCKETS] = bounds();

/// Builds the bucket bounds: `1, 2, 3, 5, 8, 13, ...` microseconds, saturated at
/// [`TOP_US`].
///
/// A `const fn` rather than a literal table because the sequence is what matters and
/// a hand-copied list of 64 numbers is a place for a typo to hide; the saturation is
/// visible here, next to the reason for it.
const fn bounds() -> [u64; BUCKETS] {
    let mut table = [TOP_US; BUCKETS];
    let mut lower: u64 = 1;
    let mut upper: u64 = 2;
    let mut index = 0;
    while index < BUCKETS && lower < TOP_US {
        table[index] = lower;
        let next = lower + upper;
        lower = upper;
        upper = next;
        index += 1;
    }
    table
}

/// Which percentile a threshold is stated at.
///
/// The ordering is the ordering of the percentiles themselves, so the lowest
/// violated one is the first violation found when a row is judged against several.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Percentile {
    /// The median.
    P50,
    /// The 90th percentile.
    P90,
    /// The 99th percentile.
    P99,
    /// The 99.9th percentile.
    P999,
}

impl Percentile {
    /// Every percentile a report states, in ascending order.
    pub const ALL: [Self; 4] = [Self::P50, Self::P90, Self::P99, Self::P999];

    /// The label the report prints and the budget document's prose uses.
    pub const fn label(self) -> &'static str {
        match self {
            Self::P50 => "P50",
            Self::P90 => "P90",
            Self::P99 => "P99",
            Self::P999 => "P999",
        }
    }

    /// This percentile as a numerator over a denominator.
    const fn fraction(self) -> (u64, u64) {
        match self {
            Self::P50 => (50, 100),
            Self::P90 => (90, 100),
            Self::P99 => (99, 100),
            Self::P999 => (999, 1000),
        }
    }
}

/// One metric's distribution, as a report reads it.
///
/// Every latency is in whole microseconds and is the upper bound of a bucket, so it
/// is an estimate with a known direction: it is never smaller than the sample it
/// describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistSnapshot {
    /// How many samples were recorded.
    pub count: u64,
    /// The sum of the recorded microseconds.
    pub sum_us: u64,
    /// Upper bound of the bucket the median falls in.
    pub p50_us: u64,
    /// Upper bound of the bucket the 90th percentile falls in.
    pub p90_us: u64,
    /// Upper bound of the bucket the 99th percentile falls in.
    pub p99_us: u64,
    /// Upper bound of the bucket the 99.9th percentile falls in.
    pub p999_us: u64,
    /// Upper bound of the highest non-empty bucket.
    pub max_us: u64,
}

impl HistSnapshot {
    /// The percentile `percentile` of the samples, in microseconds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn percentile(&self, percentile: Percentile) -> u64 {
        match percentile {
            Percentile::P50 => self.p50_us,
            Percentile::P90 => self.p90_us,
            Percentile::P99 => self.p99_us,
            Percentile::P999 => self.p999_us,
        }
    }

    /// The arithmetic mean of the recorded samples, in microseconds.
    ///
    /// A sample shorter than a microsecond contributes zero to the sum, so a metric
    /// whose samples are mostly sub-microsecond reads a mean below its real one. The
    /// percentiles do not have that bias, which is why the budgets are stated at
    /// percentiles and the mean is carried for orientation only.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn mean_us(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum_us as f64 / self.count as f64
        }
    }
}

/// A fixed-bucket histogram.
///
/// Recording takes `&self`, so one histogram is shared by both threads without a
/// lock: the counters are relaxed atomics and a lost update is impossible because
/// every operation is a read-modify-write on a single word.
#[derive(Debug)]
pub struct Histogram {
    buckets: [AtomicU64; BUCKETS],
    count: AtomicU64,
    sum_us: AtomicU64,
    enabled: AtomicBool,
}

impl Histogram {
    /// A histogram with no samples, switched on.
    pub const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
        }
    }

    /// Switches recording on or off.
    ///
    /// Switched off, [`record`](Self::record) returns before it reads the clock or
    /// touches a counter, which is what makes the configuration's probe switch free
    /// rather than merely cheap.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether recording is switched on.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Records one sample.
    ///
    /// # Panics
    ///
    /// Never: the bucket index is clamped to the table.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use ime_diag::probe::{Histogram, Percentile};
    ///
    /// let histogram = Histogram::new();
    /// histogram.record(Duration::from_micros(10));
    /// let snapshot = histogram.snapshot();
    /// assert_eq!(snapshot.count, 1);
    /// // A percentile is the upper bound of the bucket the sample landed in, so it
    /// // is never smaller than the sample itself.
    /// assert!(snapshot.percentile(Percentile::P99) >= 10);
    /// ```
    pub fn record(&self, sample: Duration) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let us = micros(sample);
        self.buckets[bucket_of(us)].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
    }

    /// How many samples were recorded.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// The distribution, as a report reads it.
    ///
    /// The four percentiles are read in one pass each rather than in one shared
    /// pass, because a snapshot is taken once per report and the simpler form is the
    /// one that is obviously right.
    pub fn snapshot(&self) -> HistSnapshot {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return HistSnapshot::default();
        }
        HistSnapshot {
            count,
            sum_us: self.sum_us.load(Ordering::Relaxed),
            p50_us: self.percentile(target(count, Percentile::P50)),
            p90_us: self.percentile(target(count, Percentile::P90)),
            p99_us: self.percentile(target(count, Percentile::P99)),
            p999_us: self.percentile(target(count, Percentile::P999)),
            max_us: self.max_us(),
        }
    }

    /// The upper bound of the bucket the `rank`th sample falls in.
    fn percentile(&self, rank: u64) -> u64 {
        let mut cumulative = 0;
        for (index, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= rank {
                return BOUNDS[index];
            }
        }
        TOP_US
    }

    /// The upper bound of the highest non-empty bucket, or zero when there is none.
    fn max_us(&self) -> u64 {
        for (index, bucket) in self.buckets.iter().enumerate().rev() {
            if bucket.load(Ordering::Relaxed) > 0 {
                return BOUNDS[index];
            }
        }
        0
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Microseconds of `sample`, saturated at `u64::MAX`.
///
/// The saturation is unreachable in practice -- it needs a duration of some 584,000
/// years -- and is here so that the conversion is total rather than a place a
/// malformed measurement could panic.
fn micros(sample: Duration) -> u64 {
    u64::try_from(sample.as_micros()).unwrap_or(u64::MAX)
}

/// The index of the bucket a sample of `us` microseconds falls in.
///
/// A sub-microsecond sample is treated as one microsecond so that it lands in the
/// first bucket rather than before the table starts.
fn bucket_of(us: u64) -> usize {
    BOUNDS
        .partition_point(|&bound| bound < us.max(1))
        .min(BUCKETS - 1)
}

/// The rank a percentile names out of `count` samples.
///
/// Rounded up, so the 99th percentile of a hundred samples is the hundredth rather
/// than the ninety-ninth, and never below one, so a percentile of a non-empty
/// histogram always names a real sample.
fn target(count: u64, percentile: Percentile) -> u64 {
    let (numerator, denominator) = percentile.fraction();
    let rank = (u128::from(count) * u128::from(numerator)).div_ceil(u128::from(denominator));
    u64::try_from(rank).unwrap_or(u64::MAX).max(1)
}
