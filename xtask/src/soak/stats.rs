//! The statistics a report's sample series comes to.
//!
//! Responsibility: turn the readings a run took into the handful of numbers a robustness
//! budget is stated about -- the envelope the resident set moved in, the same envelope over
//! the steady window, and the straight line through the steady window.
//!
//! # Why an envelope rather than a difference of two readings
//!
//! "The last reading minus the first" answers a question nobody asked: it is zero for a
//! process that climbed and came back, and it is whatever the warm-up happened to leave
//! behind for one that settled. The envelope -- the highest reading minus the lowest -- is
//! the total range the process moved in, which is what "no memory growth" is a claim about,
//! and it cannot be made small by choosing a different baseline.
//!
//! # Why the warm-up is excluded, and why it is short
//!
//! The first minute of a run is not steady state: the dictionary's mapping is being paged
//! in, the renderer allocates its first buffers, and the user store loads its counts. Those
//! costs are real and they are governed -- `memory_mb.plugin_rss` is a growth from a
//! baseline and `memory_mb.dict_mmap_rss` is the mapping's own budget -- but they are not
//! *drift*, which is what this statistic is about. The warm-up is therefore small and
//! fixed: excluding a large part of the run would hide a leak that took hold early, which
//! is the failure the budget exists to catch.
//!
//! # Why the slope is reported and not asserted
//!
//! A rising line and a bounded envelope are different findings, and the budget states one
//! of them: the drift a run may show. Judging the slope would mean inventing a rate the
//! document does not state, so the slope is printed, kept in the summary, and left for a
//! person to read next to the envelope. A steep line under a flat envelope is a process
//! that grew and was trimmed; a flat line under a wide envelope is churn.
//!
//! # What lives where
//!
//! Everything here is arithmetic over numbers that have already been read: this module
//! opens no file, reads no clock and knows no threshold. Its input is a slice of
//! [`Sample`] and a warm-up, which is what lets every case below be driven from a series
//! written out in the test rather than from a run.

use super::plan::SECONDS_PER_HOUR;
use super::report::Sample;

/// The resident-set statistics of one sample series.
///
/// Every field is in kibibytes, the unit `/proc` states its counters in; the budget
/// document states its ceiling in mebibytes and the conversion between the two happens
/// where that ceiling is read, not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RssStats {
    /// How many readings carried a resident set size.
    pub readings: u64,
    /// The first reading.
    pub first_kb: u64,
    /// The last reading.
    pub last_kb: u64,
    /// The lowest reading of the whole run.
    pub min_kb: u64,
    /// The highest reading of the whole run.
    pub max_kb: u64,
    /// The highest reading minus the lowest, over the whole run.
    pub envelope_kb: u64,
    /// How many readings fall inside the steady window.
    pub steady_readings: u64,
    /// The lowest reading of the steady window.
    pub steady_min_kb: u64,
    /// The highest reading of the steady window.
    pub steady_max_kb: u64,
    /// The highest reading of the steady window minus the lowest.
    ///
    /// This is the number a robustness budget is judged against, and it is the only one of
    /// the three differences here that a run cannot make small by starting at a different
    /// moment.
    pub steady_envelope_kb: u64,
    /// The least-squares slope of the steady window, in kibibytes per hour.
    ///
    /// Signed, because a process that gave memory back is worth seeing, and reported
    /// rather than asserted; see the module documentation.
    pub slope_kib_per_hour: f64,
    /// The same envelope as [`Self::envelope_kb`] over the anonymous resident part of the
    /// mappings, when every reading of the run carried one.
    ///
    /// This is the leak-shaped number: it excludes the dictionary's read-only mapping,
    /// whose pages the kernel may drop and fault back in. A wide resident envelope under a
    /// flat anonymous one is page-cache churn rather than growth.
    pub anonymous_envelope_kb: Option<u64>,
}

impl RssStats {
    /// The statistics `samples` come to, or `None` when they cannot describe a run.
    ///
    /// `warmup_s` is how many seconds at the start are outside the steady window. `None`
    /// when fewer than two readings carry a resident set size -- one reading is not an
    /// envelope -- and when fewer than two fall inside the steady window, which is the same
    /// refusal for the number the budget is about. A run whose statistics are absent is
    /// reported as such rather than as a drift of zero: "nothing was measured" and "nothing
    /// moved" are different findings, and only one of them is a pass.
    pub fn of(samples: &[Sample], warmup_s: f64) -> Option<Self> {
        let readings: Vec<Sample> = samples
            .iter()
            .filter(|sample| sample.measured())
            .copied()
            .collect();
        if readings.len() < 2 {
            return None;
        }
        let steady: Vec<Sample> = readings
            .iter()
            .copied()
            .filter(|sample| sample.t_s >= warmup_s)
            .collect();
        if steady.len() < 2 {
            return None;
        }
        let (min_kb, max_kb) = extent(readings.iter().map(rss))?;
        let (steady_min_kb, steady_max_kb) = extent(steady.iter().map(rss))?;
        let points: Vec<(f64, u64)> = steady
            .iter()
            .map(|sample| (sample.t_s, rss(sample)))
            .collect();
        let anonymous: Option<Vec<u64>> = readings
            .iter()
            .map(|sample| sample.anonymous_kb)
            .collect::<Option<Vec<u64>>>();
        Some(Self {
            readings: readings.len() as u64,
            first_kb: rss(readings.first()?),
            last_kb: rss(readings.last()?),
            min_kb,
            max_kb,
            envelope_kb: max_kb - min_kb,
            steady_readings: steady.len() as u64,
            steady_min_kb,
            steady_max_kb,
            steady_envelope_kb: steady_max_kb - steady_min_kb,
            slope_kib_per_hour: slope_kib_per_hour(&points),
            anonymous_envelope_kb: anonymous.and_then(|values| {
                let (min, max) = extent(values.iter().copied())?;
                Some(max - min)
            }),
        })
    }

    /// The lines a report prints for these statistics.
    ///
    /// The shape is stable: the resident envelope and the steady envelope first, because
    /// they are what a budget is stated about, then the first and last reading, the slope
    /// marked as the diagnostic it is, and the anonymous envelope last.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "rss envelope: {}KiB over {} readings (min {} max {})",
                self.envelope_kb, self.readings, self.min_kb, self.max_kb
            ),
            format!(
                "rss steady envelope: {}KiB over {} readings (min {} max {})",
                self.steady_envelope_kb,
                self.steady_readings,
                self.steady_min_kb,
                self.steady_max_kb
            ),
            format!("rss first {}KiB last {}KiB", self.first_kb, self.last_kb),
            format!(
                "rss steady slope: {:.2}KiB/h (diagnostic, not asserted)",
                self.slope_kib_per_hour
            ),
        ];
        match self.anonymous_envelope_kb {
            Some(envelope) => lines.push(format!(
                "anonymous envelope: {envelope}KiB (the leak-shaped number)"
            )),
            None => lines.push(
                "anonymous envelope: none, some reading of the run did not carry one".to_owned(),
            ),
        }
        lines
    }
}

/// The resident set size of a reading that carries one.
///
/// The caller filters to measured readings first, so the fallback is unreachable; it exists
/// because a missing counter is not a number and this module may not panic on one.
fn rss(sample: &Sample) -> u64 {
    sample.rss_kb.unwrap_or_default()
}

/// The lowest and highest of a series, or `None` when it is empty.
fn extent(values: impl Iterator<Item = u64>) -> Option<(u64, u64)> {
    values.fold(None, |extent, value| match extent {
        None => Some((value, value)),
        Some((min, max)) => Some((min.min(value), max.max(value))),
    })
}

/// The least-squares slope through `points`, in kibibytes per hour.
///
/// Zero when every point shares one instant, which no run produces -- samples are taken a
/// fixed interval apart -- and which is answered rather than divided by.
fn slope_kib_per_hour(points: &[(f64, u64)]) -> f64 {
    let count = points.len() as f64;
    let sum_t: f64 = points.iter().map(|(t, _)| t).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| *y as f64).sum();
    let sum_tt: f64 = points.iter().map(|(t, _)| t * t).sum();
    let sum_ty: f64 = points.iter().map(|(t, y)| t * *y as f64).sum();
    let denominator = count * sum_tt - sum_t * sum_t;
    if denominator == 0.0 {
        return 0.0;
    }
    (count * sum_ty - sum_t * sum_y) / denominator * SECONDS_PER_HOUR
}
