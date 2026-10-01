//! Key-event routing: the host's key events to the actions a session consumes.
//!
//! # Boundary
//!
//! This module is plain Rust. It reads a [`FcitxKeyEvent`] — the frozen `#[repr(C)]` pair
//! of keysym and modifier mask that `ffi::abi` describes — plus the `[keys]` section of
//! the configuration, and answers with the [`KeyAction`] the session state machine
//! consumes. It touches no host object, no file, no clock and no global state, which is
//! what makes every row of the routing table testable without Fcitx5 present.
//!
//! # The rule this module exists to keep
//!
//! A key is only taken away from the application when something can act on it, and that
//! takes two conditions: the routing table has to name the key, and a session has to be
//! able to execute the resulting action. [`translate_key`] answers the first condition
//! and [`claims_key`] makes it explicit; [`router::KeyRouter`] adds the second by
//! stepping a session with the action and reporting whether anything came of it. The
//! host-side effect of that pair is `filterAndAccept` in the engine's `keyEvent`, and a
//! key that fails either condition must reach neither.
//!
//! # Layout
//!
//! * The routing table itself — [`translate_key`], [`claims_key`] and the keysym and
//!   modifier constants they read — is this file, and its rows are the `rows` submodule.
//! * [`context`] is the layered bus the table is read through: which layer of the plugin
//!   owns a key, and whether the plugin may keep it.
//! * [`modifier`] is the check that the host's modifier bits are the ones this build was
//!   compiled against.
//! * [`router`] holds one session per input context and executes the effects a step
//!   returns.
//! * [`host`] is the boundary those effects are executed against: the trait the engine
//!   calls to commit text, to fill the application's preedit area, to post a command to
//!   the candidate window and to report a diagnostic.
//!
//! # Configuration
//!
//! [`KeyBindings`] is the `[keys]` section as the routing table reads it: the settings
//! the table branches on, projected from the document by `ime_config::keymap` and adopted
//! whole when the configuration is reloaded. The type is the configuration layer's rather
//! than one of this crate's, so the file format, the key-name whitelist and the validation
//! stay in one place and this table follows them; [`router::RoutingConfig`] is the rest of
//! what the routing layer acts on, projected from the document by
//! [`router::RoutingConfig::from_config`].
//!
//! # The shape of the table
//!
//! Two tables, both data rather than control flow, so that one more key is a row and not a
//! new branch:
//!
//! * `CHORDS` holds the keys whose modifiers are part of the key — `Ctrl+Space` and its
//!   neighbours — and a chord matches only the exact modifier set it declares.
//! * `rows` holds every other key, in evaluation order: the rows `keys.highlight_keys`
//!   binds, then the rows `keys.flip_keys` binds, then the rows the configuration cannot
//!   unbind.
//!
//! [`translate_key`] reads them in that order, and the predicates below it —
//! [`is_shift_press`], [`is_syllable_separator`] and [`leaves_temp_english`] — are the
//! questions about a key the table deliberately leaves open, because the answer is a fact
//! about the session or about the host rather than about the key: a modifier's own press is
//! the host's, an apostrophe pins a syllable boundary only inside a composition, and two of
//! the keys of temporary English end a mode the table cannot see.

use ime_types::KeyAction;

use crate::ffi::FcitxKeyEvent;

pub mod arbiter;
pub mod context;
pub mod host;
pub mod modifier;
pub mod router;
pub mod sequence;

mod rows;

mod badge;

pub use arbiter::{Executability, arbitrate, arbitrate_sequence, executability, is_mode_chord};
pub use context::{Consumed, Dispatcher, KeyContext, KeyEvent, Overlay, SessionView};
pub use ime_config::DigitZero;
pub use ime_config::keymap::{FlipSet, HighlightSet, KeyBindings};
pub use modifier::{
    HOLD_THRESHOLD_MS, HoldOutcome, MODIFIER_MASK_MISMATCH_CODE, ModifierHold, ModifierKey,
    check_modifier_mask,
};
pub use router::{KeyRouter, RoutingConfig};
pub use sequence::{
    KeySequence, MAX_SEQUENCE_STROKES, SEQUENCE_CONFLICT_CODE, SEQUENCE_TIMEOUT_MS,
    SEQUENCE_TOO_LONG_CODE, SequenceDecision, SequencePrefix, SequenceState, SequenceTable,
};

#[cfg(test)]
mod binding_audit;

#[cfg(test)]
mod tests;

// ── Modifier bits ────────────────────────────────────────────────────────────────
//
// The values are `fcitx::KeyState`'s, taken from the installed Fcitx5 5.1.7 header
// `fcitx-utils/keysym.h`, which defines the enum as a `uint32_t`; `FcitxKeyEvent::state`
// carries `KeyStates::toInteger()` unchanged. The test module pins the mask against the
// header's own `SimpleMask`, so a host that renumbers these bits fails a test rather than
// silently misreading every chord.

/// `fcitx::KeyState::Shift`; crate-visible so the cheat sheet spells chords from the
/// same bits this table matches on, and the two cannot drift.
pub(crate) const SHIFT: u32 = 1 << 0;
/// `fcitx::KeyState::Ctrl`; crate-visible for the cheat sheet, like [`SHIFT`].
pub(crate) const CTRL: u32 = 1 << 2;
/// `fcitx::KeyState::Alt`.
const ALT: u32 = 1 << 3;
/// `fcitx::KeyState::Hyper`, the `Mod3` alias.
const HYPER: u32 = 1 << 5;
/// `fcitx::KeyState::Super`, the `Mod4` alias.
const SUPER: u32 = 1 << 6;
/// `fcitx::KeyState::Super2`, GTK's virtual Super.
const SUPER2: u32 = 1 << 26;
/// `fcitx::KeyState::Meta`.
const META: u32 = 1 << 28;

/// Every modifier a user holds deliberately, spelled out from the header's `SimpleMask`
/// (`Ctrl_Alt_Shift | Super | Super2 | Hyper | Meta`).
///
/// The bits outside it are not modifiers in that sense: `CapsLock` and `NumLock` are
/// latched states, `MousePressed` is not a key, and `Repeat` marks a press the frontend
/// repeated — a repeated press is still a press.
const MODIFIER_MASK: u32 = CTRL | ALT | SHIFT | SUPER | SUPER2 | HYPER | META;

/// [`MODIFIER_MASK`] without `Shift`, for the rows that tolerate a held Shift.
const NON_SHIFT_MODIFIERS: u32 = MODIFIER_MASK & !SHIFT;

// ── Key symbols ──────────────────────────────────────────────────────────────────
//
// XKB keysyms as the installed `fcitx-utils/keysymgen.h` defines them; `FcitxKeyEvent::sym`
// carries `fcitx::Key::sym()` unchanged.

/// `FcitxKey_space`; crate-visible so the cheat sheet names chords from the table's keysyms.
pub(crate) const KEY_SPACE: u32 = 0x0020;
/// `FcitxKey_apostrophe`, the syllable separator the input buffer accepts.
const KEY_APOSTROPHE: u32 = 0x0027;
/// The character the [`KEY_APOSTROPHE`] row produces.
///
/// Named rather than spelled out at each use so that the row, the guard that keeps the
/// separator inside a composition and the tests all name the same character.
const SYLLABLE_SEPARATOR: char = '\'';
/// `FcitxKey_minus`.
const KEY_MINUS: u32 = 0x002d;
/// `FcitxKey_period`; crate-visible like [`KEY_SPACE`].
pub(crate) const KEY_PERIOD: u32 = 0x002e;
/// `FcitxKey_0`, the low end of the digit row.
const KEY_0: u32 = 0x0030;
/// `FcitxKey_1`, the low end of the selectable digits: `0` is routed by configuration
/// rather than being a candidate index, so the selection range starts here.
const KEY_1: u32 = 0x0031;
/// `FcitxKey_9`, the high end of the digit row.
const KEY_9: u32 = 0x0039;
/// `FcitxKey_equal`.
const KEY_EQUAL: u32 = 0x003d;
/// `FcitxKey_A`, the low end of the uppercase shape a host may deliver for `Shift+a`.
const KEY_A_UPPER: u32 = 0x0041;
/// `FcitxKey_Z`, the high end of that uppercase shape.
const KEY_Z_UPPER: u32 = 0x005a;
/// `FcitxKey_a`, the low end of the letter row.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_e`, the letter of the temporary-English chord; crate-visible like
/// [`KEY_SPACE`].
pub(crate) const KEY_E: u32 = 0x0065;
/// `FcitxKey_z`, the high end of the letter row.
const KEY_Z: u32 = 0x007a;
/// `FcitxKey_BackSpace`.
const KEY_BACKSPACE: u32 = 0xff08;
/// `FcitxKey_Tab`.
const KEY_TAB: u32 = 0xff09;
/// `FcitxKey_Return`.
const KEY_RETURN: u32 = 0xff0d;
/// `FcitxKey_Escape`.
const KEY_ESCAPE: u32 = 0xff1b;
/// `FcitxKey_Left`.
const KEY_LEFT: u32 = 0xff51;
/// `FcitxKey_Up`.
const KEY_UP: u32 = 0xff52;
/// `FcitxKey_Right`.
const KEY_RIGHT: u32 = 0xff53;
/// `FcitxKey_Down`.
const KEY_DOWN: u32 = 0xff54;
/// `FcitxKey_Page_Up`, which pages backwards when `[keys] flip_keys` names it.
const KEY_PAGE_UP: u32 = 0xff55;
/// `FcitxKey_Page_Down`, which pages forwards when `[keys] flip_keys` names it.
const KEY_PAGE_DOWN: u32 = 0xff56;
/// `FcitxKey_Shift_L`.
const KEY_SHIFT_L: u32 = 0xffe1;
/// `FcitxKey_Shift_R`.
const KEY_SHIFT_R: u32 = 0xffe2;

// The binding table and the two flag sets it is built from — [`KeyBindings`], [`FlipSet`]
// and [`HighlightSet`] — are re-exported above rather than declared here: they are the
// `[keys]` section as `ime-config` projects it, and a second declaration of the same shape
// would be a table the configuration could not reach. [`DigitZero`] travels with them for
// the same reason: it is one of the settings the table reads, so it belongs to the layer
// that parses the spelling the document uses.

/// Translates one host key event into the action the session should take.
///
/// # Arguments
///
/// * `event` — the key as the host delivered it: an XKB keysym, the modifier mask, the
///   press/release edge and the frontend's timestamp. The timestamp is not read here; it
///   belongs to the latency budget, not to the table.
/// * `keys` — the `[keys]` settings the table branches on.
///
/// # Returns
///
/// The action, or [`KeyAction::Ignore`] when the key belongs to the host: an unrecognised
/// key, a key release, or a key whose modifiers make it somebody else's chord. `Ignore`
/// means "the host keeps the key" — it is never a "do nothing".
///
/// # Panics
///
/// Never. The tables and the lookups they delegate to read only the two integers in
/// `event` and the four settings of `keys`: no indexing, no arithmetic that can overflow,
/// no allocation. The guarantee matters because the caller is an FFI entry point, which
/// must not unwind into C++.
///
/// # Examples
///
/// ```
/// use ime_types::KeyAction;
/// use rspinyin::engine::{KeyBindings, translate_key};
/// use rspinyin::ffi::FcitxKeyEvent;
///
/// let key = FcitxKeyEvent {
///     sym: 0x61,
///     state: 0,
///     is_release: false,
///     time_ms: 0,
/// };
/// assert_eq!(translate_key(&key, &KeyBindings::default()), KeyAction::InputChar('a'));
/// ```
pub fn translate_key(event: &FcitxKeyEvent, keys: &KeyBindings) -> KeyAction {
    // A release is never ours: the host delivers both edges of every key, and consuming
    // one would eat the application's key-up. Both tables are tables of presses.
    if event.is_release {
        return KeyAction::Ignore;
    }
    // A frontend that folds the case into the symbol delivers `Shift+a` as `A`, and one
    // that does not delivers it as `a`. The tables must not see the difference: a key that
    // means "type the letter a" is the same key whichever shape it arrives in, and a table
    // that answered differently would make the plugin behave differently per frontend. The
    // fold therefore runs before either table is read.
    let sym = fold_shifted_letter(event.sym, event.state);
    let state = event.state;

    if let Some(action) = chord_action(sym, state) {
        return action;
    }
    rows::action(sym, state, keys).unwrap_or(KeyAction::Ignore)
}

// ── The chords ───────────────────────────────────────────────────────────────────

/// One global mode chord: a key plus the exact modifier set it requires.
///
/// Crate-visible because the cheat sheet is generated from the chord table rather
/// than from a second spelling of it: a chord added here reaches the panel as soon
/// as a name and a label are decided for it, and one removed here cannot leave a row
/// the keyboard no longer answers.
pub(crate) struct Chord {
    /// XKB keysym of the chord's key.
    pub(crate) sym: u32,
    /// The modifier set, compared for equality against `state & MODIFIER_MASK`.
    ///
    /// Equality rather than a subset test: `Ctrl+Space` and `Ctrl+Shift+Space` are
    /// different keys, and a chord that accepted a superset would take keys the desktop
    /// environment owns.
    pub(crate) mask: u32,
    /// What the chord means.
    pub(crate) action: KeyAction,
}

/// The global mode chords, in match order.
///
/// Every entry carries a modifier: a bare key is a row of the table rather than a chord,
/// which is what lets the space bar be the commit key with nothing held and the language
/// switch with `Ctrl` held.
///
/// `Shift_L` and `Shift_R` are deliberately absent. A modifier's own press is not a chord,
/// and treating it as one made every capital letter toggle the input mode; the held key is
/// the host's own temporary switch, and [`is_shift_press`] is what keeps the routing layer
/// from taking it. The hold semantics that replaced the chord live in [`modifier`].
///
/// Crate-visible for the reason [`Chord`] states.
pub(crate) const CHORDS: &[Chord] = &[
    Chord {
        sym: KEY_SPACE,
        mask: CTRL,
        action: KeyAction::ToggleLang,
    },
    Chord {
        sym: KEY_SPACE,
        mask: SHIFT,
        action: KeyAction::ToggleFullWidth,
    },
    Chord {
        sym: KEY_PERIOD,
        mask: CTRL,
        action: KeyAction::TogglePunct,
    },
    Chord {
        sym: KEY_E,
        mask: CTRL | SHIFT,
        action: KeyAction::EnterTempEnglish,
    },
];

/// The chord this key and these modifiers make, if any.
///
/// Returns `None` when no chord names the key with exactly these modifiers, so the caller
/// goes on with the rows. A chord that does not match is not a claim on the key: the
/// space bar with `Alt` held is nobody's chord and belongs to the host, which the rows
/// answer because none of them accepts a modifier.
///
/// # Panics
///
/// Never.
fn chord_action(sym: u32, state: u32) -> Option<KeyAction> {
    let held = state & MODIFIER_MASK;
    if held == 0 {
        // Every chord carries a modifier, so a bare press cannot be one. Answering here
        // also keeps the scan off the path almost every key takes.
        return None;
    }
    CHORDS
        .iter()
        .find(|chord| chord.sym == sym && chord.mask == held)
        .map(|chord| chord.action)
}

// ── The two keys the table cannot answer for on its own ──────────────────────────

/// Folds a keysym a host may have case-folded back to its lowercase form.
///
/// Fcitx5 folds the case into the symbol when Shift is held, so `Shift+a` can arrive as
/// either `0x61` or `0x41` depending on the frontend. The routing table must not see the
/// difference: a key that means "type the letter a" is the same key whichever shape it
/// arrives in. The fold is applied only when Shift is actually held, so a bare uppercase
/// symbol — which no keyboard produces — still falls through to the host.
///
/// # Arguments
///
/// * `sym` — the keysym as the host delivered it.
/// * `state` — the modifier mask that arrived with it.
///
/// # Returns
///
/// The lowercase keysym when `sym` is an uppercase ASCII letter and Shift is held, and
/// `sym` unchanged otherwise.
///
/// # Panics
///
/// Never: the range check is what keeps the subtraction inside `0x61..=0x7a`.
fn fold_shifted_letter(sym: u32, state: u32) -> u32 {
    if (state & SHIFT) != 0 && (KEY_A_UPPER..=KEY_Z_UPPER).contains(&sym) {
        sym + (KEY_A - KEY_A_UPPER)
    } else {
        sym
    }
}

/// Whether an action is the syllable separator the composing input accepts.
///
/// The apostrophe is the one key of the composing keymap that means nothing outside a
/// composition. Inside one it pins a syllable boundary, and `ime-core`'s input alphabet
/// accepts it, so a session with nothing composing would take it as the first character of
/// a new composition and open a candidate window on a key the user typed for the
/// application. The routing table cannot tell the two apart — it is a function of the key
/// and the modifiers, never of the session — so it names the action here and the two layers
/// that hold a session ask this before they keep the key.
///
/// # Arguments
///
/// * `action` — what [`translate_key`] made of the key.
///
/// # Returns
///
/// `true` for [`KeyAction::InputChar`] of the apostrophe and `false` for every other
/// action, the letters included: a letter does start a composition with nothing composing,
/// and that is the whole of how typing begins.
///
/// # Panics
///
/// Never.
pub fn is_syllable_separator(action: KeyAction) -> bool {
    matches!(action, KeyAction::InputChar(SYLLABLE_SEPARATOR))
}

/// Whether the routing table claims `action`, i.e. whether the key it came from must be
/// kept from the application.
///
/// # Arguments
///
/// * `action` — the action [`translate_key`] produced for a key press.
///
/// # Returns
///
/// `false` for [`KeyAction::Ignore`], which is the answer that hands the key back, and
/// `true` for every action that means something.
///
/// # Why this is not the whole answer
///
/// The table answers what a key *means*; it cannot answer whether anything can act on that
/// meaning. A key may only be taken from the application when both hold, so the caller
/// asks this function **and** requires a session that can execute the action: with no
/// session the action has nowhere to go and the key must travel on. The engine's
/// `keyEvent` turns a claimed, executable action into `filterAndAccept` and does nothing
/// else, which is what keeps a key out of the "answered handled but did nothing" class the
/// shortcut table forbids. [`is_syllable_separator`] is the one action whose second
/// condition is not the session's state alone.
///
/// # Panics
///
/// Never.
pub fn claims_key(action: KeyAction) -> bool {
    !matches!(action, KeyAction::Ignore)
}

/// Whether the event is a press of a Shift key itself.
///
/// A modifier is never a key of the plugin's. Fcitx5 delivers it to the application and
/// implements the held-`Shift` Chinese / English switch itself, and an input method that
/// kept the press would take the first half of every capital letter away from the
/// application. The routing table therefore has no row for `Shift_L` or `Shift_R` at all,
/// and this predicate is how the layers that must still recognise the key — the hold
/// machine that watches its release, and the bus that answers ahead of the walk — name it.
///
/// The answer is about the key rather than about the edge: a release answers `true` as
/// well, even though [`translate_key`] claims no release at all.
///
/// # Arguments
///
/// * `event` — the key as the host delivered it.
///
/// # Returns
///
/// `true` for `Shift_L` or `Shift_R`, on either edge.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_types::KeyAction;
/// use rspinyin::engine::{KeyBindings, is_shift_press, translate_key};
/// use rspinyin::ffi::FcitxKeyEvent;
///
/// let shift = FcitxKeyEvent {
///     sym: 0xffe1,
///     state: 0x01,
///     is_release: false,
///     time_ms: 0,
/// };
/// // No row names the modifier, so the table hands it back ...
/// assert_eq!(translate_key(&shift, &KeyBindings::default()), KeyAction::Ignore);
/// // ... and the routing layer recognises it as the modifier it is.
/// assert!(is_shift_press(&shift));
/// ```
pub fn is_shift_press(event: &FcitxKeyEvent) -> bool {
    event.sym == KEY_SHIFT_L || event.sym == KEY_SHIFT_R
}

// ── The keys that leave temporary English ────────────────────────────────────────

/// Whether this key is one of the two that leave temporary English mode.
///
/// Temporary English hands every key to the application, the `Return` and `Escape` that
/// leave it included: what ends the mode changes no other state and produces no effect, so
/// taking the key would take a keystroke the user typed for the application. Which keys end
/// it is therefore not something the routing table can say, because the two do not *mean*
/// anything different from the keys that stay: the space bar and the `Return` of the
/// shipped configuration are both [`KeyAction::CommitHighlighted`], and a mode whose exit
/// was read out of the action ended on a space bar press — the one key the mode exists to
/// pass through.
///
/// The predicate is the half of the pair that names the key; the other half is
/// `Session::leave_temp_english`, which takes the mode off. Both are needed, because the
/// mode lives in the session and the key lives here, and the pair is asked by the one layer
/// that holds both. A layer that steps a session with the action a key was translated to —
/// the layered bus, whose caller steps even the keys it declined — has to ask this first
/// and take the mode off through the session, because the step alone cannot end the mode
/// for the `Return` of a document that commits the highlighted candidate on it.
///
/// # Arguments
///
/// * `event` — the key as the host delivered it.
///
/// # Returns
///
/// `true` for a press of `Return` or `Escape`, and `false` for every other key and for
/// every release. A release never changes the mode: the host delivers both edges of every
/// key, and the application's key-up is not a second gesture.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_types::KeyAction;
/// use rspinyin::engine::{KeyBindings, leaves_temp_english, translate_key};
/// use rspinyin::ffi::FcitxKeyEvent;
///
/// let space = FcitxKeyEvent { sym: 0x0020, state: 0, is_release: false, time_ms: 0 };
/// let enter = FcitxKeyEvent { sym: 0xff0d, state: 0, is_release: false, time_ms: 0 };
/// // The table reads the two keys as the same key ...
/// assert_eq!(translate_key(&space, &KeyBindings::default()), KeyAction::CommitHighlighted);
/// assert_eq!(translate_key(&enter, &KeyBindings::default()), KeyAction::CommitHighlighted);
/// // ... and only one of them ends the mode.
/// assert!(!leaves_temp_english(&space));
/// assert!(leaves_temp_english(&enter));
/// ```
pub fn leaves_temp_english(event: &FcitxKeyEvent) -> bool {
    !event.is_release && (event.sym == KEY_RETURN || event.sym == KEY_ESCAPE)
}

#[cfg(test)]
mod temp_english_tests {
    //! What the routing layer does with a key while temporary English is on.
    //!
    //! The mode is the one place where "what a key means" and "which key leaves the mode"
    //! come apart: the space bar and the `Return` key of the shipped configuration are
    //! translated to the same action, and only one of them ends the mode. These cases drive
    //! [`KeyRouter::key_event`] end to end — the table, the mode and the session — and assert
    //! the three properties that make the mode a mode: the chord that enters it is the
    //! plugin's, every key inside it reaches the application untouched, and the two that
    //! leave it end the mode *and* reach the application in the same stroke.
    //!
    //! The doubles are as small as the path allows: no key of the mode is decoded, so the
    //! dictionary is empty and the host records nothing.

    use ime_core::lm::InMemoryLm;
    use ime_core::privacy::DefaultPolicy;
    use ime_core::state::{SessionEnv, SessionState};
    use ime_core::viterbi::Decoder;
    use ime_types::{ImeError, Lexicon, SyllableId, UiCommand, UserFreqSource, WordIter};

    use crate::engine::host::Host;
    use crate::ffi::FcitxKeyEvent;
    use crate::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};

    use super::{
        CTRL, KEY_1, KEY_A, KEY_BACKSPACE, KEY_E, KEY_ESCAPE, KEY_RETURN, KEY_SPACE, KeyBindings,
        KeyRouter, RoutingConfig, SHIFT,
    };

    /// The input context every case uses.
    const IC: u64 = 1;

    /// A dictionary with no entry at all.
    ///
    /// Not a shortcut: the mode is answered before any decode, so no key of it needs a
    /// candidate, and a case that got as far as a decode would be exercising something other
    /// than the mode.
    struct EmptyLexicon;

    impl Lexicon for EmptyLexicon {
        fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
            Ok(WordIter::from_vec(Vec::new()))
        }

        fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
            Ok(WordIter::from_vec(Vec::new()))
        }

        fn fallback_single(
            &self,
            _syl: SyllableId,
            _limit: usize,
        ) -> Result<WordIter<'_>, ImeError> {
            Ok(WordIter::from_vec(Vec::new()))
        }
    }

    /// A user-frequency source that answers nothing and records nothing.
    struct SilentUser;

    impl UserFreqSource for SilentUser {
        fn freq(&self, _key: &str) -> u32 {
            0
        }

        fn record(&self, _key: &str, _weight_hint: u16) {}

        fn is_user_word(&self, _key: &str) -> bool {
            false
        }
    }

    /// A host that does nothing: no key of the mode reaches it, and a case that observed a
    /// call would be a failure of the mode rather than of this double.
    struct SilentHost;

    impl Host for SilentHost {
        fn commit(&mut self, _ic: u64, _text: &str) {}

        fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

        fn clear_preedit(&mut self, _ic: u64) {}

        fn post_ui(&mut self, _ic: u64, _command: UiCommand) {}

        fn toggle_enabled(&mut self, _ic: u64) -> bool {
            true
        }

        fn diagnose(&mut self, _ic: u64, _err: &ImeError) {}
    }

    /// The sources a router decodes against.
    struct Fixture {
        /// The dictionary the decode reads.
        lexicon: EmptyLexicon,
        /// The user frequencies the commit path writes to.
        user: SilentUser,
        /// The language model the ranking is scored with.
        lm: InMemoryLm,
        /// The decoder, built with the shipped configuration.
        decoder: Decoder,
    }

    impl Fixture {
        /// Builds the doubles.
        fn new() -> Self {
            Self {
                lexicon: EmptyLexicon,
                user: SilentUser,
                lm: InMemoryLm::new(),
                decoder: Decoder::default(),
            }
        }

        /// The sources as the environment a session step reads.
        fn env(&self) -> SessionEnv<'_> {
            SessionEnv {
                decoder: &self.decoder,
                lexicon: &self.lexicon,
                user_freq: &self.user,
                lm: &self.lm,
            }
        }

        /// A router with [`IC`] activated as an ordinary, learnable context.
        fn router(&self, config: RoutingConfig) -> KeyRouter<'_> {
            let mut router = KeyRouter::new(
                self.env(),
                ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default()),
                config,
            );
            router.activate_reported(IC, ordinary_report());
            router
        }
    }

    /// What the host reports about an ordinary context: no program name, neither the
    /// password nor the sensitive flag.
    fn ordinary_report() -> ContextReport<'static> {
        ContextReport::Reported {
            program: None,
            password: false,
            sensitive: false,
        }
    }

    /// A key press of `sym` with `state` held.
    fn press(sym: u32, state: u32) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym,
            state,
            is_release: false,
            time_ms: 0,
        }
    }

    /// Whether the session of [`IC`] is in temporary English mode.
    fn is_temp_english(router: &KeyRouter<'_>) -> bool {
        router
            .session(IC)
            .is_some_and(|session| session.temp_english)
    }

    #[test]
    fn test_temp_english_chord_is_kept_and_the_mode_hands_every_key_back() {
        // The two ends of the mode, in one session. The chord that enters it is the plugin's
        // — turning the mode on is the whole of what it did, so the key is not one the user
        // pressed and saw nothing happen on. Every key of the mode reaches the application,
        // and the mode survives all of them.
        let fixture = Fixture::new();
        let mut router = fixture.router(RoutingConfig::default());
        let mut host = SilentHost;

        assert!(
            router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host),
            "the chord enters the mode, so it is the plugin's key"
        );
        assert!(is_temp_english(&router), "and the mode is on");

        for sym in [KEY_A, KEY_SPACE, KEY_1, KEY_BACKSPACE] {
            assert!(
                !router.key_event(IC, &press(sym, 0), &mut host),
                "sym {sym:#06x} must reach the application in temporary English"
            );
            assert!(is_temp_english(&router), "and must not end the mode");
        }

        assert!(
            !router.key_event(IC, &press(KEY_RETURN, 0), &mut host),
            "the return key that leaves the mode reaches the application too"
        );
        assert!(!is_temp_english(&router), "and leaves it");

        assert!(
            router.key_event(IC, &press(KEY_A, 0), &mut host),
            "the plugin answers the next key again"
        );
    }

    #[test]
    fn test_temp_english_return_leaves_the_mode_in_both_configurations() {
        // `Return` means "commit the raw input" in one document and "commit the highlighted
        // candidate" in the other, and the space bar shares the second of those actions.
        // Leaving the mode is a property of the key, so the answer is the same either way —
        // and the key reaches the application in both.
        let raw = RoutingConfig {
            keys: KeyBindings {
                enter_commit_raw: true,
                ..KeyBindings::default()
            },
            ..RoutingConfig::default()
        };
        let documents = [
            ("the shipped document", RoutingConfig::default()),
            ("enter_commit_raw", raw),
        ];
        for (label, config) in documents {
            let fixture = Fixture::new();
            let mut router = fixture.router(config);
            let mut host = SilentHost;
            assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));

            assert!(
                !router.key_event(IC, &press(KEY_RETURN, 0), &mut host),
                "the return key reaches the application under {label}"
            );
            assert!(
                !is_temp_english(&router),
                "and leaves the mode under {label}"
            );
            assert_eq!(
                router.session(IC).map(|session| session.state),
                Some(SessionState::Idle),
                "leaving the mode commits nothing under {label}"
            );
        }
    }

    #[test]
    fn test_temp_english_escape_ends_the_mode_and_is_handed_back() {
        // The control key of the mode. It ends the mode and reaches the application in the
        // same stroke, so neither half of the gesture is lost, and the session is left in a
        // state the next key can act on rather than behind a mode nothing can leave.
        let fixture = Fixture::new();
        let mut router = fixture.router(RoutingConfig::default());
        let mut host = SilentHost;
        assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));
        // One key of the mode first, so the escape is not the stroke that entered it.
        assert!(!router.key_event(IC, &press(KEY_A, 0), &mut host));

        assert!(
            !router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host),
            "the escape reaches the application"
        );
        assert!(!is_temp_english(&router), "and ends the mode");
        assert_eq!(
            router.session(IC).map(|session| session.state),
            Some(SessionState::Idle),
            "the mode holds no composition to cancel"
        );
        assert_eq!(
            router.session(IC).map(|session| session.buf.raw()),
            Some(""),
            "and no input to take back"
        );
        assert!(
            router.key_event(IC, &press(KEY_A, 0), &mut host),
            "and the next key is the plugin's again"
        );
    }
}
