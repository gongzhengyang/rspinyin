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

use ime_core::lm::InMemoryLm;
use ime_core::state::{Effect, Session, SessionConfig, SessionEnv, SessionEvent};
use ime_core::state::{SessionState, step};
use ime_core::viterbi::Decoder;
use ime_types::{
    ImeError, KeyAction, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef,
};

use super::{Executability, arbitrate, arbitrate_sequence, executability, is_mode_chord};
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

// ── The arbitration ──────────────────────────────────────────────────────────────

/// With nothing composing, every key of the composing keymap belongs to the application.
///
/// The table names all of them, so this is exactly the defect the arbitrator closes: a
/// plugin that kept them would eat the space bar, the digits, `Return`, `BackSpace`,
/// `Escape`, `Tab` and the arrows in every application while nothing is composing.
#[test]
fn test_arbitrate_idle_hands_the_composing_keymap_back() {
    let cfg = SessionConfig::default();
    let session = Session::new();
    for (sym, state) in composing_keymap() {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "idle {sym:#x} with {state:#x} must reach the application"
        );
    }
}

/// While a composition is live, the keys it can act on are the plugin's.
#[test]
fn test_arbitrate_composing_keeps_the_keys_the_session_acts_on() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    assert!(
        session.decoded().candidates.len() > 5,
        "the fixture offers a second page"
    );
    let keys = [
        (KEY_SPACE, 0),
        (KEY_RETURN, 0),
        (KEY_BACKSPACE, 0),
        (KEY_ESCAPE, 0),
        (KEY_TAB, 0),
        (KEY_LEFT, 0),
        (KEY_EQUAL, 0),
        (KEY_DOWN, 0),
        (KEY_1, 0),
        (KEY_5, 0),
    ];
    for (sym, state) in keys {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Consumed,
            "composing {sym:#x} with {state:#x} is the plugin's"
        );
    }
}

/// A routed key the session cannot act on belongs to the application.
///
/// The other half of the same rule, and the reason a composition is not simply a keyboard
/// grab: a digit past the end of the page, a page key with no page to turn to, and an arrow
/// pointing off the end of the input all do nothing, so the application receives them.
#[test]
fn test_arbitrate_composing_hands_back_the_keys_that_would_do_nothing() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = fixture.composing("ni");
    assert!(session.decoded().candidates.len() > 5, "a page of five");
    assert_eq!(session.paging.page, 0, "on the first page");
    let keys = [
        (KEY_9, 0, "a digit past the end of the page"),
        (KEY_UP, 0, "the first page has nothing before it"),
        (KEY_MINUS, 0, "and no page to turn back to"),
        (KEY_TAB, SHIFT, "the highlight is on the first candidate"),
        (KEY_RIGHT, 0, "the caret is already at the end of the input"),
    ];
    for (sym, state, why) in keys {
        let action = action_of(sym, state);
        assert_ne!(action, KeyAction::Ignore, "the table names {sym:#x}");
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "{why}: {sym:#x}"
        );
    }
}

/// The key the table does not name is never arbitrated.
#[test]
fn test_arbitrate_ignores_the_action_the_table_does_not_name() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    for setup in Setup::ALL {
        let session = setup.session(&fixture);
        assert_eq!(
            arbitrate(KeyAction::Ignore, Some(&session), &cfg),
            Consumed::Ignored,
            "{} has nothing to arbitrate about an unnamed key",
            setup.label()
        );
    }
}

/// A key for an input context with no session belongs to the application.
#[test]
fn test_arbitrate_without_a_session_ignores_every_action() {
    let cfg = SessionConfig::default();
    for action in actions() {
        assert_eq!(
            arbitrate(action, None, &cfg),
            Consumed::Ignored,
            "{action:?} has nowhere to go without a session"
        );
    }
}

/// The engine's own mode chords are the plugin's with nothing composing.
///
/// The two switches the routing table still claims change bits the engine owns, so they
/// act with no session state to read; the layer that consumes them is the engine's, which
/// is why they are not asked of the session. The language switch is not among them: its
/// chord is the host's own hotkey, and the action stays in the vocabulary for the day a
/// host carries the state the switch would write.
#[test]
fn test_arbitrate_claims_the_mode_chords_with_nothing_composing() {
    let cfg = SessionConfig::default();
    let session = Session::new();
    let chords = [
        (KEY_SPACE, SHIFT, KeyAction::ToggleFullWidth),
        (KEY_PERIOD, CTRL, KeyAction::TogglePunct),
    ];
    for (sym, state, expected) in chords {
        assert_eq!(action_of(sym, state), expected, "the table names it");
        assert_eq!(
            arbitrate(expected, Some(&session), &cfg),
            Consumed::Consumed,
            "the engine owns {expected:?}"
        );
    }
}

/// Temporary English hands every key to the application, mode chords included.
///
/// The `Return` and `Escape` that leave the mode change no other state and produce no
/// effect, so keeping them would take a keystroke the user typed for the application.
#[test]
fn test_arbitrate_hands_every_key_back_in_temporary_english() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut session = fixture.composing("ni");
    session.temp_english = true;

    for action in actions() {
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            Consumed::Ignored,
            "temporary English hands {action:?} to the application"
        );
    }
}

/// A session waiting for the host to finish drops every key, the engine's switches aside.
///
/// The three mode bits are the engine's rather than the session's, so they are answered
/// before the session is asked at all: a user who switches the language while a commit is
/// landing still gets the switch, and the composition the host is finishing is untouched.
#[test]
fn test_arbitrate_hands_every_key_back_while_the_host_finishes() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    for setup in [Setup::Cancelling, Setup::Committing] {
        let session = setup.session(&fixture);
        for action in actions() {
            let expected = if is_mode_chord(action) {
                Consumed::Consumed
            } else {
                Consumed::Ignored
            };
            assert_eq!(
                arbitrate(action, Some(&session), &cfg),
                expected,
                "{} with {action:?}",
                setup.label()
            );
        }
    }
}

/// A key the arbitrator keeps must be a key something acts on.
///
/// The sweep runs over every row the table reads plus a pseudo-random tail: whenever the
/// verdict is `Consumed`, stepping the session with the action it named produced a
/// non-diagnostic effect, or the action is one of the engine's own mode bits, which the
/// engine acts on outside the session. Nothing else may be swallowed.
#[test]
fn test_arbitrate_never_keeps_a_key_the_session_would_not_act_on() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut kept = 0usize;
    for sym in keysym_corpus() {
        for setup in Setup::ALL {
            let mut session = setup.session(&fixture);
            let action = action_of(sym, 0);
            let verdict = arbitrate(action, Some(&session), &cfg);
            let effects = fixture.step(&mut session, action);
            let acted = step_acted(&session, &effects, false);

            assert!(
                verdict == Consumed::Ignored || acted || is_mode_chord(action),
                "{} keeps {sym:#x} as {action:?} but nothing acts on it",
                setup.label()
            );
            kept += usize::from(verdict == Consumed::Consumed);
        }
    }
    assert!(kept > 0, "the sweep exercised the keeping path");
}

/// The claim table of the design, row by row, driven through the routing table.
#[test]
fn test_arbitrate_matches_the_claim_table() {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    assert!(
        fixture.composing("hao").decoded().candidates.len() < 5,
        "the short fixture offers fewer than five readings"
    );
    let rows = [
        (KEY_SPACE, 0, Setup::Idle, Consumed::Ignored, "idle space"),
        (KEY_3, 0, Setup::Idle, Consumed::Ignored, "idle digit"),
        (KEY_RETURN, 0, Setup::Idle, Consumed::Ignored, "idle return"),
        (
            KEY_BACKSPACE,
            0,
            Setup::Idle,
            Consumed::Ignored,
            "idle backspace",
        ),
        (KEY_ESCAPE, 0, Setup::Idle, Consumed::Ignored, "idle escape"),
        (KEY_UP, 0, Setup::Idle, Consumed::Ignored, "idle up"),
        (
            KEY_SPACE,
            0,
            Setup::Composing,
            Consumed::Consumed,
            "composing space commits the highlight",
        ),
        (
            KEY_5,
            0,
            Setup::ComposingShort,
            Consumed::Ignored,
            "a digit that names no candidate",
        ),
        (
            KEY_E,
            CTRL | SHIFT,
            Setup::Composing,
            Consumed::Consumed,
            "temporary English is entered",
        ),
        (
            KEY_Q,
            CTRL,
            Setup::Composing,
            Consumed::Ignored,
            "nobody's chord",
        ),
    ];
    for (sym, state, setup, expected, what) in rows {
        let action = action_of(sym, state);
        let session = setup.session(&fixture);
        assert_eq!(
            arbitrate(action, Some(&session), &cfg),
            expected,
            "{what}: {sym:#x} with {state:#x} as {action:?}"
        );
    }
}

// ── The engine's own domain, and the sequence bridge ─────────────────────────────

/// Exactly the three mode bits are the engine's, and nothing else is.
#[test]
fn test_is_mode_chord_names_exactly_the_three_engine_bits() {
    let named: Vec<KeyAction> = actions()
        .into_iter()
        .filter(|action| is_mode_chord(*action))
        .collect();
    assert_eq!(
        named,
        vec![
            KeyAction::ToggleLang,
            KeyAction::ToggleFullWidth,
            KeyAction::TogglePunct
        ],
        "the three mode bits and nothing else"
    );
}

/// Every answer the sequence machine gives has a place in the walk's vocabulary.
///
/// The decisions come out of the machine rather than out of a literal, so the bridge is
/// tested against what the machine actually answers.
#[test]
fn test_arbitrate_sequence_maps_every_decision() {
    let ctrl_k = ctrl(KEY_K);
    let ctrl_s = ctrl(KEY_S);
    let mut table = SequenceTable::new();
    assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    let mut machine = KeySequence::new();

    // A prefix stroke: ours from that moment, nothing executed yet.
    let opened = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&opened), Consumed::ChainPending);
    // The stroke that ends it.
    let completed = machine.offer(&press(ctrl_s.sym, ctrl_s.state), &table);
    assert_eq!(arbitrate_sequence(&completed), Consumed::Consumed);
    // A stroke with nothing in flight and no sequence to open.
    let pass = machine.offer(&press(KEY_Q, CTRL), &table);
    assert_eq!(arbitrate_sequence(&pass), Consumed::Ignored);
    // A prefix, then a stroke that leads nowhere: the second one travels on.
    let reopened = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&reopened), Consumed::ChainPending);
    let abandoned = machine.offer(&press(KEY_Q, CTRL), &table);
    assert_eq!(arbitrate_sequence(&abandoned), Consumed::Ignored);
    // `Escape` while a sequence is open: ours, and it cancels.
    let resend = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&resend), Consumed::ChainPending);
    let cancelled = machine.offer(&press(KEY_ESCAPE, 0), &table);
    assert_eq!(arbitrate_sequence(&cancelled), Consumed::Consumed);
}

/// The bridge keeps a key exactly when the sequence machine says it does.
///
/// The two are written independently — one as a match over this crate's vocabulary, the
/// other as a method on the decision — so a decision added to the machine is caught here
/// rather than silently mapped to "the application's".
#[test]
fn test_arbitrate_sequence_agrees_with_keeps_key() {
    let ctrl_k = ctrl(KEY_K);
    let ctrl_s = ctrl(KEY_S);
    let mut table = SequenceTable::new();
    assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    let mut machine = KeySequence::new();
    let strokes = [
        (ctrl_k.sym, ctrl_k.state),
        (ctrl_s.sym, ctrl_s.state),
        (KEY_Q, CTRL),
        (ctrl_k.sym, ctrl_k.state),
        (KEY_ESCAPE, 0),
        (KEY_Q, CTRL),
    ];
    let mut seen = 0usize;
    for (sym, state) in strokes {
        let decision = machine.offer(&press(sym, state), &table);
        let kept = arbitrate_sequence(&decision) != Consumed::Ignored;
        assert_eq!(kept, decision.keeps_key(), "{sym:#x} {decision:?}");
        seen += 1;
    }
    assert_eq!(seen, strokes.len(), "every stroke was offered");
}
