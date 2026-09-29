//! The histogram the wakeup benchmark records its per-delivery samples in.
//!
//! The acceptance criterion states how the wakeup budget is measured: the poster records the
//! instant it hands a command over, the UI thread reads the same clock the moment it is
//! woken, and the difference goes into a histogram. This is that histogram.
//!
//! # Why fixed buckets
//!
//! The only question asked of it is one percentile over a range the budget already bounds:
//! the ceiling is fifty microseconds, and a delivery that takes a millisecond is a stall
//! rather than a measurement. A fixed array of one-microsecond buckets answers that in
//! constant memory, which a vector of samples cannot: criterion chooses the iteration count
//! at run time, so a vector would grow with however long the run happens to take.
//!
//! A general-purpose histogram crate would answer it too, and is what this project reaches
//! for by default (`AGENTS.md` 3.5). It is deliberately not used here: it would enter the
//! dependency closure of a benchmark-only probe to provide recording and a percentile query
//! that are forty lines below, and its default features carry a serialization format this
//! measurement has no use for. The one property it would add and this does not have --
//! unbounded precision -- is not wanted: a bucket edge worth two percent of the budget is
//! finer than the run-to-run spread of any wakeup measurement.
//!
//! # What it reports
//!
//! [`LatencyHistogram::p99`] is the upper edge of the bucket the ninety-ninth percentile
//! sample fell in, so it over-reports by at most one bucket. Samples past the last bucket
//! land in an overflow count, and when the overflow holds more than one percent of the
//! samples the percentile is reported *past* the window rather than inside it. A caller that
//! asserts the percentile is inside the window therefore fails on a run whose tail is a
//! stall, instead of quietly reporting a percentile of the deliveries that did arrive.

use std::time::Duration;

/// Width of one bucket, in nanoseconds.
///
/// One microsecond is the finest resolution the budget needs: it is two percent of the
/// fifty-microsecond ceiling the wakeup is held to.
const BUCKET_NANOS: u64 = 1_000;

/// How many buckets the histogram holds.
///
/// Four thousand and ninety-six buckets cover four milliseconds, two orders of magnitude
/// past the budget. A delivery that lands outside that is a stopped UI thread rather than a
/// slow wakeup, and the percentile reports it as such.
const BUCKETS: usize = 4_096;

/// A histogram of delivery latencies, in fixed one-microsecond buckets.
///
/// # Concurrency
///
/// `Send` and `Sync`: it is plain counters with no interior mutability, so a caller supplies
/// whatever synchronisation it needs -- the benchmark puts it behind a mutex, because the
/// surface records into it while the host side reads it. No method blocks.
#[derive(Debug)]
pub struct LatencyHistogram {
    /// Samples per bucket; a sample of `n` nanoseconds lands in bucket `n / BUCKET_NANOS`.
    buckets: [u32; BUCKETS],
    /// Samples slower than the last bucket.
    overflow: u32,
    /// Every sample recorded, the overflow ones included.
    samples: u64,
}

impl LatencyHistogram {
    /// The widest latency the buckets resolve, as a duration.
    pub const WINDOW: Duration = Duration::from_nanos(BUCKETS as u64 * BUCKET_NANOS);

    /// Creates an empty histogram.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Self {
        Self {
            buckets: [0; BUCKETS],
            overflow: 0,
            samples: 0,
        }
    }

    /// Records one sample.
    ///
    /// A sample at or past [`LatencyHistogram::WINDOW`] is counted as an overflow rather than
    /// dropped, so the sample count stays the number of deliveries that were made.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn record(&mut self, sample: Duration) {
        self.samples = self.samples.saturating_add(1);
        let nanos = u64::try_from(sample.as_nanos()).unwrap_or(u64::MAX);
        let bucket = usize::try_from(nanos / BUCKET_NANOS).unwrap_or(usize::MAX);
        match self.buckets.get_mut(bucket) {
            Some(count) => *count = count.saturating_add(1),
            None => self.overflow = self.overflow.saturating_add(1),
        }
    }

    /// How many samples have been recorded.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// The ninety-ninth percentile, as the upper edge of the bucket it fell in.
    ///
    /// Returns `None` for a histogram with no samples: a percentile of nothing is not zero
    /// latency, it is no measurement at all, and a caller has to tell the two apart.
    ///
    /// Returns a latency *past* [`LatencyHistogram::WINDOW`] when the overflow bucket holds
    /// enough samples to move the percentile, which is how a stalled run is reported as a
    /// stall rather than as a percentile of the deliveries that arrived.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn p99(&self) -> Option<Duration> {
        if self.samples == 0 {
            return None;
        }
        // The number of samples the percentile has to cover: ceil(0.99 * samples), written
        // as a subtraction so no rounding step can lose a sample.
        let target = self.samples - self.samples / 100;
        let mut seen = 0u64;
        for (index, count) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(u64::from(*count));
            if seen >= target {
                return Some(Duration::from_nanos((index as u64 + 1) * BUCKET_NANOS));
            }
        }
        Some(Self::WINDOW + Duration::from_nanos(BUCKET_NANOS))
    }
}

impl Default for LatencyHistogram {
    /// Creates an empty histogram, the same as [`LatencyHistogram::new`].
    fn default() -> Self {
        Self::new()
    }
}
