//! What the arbitrator answers, and the proof that its second condition is the session's.
//!
//! # The mirror
//!
//! The property this module exists to prove: [`executability`] must agree with what a step
//! actually does. The mirror test walks every action of the frozen enum through every
//! session state, asks for the verdict, steps the same session with the same action, and
//! compares the verdict against what the step produced. The two ways the answer can be
//! wrong are both defects: `Executable` with nothing produced is a swallowed key, and
//! `Inert` with something produced is a key the plugin acted on and then handed back.
//!
//! The two are compared rather than derived from one another, which is the point: the
//! guards in the state machine's transitions and the ones in the arbitrator live in
//! different files and would drift apart silently without this test.
//!
//! # The doubles
//!
//! Everything is in memory: a dictionary, a language model, and a user-frequency source
//! that answers nothing. No test here touches a dictionary file, the clock, the
//! environment or a display server, and none of them sleeps.
//!
//! The tests are grouped beside this file: `mirror` holds the mirror and the boundary
//! cases of the two answers, `arbitrate` the rows of the claim table, and `sequence`
//! the engine's own domain and the sequence bridge.

mod arbitrate;
mod mirror;
mod sequence;

use ime_core::lm::InMemoryLm;
use ime_core::state::{Effect, Session, SessionConfig, SessionEnv, SessionEvent};
use ime_core::state::{SessionState, step};
use ime_core::viterbi::Decoder;
use ime_types::{
    ImeError, KeyAction, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef,
};

use super::{Executability, arbitrate, arbitrate_sequence, executability, is_mode_chord};
use crate::engine::rows::{KEY_DELETE, KEY_END, KEY_HOME};
use crate::engine::*;

/// `FcitxKey_3`, a digit a short page does not reach.
const KEY_3: u32 = 0x0033;

/// `FcitxKey_5`, the last digit a page of five names.
const KEY_5: u32 = 0x0035;

/// `FcitxKey_q`, the letter of the stroke that leads nowhere.
const KEY_Q: u32 = 0x0071;

/// `FcitxKey_k`, the prefix of the sequence the bridge test binds.
const KEY_K: u32 = 0x006b;

/// `FcitxKey_s`, the stroke that completes it.
const KEY_S: u32 = 0x0073;

/// A stroke of `Ctrl` plus `sym`.
///
/// A function rather than a constant: both strokes the bridge test presses are built the
/// same way, and one of them is used twice.
fn ctrl(sym: u32) -> SequencePrefix {
    SequencePrefix { sym, state: CTRL }
}

// ── The doubles ──────────────────────────────────────────────────────────────────

/// One entry of the in-memory dictionary.
struct Entry {
    /// The `'`-separated syllable string the entry answers to.
    key: &'static str,
    /// The word the entry holds.
    text: &'static str,
    /// Ranking weight within one key, higher first.
    weight: u32,
    /// How many syllables the word consumes.
    syl_count: u8,
}

/// The dictionary these tests decode against.
///
/// Eight readings of `ni` and three of `hao`: the first is more than one page of five, so the
/// page keys, the highlight moves and the digit row have something to act on, and the second
/// is fewer than the digit row names, so a digit that names nothing is reachable. There is
/// no single-character fallback, so nothing here invents a candidate the rows did not
/// declare.
struct TestLexicon {
    /// Every entry, in the order it was declared.
    words: Vec<Entry>,
}

impl TestLexicon {
    /// Builds a dictionary from `(key, word, weight, syllable count)` rows.
    fn new(rows: &[(&'static str, &'static str, u32, u8)]) -> Self {
        let mut words = Vec::with_capacity(rows.len());
        for &(key, text, weight, syl_count) in rows {
            words.push(Entry {
                key,
                text,
                weight,
                syl_count,
            });
        }
        Self { words }
    }
}

impl Lexicon for TestLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = self
            .words
            .iter()
            .filter(|entry| entry.key == key)
            .map(|entry| WordRef {
                text: entry.text,
                weight: entry.weight,
                syl_count: entry.syl_count,
                flags: WordFlags::empty(),
            })
            .collect();
        Ok(WordIter::from_vec(words))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }
}

/// A user-frequency source that answers nothing.
///
/// Nothing the arbitrator asks reaches the user's frequencies: the question is what a step
/// *would* do, and no step is taken.
struct SilentUser;

impl UserFreqSource for SilentUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// The decoding sources the sessions are built against.
struct Fixture {
    /// The dictionary the decode reads.
    lexicon: TestLexicon,
    /// The user frequencies, which no test here writes to.
    user: SilentUser,
    /// The language model the ranking is scored with.
    lm: InMemoryLm,
    /// The decoder, built with the shipped configuration.
    decoder: Decoder,
}

impl Fixture {
    /// Builds the doubles.
    fn new() -> Self {
        Self {
            lexicon: TestLexicon::new(&[
                ("ni", "你", 900_000, 1),
                ("ni", "尼", 800_000, 1),
                ("ni", "泥", 700_000, 1),
                ("ni", "拟", 600_000, 1),
                ("ni", "逆", 500_000, 1),
                ("ni", "腻", 400_000, 1),
                ("ni", "妮", 300_000, 1),
                ("ni", "霓", 200_000, 1),
                ("hao", "好", 900_000, 1),
                ("hao", "号", 800_000, 1),
                ("hao", "耗", 700_000, 1),
            ]),
            user: SilentUser,
            lm: InMemoryLm::new(),
            decoder: Decoder::default(),
        }
    }

    /// The sources as the environment a session step reads.
    fn env(&self) -> SessionEnv<'_> {
        SessionEnv {
            decoder: &self.decoder,
            lexicon: &self.lexicon,
            user_freq: &self.user,
            lm: &self.lm,
        }
    }

    /// A composing session with `raw` typed into it.
    ///
    /// The session is driven the way a user drives one — one character at a time, through
    /// the state machine — so the candidates, the paging state and the highlight are the
    /// ones a real composition holds rather than ones written in by hand.
    fn composing(&self, raw: &str) -> Session {
        let cfg = SessionConfig::default();
        let mut session = Session::new();
        for ch in raw.chars() {
            let effects = session.handle_key(KeyAction::InputChar(ch), &cfg, &self.env());
            assert!(!effects.is_empty(), "typing {ch:?} composes");
        }
        assert_eq!(session.state, SessionState::Composing);
        session
    }

    /// A session whose composition is being taken back.
    fn cancelling(&self) -> Session {
        let mut session = self.composing("ni");
        let effects = self.step(&mut session, KeyAction::Escape);
        assert!(!effects.is_empty(), "cancelling acts on the composition");
        assert_eq!(session.state, SessionState::Cancelling);
        session
    }

    /// A session waiting for a commit to land.
    fn committing(&self) -> Session {
        let mut session = self.composing("ni");
        let effects = self.step(&mut session, KeyAction::CommitHighlighted);
        assert!(!effects.is_empty(), "committing acts on the composition");
        assert_eq!(session.state, SessionState::Committing);
        session
    }

    /// Steps one session with one key action and returns the effects it produced.
    fn step(&self, session: &mut Session, action: KeyAction) -> ime_core::state::Effects {
        step(
            session,
            SessionEvent::Key(action),
            &SessionConfig::default(),
            &self.env(),
        )
    }
}

// ── The setups and the corpus ────────────────────────────────────────────────────

/// The states the mirror and the sweeps run over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// A session with nothing composing.
    Idle,
    /// A live composition whose readings are more than one page of five.
    Composing,
    /// A live composition whose readings are fewer than the digit row names.
    ComposingShort,
    /// A composition being taken back.
    Cancelling,
    /// A commit in flight.
    Committing,
}

impl Setup {
    /// Every setup, for the sweeps that have to cover all of them.
    const ALL: [Setup; 5] = [
        Setup::Idle,
        Setup::Composing,
        Setup::ComposingShort,
        Setup::Cancelling,
        Setup::Committing,
    ];

    /// A session in this state.
    fn session(self, fixture: &Fixture) -> Session {
        match self {
            Setup::Idle => Session::new(),
            Setup::Composing => fixture.composing("ni"),
            Setup::ComposingShort => fixture.composing("hao"),
            Setup::Cancelling => fixture.cancelling(),
            Setup::Committing => fixture.committing(),
        }
    }

    /// The name a failure message uses.
    fn label(self) -> &'static str {
        match self {
            Setup::Idle => "idle",
            Setup::Composing => "composing",
            Setup::ComposingShort => "composing-short",
            Setup::Cancelling => "cancelling",
            Setup::Committing => "committing",
        }
    }
}

/// Every action the frozen enum has, with payloads that reach the interesting branches.
///
/// Written out rather than derived, because the payloads are what makes a row interesting: a
/// letter and a digit take different paths through the input alphabet, a digit that names a
/// candidate and one that names none take different paths through the page, and a caret move
/// acts only away from the ends of the input.
fn actions() -> Vec<KeyAction> {
    vec![
        KeyAction::InputChar('a'),
        KeyAction::InputChar('\''),
        KeyAction::InputChar('1'),
        KeyAction::InputChar('中'),
        KeyAction::Backspace,
        KeyAction::CommitHighlighted,
        KeyAction::CommitRaw,
        KeyAction::SelectIndex(1),
        KeyAction::SelectIndex(5),
        KeyAction::SelectIndex(9),
        KeyAction::PageNext,
        KeyAction::PagePrev,
        KeyAction::PageFirst,
        KeyAction::PageLast,
        KeyAction::MoveHighlight(1),
        KeyAction::MoveHighlight(-1),
        KeyAction::MoveCaret(1),
        KeyAction::MoveCaret(-1),
        KeyAction::ToggleLang,
        KeyAction::ToggleFullWidth,
        KeyAction::TogglePunct,
        KeyAction::EnterTempEnglish,
        KeyAction::Escape,
        KeyAction::Ignore,
        KeyAction::ToggleScript,
        KeyAction::ForgetHighlighted,
        KeyAction::PinHighlighted,
        KeyAction::AddPhrase,
    ]
}

/// The composing keymap: every key the composition layer claims, with its modifiers.
///
/// Driven through the routing table rather than listed as actions, so a row that changes
/// what a key means is caught here rather than assumed.
fn composing_keymap() -> Vec<(u32, u32)> {
    let mut keys = vec![
        (KEY_SPACE, 0),
        (KEY_RETURN, 0),
        (KEY_BACKSPACE, 0),
        (KEY_ESCAPE, 0),
        (KEY_TAB, 0),
        (KEY_TAB, SHIFT),
        (KEY_MINUS, 0),
        (KEY_EQUAL, 0),
        (KEY_LEFT, 0),
        (KEY_RIGHT, 0),
        (KEY_UP, 0),
        (KEY_DOWN, 0),
        (KEY_HOME, 0),
        (KEY_END, 0),
        // The user-word chord: a fixed row of the table, with the exact modifier set
        // the row's `Accepts` names.
        (KEY_DELETE, CTRL),
    ];
    keys.extend((KEY_1..=KEY_9).map(|sym| (sym, 0)));
    keys
}

/// The keysyms the sweeps run over: every row the table reads, plus a deterministic
/// pseudo-random tail so a row added to the table is exercised without anybody remembering.
fn keysym_corpus() -> Vec<u32> {
    let mut syms: Vec<u32> = (0x20..=0x100).collect();
    syms.extend(pseudo_random_syms(100));
    syms
}

/// A deterministic keysym stream from a 32-bit LCG: reproducible, and needs no dependency
/// (`rand` is not one of this workspace's crates).
fn pseudo_random_syms(count: usize) -> Vec<u32> {
    let mut seed: u32 = 0x1234_5678;
    (0..count)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        })
        .collect()
}

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 7,
    }
}

/// The action the routing table gives one key.
fn action_of(sym: u32, state: u32) -> KeyAction {
    let key = press(sym, state);
    translate_key(&key.as_host_event(), &KeyBindings::default())
}

/// Whether a step did something the plugin keeps a key for.
///
/// The reading the arbitrator is compared against: an effect other than a diagnostic, or the
/// one state change that produces no effect at all — the chord that turns temporary English
/// on, which the engine reads off the session to decide the key is its own.
fn step_acted(session: &Session, effects: &[Effect], was_temp_english: bool) -> bool {
    let reached_host = effects.iter().any(is_not_a_diagnostic);
    reached_host || (!was_temp_english && session.temp_english)
}

/// Whether an effect is anything other than a diagnostic.
///
/// A step that only reports something did nothing the user can see, which is why such a key
/// has to reach the application rather than being kept.
fn is_not_a_diagnostic(effect: &Effect) -> bool {
    // The mode flash is the idle switch's one product and the executor turns it into a
    // diagnostic line, so it counts as a report here exactly as `Diagnose` does.
    !matches!(effect, Effect::Diagnose(_) | Effect::ModeFlash { .. })
}
