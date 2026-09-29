//! The budget regression gate: what a run measured, against what the document states.
//!
//! Responsibility: turn measured numbers into verdicts against every threshold
//! `docs/dev/budgets.json` states, and turn a verdict that is past its ceiling -- or a ceiling
//! nobody measured -- into a failure of the gate rather than a line a reviewer has to notice.
//!
//! # Why this exists
//!
//! A threshold nobody asserts is a threshold nobody keeps. `xtask budget --validate` holds the
//! document against the authoritative table in the specification and says nothing about what a run
//! measured; this module is the other half. A regression past a ceiling is a gate failure rather
//! than a warning.
//!
//! # `Missing` is not `Pass`
//!
//! The discipline the rest of the module is built around. A key the document states and no
//! measurement reaches is [`Verdict::Missing`], and [`GateReport::enforce`] fails on it unless the
//! caller waives it by name -- a waiver being an explicit, printed statement that a budget is
//! unasserted rather than a silent absence. "Nothing was measured, therefore nothing is wrong" is
//! the reading this module exists to refuse: a gate that passes because it measured nothing is a
//! gate that has stopped working.
//!
//! # The ceilings are read, never restated
//!
//! [`compare`] walks the keys `Budgets::thresholds` produces and reads each ceiling out of the
//! document, so no ceiling is written down here. What this module does state is which *unit* the
//! document writes each key in -- a unit is not a number -- and the audit in the tests holds that
//! table against the document's key set in both directions.
//!
//! # Units meet here
//!
//! A measurement carries the unit it was taken in: criterion reports nanoseconds, a `/proc` counter
//! is kibibytes. [`Unit::scale_to`] is where that unit meets the one the document states the key
//! in, and it refuses to convert across dimensions: a nanosecond judged against a mebibyte ceiling
//! is [`Verdict::Missing`], never a pass.
//!
//! # The P99 estimate
//!
//! Criterion publishes a mean and a spread rather than a percentile, and the document's latency
//! ceilings are stated at P99. [`read_criterion`] therefore compares `mean + 3 sigma`, an
//! *estimate* of the percentile and not a measured one, and every report says so
//! ([`P99_ESTIMATE_NOTE`]).
//!
//! # The machine has to be idle
//!
//! A number taken while another build is running measures the other build as much as this code.
//! [`require_idle_machine`] refuses such a run before its numbers are judged at all.
//!
//! # Boundaries
//!
//! This module reads the budget document and a criterion output directory, and writes nothing. It
//! runs no benchmark, and it resolves no path of its own: the criterion directory is a parameter,
//! which is what lets a test drive the comparison against a tree it wrote.
//!
//! The tests live in a sibling file rather than inside this one: together they would be longer than
//! the file limit allows.

// Nothing here is reachable from `xtask`'s subcommand tree yet: the wiring that would call it lives
// in `xtask/src/main.rs` and `xtask/src/testd/mod.rs`, which are not this module's files. Until that
// lands every item is reported as dead code in a non-test build, and the attribute goes away with
// those lines.
//
// `unused_imports` is covered by the same reasoning: the `pub use` lines below are this module's
// surface, and a `pub use` in a *binary* crate is "unused" whenever nothing in the crate names it.
#![allow(dead_code, unused_imports)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::budget::{BUDGETS_FILE, Budgets, SIGMAS, owner};

#[cfg(test)]
mod tests;
mod unit;

pub use self::unit::Unit;
use self::unit::{KEY_UNITS, unit_of};

/// The note every report carries about how a criterion measurement is read.
pub const P99_ESTIMATE_NOTE: &str = "note: a measurement read from criterion is compared at \
     mean + 3 sigma, an estimate of the P99 the latency ceilings are stated at and not a measured \
     percentile";

/// The directory criterion writes the statistics of the last run to.
const LAST_RUN_DIR: &str = "new";

/// The file criterion writes one case's statistics to.
const ESTIMATES_FILE: &str = "estimates.json";

/// One measured value, tied to the budget key it is judged against.
///
/// `value` is stated in `unit`, and the two travel together because neither is meaningful alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// The dotted path of the threshold in the budget document this number is judged against.
    pub key: &'static str,
    /// What was measured.
    pub value: f64,
    /// The unit `value` is stated in.
    pub unit: Unit,
}

/// What judging one measurement against one ceiling came to.
///
/// `Pass` and `Fail` carry the same three numbers so that a report can print how close a passing key
/// came to its ceiling as readily as how far past it a failing one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    /// The measurement is inside the ceiling the document states.
    Pass {
        /// The ceiling, in the unit the document states the key in.
        budget: f64,
        /// The measurement, in the same unit.
        measured: f64,
        /// `measured / budget`.
        ratio: f64,
    },
    /// The measurement is past the ceiling the document states.
    Fail {
        /// The ceiling, in the unit the document states the key in.
        budget: f64,
        /// The measurement, in the same unit.
        measured: f64,
        /// `measured / budget`.
        ratio: f64,
    },
    /// The key carries no usable measurement.
    ///
    /// Either nothing was measured for it, or what was measured cannot be read against it: a unit of
    /// another dimension, or a number that is not finite. Neither is a pass, which is the whole point
    /// of the variant.
    Missing,
}

impl Verdict {
    /// Whether this verdict fails the gate.
    ///
    /// Only [`Self::Fail`] does. [`Self::Missing`] fails the gate too, through
    /// [`GateReport::enforce`] rather than through this predicate, because a waiver is the one thing
    /// that can excuse it and a waiver belongs at the call site that decided it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Fail { .. })
    }
}

/// Judges `measurements` against every key the budget document states.
///
/// The result holds one entry per key of `budgets`, in the document's own order, so a report built
/// from it is stable between runs and a key can never be absent from it. A key no measurement reaches
/// is [`Verdict::Missing`], and so is a key whose measurement cannot be read in the unit the document
/// states it in.
///
/// Several measurements may name one key -- `latency_ms.decode_p99` is the ceiling each of the four
/// decode cases is measured against -- and a ceiling is a ceiling for all of them, so the *highest*
/// measurement stands for the key.
///
/// # Panics
///
/// Never.
pub fn compare(measurements: &[Measurement], budgets: &Budgets) -> Vec<(&'static str, Verdict)> {
    budgets
        .thresholds()
        .iter()
        .map(|threshold| {
            (
                threshold.0,
                judge_key(measurements, threshold.0, threshold.1),
            )
        })
        .collect()
}

/// The measurements naming a key the budget document does not carry.
///
/// A number measured against a key nobody states is a verdict about nothing, and in practice it is a
/// binding table that drifted from the document rather than a real budget. The keys come back in the
/// order they were measured and without repeats.
///
/// # Panics
///
/// Never.
pub fn unbudgeted(measurements: &[Measurement], budgets: &Budgets) -> Vec<&'static str> {
    let keys: Vec<&'static str> = budgets
        .thresholds()
        .iter()
        .map(|threshold| threshold.0)
        .collect();
    let mut unbound: Vec<&'static str> = Vec::new();
    for measurement in measurements {
        let key = measurement.key;
        if !keys.contains(&key) && !unbound.contains(&key) {
            unbound.push(key);
        }
    }
    unbound
}

/// One criterion case, and the budget key its measurement is judged against.
///
/// Fields, in order: the case's criterion id (`<group>/<function>`) and the dotted path of the key.
/// Both are names and neither is a number, so this table cannot restate a ceiling. It is a parameter
/// of [`read_criterion`] rather than a constant of this module so that the one table which decides
/// which benchmark cases are asserted stays the one table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CriterionCase(pub &'static str, pub &'static str);

/// Reads the statistics of every bound criterion case out of a criterion output directory.
///
/// The layout is criterion's own: one directory per group, one per case, and the statistics of the
/// last run under `new/estimates.json`. Criterion states a mean and a standard deviation in
/// nanoseconds, so a measurement is `mean + 3 sigma` nanoseconds -- an *estimate* of the P99 the
/// document's latency ceilings are stated at, not a measured percentile.
///
/// A case with no output at all is not an error: it contributes no measurement, and [`compare`]
/// reports its key as [`Verdict::Missing`]. A directory that is not there at all is the same case,
/// which keeps a gate that ran no benchmark from failing for the wrong reason -- and from passing for
/// the wrong one, since every key it would have asserted comes back `Missing`. A file that *is* there
/// and is not shaped the way criterion writes one is reported rather than skipped.
///
/// # Errors
///
/// Returns [`GateError::CriterionUnreadable`] when an `estimates.json` that exists cannot be read,
/// and [`GateError::CriterionFormat`] when one does not state a `mean` and a `std_dev` as finite
/// numbers -- the refusal names the shape it found, so a criterion release that changes its document
/// is diagnosable from the message.
///
/// # Panics
///
/// Never.
pub fn read_criterion(dir: &Path, cases: &[CriterionCase]) -> Result<Vec<Measurement>, GateError> {
    let mut measurements = Vec::with_capacity(cases.len());
    for &CriterionCase(case, key) in cases {
        let path = estimate_path(dir, case);
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|source| GateError::CriterionUnreadable {
            path: path.clone(),
            source,
        })?;
        measurements.push(Measurement {
            key,
            value: parse_estimate(&text, &path)?,
            unit: Unit::Nanos,
        });
    }
    Ok(measurements)
}

/// Refuses a set of measurements taken while the machine was busy with something else.
///
/// A build or a test running beside the one being measured moves every latency number, and it moves
/// them in the direction that reads as a regression: the project's own record is a
/// `passthrough/classify` case that measured 511ns while other agents were building and 726ns on an
/// idle machine, a difference criterion reported as a regression in code that had not changed.
///
/// The count is a parameter rather than something this function reads, so that the one probe which
/// answers whether another build or test process is running -- outside this harness's own ancestry,
/// which is what keeps a harness from counting itself -- stays the one probe.
///
/// # Errors
///
/// Returns [`GateError::Contaminated`] when `concurrent_agents` is not zero.
///
/// # Panics
///
/// Never.
pub fn require_idle_machine(concurrent_agents: u32) -> Result<(), GateError> {
    if concurrent_agents == 0 {
        Ok(())
    } else {
        Err(GateError::Contaminated { concurrent_agents })
    }
}

/// The result of judging a set of measurements against the budget document.
///
/// This is the one place a verdict becomes a gate failure: [`Self::enforce`] is what a caller runs at
/// the end of a measurement pass, and it refuses to return while a budget is past its ceiling or
/// unmeasured.
#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    /// One entry per key the document states, in the document's own order.
    entries: Vec<(&'static str, Verdict)>,
    /// Measurements naming a key the document does not carry, in the order they were measured.
    unbudgeted: Vec<&'static str>,
}

impl GateReport {
    /// Judges `measurements` against every key of `budgets`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn judge(measurements: &[Measurement], budgets: &Budgets) -> Self {
        Self {
            entries: compare(measurements, budgets),
            unbudgeted: unbudgeted(measurements, budgets),
        }
    }

    /// The verdicts, one per key of the document, in the document's own order.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn entries(&self) -> &[(&'static str, Verdict)] {
        &self.entries
    }

    /// The measurements naming a key the document does not carry.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn unbudgeted(&self) -> &[&'static str] {
        &self.unbudgeted
    }

    /// The keys the document states that carry no usable measurement, in document order.
    ///
    /// A waived key is still listed here: the report says what was not measured, and the waiver says
    /// who accepted it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn missing(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter(|(_, verdict)| *verdict == Verdict::Missing)
            .map(|(key, _)| *key)
            .collect()
    }

    /// The lines a report prints, one per key of the document and one per unbound measurement.
    ///
    /// The shape is stable: `key: verdict`, the numbers in the unit the document states the key in,
    /// and the ratio `measured / budget`. A waived key is printed as waived rather than dropped, and
    /// the last line names the estimator a criterion measurement is read at.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lines(&self, waivers: &[&str]) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.entries.len() + self.unbudgeted.len() + 1);
        for (key, verdict) in &self.entries {
            lines.push(match *verdict {
                Verdict::Pass {
                    budget,
                    measured,
                    ratio,
                } => format!(
                    "{key}: {} of {} within budget (ratio {ratio:.2})",
                    amount(measured, key),
                    amount(budget, key)
                ),
                Verdict::Fail {
                    budget,
                    measured,
                    ratio,
                } => format!(
                    "{key}: {} is past the {} budget (ratio {ratio:.2})",
                    amount(measured, key),
                    amount(budget, key)
                ),
                Verdict::Missing if waivers.contains(key) => {
                    format!("{key}: waived and not measured; the waiver is explicit")
                }
                Verdict::Missing => {
                    format!("{key}: MISSING: the document states a budget and nothing measured it")
                }
            });
        }
        for key in &self.unbudgeted {
            lines.push(format!(
                "{key}: measured, but {BUDGETS_FILE} states no budget for it"
            ));
        }
        lines.push(P99_ESTIMATE_NOTE.to_owned());
        lines
    }

    /// Fails the gate unless every key of the document was judged and is inside its ceiling.
    ///
    /// `waivers` names the keys whose absence of a measurement the caller accepts. A waiver is the
    /// only way an unmeasured budget passes, and it is a deliberate statement at a call site rather
    /// than a default, which is what keeps [`Verdict::Missing`] from quietly becoming a pass.
    ///
    /// # Errors
    ///
    /// Returns [`GateError::Unbudgeted`] when a measurement names a key the document does not carry,
    /// [`GateError::OverBudget`] for the first key that is past its ceiling, and
    /// [`GateError::Unmeasured`] when keys the document states carry no measurement and are not
    /// waived.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn enforce(&self, waivers: &[&str]) -> Result<(), GateError> {
        if !self.unbudgeted.is_empty() {
            return Err(GateError::Unbudgeted {
                keys: self.unbudgeted.clone(),
            });
        }
        for (key, verdict) in &self.entries {
            let Verdict::Fail {
                budget,
                measured,
                ratio,
            } = *verdict
            else {
                continue;
            };
            return Err(GateError::OverBudget {
                key,
                owner: owner(key),
                // A key whose unit this module does not know is never a `Fail`: `compare` reports
                // it as `Missing`. The fallback exists because the message needs a label, not
                // because the case is reachable.
                unit: unit_of(key).map_or("", Unit::label),
                budget,
                measured,
                ratio,
            });
        }
        let unmeasured: Vec<&'static str> = self
            .missing()
            .into_iter()
            .filter(|key| !waivers.contains(key))
            .collect();
        if unmeasured.is_empty() {
            Ok(())
        } else {
            Err(GateError::Unmeasured { keys: unmeasured })
        }
    }
}

/// Everything the budget gate can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// A measurement is past the ceiling the document states for it.
    #[error(
        "{key} ({owner}): {measured} {unit} is past the {budget} {unit} budget (ratio {ratio:.2})"
    )]
    OverBudget {
        /// The dotted path of the budget.
        key: &'static str,
        /// The specification cell that owns the budget, e.g. `BUDGET-LAT-02`; empty only for
        /// a key no binding names, which the document validator refuses.
        owner: &'static str,
        /// The suffix of the unit both numbers are stated in.
        unit: &'static str,
        /// The ceiling the document states.
        budget: f64,
        /// What was measured.
        measured: f64,
        /// `measured / budget`.
        ratio: f64,
    },

    /// Budgets the document states that no measurement reached.
    #[error(
        "{} budget(s) the budget document states were never measured: {}; measure them, or name \
         them as waivers at the call site",
        keys.len(),
        keys.join(", ")
    )]
    Unmeasured {
        /// The dotted paths of the budgets nothing measured.
        keys: Vec<&'static str>,
    },

    /// Measurements naming a key the budget document does not carry.
    #[error(
        "{} measurement(s) name a key the budget document does not carry: {}",
        keys.len(),
        keys.join(", ")
    )]
    Unbudgeted {
        /// The keys the measurements named.
        keys: Vec<&'static str>,
    },

    /// A criterion `estimates.json` that exists could not be read.
    #[error("{path}: {source}")]
    CriterionUnreadable {
        /// The file the read was about.
        path: PathBuf,
        /// The failure the read reported.
        #[source]
        source: std::io::Error,
    },

    /// A criterion `estimates.json` is not shaped the way criterion writes one.
    #[error("{path}: {detail}")]
    CriterionFormat {
        /// The file that was read.
        path: PathBuf,
        /// What the document states instead, including the keys it does carry.
        detail: String,
    },

    /// The numbers were taken while other build or test processes were running.
    #[error(
        "{concurrent_agents} build or test process(es) were running outside this harness while \
         the numbers were taken, so they measure the load rather than this code; re-run the \
         measurement on an idle machine"
    )]
    Contaminated {
        /// The count the environment probe reported.
        concurrent_agents: u32,
    },
}

/// One key's verdict, against the ceiling the document states for it.
fn judge_key(measurements: &[Measurement], key: &'static str, budget: f64) -> Verdict {
    // A key whose unit this module does not know cannot be judged: a number compared against a
    // ceiling in an unknown unit is a guess, and a guess is not a verdict.
    let Some(unit) = unit_of(key) else {
        return Verdict::Missing;
    };
    let Some(measured) = highest(measurements, key, unit) else {
        return Verdict::Missing;
    };
    let ratio = ratio_of(measured, budget);
    if measured > budget {
        Verdict::Fail {
            budget,
            measured,
            ratio,
        }
    } else {
        Verdict::Pass {
            budget,
            measured,
            ratio,
        }
    }
}

/// The highest usable measurement naming `key`, read in `unit`.
///
/// `None` when no measurement names the key, and when none of the ones that do can be read against
/// it: a unit of another dimension is not a measurement of this budget, and a number that is not
/// finite is refused before it is ever compared.
fn highest(measurements: &[Measurement], key: &str, unit: Unit) -> Option<f64> {
    measurements
        .iter()
        .filter(|measurement| measurement.key == key && is_measurement(measurement.value))
        .filter_map(|measurement| {
            measurement
                .unit
                .scale_to(unit)
                .map(|scale| measurement.value * scale)
        })
        .reduce(f64::max)
}

/// Whether a number can be a measurement.
///
/// A duration or a size is finite and non-negative; anything else is a reader that misread its
/// source, and it is refused where it is read rather than judged against a ceiling.
fn is_measurement(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

/// `measured / budget`, with the zero ceilings the document states handled.
///
/// Two of the document's ceilings are exactly zero -- the idle redraw count and the idle polling
/// timer count -- and `0 / 0` is a `NaN` no report can print.
fn ratio_of(measured: f64, budget: f64) -> f64 {
    if budget > 0.0 {
        measured / budget
    } else if measured > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// A number with the suffix of the unit the document states its key in.
fn amount(value: f64, key: &str) -> String {
    match unit_of(key) {
        Some(unit) => format!("{value:.2}{}", unit.label()),
        None => format!("{value:.2}"),
    }
}

/// Where criterion writes one case's statistics under `dir`.
fn estimate_path(dir: &Path, case: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    for part in case.split('/') {
        path.push(part);
    }
    path.push(LAST_RUN_DIR);
    path.push(ESTIMATES_FILE);
    path
}

/// The `mean + SIGMAS * std_dev` estimate one `estimates.json` states, in nanoseconds.
fn parse_estimate(text: &str, path: &Path) -> Result<f64, GateError> {
    let value: Value = serde_json::from_str(text).map_err(|error| GateError::CriterionFormat {
        path: path.to_path_buf(),
        detail: format!("not valid JSON: {error}"),
    })?;
    let mean = point_estimate(&value, "mean", path)?;
    let std_dev = point_estimate(&value, "std_dev", path)?;
    let estimate = mean + SIGMAS * std_dev;
    if !is_measurement(estimate) {
        return Err(GateError::CriterionFormat {
            path: path.to_path_buf(),
            detail: format!("mean + {SIGMAS} sigma gives {estimate}, which is not a duration"),
        });
    }
    Ok(estimate)
}

/// Reads `"<key>": {"point_estimate": <number>}` out of a criterion document.
fn point_estimate(value: &Value, key: &str, path: &Path) -> Result<f64, GateError> {
    value
        .get(key)
        .and_then(|estimate| estimate.get("point_estimate"))
        .and_then(Value::as_f64)
        .filter(|estimate| estimate.is_finite())
        .ok_or_else(|| GateError::CriterionFormat {
            path: path.to_path_buf(),
            detail: format!(
                "`{key}.point_estimate` is missing or is not a number; the document's keys are {}",
                top_level_keys(value)
            ),
        })
}

/// The keys a criterion document carries, as a refusal that names them prints them.
fn top_level_keys(value: &Value) -> String {
    match value.as_object() {
        Some(object) => {
            let keys: Vec<&str> = object.keys().map(|key| key.as_str()).collect();
            keys.join(", ")
        }
        None => "none: the document is not an object".to_owned(),
    }
}
