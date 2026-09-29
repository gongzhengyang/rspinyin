//! The exhaustive sweeps over the session state machine.
//!
//! Two kinds of coverage live here. The first is the design's transition table,
//! transcribed row by row into cases that assert both the next state and the exact
//! sequence of effects. The second walks every `(state, event)` and every
//! `(state, key action)` pair, which is what turns "no combination is left without a
//! defined next state" into something a build can check.

use ime_types::{KeyAction, UiEvent};

use super::machine::{SessionState, step};
use super::tests::{Fixture, composing, every_key_action, kinds, representative, session_in};
use super::{MAX_EFFECTS, SessionConfig};

#[test]
fn test_step_matches_the_design_transition_table() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let revision = session_in(SessionState::Composing, &cfg, &env)
        .revision
        .value();
    let stale = revision.wrapping_add(1);
    let rows: Vec<(SessionState, super::SessionEvent, SessionState, &[&str])> = vec![
        // Idle: a letter starts a composition, a mode key is the engine's business,
        // and anything else is handed back.
        (
            SessionState::Idle,
            super::SessionEvent::Key(KeyAction::InputChar('n')),
            SessionState::Composing,
            &["update-preedit", "show", "send-frame"],
        ),
        (
            SessionState::Idle,
            super::SessionEvent::Key(KeyAction::ToggleLang),
            SessionState::Idle,
            &[],
        ),
        (
            SessionState::Idle,
            super::SessionEvent::Key(KeyAction::Ignore),
            SessionState::Idle,
            &[],
        ),
        // Composing.
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::InputChar('a')),
            SessionState::Composing,
            &["update-preedit", "send-frame"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::Backspace),
            SessionState::Composing,
            &["update-preedit", "send-frame"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::Escape),
            SessionState::Cancelling,
            &["set-client-preedit", "hide"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::CommitHighlighted),
            SessionState::Committing,
            &["commit"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::SelectIndex(1)),
            SessionState::Committing,
            &["commit"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Key(KeyAction::PageNext),
            SessionState::Composing,
            &["send-frame"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Ui(UiEvent::Select {
                revision,
                index: 1,
                trigger: ime_types::SelectTrigger::Mouse,
            }),
            SessionState::Committing,
            &["commit"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Ui(UiEvent::Select {
                revision: stale,
                index: 1,
                trigger: ime_types::SelectTrigger::Mouse,
            }),
            SessionState::Composing,
            &["diagnose"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Ui(UiEvent::Hover {
                revision,
                index: Some(3),
            }),
            SessionState::Composing,
            &["send-frame"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::Ui(UiEvent::Dismiss {
                revision,
                reason: ime_types::DismissReason::OutsideClick,
            }),
            SessionState::Cancelling,
            &["set-client-preedit", "hide"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::FocusLost,
            SessionState::Idle,
            &["set-client-preedit", "hide"],
        ),
        (
            SessionState::Composing,
            super::SessionEvent::ConfigReloaded(cfg),
            SessionState::Composing,
            &[],
        ),
        // Cancelling.
        (
            SessionState::Cancelling,
            super::SessionEvent::PreeditCleared,
            SessionState::Idle,
            &[],
        ),
        // Committing.
        (
            SessionState::Committing,
            super::SessionEvent::CommitDone,
            SessionState::Idle,
            &["record-user-freq", "set-client-preedit"],
        ),
        (
            SessionState::Committing,
            super::SessionEvent::FocusLost,
            SessionState::Idle,
            &["set-client-preedit"],
        ),
    ];
    assert!(
        rows.len() >= 18,
        "every row of the design's table is covered"
    );

    for (before, event, after, expected) in rows {
        let mut session = session_in(before, &cfg, &env);
        let effects = step(&mut session, event.clone(), &cfg, &env);
        assert_eq!(session.state, after, "{before:?} + {event:?}");
        assert_eq!(kinds(&effects), expected, "{before:?} + {event:?}");
    }
}

#[test]
fn test_step_covers_every_state_and_event_pair() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let states = [
        SessionState::Idle,
        SessionState::Composing,
        SessionState::Cancelling,
        SessionState::Committing,
    ];
    let mut covered = 0usize;

    for state in states {
        for index in 0..7 {
            let mut session = session_in(state, &cfg, &env);
            let revision = session.revision.value();
            let event = representative(index, revision);
            let effects = step(&mut session, event.clone(), &cfg, &env);

            assert!(effects.len() <= MAX_EFFECTS, "{state:?} + {event:?}");
            assert!(!effects.spilled(), "{state:?} + {event:?} allocates");
            covered += 1;
        }
    }

    // Four states and seven event kinds: the design's four-by-six table, plus the two
    // events it needs and the task card's sketch of the enum leaves out.
    assert_eq!(covered, 4 * 7);
}

#[test]
fn test_step_covers_every_key_action_in_every_state() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let states = [
        SessionState::Idle,
        SessionState::Composing,
        SessionState::Cancelling,
        SessionState::Committing,
    ];
    let mut covered = 0usize;

    for state in states {
        for action in every_key_action() {
            let mut session = session_in(state, &cfg, &env);
            let effects = session.handle_key(action, &cfg, &env);

            assert!(effects.len() <= MAX_EFFECTS, "{state:?} + {action:?}");
            assert!(!effects.spilled(), "{state:?} + {action:?} allocates");
            covered += 1;
        }
    }

    // Derived rather than a literal. `every_key_action` is sized by hand precisely so that
    // adding a variant to `KeyAction` breaks its type, and this assertion is what proves
    // the sweep above walked every action in every state rather than stopping early -- a
    // second hand-maintained number here would only be one more thing to forget.
    assert_eq!(covered, 4 * every_key_action().len());
}

#[test]
fn test_a_whole_session_stays_inside_the_effect_budget() {
    // One session, one key at a time, over every action the machine acts on: no step
    // may emit more effects than the frame budget allows, and none may allocate.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");
    let script = [
        KeyAction::InputChar('a'),
        KeyAction::MoveHighlight(1),
        KeyAction::MoveCaret(-1),
        KeyAction::PageNext,
        KeyAction::PagePrev,
        KeyAction::Backspace,
        KeyAction::SelectIndex(2),
        KeyAction::Ignore,
    ];

    for action in script {
        let effects = session.handle_key(action, &cfg, &env);
        assert!(effects.len() <= MAX_EFFECTS, "{action:?}");
        assert!(!effects.spilled(), "{action:?}");
    }
}
