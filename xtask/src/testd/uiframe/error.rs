//! Everything the frame snapshot channel can refuse to do.
//!
//! Responsibility: name each refusal, carry the path it is about, and render a message a
//! developer can act on. Splitting this out of [`super`] keeps the channel's own file about
//! the channel: nothing here touches the filesystem and nothing here reads a frame.
//!
//! Boundaries: it is a value type. No message it renders carries the text of a frame -- a
//! snapshot holds what the user typed, and an error message is the one place that content
//! must not reach, because messages are printed into terminals and archived as evidence.

use std::path::PathBuf;

/// Everything the snapshot channel can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// A snapshot file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The failure the operation reported.
        #[source]
        source: std::io::Error,
    },

    /// A file exists but is not a snapshot this build can read.
    #[error("{path}: not a readable frame snapshot ({detail})")]
    Malformed {
        /// The file that could not be read.
        path: PathBuf,
        /// What the parser objected to, named without repeating the file's own content.
        detail: String,
    },

    /// A file declares a snapshot format this build does not know.
    #[error(
        "{path}: frame snapshot format {found}, but this build writes and reads format \
         {expected}; the file was left behind by another build of the harness"
    )]
    Format {
        /// The file that could not be read.
        path: PathBuf,
        /// The version the file declares.
        found: u32,
        /// The version this build writes and reads.
        expected: u32,
    },

    /// A frame cannot be rendered as a snapshot.
    ///
    /// The frame is refused rather than written, because a snapshot that no reader can open
    /// would replace the last good one with a file every case refuses.
    #[error("the frame cannot be written as a snapshot: {detail}")]
    Encode {
        /// What the renderer objected to.
        detail: String,
    },
}
