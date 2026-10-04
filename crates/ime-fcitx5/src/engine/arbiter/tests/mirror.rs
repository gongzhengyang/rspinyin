//! The mirror between `executability` and what a step actually does, and the
//! boundary cases of the two answers.

use ime_core::state::{SessionConfig, SessionState};
use ime_types::KeyAction;

use super::{Executability, Fixture, Setup, actions, executability, step_acted};

// ── The mirror ───────────────────────────────────────────────────────────────────

/// `executability` must agree with what a step actually does, for every action and state.
///
/// The one place the two answers are allowed to differ is the shape of the question rather
/// than the answer: temporary English produces no effect when it ends, so a step with the
/// mode on changes the session by turning the flag off and the verdict is `Inert` all the
/// same, because the mode's whole meaning is that every key reaches the application.
#[test]
fn test_executability_mirrors_what_step_actually_does() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut rows = 0usize;
    for setup in Setup::ALL {
        for temp_english in [false, true] {
            for action in actions() {
                let mut session = setup.session(&fixture);
                // Written in rather than reached by stepping: the chord that turns the mode
                // on also takes a composition back, so this is the only way to ask about
                // every state with the mode on.
                session.temp_english = temp_english;

                let verdict = executability(&session, action, &cfg);
                let effects = fixture.step(&mut session, action);

                assert_eq!(
                    verdict.is_executable(),
                    step_acted(&session, &effects, temp_english),
                    "{} with temp_english={temp_english} and {action:?}",
                    setup.label()
                );
                rows += 1;
            }
        }
    }
    assert_eq!(rows, Setup::ALL.len() * 2 * actions().len(), "rows ran");
}

/// Asking what a session would do must not change it.
///
/// The two questions that need to try something — would the page turn, would the caret move
/// — are asked of copies, so the caller may ask and then still step the session it asked
/// about.
#[test]
fn test_executability_leaves_the_session_alone() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    let candidates = session.decoded().clone();
    let raw = String::from(session.buf.raw());
    let paging = session.paging;
    let caret = session.buf.caret();

    for action in actions() {
        let _ = executability(&session, action, &cfg);
    }

    assert_eq!(session.decoded(), &candidates, "the list is untouched");
    assert_eq!(session.buf.raw(), raw.as_str(), "and so is the input");
    assert_eq!(session.buf.caret(), caret, "and the caret");
    assert_eq!(session.paging, paging, "and the page on show");
    assert_eq!(session.state, SessionState::Composing);
    assert!(!session.temp_english);
}

/// A typed character that would push the input past the configured limit is not ours.
///
/// The session reports `decode/too-long` and keeps the input as it is, so the key has to
/// reach the application rather than disappear into a full buffer.
#[test]
fn test_executability_input_char_at_the_length_cap_is_inert() {
    let fixture = Fixture::new();
    let cfg = SessionConfig {
        max_raw_len: 2,
        ..SessionConfig::default()
    };
    let session = fixture.composing("ni");
    assert_eq!(session.buf.raw().len(), usize::from(cfg.max_raw_len));

    assert_eq!(
        executability(&session, KeyAction::InputChar('a'), &cfg),
        Executability::Inert
    );
    // The same key against a limit with room left in it is the session's.
    assert_eq!(
        executability(
            &session,
            KeyAction::InputChar('a'),
            &SessionConfig::default()
        ),
        Executability::Executable
    );
}

/// A character outside the input alphabet would be reported and dropped.
#[test]
fn test_executability_input_char_outside_the_alphabet_is_inert() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    for rejected in ['1', ' ', '中'] {
        assert_eq!(
            executability(&session, KeyAction::InputChar(rejected), &cfg),
            Executability::Inert,
            "{rejected:?} is not in the input alphabet"
        );
    }
    for accepted in ['a', 'Z', '\''] {
        assert_eq!(
            executability(&session, KeyAction::InputChar(accepted), &cfg),
            Executability::Executable,
            "{accepted:?} is in the input alphabet"
        );
    }
}

/// The two answers are distinct, and the accessor says which is which.
#[test]
fn test_is_executable_distinguishes_the_two_answers() {
    assert!(Executability::Executable.is_executable());
    assert!(!Executability::Inert.is_executable());
    assert_ne!(Executability::Executable, Executability::Inert);
}
