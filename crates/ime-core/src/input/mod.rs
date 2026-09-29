//! The input string of one composing session: its buffer, caret and Backspace
//! semantics.
//!
//! Responsibility: hold what the user has typed so far, where the caret sits, and
//! the syllable boundaries the latest segmentation reported, and decide what one
//! Backspace removes and how far the caret may travel.
//!
//! Boundaries: the buffer owns text and nothing else. It does not normalize (the
//! segmentation does that when it builds its graph), it holds no dictionary, no
//! clock and no global state, so a session's keystrokes replay deterministically in
//! a test with no display server involved.
//!
//! # Backspace
//!
//! Backspace removes the unit in front of the caret: the whole trailing syllable
//! while the caret sits at the end of the input, one character when it sits inside
//! it. The syllable grid is the one the caller writes back from the segmentation,
//! so the key deletes what the user saw the segmentation to be.
//!
//! # Caret
//!
//! The caret only ever lands on a syllable boundary, never inside a syllable.
//! Character-level cursor editing is deferred to a later phase: a caret that could
//! sit inside a syllable would make "delete the trailing syllable" ambiguous, and
//! resolving it needs a re-segmentation round the decoder does not do per keystroke
//! yet.

pub mod buffer;

pub use crate::input::buffer::{BackspaceOutcome, InputBuffer};
