//! The verdict on a soak report, against the robustness budgets.
//!
//! Responsibility: read the thresholds `docs/dev/budgets.json` states for a long run and
//! turn a report's observations into pass lines and violations. This is the only place a
//! soak's numbers become a verdict.
//!
//! # The ceiling is read, never restated
//!
//! `robustness.rss_drift_mb` is a memory ceiling, and the module that already owns the
//! conversion from the document's mebibytes to the kibibytes `/proc` reports is
//! [`MemoryBudgets`]. The ceiling is therefore taken from there rather than converted
//! again here, and the same binding that names the budget -- `MemoryBudget::SoakDrift` --
//! is what says which measurement it governs. No number is written down in this file.
//!
//! # Missing is not Pass
//!
//! A report whose series is too short to carry an envelope does not have a small drift; it
//! has no drift at all, and this module reports that as a violation rather than letting the
//! absence read as a pass. The same discipline applies to a run that stopped early: the
//! reason it stopped is a violation of its own, printed beside the pass rate it also cost.
//! And to the two observations that are not about memory at all: a crash count nobody took,
//! and a pass count nobody recorded, are both reported as violations.
//!
//! # What is asserted, and what is only reported
//!
//! Six things are judged: the plan was long enough to be the run the budget describes, every
//! planned stroke went out, every planned pass over the keystroke cycle was walked, the
//! plugin recorded no crash, the process was readable at every sample, and the steady
//! envelope is inside the ceiling. The slope, the anonymous envelope and the load average are
//! not judged -- the budget states neither a rate, nor an anonymous ceiling, nor a load
//! ceiling, and inventing one here would be a threshold nobody wrote down. They travel in the
//! report and are printed beside the verdict so that a person reading a failure has the
//! numbers that distinguish a leak from churn, and a measurement taken on a loaded machine
//! from one taken on an idle one.

use anyhow::{Context, Result};

use crate::budget::Budgets;
use crate::testd::memory::MemoryBudgets;

use super::plan::SECONDS_PER_HOUR;
use super::report::{CycleReport, PROCESS_DIED, SoakReport};
use super::stats::RssStats;

/// The code a run that was planned for less than the budgeted duration carries.
pub const SHORT_RUN: &str = "soak/short-run";

/// The code a run that sent fewer strokes than it planned carries.
pub const PASS_RATE: &str = "soak/pass-rate";

/// The code a run that walked fewer passes over the keystroke cycle than it planned carries.
pub const CYCLE_COVERAGE: &str = "soak/cycle-coverage";

/// The code a run in which the plugin recorded a crash carries.
pub const CRASH_RECORDED: &str = "soak/crash-recorded";

/// The code a run whose crash records could not be counted carries.
pub const CRASH_UNMEASURED: &str = "soak/crash-unmeasured";

/// The code a run whose resident set moved further than the ceiling carries.
pub const RSS_DRIFT: &str = "soak/rss-drift";

/// The code a run that stopped before its plan was exhausted carries.
pub const STOPPED_EARLY: &str = "soak/stopped-early";

/// What a report's numbers came to.
///
/// A verdict holds every line it judged, passing or not, so a report printed from one shows
/// how close a passing run came to its ceiling as readily as how far past it a failing one
/// went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoakVerdict {
    /// One line per threshold the report is inside.
    pub passed: Vec<String>,
    /// One line per threshold the report is past, each carrying a stable code.
    pub violations: Vec<String>,
}

impl SoakVerdict {
    /// Whether the report broke a budget.
    pub fn is_pass(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Judges `report` against the robustness thresholds `budgets` states.
///
/// # Errors
///
/// Returns an error when the report's own plan does not describe a schedule, when the plan
/// disagrees with the counts stored beside it, when the report's pass counts disagree with
/// the strokes they were derived from or describe a cycle of no strokes, and when the budget
/// document states no usable ceiling for the drift -- the last is a defect in the document
/// rather than in the run, and it is reported rather than skipped.
pub fn judge(report: &SoakReport, budgets: &Budgets) -> Result<SoakVerdict> {
    let plan = report.plan()?;
    let cycles = report.cycles()?;
    let robustness = &budgets.robustness;
    let mut verdict = SoakVerdict::default();

    check_duration(plan.duration_s(), robustness.soak_hours, &mut verdict);
    check_pass_rate(report, robustness.pass_rate_pct, &mut verdict);
    check_cycles(cycles, &mut verdict);
    check_crashes(report, &mut verdict);
    check_stopped(report, &mut verdict);
    check_process(report, &mut verdict);

    let stats = RssStats::of(&report.samples, plan.warmup_s());
    let limit_kb = drift_ceiling_kb(budgets)?;
    match &stats {
        Some(stats) => check_drift(stats, limit_kb, &mut verdict),
        None => verdict.violations.push(format!(
            "{RSS_DRIFT}: the report carries {} reading(s) and no steady window, so no drift \
             was measured; an unmeasured budget is not a satisfied one",
            report.samples.len()
        )),
    }
    Ok(verdict)
}

/// The drift ceiling the document states, in kibibytes.
///
/// # Errors
///
/// Returns an error when the document states no usable ceiling for the key, which its own
/// schema refuses but which this function still has to answer for.
fn drift_ceiling_kb(budgets: &Budgets) -> Result<u64> {
    let thresholds: Vec<(&'static str, f64)> = budgets
        .thresholds()
        .iter()
        .map(|threshold| (threshold.0, threshold.1))
        .collect();
    MemoryBudgets::from_thresholds(&thresholds)
        .soak_drift_kb
        .context("the budget document states no usable robustness.rss_drift_mb")
}

/// Records whether the run was planned to be the run the budget describes.
///
/// The comparison is on the plan rather than on the elapsed time, and that is deliberate:
/// the last stroke falls one key period before the nominal end, so an elapsed time would
/// need a tolerance to be read, while "was this meant to be an eight hour run" is exact.
fn check_duration(duration_s: f64, budgeted_hours: f64, verdict: &mut SoakVerdict) {
    let budgeted_s = budgeted_hours * SECONDS_PER_HOUR;
    if duration_s < budgeted_s {
        verdict.violations.push(format!(
            "{SHORT_RUN}: the run was planned for {:.3}h and the budget is stated for \
             {budgeted_hours:.3}h; a shorter run cannot support the claim",
            duration_s / SECONDS_PER_HOUR
        ));
    } else {
        verdict.passed.push(format!(
            "{SHORT_RUN}: planned for {:.3}h of {budgeted_hours:.3}h",
            duration_s / SECONDS_PER_HOUR
        ));
    }
}

/// Records how much of the driver's keystroke cycle the run walked.
///
/// The count is the run's own and the floor is the plan's, so no threshold is chosen here: a
/// run is required to have walked every pass it planned. What this catches that the pass rate
/// cannot is a cycle which stopped closing -- a driver whose script was shortened to
/// composing, or a run whose strokes went out faster than the session could follow them --
/// and a run that never completed a single pass, which delivered its strokes into one phase
/// of the session for the whole of its duration.
fn check_cycles(cycles: CycleReport, verdict: &mut SoakVerdict) {
    let line = format!(
        "{} of {} passes of {} strokes",
        cycles.delivered, cycles.planned, cycles.strokes_per_cycle
    );
    if cycles.planned == 0 {
        verdict.violations.push(format!(
            "{CYCLE_COVERAGE}: the plan comes to no whole pass over the cycle, so nothing \
             about the state loop was covered: {line}"
        ));
    } else if cycles.delivered < cycles.planned {
        verdict.violations.push(format!(
            "{CYCLE_COVERAGE}: {line}, short of the passes the plan comes to"
        ));
    } else {
        verdict.passed.push(format!("{CYCLE_COVERAGE}: {line}"));
    }
}

/// Records whether the plugin wrote a crash record while the run was going.
///
/// A crash inside the addon does not kill the daemon: it is caught at the C ABI boundary and
/// written to a record, so a crashing plugin is one that stays alive with a flat resident set
/// and a run that watched only the process would call it a pass. Counting the records is
/// therefore the only way this verdict can see it, and a count nobody took is reported as a
/// violation rather than as a run in which nothing crashed.
fn check_crashes(report: &SoakReport, verdict: &mut SoakVerdict) {
    let Some(count) = &report.crashes else {
        verdict.violations.push(format!(
            "{CRASH_UNMEASURED}: the plugin's data directory could not be resolved, so no \
             crash record was counted; a reading nobody took is not a pass"
        ));
        return;
    };
    match count.recorded() {
        None => verdict.violations.push(format!(
            "{CRASH_UNMEASURED}: {} could not be read at one end of the run, so the records \
             that appeared during it were not counted",
            count.directory
        )),
        Some(0) => verdict.passed.push(format!(
            "{CRASH_RECORDED}: none; {} held {} record(s) at both ends",
            count.directory,
            count.before.unwrap_or_default()
        )),
        Some(recorded) => verdict.violations.push(format!(
            "{CRASH_RECORDED}: {recorded} record(s) appeared in {} during the run",
            count.directory
        )),
    }
}

/// Records whether every planned stroke went out.
fn check_pass_rate(report: &SoakReport, budgeted_pct: f64, verdict: &mut SoakVerdict) {
    match report.pass_rate_pct() {
        Some(rate) if rate >= budgeted_pct => verdict.passed.push(format!(
            "{PASS_RATE}: {rate:.2}% of the planned {} strokes went out",
            report.plan.planned_strokes
        )),
        Some(rate) => verdict.violations.push(format!(
            "{PASS_RATE}: {rate:.2}% of the planned {} strokes went out, below the \
             {budgeted_pct:.2}% the budget requires",
            report.plan.planned_strokes
        )),
        None => verdict.violations.push(format!(
            "{PASS_RATE}: the plan holds no stroke, so no pass rate exists"
        )),
    }
}

/// Records why a run stopped before its plan was exhausted, when it did.
fn check_stopped(report: &SoakReport, verdict: &mut SoakVerdict) {
    match &report.stopped_early {
        Some(stop) => verdict
            .violations
            .push(format!("{STOPPED_EARLY}: {}", stop.describe())),
        None => verdict.passed.push(format!(
            "{STOPPED_EARLY}: the run reached the end of its plan"
        )),
    }
}

/// Records whether the process was readable at every sample.
fn check_process(report: &SoakReport, verdict: &mut SoakVerdict) {
    if report.process_gone() {
        verdict.violations.push(format!(
            "{PROCESS_DIED}: at least one sample could not read process {} ({})",
            report.target.pid, report.target.executable
        ));
    } else {
        verdict.passed.push(format!(
            "soak/process: {} ({}) was readable at all {} samples",
            report.target.executable,
            report.target.pid,
            report.samples.len()
        ));
    }
}

/// Records whether the steady envelope is inside the ceiling.
fn check_drift(stats: &RssStats, limit_kb: u64, verdict: &mut SoakVerdict) {
    let measured = stats.steady_envelope_kb;
    let line = format!(
        "steady envelope {measured}KiB of {limit_kb}KiB, whole-run envelope {}KiB, slope \
         {:.2}KiB/h",
        stats.envelope_kb, stats.slope_kib_per_hour
    );
    if measured > limit_kb {
        verdict
            .violations
            .push(format!("{RSS_DRIFT}: {line} is past the ceiling"));
    } else {
        verdict.passed.push(format!("{RSS_DRIFT}: {line}"));
    }
}
