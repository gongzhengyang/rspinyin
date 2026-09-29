//! Everything the injection channel refuses to do.
//!
//! Responsibility: name each refusal, carry the path, the field or the step it is about,
//! and render a message a developer can act on.
//!
//! Boundaries: it is a value type. It decides nothing, touches no file and knows no layout;
//! the module that detects a condition is the module that builds the variant. Splitting it
//! out of [`super`] is what keeps the channel's own file about the channel.

use std::io;
use std::path::PathBuf;

use ime_types::DictError;

use super::{DictErrorKind, RefusalPoint};

/// Everything the injection channel refuses to do.
///
/// Every variant is a failure of the *fixture* rather than of the code under test: a source
/// that is not a container, a mutation that would leave the file valid, an assertion made
/// before anything was mutated. They are kept apart from the refusals the loader raises,
/// because a case has to be able to tell "the loader rejected the file" from "the harness
/// could not build one".
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// A file could not be copied, written or removed.
    #[error("{path}: {source}")]
    Io {
        /// The file the operation was performed on.
        path: PathBuf,
        /// What the filesystem reported.
        #[source]
        source: io::Error,
    },

    /// A container could not be read where the fixture needed to read it.
    #[error("reading {path} failed: {cause}")]
    Unreadable {
        /// The container that could not be read.
        path: PathBuf,
        /// Why the reader refused it.
        cause: DictError,
    },

    /// The in-memory image is not a container the reader accepts.
    ///
    /// This is the failure that says the fixture itself is wrong rather than the file: the
    /// image was accepted when the fixture was built, so it can only stop being a container
    /// if the mutation machinery damaged it in a way it did not intend.
    #[error("the container image is not one the reader accepts: {cause}")]
    ImageUnreadable {
        /// Why the reader refused it.
        cause: DictError,
    },

    /// A mutation named a range that leaves the image.
    #[error("the bytes at {at}..{end} leave the {len}-byte image")]
    OutOfImage {
        /// Where the write was aimed.
        at: usize,
        /// Where it ended.
        end: usize,
        /// How long the image is.
        len: usize,
    },

    /// An offset or a length does not fit this platform's address space.
    #[error("{field} = {value} does not fit this platform's address space")]
    Unfittable {
        /// The field that does not fit.
        field: &'static str,
        /// Its value.
        value: u64,
    },

    /// The change would leave the container valid, so the case would prove nothing.
    #[error("the {mutation} mutation would leave the container valid")]
    InertMutation {
        /// The mutation that changes nothing.
        mutation: &'static str,
    },

    /// The mutation does not apply to this container.
    #[error("the {mutation} mutation does not apply to this container: {reason}")]
    Inapplicable {
        /// The mutation that does not apply.
        mutation: &'static str,
        /// What the container lacks.
        reason: String,
    },

    /// An assertion was made before anything was mutated.
    #[error("{path} has not been mutated, so there is nothing to assert")]
    NoMutation {
        /// The copy that is still pristine.
        path: PathBuf,
    },

    /// The container was accepted where a refusal was expected.
    ///
    /// The public API cannot reach this by construction -- `mutate` already refuses a change
    /// that leaves the file valid -- so a case that sees it is looking at a container
    /// something else put back on disk.
    #[error(
        "the {mutation} mutation produced a container the loader accepted; \
         the fixture proves nothing"
    )]
    NotRejected {
        /// The mutation that was not caught.
        mutation: &'static str,
    },

    /// The refusal came from a different step than the mutation names.
    #[error("the {mutation} mutation was refused at {actual:?}, not at {expected:?}: {cause}")]
    WrongStep {
        /// The mutation that was applied.
        mutation: &'static str,
        /// The step the mutation names.
        expected: RefusalPoint,
        /// The step that actually raised the refusal.
        actual: RefusalPoint,
        /// What was raised.
        cause: DictError,
    },

    /// The refusal is a different kind than the case asserted.
    #[error("the {mutation} mutation was refused as {actual:?}, not as {expected:?}: {cause}")]
    WrongKind {
        /// The mutation that was applied.
        mutation: &'static str,
        /// The kind the case asserted.
        expected: DictErrorKind,
        /// The kind that was raised.
        actual: DictErrorKind,
        /// The error itself, so the field that was out of range is visible.
        cause: DictError,
    },

    /// The refusal is a bounds failure on a different field than the case asserted.
    ///
    /// Every length and offset failure shares one `DictError` variant, so the field is the
    /// only thing that tells a broken entry count from a broken section length; a case that
    /// asserted the variant alone could pass on a container refused for a reason it never
    /// meant to produce.
    #[error("the {mutation} mutation was refused on {actual}, not on {expected}")]
    WrongField {
        /// The mutation that was applied.
        mutation: &'static str,
        /// The field the case asserted.
        expected: &'static str,
        /// What the refusal said instead, rendered, so a non-bounds failure is visible too.
        actual: String,
    },

    /// The loader refused with a code that is not a dictionary failure.
    #[error("{what} was refused with {detail}, which is not a dictionary failure")]
    NonDictRefusal {
        /// What was refused.
        what: String,
        /// The code that was raised instead.
        detail: String,
    },

    /// The store behaved differently from what the case asserted.
    #[error("the {mutation} store {actual}; the case expected it to be {expected}")]
    WrongVerdict {
        /// The shape of damage that was placed.
        mutation: &'static str,
        /// What the case asserted.
        expected: &'static str,
        /// What the loader did.
        actual: String,
    },
}
