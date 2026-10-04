//! A live composition: the keys that change the input and the window, the escape
//! that cancels it, and the states the machine passes through while the composition
//! is being taken back.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{HideReason, KeyAction};

use crate::state::{SessionConfig, SessionEvent, SessionState, step};

use super::{Fixture, committed, composing, diagnosed, frame_of, hidden, kinds, preedit_text};

#[test]
fn test_composing_full_width_toggle_repaints_the_window_and_flashes_nothing() {
    // Inside a composition the window is on screen and its strip catches up on the
    // frame: that repaint is the feedback, and a flash beside it would say the same
    // thing twice.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::ToggleFullWidth, &cfg, &env);

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_composing_letter_appends_and_redecodes() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "n");

    let effects = session.handle_key(KeyAction::InputChar('i'), &cfg, &env);

    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    assert_eq!(session.buf.raw(), "ni");
    assert_eq!(session.state, SessionState::Composing);
    assert!(
        session.decoded().candidates.len() > usize::from(cfg.max_per_row),
        "more than one page of readings"
    );
    assert_eq!(frame_of(&effects).map(|frame| frame.revision), Some(2));
    assert_eq!(preedit_text(&effects), Some("ni"));
}

#[test]
fn test_composing_letter_at_the_length_cap_is_dropped_with_a_diagnostic() {
    let cfg = SessionConfig {
        max_raw_len: 2,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);

    assert_eq!(kinds(&effects), ["diagnose"]);
    assert_eq!(
        diagnosed(&effects),
        Some(String::from("decode/too-long: len=3 max=2"))
    );
    assert_eq!(session.buf.raw(), "ni", "the input is left as it was");
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_composing_backspace_removes_a_whole_syllable() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");
    assert_eq!(session.buf.raw(), "nihao");

    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    // One keystroke removes the trailing syllable, not one character: that is what
    // the grid written back from the segmentation is for.
    assert_eq!(session.buf.raw(), "ni");
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_composing_backspace_that_empties_the_input_returns_to_idle() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "n");

    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(hidden(&effects), Some(HideReason::EmptyInput));
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
    assert_eq!(
        committed(&effects),
        None,
        "nothing is committed on a backspace"
    );
}

#[test]
fn test_composing_escape_cancels_and_hides() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::Escape, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(hidden(&effects), Some(HideReason::Cancelled));
    assert_eq!(session.state, SessionState::Cancelling);
    assert_eq!(session.buf.raw(), "", "the input is taken back at once");
    assert_eq!(session.decoded().candidates.len(), 0);
}

#[test]
fn test_cancelling_preedit_cleared_returns_to_idle() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);

    let effects = step(&mut session, SessionEvent::PreeditCleared, &cfg, &env);

    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_cancelling_ignores_keys_until_the_preedit_is_cleared() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);

    let effects = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Cancelling);
    assert_eq!(session.buf.raw(), "");
}

#[test]
fn test_composing_focus_lost_hides_without_committing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = step(&mut session, SessionEvent::FocusLost, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(hidden(&effects), Some(HideReason::FocusLost));
    assert_eq!(
        committed(&effects),
        None,
        "losing the focus commits nothing"
    );
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
}

#[test]
fn test_composing_reset_behaves_like_losing_the_focus() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = step(&mut session, SessionEvent::Reset, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(session.state, SessionState::Idle);
}
