//! `xtask soak` -- long-run stability, measured rather than asserted by hand.
//!
//! Responsibility: drive continuous input against a live session for a fixed duration,
//! sample the plugin process's memory at a fixed interval, write a report of what was
//! observed, and judge a report against the robustness thresholds
//! `docs/dev/budgets.json` states. This is the tool behind `BUDGET-ROB-01`, which every
//! other gate is a snapshot of and none of them can replace: a leak, a slow accumulation,
//! a resource that is released a thousand keystrokes late, and a crash that needs an hour
//! of state to reach are all invisible to a test that runs for a second.
//!
//! # Two entry points, one report
//!
//! [`run`] is the subcommand: it either performs a soak or judges a report one left behind.
//! [`assert_report`] is the judging half on its own, so a job can judge a report that was
//! taken elsewhere -- as `soak --assert <PATH>`, or through the budget gate's
//! `budget --soak-report <PATH>`, which is the same function -- without a second
//! implementation of the verdict.
//!
//! # What the report carries besides the resident set
//!
//! Three observations travel with the memory series, because a long run can fail in three
//! ways the series cannot show. The pass counts say how much of the driver's keystroke cycle
//! the run walked: a run that delivered every stroke it planned without ever closing the
//! state loop would have exercised one phase of the session for eight hours. The crash count
//! is the plugin's own record directory, because a panic inside the addon is caught at the C
//! ABI boundary and written to a record rather than killing the daemon -- so a crashing
//! plugin is one that stays alive with a flat resident set. And the load average is recorded
//! so that a run taken on a busy machine is visibly not comparable with one taken on an idle
//! one, which is the only condition under which the drift it measures means anything.
//!
//! # Why the report is a file rather than an exit code
//!
//! A soak that failed tells you nothing on its own. The interesting question is whether the
//! resident set was flat for seven hours and then jumped, or climbed steadily from the first
//! minute: those two have different causes, and the curve is what distinguishes them. So the
//! raw readings are kept rather than reduced to a verdict, and the verdict is derived when
//! the report is read. A run that went badly still writes its report and still exits zero --
//! the assertion is the next step's job -- because a tool that failed its own exit code
//! would leave a job with no artifact to read and no gate to run.
//!
//! # The ceiling comes from the document
//!
//! Nothing in this module states a threshold. `robustness.rss_drift_mb`, `soak_hours` and
//! `pass_rate_pct` are read from `docs/dev/budgets.json` through the budget document's own
//! reader, and the memory ceiling in particular travels through the module that already
//! owns the conversion from the document's mebibytes to the kibibytes `/proc` reports. A
//! number written down here would be a second copy of a budget the specification owns.
//!
//! # What lives where
//!
//! [`plan`] is the schedule, [`script`] the keystrokes, [`driver`] the live loop,
//! [`report`] the document, [`stats`] the arithmetic over its readings, and [`judge`] the
//! verdict. Everything except [`driver`] is arithmetic and parsing over data a test can
//! write out by hand, which is what lets the whole tool be covered without a display
//! server, a session or eight hours.

// The subcommand is mounted by `xtask/src/main.rs`, which this module does not own; the
// attribute below is not about that wiring. It is about this module's surface: the `pub use`
// lines are what the rest of the tool reaches, and a `pub use` in a *binary* crate is
// reported as unused whenever nothing in the crate names it -- which is the case for every
// one of them, since the subcommand's entry point is the only thing outside this module that
// refers to it. The attribute goes away when those re-exports do.
#![allow(dead_code, unused_imports)]

mod driver;
mod judge;
mod plan;
mod report;
mod script;
mod stats;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod verdict_tests;

pub use self::judge::SoakVerdict;
pub use self::plan::{Plan, PlanError, Step};
pub use self::report::{PlanReport, Sample, SoakReport, StopReason, Target};
pub use self::script::Phase;
pub use self::stats::RssStats;

use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use clap::Args;

use crate::budget;
use crate::testd::x11::Window;

/// Everything the soak tool can refuse to do before a run exists.
///
/// These are the refusals a caller can act on: each names what was missing and what to do
/// about it. A run that started and then went badly is not one of them -- that is an
/// outcome the report describes, and the verdict belongs to [`judge`].
#[derive(Debug, thiserror::Error)]
pub enum SoakError {
    /// Nothing named Fcitx5 is running and nothing was named.
    #[error(
        "no `{name}` process is running, so there is nothing to sample; start a session or \
         name the process holding the addon with `--pid`"
    )]
    NoProcess {
        /// The program name that was looked for.
        name: &'static str,
    },

    /// More than one Fcitx5 is running and nothing was named.
    #[error(
        "more than one Fcitx5 process is running ({pids}); name the one holding the addon \
         with `--pid`, because a report about the wrong process describes a session that \
         never saw a keystroke"
    )]
    AmbiguousProcess {
        /// The pids that were found, in the order the process table listed them.
        pids: String,
    },

    /// The process a caller named is not there.
    #[error("process {pid} is not running")]
    NoSuchProcess {
        /// The pid that was named.
        pid: u32,
    },

    /// The server's focus is not a window.
    #[error(
        "the X server on {display} reports {focus:#x} as the input focus, which is not a \
         window a key can be sent to; name the window to type into with `--window`"
    )]
    NoFocusWindow {
        /// The display the focus was asked about.
        display: String,
        /// What the server answered.
        focus: u32,
    },

    /// The stroke cycle came to nothing.
    #[error("the stroke cycle came to no stroke, so a run would type nothing and measure nothing")]
    EmptyScript,

    /// The stroke cycle does not leave the session in the phase it entered it in.
    ///
    /// A cycle the run repeats thousands of times has to close: one that does not accumulates
    /// state across passes, so what the resident set grew by over eight hours would be the
    /// driver's own growth rather than the plugin's, and the report would describe the
    /// harness. The run is refused before the first key goes out rather than reported
    /// afterwards, because there is no run worth describing.
    #[error(
        "the stroke cycle leaves the session in {to:?} rather than {from:?}; a cycle that \
         does not close would accumulate state across passes and measure the driver rather \
         than the plugin"
    )]
    CycleNotClosed {
        /// The phase the cycle is entered in.
        from: Phase,
        /// The phase it leaves behind.
        to: Phase,
    },
}

/// Command-line surface of `xtask soak`.
#[derive(Debug, Args)]
pub struct SoakArgs {
    /// Judge an existing report instead of running a soak.
    #[arg(long, value_name = "PATH")]
    pub assert: Option<PathBuf>,
    /// Duration of the run, in hours; `0.25` is fifteen minutes.
    #[arg(long, default_value_t = 8.0)]
    pub hours: f64,
    /// Where the report is written.
    #[arg(long, value_name = "PATH", default_value = "soak.json")]
    pub report: PathBuf,
    /// Keys typed per second.
    #[arg(long, default_value_t = 10.0)]
    pub rate_hz: f64,
    /// Seconds between two samples of the process.
    #[arg(long, default_value_t = 10.0)]
    pub interval_s: f64,
    /// Seconds at the start excluded from the drift statistic.
    #[arg(long, default_value_t = 60.0)]
    pub warmup_s: f64,
    /// X display to type into; defaults to `$DISPLAY`.
    #[arg(long)]
    pub display: Option<String>,
    /// Window that must keep the input focus, in decimal as `xdotool` prints it; defaults
    /// to whatever holds the focus when the run starts.
    #[arg(long)]
    pub window: Option<Window>,
    /// Process to sample; defaults to the only Fcitx5 on the machine.
    #[arg(long)]
    pub pid: Option<u32>,
    /// Device pixel ratio the candidate window is rasterised with.
    #[arg(long, default_value_t = 1.0)]
    pub scale: f32,
}

/// Entry point for `xtask soak`.
///
/// # Errors
///
/// Returns an error when the schedule the arguments describe is not one a run can follow,
/// when no process can be resolved to sample, when the display cannot be opened or has no
/// XTEST extension, when the window to type into cannot be resolved or focused, and when
/// the report cannot be written. A run that started and then went badly is not an error:
/// see the module documentation.
pub fn run(args: SoakArgs) -> Result<()> {
    if let Some(path) = &args.assert {
        return assert_report(path);
    }
    let plan = Plan::new(args.hours, args.rate_hz, args.interval_s, args.warmup_s)?;
    driver::run(driver::DriverArgs {
        plan,
        display: args.display.as_deref(),
        window: args.window,
        pid: args.pid,
        scale: args.scale,
        report: &args.report,
    })
}

/// Judges the report at `path` against the robustness thresholds.
///
/// The verdict is printed either way, so a job that reads only the log sees how close a
/// passing run came to its ceiling as readily as how far past it a failing one went.
///
/// # Errors
///
/// Returns an error when the report cannot be read or does not describe a schedule, when its
/// own counts disagree with the strokes they were derived from, when the budget document
/// cannot be read, and when the report is past a threshold or a threshold it states was never
/// measured.
pub fn assert_report(path: &Path) -> Result<()> {
    let report = SoakReport::read(path)?;
    let budgets = budget::read_budgets(&budget::repo_root()?)?;
    let plan = report.plan()?;
    let verdict = judge::judge(&report, &budgets)?;
    let stats = RssStats::of(&report.samples, plan.warmup_s());
    for line in report.lines(stats.as_ref()) {
        println!("soak: {line}");
    }
    for line in &verdict.passed {
        println!("soak: {line}");
    }
    ensure!(
        verdict.is_pass(),
        "{} robustness budget(s) past their threshold:\n  {}",
        verdict.violations.len(),
        verdict.violations.join("\n  ")
    );
    println!(
        "soak: {} robustness budget(s) within budget",
        verdict.passed.len()
    );
    Ok(())
}
