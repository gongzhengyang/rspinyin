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
//! # The tiers
//!
//! A run asks for one of three tiers, and the ladder is the case table's own `tier` column --
//! the runner keeps no second classification. `--tier smoke` (the default) runs the fast set,
//! everywhere, CI included. `--tier acceptance` runs the smoke set first as a *gate*: the
//! acceptance cases claim a real display and a serial X server, which is work a suite whose
//! fast cases are red has no business starting, so the acceptance set begins only after every
//! smoke case of the run passed. `--tier stress` runs the soak set and refuses to start at all
//! unless the machine is idle, because a number a soak collects under load is a number about
//! the load.
//!
//! A case that declares an environment requirement -- a live X11 session, an fcitx5 session --
//! is not run and judged on a machine that cannot answer for it. The runner hands the
//! declaration to [`crate::testd::env_gate`], and a case whose verdict is not runnable is
//! recorded as `skipped` with a failing `eligibility` claim whose text carries the same
//! `needs-environment:` / `pending-impl:` prefix the gate's own reports publish. A skipped
//! case is neither a pass nor a failure and leaves the batch verdict alone, which is what
//! makes a headless CI run of the acceptance tier produce a report instead of a red gate.
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
use crate::testd::env_gate::{
    self, ELIGIBILITY_CLAIM, EnvRequirement, Executability, TestCaseMeta,
};
use crate::testd::evidence::{
    AssertionRecord, AssertionV3, BatchSummary, BatchVerdict, CaseBundle, CaseEvidence, CaseStatus,
    Claim, EnvironmentFact, EvidenceError, EvidenceValue, FinalReport, RunDir, RunJournal, Tier,
    TraceRecord, Utc,
};
use crate::testd::purity::{PurityPolicy, PurityReport};

use self::cases::CaseLine;
use self::command::CommandRun;
use self::environment::Environment;
use self::error::SuiteError;

/// The suite tier a run executes, as the command line names it.
///
/// The three tiers are the ladder `features-test.md` fixes, and an unknown spelling is a
/// usage error rather than a silent fall-back to the default: a recipe that mistyped its tier
/// would otherwise always get the smoke set and never notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum TierFilter {
    /// The fast set every invocation may run, CI included.
    Smoke,
    /// The smoke set first, as a gate, then the acceptance set serially on a real display.
    Acceptance,
    /// The soak set, on a machine nothing else is using; never a CI gate.
    Stress,
}

impl TierFilter {
    /// The phases a run of this tier executes, in order.
    ///
    /// Acceptance runs the smoke set first because the acceptance cases claim a real display
    /// and a serial X server: work a suite whose fast cases are red has no business starting.
    /// The gate is the same cases the smoke tier runs, judged by the same rules.
    ///
    /// # Panics
    ///
    /// Never.
    fn phases(self) -> &'static [Tier] {
        match self {
            Self::Smoke => &[Tier::Smoke],
            Self::Acceptance => &[Tier::Smoke, Tier::Acceptance],
            Self::Stress => &[Tier::Stress],
        }
    }

    /// The name a refusal and the console print.
    ///
    /// # Panics
    ///
    /// Never.
    fn label(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Acceptance => "acceptance",
            Self::Stress => "stress",
        }
    }
}

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
    /// The tier to run: `smoke` (the default), `acceptance` or `stress`.
    ///
    /// Acceptance runs the smoke set first as its gate, and stress refuses to start unless
    /// the machine is idle. Within a tier the cases run serially, in the table's order.
    #[arg(long, value_enum, default_value_t = TierFilter::Smoke)]
    pub tier: TierFilter,
}

/// Entry point for `xtask testd-suite`.
///
/// # Errors
///
/// Returns an error when the repository root cannot be resolved, when the selection names a case
/// or a module the table does not hold, when the asked-for tier holds no case of the module, when
/// the stress tier finds the machine busy, when the run's directory cannot be created or its
/// documents cannot be written, and when the batch does not come out green -- a case that failed
/// or could not be judged is this subcommand's own failure, and its evidence is archived before
/// the failure is returned. An acceptance run whose smoke gate is not green stops before the
/// acceptance set and fails for the same reason.
///
/// # Panics
///
/// Never.
pub fn run(args: SuiteArgs) -> Result<()> {
    let root = repo_root()?;
    let results = results_root(&args.results_root, &root);
    // The stress tier measures the machine, so it refuses to start on one that is already
    // working: a soak sampled under load is a number about the load, not about the decoder.
    if args.tier == TierFilter::Stress {
        let purity = PurityReport::sample_below(&root, &PurityPolicy::DEFAULT);
        if !purity.is_clean() {
            let reason = purity
                .rejection_reason()
                .unwrap_or_else(|| String::from("the machine's state could not be read"));
            println!("suite: the stress tier refuses to start: {reason}");
            return Err(SuiteError::MachineBusy { reason }.into());
        }
    }
    let capabilities = EnvCapabilities::probe();
    let environment = Environment::of(&capabilities, &root);
    let phases = plan_phases(&args)?;
    let started = Utc::now();
    let run_dir = RunDir::create(&results, started)
        .with_context(|| format!("creating a run under {}", results.display()))?;
    let mut journal = RunJournal::new(started);
    let total: usize = phases.iter().map(|(_, pool)| pool.len()).sum();
    println!(
        "suite: the {} tier runs {total} case(s) from module `{}` in {} phase(s)",
        args.tier.label(),
        args.module,
        phases.len(),
    );
    for (phase, pool) in &phases {
        for case in pool {
            // A case that declares a requirement this machine cannot satisfy is skipped with
            // the gate's own words instead of run: a screen case judged on a headless box
            // would be a verdict about nothing.
            if let Some(declared) = case.requirement {
                let verdict = classify_case(case, declared, &capabilities);
                if !verdict.is_runnable() {
                    let evidence = skipped_case(case, environment.facts(), &verdict);
                    let bundle = run_dir.write_case(&evidence, None, None);
                    println!("{}", skipped_line(case, &bundle, &verdict));
                    journal.record(&evidence, bundle);
                    continue;
                }
            }
            let (runs, duration_ms) = execute_all(case, &root);
            let evidence = run_case(case, environment.facts(), &runs, duration_ms);
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
        // The acceptance tier's gate: the smoke phase just recorded has to be entirely green
        // before the acceptance set may claim the display. The gate reads the journal rather
        // than a counter kept beside the loop, so the rule it applies is the one the run's
        // own records state.
        if args.tier == TierFilter::Acceptance
            && *phase == Tier::Smoke
            && !smoke_gate_green(&journal)
        {
            println!("suite: the smoke gate is not green, so the acceptance set does not run");
            break;
        }
    }
    journal
        .publish(&run_dir, &results)
        .with_context(|| format!("publishing the run under {}", results.display()))?;
    // The batch report is rendered from the checklist the publish just wrote, and written
    // beside it: a run directory carries its own report, and the same checklist would
    // re-render it byte for byte.
    let report = FinalReport::of(&journal.checklist(&run_dir));
    let report_path = run_dir
        .write_report(&report.render())
        .with_context(|| format!("writing the batch report into {}", run_dir.name()))?;
    println!("suite: the batch report is {}", report_path.display());
    let summary = journal.summary();
    println!(
        "suite: {} case(s): {} passed, {} failed, {} flawed, {} skipped, {} evidence gap(s); verdict {}",
        summary.cases(),
        summary.passed,
        summary.failed,
        summary.flawed,
        summary.skipped,
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

/// The phases a run executes with the cases each one holds.
///
/// The planning happens before the run directory exists, so a tier the table cannot fill
/// refuses before anything is written at all. The tier asked for by name must hold cases --
/// a run that would judge nothing still has no verdict to give -- and so must the acceptance
/// tier's gate: an acceptance run whose smoke set is empty has no gate to pass. The
/// acceptance set behind the gate is the one phase allowed to be empty, because the cases
/// arrive as later edits to the table's tier column and an empty acceptance plane is a state
/// of the table, not a failure of the run.
///
/// # Errors
///
/// Returns [`SuiteError::NoCases`] when the module has no case in the table,
/// [`SuiteError::UnknownCase`] when the selection names one the module does not hold, and
/// [`SuiteError::EmptyTier`] when a phase that must hold cases holds none.
///
/// # Panics
///
/// Never.
fn plan_phases(args: &SuiteArgs) -> Result<Vec<(Tier, Vec<&'static cases::CaseLine>)>, SuiteError> {
    let selected = cases::selected(&args.module, &args.only)?;
    let mut phases = Vec::new();
    for phase in args.tier.phases() {
        let pool = cases::of_tier(&selected, *phase);
        // A tier asked for by name must hold cases, and so must the acceptance tier's gate;
        // the acceptance set behind the gate is the one phase allowed to be empty, because
        // its cases arrive as later edits to the table's tier column and an empty
        // acceptance plane is a state of the table, not a failure of the run.
        let must_hold = match args.tier {
            TierFilter::Acceptance => *phase == Tier::Smoke,
            TierFilter::Smoke | TierFilter::Stress => true,
        };
        if pool.is_empty() && must_hold {
            return Err(SuiteError::EmptyTier {
                tier: phase.label(),
                module: args.module.clone(),
            });
        }
        if !pool.is_empty() {
            phases.push((*phase, pool));
        }
    }
    Ok(phases)
}

/// The gate's verdict on a case that declares `declared` as its requirement.
///
/// # Panics
///
/// Never.
fn classify_case(
    case: &cases::CaseLine,
    declared: &str,
    capabilities: &EnvCapabilities,
) -> Executability {
    let meta = TestCaseMeta {
        id: case.id.to_owned(),
        module: case.module.to_owned(),
        requirement: EnvRequirement::parse(declared),
        // The suite's table has no pending-impl annotation for the gate to anchor a case on,
        // so a blocked verdict here comes from the requirement itself, never from a guessed
        // task; a table row that waits on code states that in its requirement instead.
        pending_impl: None,
    };
    env_gate::classify(&meta, capabilities)
}

/// Whether every smoke case the run has recorded so far passed, which is what the acceptance
/// tier's gate asks.
///
/// # Panics
///
/// Never.
fn smoke_gate_green(journal: &RunJournal) -> bool {
    journal
        .rows()
        .iter()
        .filter(|row| row.tier == Some(Tier::Smoke))
        .all(|row| row.status == CaseStatus::Pass)
}

/// The evidence a case the gate withheld leaves: a skipped verdict and the refusal itself.
///
/// The refusal is one assertion and one claim, both named `eligibility`, that do not pass,
/// and the text carries the `needs-environment:` or `pending-impl:` prefix the checklist
/// reads the outcome from -- the same convention the gate's own reports publish, so a run
/// directory and a gate report disagree about nothing.
///
/// # Panics
///
/// Never.
fn skipped_case(
    case: &cases::CaseLine,
    facts: &[EnvironmentFact],
    verdict: &Executability,
) -> CaseEvidence {
    let outcome = match verdict {
        Executability::Blocked { .. } => "pending-impl",
        Executability::Runnable | Executability::Unverifiable { .. } => "needs-environment",
    };
    let reason = format!("{outcome}: {}", verdict.describe());
    let assertion = AssertionRecord {
        name: String::from(ELIGIBILITY_CLAIM),
        expected: EvidenceValue::fact("runnable on this machine"),
        actual: EvidenceValue::fact(reason.clone()),
        ok: false,
    };
    let claim = Claim {
        name: String::from(ELIGIBILITY_CLAIM),
        expect: String::from("runnable on this machine"),
        actual: reason,
        pass: false,
    };
    CaseEvidence {
        tc: case.id.to_owned(),
        module: case.module.to_owned(),
        status: CaseStatus::Skipped,
        started_at: Utc::now(),
        duration_ms: 0,
        assertions: vec![assertion],
        visual: Vec::new(),
        healed: Vec::new(),
        environment: facts.to_vec(),
        v3: Some(AssertionV3 {
            case: case.id.to_owned(),
            tier: case.tier,
            dims: Vec::new(),
            requires_screenshots: false,
            screenshots_expected: Vec::new(),
            budget_keys: Vec::new(),
            claims: vec![claim],
            diffs: Vec::new(),
        }),
    }
}

/// The line a skipped case prints: the verdict, and the remedy that would lift it.
///
/// # Panics
///
/// Never.
fn skipped_line(
    case: &cases::CaseLine,
    bundle: &Result<CaseBundle, EvidenceError>,
    verdict: &Executability,
) -> String {
    let state = match bundle {
        Ok(_) => String::from("complete"),
        Err(error) => format!("gap: {}", error.reason()),
    };
    let remedy = case
        .requirement
        .map(|declared| EnvRequirement::parse(declared).remedy())
        .unwrap_or_default();
    format!(
        "suite: {} skipped, {}, {} -- remedy: {remedy} ({state})",
        case.id,
        verdict.describe(),
        verdict.marker(),
    )
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
/// and the bundle a case produces be asserted without running a command at all. The facts a
/// case's evidence carries arrive as the slice the environment collected rather than as the
/// collector itself, so a caller -- and a test -- can name exactly what the record sees.
///
/// The v3 record is built from the same assertions the v1 document holds: every assertion
/// becomes a [`Claim`], so the checklist carries the per-claim verdicts without a reader having
/// to re-derive them from the v1 list. The tier comes from the case table's own row; the
/// dimensions, the screenshot declarations and the budget keys stay empty until the case table
/// transcribes them -- an honest "nothing declared", never a coverage claim.
///
/// # Panics
///
/// Never.
fn run_case(
    case: &CaseLine,
    facts: &[EnvironmentFact],
    runs: &[CommandRun],
    duration_ms: u64,
) -> CaseEvidence {
    let assertions = verdict::assertions_of(case, runs);
    // The claim texts are the assertion texts as they are written, withheld values included:
    // a tail that was withheld stays the redaction placeholder here, which is what keeps the
    // checklist as publishable as the bundle it was read from.
    let claims = assertions
        .iter()
        .map(|assertion| Claim {
            name: assertion.name.clone(),
            expect: assertion.expected.as_str().to_owned(),
            actual: assertion.actual.as_str().to_owned(),
            pass: assertion.ok,
        })
        .collect();
    CaseEvidence {
        tc: case.id.to_owned(),
        module: case.module.to_owned(),
        status: verdict::judge(runs),
        started_at: Utc::now(),
        duration_ms,
        assertions,
        visual: Vec::new(),
        healed: Vec::new(),
        environment: facts.to_vec(),
        v3: Some(AssertionV3 {
            case: case.id.to_owned(),
            tier: case.tier,
            dims: Vec::new(),
            requires_screenshots: false,
            screenshots_expected: Vec::new(),
            budget_keys: Vec::new(),
            claims,
            diffs: Vec::new(),
        }),
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
