//! `xtask testd-suite` -- the case-suite runner: the P0 baseline cases, executed.
//!
//! Responsibility: take the P0 baseline cases the case document states, run each one's own
//! command, judge what the command produced, and archive the verdict in the layout the evidence
//! module fixes. It is the production caller that archive was waiting for: until this module
//! existed, [`crate::testd::evidence`] was exercised by its own tests and by nothing else, and
//! the thirty baseline cases of `docs/dev/tests.md` had never been run by anything at all.
//!
//! # The table is a transcription, not a design
//!
//! [`cases`] holds one row per case, and every row's command lines, precondition and pass
//! criteria are copied out of `docs/dev/tests.md`. Nothing here invents a command for a case that
//! states none: a case whose document declares only a precondition -- the double it is driven
//! with, the fixture it needs -- is run nowhere and recorded as `flawed`, with the document's own
//! precondition as the reason. A runner that guessed a filter from such a case's subject would
//! run *some* tests and report a pass for a criterion nobody checked, which is the one outcome
//! this whole exercise exists to prevent.
//!
//! # The verdict rule
//!
//! Three outcomes, and no fourth:
//!
//! * every command line exited `0` -> `pass`;
//! * a command line exited non-zero, or a signal killed it -> `fail`;
//! * a command line was never started, or the case declares none -> `flawed`.
//!
//! A failure outranks a gap, which is the precedence the run's own verdict uses. What a non-zero
//! exit *means* is deliberately not this module's judgement: the runner does not read the tool's
//! prose to tell a failing test from a missing feature or a build that would not compile, because
//! that reading is exactly the heuristic that lets a broken run be filed as a green one. Only
//! "the process was never started" is a gap, and a gap is never a pass.
//!
//! # The run fails when the batch does
//!
//! The subcommand's exit status is the batch's verdict, so a run in which a case failed or could
//! not be judged exits non-zero. A runner that archived a failed run and then exited `0` would be
//! a gate that always opens, and the archived evidence -- which is written before the verdict is
//! returned -- would be the only place the failure showed.
//!
//! # Privacy
//!
//! A command's output can carry what the user typed: an assertion message that prints a pinyin
//! buffer, a candidate sequence, a test name built from one. So the captured output reaches a
//! document only through [`EvidenceValue::withheld`](crate::testd::evidence::EvidenceValue::withheld),
//! and a bundle records how much output there was and never what it said. The console is the
//! operator's own channel and prints the tail of a case that did not pass -- no more than running
//! the command by hand would have printed -- and nothing this module prints is written to a file
//! of its own.
//!
//! # Environment facts
//!
//! [`CaseEvidence::environment`] is filled from the environment channel's own lines rather than
//! from a second probe, so a case's evidence and the environment gate's verdict describe the same
//! machine by construction. [`environment`] adds exactly two facts that channel does not carry --
//! whether the kernel is WSL's, and the commit the tree is at -- and records either as `unknown`
//! rather than as a negative answer when it cannot be read.
//!
//! # What this module does not do
//!
//! It decides no executability: whether a case may run on this machine at all is
//! [`crate::testd::env_gate`]'s verdict, and this runner runs what its table states. It writes no
//! pixels, opens no socket, and adds no dependency.
//!
//! # Reachability
//!
//! The subcommand that would call [`run`] lives in `xtask/src/main.rs` and the module declaration
//! that would compile this file lives in `xtask/src/testd/mod.rs`, neither of which this module
//! owns.
//!
//! # Modules
//!
//! [`cases`] holds the table, [`command`] runs one declared line, [`verdict`] judges what came
//! back, [`environment`] collects the machine facts, and [`error`] holds the refusals.

mod cases;
mod command;
mod environment;
mod error;
mod verdict;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;

use crate::budget::repo_root;
use crate::testd::env::EnvCapabilities;
use crate::testd::evidence::{
    BatchSummary, BatchVerdict, CaseBundle, CaseEvidence, CaseStatus, EvidenceError, RunDir,
    RunJournal, TraceRecord, Utc,
};

use self::cases::CaseLine;
use self::command::CommandRun;
use self::environment::Environment;
use self::error::SuiteError;

/// Command-line surface of `xtask testd-suite`.
#[derive(Debug, Args)]
pub struct SuiteArgs {
    /// Root of the evidence tree.
    ///
    /// A relative path resolves against the repository root rather than against the directory the
    /// command was started in, so the default writes the tree `features-test.md` fixes --
    /// `results/runs/run-<stamp>/<module>/<TC-ID>/` -- wherever the command is run from.
    #[arg(long, value_name = "DIR", default_value = "results")]
    pub results_root: PathBuf,
    /// Run only this case; repeat the option for more than one.
    ///
    /// Without it, every case of the selected module runs. The cases run in the document's order
    /// whatever order they are named in, so two invocations that select the same cases publish
    /// comparable runs.
    #[arg(long, value_name = "TC-ID")]
    pub only: Vec<String>,
    /// Module code whose cases run.
    ///
    /// Defaults to `core`, the module the case table is written for. A module the table does not
    /// cover selects nothing and is refused: a run of zero cases has no verdict to give, and
    /// reporting one green would be a claim about work nobody did.
    #[arg(long, value_name = "CODE", default_value = "core")]
    pub module: String,
}

/// Entry point for `xtask testd-suite`.
///
/// # Errors
///
/// Returns an error when the repository root cannot be resolved, when the selection names a case
/// or a module the table does not hold, when the run's directory cannot be created or its
/// documents cannot be written, and when the batch does not come out green -- a case that failed
/// or could not be judged is this subcommand's own failure, and its evidence is archived before
/// the failure is returned.
///
/// # Panics
///
/// Never.
pub fn run(args: SuiteArgs) -> Result<()> {
    let root = repo_root()?;
    let results = results_root(&args.results_root, &root);
    let selected = cases::selected(&args.module, &args.only)?;
    let environment = Environment::of(&EnvCapabilities::probe(), &root);
    let started = Utc::now();
    let run_dir = RunDir::create(&results, started)
        .with_context(|| format!("creating a run under {}", results.display()))?;
    let mut journal = RunJournal::new(started);
    println!(
        "suite: {} case(s) selected from module `{}`",
        selected.len(),
        args.module
    );
    for case in &selected {
        let (runs, duration_ms) = execute_all(case, &root);
        let evidence = run_case(case, &environment, &runs, duration_ms);
        let trace = evidence
            .status
            .needs_trace()
            .then(|| TraceRecord::of(&evidence));
        let bundle = run_dir.write_case(&evidence, trace.as_ref(), None);
        println!("{}", line_of(&evidence, &bundle));
        if evidence.status != CaseStatus::Pass {
            eprint!("{}", verdict::tail_block(case, &runs));
        }
        journal.record(&evidence, bundle);
    }
    journal
        .publish(&run_dir, &results)
        .with_context(|| format!("publishing the run under {}", results.display()))?;
    let summary = journal.summary();
    println!(
        "suite: {} case(s): {} passed, {} failed, {} flawed, {} evidence gap(s); verdict {}",
        summary.cases(),
        summary.passed,
        summary.failed,
        summary.flawed,
        summary.evidence_gaps,
        summary.verdict().label()
    );
    println!(
        "suite: the run is {} under {}",
        run_dir.name(),
        results.display()
    );
    match outcome(summary) {
        Ok(()) => Ok(()),
        Err(refusal) => {
            // The stable code goes to the console as well as into the error: a CI job matches on
            // that string, and a run whose only machine-readable output were the exit status
            // would leave it parsing a message.
            println!("suite: {}", refusal.code());
            Err(refusal.into())
        }
    }
}

/// Runs every command line the case states, in the document's order.
///
/// The duration is measured around the whole case rather than around one command, because the
/// number a reader wants beside a case is how long the case took.
///
/// # Panics
///
/// Never.
fn execute_all(case: &CaseLine, root: &Path) -> (Vec<CommandRun>, u64) {
    let began = Instant::now();
    let runs = case
        .commands
        .iter()
        .map(|declared| command::execute(declared, root))
        .collect();
    (runs, command::elapsed_ms(began))
}

/// The verdict record one case's runs produce.
///
/// `runs` arrives as a value rather than being executed here, which is what lets the judgement
/// and the bundle a case produces be asserted without running a command at all.
///
/// # Panics
///
/// Never.
fn run_case(
    case: &CaseLine,
    environment: &Environment,
    runs: &[CommandRun],
    duration_ms: u64,
) -> CaseEvidence {
    CaseEvidence {
        tc: case.id.to_owned(),
        module: case.module.to_owned(),
        status: verdict::judge(runs),
        started_at: Utc::now(),
        duration_ms,
        assertions: verdict::assertions_of(case, runs),
        visual: Vec::new(),
        healed: Vec::new(),
        environment: environment.facts().to_vec(),
    }
}

/// The line the console prints for one case.
///
/// The evidence column reports what became of the case's bundle rather than what the case came
/// to: a bundle that could not be written is a second failure beside the verdict, and a reader
/// has to see both.
///
/// # Panics
///
/// Never.
fn line_of(evidence: &CaseEvidence, bundle: &Result<CaseBundle, EvidenceError>) -> String {
    let state = match bundle {
        Ok(_) => String::from("complete"),
        Err(error) => format!("gap: {}", error.reason()),
    };
    format!(
        "suite: {} {} {} ms, {} assertion(s), {state}",
        evidence.tc,
        evidence.status.label(),
        evidence.duration_ms,
        evidence.assertion_count()
    )
}

/// The subcommand's own result: the batch's verdict, as a failure when it is not green.
///
/// # Errors
///
/// Returns [`SuiteError::EmptyBatch`] when the batch holds no case at all, and
/// [`SuiteError::BatchNotGreen`] when any case failed or could not be judged.
///
/// # Panics
///
/// Never.
fn outcome(summary: BatchSummary) -> Result<(), SuiteError> {
    if summary.cases() == 0 {
        return Err(SuiteError::EmptyBatch);
    }
    match summary.verdict() {
        BatchVerdict::Pass => Ok(()),
        verdict => Err(SuiteError::BatchNotGreen {
            verdict,
            passed: summary.passed,
            failed: summary.failed,
            flawed: summary.flawed,
            gaps: summary.evidence_gaps,
        }),
    }
}

/// The directory the evidence tree lives in, resolved against the repository root.
///
/// # Panics
///
/// Never.
fn results_root(argument: &Path, root: &Path) -> PathBuf {
    if argument.is_absolute() {
        argument.to_path_buf()
    } else {
        root.join(argument)
    }
}
