//! The benchmark-budget gate: criterion output against the budget document.
//!
//! Responsibility: read the statistics a criterion run left behind, compare the
//! cases the design names against the thresholds `docs/dev/budgets.json` carries,
//! and report every case that is past its budget. The thresholds come from the
//! document and nowhere else -- this module names *which* threshold a case is
//! asserted against, never what it is -- so the benchmark and the spec cannot drift
//! apart without the validation in [`super`] noticing first.
//!
//! Two properties of a benchmark are asserted over its sources rather than over its
//! output, by the audit at the end of this file: that it takes its inputs from the
//! binary and not from the machine, and that the case names it declares are the ones
//! this gate binds thresholds to. Neither is visible in a criterion document, and a
//! run that measured the wrong machine or published a case under a new name produces
//! a plausible one all the same.
//!
//! Every case the design names has to have produced a number, whether or not a
//! threshold is bound to it. Three of the nine -- the sixty-four-byte decode, the
//! segmentation graph and the language-model edge score -- are measured and not
//! asserted, because the design states no threshold for any of them. Requiring their
//! output is what keeps "the run measured all nine" a claim the gate checks rather
//! than one a report makes: a case that quietly stopped being registered would
//! otherwise pass, having nothing bound to it.
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

/// Standard deviations a criterion measurement is read at.
///
/// Criterion publishes a mean and a spread rather than a percentile, and the budget
/// document states its ceilings at P99, so `mean + SIGMAS * std_dev` is the estimator
/// that closes the gap between the two. It is defined here and read from here by every
/// reader of a criterion document -- this gate and the regression gate in
/// `crate::testd::budget_gate` -- so the two cannot come to different verdicts about
/// the same run.
pub(crate) const SIGMAS: f64 = 3.0;

/// The cases the gate asserts, and the threshold each is measured against.
///
/// `decode/64byte` is deliberately absent. The decode budget is stated for an input
/// of at most twelve syllables, and the sixty-four-byte case spells twenty-six, so
/// asserting it against that budget would be asserting something the budget does not
/// claim; it is measured, reported as unasserted, and left to the day a budget
/// covers it. `segment/dag_build` and `lm/edge_score` are unasserted for the same
/// reason: the design names them as cases and states no threshold for either.
///
/// The three are still required to produce a number; see [`DESIGN_CASES`].
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
    CaseBinding("transport/post_frame", "bench.post_ui_ns", BenchUnit::Nanos),
];

/// One benchmark case bound to the threshold it is asserted against.
///
/// Fields, in order: the case's criterion id (`<group>/<function>`), the dotted path
/// of the threshold in the budget document, and the unit that threshold is written
/// in. The path is a key and never a number: the value is read from the document, so
/// no threshold is written down twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaseBinding(&'static str, &'static str, BenchUnit);

/// The benchmark cases the design names, whether or not a threshold is bound to them.
///
/// This is the case set the acceptance criterion is stated for, and it is a table of
/// *names*: a case that is bound to a threshold appears here too, so the two tables
/// cannot disagree about which cases a run has to produce. The audit in the tests
/// holds every binding against this list.
///
/// The table is read by the gate and not by a benchmark: what it decides is which
/// criterion output a run is required to have left behind. Nothing here says what any
/// case may cost -- that is [`CASE_BINDINGS`] and, behind it, the budget document.
pub(super) const DESIGN_CASES: &[&str] = &[
    "decode/2syl",
    "decode/4syl",
    "decode/8syl",
    "decode/12syl",
    "decode/64byte",
    "segment/dag_build",
    "input/buffer_ops",
    "passthrough/classify",
    "lm/edge_score",
    // The design names the nine above; these two are not in that list but do carry
    // thresholds, and a case a threshold is bound to is a case a run has to produce.
    "decode/holdout",
    "ui/wakeup_latency",
    // The transport case (ADR-0011) belongs to the post path's own card, which states
    // its threshold in the 0.5.3 table as `BUDGET-LAT-06`.
    "transport/post_frame",
];

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
    /// The estimate the gate compares with the threshold: `mean + SIGMAS * std_dev`.
    pub(super) fn p99(&self) -> f64 {
        self.mean + SIGMAS * self.std_dev
    }
}

/// Outcome of a benchmark-budget check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BenchReport {
    /// Bound cases that were found and are inside their threshold.
    pub passed: Vec<String>,
    /// Bound cases that are past their threshold, one message each.
    pub violations: Vec<String>,
    /// Cases the gate requires that the criterion output does not carry.
    ///
    /// A bound case is required because its threshold has to be judged, and every case
    /// the design names is required whether a threshold is bound to it or not; see
    /// [`DESIGN_CASES`].
    pub missing: Vec<String>,
    /// Cases the criterion output carries that no threshold is bound to.
    pub unbound: Vec<String>,
}

/// Compares a criterion output directory against a budget document.
///
/// `budgets` is the validated budget document and `dir` is the directory criterion
/// wrote its output to. `group` narrows the comparison to one criterion group
/// (`decode`, `input`, `passthrough`, ...); `None` compares every case the gate knows
/// of. The directory is a parameter rather than something this function resolves for
/// itself so that a test can drive the comparison over a directory it wrote, which is
/// what makes the assertion verifiable without running a benchmark.
///
/// A case the design names that the output does not carry is reported as missing
/// whether or not a threshold is bound to it, so a run that measured fewer cases than
/// the design names fails the gate and not only a run that measured them badly.
///
/// # Errors
/// Returns an error when `group` names a criterion group no case belongs to -- a gate
/// that asserted nothing because nothing matched must not pass -- when the criterion
/// output cannot be read, and when a binding names a threshold the document does not
/// carry; the last one is a defect in this module rather than in the documents, and it
/// is reported rather than skipped.
pub(super) fn check(budgets: &Budgets, dir: &Path, group: Option<&str>) -> Result<BenchReport> {
    // A group no case belongs to would leave the gate comparing nothing and exiting
    // successfully -- the "nothing was measured, therefore nothing is wrong" reading this
    // gate exists to refuse. The refusal names the groups that do carry a case.
    let unknown = group.filter(|wanted| !known_groups().contains(wanted));
    if let Some(wanted) = unknown {
        bail!(
            "--bench {wanted}: no benchmark case belongs to that group, so the gate would \
             assert nothing; the groups that carry one are {}",
            known_groups().join(", ")
        );
    }

    let estimates = read_estimates(dir)?;
    let mut report = BenchReport::default();

    // Every case the design names has to be in the output, bound or not: the acceptance
    // criterion is that the run produced all of them, and a case with nothing bound to it
    // would otherwise stop being measured without the gate noticing.
    for case in DESIGN_CASES {
        if in_group(case, group) && !estimates.contains_key(*case) {
            report.missing.push((*case).to_owned());
        }
    }

    for binding in CASE_BINDINGS {
        let CaseBinding(case, key, unit) = *binding;
        if !in_group(case, group) {
            continue;
        }
        let Some(measurement) = estimates.get(case) else {
            // The design-case sweep above has already reported it, unless the binding
            // names a case the design does not -- which the audit in the tests forbids.
            // Reported once either way.
            if !report
                .missing
                .iter()
                .any(|missing| missing.as_str() == case)
            {
                report.missing.push((*case).to_owned());
            }
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

/// The criterion groups at least one case belongs to, sorted and without repeats.
///
/// A refusal names them, so a caller who asked for a group that carries no case can see
/// which groups do. Both tables contribute: a group whose only case is unasserted still
/// has something to check, because that case's output is still required.
fn known_groups() -> Vec<&'static str> {
    let mut groups: Vec<&'static str> = CASE_BINDINGS
        .iter()
        .map(|binding| group_of(binding.0))
        .chain(DESIGN_CASES.iter().map(|case| group_of(case)))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// Whether `case` is one the caller asked the gate to check.
///
/// A narrowing to one group skips the cases outside it, both the ones that are bound and
/// the ones that are only required to have been measured: `--bench passthrough` asks a
/// question about that group and not about the rest of the run.
fn in_group(case: &str, group: Option<&str>) -> bool {
    group.is_none_or(|wanted| wanted == group_of(case))
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
///
/// Empty when no binding names the key. Every threshold of the budget document is bound --
/// the spec comparison refuses one that is not -- so the empty answer is what an unbound
/// key prints in a message, not a case the repository's document reaches.
pub(crate) fn owner(key: &str) -> &'static str {
    BINDINGS
        .iter()
        .find(|binding| binding.0 == key)
        .map_or("", |binding| binding.1)
}

/// The audit of the benchmark sources, which no criterion number can show.
///
/// Two properties of a benchmark target are invisible in the output it leaves behind:
/// that it took its inputs from the binary rather than from the machine, and that the
/// case names it declares are the ones this gate binds thresholds to. Both are
/// asserted here, over the sources, because a run that measured a different machine or
/// published a case under a new name still produces a plausible `estimates.json`.
///
/// The tests live in this file rather than beside the rest of the module's tests
/// because they read the source tree instead of a criterion directory, and because
/// together they would take the sibling file past the project's line limit.
#[cfg(test)]
mod sources {
    use anyhow::ensure;

    use crate::budget::repo_root;

    use super::*;

    /// The benchmark directories whose cases the budget gate asserts.
    ///
    /// `ime-dict`'s benchmarks are deliberately outside the set: they measure a mapped
    /// dictionary container and a store in a directory of its own, so a file is what
    /// they are about, and no threshold is bound to any of their cases.
    const BOUND_BENCH_DIRS: &[&str] = &[
        "crates/ime-core/benches",
        "crates/ime-ui/benches",
        "crates/ime-fcitx5/benches",
    ];

    /// Constructs that would make a benchmark depend on the machine it runs on, and why.
    const MACHINE_DEPENDENT: &[(&str, &str)] = &[
        ("home_dir", "resolves the user's home directory"),
        ("\"HOME\"", "reads $HOME"),
        ("temp_dir", "resolves the system temporary directory"),
        ("\"/tmp/", "names a path outside the target directory"),
        ("fs::", "reads or writes a file at run time"),
        ("File::", "opens a file at run time"),
    ];

    /// Every Rust source under the benchmark directories the gate asserts.
    ///
    /// The tree is walked rather than a list of targets kept: a benchmark that moves to
    /// a new file, or a fixture module beside it, stays inside the check without anyone
    /// having to remember to add it.
    ///
    /// # Errors
    /// Returns an error when a directory cannot be read, and when one of them carries no
    /// Rust source at all -- a check that asserted nothing because the tree moved must
    /// not pass.
    fn bench_sources() -> Result<Vec<(PathBuf, String)>> {
        let root = repo_root()?;
        let mut sources = Vec::new();
        for dir in BOUND_BENCH_DIRS {
            let before = sources.len();
            collect_sources(&root.join(dir), &mut sources)?;
            ensure!(
                sources.len() > before,
                "no Rust source under {dir}: the benchmark tree moved and this check would \
                 assert nothing"
            );
        }
        Ok(sources)
    }

    /// Appends every `.rs` file under `dir`, and under the directories below it.
    fn collect_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
        let entries =
            fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect_sources(&path, out)?;
            } else if path.to_str().is_some_and(|name| name.ends_with(".rs")) {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                out.push((path, text));
            }
        }
        Ok(())
    }

    #[test]
    fn test_bench_sources_are_self_contained() -> Result<()> {
        // A benchmark that resolves a path under `$HOME`, or that reads a file at run
        // time, measures the machine it happens to run on: the corpus is generated from
        // constants and the held-out set is embedded in the binary, which is what lets a
        // run on a machine with no `$HOME` and no compiled dictionary measure exactly
        // what a developer's run measures. A criterion number cannot show that, so the
        // property is asserted over the sources.
        let mut findings = Vec::new();
        for (path, text) in bench_sources()? {
            for &(needle, why) in MACHINE_DEPENDENT {
                if text.contains(needle) {
                    findings.push(format!("{}: `{needle}` {why}", path.display()));
                }
            }
        }
        assert!(
            findings.is_empty(),
            "a benchmark takes its inputs from the binary and not from the machine: {}",
            findings.join("; ")
        );
        Ok(())
    }

    #[test]
    fn test_bench_sources_declare_every_case_the_gate_names() -> Result<()> {
        // A case's criterion id is built from the group and the function name its target
        // declares, and the gate binds thresholds to that id. A rename in either half
        // leaves the gate asserting a case nothing produces, which it reports as missing
        // -- but only after a full benchmark run. Holding the names against the sources
        // catches the rename here instead.
        let sources = bench_sources()?;
        let text = sources
            .iter()
            .map(|(_, source)| source.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        for case in DESIGN_CASES {
            let (group, function) = case
                .split_once('/')
                .with_context(|| format!("{case} is not a <group>/<function> criterion id"))?;
            for part in [group, function] {
                let declared = format!("\"{part}\"");
                assert!(
                    text.contains(&declared),
                    "{case}: no benchmark declares the group or case name {part:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn test_every_bound_case_is_a_case_the_design_names() {
        // The binding table and the case list are two views of one set: a case a
        // threshold is bound to is a case a run has to produce. A binding the case list
        // does not carry would be a threshold asserted against something the gate never
        // required to exist.
        for binding in CASE_BINDINGS {
            assert!(
                DESIGN_CASES.contains(&binding.0),
                "{} is bound to a threshold and is not a case the design names",
                binding.0
            );
        }
    }
}
