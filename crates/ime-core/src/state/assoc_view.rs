//! The association view data split out of `machine.rs` to respect the file limit.

use crate::assoc::AssocHit;

/// An association candidate view held after a successful commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AssocView {
    /// The commit text the providers were queried after.
    pub(super) previous: String,
    /// The merged hits, in the order rendered to the user.
    pub(super) hits: Vec<AssocHit>,
}
