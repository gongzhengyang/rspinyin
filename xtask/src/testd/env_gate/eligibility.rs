//! The gate's machine-readable verdict: the three states the ledger records.
//!
//! Responsibility: reduce the verdict the decision table reaches -- and the case card's own
//! pending-implementation annotation, which decides over it -- to the three-state answer the
//! tests v3.0 ledger consumes, and render the claim a withheld case's record carries. Boundaries:
//! this file invents no verdict of its own; [`eligibility_of`] asks the table and the card and
//! maps what they say.
//!
//! # Three states, machine-readable
//!
//! [`Executability`](super::Executability) is what a human reads: it carries a reason in prose
//! and the environment a withheld case would need. [`CaseEligibility`] is what the ledger
//! records:
//!
//! * [`CaseEligibility::Runnable`] -- the case may run here, and a pass it reports may be
//!   believed.
//! * [`CaseEligibility::PendingImpl`] -- the case is legal, its subject does not exist yet,
//!   and the ledger must say so instead of failing it. `task` names the task the case is
//!   anchored to, read from the case card's own `锚定` field; the wait ends by itself as that
//!   task lands and the card is flipped, with no gate edit.
//! * [`CaseEligibility::NeedsEnvironment`] -- code exists, this machine cannot judge it
//!   (Wayland tiers, real blur, multi-monitor, an eight-hour soak). `remedy` is the decision
//!   table's remedy verbatim, as v1 cited it.
//!
//! # Why `task` is owned and `remedy` is not
//!
//! The card contract wrote both fields `&'static str`, and only half of that survives contact
//! with the probe: the anchored tasks are read from the case card at run time, and a static
//! lifetime would require this module to enumerate every features-add task -- precisely the
//! hardcoded list the verdict table must never become. The remedy keeps its static lifetime
//! honestly: it is the table's own citation, never a paraphrase built at run time.

use crate::testd::env::EnvCapabilities;
use crate::testd::evidence::Claim;

use super::{Executability, TestCaseMeta, classify};

/// The name the eligibility claim carries in a case's v3 record.
pub const ELIGIBILITY_CLAIM: &str = "eligibility";

/// The gate's machine-readable verdict for one case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaseEligibility {
    /// The case may run here.
    Runnable,
    /// The implementation behind the case is not merged yet; `task` is the task id, or the
    /// `/`-joined span of ids, the case card anchors it to.
    PendingImpl {
        /// The anchored task, e.g. `TASK-1.03.02`.
        task: String,
    },
    /// Code exists, this machine cannot judge it. The remedy is mandatory and is the decision
    /// table's own text, never a paraphrase.
    NeedsEnvironment {
        /// What to do about it, in one line.
        remedy: &'static str,
    },
}

impl CaseEligibility {
    /// The label the ledger records.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Runnable => "runnable",
            Self::PendingImpl { .. } => "pending-impl",
            Self::NeedsEnvironment { .. } => "needs-environment",
        }
    }

    /// The ledger claim the verdict lands as, or `None` for a runnable case.
    ///
    /// The claim never passes -- a withheld case settled no claim -- and its `actual` names
    /// the anchor task or the remedy, which is the part a reader acts on. A runnable case
    /// makes no claim here: its own assertions are its record.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn claim(&self) -> Option<Claim> {
        let claim = match self {
            Self::Runnable => return None,
            Self::PendingImpl { task } => Claim {
                name: String::from(ELIGIBILITY_CLAIM),
                expect: String::from("the case's subject exists on this machine"),
                actual: format!("pending-impl: {task}"),
                pass: false,
            },
            Self::NeedsEnvironment { remedy } => Claim {
                name: String::from(ELIGIBILITY_CLAIM),
                expect: String::from("an environment that can exercise the case"),
                actual: format!("needs-environment: {remedy}"),
                pass: false,
            },
        };
        Some(claim)
    }
}

/// The machine-readable verdict the gate reaches for `case` on this machine.
///
/// The case card's own `可执行性：[待实现]` annotation decides over the decision table: a case
/// whose subject does not exist is pending implementation wherever it runs, and no report can
/// change that. Everything else is the table's answer, mapped -- a row that waits on code
/// lands as [`CaseEligibility::PendingImpl`] with the tasks the row names, and a withholding
/// row lands as [`CaseEligibility::NeedsEnvironment`] with the row's remedy verbatim.
///
/// # Panics
///
/// Never.
pub fn eligibility_of(case: &TestCaseMeta, env: &EnvCapabilities) -> CaseEligibility {
    if let Some(task) = &case.pending_impl {
        return CaseEligibility::PendingImpl { task: task.clone() };
    }
    match classify(case, env) {
        Executability::Runnable => CaseEligibility::Runnable,
        Executability::Blocked { task, .. } => CaseEligibility::PendingImpl { task },
        Executability::Unverifiable { .. } => CaseEligibility::NeedsEnvironment {
            remedy: case.requirement.remedy(),
        },
    }
}
