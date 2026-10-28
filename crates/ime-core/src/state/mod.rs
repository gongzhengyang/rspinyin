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
//! - `scheme` owns the double-pinyin layout a composition's keystrokes follow, the
//!   rewrite that turns them into the spelling the segmentation layer reads, and the
//!   syllable grid that rewrite reports.
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
//!
//! The scheme keys are the one exception to "a reload changes what the session
//! does": they are adopted while the session is idle and frozen once a composition is
//! live, because re-reading the input the user has already typed in another layout
//! would move the candidate list under the caret. The new layout answers the next
//! composition instead.

mod assoc;
mod assoc_view;
mod boundaries;
pub mod effects;
pub mod frame;
mod lifecycle;
pub mod machine;
pub mod outcome;
pub mod paging;
mod scheme;
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
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MAX_PAGES, MAX_REACHABLE_CANDIDATES, MIN_PAGE_SIZE, Paging,
};

use ime_types::SchemeId;

use crate::viterbi::context::ContextBand;

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
    /// `scheme.scheme`: which double-pinyin layout the keystrokes follow.
    ///
    /// [`SchemeId::FULL`] is full pinyin, where the input is decoded exactly as
    /// typed. The layout is adopted when a composition starts and is not re-read
    /// while one is live, so a reload changes the reading from the next composition
    /// on rather than under the caret of the one in progress (0.4 rule 10).
    pub scheme: SchemeId,
    /// `scheme.keep_full_pinyin`: whether a keystroke the layout cannot read is read
    /// as full pinyin.
    ///
    /// Without it, a user who switched to a double-pinyin layout and then typed a
    /// full-pinyin syllable gets nothing at all, which reads as "the input method
    /// broke" rather than as "you are in the wrong mode".
    pub keep_full_pinyin: bool,
    /// The fuzzy-equivalence classes the `[fuzzy]` section selects, as the decode
    /// flag mask the projection computed: the master [`ime_types::DecodeFlags::FUZZY`]
    /// bit exactly when at least one class bit is set.
    ///
    /// The mask is precomputed by the projection rather than assembled here because
    /// the master bit and the class bits must agree, and a projection that runs once
    /// per configuration load is where that agreement is kept. Empty -- the default --
    /// decodes exactly as a build before the section existed.
    pub fuzzy: ime_types::DecodeFlags,
    /// `engine.abbrev`: whether an initial-letter abbreviation is expanded, so that
    /// `nh` reaches a word spelled `ni'hao`.
    ///
    /// Off by default: an abbreviation is ambiguous by nature, and a user who never
    /// asked for one must not have it answered ahead of the full spelling.
    pub abbrev: bool,
    /// `engine.correction`: whether a one-step typing slip is corrected for an input
    /// no exact spelling covers, so that `zhegn` reaches a word spelled `zheng`.
    ///
    /// Off by default, for the same reason the abbreviation switch is: a corrected
    /// reading is what the user meant only by accident, and the penalty that keeps
    /// it below everything the input spells is the engine's business, not the
    /// configuration's.
    pub correction: bool,
    /// `engine.mixed_english`: whether an input no pinyin reading cuts is offered
    /// the English lexicon's entry for it, so that `github` reaches the word
    /// spelled `GitHub`.
    ///
    /// Off by default: a user who never asked for English candidates must not have
    /// their compositions answered from a table they did not know was there. With
    /// the switch off the decode never queries the English lexicon, whatever it
    /// holds.
    pub english: bool,
    /// `engine.on_im_switch`: what a live composition does when the input method
    /// is switched away from or reset.
    pub on_im_switch: ImSwitchPolicy,
    /// `data.context_tie_band_q16`: the positive context re-rank band, projected
    /// into its engine-side Q16.16 value.
    pub context_band: ContextBand,
    /// `[assoc]`: the next-word association projection. Disabled by default, so a
    /// commit followed by association candidates is strictly opt-in.
    pub assoc: AssocSessionConfig,
}

/// What a live composition does when the input method is switched away from.
///
/// The configuration's own spelling of the choice lives in `ime-config`; this is
/// the engine's half, projected once per configuration load like the scheme and
/// the fuzzy mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImSwitchPolicy {
    /// The shipped behaviour: the composing text is taken back and nothing
    /// reaches the application.
    #[default]
    Clear,
    /// The raw input is committed first, so a user who switches away mid-word
    /// keeps the letters they typed.
    CommitRaw,
}

/// The association values a session acts on.
///
/// `ime-core` cannot name `ime-config`'s section type, so the engine projects the
/// three keys into this internal view at load time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssocSessionConfig {
    /// Whether a successful commit may open an association view.
    pub enabled: bool,
    /// The greatest number of association candidates one view may hold, `1..=9`.
    pub max_candidates: u8,
    /// How many recent commits the session history may retain, `0..=64`.
    pub history_depth: u8,
}

impl Default for AssocSessionConfig {
    /// The shipped projection: association is off and the memory is empty.
    fn default() -> Self {
        Self {
            enabled: false,
            max_candidates: 5,
            history_depth: 8,
        }
    }
}

impl From<crate::assoc::AssocLimits> for AssocSessionConfig {
    /// Adopts already-bounded engine limits without re-deciding their policy.
    fn from(limits: crate::assoc::AssocLimits) -> Self {
        Self {
            enabled: limits.enabled,
            max_candidates: limits.max_candidates,
            history_depth: limits.history_depth,
        }
    }
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
            scheme: SchemeId::FULL,
            keep_full_pinyin: true,
            fuzzy: ime_types::DecodeFlags::empty(),
            abbrev: false,
            correction: false,
            english: false,
            on_im_switch: ImSwitchPolicy::Clear,
            context_band: ContextBand::default(),
            assoc: AssocSessionConfig::default(),
        }
    }
}

impl SessionConfig {
    /// Builds the view from the four values that are not a scheme key.
    ///
    /// `max_raw_len` is clamped into the `1..=64` the schema allows, so a caller
    /// that hands over a value from somewhere else cannot leave the session without
    /// a length limit at all. `max_per_row` is clamped when the paging state adopts
    /// it, against the range the schema enforces for that key.
    ///
    /// The two scheme keys take their defaults -- full pinyin, and the mixed-input
    /// reading on -- so a host that has a layout sets [`SessionConfig::scheme`] and
    /// [`SessionConfig::keep_full_pinyin`] on the value this returns.
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
            ..Self::default()
        }
    }

    /// The first candidate-materialization quota the engine uses.
    ///
    /// The session projects the engine's fixed ladder rather than turning it into a
    /// configuration key: paging depth is a performance boundary, not a surface a user
    /// document should be able to tune silently past the window's own limits.
    pub const INITIAL_CANDIDATE_QUOTA: u16 = 24;

    /// The doubled quota used when paging asks beyond the first materialization.
    pub const EXPANDED_CANDIDATE_QUOTA: u16 = Self::INITIAL_CANDIDATE_QUOTA * 2;

    /// The hard cap of the candidate-materialization ladder.
    pub const SUPPLY_CANDIDATE_CAP: u16 = crate::viterbi::decoder::SUPPLY_CANDIDATE_CAP;

    /// Returns the next candidate-materialization quota for a paging request.
    ///
    /// This is the session-facing read-only projection of the engine ladder. The values
    /// are identical to [`crate::viterbi::decoder::candidate_supply_quota`]; the second
    /// name keeps session code from constructing decoder configuration details directly.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn candidate_supply_quota(materialized: u16) -> u16 {
        crate::viterbi::decoder::candidate_supply_quota(materialized)
    }

    /// Reports whether paging past the materialized prefix may expand the decode.
    ///
    /// The cap reports `false`, so a caller cannot keep growing a list after the final
    /// ladder rung. The caller supplies the requested count (one past the highest global
    /// index it needs), and the engine re-decodes from scratch on expansion; therefore a
    /// wider list contains the narrower one as a duplicate-free prefix.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn has_more_candidate_supply(materialized: u16, requested: u16) -> bool {
        crate::viterbi::decoder::has_more_candidate_supply(materialized, requested)
    }
}

#[cfg(test)]
mod supply_tests {
    use super::*;

    #[test]
    fn test_candidate_supply_ladder_has_the_three_documented_boundaries() {
        assert_eq!(SessionConfig::INITIAL_CANDIDATE_QUOTA, 24);
        assert_eq!(SessionConfig::EXPANDED_CANDIDATE_QUOTA, 48);
        assert_eq!(SessionConfig::SUPPLY_CANDIDATE_CAP, 64);
        assert_eq!(SessionConfig::candidate_supply_quota(0), 24);
        assert_eq!(SessionConfig::candidate_supply_quota(23), 24);
        assert_eq!(SessionConfig::candidate_supply_quota(24), 48);
        assert_eq!(SessionConfig::candidate_supply_quota(47), 48);
        assert_eq!(SessionConfig::candidate_supply_quota(48), 64);
        assert_eq!(SessionConfig::candidate_supply_quota(63), 64);
        assert_eq!(SessionConfig::candidate_supply_quota(64), 64);
        assert_eq!(SessionConfig::candidate_supply_quota(u16::MAX), 64);
    }

    #[test]
    fn test_candidate_supply_has_more_stops_at_the_cap() {
        assert!(SessionConfig::has_more_candidate_supply(24, 25));
        assert!(SessionConfig::has_more_candidate_supply(48, 49));
        assert!(!SessionConfig::has_more_candidate_supply(24, 24));
        assert!(!SessionConfig::has_more_candidate_supply(64, 65));
        assert!(!SessionConfig::has_more_candidate_supply(
            u16::MAX,
            u16::MAX
        ));
    }
}
