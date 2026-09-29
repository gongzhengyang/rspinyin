//! The benchmark-budget gate: criterion output against the budget document.
//!
//! Responsibility: read the statistics a criterion run left behind, compare the
//! cases the design names against the thresholds `docs/dev/budgets.json` carries,
//! and report every case that is past its budget. The thresholds come from the
//! document and nowhere else -- this module names *which* threshold a case is
//! asserted against, never what it is -- so the benchmark and the spec cannot drift
//! apart without the validation in [`super`] noticing first.
//!
//! Criterion reports a mean and a spread rather than a percentile, and the budgets
//! are stated at P99, so a case is compared at `mean + 3 sigma`. The message says
//! `p99_est` rather than `p99` for that reason: it is an estimate of the percentile
//! built from the two numbers criterion publishes, not a measured one.
//!
//! Boundaries: this layer reads criterion's output directory and the budget
//! document, and writes nothing except the run metadata [`super::meta`] records. It
//! never runs a benchmark, and it never touches the spec, whose validation belongs
//! to [`super::validate`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{BINDINGS, BUDGETS_FILE, Budgets};

/// Directory criterion writes its output under, inside the target directory.
const CRITERION_DIR: &str = "criterion";

/// File criterion writes one case's statistics to.
const ESTIMATES_FILE: &str = "estimates.json";

/// Directory criterion writes the statistics of the last run to.
const LAST_RUN_DIR: &str = "new";

/// The cases the gate asserts, and the threshold each is measured against.
///
/// `decode/64byte` is deliberately absent. The decode budget is stated for an input
/// of at most twelve syllables, and the sixty-four-byte case spells twenty-six, so
/// asserting it against that budget would be asserting something the budget does not
/// claim; it is measured, reported as unasserted, and left to the day a budget
/// covers it. `segment/dag_build` and `lm/edge_score` are unasserted for the same
/// reason: the design names them as cases and states no threshold for either.
const CASE_BINDINGS: &[CaseBinding] = &[
    CaseBinding("decode/2syl", "latency_ms.decode_p99", BenchUnit::Millis),
    CaseBinding("decode/4syl", "latency_ms.decode_p99", BenchUnit::Millis),
    CaseBinding("decode/8syl", "latency_ms.decode_p99", BenchUnit::Millis),
    CaseBinding("decode/12syl", "latency_ms.decode_p99", BenchUnit::Millis),
    CaseBinding(
        "decode/holdout",
        "bench.decode_holdout_s",
        BenchUnit::Seconds,
    ),
    CaseBinding(
        "input/buffer_ops",
        "bench.input_buffer_ops_us",
        BenchUnit::Micros,
    ),
    CaseBinding(
        "ui/wakeup_latency",
        "bench.ui_wakeup_latency_us",
        BenchUnit::Micros,
    ),
    CaseBinding(
        "passthrough/classify",
        "bench.passthrough_classify_ns",
        BenchUnit::Nanos,
    ),
];

/// One benchmark case bound to the threshold it is asserted against.
///
/// Fields, in order: the case's criterion id (`<group>/<function>`), the dotted path
/// of the threshold in the budget document, and the unit that threshold is written
/// in. The path is a key and never a number: the value is read from the document, so
/// no threshold is written down twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaseBinding(&'static str, &'static str, BenchUnit);

/// The unit a case's threshold is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BenchUnit {
    /// Nanoseconds.
    Nanos,
    /// Microseconds.
    Micros,
    /// Milliseconds.
    Millis,
    /// Seconds.
    Seconds,
}

impl BenchUnit {
    /// How many nanoseconds one unit holds.
    fn nanos(self) -> f64 {
        match self {
            Self::Nanos => 1.0,
            Self::Micros => 1e3,
            Self::Millis => 1e6,
            Self::Seconds => 1e9,
        }
    }

    /// The unit's suffix, spelled the way the budget document's field names spell it.
    fn suffix(self) -> &'static str {
        match self {
            Self::Nanos => "ns",
            Self::Micros => "us",
            Self::Millis => "ms",
            Self::Seconds => "s",
        }
    }

    /// Renders a number of nanoseconds in this unit.
    pub(super) fn render(self, nanos: f64) -> String {
        format!("{}{}", nanos / self.nanos(), self.suffix())
    }
}

/// One case's statistics, as criterion left them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Measurement {
    /// Mean of one iteration, in nanoseconds.
    pub(super) mean: f64,
    /// Standard deviation of the samples, in nanoseconds.
    pub(super) std_dev: f64,
}

impl Measurement {
    /// The estimate the gate compares with the threshold: `mean + 3 sigma`.
    pub(super) fn p99(&self) -> f64 {
        self.mean + 3.0 * self.std_dev
    }
}

/// Outcome of a benchmark-budget check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BenchReport {
    /// Bound cases that were found and are inside their threshold.
    pub passed: Vec<String>,
    /// Bound cases that are past their threshold, one message each.
    pub violations: Vec<String>,
    /// Bound cases the criterion output does not carry.
    pub missing: Vec<String>,
    /// Cases the criterion output carries that no threshold is bound to.
    pub unbound: Vec<String>,
}

/// Compares a criterion output directory against a budget document.
///
/// `budgets` is the validated budget document and `dir` is the directory criterion
/// wrote its output to. `group` narrows the comparison to one criterion group
/// (`decode`, `input`, `passthrough`, ...); `None` compares every bound case. The
/// directory is a parameter rather than something this function resolves for itself
/// so that a test can drive the comparison over a directory it wrote, which is what
/// makes the assertion verifiable without running a benchmark.
///
/// # Errors
/// Returns an error when the criterion output cannot be read, or when a binding names
/// a threshold the document does not carry -- the last one is a defect in this module
/// rather than in the documents, and it is reported rather than skipped.
pub(super) fn check(budgets: &Budgets, dir: &Path, group: Option<&str>) -> Result<BenchReport> {
    let estimates = read_estimates(dir)?;
    let mut report = BenchReport::default();

    for binding in CASE_BINDINGS {
        let CaseBinding(case, key, unit) = *binding;
        if group.is_some_and(|wanted| wanted != group_of(case)) {
            continue;
        }
        let Some(measurement) = estimates.get(case) else {
            report.missing.push((*case).to_owned());
            continue;
        };
        let Some(budget) = threshold(budgets, key) else {
            bail!("{case}: bound to {key}, which {BUDGETS_FILE} does not carry");
        };
        let limit = budget * unit.nanos();
        if measurement.p99() <= limit {
            report.passed.push((*case).to_owned());
        } else {
            report.violations.push(format!(
                "{} VIOLATED: {case} p99_est={} budget={}",
                owner(key),
                unit.render(measurement.p99()),
                unit.render(limit)
            ));
        }
    }

    for case in estimates.keys() {
        if !CASE_BINDINGS
            .iter()
            .any(|binding| binding.0 == case.as_str())
        {
            report.unbound.push(case.clone());
        }
    }
    Ok(report)
}

/// The directory criterion writes its output to.
///
/// The order is the one criterion resolves for itself: `$CRITERION_HOME` when the
/// environment sets it, and otherwise `criterion/` inside the target directory. A
/// run whose output went somewhere else is reported as carrying no case at all,
/// which is what makes the two resolutions having to agree a visible failure rather
/// than a silent pass.
pub(super) fn criterion_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CRITERION_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => target_dir(root).join(CRITERION_DIR),
    }
}

/// The directory cargo builds into.
fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => root.join("target"),
    }
}

/// Reads every case's statistics out of a criterion output directory.
///
/// The layout is criterion's own: one directory per group, one per case, and the
/// statistics of the last run under `new/estimates.json`. A case that was never run
/// has no file, which is what [`BenchReport::missing`] reports.
fn read_estimates(dir: &Path) -> Result<BTreeMap<String, Measurement>> {
    let mut estimates = BTreeMap::new();
    let groups = fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    for group in groups {
        let group = group.with_context(|| format!("cannot read {}", dir.display()))?;
        if !group.file_type()?.is_dir() {
            continue;
        }
        let group_name = group.file_name().to_string_lossy().into_owned();
        for case in fs::read_dir(group.path())? {
            let case = case?;
            if !case.file_type()?.is_dir() {
                continue;
            }
            let path = case.path().join(LAST_RUN_DIR).join(ESTIMATES_FILE);
            if !path.is_file() {
                continue;
            }
            let text = fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let name = format!("{group_name}/{}", case.file_name().to_string_lossy());
            let measurement = parse_estimates(&text)
                .with_context(|| format!("{}: unusable statistics", path.display()))?;
            estimates.insert(name, measurement);
        }
    }
    Ok(estimates)
}

/// Reads the mean and the standard deviation out of one `estimates.json`.
pub(super) fn parse_estimates(text: &str) -> Result<Measurement> {
    let value: Value = serde_json::from_str(text).context("not valid JSON")?;
    Ok(Measurement {
        mean: point_estimate(&value, "mean")?,
        std_dev: point_estimate(&value, "std_dev")?,
    })
}

/// Reads `"<key>": {"point_estimate": <number>}` out of a criterion document.
fn point_estimate(value: &Value, key: &str) -> Result<f64> {
    value
        .get(key)
        .and_then(|estimate| estimate.get("point_estimate"))
        .and_then(Value::as_f64)
        .with_context(|| format!("`{key}.point_estimate` is missing or is not a number"))
}

/// The group a criterion id belongs to: the part before the first `/`.
fn group_of(case: &str) -> &str {
    case.split_once('/').map_or(case, |(group, _)| group)
}

/// The value of one threshold, or `None` when the document does not carry the key.
pub(super) fn threshold(budgets: &Budgets, key: &str) -> Option<f64> {
    budgets
        .thresholds()
        .into_iter()
        .find(|entry| entry.0 == key)
        .map(|entry| entry.1)
}

/// The spec cell that owns a threshold, as the binding table names it.
pub(super) fn owner(key: &str) -> &'static str {
    BINDINGS
        .iter()
        .find(|binding| binding.0 == key)
        .map_or("", |binding| binding.1)
}
