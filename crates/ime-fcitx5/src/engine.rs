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
//! and [`claims_key`] makes it explicit; the caller adds the second. The host-side effect
//! of the pair is `filterAndAccept` in the engine's `keyEvent`, and a key that fails
//! either condition must reach neither.
//!
//! # What is not here yet
//!
//! The session state machine (`ime-core`) and the host-side effect application
//! (`apply_effects`) do not exist, so nothing can execute an action and no key may be
//! claimed: the callback in `ffi::abi` answers "not handled" for every key and the engine
//! therefore consumes none of them. The routing table below is what the session will be
//! driven with once it lands, and its rows are pinned by the tests at the bottom of this
//! file.
//!
//! # Configuration
//!
//! [`KeyBindings`] is this layer's projection of the `[keys]` section: the three settings
//! the routing table reads, with the defaults the shipped `config/default.toml` declares.
//! It is a host-layer type rather than a contract type — `ime-config` owns the file
//! format, the key-name whitelist and the validation — so a change to the configuration
//! schema lands there and is converted here.

use ime_types::KeyAction;

use crate::ffi::FcitxKeyEvent;

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
/// `FcitxKey_Shift_L`.
const KEY_SHIFT_L: u32 = 0xffe1;
/// `FcitxKey_Shift_R`.
const KEY_SHIFT_R: u32 = 0xffe2;

/// What the `0` key does while a composition is active (`[keys] digit_zero`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigitZero {
    /// `"passthrough"`, the default: `0` is a digit the application receives.
    Passthrough,
    /// `"flip"`: `0` pages the candidate list forward, like the other page keys.
    Flip,
}

/// The keys that can page the candidate list (`[keys] flip_keys`).
///
/// A set rather than a list: the routing table asks about one key at a time, and the
/// configuration's whitelist offers exactly these four names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlipKeys {
    /// Whether `-` pages backwards.
    pub minus: bool,
    /// Whether `=` pages forwards.
    pub equal: bool,
    /// Whether `Up` pages backwards.
    pub up: bool,
    /// Whether `Down` pages forwards.
    pub down: bool,
}

impl FlipKeys {
    /// The set the shipped `config/default.toml` declares: all four keys.
    pub const fn all() -> Self {
        Self {
            minus: true,
            equal: true,
            up: true,
            down: true,
        }
    }
}

impl Default for FlipKeys {
    fn default() -> Self {
        Self::all()
    }
}

/// The `[keys]` settings the routing table reads.
///
/// The defaults are the ones the shipped `config/default.toml` declares, so a caller that
/// has no configuration yet routes keys exactly as a fresh installation would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBindings {
    /// What `0` does while a composition is active.
    pub digit_zero: DigitZero,
    /// Whether `Enter` commits the raw input instead of the highlighted candidate.
    pub enter_commit_raw: bool,
    /// The keys that page the candidate list.
    pub flip_keys: FlipKeys,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            digit_zero: DigitZero::Passthrough,
            enter_commit_raw: false,
            flip_keys: FlipKeys::all(),
        }
    }
}

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
/// Never. The table and the three lookups it delegates to read only the two integers in
/// `event` and the three fields of `keys`: no indexing, no arithmetic that can overflow,
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
        if let Some(action) = shift_tolerant_action(sym, state) {
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

/// The rows that tolerate a held Shift: a letter, and Tab with its reversed direction.
///
/// Returns `None` when the key is neither, so the caller can go on with the rows that
/// require a bare press.
fn shift_tolerant_action(sym: u32, state: u32) -> Option<KeyAction> {
    if (KEY_A..=KEY_Z).contains(&sym) {
        // The range is ASCII, so the low byte is the character.
        return Some(KeyAction::InputChar(char::from(sym as u8)));
    }
    if sym == KEY_TAB && (state & SHIFT) == 0 {
        return Some(KeyAction::MoveHighlight(1));
    }
    if sym == KEY_TAB {
        return Some(KeyAction::MoveHighlight(-1));
    }
    None
}

/// The rows that require a bare press: the digits, the page keys, the arrows and the
/// three editing keys. Anything else stays with the host.
fn bare_action(sym: u32, keys: &KeyBindings) -> KeyAction {
    match sym {
        KEY_0 if keys.digit_zero == DigitZero::Flip => KeyAction::PageNext,
        KEY_0 => KeyAction::Ignore,
        KEY_1..=KEY_9 => KeyAction::SelectIndex((sym - KEY_0) as u8),
        KEY_MINUS if keys.flip_keys.minus => KeyAction::PagePrev,
        KEY_EQUAL if keys.flip_keys.equal => KeyAction::PageNext,
        KEY_UP if keys.flip_keys.up => KeyAction::PagePrev,
        KEY_DOWN if keys.flip_keys.down => KeyAction::PageNext,
        KEY_LEFT => KeyAction::MoveCaret(-1),
        KEY_RIGHT => KeyAction::MoveCaret(1),
        KEY_RETURN if keys.enter_commit_raw => KeyAction::CommitRaw,
        KEY_RETURN => KeyAction::CommitHighlighted,
        KEY_ESCAPE => KeyAction::Escape,
        KEY_BACKSPACE => KeyAction::Backspace,
        _ => KeyAction::Ignore,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A key press of `sym` with `state` held.
    fn press(sym: u32, state: u32) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym,
            state,
            is_release: false,
            time_ms: 7,
        }
    }

    /// A key release of `sym` with `state` held.
    fn release(sym: u32, state: u32) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym,
            state,
            is_release: true,
            time_ms: 7,
        }
    }

    /// Asserts a table of cases, naming the key that did not match.
    fn assert_rows(cases: &[(FcitxKeyEvent, KeyAction)], keys: &KeyBindings) {
        for (key, expected) in cases {
            assert_eq!(
                translate_key(key, keys),
                *expected,
                "sym {:#06x} state {:#x} release {}",
                key.sym,
                key.state,
                key.is_release
            );
        }
    }

    /// The keysyms the routing table has a row for under some modifiers.
    ///
    /// The "never swallow" test asserts that nothing outside this set is ever claimed, so
    /// a table edit that starts claiming an unrelated key fails here instead of silently
    /// removing that key from the application's input.
    fn is_routed(sym: u32) -> bool {
        const NAMED: [u32; 15] = [
            KEY_SPACE,
            KEY_PERIOD,
            KEY_MINUS,
            KEY_EQUAL,
            KEY_BACKSPACE,
            KEY_TAB,
            KEY_RETURN,
            KEY_ESCAPE,
            KEY_LEFT,
            KEY_UP,
            KEY_RIGHT,
            KEY_DOWN,
            KEY_SHIFT_L,
            KEY_SHIFT_R,
            KEY_E_UPPER,
        ];
        (KEY_A..=KEY_Z).contains(&sym) || (KEY_0..=KEY_9).contains(&sym) || NAMED.contains(&sym)
    }

    /// A deterministic keysym stream from a 32-bit LCG: reproducible, and needs no
    /// dependency (`rand` is not one of this workspace's crates).
    fn keysym_stream(count: usize) -> impl Iterator<Item = u32> {
        let mut seed: u32 = 0x1234_5678;
        (0..count).map(move |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        })
    }

    /// The default bindings with only the two punctuation keys paging the list.
    fn punctuation_flip_bindings() -> KeyBindings {
        KeyBindings {
            flip_keys: FlipKeys {
                minus: true,
                equal: true,
                up: false,
                down: false,
            },
            ..KeyBindings::default()
        }
    }

    #[test]
    fn test_modifier_mask_matches_the_installed_key_state_header() {
        // 5.1.7 `fcitx-utils/keysym.h`: `SimpleMask = Ctrl_Alt_Shift | Super | Super2 |
        // Hyper | Meta`. A mismatch means the host moved the bits, not that this table is
        // free to be renumbered.
        assert_eq!(MODIFIER_MASK, 0x1400_006d);
        assert_eq!(NON_SHIFT_MODIFIERS, 0x1400_006c);
        assert_eq!(NON_SHIFT_MODIFIERS | SHIFT, MODIFIER_MASK);
    }

    #[test]
    fn test_key_bindings_default_matches_the_shipped_configuration() {
        let keys = KeyBindings::default();
        assert_eq!(keys.digit_zero, DigitZero::Passthrough);
        assert!(!keys.enter_commit_raw);
        assert_eq!(keys.flip_keys, FlipKeys::all());
    }

    #[test]
    fn test_translate_key_ignores_every_key_release() {
        let keys = KeyBindings::default();
        for (sym, state) in [
            (KEY_A, 0),
            (KEY_SPACE, 0),
            (KEY_RETURN, 0),
            (KEY_ESCAPE, 0),
            (KEY_BACKSPACE, 0),
            (KEY_TAB, SHIFT),
            (KEY_SHIFT_L, SHIFT),
        ] {
            let action = translate_key(&release(sym, state), &keys);
            assert_eq!(action, KeyAction::Ignore, "{sym:#06x} must reach the app");
        }
    }

    #[test]
    fn test_translate_key_maps_unmodified_letters_to_input_char() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_A, 0), KeyAction::InputChar('a')),
                (press(KEY_E, 0), KeyAction::InputChar('e')),
                (press(KEY_Z, 0), KeyAction::InputChar('z')),
                // Shift is how an uppercase letter is typed, and the row tolerates it.
                (press(KEY_A, SHIFT), KeyAction::InputChar('a')),
                // Every other modifier makes the key somebody else's chord.
                (press(KEY_A, CTRL), KeyAction::Ignore),
                (press(KEY_A, ALT), KeyAction::Ignore),
                (press(KEY_A, SUPER), KeyAction::Ignore),
                (press(KEY_A, CTRL | SHIFT), KeyAction::Ignore),
                // The syms one step outside the range are not letters.
                (press(KEY_A - 1, 0), KeyAction::Ignore),
                (press(KEY_Z + 1, 0), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_maps_space_to_commit_highlighted() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_SPACE, 0), KeyAction::CommitHighlighted),
                (press(KEY_SPACE, ALT), KeyAction::Ignore),
                (press(KEY_SPACE, CTRL | SHIFT), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_selects_candidates_with_the_digits() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_0 + 1, 0), KeyAction::SelectIndex(1)),
                (press(KEY_0 + 5, 0), KeyAction::SelectIndex(5)),
                (press(KEY_0 + 9, 0), KeyAction::SelectIndex(9)),
                (press(KEY_0 + 5, CTRL), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_routes_digit_zero_by_configuration() {
        let passthrough = KeyBindings::default();
        let flipping = KeyBindings {
            digit_zero: DigitZero::Flip,
            ..KeyBindings::default()
        };
        assert_rows(
            &[
                (press(KEY_0, 0), KeyAction::Ignore),
                (press(KEY_0, CTRL), KeyAction::Ignore),
            ],
            &passthrough,
        );
        assert_rows(
            &[
                (press(KEY_0, 0), KeyAction::PageNext),
                // The branch is about the digit, not about the modifier.
                (press(KEY_0, CTRL), KeyAction::Ignore),
            ],
            &flipping,
        );
    }

    #[test]
    fn test_translate_key_pages_with_the_configured_flip_keys() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_MINUS, 0), KeyAction::PagePrev),
                (press(KEY_EQUAL, 0), KeyAction::PageNext),
                (press(KEY_UP, 0), KeyAction::PagePrev),
                (press(KEY_DOWN, 0), KeyAction::PageNext),
                // The flip rows are bare presses, so Shift keeps them with the host.
                (press(KEY_MINUS, SHIFT), KeyAction::Ignore),
            ],
            &keys,
        );
        // A configuration that drops the arrows hands those two back to the application.
        let punctuation_only = punctuation_flip_bindings();
        assert_rows(
            &[
                (press(KEY_MINUS, 0), KeyAction::PagePrev),
                (press(KEY_EQUAL, 0), KeyAction::PageNext),
                (press(KEY_UP, 0), KeyAction::Ignore),
                (press(KEY_DOWN, 0), KeyAction::Ignore),
            ],
            &punctuation_only,
        );
    }

    #[test]
    fn test_translate_key_moves_the_highlight_and_the_caret() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_TAB, 0), KeyAction::MoveHighlight(1)),
                (press(KEY_TAB, SHIFT), KeyAction::MoveHighlight(-1)),
                (press(KEY_TAB, CTRL), KeyAction::Ignore),
                (press(KEY_TAB, CTRL | SHIFT), KeyAction::Ignore),
                (press(KEY_LEFT, 0), KeyAction::MoveCaret(-1)),
                (press(KEY_RIGHT, 0), KeyAction::MoveCaret(1)),
                (press(KEY_LEFT, SHIFT), KeyAction::Ignore),
                (press(KEY_RIGHT, ALT), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_routes_enter_by_configuration() {
        let keys = KeyBindings::default();
        let raw = KeyBindings {
            enter_commit_raw: true,
            ..KeyBindings::default()
        };
        assert_eq!(
            translate_key(&press(KEY_RETURN, 0), &keys),
            KeyAction::CommitHighlighted
        );
        assert_eq!(
            translate_key(&press(KEY_RETURN, 0), &raw),
            KeyAction::CommitRaw
        );
        // The keypad's Enter is not this row: it stays with the application.
        assert_eq!(translate_key(&press(0xff8d, 0), &raw), KeyAction::Ignore);
    }

    #[test]
    fn test_translate_key_maps_escape_and_backspace() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_ESCAPE, 0), KeyAction::Escape),
                (press(KEY_BACKSPACE, 0), KeyAction::Backspace),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_maps_the_global_mode_chords() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_SHIFT_L, SHIFT), KeyAction::ToggleLang),
                (press(KEY_SHIFT_R, SHIFT), KeyAction::ToggleLang),
                (press(KEY_SPACE, CTRL), KeyAction::ToggleLang),
                (press(KEY_SPACE, SHIFT), KeyAction::ToggleFullWidth),
                (press(KEY_PERIOD, CTRL), KeyAction::TogglePunct),
                // A chord is its whole modifier set: one extra modifier is another key.
                (press(KEY_SPACE, CTRL | ALT), KeyAction::Ignore),
                (press(KEY_SPACE, CTRL | SHIFT), KeyAction::Ignore),
                (press(KEY_SPACE, SHIFT | SUPER), KeyAction::Ignore),
                (press(KEY_PERIOD, CTRL | SHIFT), KeyAction::Ignore),
                (press(KEY_PERIOD, 0), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_enters_temporary_english_on_ctrl_shift_e() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(KEY_E, CTRL | SHIFT), KeyAction::EnterTempEnglish),
                // A chord containing Shift may arrive with the uppercase symbol, because
                // the host folds the case into the symbol; the row matches both shapes.
                (
                    press(KEY_E_UPPER, CTRL | SHIFT),
                    KeyAction::EnterTempEnglish,
                ),
                // Either modifier alone is not the chord.
                (press(KEY_E, CTRL), KeyAction::Ignore),
                (press(KEY_E, SHIFT), KeyAction::InputChar('e')),
                (press(KEY_E_UPPER, SHIFT), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_ignores_keys_the_table_does_not_name() {
        let keys = KeyBindings::default();
        assert_rows(
            &[
                (press(0xffbe, 0), KeyAction::Ignore),
                (press(0xff63, 0), KeyAction::Ignore),
                (press(0xff8d, 0), KeyAction::Ignore),
                (press(0x002c, 0), KeyAction::Ignore),
                (press(0xffe3, SHIFT), KeyAction::Ignore),
            ],
            &keys,
        );
    }

    #[test]
    fn test_translate_key_never_claims_a_key_the_table_does_not_name() {
        // The shortcut table forbids consuming a key nothing acts on. Until the session
        // exists the only half that can be asserted is this one: an action may be claimed
        // for a key the table names, and for no other.
        let keys = KeyBindings::default();
        let states = [0, SHIFT, CTRL, CTRL | SHIFT | ALT];
        let mut checked = 0;
        for sym in keysym_stream(200) {
            for state in states {
                let action = translate_key(&press(sym, state), &keys);
                assert!(
                    !claims_key(action) || is_routed(sym),
                    "sym {sym:#010x} state {state:#x} claimed as {action:?} without a row"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 800, "the sweep must have run");
    }

    #[test]
    fn test_claims_key_follows_ignore() {
        assert!(!claims_key(KeyAction::Ignore));
        assert!(claims_key(KeyAction::InputChar('a')));
        assert!(claims_key(KeyAction::Escape));
        assert!(claims_key(KeyAction::MoveHighlight(-1)));
    }
}
