//! The environment capability gate: which cases this machine may run, and why it may not.
//!
//! Responsibility: read a case's declared environment requirement together with the report
//! [`crate::testd::env`] produced, and answer -- before the case runs -- whether this machine
//! can exercise it. A case that cannot run is withheld *with a reason*, and the reason is one
//! of two kinds that must never be confused.
//!
//! Boundaries: the gate consumes the report and re-probes nothing, so `env` stays the one place
//! a capability question is answered. It opens no socket, starts no process, reads no display
//! server and touches none of the plugin's files. It also decides nothing about a case's own
//! assertions: it says whether a case may run, never whether it passed.
//!
//! # Three states, not two
//!
//! [`Executability::Blocked`] and [`Executability::Unverifiable`] both mean "this case does not
//! run here", for reasons that have nothing to do with each other. A blocked case waits on code
//! that does not exist yet, and the wait ends by itself as the tasks that own that code land. An
//! unverifiable case waits on an environment, and no amount of code will bring one. Folding the
//! two together would let "not implemented yet" read as "this machine is the wrong one", which
//! is the misreading the gate exists to prevent.
//!
//! # The verdict is a rule, not this machine's answers written down
//!
//! `features.md` 0.5.5 registers what the development machine can and cannot verify, and the
//! decision table in [`table`] reaches those same answers -- but by asking the report, not by
//! remembering them. A gate that answered from a hardcoded list would clear an X11 case on a
//! headless CI runner, and a cleared case is a case whose pass is believed. What 0.5.5 registers
//! is therefore checked *against* the table by [`cross_check_spec`] rather than compiled in.
//!
//! # Fail closed
//!
//! A requirement the decision table does not declare is [`Executability::Unverifiable`], never
//! `Runnable`, and a capability the report does not show is withheld even when the machine
//! probably has it. An unknown read as a yes is how a gate passes a case for the wrong reason,
//! and the whole value of this gate is that a pass it allowed can be believed. A case document
//! that declares no requirement therefore runs nowhere until its metadata says what it needs,
//! which is the intended default rather than an oversight.
//!
//! # What a withheld case has to say
//!
//! Three things, and a reader needs all three: `needs` names the environment, the verdict's
//! `reason` says why this machine is not it, and [`EnvRequirement::remedy`] says what to do
//! about it -- which package to install, or which session to run the case on instead. A case
//! that declares no requirement is told which field declares one, which is the answer the case
//! suite needs until its metadata says what each case wants.
//!
//! # A pass this gate allowed is a pass that can be believed
//!
//! The verdict travels with a result, and [`audit_batch_against`] re-derives it anyway from the
//! case and the report rather than trusting the copy. A batch audited against the verdicts its
//! own results state agrees with itself: a runner that recorded a withheld case as a pass would
//! be reported clean, which is the one outcome this gate exists to make impossible.
//!
//! # What the gate does not do
//!
//! It never looks at the source tree. Whether a task has landed is the task document's answer,
//! not a probe's: a gate that guessed it from the presence of a file would clear a case whose
//! crate exists but whose wiring does not, which is the state several crates here are in.
//!
//! # Modules
//!
//! [`table`] holds the decision table and the capability predicates it answers with, [`spec`]
//! reads the register `features.md` 0.5.5 keeps and holds the table to it, and [`cases`] reads
//! the case suite's own metadata. This file holds the vocabulary they share and the functions a
//! caller reaches for first.

// The gate is exercised by the tests beside it and by nothing else yet: the case runner that
// would call it and the subcommand tree that would print its report both live in files this
// module does not own. Until that wiring lands, every item here is reported as dead code in a
// non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the `pub` items below are this module's
// surface, and an import a *binary* crate never names is reported as unused however public the
// item it feeds is.
#![allow(dead_code, unused_imports)]

mod cases;
mod spec;
mod table;

#[cfg(test)]
mod tests;

pub use self::cases::{CASE_DOCUMENT, CASE_SHARD_DIR, parse_cases, read_cases};
pub use self::spec::{
    REGISTER_SECTION, RegisterEntry, SPEC_DOCUMENT, TableDrift, UnverifiableRegister,
    cross_check_spec, read_register,
};

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::testd::env::EnvCapabilities;

/// What this machine can do with one case.
///
/// The two withholding variants are the point of the type: they both say a case does not run,
/// and they say it for reasons a reader has to be able to tell apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Executability {
    /// The case may run here, and a pass it reports may be believed.
    Runnable,
    /// The code under test does not exist yet; blocked on a `features.md` task.
    Blocked {
        /// The task, or the span of tasks, the case waits on.
        task: String,
        /// What is missing, in one line.
        reason: String,
    },
    /// The environment cannot exercise this; verified elsewhere.
    Unverifiable {
        /// The environment that would settle it.
        needs: String,
        /// Why this machine cannot show the capability.
        reason: String,
    },
}

impl Executability {
    /// The label a report prints and a batch counts by.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Runnable => "runnable",
            Self::Blocked { .. } => "blocked",
            Self::Unverifiable { .. } => "unverifiable",
        }
    }

    /// Whether the case may run here.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_runnable(&self) -> bool {
        matches!(self, Self::Runnable)
    }

    /// The marker the case document writes for this verdict.
    ///
    /// The three markers are the ones the case suite marks its cases with -- cleared, blocked
    /// with the task it waits on, and unverifiable -- so a withheld case can be written back
    /// into the suite unchanged. A case the gate withheld is never silent about it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn marker(&self) -> String {
        match self {
            Self::Runnable => String::from("[可执行]"),
            Self::Blocked { task, .. } => format!("[待实现: {task}]"),
            Self::Unverifiable { .. } => String::from("[不可验证]"),
        }
    }

    /// Why the case is withheld, or `None` when it is not.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Runnable => None,
            Self::Blocked { reason, .. } | Self::Unverifiable { reason, .. } => {
                Some(reason.as_str())
            }
        }
    }

    /// The verdict as a report prints it, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::Runnable => String::from("may run here"),
            Self::Blocked { task, reason } => format!("blocked on {task}: {reason}"),
            Self::Unverifiable { needs, reason } => {
                format!("unverifiable here: needs {needs} ({reason})")
            }
        }
    }
}

/// The environment a case declares it needs, as the case's own metadata states it.
///
/// The first nine variants are the requirements the platform's decision table answers.
/// [`EnvRequirement::Undeclared`] is the tenth and the important one: a case that declares
/// something else is not a parse error, it is a case this gate will not clear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvRequirement {
    /// A live X11 session, at whatever display the session names.
    X11Session,
    /// A live Fcitx5 session that loads both addons.
    Fcitx5Session,
    /// One of the real Wayland compositor tiers of `features.md` 0.5.2.
    WaylandTier,
    /// A compositor that honours an application-side blur request.
    CompositorBlur,
    /// More than one monitor, at more than one scale.
    MultipleMonitors,
    /// An eight-hour uninterrupted run on bare metal.
    LongRun,
    /// The candidate window's own drawing.
    CandidateWindowUi,
    /// Reloading the configuration without restarting the session.
    ConfigReload,
    /// Redacting the log stream.
    LogRedaction,
    /// A requirement no row of the decision table declares.
    Undeclared(String),
}

impl EnvRequirement {
    /// The label the case document declares this requirement with.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(&self) -> &str {
        match self {
            Self::X11Session => "X11 会话",
            Self::Fcitx5Session => "fcitx5 会话",
            Self::WaylandTier => "Wayland wlroots/KWin/Mutter",
            Self::CompositorBlur => "合成器模糊",
            Self::MultipleMonitors => "多显示器",
            Self::LongRun => "8 小时长稳",
            Self::CandidateWindowUi => "候选框 UI",
            Self::ConfigReload => "配置热重载",
            Self::LogRedaction => "日志脱敏",
            Self::Undeclared(text) => text,
        }
    }

    /// Reads a case's declaration, with the backticks a document may wrap it in removed.
    ///
    /// Nothing here fails: a declaration the table does not know becomes
    /// [`EnvRequirement::Undeclared`], which the gate withholds. A parser that refused the text
    /// instead would turn a mistyped requirement into a crash rather than a withheld case.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> Self {
        match text.trim().trim_matches('`').trim() {
            "X11 会话" => Self::X11Session,
            "fcitx5 会话" => Self::Fcitx5Session,
            "Wayland wlroots/KWin/Mutter" => Self::WaylandTier,
            "合成器模糊" => Self::CompositorBlur,
            "多显示器" => Self::MultipleMonitors,
            "8 小时长稳" => Self::LongRun,
            "候选框 UI" => Self::CandidateWindowUi,
            "配置热重载" => Self::ConfigReload,
            "日志脱敏" => Self::LogRedaction,
            other => Self::Undeclared(other.to_owned()),
        }
    }

    /// What would let a case that declares this requirement run here, in one line.
    ///
    /// The remedy is the third thing a withheld case owes a reader. `needs` names the
    /// environment and the verdict's `reason` says why this machine is not it; this says what to
    /// do about it, because a reader told only the first two has to guess whether the machine
    /// needs a package or the case needs code. A requirement whose code does not exist yet has
    /// no such answer, and its remedy says so rather than sending the reader after a compositor.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn remedy(&self) -> &'static str {
        table::remedy_for(self)
    }
}

/// One case's metadata: what it is called, where it lives and what it declares it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCaseMeta {
    /// The case identifier, as the suite writes it: `TC-<module>-<number>`.
    pub id: String,
    /// The module the case belongs to: `core`, `dict`, `rt`, `ui`, `sec`, `diag` or `infra`.
    pub module: String,
    /// The environment the case declares it needs.
    pub requirement: EnvRequirement,
}

/// What a run reported about a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseOutcome {
    /// The case ran and every assertion held.
    Pass,
    /// The case ran and at least one assertion failed.
    Fail,
    /// The case did not run.
    NotRun,
}

impl CaseOutcome {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotRun => "not-run",
        }
    }
}

/// One case's result: the verdict the gate reached, and what the run reported afterwards.
///
/// The verdict travels with the result so that a bare result list can be audited at all:
/// [`audit_batch`] needs nothing else. Carrying it is not the same as believing it, and
/// [`audit_batch_against`] re-derives the verdict from the case and the report and reports a
/// result that disagrees with the one it reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseResult {
    /// The case identifier, as [`TestCaseMeta::id`] writes it.
    pub id: String,
    /// The verdict the gate reached before the case ran.
    pub verdict: Executability,
    /// What the run reported.
    pub outcome: CaseOutcome,
}

/// One way a batch's results contradict the verdicts the gate reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateViolation {
    /// A case the gate did not clear was reported as having run.
    ///
    /// This is the assertion the gate exists for: a blocked or unverifiable case recorded as a
    /// pass claims a verification that never happened, and one recorded as a failure claims an
    /// exercise that never happened.
    ClaimedRun {
        /// The case.
        id: String,
        /// The verdict the gate reached before the run.
        verdict: Executability,
        /// What the run reported.
        outcome: CaseOutcome,
    },
    /// A case the gate cleared was never run.
    ///
    /// Not a false pass, but a hole in the coverage a batch's counts would otherwise claim: a
    /// case that was cleared and then quietly not run is the silent skip the gate forbids.
    SilentSkip {
        /// The case.
        id: String,
    },
    /// A result carries a verdict the gate does not reach for its case.
    ///
    /// The re-derived verdict wins. A batch audited against the verdicts its own results state
    /// agrees with itself, so a runner that recorded a withheld case as runnable would be
    /// reported clean; this variant is that case caught, and it is the reason
    /// [`audit_batch_against`] exists beside [`audit_batch`].
    VerdictMismatch {
        /// The case.
        id: String,
        /// The verdict the result carries.
        carried: Executability,
        /// The verdict the gate reaches for that case on this machine.
        derived: Executability,
    },
    /// A result names a case the batch does not hold.
    ///
    /// Nothing classified it, so no verdict about it was ever reached and nothing it reports may
    /// be believed -- neither a pass nor a failure.
    UnknownCase {
        /// The case.
        id: String,
    },
}

impl GateViolation {
    /// The case the violation is about.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn case(&self) -> &str {
        match self {
            Self::ClaimedRun { id, .. }
            | Self::SilentSkip { id }
            | Self::VerdictMismatch { id, .. }
            | Self::UnknownCase { id } => id,
        }
    }

    /// The violation as a report prints it, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::ClaimedRun {
                id,
                verdict,
                outcome,
            } => format!(
                "{id}: the gate says {} ({}), but the run reported {}",
                verdict.marker(),
                verdict.describe(),
                outcome.label()
            ),
            Self::SilentSkip { id } => {
                format!("{id}: the gate cleared this case and no result was reported for it")
            }
            Self::VerdictMismatch {
                id,
                carried,
                derived,
            } => format!(
                "{id}: the result records {}, but the gate reaches {} ({})",
                carried.marker(),
                derived.marker(),
                derived.describe()
            ),
            Self::UnknownCase { id } => {
                format!("{id}: the batch holds no case with that identifier")
            }
        }
    }
}

/// A case the gate did not clear, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Withheld {
    /// The case identifier.
    pub id: String,
    /// The module the case belongs to.
    pub module: String,
    /// The requirement the case declared, which is where the remedy is looked up.
    pub requirement: EnvRequirement,
    /// The verdict, which carries the reason and what the environment would have to be.
    pub verdict: Executability,
}

impl Withheld {
    /// What would let the case run here, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn remedy(&self) -> &'static str {
        self.requirement.remedy()
    }

    /// The case, its marker, its reason and its remedy, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        format!(
            "{} [{}] {} {} -- {}",
            self.id,
            self.module,
            self.verdict.marker(),
            self.verdict.describe(),
            self.remedy()
        )
    }
}

/// A batch's three counts, every case it withheld and why, and the audit's findings.
///
/// The counts are the three states of [`Executability`] and they sum to the batch's size, so a
/// report that quotes them quotes the whole batch. A batch of cases with no results yet is a
/// plan: it has counts and reasons and no violations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchReport {
    /// Cases the gate cleared.
    pub runnable: usize,
    /// Cases blocked on code that does not exist yet.
    pub blocked: usize,
    /// Cases that need an environment this machine is not.
    pub unverifiable: usize,
    /// Every case the gate did not clear, in the order the batch listed it.
    pub withheld: Vec<Withheld>,
    /// What [`audit_batch`] found in the results.
    pub violations: Vec<GateViolation>,
}

impl BatchReport {
    /// Classifies a batch of cases and audits the results the runs reported.
    ///
    /// The counts come from the cases and the environment report. The violations come from the
    /// results, judged against the verdict the gate *re-derives* for each case rather than the
    /// one the result carries: see [`audit_batch_against`], which is the audit this report is
    /// built on. A batch judged by the verdicts its own results state would agree with itself.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(cases: &[TestCaseMeta], results: &[CaseResult], env: &EnvCapabilities) -> Self {
        let mut report = Self {
            runnable: 0,
            blocked: 0,
            unverifiable: 0,
            withheld: Vec::new(),
            violations: audit_batch_against(cases, results, env),
        };
        for case in cases {
            let verdict = classify(case, env);
            match &verdict {
                Executability::Runnable => report.runnable += 1,
                Executability::Blocked { .. } => report.blocked += 1,
                Executability::Unverifiable { .. } => report.unverifiable += 1,
            }
            if !verdict.is_runnable() {
                report.withheld.push(Withheld {
                    id: case.id.clone(),
                    module: case.module.clone(),
                    requirement: case.requirement.clone(),
                    verdict,
                });
            }
        }
        report
    }

    /// How many cases the batch held.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn total(&self) -> usize {
        self.runnable + self.blocked + self.unverifiable
    }

    /// The report as lines of text, for `xtask` to print and a delivery report to quote.
    ///
    /// The shape is stable: one line of counts, then one line per withheld case carrying its
    /// marker, its requirement, the reason and the remedy, then one line per violation. Nothing
    /// here is user input -- a case identifier, a task identifier, an environment description
    /// and a remedy are the whole of it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lines(&self) -> Vec<String> {
        let total = self.total();
        let mut lines = vec![format!(
            "cases: {} total, {} runnable, {} blocked, {} unverifiable",
            total, self.runnable, self.blocked, self.unverifiable
        )];
        for case in &self.withheld {
            lines.push(format!("withheld {}", case.describe()));
        }
        for violation in &self.violations {
            lines.push(format!("violation {}", violation.describe()));
        }
        lines
    }
}

/// Maps a case's declared environment requirement to a verdict for this machine.
///
/// The verdict is a rule over the report rather than a constant, and the difference matters:
/// see the module documentation. A requirement the decision table does not declare is withheld.
///
/// # Panics
///
/// Never.
pub fn classify(case: &TestCaseMeta, env: &EnvCapabilities) -> Executability {
    table::verdict_for(&case.requirement, env)
}

/// Finds every result whose outcome contradicts the verdict it carries.
///
/// This is the comparison a bare result list can make: the verdict is the one the result states.
/// [`audit_batch_against`] is the stronger form, for the caller that still holds the cases and
/// the report and can therefore re-derive it.
///
/// Every violation is reported rather than the first, because a batch that recorded four
/// withheld cases as passes has four things to fix and one run has to show all four.
///
/// # Panics
///
/// Never.
pub fn audit_batch(results: &[CaseResult]) -> Vec<GateViolation> {
    let mut violations = Vec::new();
    for result in results {
        audit_outcome(result, &mut violations);
    }
    violations
}

/// Finds every result that contradicts the verdict the gate reaches for its case.
///
/// This is [`audit_batch`] plus the comparison a bare result list cannot make: the verdict is
/// re-derived from the case's own declaration and the environment report, and a result that
/// carries a different one is reported as [`GateViolation::VerdictMismatch`]. The difference is
/// the whole anti-cheat value of the gate -- a batch audited against the verdicts its own
/// results state agrees with itself, so a runner that recorded a withheld case as a pass would
/// be reported clean and a pass the gate never allowed would be believed. The re-derived verdict
/// wins, and the report passed here is the one the batch is judged against.
///
/// A result that names no case of the batch is reported as [`GateViolation::UnknownCase`]:
/// nothing classified it, so nothing about it may be believed either.
///
/// # Panics
///
/// Never.
pub fn audit_batch_against(
    cases: &[TestCaseMeta],
    results: &[CaseResult],
    env: &EnvCapabilities,
) -> Vec<GateViolation> {
    let by_id: BTreeMap<&str, &TestCaseMeta> =
        cases.iter().map(|case| (case.id.as_str(), case)).collect();
    let mut violations = Vec::new();
    for result in results {
        audit_outcome(result, &mut violations);
        let Some(case) = by_id.get(result.id.as_str()).copied() else {
            violations.push(GateViolation::UnknownCase {
                id: result.id.clone(),
            });
            continue;
        };
        let derived = classify(case, env);
        if derived != result.verdict {
            violations.push(GateViolation::VerdictMismatch {
                id: result.id.clone(),
                carried: result.verdict.clone(),
                derived,
            });
        }
    }
    violations
}

/// Reports what one result's outcome says against the verdict it carries.
///
/// # Panics
///
/// Never.
fn audit_outcome(result: &CaseResult, violations: &mut Vec<GateViolation>) {
    if result.verdict.is_runnable() {
        if result.outcome == CaseOutcome::NotRun {
            violations.push(GateViolation::SilentSkip {
                id: result.id.clone(),
            });
        }
        return;
    }
    if result.outcome != CaseOutcome::NotRun {
        violations.push(GateViolation::ClaimedRun {
            id: result.id.clone(),
            verdict: result.verdict.clone(),
            outcome: result.outcome,
        });
    }
}

/// Reads a document as UTF-8 text.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
///
/// # Panics
///
/// Never.
fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}
