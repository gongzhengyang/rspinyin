//! Selection and the commit: how a composition becomes text, what the step asks the
//! host to record, and the one state the machine rests in while the commit lands.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{KeyAction, UiEvent};

use crate::state::{Session, SessionConfig, SessionEvent, SessionState, step};

use super::{Fixture, committed, composing, kinds, recorded};

#[test]
fn test_composing_space_commits_the_highlighted_candidate() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let _ = step(
        &mut session,
        SessionEvent::Ui(UiEvent::Hover {
            revision,
            index: Some(2),
        }),
        &cfg,
        &env,
    );
    let expected = session.decoded().candidates[2].text.clone();

    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(kinds(&effects), ["commit"]);
    assert_eq!(committed(&effects), Some(expected.as_str()));
    assert_eq!(session.state, SessionState::Committing);
}

#[test]
fn test_composing_digit_commits_the_candidate_it_names() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let expected = session.decoded().candidates[2].text.clone();

    let effects = session.handle_key(KeyAction::SelectIndex(3), &cfg, &env);

    assert_eq!(committed(&effects), Some(expected.as_str()));
    assert_eq!(
        session.paging.highlight, 2,
        "the digit moves the highlight with it"
    );
}

#[test]
fn test_composing_digit_past_the_page_commits_nothing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    // The page shows five candidates, so the sixth digit names nothing and the key
    // goes back to the host.
    let effects = session.handle_key(KeyAction::SelectIndex(6), &cfg, &env);

    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(committed(&effects), None);
}

#[test]
fn test_composing_digit_zero_is_never_a_selection() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::SelectIndex(0), &cfg, &env);

    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_composing_commit_raw_commits_the_typed_input() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::CommitRaw, &cfg, &env);

    assert_eq!(committed(&effects), Some("ni"));
    assert_eq!(session.state, SessionState::Committing);
}

#[test]
fn test_committing_commit_done_records_the_word_and_clears_the_session() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let word = session.decoded().candidates[0].text.clone();
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    let effects = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    assert_eq!(kinds(&effects), ["record-user-freq", "set-client-preedit"]);
    assert_eq!(recorded(&effects), Some((word.as_str(), 1)));
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
    // The step only *asks* for the record: `Effect::RecordUserFreq` is the request, and
    // writing it is the caller's job, because the write is batched and deferred rather
    // than done on the host thread. So the source the fixture injects is still untouched
    // here; the addon is where the effect is applied.
    assert!(
        fixture.user.take().is_empty(),
        "the step emits the request; applying it belongs to the caller"
    );
}

#[test]
fn test_committing_raw_commit_is_not_recorded_as_a_word() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitRaw, &cfg, &env);

    let effects = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    assert_eq!(
        recorded(&effects),
        None,
        "pinyin is not a word the user chose"
    );
    assert!(fixture.user.take().is_empty());
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_committing_commit_done_without_a_commit_records_nothing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let effects = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit"]);
    assert!(fixture.user.take().is_empty());
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_committing_focus_lost_keeps_the_commit_and_returns_to_idle() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    let effects = step(&mut session, SessionEvent::FocusLost, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit"]);
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
}

#[test]
fn test_committing_ignores_a_second_commit_key() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert!(
        effects.is_empty(),
        "a commit in flight is not started twice"
    );
    assert_eq!(session.state, SessionState::Committing);
}
