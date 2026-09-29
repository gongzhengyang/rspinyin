//! The routing table, row by row.
//!
//! Every row of the design's shortcut table is a case here, including the two rows whose
//! answer depends on the configuration (`digit_zero` and `enter_commit_raw`), the chords
//! whose modifiers are part of the key, and the keys the table deliberately does not name.
//! The last test is the "never swallow" half of the contract: a keysym the table has no
//! row for is never claimed.

use ime_types::KeyAction;

use crate::engine::*;

use super::{keysym_stream, press, release};

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
fn test_is_shift_press_names_the_two_shift_keys_and_nothing_else() {
    assert!(is_shift_press(&press(KEY_SHIFT_L, SHIFT)));
    assert!(is_shift_press(&press(KEY_SHIFT_R, SHIFT)));
    // The key is the same on both edges, and the routing layer hands both back.
    assert!(is_shift_press(&release(KEY_SHIFT_L, 0)));
    // The chords Shift takes part in carry the other key's symbol.
    assert!(!is_shift_press(&press(KEY_SPACE, SHIFT)));
    assert!(!is_shift_press(&press(KEY_A, SHIFT)));
    assert!(!is_shift_press(&press(KEY_E_UPPER, CTRL | SHIFT)));
}

#[test]
fn test_translate_key_never_claims_a_key_the_table_does_not_name() {
    // The shortcut table forbids consuming a key nothing acts on. This is the half the
    // table can assert on its own: an action may be claimed for a key the table names,
    // and for no other. The other half -- that a claimed key reached the session -- is
    // asserted where the session is driven.
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
