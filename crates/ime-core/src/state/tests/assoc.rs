//! Deterministic state-machine coverage for the post-commit association view.
//!
//! Providers are in-memory doubles with observable call counts. No dictionary file,
//! clock, display server or persistence participates, and no committed text leaves a
//! test's local assertion.

use std::sync::Mutex;

use ime_types::{CandidateSource, HideReason, KeyAction};

use crate::assoc::{AssocHit, AssocProvider, AssocSourceKind};
use crate::state::{Session, SessionConfig, SessionEvent, SessionState, step};

use super::{Fixture, committed, composing, frame_of, hidden, kinds, recorded};

/// An explicitly injected association source whose calls a test can observe.
pub(in crate::state) struct AssocProviderStub {
    pub(in crate::state) kind: AssocSourceKind,
    hits: Vec<AssocHit>,
    previous: Mutex<Vec<String>>,
}

impl AssocProviderStub {
    fn new(kind: AssocSourceKind, hits: &[(&str, &str)]) -> Self {
        Self {
            kind,
            hits: hits
                .iter()
                .map(|(text, commit)| AssocHit::new(*text, *commit, kind))
                .collect(),
            previous: Mutex::new(Vec::new()),
        }
    }

    fn empty(kind: AssocSourceKind) -> Self {
        Self::new(kind, &[])
    }

    fn previous(&self) -> Vec<String> {
        self.previous
            .lock()
            .map(|previous| previous.clone())
            .unwrap_or_default()
    }
}

impl AssocProvider for AssocProviderStub {
    fn kind(&self) -> AssocSourceKind {
        self.kind
    }

    fn hits_after(&self, previous: &str, _limit: u8) -> Vec<AssocHit> {
        if let Ok(mut previous_hits) = self.previous.lock() {
            previous_hits.push(String::from(previous));
        }
        self.hits.clone()
    }
}

fn assoc_cfg(max_candidates: u8, history_depth: u8) -> SessionConfig {
    SessionConfig {
        assoc: crate::state::AssocSessionConfig {
            enabled: true,
            max_candidates,
            history_depth,
        },
        ..SessionConfig::default()
    }
}

fn commit_once(cfg: &SessionConfig, env: &super::super::SessionEnv<'_>, key: &str) -> Session {
    let mut session = composing(cfg, env, "ni");
    let committed_word = session.decoded().candidates[0].text.clone();
    assert_eq!(committed_word, key);
    let _ = session.handle_key(KeyAction::CommitHighlighted, cfg, env);
    let _ = step(&mut session, SessionEvent::CommitDone, cfg, env);
    session
}

#[test]
fn test_assoc_disabled_default_does_not_query_or_open_a_view() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, false);

    let session = commit_once(&cfg, &env, "你");

    assert_eq!(session.state, SessionState::Idle);
    assert!(session.decoded().candidates.is_empty());
    assert!(provider.previous().is_empty());
}

#[test]
fn test_assoc_enabled_commit_opens_a_bounded_candidate_view() {
    let cfg = assoc_cfg(2, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(
        AssocSourceKind::Bigram,
        &[("first", "first"), ("second", "second"), ("third", "third")],
    );
    let env = fixture.env_assoc(&provider, false);

    let session = commit_once(&cfg, &env, "你");
    let candidates = session.decoded().candidates.clone();

    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.annotation.as_deref() == Some("联想"))
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.source == CandidateSource::Assoc)
    );
    assert_eq!(provider.previous(), ["你"]);
}

#[test]
fn test_assoc_empty_hits_keep_the_ordinary_idle_post_commit_state() {
    let cfg = assoc_cfg(5, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::empty(AssocSourceKind::Bigram);
    let env = fixture.env_assoc(&provider, false);

    let session = commit_once(&cfg, &env, "你");

    assert_eq!(session.state, SessionState::Idle);
    assert!(session.decoded().candidates.is_empty());
    assert_eq!(provider.previous(), ["你"]);
}

#[test]
fn test_assoc_sensitive_context_is_suppressed_before_provider_queries() {
    let cfg = assoc_cfg(5, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, true);

    let session = commit_once(&cfg, &env, "你");

    assert_eq!(session.state, SessionState::Idle);
    assert!(session.decoded().candidates.is_empty());
    assert!(provider.previous().is_empty());
    assert_eq!(session.assoc_history_len(), 0);
}

#[test]
fn test_assoc_selection_commits_the_hit_commit_text_not_display_text() {
    let cfg = assoc_cfg(1, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("shown", "inserted")]);
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(kinds(&effects), ["commit"]);
    assert_eq!(committed(&effects), Some("inserted"));
    assert_eq!(session.state, SessionState::Committing);
    assert_eq!(recorded(&effects), None);
}

#[test]
fn test_assoc_digit_selects_the_numbered_view_candidate() {
    let cfg = assoc_cfg(3, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(
        AssocSourceKind::Bigram,
        &[
            ("first", "first"),
            ("second-insert", "second-commit"),
            ("third", "third"),
        ],
    );
    let env = fixture.env_assoc(&provider, false);
    assert_eq!(provider.hits_after("probe", 5).len(), 3);
    let runtime = env.assoc.expect("the fixture injects an assoc runtime");
    assert!(runtime.providers.bigram.is_some());
    let injected = runtime
        .providers
        .bigram
        .expect("the fixture selects the Bigram slot")
        .hits_after("probe", 5);
    assert_eq!(injected.len(), 3);
    assert_eq!(injected[0].text, "first");
    let merged = crate::assoc::merge_assoc(5, &injected, &[], &[]);
    assert_eq!(merged[0].text, "first");
    assert_eq!(merged[1].text, "second-insert");
    assert_eq!(merged[2].text, "third");

    let mut session = commit_once(&cfg, &env, "你");
    assert_eq!(provider.previous(), ["probe", "probe", "你"]);
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(
        session.assoc_view.as_ref().map(|view| view
            .hits
            .iter()
            .map(|hit| hit.text.as_str())
            .collect::<Vec<_>>()),
        Some(vec!["first", "second-insert", "third"])
    );
    assert_eq!(session.decoded().candidates.len(), 3);
    assert_eq!(session.paging.highlight, 0);
    assert_eq!(
        session.assoc_view.as_ref().map(|view| view.hits.len()),
        Some(3)
    );
    assert_eq!(session.decoded().candidates[2].text, "third");
    assert_eq!(session.decoded().candidates[2].text, "third");
    let effects = session.handle_key(KeyAction::SelectIndex(3), &cfg, &env);

    assert_eq!(committed(&effects), Some("third"));
    assert_eq!(session.state, SessionState::Committing);
}

#[test]
fn test_assoc_paging_and_highlight_move_without_provider_requery() {
    let cfg = SessionConfig {
        max_per_row: 3,
        ..assoc_cfg(4, 8)
    };
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(
        AssocSourceKind::Bigram,
        &[
            ("one", "one"),
            ("two", "two"),
            ("three", "three"),
            ("four", "four"),
            ("five", "five"),
        ],
    );
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let first = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let second = session.handle_key(KeyAction::MoveHighlight(-1), &cfg, &env);

    assert_eq!(kinds(&first), ["send-frame"]);
    assert_eq!(kinds(&second), ["send-frame"]);
    assert!(frame_of(&first).is_some_and(|frame| frame.page.current == 2));
    assert_eq!(provider.previous(), ["你"]);
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_assoc_printable_input_breaks_to_ordinary_decoding() {
    let cfg = assoc_cfg(1, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let effects = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.buf.raw(), "n");
    assert!(!session.decoded().candidates.is_empty());
    assert!(
        session
            .decoded()
            .candidates
            .iter()
            .all(|candidate| candidate.source != CandidateSource::Assoc)
    );
    assert_eq!(provider.previous(), ["你"]);
    assert!(kinds(&effects).contains(&"send-frame"));
}

#[test]
fn test_assoc_backspace_breaks_and_hides_immediately() {
    let cfg = assoc_cfg(1, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(hidden(&effects), Some(HideReason::EmptyInput));
    assert_eq!(session.state, SessionState::Idle);
    assert!(session.decoded().candidates.is_empty());
}

#[test]
fn test_assoc_escape_breaks_and_hides_immediately() {
    let cfg = assoc_cfg(1, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let effects = session.handle_key(KeyAction::Escape, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(hidden(&effects), Some(HideReason::Cancelled));
    assert_eq!(session.state, SessionState::Cancelling);
    assert!(session.decoded().candidates.is_empty());
}

#[test]
fn test_assoc_reset_focus_and_caret_each_clear_without_cross_focus_memory() {
    for action in [KeyAction::Ignore, KeyAction::MoveCaret(1)] {
        let cfg = assoc_cfg(1, 8);
        let fixture = Fixture::new();
        let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
        let env = fixture.env_assoc(&provider, false);
        let mut session = commit_once(&cfg, &env, "你");

        let effects = session.handle_key(action, &cfg, &env);

        assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
        assert_eq!(session.state, SessionState::Idle);
        assert!(session.decoded().candidates.is_empty());
        assert_eq!(session.assoc_history_len(), 0);
    }

    let cfg = assoc_cfg(1, 8);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("after", "after")]);
    let env = fixture.env_assoc(&provider, false);
    let mut session = commit_once(&cfg, &env, "你");
    let focus = step(&mut session, SessionEvent::FocusLost, &cfg, &env);
    assert_eq!(kinds(&focus), ["set-client-preedit", "hide"]);
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.assoc_history_len(), 0);

    let mut session = commit_once(&cfg, &env, "你");
    let reset = step(&mut session, SessionEvent::Reset, &cfg, &env);
    assert_eq!(kinds(&reset), ["set-client-preedit", "hide"]);
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.assoc_history_len(), 0);
}

#[test]
fn test_assoc_history_retains_only_the_configured_newest_depth() {
    let cfg = assoc_cfg(1, 1);
    let fixture = Fixture::new();
    let provider = AssocProviderStub::empty(AssocSourceKind::Bigram);
    let env = fixture.env_assoc(&provider, false);

    let mut session = commit_once(&cfg, &env, "你");
    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('a'), &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('o'), &cfg, &env);
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let effects = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(provider.previous(), ["你", "好"]);
    assert_eq!(session.assoc_history_len(), 1);
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.decoded().candidates[0].text, "你");
    assert_eq!(kinds(&effects).last(), Some(&"send-frame"));
}

#[test]
fn test_assoc_provider_absence_leaves_post_commit_behaviour_unchanged() {
    let cfg = assoc_cfg(5, 8);
    let fixture = Fixture::new();
    let env = fixture.env();

    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let effects = step(&mut session, SessionEvent::CommitDone, &cfg, &env);

    assert_eq!(kinds(&effects), ["record-user-freq", "set-client-preedit"]);
    assert_eq!(session.state, SessionState::Idle);
    assert!(session.decoded().candidates.is_empty());
}
