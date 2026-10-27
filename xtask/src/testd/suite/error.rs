//! Everything the case-suite runner refuses to do.
//!
//! Each variant is a verdict a caller acts on differently: an identifier the table does not
//! hold means the invocation is wrong and nothing has run yet, a module the table does not
//! cover means the suite has not been written for that module yet, and a run that did not come
//! out green means the cases themselves have something to say. A single "the suite failed"
//! would leave a CI job to work that out from a string.
//!
//! # The codes are stable
//!
//! [`SuiteError::code`] renders the `domain/action/reason` string the diagnostics contract
//! fixes. A CI job matches on that string rather than on the message, so the message is free to
//! get clearer while the code stays put -- and rewording a code is a contract change, not an
//! edit.

use crate::testd::evidence::BatchVerdict;

/// Everything the case-suite runner can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum SuiteError {
    /// A selection named a case the table does not hold.
    ///
    /// The whole selection is refused rather than the unknown identifier being dropped: a run
    /// that silently ran the identifiers it recognised would publish a green verdict for a
    /// command that asked for a case nobody ran.
    #[error(
        "the suite holds no case `{tc}` in module `{module}`; the identifiers are the ones \
         `docs/dev/tests.md` names, e.g. `TC-CORE-01`"
    )]
    UnknownCase {
        /// The identifier the caller named.
        tc: String,
        /// The module it was looked for in.
        module: String,
    },

    /// A selection named no case at all.
    ///
    /// An empty run is refused rather than reported green: a batch of zero cases has no verdict
    /// to give, and a run that published `pass` for it would claim a verification that never
    /// happened.
    #[error(
        "no case of module `{module}` is in the suite, so this run would have nothing to \
         judge; the module code is the one the case document's own metadata states"
    )]
    NoCases {
        /// The module code that selected nothing.
        module: String,
    },

    /// The tier the run asked for holds no case of this module.
    ///
    /// A tier asked for by name is a claim about a set of cases, and a set of zero is not a
    /// green run waiting to happen: refusing names the ladder's actual state instead of letting
    /// a recipe report a verification of nothing. The acceptance tier's *gate* phase refuses
    /// the same way, because an acceptance run whose fast set is empty has no gate at all.
    #[error(
        "no case of module `{module}` is filed under the {tier} tier, so this run would have \
         nothing to judge; tiers arrive as edits to the case table's own tier column"
    )]
    EmptyTier {
        /// The tier that selected nothing, as its label spells it.
        tier: &'static str,
        /// The module code that holds no case of the tier.
        module: String,
    },

    /// The stress tier refused to start because the machine is not exclusive.
    ///
    /// The stress tier measures the machine, so a number it collected under load would be a
    /// number about the load; refusing names what the machine was doing instead of collecting
    /// it anyway.
    #[error("the stress tier refuses a busy machine: {reason}")]
    MachineBusy {
        /// What the machine was doing when it was sampled.
        reason: String,
    },

    /// The batch held no case at all.
    ///
    /// The last gate before a run's exit status, and it fails closed: a batch of zero cases is a
    /// run that judged nothing, and publishing `pass` for it would claim a verification that
    /// never happened. The selection refuses an empty one before it is built, so this is the
    /// check that keeps the promise true if a caller ever reaches the batch another way.
    #[error(
        "the batch holds no case, so it has no verdict to give; a run that judged nothing may \
         not be published as one that passed"
    )]
    EmptyBatch,

    /// The run finished with cases that failed or could not be judged.
    ///
    /// The run's own verdict, carried as a value rather than as a message, so a caller can tell
    /// a run in which cases failed from one whose evidence is incomplete.
    #[error(
        "the run is {}: {passed} passed, {failed} failed, {flawed} could not be judged, \
         {gaps} evidence gap(s)",
        .verdict.label()
    )]
    BatchNotGreen {
        /// What the batch came to.
        verdict: BatchVerdict,
        /// How many cases passed.
        passed: usize,
        /// How many failed.
        failed: usize,
        /// How many could not be judged.
        flawed: usize,
        /// How many bundles are incomplete.
        gaps: usize,
    },
}

impl SuiteError {
    /// The stable `domain/action/reason` code, which diagnostics and tests match on.
    ///
    /// These strings are part of the diagnostic contract: never reword one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownCase { .. } => "suite/case/unknown",
            Self::NoCases { .. } => "suite/selection/empty",
            Self::EmptyTier { .. } => "suite/tier/empty",
            Self::MachineBusy { .. } => "suite/purity/busy",
            Self::EmptyBatch => "suite/batch/empty",
            Self::BatchNotGreen { .. } => "suite/batch/not-green",
        }
    }
}
