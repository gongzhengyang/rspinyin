//! Shared doubles and the life-cycle tests of the session state machine.
//!
//! The helpers live here and are shared with the table-driven suite next door, so
//! that both files describe effects the same way. Everything is driven from
//! in-memory doubles: no dictionary file, no clock, no environment, no display
//! server.

mod frame;
mod scheme;
mod workspace;

use std::sync::Mutex;

use ime_types::{
    CandidateSource, HideReason, KeyAction, Placement, SelectTrigger, UiEvent, UserFreqSource,
};

use super::machine::{Session, SessionEnv, SessionState};
use super::paging::Paging;
use super::{Effect, SessionConfig};
use crate::lm::InMemoryLm;
use crate::segment::{SYLLABLES, SyllableDag, normalize};
use crate::viterbi::Decoder;
use crate::viterbi::lattice::testing::MockLexicon;

/// A user-frequency source that remembers every record it was given.
///
/// The trait's `record` takes `&self`, so the log needs interior mutability; a mutex
/// is used rather than a cell because the trait requires `Sync`.
#[derive(Default)]
pub(super) struct RecordingUser {
    recorded: Mutex<Vec<(String, u16)>>,
}

impl RecordingUser {
    /// Takes everything recorded so far, leaving the log empty.
    pub(super) fn take(&self) -> Vec<(String, u16)> {
        match self.recorded.lock() {
            Ok(mut log) => core::mem::take(&mut *log),
            Err(_) => Vec::new(),
        }
    }
}

impl UserFreqSource for RecordingUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, key: &str, weight_hint: u16) {
        if let Ok(mut log) = self.recorded.lock() {
            log.push((String::from(key), weight_hint));
        }
    }

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// The injected data sources, kept together so a test can hand them to `step`.
pub(super) struct Fixture {
    lexicon: MockLexicon,
    pub(super) user: RecordingUser,
    lm: InMemoryLm,
    decoder: Decoder,
}

impl Fixture {
    /// Builds a dictionary with enough readings of `ni` to fill more than one page,
    /// enough of `hao` for a two-syllable input, and several whole-input readings of
    /// `ni'hao` so that the two-syllable cases also page.
    pub(super) fn new() -> Self {
        let lexicon = MockLexicon::with(&[
            ("ni", "你"),
            ("ni", "尼"),
            ("ni", "泥"),
            ("ni", "拟"),
            ("ni", "逆"),
            ("ni", "妮"),
            ("ni", "匿"),
            ("ni", "腻"),
            ("ni", "溺"),
            ("ni", "倪"),
            ("ni", "霓"),
            ("ni", "呢"),
            ("hao", "好"),
            ("hao", "号"),
            ("hao", "浩"),
            ("ni'hao", "你好"),
            ("ni'hao", "拟好"),
            ("ni'hao", "泥号"),
            ("ni'hao", "妮浩"),
            ("ni'hao", "逆号"),
            ("ni'hao", "匿好"),
        ])
        .single("ni", &["伱"])
        .single("hao", &["郝"]);
        Self {
            lexicon,
            user: RecordingUser::default(),
            lm: InMemoryLm::new(),
            decoder: Decoder::default(),
        }
    }

    /// Borrows the sources as the environment a step reads.
    pub(super) fn env(&self) -> SessionEnv<'_> {
        SessionEnv {
            decoder: &self.decoder,
            lexicon: &self.lexicon,
            user_freq: &self.user,
            lm: &self.lm,
        }
    }
}

/// Names the variant of one effect.
fn kind(effect: &Effect) -> &'static str {
    match effect {
        Effect::UpdatePreedit(_) => "update-preedit",
        Effect::SendFrame(_) => "send-frame",
        Effect::Show(_) => "show",
        Effect::Hide(_) => "hide",
        Effect::Commit(_) => "commit",
        Effect::RecordUserFreq { .. } => "record-user-freq",
        Effect::AddPhrase { .. } => "add-phrase",
        Effect::ForgetUserWord { .. } => "forget-user-word",
        Effect::Diagnose(_) => "diagnose",
        Effect::SetClientPreedit(_) => "set-client-preedit",
    }
}

/// Names the variant of every effect, in order.
pub(super) fn kinds(effects: &[Effect]) -> Vec<&'static str> {
    effects.iter().map(kind).collect()
}

/// Returns the text of the preedit the step asked for.
pub(super) fn preedit_text(effects: &[Effect]) -> Option<&str> {
    effects.iter().find_map(|effect| match effect {
        Effect::UpdatePreedit(preedit) => Some(preedit.text.as_str()),
        _ => None,
    })
}

/// Returns the frame the step asked for.
pub(super) fn frame_of(effects: &[Effect]) -> Option<&ime_types::UiFrame> {
    effects.iter().find_map(|effect| match effect {
        Effect::SendFrame(frame) => Some(&**frame),
        _ => None,
    })
}

/// Returns where the step asked the window to appear.
pub(super) fn shown(effects: &[Effect]) -> Option<Placement> {
    effects.iter().find_map(|effect| match effect {
        Effect::Show(hint) => Some(hint.placement),
        _ => None,
    })
}

/// Returns the revision the show effect names.
pub(super) fn shown_revision(effects: &[Effect]) -> Option<u32> {
    effects.iter().find_map(|effect| match effect {
        Effect::Show(hint) => Some(hint.revision.value()),
        _ => None,
    })
}

/// Returns why the step asked the window to hide.
pub(super) fn hidden(effects: &[Effect]) -> Option<HideReason> {
    effects.iter().find_map(|effect| match effect {
        Effect::Hide(reason) => Some(*reason),
        _ => None,
    })
}

/// Returns the text the step asked to commit.
pub(super) fn committed(effects: &[Effect]) -> Option<&str> {
    effects.iter().find_map(|effect| match effect {
        Effect::Commit(text) => Some(text.as_str()),
        _ => None,
    })
}

/// Returns the word and weight the step asked to record.
pub(super) fn recorded(effects: &[Effect]) -> Option<(&str, u16)> {
    effects.iter().find_map(|effect| match effect {
        Effect::RecordUserFreq { key, weight_hint } => Some((key.as_str(), *weight_hint)),
        _ => None,
    })
}

/// Returns the rendered code of the first diagnostic the step raised.
pub(super) fn diagnosed(effects: &[Effect]) -> Option<String> {
    effects.iter().find_map(|effect| match effect {
        Effect::Diagnose(err) => Some(err.to_string()),
        _ => None,
    })
}

/// Returns the index of `text` in the session's candidate list.
pub(super) fn index_of(session: &Session, text: &str) -> Option<u16> {
    session
        .decoded()
        .candidates
        .iter()
        .position(|candidate| candidate.text == text)
        .and_then(|position| u16::try_from(position).ok())
}

/// Returns the text of the candidate the highlight is on.
pub(super) fn highlighted(session: &Session) -> Option<&str> {
    session
        .highlighted_candidate()
        .map(|held| held.text.as_str())
}

/// Builds a session by typing `raw` one character at a time.
pub(super) fn composing(cfg: &SessionConfig, env: &SessionEnv<'_>, raw: &str) -> Session {
    let mut session = Session::new();
    for ch in raw.chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), cfg, env);
    }
    session
}

/// Builds a session in one of the four states, for the table-driven suite.
///
/// The composition is two syllables long so that a Backspace in the table has a
/// syllable to remove and leaves the session composing.
pub(super) fn session_in(
    state: SessionState,
    cfg: &SessionConfig,
    env: &SessionEnv<'_>,
) -> Session {
    let mut session = composing(cfg, env, "nihao");
    match state {
        SessionState::Idle => Session::new(),
        SessionState::Composing => session,
        SessionState::Cancelling => {
            let _ = session.handle_key(KeyAction::Escape, cfg, env);
            session
        }
        SessionState::Committing => {
            let _ = session.handle_key(KeyAction::CommitHighlighted, cfg, env);
            session
        }
    }
}

/// One representative of each event kind, for the totality sweep.
pub(super) fn representative(index: usize, revision: u32) -> super::SessionEvent {
    match index {
        0 => super::SessionEvent::Key(KeyAction::InputChar('n')),
        1 => super::SessionEvent::Ui(UiEvent::Select {
            revision,
            index: 0,
            trigger: SelectTrigger::Mouse,
        }),
        2 => super::SessionEvent::ConfigReloaded(SessionConfig::default()),
        3 => super::SessionEvent::FocusLost,
        4 => super::SessionEvent::Reset,
        5 => super::SessionEvent::PreeditCleared,
        _ => super::SessionEvent::CommitDone,
    }
}

/// Every key action the frozen contract defines.
pub(super) fn every_key_action() -> [KeyAction; 19] {
    [
        KeyAction::InputChar('n'),
        KeyAction::Backspace,
        KeyAction::CommitHighlighted,
        KeyAction::CommitRaw,
        KeyAction::SelectIndex(1),
        KeyAction::PageNext,
        KeyAction::PagePrev,
        KeyAction::MoveHighlight(1),
        KeyAction::MoveCaret(1),
        KeyAction::ToggleLang,
        KeyAction::ToggleFullWidth,
        KeyAction::TogglePunct,
        KeyAction::EnterTempEnglish,
        KeyAction::Escape,
        KeyAction::Ignore,
        // Appended by ADR-0005. The array is sized by hand rather than built from
        // an iterator so that adding a variant to the enum breaks this function's
        // type, which is what forces the sweep below to keep covering the whole
        // action space.
        KeyAction::ToggleScript,
        KeyAction::ForgetHighlighted,
        KeyAction::PinHighlighted,
        KeyAction::AddPhrase,
    ]
}

/// Builds a click that refers to `revision`.
pub(super) fn select_event(revision: u32) -> UiEvent {
    UiEvent::Select {
        revision,
        index: 1,
        trigger: SelectTrigger::Mouse,
    }
}

/// Builds a hover that refers to `revision`.
pub(super) fn hover_event(revision: u32) -> UiEvent {
    UiEvent::Hover {
        revision,
        index: Some(1),
    }
}

/// Builds a page flip that refers to `revision`.
pub(super) fn page_event(revision: u32) -> UiEvent {
    UiEvent::Page {
        revision,
        dir: ime_types::PageDir::Next,
    }
}

/// Builds a dismissal that refers to `revision`.
pub(super) fn dismiss_event(revision: u32) -> UiEvent {
    UiEvent::Dismiss {
        revision,
        reason: ime_types::DismissReason::OutsideClick,
    }
}

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
    let _ = super::step(
        &mut session,
        super::SessionEvent::PreeditCleared,
        &cfg,
        &env,
    );
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
        KeyAction::ToggleFullWidth,
        KeyAction::TogglePunct,
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

    let effects = super::step(
        &mut session,
        super::SessionEvent::PreeditCleared,
        &cfg,
        &env,
    );

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

    let effects = super::step(&mut session, super::SessionEvent::FocusLost, &cfg, &env);

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

    let effects = super::step(&mut session, super::SessionEvent::Reset, &cfg, &env);

    assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_composing_space_commits_the_highlighted_candidate() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let _ = super::step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Hover {
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

    let effects = super::step(&mut session, super::SessionEvent::CommitDone, &cfg, &env);

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

    let effects = super::step(&mut session, super::SessionEvent::CommitDone, &cfg, &env);

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

    let effects = super::step(&mut session, super::SessionEvent::CommitDone, &cfg, &env);

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

    let effects = super::step(&mut session, super::SessionEvent::FocusLost, &cfg, &env);

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

#[test]
fn test_session_is_idle_before_anything_happens() {
    let session = Session::default();

    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
    assert_eq!(session.decoded().candidates.len(), 0);
    assert_eq!(session.paging, Paging::new());
    assert_eq!(session.revision.value(), 0);
    assert!(!session.temp_english);
    assert_eq!(session.preedit().text, "");
    assert_eq!(session.highlighted_candidate(), None);
}

#[test]
fn test_session_id_is_allocated_from_the_counter_it_was_given() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new_with_counter(1000);

    let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert_eq!(session.id.value(), 1000);
}

#[test]
fn test_session_config_clamps_a_length_limit_the_schema_forbids() {
    assert_eq!(SessionConfig::new(0, 5, true, 720).max_raw_len, 1);
    assert_eq!(SessionConfig::new(200, 5, true, 720).max_raw_len, 64);
    assert_eq!(
        SessionConfig::default(),
        SessionConfig::new(64, 5, true, 720)
    );
    assert_eq!(
        SessionConfig::default().max_per_row,
        super::paging::DEFAULT_PAGE_SIZE
    );
}

#[test]
fn test_candidate_index_helper_finds_a_word() {
    // The helper the assertions above rely on, checked on its own so that a silent
    // change in the candidate list cannot make them vacuous.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "ni");

    let found = index_of(&session, "你");
    assert!(found.is_some(), "the reading is offered");
    assert_eq!(
        found
            .and_then(|at| session.decoded().candidates.get(usize::from(at)))
            .map(|held| held.text.as_str()),
        Some("你")
    );
    assert_eq!(index_of(&session, "nothing like this"), None);
    assert_eq!(
        session.decoded().candidates[0].source,
        CandidateSource::Dict,
        "the readings come from the dictionary"
    );
}

#[test]
fn test_typing_every_table_syllable_with_bare_keys_composes() {
    // Ergonomics: the whole syllable table is reachable with the keyboard alone. Each
    // entry is typed as its own spelling, with `v` standing in for the umlaut the
    // normalizer folds, one `InputChar` per character -- the action a bare letter key
    // produces -- and no modifier and no mouse event takes part in it.
    //
    // `ê` is the one entry no bare key types: the input alphabet is the ASCII letters and
    // `'`, and the table keeps `ê` as the syllable the normalizer passes through
    // unchanged. The list is pinned so that a second such entry cannot appear unnoticed.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut typed = String::new();
    let mut not_typeable = Vec::new();
    for entry in SYLLABLES {
        typed.clear();
        for ch in entry.chars() {
            typed.push(if ch == 'ü' { 'v' } else { ch });
        }
        if !typed
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '\'')
        {
            not_typeable.push(*entry);
            continue;
        }
        // The typed form reaches the entry it is spelled after...
        assert_eq!(normalize(&typed).text, *entry, "typing {typed:?}");
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build(&typed), Ok(()), "building {typed:?}");
        assert!(dag.has_path(), "{typed:?} must cut into syllables");
        // ...and the session takes it one key press at a time, with nothing typed lost.
        let mut session = Session::new();
        for ch in typed.chars() {
            let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
        }
        assert_eq!(session.state, SessionState::Composing, "typing {typed:?}");
        assert_eq!(session.buf.raw(), typed.as_str(), "typing {typed:?}");
    }
    assert_eq!(not_typeable, ["ê"]);
}
