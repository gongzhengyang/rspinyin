//! Everything the evidence archive can refuse to do.
//!
//! Each variant is a verdict a caller acts on differently: a legacy layout means the results
//! tree has to be moved before anything is written, a run that already exists means the clock
//! has to be waited out, and an unwritable file means the case has evidence missing. A single
//! "the evidence could not be written" would leave the reader to work that out from a string.
//!
//! # The reasons the index carries are path-free
//!
//! [`EvidenceError::reason`] renders a one-line description without any filesystem path in it,
//! because that line is what a run's `index.md` records and the index is itself evidence that
//! may be attached to a bug report. A path can name the operator, and a reviewer needs to know
//! what went wrong rather than where the harness keeps its files. The full path is in the
//! error's own `Display`, which is what a terminal prints and what an operator reads.

use std::path::PathBuf;

use crate::testd::capture::CaptureError;

/// Everything the evidence archive can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    /// A file or directory could not be created, written or read.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// What the filesystem reported.
        #[source]
        source: std::io::Error,
    },

    /// The results tree is one of the flat layouts `dev-check` files as legacy.
    ///
    /// Writing there would produce a bundle the report layer moves elsewhere, so the path is
    /// refused before anything is created under it.
    #[error(
        "`{path}` is the legacy `{found}` layout: evidence belongs under \
         `results/runs/run-<stamp>/<module>/<TC-ID>/`, and `dev-check` archives anything it \
         finds in a flat `screenshots` or `traces` directory"
    )]
    LegacyLayout {
        /// The path that was refused.
        path: PathBuf,
        /// The legacy directory the path names.
        found: String,
    },

    /// A run of this name is already in the results tree.
    ///
    /// A run is named for the second it started in, so this is what a second run started inside
    /// the same second looks like. Refusing is the point: two runs sharing a directory would
    /// interleave their cases, and a case would then be read against another run's pixels.
    #[error(
        "a run directory at {path} already exists; wait for the next second, or move the \
         finished run aside"
    )]
    RunExists {
        /// The run directory that is already there.
        path: PathBuf,
    },

    /// A document could not be rendered as JSON.
    #[error("{document} could not be rendered as JSON: {detail}")]
    Json {
        /// The document that was being written.
        document: String,
        /// What the encoder reported.
        detail: String,
    },

    /// A path segment a case's directory is built from is not usable.
    ///
    /// The rules are the screenshot channel's, which owns the `RUN/<module>/<TC-ID>/` shape;
    /// this variant carries its refusal rather than a second copy of those rules.
    #[error("{0}")]
    Capture(#[from] CaptureError),

    /// A passing case was handed a failure trace.
    #[error(
        "the case {tc} of {module} passed and was handed a trace; a trace is written for a \
         failed or flawed case only, so an empty one never has to be skipped by a reader"
    )]
    UnexpectedTrace {
        /// The module code of the case.
        module: String,
        /// The case's identifier.
        tc: String,
    },

    /// A failed or flawed case was handed no trace.
    #[error(
        "the case {tc} of {module} did not pass and was handed no trace; a failure without \
         its diffs is an evidence gap, not a verdict"
    )]
    MissingTrace {
        /// The module code of the case.
        module: String,
        /// The case's identifier.
        tc: String,
    },

    /// The cross-run index could not be read.
    ///
    /// A document whose format version this build does not know arrives here too: reading
    /// fields whose meaning may have changed would be worse than refusing, and overwriting the
    /// document would destroy the history of every run before this one.
    #[error("{path} is not a cross-run index this build understands: {detail}")]
    Index {
        /// The file that was read.
        path: PathBuf,
        /// What was wrong with it.
        detail: String,
    },
}

impl EvidenceError {
    /// A one-line reason with no path in it, for a run's index to carry.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reason(&self) -> String {
        match self {
            Self::Io { source, .. } => {
                format!("the evidence could not be written: {source}")
            }
            Self::LegacyLayout { found, .. } => {
                format!("the results tree is the legacy `{found}` layout")
            }
            Self::RunExists { .. } => {
                String::from("a run of this name is already in the results tree")
            }
            Self::Json { document, detail } => {
                format!("{document} could not be rendered as JSON: {detail}")
            }
            // The only refusal that reaches here is a segment refusal: the shape is built
            // before any file is touched, and a segment carries the case's own name rather
            // than a path.
            Self::Capture(error) => format!("a case path is not usable: {error}"),
            Self::UnexpectedTrace { .. } => {
                String::from("a passing case was handed a failure trace")
            }
            Self::MissingTrace { .. } => String::from("a failed case left no failure trace"),
            Self::Index { detail, .. } => {
                format!("the cross-run index could not be read: {detail}")
            }
        }
    }
}

/// Renders one document as the JSON text its file holds.
///
/// Pretty-printed rather than compact: these documents are read by a person in a bug report far
/// more often than by a program, and the cost is a file nobody has to run a formatter over.
///
/// # Errors
///
/// Returns [`EvidenceError::Json`] when the encoder refuses a field.
///
/// # Panics
///
/// Never.
pub(super) fn encode<T: serde::Serialize>(
    document: &str,
    value: &T,
) -> Result<String, EvidenceError> {
    serde_json::to_string_pretty(value).map_err(|error| EvidenceError::Json {
        document: document.to_owned(),
        detail: error.to_string(),
    })
}
