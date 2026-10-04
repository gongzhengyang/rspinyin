//! The machine at rest: with nothing composing, keys are handed back or announced,
//! and the first letter of a composition opens the window.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{KeyAction, Placement};

use crate::state::{
    Effect, Session, SessionConfig, SessionEvent, SessionState, effects::ModeBit, step,
};

use super::{Fixture, diagnosed, frame_of, kinds, preedit_text, shown, shown_revision};

#[test]
fn test_idle_letter_starts_a_composing_session_and_shows_the_window() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let effects = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert_eq!(
        kinds(&effects),
        ["update-preedit", "show", "send-frame"],
        "the show must precede the frame, which the window drops while hidden"
    );
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.buf.raw(), "n");
    assert_ne!(
        session.id.value(),
        0,
        "a composition gets a fresh session id"
    );
    assert_eq!(shown(&effects), Some(Placement::Auto));
    assert_eq!(shown_revision(&effects), Some(1));
    assert_eq!(frame_of(&effects).map(|frame| frame.revision), Some(1));
    assert_eq!(preedit_text(&effects), Some("n"));
}

#[test]
fn test_idle_letter_allocates_a_new_session_id_each_time() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);
    let first = session.id;
    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);
    let _ = step(&mut session, SessionEvent::PreeditCleared, &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);

    assert!(session.id.value() > first.value(), "the id is monotonic");
}

#[test]
fn test_idle_character_outside_the_alphabet_is_reported_and_not_typed() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let effects = session.handle_key(KeyAction::InputChar('1'), &cfg, &env);

    assert_eq!(kinds(&effects), ["diagnose"]);
    assert!(diagnosed(&effects).is_some_and(|code| code.starts_with("decode/invalid-char")));
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
}

#[test]
fn test_idle_keys_with_nothing_to_act_on_are_handed_back() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let actions = [
        KeyAction::Backspace,
        KeyAction::CommitHighlighted,
        KeyAction::CommitRaw,
        KeyAction::SelectIndex(3),
        KeyAction::PageNext,
        KeyAction::PagePrev,
        KeyAction::MoveHighlight(1),
        KeyAction::MoveCaret(-1),
        KeyAction::ToggleLang,
        KeyAction::ToggleScript,
        KeyAction::Escape,
        KeyAction::Ignore,
    ];
    for action in actions {
        let mut session = Session::new();
        let effects = session.handle_key(action, &cfg, &env);
        assert!(
            effects.is_empty(),
            "{action:?} must produce no effect when idle"
        );
        assert_eq!(session.state, SessionState::Idle, "{action:?}");
        assert_eq!(
            session.revision.value(),
            0,
            "{action:?} must not emit a frame"
        );
    }
}

#[test]
fn test_idle_mode_toggles_flash_their_own_bits_and_touch_nothing_else() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();

    // Each switch announces its own bit, and does nothing else: with nothing composing
    // there is no window to repaint, so the flash is the whole of the transition.
    for (action, bit) in [
        (KeyAction::ToggleFullWidth, ModeBit::FullWidth),
        (KeyAction::TogglePunct, ModeBit::PunctFull),
    ] {
        let mut session = Session::new();
        let effects = session.handle_key(action, &cfg, &env);
        assert_eq!(
            kinds(&effects),
            ["mode-flash"],
            "{action:?} announces itself, and does nothing else"
        );
        assert!(
            matches!(effects.first(), Some(Effect::ModeFlash { bit: named }) if *named == bit),
            "{action:?} names the bit it moved"
        );
        assert_eq!(session.state, SessionState::Idle, "{action:?}");
        assert_eq!(
            session.revision.value(),
            0,
            "{action:?} must not emit a frame: there is no window"
        );
    }
}
