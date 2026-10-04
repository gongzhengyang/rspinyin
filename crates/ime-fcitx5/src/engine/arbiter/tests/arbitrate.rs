//! The rows of the claim table: the keys the composing keymap hands back while
//! nothing is composing, the keys a live composition keeps and hands back, and the
//! modes the engine owns outside the session.

use ime_core::state::{Session, SessionConfig};
use ime_types::KeyAction;

use crate::engine::*;

use super::{
    Fixture, KEY_3, KEY_5, KEY_Q, Setup, action_of, actions, arbitrate, composing_keymap,
    is_mode_chord, keysym_corpus, step_acted,
};

// ── The arbitration ──────────────────────────────────────────────────────────────

/// With nothing composing, every key of the composing keymap belongs to the application.
///
/// The table names all of them, so this is exactly the defect the arbitrator closes: a
/// plugin that kept them would eat the space bar, the digits, `Return`, `BackSpace`,
/// `Escape`, `Tab` and the arrows in every application while nothing is composing.
#[test]
fn test_arbitrate_idle_hands_the_composing_keymap_back() {
    let cfg = SessionConfig::default();
    let session = Session::new();
    for (sym, state) in composing_keymap() {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "idle {sym:#x} with {state:#x} must reach the application"
        );
    }
}

/// While a composition is live, the keys it can act on are the plugin's.
#[test]
fn test_arbitrate_composing_keeps_the_keys_the_session_acts_on() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    assert!(
        session.decoded().candidates.len() > 5,
        "the fixture offers a second page"
    );
    let keys = [
        (KEY_SPACE, 0),
        (KEY_RETURN, 0),
        (KEY_BACKSPACE, 0),
        (KEY_ESCAPE, 0),
        (KEY_TAB, 0),
        (KEY_LEFT, 0),
        (KEY_EQUAL, 0),
        (KEY_DOWN, 0),
        (KEY_1, 0),
        (KEY_5, 0),
    ];
    for (sym, state) in keys {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Consumed,
            "composing {sym:#x} with {state:#x} is the plugin's"
        );
    }
}

/// A routed key the session cannot act on belongs to the application.
///
/// The other half of the same rule, and the reason a composition is not simply a keyboard
/// grab: a digit past the end of the page, a page key with no page to turn to, and an arrow
/// pointing off the end of the input all do nothing, so the application receives them.
#[test]
fn test_arbitrate_composing_hands_back_the_keys_that_would_do_nothing() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    assert!(session.decoded().candidates.len() > 5, "a page of five");
    assert_eq!(session.paging.page, 0, "on the first page");
    let keys = [
        (KEY_9, 0, "a digit past the end of the page"),
        (KEY_UP, 0, "the first page has nothing before it"),
        (KEY_MINUS, 0, "and no page to turn back to"),
        (KEY_TAB, SHIFT, "the highlight is on the first candidate"),
        (KEY_RIGHT, 0, "the caret is already at the end of the input"),
    ];
    for (sym, state, why) in keys {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "{why}: {sym:#x}"
        );
    }
}

/// The key the table does not name is never arbitrated.
#[test]
fn test_arbitrate_ignores_the_action_the_table_does_not_name() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    for setup in Setup::ALL {
        let session = setup.session(&fixture);
        assert_eq!(
            arbitrate(KeyAction::Ignore, Some(&session), &cfg),
            Consumed::Ignored,
            "{} has nothing to arbitrate about an unnamed key",
            setup.label()
        );
    }
}

/// A key for an input context with no session belongs to the application.
#[test]
fn test_arbitrate_without_a_session_ignores_every_action() {
    let cfg = SessionConfig::default();
    for action in actions() {
        assert_eq!(
            arbitrate(action, None, &cfg),
            Consumed::Ignored,
            "{action:?} has nowhere to go without a session"
        );
    }
}

/// The engine's own mode chords are the plugin's with nothing composing.
///
/// The two switches the routing table still claims change bits the engine owns, so they
/// act with no session state to read; the layer that consumes them is the engine's, which
/// is why they are not asked of the session. The language switch is not among them: its
/// chord is the host's own hotkey, and the action stays in the vocabulary for the day a
/// host carries the state the switch would write.
#[test]
fn test_arbitrate_claims_the_mode_chords_with_nothing_composing() {
    let cfg = SessionConfig::default();
    let session = Session::new();
    let chords = [
        (KEY_SPACE, SHIFT, KeyAction::ToggleFullWidth),
        (KEY_PERIOD, CTRL, KeyAction::TogglePunct),
    ];
    for (sym, state, expected) in chords {
        assert_eq!(action_of(sym, state), expected, "the table names it");
        assert_eq!(
            arbitrate(expected, Some(&session), &cfg),
            Consumed::Consumed,
            "the engine owns {expected:?}"
        );
    }
}

/// Temporary English hands every key to the application, mode chords included.
///
/// The `Return` and `Escape` that leave the mode change no other state and produce no
/// effect, so keeping them would take a keystroke the user typed for the application.
#[test]
fn test_arbitrate_hands_every_key_back_in_temporary_english() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut session = fixture.composing("ni");
    session.temp_english = true;

    for action in actions() {
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "temporary English hands {action:?} to the application"
        );
    }
}

/// A session waiting for the host to finish drops every key, the engine's switches aside.
///
/// The three mode bits are the engine's rather than the session's, so they are answered
/// before the session is asked at all: a user who switches the language while a commit is
/// landing still gets the switch, and the composition the host is finishing is untouched.
#[test]
fn test_arbitrate_hands_every_key_back_while_the_host_finishes() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    for setup in [Setup::Cancelling, Setup::Committing] {
        let session = setup.session(&fixture);
        for action in actions() {
            let expected = if is_mode_chord(action) {
                Consumed::Consumed
            } else {
                Consumed::Ignored
            };
            assert_eq!(
                arbitrate(action, Some(&session), &cfg),
                expected,
                "{} with {action:?}",
                setup.label()
            );
        }
    }
}

/// A key the arbitrator keeps must be a key something acts on.
///
/// The sweep runs over every row the table reads plus a pseudo-random tail: whenever the
/// verdict is `Consumed`, stepping the session with the action it named produced a
/// non-diagnostic effect, or the action is one of the engine's own mode bits, which the
/// engine acts on outside the session. Nothing else may be swallowed.
#[test]
fn test_arbitrate_never_keeps_a_key_the_session_would_not_act_on() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut kept = 0usize;
    for sym in keysym_corpus() {
        for setup in Setup::ALL {
            let mut session = setup.session(&fixture);
            let action = action_of(sym, 0);
            let verdict = arbitrate(action, Some(&session), &cfg);
            let effects = fixture.step(&mut session, action);
            let acted = step_acted(&session, &effects, false);

            assert!(
                verdict == Consumed::Ignored || acted || is_mode_chord(action),
                "{} keeps {sym:#x} as {action:?} but nothing acts on it",
                setup.label()
            );
            kept += usize::from(verdict == Consumed::Consumed);
        }
    }
    assert!(kept > 0, "the sweep exercised the keeping path");
}

/// The claim table of the design, row by row, driven through the routing table.
#[test]
fn test_arbitrate_matches_the_claim_table() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    assert!(
        fixture.composing("hao").decoded().candidates.len() < 5,
        "the short fixture offers fewer than five readings"
    );
    let rows = [
        (KEY_SPACE, 0, Setup::Idle, Consumed::Ignored, "idle space"),
        (KEY_3, 0, Setup::Idle, Consumed::Ignored, "idle digit"),
        (KEY_RETURN, 0, Setup::Idle, Consumed::Ignored, "idle return"),
        (
            KEY_BACKSPACE,
            0,
            Setup::Idle,
            Consumed::Ignored,
            "idle backspace",
        ),
        (KEY_ESCAPE, 0, Setup::Idle, Consumed::Ignored, "idle escape"),
        (KEY_UP, 0, Setup::Idle, Consumed::Ignored, "idle up"),
        (
            KEY_SPACE,
            0,
            Setup::Composing,
            Consumed::Consumed,
            "composing space commits the highlight",
        ),
        (
            KEY_5,
            0,
            Setup::ComposingShort,
            Consumed::Ignored,
            "a digit that names no candidate",
        ),
        (
            KEY_E,
            CTRL | SHIFT,
            Setup::Composing,
            Consumed::Consumed,
            "temporary English is entered",
        ),
        (
            KEY_Q,
            CTRL,
            Setup::Composing,
            Consumed::Ignored,
            "nobody's chord",
        ),
    ];
    for (sym, state, setup, expected, what) in rows {
        let action = action_of(sym, state);
        let session = setup.session(&fixture);
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            expected,
            "{what}: {sym:#x} with {state:#x} as {action:?}"
        );
    }
}
