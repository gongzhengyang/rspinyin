//! The claim arbitrator: whether the plugin may take a key from the application.
//!
//! # Responsibility
//!
//! The host asks one question about every key it delivers — *may I keep this one?* — and
//! meaning is not permission. A `Space` means [`KeyAction::CommitHighlighted`] and is the
//! plugin's while a composition is live; the same `Space` with nothing composing belongs to
//! the application, and keeping it would leave the user unable to type a space. A `5` means
//! [`KeyAction::SelectIndex`] and is the plugin's while the page on show offers a fifth
//! candidate; on a page offering four it names nothing and belongs to the application too.
//!
//! [`arbitrate`] answers that question, and it is the only place in this crate that answers
//! it. Two conditions have to hold together:
//!
//! 1. the routing table names the key, which [`claims_key`] reports; and
//! 2. a live session can act on the action the key was translated to, which
//!    [`executability`] reports.
//!
//! The second condition is the one the table cannot see. Leaving it out is the defect this
//! module exists to close: a plugin that keeps every routed key eats `Space`, the digits,
//! `Return`, `BackSpace`, `Escape`, `Tab` and the arrows in every application while nothing
//! is composing, and the user sees nothing happen.
//!
//! # Why the second condition is asked of the session
//!
//! The answer has to be the one a step would give, or a key would be kept and then do
//! nothing. Only the state machine knows it: whether a page holds the candidate a digit
//! names, whether the caret can move the way an arrow points, whether the input is already
//! as long as the configuration allows. [`executability`] therefore mirrors the guards of
//! the session's own transitions rather than re-deciding anything, and the test beside this
//! module walks every action through both and asserts that the two agree — a drift there is
//! the defect, so it is caught in the suite rather than in the user's typing.
//!
//! # The engine's own domain
//!
//! Three actions are the engine's rather than the session's: the Chinese / English,
//! full-width and punctuation switches change bits the host or the engine owns. They act
//! with no composition and nothing in the session to read, so [`is_mode_chord`] names them
//! and [`arbitrate`] keeps such a key whatever the session is doing — with nothing
//! composing, and while the host is finishing a commit or a cancellation — and hands it
//! back in temporary English, where the mode's whole meaning is that every key reaches the
//! application.
//!
//! # The one key that needs a composition behind it
//!
//! The syllable separator is the single action the session would act on and the plugin
//! still must not take. Its row is named in every context, because the routing table sees a
//! key and its modifiers and never the session; the apostrophe pins a syllable boundary
//! inside a composition, and the input alphabet accepts it, so a session with nothing
//! composing would take it as the first character of a new composition and open a candidate
//! window on a character the user typed for the application.
//! [`is_syllable_separator`] names the action and [`arbitrate`] hands it back outside a
//! composition. The rule sits here rather than in [`executability`] because that function
//! mirrors the step and the step really would compose — the two answers differ, and the
//! difference is the whole point.
//!
//! # What the arbitrator does not decide
//!
//! It answers about an *action*, so three things stay with the caller, all of them
//! properties of the key rather than of the action:
//!
//! * A modifier press. The held `Shift` is the host's own temporary switch: a plugin that
//!   kept the press would take the first half of every capital letter from the
//!   application. [`is_shift_press`](super::is_shift_press) is the predicate the caller
//!   asks first.
//! * A key release. Both edges of every key reach the host, and keeping one would eat the
//!   application's key-up; the one release the plugin watches is the edge that ends a held
//!   modifier, and [`ModifierHold`](super::modifier::ModifierHold) answers for it.
//! * A sequence. A stroke that opens one is the plugin's before any action exists, so the
//!   sequence machine decides first and [`arbitrate_sequence`] puts its answer into this
//!   module's vocabulary.
//!
//! # Boundary
//!
//! Plain Rust: no host object, no file, no clock and no global state, which is what makes
//! every row below a deterministic test. Nothing here mutates either — the two questions
//! that need to try something, whether the page would turn and whether the caret would
//! move, are asked of copies — so a caller may ask and then still step the session.

use ime_core::segment::MAX_RAW_LEN;
use ime_core::state::{Session, SessionConfig, SessionState};
use ime_types::{KeyAction, PageDir};

use super::context::Consumed;
use super::sequence::SequenceDecision;
use super::{claims_key, is_syllable_separator};

#[cfg(test)]
mod tests;

/// What a session would do with an action, without doing it.
///
/// Two answers and no third: the caller has to choose between keeping a key and handing it
/// back, and "the session would do something small" is not an answer the host can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executability {
    /// The step would act on this action: it produces an effect other than a diagnostic, or
    /// it changes a state the engine reads.
    Executable,
    /// The step would do nothing with it. The key must travel on to the application.
    Inert,
}

impl Executability {
    /// Whether the session would act on the action.
    ///
    /// # Returns
    ///
    /// `true` for [`Executability::Executable`] and `false` for [`Executability::Inert`].
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn is_executable(&self) -> bool {
        matches!(self, Self::Executable)
    }
}

/// Decides whether the plugin may take a key from the application.
///
/// # Arguments
///
/// * `action` — what the routing table made of the key, from
///   [`translate_key`](super::translate_key).
/// * `session` — the session of the input context the key arrived in, or `None` for a
///   context that has none.
/// * `cfg` — the configuration in force, which the input length limit is read from.
///
/// # Returns
///
/// [`Consumed::Consumed`] when both conditions hold — the caller calls `filterAndAccept`
/// and the application never sees the key — and [`Consumed::Ignored`] otherwise, which
/// includes the case the table names a key nothing can act on and the case of the syllable
/// separator outside a composition: that one the session would act on, and taking it is
/// still the wrong answer. See the module documentation.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never: every branch is a comparison, and the two probes take copies rather than
/// mutating. The guarantee matters because the caller is an FFI entry point, which must not
/// unwind into C++.
///
/// # Examples
///
/// ```
/// use ime_types::KeyAction;
/// use rspinyin::engine::Consumed;
/// use rspinyin::engine::arbiter::arbitrate;
/// use rspinyin::engine::RoutingConfig;
///
/// let cfg = RoutingConfig::default().session;
/// // A key the table does not name, and a routed key with no session behind it: both stay
/// // with the application.
/// assert_eq!(arbitrate(KeyAction::Ignore, None, &cfg), Consumed::Ignored);
/// assert_eq!(arbitrate(KeyAction::Escape, None, &cfg), Consumed::Ignored);
/// ```
pub fn arbitrate(action: KeyAction, session: Option<&Session>, cfg: &SessionConfig) -> Consumed {
    if !claims_key(action) {
        // The table does not name the key, so there is nothing to arbitrate.
        return Consumed::Ignored;
    }
    let Some(session) = session else {
        // A key for an input context with no session: the action has nowhere to go.
        return Consumed::Ignored;
    };
    if session.temp_english {
        // Temporary English hands every key to the application, the `Return` and `Escape`
        // that leave the mode included: leaving the mode produces no effect, so keeping the
        // key would take a keystroke the user typed for the application.
        return Consumed::Ignored;
    }
    if is_syllable_separator(action) && session.state != SessionState::Composing {
        // The one action the session would act on and the plugin still must not take. The
        // apostrophe pins a syllable boundary inside a composition, and the input alphabet
        // accepts it, so an idle session would start a composition on it and open a
        // candidate window over a character the user typed for the application.
        // `executability` keeps mirroring the step -- the step really would compose -- which
        // is why the rule is stated here rather than there.
        return Consumed::Ignored;
    }
    if is_mode_chord(action) {
        // The engine's own bits, answered before the session is asked at all: they act with
        // nothing composing, and they still act while the host is finishing a commit or a
        // cancellation, because nothing about them is the session's.
        return Consumed::Consumed;
    }
    if executability(session, action, cfg).is_executable() {
        Consumed::Consumed
    } else {
        Consumed::Ignored
    }
}

/// Whether a session would act on this action, without performing it.
///
/// The session half of [`arbitrate`], and the mirror of the guards in the state machine's
/// transitions: a key may only be kept when the step it stands for changes something. The
/// exceptions the caller adds on top are the engine's own mode bits, temporary English and
/// the syllable separator outside a composition; see the module documentation.
///
/// This function answers what the *step* would do and nothing else, so it is the one the
/// mirror test compares against the step. A caller that wants to know whether a key may be
/// kept asks [`arbitrate`], which is this answer plus the exceptions.
///
/// # Arguments
///
/// * `session` — the session to ask. It is read only: the questions that need to try
///   something are asked of copies of the paging state and of the input buffer.
/// * `action` — the action a key was translated to.
/// * `cfg` — the configuration in force, for the input length limit a typed character is
///   measured against.
///
/// # Returns
///
/// [`Executability::Executable`] when a step with this action would change the session or
/// produce an effect other than a diagnostic, and [`Executability::Inert`] otherwise.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn executability(session: &Session, action: KeyAction, cfg: &SessionConfig) -> Executability {
    if !claims_key(action) {
        return Executability::Inert;
    }
    if session.temp_english {
        return Executability::Inert;
    }
    match session.state {
        SessionState::Idle => idle(action),
        SessionState::Composing => composing(session, action, cfg),
        // A session waiting for the host to finish a commit or a cancellation drops every
        // key that reaches it: there is nothing left to act on.
        SessionState::Cancelling | SessionState::Committing => Executability::Inert,
    }
}

/// Whether the engine owns this action rather than the session.
///
/// The three mode bits are Fcitx5's input-method state and the plugin's own output choices.
/// They are live whether or not anything is composing, which is why [`arbitrate`] answers
/// for them even with a session that has no state to change.
///
/// # Arguments
///
/// * `action` — the action a key was translated to.
///
/// # Returns
///
/// `true` for the language, full-width and punctuation switches, `false` for every other
/// action — the ones a session acts on included.
///
/// # Panics
///
/// Never.
pub fn is_mode_chord(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::ToggleLang | KeyAction::ToggleFullWidth | KeyAction::TogglePunct
    )
}

/// The answer for a stroke the sequence machine has already decided about.
///
/// A sequence is decided before any action exists: a stroke that opens one is the plugin's
/// from that moment even though nothing has been executed, and a stroke that leads nowhere
/// is the application's. This is the sequence machine's answer in the vocabulary of the
/// walk, so that one value travels from the stroke to the host's `filterAndAccept`.
///
/// # Arguments
///
/// * `decision` — what [`KeySequence::offer`](super::sequence::KeySequence::offer) decided
///   about one stroke.
///
/// # Returns
///
/// [`Consumed::ChainPending`] for a stroke that opened or continued a sequence — the key is
/// ours, nothing has happened yet — [`Consumed::Consumed`] for one that completed or
/// cancelled a sequence, and [`Consumed::Ignored`] for a stroke that leads nowhere, the
/// second stroke of an abandoned sequence included: that one travels on to the context tree
/// and then to the application.
///
/// # Panics
///
/// Never.
pub fn arbitrate_sequence<T>(decision: &SequenceDecision<T>) -> Consumed {
    match decision {
        SequenceDecision::Opened => Consumed::ChainPending,
        SequenceDecision::Completed { .. } | SequenceDecision::Cancelled => Consumed::Consumed,
        SequenceDecision::Pass | SequenceDecision::Abandoned { .. } => Consumed::Ignored,
    }
}

/// Whether an idle session would act on this action.
///
/// With nothing composing, a session acts on exactly two things: a typed character, which
/// starts a composition, and the chord that turns temporary English on. Every other routed
/// key — `Space`, the digits, `Return`, `BackSpace`, `Escape`, `Tab`, the arrows, the page
/// keys and the page jumps — has nothing to act on, and keeping it is the swallowed-key
/// defect.
fn idle(action: KeyAction) -> Executability {
    match action {
        KeyAction::InputChar(ch) if is_input_char(ch) => Executability::Executable,
        // A character outside the input alphabet would be reported and dropped.
        KeyAction::InputChar(_) => Executability::Inert,
        KeyAction::EnterTempEnglish => Executability::Executable,
        KeyAction::Backspace
        | KeyAction::CommitHighlighted
        | KeyAction::CommitRaw
        | KeyAction::SelectIndex(_)
        | KeyAction::PageNext
        | KeyAction::PagePrev
        | KeyAction::PageFirst
        | KeyAction::PageLast
        | KeyAction::MoveHighlight(_)
        | KeyAction::MoveCaret(_)
        | KeyAction::ToggleLang
        | KeyAction::ToggleFullWidth
        | KeyAction::TogglePunct
        | KeyAction::ToggleScript
        | KeyAction::ForgetHighlighted
        | KeyAction::PinHighlighted
        | KeyAction::AddPhrase
        | KeyAction::Escape
        | KeyAction::Ignore => Executability::Inert,
    }
}

/// Whether a composing session would act on this action.
fn composing(session: &Session, action: KeyAction, cfg: &SessionConfig) -> Executability {
    match action {
        KeyAction::InputChar(ch) => fits(session, ch, cfg),
        // Either the buffer loses a unit or the composition ends: both act.
        KeyAction::Backspace => Executability::Executable,
        KeyAction::CommitHighlighted => has_highlight(session),
        KeyAction::CommitRaw => non_empty_input(session),
        KeyAction::SelectIndex(digit) => selectable(session, digit),
        KeyAction::PageNext => flip(session, PageDir::Next),
        KeyAction::PagePrev => flip(session, PageDir::Prev),
        KeyAction::PageFirst => flip_edge(session, false),
        KeyAction::PageLast => flip_edge(session, true),
        KeyAction::MoveHighlight(delta) => move_highlight(session, delta),
        KeyAction::MoveCaret(delta) => move_caret(session, delta),
        // The window is repainted with the status strip the engine wrote for the new bits.
        KeyAction::ToggleLang | KeyAction::ToggleFullWidth | KeyAction::TogglePunct => {
            Executability::Executable
        }
        // A script switch keeps the composition and re-sends the frame.
        KeyAction::ToggleScript => Executability::Executable,
        // The mode turns on and the composition is taken back.
        KeyAction::EnterTempEnglish => Executability::Executable,
        // The composition is cancelled: the preedit is cleared and the window hidden.
        KeyAction::Escape => Executability::Executable,
        KeyAction::ForgetHighlighted => has_highlight(session),
        KeyAction::AddPhrase => add_phrase(session),
        // The pin set does not exist yet: the step reports `dict/unsupported` and changes
        // nothing, so the key is not the plugin's to take.
        KeyAction::PinHighlighted => Executability::Inert,
        KeyAction::Ignore => Executability::Inert,
    }
}

/// Whether one typed character fits in the input the session is holding.
///
/// The two limits the buffer is measured against: the configured length, and the hard cap
/// `ime-core` enforces on the raw input. A character that fits neither is reported and
/// dropped, so the key has to reach the application.
fn fits(session: &Session, ch: char, cfg: &SessionConfig) -> Executability {
    if !is_input_char(ch) {
        return Executability::Inert;
    }
    let limit = usize::from(cfg.max_raw_len.max(1)).min(MAX_RAW_LEN);
    let len = session.buf.raw().len().saturating_add(ch.len_utf8());
    if len > limit {
        return Executability::Inert;
    }
    Executability::Executable
}

/// Whether the highlight is on a candidate.
fn has_highlight(session: &Session) -> Executability {
    if session.highlighted_candidate().is_some() {
        return Executability::Executable;
    }
    Executability::Inert
}

/// Whether the input holds anything to commit as it was typed.
fn non_empty_input(session: &Session) -> Executability {
    if session.buf.raw().is_empty() {
        return Executability::Inert;
    }
    Executability::Executable
}

/// Whether the digit names a candidate on the page on show.
fn selectable(session: &Session, digit: u8) -> Executability {
    if session.paging.digit_target(digit, total(session)).is_some() {
        return Executability::Executable;
    }
    Executability::Inert
}

/// Whether saving the highlighted candidate as a phrase would record anything.
fn add_phrase(session: &Session) -> Executability {
    if session.buf.raw().is_empty() || session.highlighted_candidate().is_none() {
        return Executability::Inert;
    }
    Executability::Executable
}

/// Whether the page would turn.
///
/// Asked of a copy of the paging state, which is a plain value: the probe costs nothing and
/// the session is left exactly as it was.
fn flip(session: &Session, dir: PageDir) -> Executability {
    let mut probe = session.paging;
    if probe.flip(dir, total(session)) {
        return Executability::Executable;
    }
    Executability::Inert
}

/// Whether the jump to the first or last page would move.
///
/// Asked of a copy of the paging state, like [`flip`]: a jump that lands re-sends the
/// frame, and one that finds the grid already at that end is a key the plugin would
/// swallow for nothing.
fn flip_edge(session: &Session, to_last: bool) -> Executability {
    let mut probe = session.paging;
    let moved = if to_last {
        probe.flip_to_last(total(session))
    } else {
        probe.flip_to_first(total(session))
    };
    if moved {
        return Executability::Executable;
    }
    Executability::Inert
}

/// Whether the highlight would move.
fn move_highlight(session: &Session, delta: i8) -> Executability {
    let mut probe = session.paging;
    if probe.move_highlight(delta, total(session)) {
        return Executability::Executable;
    }
    Executability::Inert
}

/// Whether the caret would move.
///
/// Asked of a copy of the input buffer rather than re-derived: a move steps along the
/// syllable grid the last segmentation reported, that rule lives in `ime-core`, and a
/// second copy of it here would drift. The copy costs one bounded allocation — the input is
/// capped at [`MAX_RAW_LEN`] bytes — which is cheaper than a key that is kept and then does
/// nothing.
fn move_caret(session: &Session, delta: i8) -> Executability {
    let mut probe = session.buf.clone();
    if probe.move_caret(delta) {
        return Executability::Executable;
    }
    Executability::Inert
}

/// How many candidates the session is holding.
fn total(session: &Session) -> u16 {
    u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX)
}

/// Whether the input alphabet accepts `ch`.
///
/// The alphabet is the buffer's — ASCII letters and the apostrophe that pins a syllable
/// boundary. The rule is stated once in `ime-core` and read from here, so a change there
/// has to be made in both places; the mirror test walks characters the alphabet accepts and
/// ones it rejects, which is where the two are compared.
fn is_input_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '\''
}
