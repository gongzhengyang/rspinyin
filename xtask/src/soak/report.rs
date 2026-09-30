//! The soak report: what a run observed, as a document a gate can read.
//!
//! Responsibility: define the report's schema, read it back, and render the summary a
//! person reads. The document holds observations -- the plan the run followed, the process
//! it followed, how many strokes went out, and the sample series -- and no verdict at all.
//!
//! # Why the report carries the series and not the verdict
//!
//! A soak that failed tells you nothing on its own. The interesting question is whether the
//! resident set was flat for seven hours and then jumped, or climbed steadily from the
//! first minute: those two have different causes, and the curve is what distinguishes them.
//! A document that kept only the last number could not answer it, so every reading is kept.
//!
//! The verdict is derived when the report is read, by [`super::judge`], from these
//! observations and the thresholds in `docs/dev/budgets.json`. Nothing in this file knows
//! what a threshold is, which is what keeps a stored verdict from disagreeing with the
//! document it was supposed to have been judged against.
//!
//! # What is stored twice, and why that is checked
//!
//! The plan's derived counts are written into the document as well as its four inputs, so a
//! reader sees what the run was for without doing the arithmetic. That is one number in two
//! places, and [`PlanReport::plan`] refuses a document whose copies disagree rather than
//! picking one of them: a report that cannot say what it planned cannot say what fraction
//! of it was done.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::plan::{Plan, PlanError, SECONDS_PER_HOUR};

/// Schema version of the report this build writes.
///
/// Raised when the document gains an observation a reader has to understand rather than
/// skip: the pass counts of the keystroke cycle and the crash records are judged, not
/// printed, so a report that does not carry them is refused by [`SoakReport::read`] instead
/// of being read as a run in which neither happened.
pub const REPORT_VERSION: u32 = 2;

/// The code a run that stopped because the process it followed went away carries.
pub const PROCESS_DIED: &str = "soak/process-died";

/// The code a run that stopped because the input focus left the client carries.
pub const FOCUS_STOLEN: &str = "soak/focus-stolen";

/// The code a run that stopped because the server refused an injected event carries.
pub const INJECTION_FAILED: &str = "soak/injection-failed";

/// The code a run that stopped because a sample could not be read carries.
pub const SAMPLING_FAILED: &str = "soak/sampling-failed";

/// What every report says about how its numbers were taken.
///
/// These are limits of the measurement rather than of the run, and they travel with the
/// document because a report is read months later by someone who did not take it. The two
/// that matter most: a resident set size counts the dictionary's read-only mapping, whose
/// pages the kernel may drop and fault back in, so a rise in it is not by itself a leak;
/// and the run ends with a composition open, which is what makes the candidate window
/// observable at the end rather than a race against the next commit.
pub const NOTES: &[&str] = &[
    "the drift statistic is the envelope of VmRSS over the steady window, and VmRSS counts \
     the dictionary's read-only mapping; the anonymous envelope in the same series is the \
     leak-shaped number when the two disagree",
    "the slope is reported and not asserted: the budget is stated as a drift over the run, \
     and a rate would be a second threshold nobody wrote down",
    "the run ends with a composition open so that the candidate window is observable when \
     the run returns",
    "a sample whose rss_kb is absent is a reading that could not be taken; the run stops at \
     the first one rather than continuing against a process that is no longer there",
    "the pass counts are whole passes over the driver's keystroke cycle, so a run that \
     stopped in the middle of one has walked the passes it counts and no more",
    "the crash count is the plugin's own record directory and not this process's: a panic \
     inside the addon is caught at the C ABI boundary and written to a record rather than \
     killing the daemon, so a run in which the plugin crashed is a run whose resident set \
     can still be flat",
    "the load average is recorded and not asserted, because the budget document states no \
     ceiling for it; a run taken on a busy machine is a different measurement of the same \
     process and is not comparable with one taken on an idle machine",
];

/// One soak run, as it was observed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoakReport {
    /// Schema version of this document.
    pub report_version: u32,
    /// The schedule the run followed.
    pub plan: PlanReport,
    /// The process and window the run drove.
    pub target: Target,
    /// What the machine was doing while the run took its readings.
    pub host: Host,
    /// When the run started, as a Unix timestamp in seconds.
    pub started_at_unix_s: u64,
    /// How long the run lasted, in seconds.
    pub elapsed_s: f64,
    /// How many strokes the run actually sent.
    pub delivered_strokes: u64,
    /// How much of the driver's keystroke cycle the run walked.
    pub cycles: CycleReport,
    /// The plugin's crash records at the two ends of the run.
    ///
    /// Absent when the plugin's data directory could not be resolved at all, which is a
    /// reading nobody took rather than a count of zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crashes: Option<CrashCount>,
    /// Why the run stopped before its plan was exhausted, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_early: Option<StopReason>,
    /// Every reading the run took, in the order it took them.
    pub samples: Vec<Sample>,
    /// What the report says about how its numbers were taken.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl SoakReport {
    /// Reads the report at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, when it is not a soak report, and
    /// when its schema version is not the one this build writes -- a report from a newer
    /// tool may carry observations this one would read as absent.
    pub fn read(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let report: Self = serde_json::from_str(&text)
            .with_context(|| format!("{}: not a soak report", path.display()))?;
        ensure!(
            report.report_version == REPORT_VERSION,
            "{}: soak report schema version {} is not the {REPORT_VERSION} this build reads",
            path.display(),
            report.report_version
        );
        Ok(report)
    }

    /// Writes the report to `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be rendered and when it cannot be written.
    pub fn write(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("rendering the soak report")?;
        fs::write(path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
    }

    /// The plan the run followed, or the reason the document does not state one.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan's numbers do not describe a schedule, and when the
    /// counts stored beside them disagree with the schedule they were derived from.
    pub fn plan(&self) -> Result<Plan> {
        self.plan.plan()
    }

    /// How much of the driver's keystroke cycle the run walked.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan does not describe a schedule, when the cycle holds no
    /// stroke, and when the stored pass counts disagree with the strokes they are derived
    /// from. The last is the same refusal [`PlanReport::plan`] makes about the plan's own
    /// copies: a report that cannot say how much of its cycle it walked cannot say whether
    /// the state loop the long-run budget is about was covered at all.
    pub fn cycles(&self) -> Result<CycleReport> {
        let plan = self.plan()?;
        self.cycles.verify(&plan, self.delivered_strokes)?;
        Ok(self.cycles)
    }

    /// The fraction of the planned strokes that went out, as a percentage.
    ///
    /// The denominator is the plan rather than what the run reached, which is the whole
    /// point: a run that sent ten strokes and completed ten of them is not a perfect run,
    /// it is a run that did a hundred-thousandth of its job. `None` when the document
    /// states a plan of no strokes at all, which [`PlanReport::plan`] refuses anyway.
    pub fn pass_rate_pct(&self) -> Option<f64> {
        let planned = self.plan.planned_strokes;
        if planned == 0 {
            return None;
        }
        Some(100.0 * self.delivered_strokes as f64 / planned as f64)
    }

    /// Whether any reading found the process gone.
    pub fn process_gone(&self) -> bool {
        self.samples.iter().any(|sample| !sample.measured())
    }

    /// The summary a person reads, one line per fact.
    ///
    /// `stats` is the run's resident-set statistics, or `None` when the series is too short
    /// to carry any. The shape is stable: what the run was, what it planned, what it did,
    /// why it stopped, and then the numbers.
    pub fn lines(&self, stats: Option<&super::stats::RssStats>) -> Vec<String> {
        let mut lines = vec![
            format!(
                "target: pid {} ({}) on {} window {:#x}",
                self.target.pid, self.target.executable, self.target.display, self.target.window
            ),
            format!(
                "plan: {:.3}h at {:.1} keys/s, sampled every {:.1}s, warm-up {:.1}s",
                self.plan.duration_s / SECONDS_PER_HOUR,
                self.plan.rate_hz,
                self.plan.interval_s,
                self.plan.warmup_s
            ),
            format!(
                "strokes: {} of {} planned",
                self.delivered_strokes, self.plan.planned_strokes
            ),
            format!("elapsed: {:.1}s", self.elapsed_s),
            format!("samples: {}", self.samples.len()),
        ];
        lines.push(format!(
            "cycles: {} of {} complete, {} strokes each",
            self.cycles.delivered, self.cycles.planned, self.cycles.strokes_per_cycle
        ));
        lines.push(match &self.crashes {
            None => "crash records: unmeasured, no data directory resolved".to_owned(),
            Some(count) => match (count.before, count.after) {
                (Some(before), Some(after)) => format!(
                    "crash records: {before} before and {after} after, in {}",
                    count.directory
                ),
                _ => format!("crash records: unreadable in {}", count.directory),
            },
        });
        lines.push(match (self.host.load1_at_start, self.host.load1_at_end) {
            (Some(start), Some(end)) => format!(
                "host: {} cpu(s), load1 {start:.2} at start and {end:.2} at end \
                 (diagnostic, not asserted)",
                self.host.cpus
            ),
            _ => format!(
                "host: {} cpu(s), load average unreadable (diagnostic, not asserted)",
                self.host.cpus
            ),
        });
        match self.pass_rate_pct() {
            Some(rate) => lines.push(format!("pass rate: {rate:.2}%")),
            None => lines.push("pass rate: none, the plan holds no stroke".to_owned()),
        }
        if let Some(stop) = &self.stopped_early {
            lines.push(format!("stopped early: {}", stop.describe()));
        }
        if self.process_gone() {
            lines.push("the process could not be read at least once".to_owned());
        }
        match stats {
            None => lines.push("rss: too few readings to describe".to_owned()),
            Some(stats) => lines.extend(stats.lines()),
        }
        lines
    }
}

/// The schedule a run followed, as the document states it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanReport {
    /// How long the run was to last, in seconds.
    pub duration_s: f64,
    /// How many strokes per second it was to type.
    pub rate_hz: f64,
    /// How many seconds apart its samples were to be.
    pub interval_s: f64,
    /// How many seconds at the start were excluded from the drift statistic.
    pub warmup_s: f64,
    /// How many strokes the plan came to.
    pub planned_strokes: u64,
    /// How many samples the plan came to.
    pub planned_samples: u64,
}

impl PlanReport {
    /// The plan `plan` states, with its derived counts.
    pub fn of(plan: &Plan) -> Self {
        Self {
            duration_s: plan.duration_s(),
            rate_hz: plan.rate_hz(),
            interval_s: plan.interval_s(),
            warmup_s: plan.warmup_s(),
            planned_strokes: plan.planned_strokes(),
            planned_samples: plan.planned_samples(),
        }
    }

    /// The plan this document states.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError`] wrapped in an error when the four inputs do not describe a
    /// schedule, and an error naming both copies when the stored counts disagree with the
    /// schedule they were derived from.
    pub fn plan(&self) -> Result<Plan> {
        let plan = Plan::new(
            self.duration_s / SECONDS_PER_HOUR,
            self.rate_hz,
            self.interval_s,
            self.warmup_s,
        )?;
        ensure!(
            plan.planned_strokes() == self.planned_strokes,
            "the report plans {} strokes and its schedule comes to {}",
            self.planned_strokes,
            plan.planned_strokes()
        );
        ensure!(
            plan.planned_samples() == self.planned_samples,
            "the report plans {} samples and its schedule comes to {}",
            self.planned_samples,
            plan.planned_samples()
        );
        Ok(plan)
    }
}

/// How much of the driver's keystroke cycle a run walked.
///
/// The counts are whole passes rather than strokes, because the claim the long-run budget
/// rests on is about the state loop: a run that delivered its strokes without ever closing
/// the loop would have exercised one phase of the session for eight hours, and its stroke
/// count would say nothing about that. The cycle's length travels with the counts so that a
/// reader does not have to recompile the script to see what a pass costs, and so that a
/// count that disagrees with the strokes it came from can be refused rather than believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleReport {
    /// Strokes one pass over the cycle holds.
    pub strokes_per_cycle: u64,
    /// Whole passes the plan comes to.
    pub planned: u64,
    /// Whole passes the delivered strokes cover.
    pub delivered: u64,
}

impl CycleReport {
    /// The counts `plan` and `delivered_strokes` come to for a cycle of `strokes_per_cycle`.
    ///
    /// A cycle of no strokes comes to no whole pass of anything, which is answered as zero
    /// passes rather than by dividing by it; [`CycleReport::verify`] is what refuses such a
    /// report, because zero is also what a plan of no strokes comes to and the two must not
    /// read alike.
    pub fn of(plan: &Plan, strokes_per_cycle: u64, delivered_strokes: u64) -> Self {
        Self {
            strokes_per_cycle,
            planned: plan.planned_strokes().checked_div(strokes_per_cycle).unwrap_or(0),
            delivered: delivered_strokes.checked_div(strokes_per_cycle).unwrap_or(0),
        }
    }

    /// Whether these counts are the ones the run's own numbers come to.
    ///
    /// # Errors
    ///
    /// Returns an error when the cycle holds no stroke, and when either count disagrees with
    /// the strokes it is derived from.
    pub fn verify(&self, plan: &Plan, delivered_strokes: u64) -> Result<()> {
        ensure!(
            self.strokes_per_cycle > 0,
            "the report states a cycle of no strokes, so no pass count exists"
        );
        let derived = Self::of(plan, self.strokes_per_cycle, delivered_strokes);
        ensure!(
            self.planned == derived.planned,
            "the report counts {} planned passes and its plan of {} strokes comes to {}",
            self.planned,
            plan.planned_strokes(),
            derived.planned
        );
        ensure!(
            self.delivered == derived.delivered,
            "the report counts {} delivered passes and its {delivered_strokes} delivered \
             strokes come to {}",
            self.delivered,
            derived.delivered
        );
        Ok(())
    }
}

/// The process and window a run drove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The process whose counters were sampled; Fcitx5 holds the addon.
    pub pid: u32,
    /// The executable that process was running, as `/proc` named it when the run started.
    ///
    /// Recorded because a pid is reused: a run that followed a process which exited would
    /// otherwise keep reading whatever took its number, and a path that changed between the
    /// first and the last reading is the cheapest way to notice.
    pub executable: String,
    /// The X display the events went to.
    pub display: String,
    /// The window that had to keep the input focus, as the server numbered it.
    pub window: u32,
}

/// What the machine was doing while the run took its readings.
///
/// A long-run measurement is comparable with the next one only when both were taken on a
/// machine that was otherwise idle, and the readings themselves cannot say whether that was
/// so: the same process measured on a loaded host is a different number. The load average is
/// therefore recorded rather than assumed. It is printed beside the verdict and not asserted
/// -- the budget document states no ceiling for it, and a ceiling invented here would be a
/// threshold nobody wrote down.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Host {
    /// Processors the machine reports, which is what the load averages are relative to.
    pub cpus: u32,
    /// The one-minute load average when the run started, or absent when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1_at_start: Option<f64>,
    /// The one-minute load average when the run ended, or absent when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1_at_end: Option<f64>,
}

/// The crash records the plugin's own directory held at the two ends of a run.
///
/// The counter is the plugin's and not this process's. A panic inside the addon is caught at
/// the C ABI boundary and written to a record rather than killing the daemon, so a run in
/// which the plugin crashed repeatedly is a run that stays alive with a flat resident set --
/// which is exactly the shape a crash check that watched only the exit code would pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashCount {
    /// The directory the records were counted in.
    pub directory: String,
    /// How many records it held when the run started, or absent when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    /// How many it held when the run ended, or absent when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
}

impl CrashCount {
    /// How many records appeared between the two readings.
    ///
    /// `None` when either reading is absent, which is a count nobody took rather than a
    /// difference of zero: a directory that could not be read is not a directory in which
    /// nothing was recorded.
    pub fn recorded(&self) -> Option<u64> {
        Some(self.after?.saturating_sub(self.before?))
    }
}

/// One reading of the process's memory counters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Seconds from the start of the run to this reading.
    pub t_s: f64,
    /// Resident set size in kibibytes, or absent when the reading could not be taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_kb: Option<u64>,
    /// The anonymous resident part of the mappings in kibibytes, or absent when the
    /// reading could not be taken or the kernel withheld the rolled-up counters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous_kb: Option<u64>,
}

impl Sample {
    /// Whether this reading carries a resident set size.
    ///
    /// A reading without one is not a process that holds nothing: it is a process that
    /// could not be read, which is what a run stops on.
    pub fn measured(&self) -> bool {
        self.rss_kb.is_some()
    }
}

/// Why a run stopped before its plan was exhausted.
///
/// The code is a stable string of the `domain/action/reason` form, so a job that reads the
/// report can match on the reason rather than on prose, and the detail is what a person
/// reads. A run that reaches the end of its plan carries none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopReason {
    /// The stable code for the reason.
    pub code: String,
    /// What the run observed, in one sentence.
    pub detail: String,
}

impl StopReason {
    /// The process the run followed went away.
    pub fn process_died(detail: impl Into<String>) -> Self {
        Self::new(PROCESS_DIED, detail)
    }

    /// The input focus left the client under test.
    ///
    /// This is the project's highest-severity defect class: every later stroke would land
    /// in whatever took the focus, so the run stops rather than measuring the wrong window.
    pub fn focus_stolen(detail: impl Into<String>) -> Self {
        Self::new(FOCUS_STOLEN, detail)
    }

    /// The server refused an injected event.
    pub fn injection_failed(detail: impl Into<String>) -> Self {
        Self::new(INJECTION_FAILED, detail)
    }

    /// A reading of the process could not be taken for a reason other than its absence.
    pub fn sampling_failed(detail: impl Into<String>) -> Self {
        Self::new(SAMPLING_FAILED, detail)
    }

    /// The reason as a report prints it.
    pub fn describe(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }

    /// A reason from its code and its detail.
    fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
        }
    }
}
