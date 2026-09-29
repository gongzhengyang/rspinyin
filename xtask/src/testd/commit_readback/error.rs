//! Everything the client-text channel can refuse to do.
//!
//! Responsibility: name each refusal, carry what a case needs to act on it, and render a message a
//! developer can read. Splitting this out of [`super`] keeps the channel's own file about the
//! channel: nothing here reads a line, starts a process or decides what a wait means.
//!
//! # The verdicts are the point
//!
//! A `CommitTimeout` means the plugin did not commit, a `ClientExited` means the client under test
//! is gone, and a `Protocol` means the harness and the client disagree about the wire. A single
//! "readback failed" would leave the reader to work that out from a string, which is why the
//! variants are named after the verdict rather than after the call that produced them.
//!
//! # Content is a field, never a message
//!
//! A refusal may carry a [`Transcript`] or the client's standard error, because a case has to be
//! able to see what arrived when what it wanted did not. Neither reaches the rendered message: a
//! message is printed into a terminal and archived as evidence, and the project's logging rules
//! put input content on the redaction denylist for exactly that reason. The messages here
//! therefore name counts, line numbers, capability sets and exit statuses, and say where the
//! content is attached.

use std::path::PathBuf;
use std::time::Duration;

use super::{ClientCaps, Transcript};

/// Everything this channel can refuse to do.
///
/// `PartialEq` is derived so a case can assert on a returned `Result` directly instead of matching
/// every variant by hand, the way the other channels' errors are written.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadbackError {
    /// The client program could not be started.
    #[error("the client {program} could not be started: {detail}")]
    ClientSpawn {
        /// The program that was named.
        program: PathBuf,
        /// What the operating system reported.
        detail: String,
    },

    /// The client ran but never announced itself: it did not write a `ready` line before the
    /// deadline, and it did not stop writing either.
    #[error(
        "the client did not announce itself within {timeout:?}, in {attempts} attempt(s); it must \
         write a `ready` line once its window is mapped, and its standard error is attached to \
         this error"
    )]
    ClientNotReady {
        /// The deadline one attempt was given.
        timeout: Duration,
        /// How many times it was started.
        attempts: u32,
        /// The tail of the client's standard error, as evidence.
        stderr: Vec<String>,
    },

    /// The client declared a different capability set than it was started with, so the run would
    /// assert about a path it never reached: the host decides whether to write a preedit into the
    /// client from the capabilities the client declared.
    #[error(
        "the client was started with capabilities `{asked}` and declared `{declared}`; the \
         capability set decides which paths the host takes, so this run would assert about a path \
         it never reached"
    )]
    CapsMismatch {
        /// The set the harness started it with.
        asked: ClientCaps,
        /// The set the client declared.
        declared: ClientCaps,
    },

    /// The client stopped writing before the event that was being waited for.
    #[error(
        "the client stopped writing before a commit arrived (exit status: {status}); its standard \
         error is attached to this error"
    )]
    ClientExited {
        /// How the client exited, or that it is still running.
        status: String,
        /// The tail of the client's standard error, as evidence.
        stderr: Vec<String>,
    },

    /// A line on the wire is not an event this protocol defines.
    #[error("line {line} of the client's output is not a protocol event: {detail}")]
    Protocol {
        /// Which line of the client's standard output it was, counting from one.
        line: usize,
        /// What is wrong with it, named without repeating what it held.
        detail: String,
    },

    /// No commit arrived before the deadline.
    #[error(
        "no commit arrived within {timeout:?}; {received} event(s) arrived and none of them was a \
         commit, and they are attached to this error as a transcript"
    )]
    CommitTimeout {
        /// The deadline the wait was given.
        timeout: Duration,
        /// How many events arrived instead.
        received: usize,
        /// Everything the client said while the wait was running.
        transcript: Transcript,
    },

    /// No preedit arrived before the deadline, which is how the `client_preedit = false` half of
    /// the policy is asserted.
    #[error(
        "no preedit arrived within {timeout:?}; {received} event(s) arrived and none of them was a \
         preedit, and they are attached to this error as a transcript"
    )]
    PreeditTimeout {
        /// The deadline the wait was given.
        timeout: Duration,
        /// How many events arrived instead.
        received: usize,
        /// Everything the client said while the wait was running.
        transcript: Transcript,
    },

    /// The client's output could not be read.
    #[error("the client's output could not be read: {detail}")]
    Stream {
        /// What the failing read reported.
        detail: String,
    },

    /// The stream was closed and was asked for another line.
    #[error("the client's stream is closed; a probe reads nothing after it is closed")]
    Closed,
}
