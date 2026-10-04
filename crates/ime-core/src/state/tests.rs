//! Shared doubles and the life-cycle tests of the session state machine.
//!
//! The helpers live here and are shared with the table-driven suite next door, so
//! that both files describe effects the same way. The life-cycle tests themselves
//! are grouped by the part of the machine they drive, in the files beside this one.
//! Everything is driven from in-memory doubles: no dictionary file, no clock, no
//! environment, no display server.

mod commit;
mod composing;
mod frame;
mod idle;
mod scheme;
mod session;
mod workspace;

use std::sync::Mutex;

use ime_types::{HideReason, KeyAction, Placement, SelectTrigger, UiEvent, UserFreqSource};

use super::machine::{Session, SessionEnv, SessionState};
use super::{Effect, SessionConfig};
use crate::lm::InMemoryLm;
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
        Effect::ModeFlash { .. } => "mode-flash",
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
