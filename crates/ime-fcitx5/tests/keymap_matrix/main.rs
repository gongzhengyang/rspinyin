//! The keymap matrix: every row of the shortcut table, driven end to end.
//!
//! # What this file is for
//!
//! `docs/dev/features.md` 3.5 is the shortcut table the product promises, and until this
//! file existed nothing drove it end to end. The engine's own unit tests translate keys
//! one at a time, and the declarative harness under `xtask/src/testd` injects
//! `KeyAction`s directly — so a green run there proved the session state machine and said
//! nothing about whether a keystroke can reach it. This file closes that gap: one table
//! of keysyms and modifier masks, driven through the routing table, the layered bus, the
//! session and the host boundary, with the verdict asserted per row.
//!
//! # The three questions a row answers
//!
//! 1. **What does the key mean?** [`translate_key`] is the routing table's whole answer,
//!    and the table below states it for every row.
//! 2. **Does the plugin keep it?** The answer depends on the session context, which is
//!    why every row is checked in all three of [`Situation`]: nothing composing, a
//!    composition in flight, and temporary English. The same `Space` is the plugin's
//!    while a candidate is highlighted and the application's otherwise; a key kept with
//!    nothing to act on is the defect this project calls a swallowed key.
//! 3. **Do the two paths agree?** The layered bus arbitrates a key from the table *and*
//!    from what the session would do with it. The bus must claim a key exactly when the
//!    table names it and the session would act on it — never on the table's answer alone.
//!
//! # Determinism
//!
//! No case reads a file, a clock, an environment variable or a display server. The
//! dictionary, the user frequencies and the language model are in-memory values; the
//! never-swallow corpus is drawn from a seeded generator rather than a random-number
//! crate, so the same 200 keysyms are checked on every run and in every process.
//!
//! # Rows whose contract is a gesture rather than a translation
//!
//! The `Shift` keys are the one place where the table's own answer is deliberately not
//! the contract. `features.md` 3.5 gives the *held* modifier the temporary Chinese /
//! English switch, which is engine state rather than a `KeyAction`; what a case can
//! assert is that the plugin never takes the press and never switches the mode. The two
//! rows for the modifier's own press therefore carry no expected action, and the gesture
//! is asserted by [`test_holding_shift_never_switches_the_input_mode`] instead.

// The keyboard-only end-to-end walks, one per row of the scenario table; the module
// itself holds the id-to-test mapping and the walks' shared strokes.
mod scenarios;

mod support;

use std::collections::BTreeSet;

use ime_types::KeyAction;
use rspinyin::engine::{
    Consumed, DigitZero, Dispatcher, FlipSet, HighlightSet, KeyBindings, KeyEvent, SessionView,
    arbitrate, is_mode_chord, is_shift_press, translate_key,
};

use support::{
    ALT, CTRL, DESKTOP_MODIFIERS, Fixture, IC, KEY_0, KEY_1, KEY_9, KEY_A, KEY_A_UPPER,
    KEY_APOSTROPHE, KEY_AT, KEY_BACKSPACE, KEY_BRACKET_LEFT, KEY_DELETE, KEY_DOWN, KEY_E, KEY_END,
    KEY_EQUAL, KEY_ESCAPE, KEY_HOME, KEY_I, KEY_LEFT, KEY_MINUS, KEY_N, KEY_P, KEY_PAGE_DOWN,
    KEY_PAGE_UP, KEY_PERIOD, KEY_RETURN, KEY_RIGHT, KEY_SHIFT_L, KEY_SHIFT_R, KEY_SLASH, KEY_SPACE,
    KEY_TAB, KEY_UP, KEY_Z, KEY_Z_UPPER, Lcg, RecordingHost, SHIFT, Situation, press, release,
};

// ── The matrix ───────────────────────────────────────────────────────────────────

/// What the plugin must do with one key in one session context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The plugin acted on the key: the application never sees it.
    Kept,
    /// The key belongs to the application.
    Passed,
}

/// The verdict a `key_event` answer stands for.
fn verdict_of(kept: bool) -> Verdict {
    if kept { Verdict::Kept } else { Verdict::Passed }
}

/// One row of the shortcut matrix.
struct Row {
    /// The name a failure is reported under.
    label: &'static str,
    /// XKB keysym of the key.
    sym: u32,
    /// The modifier mask held while it is pressed.
    state: u32,
    /// The `[keys]` settings the row is routed under, or `None` for the shipped default.
    keys: Option<KeyBindings>,
    /// What the routing table must make of the key.
    ///
    /// `None` for a row whose contract is the gesture rather than the translation: the
    /// modifier's own press, which the plugin hands back whatever the table says.
    action: Option<KeyAction>,
    /// What the plugin must do with the key while a composition is live.
    composing: Verdict,
    /// What the plugin must do with the key with nothing composing.
    idle: Verdict,
}

impl Row {
    /// The `[keys]` settings this row is routed under.
    fn bindings(&self) -> KeyBindings {
        self.keys.unwrap_or_default()
    }

    /// What the plugin must do with this row's key in `situation`.
    ///
    /// Temporary English is not a column: it hands every key to the application, the two
    /// keys that leave the mode included, because what ends the mode changes no other
    /// state and taking the key would take a keystroke the user typed for the
    /// application.
    fn verdict(&self, situation: Situation) -> Verdict {
        match situation {
            Situation::Idle => self.idle,
            Situation::Composing => self.composing,
            Situation::TempEnglish => Verdict::Passed,
        }
    }
}

/// The shortcut matrix, one row per line of `features.md` 3.5.
///
/// The composing column assumes the fixture's dictionary: a composition on `ni` whose
/// readings fill more than one page. That is what gives the page keys somewhere to go,
/// `1` a candidate to name and `9` none, and `Tab` a neighbour to move to — a dictionary
/// with a single reading would make most of the composing column vacuous.
fn rows() -> Vec<Row> {
    let shipped = KeyBindings::default();
    let zero_flips = KeyBindings {
        digit_zero: DigitZero::Flip,
        ..shipped
    };
    let enter_is_raw = KeyBindings {
        enter_commit_raw: true,
        ..shipped
    };
    vec![
        // `a-z`: the input, in either shape the host may deliver it in.
        Row {
            label: "letter a",
            sym: KEY_A,
            state: 0,
            keys: None,
            action: Some(KeyAction::InputChar('a')),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "letter z",
            sym: KEY_Z,
            state: 0,
            keys: None,
            action: Some(KeyAction::InputChar('z')),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "letter a with Shift held",
            sym: KEY_A,
            state: SHIFT,
            keys: None,
            action: Some(KeyAction::InputChar('a')),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "uppercase A with Shift held",
            sym: KEY_A_UPPER,
            state: SHIFT,
            keys: None,
            action: Some(KeyAction::InputChar('a')),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "uppercase A with no modifier",
            sym: KEY_A_UPPER,
            state: 0,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        // `Space`, `1`~`9`, `0`.
        Row {
            label: "Space",
            sym: KEY_SPACE,
            state: 0,
            keys: None,
            action: Some(KeyAction::CommitHighlighted),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "digit 1",
            sym: KEY_1,
            state: 0,
            keys: None,
            action: Some(KeyAction::SelectIndex(1)),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "digit 9",
            sym: KEY_9,
            state: 0,
            keys: None,
            action: Some(KeyAction::SelectIndex(9)),
            // The page holds five candidates, so `9` names nothing and the key travels on.
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "digit 0 (passthrough)",
            sym: KEY_0,
            state: 0,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "digit 0 (flip)",
            sym: KEY_0,
            state: 0,
            keys: Some(zero_flips),
            action: Some(KeyAction::PageNext),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        // The page keys.
        Row {
            label: "minus",
            sym: KEY_MINUS,
            state: 0,
            keys: None,
            action: Some(KeyAction::PagePrev),
            // The first page is the one on show, so there is nothing behind it.
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "equal",
            sym: KEY_EQUAL,
            state: 0,
            keys: None,
            action: Some(KeyAction::PageNext),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Up",
            sym: KEY_UP,
            state: 0,
            keys: None,
            action: Some(KeyAction::PagePrev),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "Down",
            sym: KEY_DOWN,
            state: 0,
            keys: None,
            action: Some(KeyAction::PageNext),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Page_Up",
            sym: KEY_PAGE_UP,
            state: 0,
            keys: None,
            // The shipped configuration does not name it, so the key is the host's.
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "Page_Down",
            sym: KEY_PAGE_DOWN,
            state: 0,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        // The highlight keys.
        Row {
            label: "Tab",
            sym: KEY_TAB,
            state: 0,
            keys: None,
            action: Some(KeyAction::MoveHighlight(1)),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Shift+Tab",
            sym: KEY_TAB,
            state: SHIFT,
            keys: None,
            action: Some(KeyAction::MoveHighlight(-1)),
            // The highlight starts on the first candidate, so there is nowhere to go back.
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        // The caret keys.
        Row {
            label: "Left",
            sym: KEY_LEFT,
            state: 0,
            keys: None,
            action: Some(KeyAction::MoveCaret(-1)),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Right",
            sym: KEY_RIGHT,
            state: 0,
            keys: None,
            action: Some(KeyAction::MoveCaret(1)),
            // The caret sits at the end of the input, so it cannot move further right.
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        // The editing keys.
        Row {
            label: "Return",
            sym: KEY_RETURN,
            state: 0,
            keys: None,
            action: Some(KeyAction::CommitHighlighted),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Return with enter_commit_raw",
            sym: KEY_RETURN,
            state: 0,
            keys: Some(enter_is_raw),
            action: Some(KeyAction::CommitRaw),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Escape",
            sym: KEY_ESCAPE,
            state: 0,
            keys: None,
            action: Some(KeyAction::Escape),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        Row {
            label: "Backspace",
            sym: KEY_BACKSPACE,
            state: 0,
            keys: None,
            action: Some(KeyAction::Backspace),
            composing: Verdict::Kept,
            idle: Verdict::Passed,
        },
        // The syllable separator, which is an input character and not a key of its own.
        Row {
            label: "apostrophe",
            sym: KEY_APOSTROPHE,
            state: 0,
            keys: None,
            action: Some(KeyAction::InputChar('\'')),
            composing: Verdict::Kept,
            // With nothing composing an apostrophe is an apostrophe: it must not open a
            // candidate window, and the application must receive it.
            idle: Verdict::Passed,
        },
        // The held modifier. Its own press is not a translation the plugin acts on.
        Row {
            label: "Shift_L press",
            sym: KEY_SHIFT_L,
            state: SHIFT,
            keys: None,
            action: None,
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "Shift_R press",
            sym: KEY_SHIFT_R,
            state: SHIFT,
            keys: None,
            action: None,
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        // The global mode chords. `Ctrl+Space` is not one of them any more: the language
        // switch is the host's own hotkey, the chord table claims no chord for it, and
        // the key reaches the application whatever is composing.
        Row {
            label: "Ctrl+Space",
            sym: KEY_SPACE,
            state: CTRL,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "Shift+Space",
            sym: KEY_SPACE,
            state: SHIFT,
            keys: None,
            action: Some(KeyAction::ToggleFullWidth),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "Ctrl+period",
            sym: KEY_PERIOD,
            state: CTRL,
            keys: None,
            action: Some(KeyAction::TogglePunct),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        Row {
            label: "Ctrl+Shift+E",
            sym: KEY_E,
            state: CTRL | SHIFT,
            keys: None,
            action: Some(KeyAction::EnterTempEnglish),
            composing: Verdict::Kept,
            idle: Verdict::Kept,
        },
        // The two Phase 2 panel entries. Phase 1 reserves the keys and implements no
        // panel, so what the routing table does today is leave them to the application.
        Row {
            label: "Ctrl+Shift+slash",
            sym: KEY_SLASH,
            state: CTRL | SHIFT,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
        Row {
            label: "Ctrl+Shift+P",
            sym: KEY_P,
            state: CTRL | SHIFT,
            keys: None,
            action: Some(KeyAction::Ignore),
            composing: Verdict::Passed,
            idle: Verdict::Passed,
        },
    ]
}

/// One chord with one modifier too many.
///
/// A chord is matched on the exact modifier set, so `Ctrl+Space` and `Ctrl+Shift+Space`
/// are different keys. A chord that accepted a superset would take keys the desktop
/// environment owns, and the user would lose a system shortcut to an input method that
/// then did nothing with it.
const OVER_SPECIFIED: &[(&str, u32, u32)] = &[
    ("Ctrl+Shift+Space", KEY_SPACE, CTRL | SHIFT),
    ("Ctrl+Alt+Space", KEY_SPACE, CTRL | ALT),
    ("Alt+Space", KEY_SPACE, ALT),
    ("Ctrl+Shift+period", KEY_PERIOD, CTRL | SHIFT),
    ("Ctrl+E", KEY_E, CTRL),
    ("Ctrl+Shift+Alt+E", KEY_E, CTRL | SHIFT | ALT),
    ("Ctrl+Tab", KEY_TAB, CTRL),
    ("Ctrl+Shift+Tab", KEY_TAB, CTRL | SHIFT),
    ("Ctrl+a", KEY_A, CTRL),
    ("a desktop chord on Space", KEY_SPACE, DESKTOP_MODIFIERS),
    ("a desktop chord on a letter", KEY_A, DESKTOP_MODIFIERS),
];

/// The keysyms the never-swallow corpus is seeded with.
const ROUTED: &[u32] = &[
    KEY_SPACE,
    KEY_APOSTROPHE,
    KEY_MINUS,
    KEY_PERIOD,
    KEY_SLASH,
    KEY_0,
    KEY_1,
    KEY_9,
    KEY_EQUAL,
    KEY_AT,
    KEY_A_UPPER,
    KEY_BRACKET_LEFT,
    KEY_A,
    KEY_E,
    KEY_P,
    KEY_Z,
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
    KEY_SHIFT_L,
    KEY_SHIFT_R,
];

/// How many keysyms the never-swallow corpus holds.
///
/// Seeded with every keysym the routing table names, so the check covers the keys a key
/// *can* be swallowed for, and filled up with pseudo-random values so it also covers the
/// keys that must simply travel on.
const CORPUS_KEYSYMS: usize = 200;

/// The modifier sets the corpus is drawn over.
///
/// A bare press, Shift, Control, and the set a desktop environment owns. The last one is
/// the boundary that matters most: whatever else it holds, a key carrying Super, Hyper,
/// Super2 or Meta belongs to the window manager.
const CORPUS_STATES: [u32; 4] = [0, SHIFT, CTRL, DESKTOP_MODIFIERS];

// ── The cases ────────────────────────────────────────────────────────────────────

#[test]
fn test_the_shortcut_table_translates_every_row_as_it_says() {
    for row in rows() {
        let Some(expected) = row.action else {
            continue;
        };
        let label = row.label;
        let seen = translate_key(&press(row.sym, row.state), &row.bindings());
        assert_eq!(seen, expected, "{label}");
    }
}

#[test]
fn test_the_matrix_reaches_every_action_the_routing_table_can_produce() {
    let mut reached = BTreeSet::new();
    for row in rows() {
        if let Some(action) = row.action {
            reached.insert(action_name(action));
        }
    }
    let expected: BTreeSet<&str> = ROUTABLE_ACTIONS.iter().copied().collect();
    assert_eq!(
        reached, expected,
        "the matrix must name a key for every action the table can reach"
    );
}

#[test]
fn test_the_router_keeps_exactly_the_keys_the_matrix_names() {
    let fixture = Fixture::default();
    for row in rows() {
        let keys = row.bindings();
        let label = row.label;
        for situation in [
            Situation::Idle,
            Situation::Composing,
            Situation::TempEnglish,
        ] {
            let mut host = RecordingHost::default();
            let mut router = fixture.router_in(keys, situation, &mut host);
            if situation == Situation::Composing {
                // The matrix's composing column is stated in terms of what the setup
                // leaves behind, so the setup is checked first: a fixture that offered one
                // page or one candidate would make half of that column vacuous.
                let pages = host.newest_page.map_or(0, |page| page.total);
                let preedit = host.newest_preedit.as_deref().unwrap_or("<no frame>");
                assert!(
                    pages >= 2 && host.newest_candidates >= 2,
                    "the composing fixture must offer a second page and a highlight to \
                     move, the setup's frame reports {pages} page(s) and {} candidates",
                    host.newest_candidates
                );
                assert!(
                    preedit.contains("ni"),
                    "the composing setup must have typed `ni`, the frame's preedit reads \
                     {preedit:?}"
                );
            }
            let event = press(row.sym, row.state);
            let before = host.calls;
            let kept = router.key_event(IC, &event, &mut host);
            assert_eq!(
                verdict_of(kept),
                row.verdict(situation),
                "{label} ({:#06x} with {:#x}) in {situation:?}",
                row.sym,
                row.state
            );
            if kept {
                let action = translate_key(&event, &keys);
                assert_ne!(
                    action,
                    KeyAction::Ignore,
                    "{label} was kept although the table does not name the key"
                );
                assert!(
                    host.calls > before || is_engine_action(action),
                    "{label} was kept and nothing happened: {action:?} reached neither the \
                     host nor a state the engine owns"
                );
            }
        }
    }
}

#[test]
fn test_the_bus_claims_a_key_exactly_when_the_table_and_the_session_agree() {
    let fixture = Fixture::default();
    let cfg = fixture.config.session;
    for row in rows() {
        let keys = row.bindings();
        let label = row.label;
        let event = press(row.sym, row.state);
        for situation in [
            Situation::Idle,
            Situation::Composing,
            Situation::TempEnglish,
        ] {
            let session = fixture.session_in(situation);
            let mut dispatcher = Dispatcher::new(keys);
            dispatcher.set_session_config(cfg);
            let claimed = dispatcher.dispatch(&KeyEvent::from(event), &SessionView::new(&session));
            if is_shift_press(&event) {
                // A modifier press is not a layer's key: the bus hands it back before the
                // walk, because keeping it would take the first half of every capital
                // letter from the application.
                assert_eq!(
                    claimed,
                    Consumed::Ignored,
                    "{label} must reach the application in {situation:?}"
                );
                continue;
            }
            if is_panel_chord(row.sym, row.state) && situation != Situation::TempEnglish {
                // The panel chords are not table rows and never become a `KeyAction`, so
                // the arbitrator is not the reference answer for them: the bus answers
                // them ahead of the walk wherever the plugin may open the panel its chord
                // names, and temporary English hands them back with every other key.
                assert_eq!(
                    claimed,
                    Consumed::Consumed,
                    "{label} must open its panel in {situation:?}"
                );
                continue;
            }
            let arbitrated = arbitrate(translate_key(&event, &keys), Some(&session), &cfg);
            assert_eq!(
                claimed, arbitrated,
                "the bus and the arbitrator disagree about {label} in {situation:?}"
            );
            if claimed != Consumed::Ignored {
                assert_ne!(
                    translate_key(&event, &keys),
                    KeyAction::Ignore,
                    "the bus kept {label} although the table does not name the key"
                );
            }
        }
    }
}

/// Whether the key is one of the two panel chords the bus answers ahead of the walk.
///
/// The modifier set is compared for equality, exactly as the engine compares it: a chord
/// is `Ctrl+Shift` and nothing else, so `Ctrl+Shift+Alt+/` is a different key that
/// belongs to the desktop environment. The uppercase shapes are spelled as literals
/// because the host folds `Shift` into the symbol the way it folds it into a letter.
fn is_panel_chord(sym: u32, state: u32) -> bool {
    state == (CTRL | SHIFT) && matches!(sym, KEY_SLASH | 0x003f | KEY_P | 0x0050)
}

#[test]
fn test_no_keystroke_is_swallowed() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(keys);
    let mut corpus: Vec<u32> = ROUTED.to_vec();
    let mut lcg = Lcg::new(0x5eed_1234_abcd_0001);
    while corpus.len() < CORPUS_KEYSYMS {
        corpus.push(lcg.next_u32() & 0xffff);
    }

    let mut kept_count = 0;
    for sym in corpus {
        for state in CORPUS_STATES {
            let event = press(sym, state);
            let before = host.calls;
            let kept = router.key_event(IC, &event, &mut host);
            let action = translate_key(&event, &keys);
            if is_shift_press(&event) {
                assert!(
                    !kept,
                    "the modifier press {sym:#06x} with {state:#x} must reach the application"
                );
                continue;
            }
            if !kept {
                continue;
            }
            kept_count += 1;
            assert_ne!(
                action,
                KeyAction::Ignore,
                "{sym:#06x} with {state:#x} was kept although the table does not name the key"
            );
            assert!(
                host.calls > before || is_engine_action(action),
                "{sym:#06x} with {state:#x} was kept and nothing happened: {action:?} reached \
                 neither the host nor a state the engine owns"
            );
        }
    }
    assert!(
        kept_count > 0,
        "the corpus kept nothing at all, so it proves nothing about the keys the plugin takes"
    );
}

#[test]
fn test_every_key_release_is_handed_back() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(keys);
    for sym in ROUTED {
        for state in CORPUS_STATES {
            let event = release(*sym, state);
            assert!(
                !router.key_event(IC, &event, &mut host),
                "the release of {sym:#06x} with {state:#x} must reach the application"
            );
            assert_eq!(
                translate_key(&event, &keys),
                KeyAction::Ignore,
                "the table must name no release"
            );
        }
    }
    assert_eq!(
        host.calls, 0,
        "a release must reach the host untouched, the application's key-up included"
    );
    assert!(
        host.diagnostics.is_empty(),
        "a release reports nothing: {:?}",
        host.diagnostics
    );
}

#[test]
fn test_the_letter_rows_do_not_depend_on_the_shape_the_host_delivers() {
    let keys = KeyBindings::default();
    for offset in 0..26u32 {
        let lower = KEY_A + offset;
        let upper = KEY_A_UPPER + offset;
        // The letter row is ASCII, so the low byte of the keysym is the character.
        let letter = char::from(lower as u8);
        let expected = KeyAction::InputChar(letter);
        assert_eq!(
            translate_key(&press(lower, 0), &keys),
            expected,
            "the lowercase shape of {letter:?}"
        );
        assert_eq!(
            translate_key(&press(lower, SHIFT), &keys),
            expected,
            "{letter:?} with Shift held, in its lowercase shape"
        );
        assert_eq!(
            translate_key(&press(upper, SHIFT), &keys),
            expected,
            "{letter:?} with Shift held, in the uppercase shape the host may fold in"
        );
        assert_eq!(
            translate_key(&press(upper, 0), &keys),
            KeyAction::Ignore,
            "a bare uppercase symbol is not a key any keyboard produces"
        );
    }
    // The two keysyms on either side of the letter row, so the fold cannot be off by one.
    for sym in [KEY_AT, KEY_BRACKET_LEFT] {
        assert_eq!(
            translate_key(&press(sym, SHIFT), &keys),
            KeyAction::Ignore,
            "{sym:#06x} sits outside the letter row and must not be folded into it"
        );
    }
    assert_eq!(KEY_A_UPPER + 25, KEY_Z_UPPER);
}

#[test]
fn test_holding_shift_never_switches_the_input_mode() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(keys);
    assert!(
        !router.key_event(IC, &press(KEY_SHIFT_L, SHIFT), &mut host),
        "a modifier press is the application's"
    );
    for offset in 0..26u32 {
        router.key_event(IC, &press(KEY_A_UPPER + offset, SHIFT), &mut host);
    }
    assert!(
        !router.key_event(IC, &release(KEY_SHIFT_L, SHIFT), &mut host),
        "a modifier release is the application's"
    );
    assert_eq!(
        host.toggles, 0,
        "holding Shift is how a capital letter is typed, not how the mode is switched"
    );
}

#[test]
fn test_temporary_english_hands_every_key_back() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    for row in rows() {
        let mut host = RecordingHost::default();
        let mut router = fixture.router_in(keys, Situation::TempEnglish, &mut host);
        assert!(
            !router.key_event(IC, &press(row.sym, row.state), &mut host),
            "{} must reach the application in temporary English",
            row.label
        );
    }
}

#[test]
fn test_space_does_not_leave_temporary_english() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router_in(keys, Situation::TempEnglish, &mut host);
    assert!(
        !router.key_event(IC, &press(KEY_SPACE, 0), &mut host),
        "the space bar is a character the mode exists to pass through"
    );
    // The observable difference: with the mode still on a letter passes through, and with
    // it off the letter would start a composition and be kept.
    assert!(
        !router.key_event(IC, &press(KEY_A, 0), &mut host),
        "the mode is still on after Space"
    );
    assert!(
        host.commits.is_empty(),
        "temporary English commits nothing: {:?}",
        host.commits
    );
}

#[test]
fn test_every_chord_falls_back_to_the_host_with_one_modifier_too_many() {
    let fixture = Fixture::default();
    let keys = KeyBindings::default();
    for &(label, sym, state) in OVER_SPECIFIED {
        assert_eq!(
            translate_key(&press(sym, state), &keys),
            KeyAction::Ignore,
            "{label} is nobody's chord"
        );
        let mut host = RecordingHost::default();
        let mut router = fixture.router_in(keys, Situation::Composing, &mut host);
        assert!(
            !router.key_event(IC, &press(sym, state), &mut host),
            "{label} must reach the application while a composition is live"
        );
    }
}

#[test]
fn test_every_pageable_key_pages_only_when_the_configuration_names_it() {
    let mut all = FlipSet::empty();
    for (_, _, bit, _) in PAGEABLE {
        all |= bit;
    }
    for (label, sym, bit, expected) in PAGEABLE {
        let named = KeyBindings {
            flip_keys: all,
            highlight_keys: HighlightSet::empty(),
            ..KeyBindings::default()
        };
        assert_eq!(
            translate_key(&press(sym, 0), &named),
            expected,
            "{label} pages when the configuration names it"
        );
        let dropped = KeyBindings {
            flip_keys: all & !bit,
            highlight_keys: HighlightSet::empty(),
            ..KeyBindings::default()
        };
        assert_eq!(
            translate_key(&press(sym, 0), &dropped),
            KeyAction::Ignore,
            "{label} is the host's when the configuration does not name it"
        );
    }
}

#[test]
fn test_the_highlight_bindings_decide_which_keys_move_the_highlight() {
    let paging = FlipSet::MINUS | FlipSet::EQUAL | FlipSet::UP | FlipSet::DOWN;
    let cases = [
        HighlightCase {
            label: "the shipped default",
            highlight: HighlightSet::TAB | HighlightSet::SHIFT_TAB,
            expect: [
                KeyAction::MoveHighlight(1),
                KeyAction::MoveHighlight(-1),
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        },
        HighlightCase {
            label: "the arrows move the highlight",
            highlight: HighlightSet::UP | HighlightSet::DOWN,
            expect: [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::MoveHighlight(-1),
                KeyAction::MoveHighlight(1),
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        },
        HighlightCase {
            label: "the horizontal arrows move the highlight",
            highlight: HighlightSet::LEFT | HighlightSet::RIGHT,
            expect: [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveHighlight(-1),
                KeyAction::MoveHighlight(1),
            ],
        },
        HighlightCase {
            label: "no key moves the highlight",
            highlight: HighlightSet::empty(),
            expect: [
                KeyAction::Ignore,
                KeyAction::Ignore,
                KeyAction::PagePrev,
                KeyAction::PageNext,
                KeyAction::MoveCaret(-1),
                KeyAction::MoveCaret(1),
            ],
        },
    ];
    for case in &cases {
        let keys = KeyBindings {
            flip_keys: paging,
            highlight_keys: case.highlight,
            ..KeyBindings::default()
        };
        for (index, &(sym, state)) in HIGHLIGHT_KEYS.iter().enumerate() {
            assert_eq!(
                translate_key(&press(sym, state), &keys),
                case.expect[index],
                "{}: key {index} of the highlight row",
                case.label
            );
        }
    }
}

// ── The tables the cases above read ──────────────────────────────────────────────

/// One `keys.highlight_keys` configuration and what each shared key must do.
struct HighlightCase {
    /// The name a failure is reported under.
    label: &'static str,
    /// The keys that move the highlight.
    highlight: HighlightSet,
    /// The action each of [`HIGHLIGHT_KEYS`] must be translated to, in that order.
    expect: [KeyAction; 6],
}

/// The keys the highlight list and the page list share, in the order the cases list them.
const HIGHLIGHT_KEYS: [(u32, u32); 6] = [
    (KEY_TAB, 0),
    (KEY_TAB, SHIFT),
    (KEY_UP, 0),
    (KEY_DOWN, 0),
    (KEY_LEFT, 0),
    (KEY_RIGHT, 0),
];

/// The pageable keys, the flag each carries, and the action it stands for.
const PAGEABLE: [(&str, u32, FlipSet, KeyAction); 6] = [
    ("minus", KEY_MINUS, FlipSet::MINUS, KeyAction::PagePrev),
    ("equal", KEY_EQUAL, FlipSet::EQUAL, KeyAction::PageNext),
    ("Up", KEY_UP, FlipSet::UP, KeyAction::PagePrev),
    ("Down", KEY_DOWN, FlipSet::DOWN, KeyAction::PageNext),
    (
        "Page_Up",
        KEY_PAGE_UP,
        FlipSet::PAGE_UP,
        KeyAction::PagePrev,
    ),
    (
        "Page_Down",
        KEY_PAGE_DOWN,
        FlipSet::PAGE_DOWN,
        KeyAction::PageNext,
    ),
];

/// The actions the routing table can reach.
///
/// The frozen enum carries nineteen variants; five of them are named by no key. Four were
/// appended by the script, forget, pin and phrase work; the fifth is the language switch,
/// whose chord the host owns — the plugin that took the key could only have swallowed it,
/// because Fcitx5's headers expose no state for the engine to switch. The list is written
/// out rather than derived, because a variant that gained a key without anyone adding a
/// row here is exactly the drift the coverage case exists to catch.
const ROUTABLE_ACTIONS: [&str; 14] = [
    "input-char",
    "backspace",
    "commit-highlighted",
    "commit-raw",
    "select-index",
    "page-next",
    "page-prev",
    "move-highlight",
    "move-caret",
    "toggle-full-width",
    "toggle-punct",
    "enter-temp-english",
    "escape",
    "ignore",
];

/// The name a case reports an action under.
///
/// Exhaustive over the frozen enum, so adding a variant breaks the build here rather than
/// silently leaving it out of the coverage case.
fn action_name(action: KeyAction) -> &'static str {
    match action {
        KeyAction::InputChar(_) => "input-char",
        KeyAction::Backspace => "backspace",
        KeyAction::CommitHighlighted => "commit-highlighted",
        KeyAction::CommitRaw => "commit-raw",
        KeyAction::SelectIndex(_) => "select-index",
        KeyAction::PageNext => "page-next",
        KeyAction::PagePrev => "page-prev",
        KeyAction::MoveHighlight(_) => "move-highlight",
        KeyAction::MoveCaret(_) => "move-caret",
        KeyAction::ToggleLang => "toggle-lang",
        KeyAction::ToggleFullWidth => "toggle-full-width",
        KeyAction::TogglePunct => "toggle-punct",
        KeyAction::EnterTempEnglish => "enter-temp-english",
        KeyAction::Escape => "escape",
        KeyAction::Ignore => "ignore",
        KeyAction::ToggleScript => "toggle-script",
        KeyAction::ForgetHighlighted => "forget-highlighted",
        KeyAction::PinHighlighted => "pin-highlighted",
        KeyAction::AddPhrase => "add-phrase",
    }
}

/// Whether the engine acts on this action without the session being involved.
///
/// The three mode bits and the temporary-English chord change a state the engine owns, so
/// a key kept for one of them has done something even though nothing reached the host.
fn is_engine_action(action: KeyAction) -> bool {
    is_mode_chord(action) || matches!(action, KeyAction::EnterTempEnglish)
}
