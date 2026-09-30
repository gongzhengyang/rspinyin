//! The states a verification passes through, and the steps of the lifecycle it does not take.
//!
//! Responsibility: make the delivery contract's state machine explicit in code -- the states,
//! the transition a passing check makes, and the codes a failing one leaves under -- and
//! record, just as explicitly, the three lifecycle steps this command refuses to take.
//!
//! # The machine
//!
//! The delivery contract in `docs/dev/opt-deploy.md` draws it as six states, S0 to S5:
//!
//! | State | Check | Codes it can fail under |
//! |---|---|---|
//! | S1 manifest parse | the document is JSON, declares version 1, and matches the schema | `dist/manifest/unsupported-version`, `dist/manifest/malformed` |
//! | S2 digest | every recorded digest, size and ceiling agrees with the file | `dist/verify/digest-mismatch`, `dist/verify/artifact-missing`, `dist/verify/size-budget-exceeded` |
//! | S3 signature | the detached signature verifies against the local keyring | `dist/verify/signing-key-absent`, `dist/verify/signature-invalid` |
//! | S4 contents | the libraries export what the manifest lists, the dictionary reads | `dist/verify/factory-symbol-missing`, `dist/verify/dictionary-invalid` |
//! | S5 verdict | nothing is left to check | -- |
//!
//! A check runs before the machine moves, so a refusal leaves the machine in the state that
//! refused: what the caller reports as "reached" is what actually passed.

use super::error::VerifyError;

/// One state of the verification state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// S0: nothing has been read.
    #[default]
    Start,
    /// S1: the manifest parsed and declares a schema version this build understands.
    ManifestParsed,
    /// S2: every digest, size and size ceiling agreed with the records for it.
    DigestsChecked,
    /// S3: the detached signature verified against a key in the local keyring.
    SignatureChecked,
    /// S4: every library exports what the manifest lists, and the dictionary reads.
    ContentsChecked,
    /// S5: the verdict is that the release is intact.
    Verified,
}

impl State {
    /// The state a passing check moves to.
    ///
    /// [`State::Verified`] is terminal and maps to itself, so a machine that has reached a
    /// verdict cannot be moved back into an unfinished state by a later call.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn next(self) -> Self {
        match self {
            Self::Start => Self::ManifestParsed,
            Self::ManifestParsed => Self::DigestsChecked,
            Self::DigestsChecked => Self::SignatureChecked,
            Self::SignatureChecked => Self::ContentsChecked,
            Self::ContentsChecked | Self::Verified => Self::Verified,
        }
    }

    /// The label the contract's diagram writes for this state.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "S0 not started",
            Self::ManifestParsed => "S1 manifest parsed",
            Self::DigestsChecked => "S2 digests checked",
            Self::SignatureChecked => "S3 signature checked",
            Self::ContentsChecked => "S4 symbols and dictionary checked",
            Self::Verified => "S5 verified",
        }
    }
}

/// A step of the delivery lifecycle, including the three this command does not take.
///
/// The full lifecycle is fetch, check, apply, roll back. Three of those four steps have no
/// implementation here, and that is a property of the product rather than an omission: there
/// is no update channel to fetch from, and a component that could rewrite its own
/// installation would be a code path a network-reachable program could drive. Naming them in
/// the same enum as the one step that does run is what makes the absence visible instead of
/// leaving a reader to infer it from what is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// Obtain the artifacts.
    ///
    /// Absent by design: the user obtains a release however they obtained it -- a
    /// distribution package, a mirror, a copy from the maintainer -- and this command has no
    /// channel of its own to look for a newer one in.
    Fetch,
    /// Check the artifacts against the records published beside them.
    Check,
    /// Write the artifacts into the system.
    ///
    /// Absent by design: the distribution's package manager owns this, and it is the only
    /// thing that may own it.
    Apply,
    /// Put the system back as it was before an apply.
    ///
    /// Absent for the same reason as [`Lifecycle::Apply`], and it has the same owner: a
    /// rollback is an uninstall and a reinstall of a pinned version, which is a package
    /// manager operation.
    Rollback,
}

impl Lifecycle {
    /// Every step, in the order the lifecycle runs them.
    pub const ALL: [Self; 4] = [Self::Fetch, Self::Check, Self::Apply, Self::Rollback];

    /// The label a report prints for this step.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "the fetch",
            Self::Check => "the check",
            Self::Apply => "the apply",
            Self::Rollback => "the rollback",
        }
    }

    /// Who performs this step.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn owner(self) -> &'static str {
        match self {
            Self::Fetch => "the user",
            Self::Check => "xtask verify",
            Self::Apply | Self::Rollback => "the package manager",
        }
    }

    /// Whether this command performs this step.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_implemented(self) -> bool {
        matches!(self, Self::Check)
    }
}

/// The state a verification has reached, and the transition that moves it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Machine {
    /// The state the checks so far have left the machine in.
    state: State,
}

impl Machine {
    /// A machine that has not started.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }

    /// The state the machine is in.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn state(&self) -> State {
        self.state
    }

    /// Runs the check that leaves the current state, advancing only when it passes.
    ///
    /// The check's own value is returned on success. A failing check leaves the machine where
    /// it was, so the state it reports afterwards is the one that refused rather than the one
    /// that would have followed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn advance<T>(
        &mut self,
        check: impl FnOnce() -> Result<T, VerifyError>,
    ) -> Result<T, VerifyError> {
        let value = check()?;
        self.state = self.state.next();
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::verify::error::Code;

    #[test]
    fn test_state_next_walks_every_state_in_order_and_stops() {
        let mut state = State::Start;
        let mut visited = vec![state];
        while state.next() != state {
            state = state.next();
            visited.push(state);
        }
        assert_eq!(
            visited,
            [
                State::Start,
                State::ManifestParsed,
                State::DigestsChecked,
                State::SignatureChecked,
                State::ContentsChecked,
                State::Verified,
            ]
        );
    }

    #[test]
    fn test_state_verified_is_absorbing() {
        // A machine that has reached its verdict cannot be walked back into an unfinished
        // state, which is what keeps a late check from re-opening a decision.
        assert_eq!(State::Verified.next(), State::Verified);
    }

    #[test]
    fn test_state_labels_name_the_contracts_states() {
        assert!(State::Start.label().starts_with("S0"));
        assert!(State::ManifestParsed.label().starts_with("S1"));
        assert!(State::DigestsChecked.label().starts_with("S2"));
        assert!(State::SignatureChecked.label().starts_with("S3"));
        assert!(State::ContentsChecked.label().starts_with("S4"));
        assert!(State::Verified.label().starts_with("S5"));
    }

    #[test]
    fn test_machine_advance_moves_only_on_success() {
        let mut machine = Machine::new();
        assert_eq!(machine.state(), State::Start);
        let parsed = machine.advance(|| Ok::<u8, VerifyError>(7));
        assert_eq!(parsed.expect("the check passed"), 7);
        assert_eq!(machine.state(), State::ManifestParsed);
    }

    #[test]
    fn test_machine_advance_leaves_a_refusal_in_the_state_that_refused() {
        let mut machine = Machine::new();
        let failure = machine
            .advance(|| Err::<(), _>(VerifyError::rejected(Code::Malformed, "not JSON")))
            .expect_err("the check failed");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
        assert_eq!(
            machine.state(),
            State::Start,
            "a refusal reports the state that refused, not the one that would have followed"
        );
    }

    #[test]
    fn test_lifecycle_implements_the_check_step_alone() {
        // The three steps this command does not take are the product promise, not an
        // omission: naming them is what makes their absence assertable.
        let implemented: Vec<Lifecycle> = Lifecycle::ALL
            .into_iter()
            .filter(|step| step.is_implemented())
            .collect();
        assert_eq!(implemented, [Lifecycle::Check]);
        assert_eq!(Lifecycle::Fetch.owner(), "the user");
        assert_eq!(Lifecycle::Apply.owner(), Lifecycle::Rollback.owner());
    }
}
