//! Everything the metrics channel can refuse to do.
//!
//! Responsibility: name each refusal, carry where it happened, and render a message a developer
//! can act on. Splitting this out of [`super`] keeps the channel's own file about the channel:
//! nothing here reads a document and nothing here compares a value.
//!
//! A document this module reads is a source file or the architecture spec, so unlike a log line
//! its text is not user content and may be quoted back. The detail strings do quote it -- that
//! is what makes a parse failure actionable -- and the one thing they never carry is a
//! *rendered* value from a file the harness did not read itself.

use std::path::PathBuf;

/// Everything the metrics channel can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum MetricError {
    /// A source could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The path the read was about.
        path: PathBuf,
        /// The failure the read reported.
        #[source]
        source: std::io::Error,
    },

    /// A specification table is not in the shape this harness reads.
    ///
    /// The tables of `features.md` 3.1 and 3.2 are the authority, and this reader anchors on
    /// their column names. A heading that moved, a column that was renamed, a row that gained a
    /// cell or a section that grew a second table is therefore a failure rather than a quietly
    /// smaller result: an empty table would make every assertion against it pass.
    #[error("{document}: {detail}")]
    SpecUnparsable {
        /// The document that was read.
        document: &'static str,
        /// What is wrong with it, and the shape that was expected.
        detail: String,
    },

    /// A `.slint` declaration cannot be read.
    #[error("{document}:{line}: {detail}")]
    SlintUnparsable {
        /// The document that was read, named the way the caller named it.
        document: String,
        /// The line the declaration is on, counting from one.
        line: usize,
        /// What could not be read.
        detail: String,
    },
}
