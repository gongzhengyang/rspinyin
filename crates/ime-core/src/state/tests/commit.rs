//! Selection and the commit: how a composition becomes text, what the step asks the
//! host to record, and the one state the machine rests in while the commit lands.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{CandidateSource, KeyAction, UiEvent};

use crate::state::{Session, SessionConfig, SessionEnv, SessionEvent, SessionState, SpacingMode, step};
use crate::viterbi::lattice::testing::MockLexicon;

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

/// The mixed-English configuration the commit-bridge tests run under: the
/// English domain on, the spacing switch at the value the case names.
fn cfg_english(spacing: SpacingMode) -> SessionConfig {
    SessionConfig {
        english: true,
        english_spacing: spacing,
        ..SessionConfig::default()
    }
}

/// Types `raw` into a session that already exists, the way a conversation
/// continues after a landed commit.
fn type_raw(session: &mut Session, raw: &str, cfg: &SessionConfig, env: &SessionEnv<'_>) {
    for ch in raw.chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), cfg, env);
    }
}

/// Moves the highlight onto the English candidate, the one commit the bridge
/// dresses.
fn highlight_english(session: &mut Session) {
    let index = u16::try_from(
        session
            .decoded()
            .candidates
            .iter()
            .position(|candidate| candidate.source == CandidateSource::English)
            .expect("the English edge answers an input no pinyin reading cuts"),
    )
    .expect("the candidate list is short");
    session.paging.highlight = index;
}

#[test]
fn test_commit_highlighted_english_after_a_chinese_commit_takes_the_boundary_space() {
    let cfg = cfg_english(SpacingMode::Auto);
    let fixture = Fixture::with_dictionary(&[("ni", "你")]);
    let english = MockLexicon::with(&[("github", "GitHub")]);
    let env = fixture.env_with_english(&english);
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    type_raw(&mut session, "github", &cfg, &env);
    highlight_english(&mut session);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(
        committed(&effects),
        Some(" GitHub"),
        "the boundary between 你 and the word takes its space, and the entry's own casing stands"
    );
    let done = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(
        recorded(&done).map(|(key, _)| key),
        Some("GitHub"),
        "the record keys the entry as authored, never the dressed commit"
    );
    assert_eq!(
        session.spacing.tail_for_test(),
        Some(" GitHub"),
        "the landing promoted the dressed text, which is what the screen holds"
    );
}

#[test]
fn test_commit_done_promotes_the_tail_so_the_next_english_commit_takes_one_space() {
    let cfg = cfg_english(SpacingMode::Auto);
    let fixture = Fixture::with_dictionary(&[]);
    let english = MockLexicon::with(&[("hi", "hi")]);
    let env = fixture.env_with_english(&english);
    let mut session = Session::new();

    type_raw(&mut session, "hi", &cfg, &env);
    highlight_english(&mut session);
    let first = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    assert_eq!(
        committed(&first),
        Some("hi"),
        "the first commit has no screen to bridge against"
    );
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(session.spacing.tail_for_test(), Some("hi"));

    type_raw(&mut session, "hi", &cfg, &env);
    highlight_english(&mut session);
    let second = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(
        committed(&second),
        Some(" hi"),
        "the promoted tail answers the boundary, so exactly one space"
    );
}

#[test]
fn test_committing_focus_lost_drops_the_staged_commit_and_keeps_the_tail() {
    let cfg = cfg_english(SpacingMode::Auto);
    let fixture = Fixture::with_dictionary(&[("ni", "你")]);
    let english = MockLexicon::with(&[("hi", "hi")]);
    let env = fixture.env_with_english(&english);
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(session.spacing.tail_for_test(), Some("你"));

    type_raw(&mut session, "hi", &cfg, &env);
    highlight_english(&mut session);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    assert_eq!(committed(&effects), Some(" hi"));

    let _ = step(&mut session, SessionEvent::FocusLost, &cfg, &env);

    assert_eq!(
        session.spacing.staged_for_test(),
        None,
        "a commit the focus took away never landed, so its staging is forgotten"
    );
    assert_eq!(
        session.spacing.tail_for_test(),
        Some("你"),
        "the tail is still what the screen holds"
    );
}

#[test]
fn test_commit_highlighted_with_spacing_off_skips_the_space_and_keeps_the_casing() {
    let cfg = cfg_english(SpacingMode::Off);
    let fixture = Fixture::with_dictionary(&[("ni", "你")]);
    let english = MockLexicon::with(&[("github", "GitHub")]);
    let env = fixture.env_with_english(&english);
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    type_raw(&mut session, "GITHUB", &cfg, &env);
    highlight_english(&mut session);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(
        committed(&effects),
        Some("GITHUB"),
        "the switch shuts the space, not the casing"
    );
    let done = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(
        recorded(&done).map(|(key, _)| key),
        Some("GitHub"),
        "the record keys the entry as authored"
    );
}

#[test]
fn test_commit_highlighted_chinese_candidate_is_not_dressed() {
    let cfg = cfg_english(SpacingMode::Auto);
    let fixture = Fixture::with_dictionary(&[("ni", "你")]);
    let english = MockLexicon::with(&[("hi", "hi")]);
    let env = fixture.env_with_english(&english);
    let mut session = Session::new();

    type_raw(&mut session, "hi", &cfg, &env);
    highlight_english(&mut session);
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(session.spacing.tail_for_test(), Some("hi"));

    type_raw(&mut session, "ni", &cfg, &env);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(
        committed(&effects),
        Some("你"),
        "the boundary has no opinion on English into Chinese, and the commit carries none"
    );
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(
        session.spacing.tail_for_test(),
        Some("你"),
        "the landing promoted the Chinese text, which is what the next boundary reads"
    );
}
