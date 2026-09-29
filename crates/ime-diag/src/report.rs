//! The budget dashboard: a probe snapshot read against `docs/dev/budgets.json`.
//!
//! Responsibility: turn measurements into verdicts. Every metric is compared with
//! each threshold the budget document declares for it, at the percentile that
//! threshold's own statement names, and the report says *which* percentile was
//! missed rather than only that something was.
//!
//! # Where the thresholds come from
//!
//! From the budget document and nowhere else. [`BUDGET_BINDINGS`] names which
//! threshold governs which metric -- it never states a value -- so a threshold
//! cannot be restated here and drift from the document the budget gate validates
//! against the specification. A metric whose key the document does not carry is
//! reported without a budget instead of against an invented one, which is why a row
//! can read `-` in the budget column: the design fixes no threshold for
//! `raster_partial`, and two thresholds the cards fix are not in the document yet
//! ([`PENDING_KEYS`]).
//!
//! # Boundaries
//!
//! This module compares and formats. It reads no file and knows nothing about where
//! the snapshot or the document live; the command line that reads both and prints
//! the result is `xtask`'s `report`. It also never touches user input: a report is
//! built from durations and counters, and the only strings in it are the names the
//! contract fixes.

mod render;

#[cfg(test)]
mod tests;

use std::time::Duration;

use crate::probe::{
    COUNTER_COUNT, Counter, HistSnapshot, Metric, Percentile, ProbeSnapshot, Probes,
};

/// The key count below which a report says its sample is too small to conclude from.
///
/// A percentile of a handful of keystrokes is noise: five minutes of real typing is
/// the window the design asks for, and this is the floor below which the report says
/// so rather than presenting the numbers as if they meant something.
pub const MIN_KEYS: u64 = 500;

/// The fraction of lost samples above which a report says its sample is incomplete.
pub const MAX_LOST_RATIO: f64 = 0.1;

/// One row of [`BUDGET_BINDINGS`]: which threshold governs which metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The metric the threshold governs.
    pub metric: Metric,
    /// The percentile the threshold is stated at.
    pub percentile: Percentile,
    /// The dotted path of the threshold in the budget document.
    pub key: &'static str,
}

/// The thresholds that govern each metric, and the percentile each is stated at.
///
/// The keys are the budget document's own, and the values are read from it; this
/// table is the only place that says which metric a key belongs to. A key the
/// document does not carry is skipped, so this table may name a threshold before the
/// document states it -- which is what [`PENDING_KEYS`] records.
///
/// `latency_ms.key_to_present_p99_144hz` is deliberately absent: it is the same
/// budget as `key_to_present_p99` stated for a 144 Hz display, and a probe cannot
/// know which display it ran on. Judging both would fail a run on the stricter of two
/// thresholds the document offers as alternatives.
pub const BUDGET_BINDINGS: &[Binding] = &[
    Binding {
        metric: Metric::KeyToPresent,
        percentile: Percentile::P50,
        key: "latency_ms.key_to_present_p50",
    },
    Binding {
        metric: Metric::KeyToPresent,
        percentile: Percentile::P99,
        key: "latency_ms.key_to_present_p99",
    },
    Binding {
        metric: Metric::Decode,
        percentile: Percentile::P99,
        key: "latency_ms.decode_p99",
    },
    Binding {
        metric: Metric::Decode,
        percentile: Percentile::P999,
        key: "latency_ms.decode_p999",
    },
    Binding {
        metric: Metric::RasterFull,
        percentile: Percentile::P99,
        key: "latency_ms.raster_p99",
    },
    Binding {
        metric: Metric::FirstKeyToVisible,
        percentile: Percentile::P99,
        key: "latency_ms.first_key_to_visible_p99",
    },
    Binding {
        metric: Metric::Wakeup,
        percentile: Percentile::P99,
        key: "latency_ms.wakeup_p99",
    },
    Binding {
        metric: Metric::EventLoopKey,
        percentile: Percentile::P99,
        key: "latency_ms.event_loop_key_p99",
    },
];

/// Thresholds a card fixes but the budget document does not carry yet.
///
/// The wakeup latency is stated at 50µs by the UI-thread card and the `on_key_event`
/// total at 2ms by the key-routing card; neither is a row of the specification's
/// budget table, so neither is in the document the budget gate validates. The report
/// judges them the moment the document states them, and until then reports those two
/// metrics without a budget. A test keeps this list and the document from drifting
/// apart in either direction: a key here must be absent from the document, and a
/// bound key must be present unless it is listed here.
pub const PENDING_KEYS: &[&str] = &["latency_ms.wakeup_p99", "latency_ms.event_loop_key_p99"];

/// One threshold a metric is measured against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BudgetCheck {
    /// The metric this threshold governs.
    pub metric: Metric,
    /// The percentile the document states the threshold at.
    pub percentile: Percentile,
    /// The dotted path of the threshold in the budget document.
    pub key: &'static str,
    /// The threshold, in microseconds.
    pub limit_us: f64,
}

impl BudgetCheck {
    /// What `sample` amounts to against this threshold.
    ///
    /// A histogram with no samples has a percentile of zero and so passes every
    /// threshold; that is the right answer for the comparison, and the report's own
    /// note about the sample being too small is what stops the pass being read as a
    /// result.
    pub fn verdict(&self, sample: &HistSnapshot) -> Verdict {
        let measured = sample.percentile(self.percentile) as f64;
        if measured > self.limit_us {
            Verdict::Fail(self.percentile)
        } else {
            Verdict::Pass
        }
    }
}

/// The thresholds a report measures against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    checks: Vec<BudgetCheck>,
}

impl Limits {
    /// The thresholds `thresholds` states, kept to the keys a metric is bound to.
    ///
    /// `thresholds` is the budget document flattened to `(dotted path, value)` pairs
    /// with the values in **microseconds**, the unit a histogram records in. The
    /// document's own `latency_ms` section is stated in milliseconds, so the caller
    /// converts once, where it reads the document: the conversion belongs there,
    /// because here it would be a second statement of a unit the document's section
    /// name already carries.
    ///
    /// A key the document does not carry is skipped rather than defaulted, so an
    /// absent threshold reads as absent instead of as a pass.
    pub fn from_thresholds(thresholds: &[(&'static str, f64)]) -> Self {
        let checks = BUDGET_BINDINGS
            .iter()
            .filter_map(|binding| {
                let (_, limit_us) = thresholds.iter().find(|(key, _)| *key == binding.key)?;
                Some(BudgetCheck {
                    metric: binding.metric,
                    percentile: binding.percentile,
                    key: binding.key,
                    limit_us: *limit_us,
                })
            })
            .collect();
        Self { checks }
    }

    /// A report measured against nothing: every metric is reported without a budget.
    pub fn unbudgeted() -> Self {
        Self::default()
    }

    /// The thresholds governing `metric`, in the order they are declared.
    pub fn checks(&self, metric: Metric) -> impl Iterator<Item = &BudgetCheck> {
        self.checks
            .iter()
            .filter(move |check| check.metric == metric)
    }

    /// How many thresholds are declared.
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    /// Whether no threshold is declared at all.
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

/// What a metric's measurements amount to against its thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every declared threshold holds.
    Pass,
    /// The named percentile is past its threshold.
    Fail(Percentile),
    /// The document declares no threshold for this metric.
    Unbudgeted,
}

impl Verdict {
    /// The word the machine-readable document carries.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail(_) => "FAIL",
            Self::Unbudgeted => "UNBUDGETED",
        }
    }

    /// The label with the percentile a failure was found at, as the report's verdict
    /// column prints it.
    ///
    /// A metric the document declares nothing for prints a dash rather than a word,
    /// so that a row nobody has stated a threshold for cannot be read as a result.
    pub fn detail(self) -> String {
        match self {
            Self::Fail(percentile) => format!("FAIL({})", percentile.label()),
            Self::Unbudgeted => "-".to_owned(),
            Self::Pass => Self::Pass.label().to_owned(),
        }
    }
}

/// One metric's row in a report.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The metric.
    pub metric: Metric,
    /// What the probes measured.
    pub sample: HistSnapshot,
    /// The thresholds this metric is measured against, in declaration order.
    pub checks: Vec<BudgetCheck>,
    /// The verdict over all of them.
    pub verdict: Verdict,
}

impl Row {
    /// The verdict over `checks`, which is the lowest percentile that failed.
    ///
    /// The lowest rather than the first: the thresholds of one metric are stated at
    /// different percentiles, and a run that misses the median has told its reader
    /// more than one that misses the tail.
    fn judge(checks: &[BudgetCheck], sample: &HistSnapshot) -> Verdict {
        let mut failed: Option<Percentile> = None;
        for check in checks {
            if let Verdict::Fail(percentile) = check.verdict(sample) {
                failed = Some(failed.map_or(percentile, |current| current.min(percentile)));
            }
        }
        match failed {
            Some(percentile) => Verdict::Fail(percentile),
            None if checks.is_empty() => Verdict::Unbudgeted,
            None => Verdict::Pass,
        }
    }
}

/// A probe snapshot judged against a set of thresholds.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeReport {
    /// How long the probes had been running.
    pub sampled: Duration,
    /// How many input sessions the window covered.
    pub sessions: u64,
    /// How many key presses the window covered.
    pub keys: u64,
    /// One row per metric, in [`Metric::ALL`] order.
    pub rows: Vec<Row>,
    /// Every counter's value, indexed by [`Counter::index`].
    pub counters: [u64; COUNTER_COUNT],
    /// What a reader needs to know about the sample itself.
    pub notes: Vec<String>,
}

impl ProbeReport {
    /// Judges `snapshot` against `limits`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_diag::probe::{HistSnapshot, Metrics, ProbeSnapshot};
    /// use ime_diag::report::{Limits, ProbeReport, Verdict};
    ///
    /// let snapshot = ProbeSnapshot {
    ///     metrics: Metrics {
    ///         decode: HistSnapshot {
    ///             count: 100,
    ///             sum_us: 42_000,
    ///             p50_us: 420,
    ///             p90_us: 1_100,
    ///             p99_us: 2_400,
    ///             p999_us: 4_800,
    ///             max_us: 4_800,
    ///         },
    ///         ..Metrics::default()
    ///     },
    ///     ..ProbeSnapshot::default()
    /// };
    /// let limits = Limits::from_thresholds(&[("latency_ms.decode_p99", 3_000.0)]);
    /// assert_eq!(ProbeReport::compare(&snapshot, &limits).verdict(), Verdict::Pass);
    ///
    /// // The threshold is the document's, so a tighter one fails the same run.
    /// let tight = Limits::from_thresholds(&[("latency_ms.decode_p99", 100.0)]);
    /// assert!(matches!(
    ///     ProbeReport::compare(&snapshot, &tight).verdict(),
    ///     Verdict::Fail(_)
    /// ));
    /// ```
    pub fn compare(snapshot: &ProbeSnapshot, limits: &Limits) -> Self {
        let rows = Metric::ALL
            .into_iter()
            .map(|metric| {
                let sample = *metric.snapshot(&snapshot.metrics);
                let checks: Vec<BudgetCheck> = limits.checks(metric).copied().collect();
                let verdict = Row::judge(&checks, &sample);
                Row {
                    metric,
                    sample,
                    checks,
                    verdict,
                }
            })
            .collect();
        Self {
            sampled: snapshot.sampled,
            sessions: snapshot.sessions,
            keys: snapshot.keys,
            rows,
            counters: snapshot.counters,
            notes: notes_for(snapshot),
        }
    }

    /// The verdict over every metric.
    ///
    /// A pass only when at least one threshold was actually judged: a run whose
    /// document declares nothing is reported as unbudgeted rather than as a pass.
    pub fn verdict(&self) -> Verdict {
        let mut failed: Option<Percentile> = None;
        let mut budgeted = false;
        for row in &self.rows {
            match row.verdict {
                Verdict::Fail(percentile) => {
                    failed = Some(failed.map_or(percentile, |current| current.min(percentile)));
                }
                Verdict::Pass => budgeted = true,
                Verdict::Unbudgeted => {}
            }
        }
        match failed {
            Some(percentile) => Verdict::Fail(percentile),
            None if budgeted => Verdict::Pass,
            None => Verdict::Unbudgeted,
        }
    }

    /// Every threshold that was missed, one message each.
    ///
    /// One message per missed threshold rather than one per row, because a row can be
    /// judged at several percentiles and a reader fixing a regression needs to know
    /// which one moved.
    pub fn violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        for row in &self.rows {
            let unit = row.metric.unit();
            for check in &row.checks {
                let measured = row.sample.percentile(check.percentile);
                if measured as f64 > check.limit_us {
                    violations.push(format!(
                        "{} {} {} over {} ({})",
                        row.metric.name(),
                        check.percentile.label(),
                        unit.format(measured as f64),
                        unit.format(check.limit_us),
                        check.key
                    ));
                }
            }
        }
        violations
    }

    /// One counter's value.
    ///
    /// # Panics
    ///
    /// Never: the index comes from the counter itself.
    pub fn counter(&self, counter: Counter) -> u64 {
        self.counters[counter.index()]
    }
}

impl Probes {
    /// Judges everything this probe has measured against `limits`.
    pub fn report(&self, limits: &Limits) -> ProbeReport {
        ProbeReport::compare(&self.snapshot(), limits)
    }
}

/// What a reader needs to know about the sample behind a report.
fn notes_for(snapshot: &ProbeSnapshot) -> Vec<String> {
    let mut notes = Vec::new();
    if snapshot.keys < MIN_KEYS {
        notes.push(format!(
            "insufficient samples: {} keys, fewer than {MIN_KEYS}",
            snapshot.keys
        ));
    }
    let lost = snapshot.counter(Counter::ProbeLost);
    if lost > 0 && lost as f64 > snapshot.keys as f64 * MAX_LOST_RATIO {
        notes.push(format!(
            "incomplete samples: {lost} latency samples were lost"
        ));
    }
    notes
}
