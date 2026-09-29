//! Everything a locator can refuse to do.
//!
//! Responsibility: name each refusal and render a message a developer can act on. Splitting this
//! out of [`super`] keeps the locators' own files about the locators: nothing here reads a
//! source, and nothing here decides whether a locator healed.
//!
//! The two variants answer two different questions, and telling them apart is what lets a failure
//! be attributed rather than merely reported. [`HealError::Unresolvable`] is a *locator* that no
//! longer names anything and cannot be re-derived from the specification: a test script to
//! repair. Refusing rather than guessing is what this module is built around -- a harness that
//! healed towards the nearest match would report a passing case about a window that does not
//! exist. [`HealError::Drifted`] is the other side of the same line: the name moved and the
//! declaration that took it over carries another value, so the *source* has moved away from the
//! specification. That is a finding about the product and never a repair, and a report that mixed
//! the two would send a reader to the wrong file.

/// Everything a locator can refuse to do.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HealError {
    /// The locator names something the two sources do not state, and nothing here guesses at it.
    #[error("no locator resolves {wanted}: {detail}")]
    Unresolvable {
        /// What the locator named.
        wanted: String,
        /// Why it cannot be resolved, in a form a developer can act on.
        detail: String,
    },

    /// The name a locator used moved, and the value it named moved with it.
    ///
    /// A name that moved on its own is a rename and heals; a value that moved on its own is drift
    /// and is left to the cross-check. Both at once is neither, and following the new number would
    /// cover exactly the change the specification exists to catch, so the locator refuses and says
    /// which declaration it refused to follow.
    #[error(
        "the constant `{name}` is declared as `{declared_name}`, which carries {declared} where \
         the specification states {spec}: a value that moved is drift and not a rename"
    )]
    Drifted {
        /// The name the locator used, which the metrics block no longer declares.
        name: String,
        /// The name the metrics block declares now.
        declared_name: String,
        /// The value the declaration carries, as a report writes it.
        declared: String,
        /// The value the specification's row states, as a report writes it.
        spec: String,
    },
}
