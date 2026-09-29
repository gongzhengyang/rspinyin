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
//!   modifier constants they read — is this file. The bindings it branches on
//!   ([`KeyBindings`], [`FlipSet`], [`HighlightSet`]) belong to `ime-config` and are
//!   re-exported here, so the table and the configuration document cannot drift apart.
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

use ime_types::KeyAction;

use crate::ffi::FcitxKeyEvent;

pub mod arbiter;
pub mod context;
pub mod host;
pub mod modifier;
pub mod router;
pub mod sequence;

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
mod tests;

// ── Modifier bits ────────────────────────────────────────────────────────────────
//
// The values are `fcitx::KeyState`'s, taken from the installed Fcitx5 5.1.7 header
// `fcitx-utils/keysym.h`, which defines the enum as a `uint32_t`; `FcitxKeyEvent::state`
// carries `KeyStates::toInteger()` unchanged. The test module pins the mask against the
// header's own `SimpleMask`, so a host that renumbers these bits fails a test rather than
// silently misreading every chord.

/// `fcitx::KeyState::Shift`.
const SHIFT: u32 = 1 << 0;
/// `fcitx::KeyState::Ctrl`.
const CTRL: u32 = 1 << 2;
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

/// `FcitxKey_space`.
const KEY_SPACE: u32 = 0x0020;
/// `FcitxKey_minus`.
const KEY_MINUS: u32 = 0x002d;
/// `FcitxKey_period`.
const KEY_PERIOD: u32 = 0x002e;
/// `FcitxKey_0`, the low end of the digit row.
const KEY_0: u32 = 0x0030;
/// `FcitxKey_1`, the low end of the selectable digits: `0` is routed by configuration
/// rather than being a candidate index, so the selection range starts here.
const KEY_1: u32 = 0x0031;
/// `FcitxKey_9`, the high end of the digit row.
const KEY_9: u32 = 0x0039;
/// `FcitxKey_equal`.
const KEY_EQUAL: u32 = 0x003d;
/// `FcitxKey_E`, the uppercase shape a chord containing Shift may deliver.
const KEY_E_UPPER: u32 = 0x0045;
/// `FcitxKey_a`, the low end of the letter row.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_e`, the letter of the temporary-English chord.
const KEY_E: u32 = 0x0065;
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
/// Never. The table and the lookups it delegates to read only the two integers in `event`
/// and the four settings of `keys`: no indexing, no arithmetic that can overflow, no
/// allocation. The guarantee matters because the caller is an FFI entry point, which must
/// not unwind into C++.
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
    // one would eat the application's key-up. The table below is a table of presses.
    if event.is_release {
        return KeyAction::Ignore;
    }
    let sym = event.sym;
    let state = event.state;

    if let Some(action) = chord_action(sym, state) {
        return action;
    }
    // The rows that tolerate a held Shift: a letter, because Shift is how its uppercase
    // form is typed, and Tab, where Shift reverses the direction.
    if (state & NON_SHIFT_MODIFIERS) == 0 {
        if let Some(action) = shift_tolerant_action(sym, state, keys) {
            return action;
        }
    }
    // Every row below is a bare press: a surviving modifier belongs to somebody else.
    if (state & MODIFIER_MASK) != 0 {
        return KeyAction::Ignore;
    }
    bare_action(sym, keys)
}

/// The rows whose modifier is part of the key: the global mode chords.
///
/// Returns `None` when the key is not one of them, so the caller can go on with the rows
/// that read a bare key. `Some(KeyAction::Ignore)` is a real answer here — the space bar
/// with a modifier combination that is nobody's chord is the host's key.
fn chord_action(sym: u32, state: u32) -> Option<KeyAction> {
    if sym == KEY_SHIFT_L || sym == KEY_SHIFT_R {
        return Some(KeyAction::ToggleLang);
    }
    if sym == KEY_SPACE {
        return Some(match state & MODIFIER_MASK {
            CTRL => KeyAction::ToggleLang,
            SHIFT => KeyAction::ToggleFullWidth,
            0 => KeyAction::CommitHighlighted,
            _ => KeyAction::Ignore,
        });
    }
    if sym == KEY_PERIOD && (state & MODIFIER_MASK) == CTRL {
        return Some(KeyAction::TogglePunct);
    }
    if (sym == KEY_E || sym == KEY_E_UPPER) && (state & MODIFIER_MASK) == (CTRL | SHIFT) {
        return Some(KeyAction::EnterTempEnglish);
    }
    None
}

/// The rows that tolerate a held Shift: a letter, and the Tab shapes.
///
/// Tab is the one key whose held Shift changes its meaning rather than blocking it, and the
/// configuration names its two shapes apart (`keys.highlight_keys`): a document that binds
/// only one of them leaves the other to the application. Returns `None` when the key is
/// neither, so the caller can go on with the rows that require a bare press.
fn shift_tolerant_action(sym: u32, state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    if (KEY_A..=KEY_Z).contains(&sym) {
        // The range is ASCII, so the low byte is the character.
        return Some(KeyAction::InputChar(char::from(sym as u8)));
    }
    if sym != KEY_TAB {
        return None;
    }
    let (binding, delta) = if (state & SHIFT) == 0 {
        (HighlightSet::TAB, 1)
    } else {
        (HighlightSet::SHIFT_TAB, -1)
    };
    keys.highlight_keys
        .contains(binding)
        .then_some(KeyAction::MoveHighlight(delta))
}

/// The rows that require a bare press: the digits, the page keys, the arrows and the
/// three editing keys. Anything else stays with the host.
fn bare_action(sym: u32, keys: &KeyBindings) -> KeyAction {
    // The highlight rows come first. The projection drops a page binding from a key both
    // lists name, so a key the configuration bound twice moves the highlight here -- which
    // is the precedence `ime-config` states when it reports the collision.
    if let Some(action) = bare_highlight(sym, keys.highlight_keys) {
        return action;
    }
    match sym {
        KEY_0 if keys.digit_zero == DigitZero::Flip => KeyAction::PageNext,
        KEY_0 => KeyAction::Ignore,
        KEY_1..=KEY_9 => KeyAction::SelectIndex((sym - KEY_0) as u8),
        KEY_MINUS if keys.flip_keys.contains(FlipSet::MINUS) => KeyAction::PagePrev,
        KEY_EQUAL if keys.flip_keys.contains(FlipSet::EQUAL) => KeyAction::PageNext,
        KEY_UP if keys.flip_keys.contains(FlipSet::UP) => KeyAction::PagePrev,
        KEY_DOWN if keys.flip_keys.contains(FlipSet::DOWN) => KeyAction::PageNext,
        KEY_PAGE_UP if keys.flip_keys.contains(FlipSet::PAGE_UP) => KeyAction::PagePrev,
        KEY_PAGE_DOWN if keys.flip_keys.contains(FlipSet::PAGE_DOWN) => KeyAction::PageNext,
        KEY_LEFT => KeyAction::MoveCaret(-1),
        KEY_RIGHT => KeyAction::MoveCaret(1),
        KEY_RETURN if keys.enter_commit_raw => KeyAction::CommitRaw,
        KEY_RETURN => KeyAction::CommitHighlighted,
        KEY_ESCAPE => KeyAction::Escape,
        KEY_BACKSPACE => KeyAction::Backspace,
        _ => KeyAction::Ignore,
    }
}

/// The row a `keys.highlight_keys` binding names, or `None` when the key carries none.
///
/// The four arrows are shared with the page and the caret rows: a key the configuration put
/// in `highlight_keys` moves the highlight, and the row below it in [`bare_action`] is what
/// answers when the configuration did not.
fn bare_highlight(sym: u32, set: HighlightSet) -> Option<KeyAction> {
    let (binding, delta) = match sym {
        KEY_UP => (HighlightSet::UP, -1),
        KEY_DOWN => (HighlightSet::DOWN, 1),
        KEY_LEFT => (HighlightSet::LEFT, -1),
        KEY_RIGHT => (HighlightSet::RIGHT, 1),
        _ => return None,
    };
    set.contains(binding)
        .then_some(KeyAction::MoveHighlight(delta))
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
/// shortcut table forbids.
///
/// # Panics
///
/// Never.
pub fn claims_key(action: KeyAction) -> bool {
    !matches!(action, KeyAction::Ignore)
}

/// Whether the event is a press of a Shift key itself.
///
/// The routing table names a Shift press [`KeyAction::ToggleLang`], because the design's
/// shortcut table lists the held Shift key as the temporary Chinese / English switch. The
/// switch is not the plugin's to make: Fcitx5 delivers the modifier to the application and
/// implements the temporary behaviour of its own, and an input method that consumed the
/// press would take the first half of every capital letter away from the application. The
/// routing layer therefore asks this and hands the press straight back, which leaves the
/// table's meaning of the key intact for the chords that are the plugin's own — `Shift` in
/// combination with a letter or with Space never reaches here, because those events carry
/// the letter's or the space bar's symbol and not the modifier's.
///
/// # Arguments
///
/// * `event` — the key as the host delivered it.
///
/// # Returns
///
/// `true` for a press or a release of `Shift_L` or `Shift_R`. A release is not claimed by
/// [`translate_key`] in the first place; the answer is about the key, not about the edge.
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
/// // The table names the key ...
/// assert_eq!(translate_key(&shift, &KeyBindings::default()), KeyAction::ToggleLang);
/// // ... and the routing layer still hands it back.
/// assert!(is_shift_press(&shift));
/// ```
pub fn is_shift_press(event: &FcitxKeyEvent) -> bool {
    event.sym == KEY_SHIFT_L || event.sym == KEY_SHIFT_R
}
