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

/// `FcitxKey_E`, the uppercase shape a host that folds the case into the symbol delivers
/// for `Shift+e`.
///
/// Derived from the two range ends rather than written as `0x45`, so the test names the
/// letter it means and the fold's arithmetic stays in one place.
const KEY_E_UPPER: u32 = KEY_A_UPPER + (KEY_E - KEY_A);

/// The default bindings with only the two punctuation keys paging the list.
fn punctuation_flip_bindings() -> KeyBindings {
    KeyBindings {
        flip_keys: FlipSet::MINUS | FlipSet::EQUAL,
        ..KeyBindings::default()
    }
}

/// The keysyms the routing table has a row for under some modifiers.
///
/// The "never swallow" test asserts that nothing outside this set is ever claimed, so
/// a table edit that starts claiming an unrelated key fails here instead of silently
/// removing that key from the application's input. The uppercase letters are in the set
/// only because the fold turns a shifted one back into its lowercase shape: a bare
/// uppercase keysym has no row, and the sweep below asserts that it stays with the host.
fn is_routed(sym: u32, state: u32) -> bool {
    const NAMED: [u32; 15] = [
        KEY_SPACE,
        KEY_APOSTROPHE,
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
        KEY_PAGE_UP,
        KEY_PAGE_DOWN,
    ];
    let read = fold_shifted_letter(sym, state);
    (KEY_A..=KEY_Z).contains(&read)
        || (KEY_0..=KEY_9).contains(&read)
        || NAMED.contains(&read)
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
    assert_eq!(
        keys.flip_keys,
        FlipSet::MINUS | FlipSet::EQUAL | FlipSet::UP | FlipSet::DOWN
    );
    assert_eq!(
        keys.highlight_keys,
        HighlightSet::TAB | HighlightSet::SHIFT_TAB
    );
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
fn test_translate_key_pages_with_the_page_keys_the_configuration_binds() {
    // `page_up` and `page_down` are two of the six names `keys.flip_keys` accepts, and the
    // flag set is what carries them: the four-field struct this used to be read them out of
    // the configuration and then dropped them in silence.
    let page_keys = KeyBindings {
        flip_keys: FlipSet::PAGE_UP | FlipSet::PAGE_DOWN,
        ..KeyBindings::default()
    };
    assert_rows(
        &[
            (press(KEY_PAGE_UP, 0), KeyAction::PagePrev),
            (press(KEY_PAGE_DOWN, 0), KeyAction::PageNext),
            // The page rows are bare presses, like every other row of the list.
            (press(KEY_PAGE_UP, SHIFT), KeyAction::Ignore),
        ],
        &page_keys,
    );
    // The shipped configuration does not bind them, so they stay with the application.
    let shipped = KeyBindings::default();
    assert_rows(
        &[
            (press(KEY_PAGE_UP, 0), KeyAction::Ignore),
            (press(KEY_PAGE_DOWN, 0), KeyAction::Ignore),
        ],
        &shipped,
    );
}

#[test]
fn test_translate_key_moves_the_highlight_with_the_configured_highlight_keys() {
    // Tab and Shift+Tab are two bindings rather than one key, so a document that binds only
    // the shifted shape hands the bare Tab to the application.
    let shifted_only = KeyBindings {
        highlight_keys: HighlightSet::SHIFT_TAB,
        ..KeyBindings::default()
    };
    assert_rows(
        &[
            (press(KEY_TAB, 0), KeyAction::Ignore),
            (press(KEY_TAB, SHIFT), KeyAction::MoveHighlight(-1)),
        ],
        &shifted_only,
    );

    // The four arrows are the other half of the whitelist. Bound as highlight keys they move
    // the highlight, and the page and caret rows they would otherwise answer give way.
    let arrows = KeyBindings {
        flip_keys: FlipSet::empty(),
        highlight_keys: HighlightSet::UP
            | HighlightSet::DOWN
            | HighlightSet::LEFT
            | HighlightSet::RIGHT,
        ..KeyBindings::default()
    };
    assert_rows(
        &[
            (press(KEY_UP, 0), KeyAction::MoveHighlight(-1)),
            (press(KEY_DOWN, 0), KeyAction::MoveHighlight(1)),
            (press(KEY_LEFT, 0), KeyAction::MoveHighlight(-1)),
            (press(KEY_RIGHT, 0), KeyAction::MoveHighlight(1)),
        ],
        &arrows,
    );

    // With the shipped bindings the four are what they always were: pages and a caret.
    let shipped = KeyBindings::default();
    assert_rows(
        &[
            (press(KEY_UP, 0), KeyAction::PagePrev),
            (press(KEY_DOWN, 0), KeyAction::PageNext),
            (press(KEY_LEFT, 0), KeyAction::MoveCaret(-1)),
            (press(KEY_RIGHT, 0), KeyAction::MoveCaret(1)),
        ],
        &shipped,
    );
}

#[test]
fn test_translate_key_routes_the_six_keys_the_highlight_list_can_name() {
    // The whole matrix of `keys.highlight_keys`, four configurations by six keys. Every one
    // of the six is a name the configuration's whitelist accepts, and the row each key falls
    // through to when the list does not name it is what makes the setting mean something:
    // before the table read the list, all six were decided by hard-coded rows and a user who
    // wrote `highlight_keys = ["up", "down"]` got the page keys they had removed.
    //
    // `Up` moves the highlight backwards and `Down` forwards, which is the direction the
    // page rows they displace already had: `Up` turns to the previous page.
    let tab = KeyBindings::default();
    let arrows = KeyBindings {
        highlight_keys: HighlightSet::UP | HighlightSet::DOWN,
        ..KeyBindings::default()
    };
    let sideways = KeyBindings {
        highlight_keys: HighlightSet::LEFT | HighlightSet::RIGHT,
        ..KeyBindings::default()
    };
    let none = KeyBindings {
        highlight_keys: HighlightSet::empty(),
        ..KeyBindings::default()
    };
    // `Tab` and `Shift+Tab`, then the four arrows, in the order the design's matrix lists
    // them.
    let columns: [(u32, u32); 6] = [
        (KEY_TAB, 0),
        (KEY_TAB, SHIFT),
        (KEY_UP, 0),
        (KEY_DOWN, 0),
        (KEY_LEFT, 0),
        (KEY_RIGHT, 0),
    ];
    let configs: [(&str, &KeyBindings, [KeyAction; 6]); 4] = [
        (
            "tab and shift_tab",
            &tab,
            [
                KeyAction::MoveHighlight(1),
                KeyAction::MoveHighlight(-1),
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        ),
        (
            "up and down",
            &arrows,
            [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::MoveHighlight(-1),
                KeyAction::MoveHighlight(1),
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        ),
        (
            "left and right",
            &sideways,
            [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveHighlight(-1),
                KeyAction::MoveHighlight(1),
            ],
        ),
        (
            "nothing",
            &none,
            [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        ),
    ];
    let mut checked = 0;
    for (name, keys, expected) in &configs {
        for ((sym, state), want) in columns.iter().zip(expected) {
            assert_eq!(
                translate_key(&press(*sym, *state), keys),
                *want,
                "highlight_keys = {name}: {sym:#06x} with {state:#x}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 24, "the whole matrix must have run");
}

#[test]
fn test_translate_key_prefers_the_highlight_binding_over_the_page_binding() {
    // A document that names one key in both lists is reported by the configuration layer and
    // dropped from `flip_keys` there, so a projection never produces this table. The router
    // still has to answer the same way the report says it will, because the precedence is
    // the run-time backstop for a table that reached it by another route.
    let both = KeyBindings {
        flip_keys: FlipSet::UP | FlipSet::DOWN,
        highlight_keys: HighlightSet::UP | HighlightSet::DOWN,
        ..KeyBindings::default()
    };
    assert_rows(
        &[
            (press(KEY_UP, 0), KeyAction::MoveHighlight(-1)),
            (press(KEY_DOWN, 0), KeyAction::MoveHighlight(1)),
        ],
        &both,
    );
}

#[test]
fn test_translate_key_pages_with_every_flip_key_the_whitelist_names() {
    // The six names `keys.flip_keys` accepts, all bound at once. The four-field struct the
    // set replaced could carry four of them, so `page_up` and `page_down` were accepted by
    // the configuration and then read by no row at all.
    let all = KeyBindings {
        flip_keys: FlipSet::all(),
        ..KeyBindings::default()
    };
    assert_rows(
        &[
            (press(KEY_MINUS, 0), KeyAction::PagePrev),
            (press(KEY_EQUAL, 0), KeyAction::PageNext),
            (press(KEY_UP, 0), KeyAction::PagePrev),
            (press(KEY_DOWN, 0), KeyAction::PageNext),
            (press(KEY_PAGE_UP, 0), KeyAction::PagePrev),
            (press(KEY_PAGE_DOWN, 0), KeyAction::PageNext),
        ],
        &all,
    );
    // Binding them all does not make them shift-tolerant: the page rows are bare presses.
    assert_rows(
        &[
            (press(KEY_MINUS, SHIFT), KeyAction::Ignore),
            (press(KEY_PAGE_UP, SHIFT), KeyAction::Ignore),
        ],
        &all,
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
fn test_translate_key_has_no_row_for_a_modifier_press() {
    // A modifier's own press is not a chord and not a row. Treating it as one is what made
    // every capital letter toggle the input mode: the press was translated before the
    // letter that followed it, so a user typing `Nihao` switched modes twice on the way.
    let keys = KeyBindings::default();
    assert_rows(
        &[
            (press(KEY_SHIFT_L, SHIFT), KeyAction::Ignore),
            (press(KEY_SHIFT_R, SHIFT), KeyAction::Ignore),
            // The key is the same with nothing held; no keyboard produces that either.
            (press(KEY_SHIFT_L, 0), KeyAction::Ignore),
        ],
        &keys,
    );
    // The predicate the routing layer uses to recognise the key still names it, because the
    // hold machine has to watch the release that ends the gesture.
    assert!(is_shift_press(&press(KEY_SHIFT_L, SHIFT)));
}

#[test]
fn test_translate_key_matches_every_chord_it_declares() {
    // Written out rather than read from `CHORDS`, so that a chord added to the table has to
    // be added here too: the table is data, and a data edit nobody exercised is how a key
    // silently changes meaning.
    let rows = [
        (KEY_SPACE, CTRL, KeyAction::ToggleLang),
        (KEY_SPACE, SHIFT, KeyAction::ToggleFullWidth),
        (KEY_PERIOD, CTRL, KeyAction::TogglePunct),
        (KEY_E, CTRL | SHIFT, KeyAction::EnterTempEnglish),
    ];
    let keys = KeyBindings::default();
    for (sym, mask, expected) in rows {
        assert_eq!(
            translate_key(&press(sym, mask), &keys),
            expected,
            "{sym:#06x} with {mask:#x}"
        );
    }
    assert_eq!(rows.len(), CHORDS.len(), "every chord has a case here");
}

#[test]
fn test_translate_key_requires_a_chords_whole_modifier_set() {
    // One modifier more than a chord declares is a different key, and the desktop
    // environment's rather than the plugin's. `Ctrl+Shift+Space`, `Ctrl+Alt+Space` and
    // `Ctrl+Shift+.` all have to reach the application.
    let keys = KeyBindings::default();
    let chords = [
        (KEY_SPACE, CTRL),
        (KEY_SPACE, SHIFT),
        (KEY_PERIOD, CTRL),
        (KEY_E, CTRL | SHIFT),
    ];
    for (sym, mask) in chords {
        for extra in [SHIFT, CTRL, ALT, SUPER] {
            if (mask & extra) != 0 {
                continue;
            }
            let state = mask | extra;
            assert_eq!(
                translate_key(&press(sym, state), &keys),
                KeyAction::Ignore,
                "{sym:#06x} with {state:#x} is not the chord"
            );
        }
    }
}

#[test]
fn test_translate_key_enters_temporary_english_on_ctrl_shift_e() {
    let keys = KeyBindings::default();
    assert_rows(
        &[
            (press(KEY_E, CTRL | SHIFT), KeyAction::EnterTempEnglish),
            // A host that folds the case into the symbol delivers the chord with the
            // uppercase shape; the fold turns it back before the table is read, so the
            // chord's row names the lowercase keysym alone.
            (press(KEY_E_UPPER, CTRL | SHIFT), KeyAction::EnterTempEnglish),
            // Either modifier alone is not the chord.
            (press(KEY_E, CTRL), KeyAction::Ignore),
            (press(KEY_E, SHIFT), KeyAction::InputChar('e')),
        ],
        &keys,
    );
}

#[test]
fn test_translate_key_folds_the_uppercase_shape_of_every_letter() {
    // The two shapes a frontend may deliver for `Shift+<letter>` have to produce the same
    // action, or the plugin's behaviour would depend on which frontend is in use.
    let keys = KeyBindings::default();
    for offset in 0..26u32 {
        let lower = KEY_A + offset;
        let upper = KEY_A_UPPER + offset;
        let expected = KeyAction::InputChar(char::from(lower as u8));
        assert_eq!(
            translate_key(&press(lower, SHIFT), &keys),
            expected,
            "the lowercase shape of {lower:#06x}"
        );
        assert_eq!(
            translate_key(&press(upper, SHIFT), &keys),
            expected,
            "the folded shape of {upper:#06x}"
        );
    }
    // A bare uppercase symbol is not a key any keyboard produces, so it stays with the host
    // rather than being read as its lowercase letter.
    for offset in 0..26u32 {
        let upper = KEY_A_UPPER + offset;
        assert_eq!(
            translate_key(&press(upper, 0), &keys),
            KeyAction::Ignore,
            "{upper:#06x} with nothing held is nobody's key"
        );
    }
}

#[test]
fn test_fold_shifted_letter_folds_only_a_shifted_uppercase_letter() {
    assert_eq!(fold_shifted_letter(KEY_A_UPPER, SHIFT), KEY_A);
    assert_eq!(fold_shifted_letter(KEY_Z_UPPER, SHIFT), KEY_Z);
    assert_eq!(fold_shifted_letter(KEY_A_UPPER, CTRL | SHIFT), KEY_A);
    // Without Shift there is nothing to fold.
    assert_eq!(fold_shifted_letter(KEY_A_UPPER, 0), KEY_A_UPPER);
    assert_eq!(fold_shifted_letter(KEY_Z_UPPER, CTRL), KEY_Z_UPPER);
    // The keysyms one step outside the range are not letters, and the fold is a no-op on
    // them whatever the modifiers say.
    assert_eq!(fold_shifted_letter(KEY_A_UPPER - 1, SHIFT), KEY_A_UPPER - 1);
    assert_eq!(fold_shifted_letter(KEY_Z_UPPER + 1, SHIFT), KEY_Z_UPPER + 1);
    // A lowercase keysym is already folded; the fold must not move it.
    assert_eq!(fold_shifted_letter(KEY_A, SHIFT), KEY_A);
}

#[test]
fn test_translate_key_routes_the_syllable_separator() {
    let keys = KeyBindings::default();
    assert_rows(
        &[
            (press(KEY_APOSTROPHE, 0), KeyAction::InputChar('\'')),
            // Every modifier makes it somebody else's key, Shift included: `"` is a
            // different keysym and this row does not cover it.
            (press(KEY_APOSTROPHE, SHIFT), KeyAction::Ignore),
            (press(KEY_APOSTROPHE, CTRL), KeyAction::Ignore),
        ],
        &keys,
    );
    // The action is the one the layers that hold a session have to guard, and no other
    // character needs the guard: a letter is what starts a composition with nothing
    // composing.
    assert!(is_syllable_separator(KeyAction::InputChar('\'')));
    assert!(!is_syllable_separator(KeyAction::InputChar('a')));
    assert!(!is_syllable_separator(KeyAction::Ignore));
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
                !claims_key(action) || is_routed(sym, state),
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
