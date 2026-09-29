//! The refusals of the review channel.

/// Why a review request could not be built.
///
/// The two variants are the two inputs a request resolves before a prompt can be rendered: the
/// case it belongs to and the snapshot it audits. Both are refused rather than repaired, because
/// the prompt quotes them back to the reviewer. A case identifier that had been rewritten would
/// file the review under a name nobody looks for, and a snapshot path that had been guessed would
/// point the reviewer at a file that may not be the one on screen -- and a review of the wrong
/// image is worse than no review, because it looks like evidence.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReviewError {
    /// The case identifier is not one the test suite could have named.
    #[error(
        "`{case_id}` is not a case identifier: the suite names its cases `TC-<MODULE>-<NUMBER>`, \
         as in `TC-UI-16`"
    )]
    BadCaseId {
        /// The identifier that was refused.
        case_id: String,
    },

    /// The snapshot's path is not one the evidence archive laid out.
    #[error(
        "`{path}` is not a snapshot of the evidence archive: the layout is \
         `RUN/<module>/<TC-ID>/<step>_<state>.png`"
    )]
    BadSnapshotPath {
        /// The path that was refused.
        path: String,
    },
}
