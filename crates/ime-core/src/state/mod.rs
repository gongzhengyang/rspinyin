//! The input session: the state machine the engine and the candidate window share.
//!
//! Responsibility: own everything that changes while the user types -- the input
//! buffer, the segmentation graph, the candidate list, the paging state and the
//! session's life cycle -- and decide, for one event at a time, what the host must do
//! about it. The window never derives anything the engine has not told it, which is
//! what makes this module the single source of truth between the two threads.
//!
//! Boundaries: this layer is pure (0.4 rule 4). It reaches the dictionary, the user's
//! frequencies and the language model only through the trait objects in
//! [`SessionEnv`], and it touches no file, no clock, no environment variable and no
//! global state. Everything the host must do leaves as an [`Effect`], so a whole
//! session replays in a test from literals and the host thread does nothing but
//! execute what it is handed.
//!
//! # Layout
//!
//! - [`paging`] owns the page, the page size and the highlighted candidate.
//! - [`machine`] owns the session, its states, its events and the effects.
//! - [`SessionConfig`] is the view of the configuration this layer acts on.
//!
//! # The configuration view
//!
//! `ime-config` owns the user's document, but it sits *above* this crate in the
//! one-way dependency order (`ime-types`, `ime-core`, `ime-dict`, `ime-config`), so
//! this crate cannot name its `Config`. The engine copies the handful of values the
//! session reads into [`SessionConfig`] and hands it to every [`machine::step`], which
//! is what keeps the machine from looking a configuration up mid-transition.
//!
//! # Reloading the configuration
//!
//! A reload never resets a session in progress (0.4 rule 10): the input buffer, the
//! candidate list and the commit that is in flight all survive it. Only the values
//! the session derives its behaviour from change, and only the ones a frame carries
//! -- the page size and the layout hint -- are visible at all. A reload that changes
//! none of them produces no effect, so reloading an unchanged file is idempotent.

mod boundaries;
pub mod machine;
pub mod paging;
mod transitions;

#[cfg(test)]
mod sweep_tests;
#[cfg(test)]
mod table_tests;
#[cfg(test)]
mod tests;

pub use crate::state::machine::{
    AnchorHint, Effect, Effects, FrameContext, MAX_EFFECTS, Session, SessionEnv, SessionEvent,
    SessionState, step,
};
pub use crate::state::paging::{
    DEFAULT_PAGE_SIZE, MAX_PAGES, MAX_PAGE_SIZE, MAX_REACHABLE_CANDIDATES, MIN_PAGE_SIZE, Paging,
};

/// The configuration values the session state machine acts on.
///
/// A view rather than the whole configuration: the engine owns the user's document
/// and this layer only needs the values that change what a session does. Grouping
/// them here means the machine never performs a configuration lookup while it is
/// deciding a transition, and a test can build one from a struct literal.
///
/// Every field mirrors a key of the design's configuration document; the defaults
/// are the values that document ships with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionConfig {
    /// `engine.max_raw_len`: the hard limit on the raw input, in bytes, `1..=64`.
    ///
    /// A limit the user lowers never truncates an input that is already longer: it
    /// applies to the next character pushed, so a reload cannot disturb the text in
    /// progress.
    pub max_raw_len: u8,
    /// `ui.max_per_row`: candidates per page, `3..=9`.
    pub max_per_row: u8,
    /// `ui.show_annotation`: whether a candidate's annotation is shown.
    pub show_annotation: bool,
    /// `ui.max_width_dp`: the widest the candidate window may be, in dp.
    pub max_width_dp: u16,
}

impl Default for SessionConfig {
    /// The configuration a user who has no document gets, which is also what
    /// `ime-config` writes into the sample document it hands them.
    fn default() -> Self {
        Self {
            max_raw_len: 64,
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        }
    }
}

impl SessionConfig {
    /// Builds the view from the four values the session reads.
    ///
    /// `max_raw_len` is clamped into the `1..=64` the schema allows, so a caller
    /// that hands over a value from somewhere else cannot leave the session without
    /// a length limit at all. `max_per_row` is clamped when the paging state adopts
    /// it, against the range the schema enforces for that key.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(max_raw_len: u8, max_per_row: u8, show_annotation: bool, max_width_dp: u16) -> Self {
        Self {
            max_raw_len: max_raw_len.clamp(1, 64),
            max_per_row,
            show_annotation,
            max_width_dp,
        }
    }
}
