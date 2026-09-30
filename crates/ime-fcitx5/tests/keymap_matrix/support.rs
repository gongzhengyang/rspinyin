//! The doubles the keymap matrix runs against.
//!
//! Nothing here reads a file, a clock, an environment variable or a display server: the
//! dictionary, the user frequencies and the language model are in-memory values, the
//! configuration is the shipped default, and the host is a recorder. That is what lets the
//! whole matrix be checked on a machine with no Fcitx5 session and no candidate window.
//!
//! # What each piece is for
//!
//! * [`Fixture`] pairs one in-memory dictionary with the decoder and the routing
//!   configuration a case routes keys under. It can build either of the two things the
//!   matrix drives: a [`KeyRouter`], which is the production path from a key event to the
//!   effects a session produces, and a bare [`Session`], which is what the layered bus
//!   arbitrates against.
//! * [`RecordingHost`] is the boundary the routing layer executes its effects against. It
//!   keeps what the host was handed, so a case can assert on what the user would have seen
//!   rather than on an internal flag.
//! * [`Situation`] names the three session contexts the shortcut table distinguishes.
//! * [`Lcg`] is the deterministic source the never-swallow corpus is drawn from. The
//!   corpus has to be the same on every run and in every process, so it cannot come from a
//!   random-number crate, and the workspace forbids adding one for a test's convenience.

use ime_core::lm::InMemoryLm;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::{Session, SessionEnv, SessionEvent, step};
use ime_core::viterbi::Decoder;
use ime_types::{
    ImeError, Lexicon, PageState, SyllableId, UiCommand, UserFreqSource, WordFlags, WordIter,
    WordRef,
};
use rspinyin::engine::host::Host;
use rspinyin::engine::{KeyBindings, KeyRouter, RoutingConfig, translate_key};
use rspinyin::ffi::FcitxKeyEvent;
use rspinyin::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};

// ── The keysym vocabulary ────────────────────────────────────────────────────────
//
// The values are the XKB keysyms the routing table matches on, spelled out here rather
// than imported: the table's constants are private to the crate, and a test that reached
// into them could not notice the two drifting apart.

/// `FcitxKey_space`.
pub const KEY_SPACE: u32 = 0x0020;
/// `FcitxKey_apostrophe`, the syllable separator the input alphabet accepts.
pub const KEY_APOSTROPHE: u32 = 0x0027;
/// `FcitxKey_minus`, the shipped configuration's "page back" key.
pub const KEY_MINUS: u32 = 0x002d;
/// `FcitxKey_period`, the key of the punctuation chord.
pub const KEY_PERIOD: u32 = 0x002e;
/// `FcitxKey_slash`, the key of the command-palette chord.
pub const KEY_SLASH: u32 = 0x002f;
/// `FcitxKey_0`, the low end of the digit row.
pub const KEY_0: u32 = 0x0030;
/// `FcitxKey_1`, the low end of the selectable digits.
pub const KEY_1: u32 = 0x0031;
/// `FcitxKey_9`, the high end of the digit row.
pub const KEY_9: u32 = 0x0039;
/// `FcitxKey_equal`, the shipped configuration's "page forward" key.
pub const KEY_EQUAL: u32 = 0x003d;
/// `FcitxKey_at`, the keysym immediately below the uppercase letter row.
pub const KEY_AT: u32 = 0x0040;
/// `FcitxKey_A`, the uppercase shape a host that folds the case into the symbol delivers.
pub const KEY_A_UPPER: u32 = 0x0041;
/// `FcitxKey_Z`, the high end of the uppercase shape.
pub const KEY_Z_UPPER: u32 = 0x005a;
/// `FcitxKey_bracketleft`, the keysym immediately above the uppercase letter row.
pub const KEY_BRACKET_LEFT: u32 = 0x005b;
/// `FcitxKey_a`, the low end of the letter row.
pub const KEY_A: u32 = 0x0061;
/// `FcitxKey_e`, the letter of the temporary-English chord.
pub const KEY_E: u32 = 0x0065;
/// `FcitxKey_i`.
pub const KEY_I: u32 = 0x0069;
/// `FcitxKey_n`.
pub const KEY_N: u32 = 0x006e;
/// `FcitxKey_p`, the letter of the diagnostics-panel chord.
pub const KEY_P: u32 = 0x0070;
/// `FcitxKey_z`, the high end of the letter row.
pub const KEY_Z: u32 = 0x007a;
/// `FcitxKey_BackSpace`.
pub const KEY_BACKSPACE: u32 = 0xff08;
/// `FcitxKey_Tab`.
pub const KEY_TAB: u32 = 0xff09;
/// `FcitxKey_Return`.
pub const KEY_RETURN: u32 = 0xff0d;
/// `FcitxKey_Escape`.
pub const KEY_ESCAPE: u32 = 0xff1b;
/// `FcitxKey_Left`.
pub const KEY_LEFT: u32 = 0xff51;
/// `FcitxKey_Up`.
pub const KEY_UP: u32 = 0xff52;
/// `FcitxKey_Right`.
pub const KEY_RIGHT: u32 = 0xff53;
/// `FcitxKey_Down`.
pub const KEY_DOWN: u32 = 0xff54;
/// `FcitxKey_Page_Up`.
pub const KEY_PAGE_UP: u32 = 0xff55;
/// `FcitxKey_Page_Down`.
pub const KEY_PAGE_DOWN: u32 = 0xff56;
/// `FcitxKey_Shift_L`.
pub const KEY_SHIFT_L: u32 = 0xffe1;
/// `FcitxKey_Shift_R`.
pub const KEY_SHIFT_R: u32 = 0xffe2;

// ── The modifier vocabulary ──────────────────────────────────────────────────────
//
// `fcitx::KeyState`'s bits, taken from the installed Fcitx5 header. The routing layer's
// copy of them is private, so these are the second, independent reading of the same
// header: a drift between the two shows up as a wrong verdict rather than as a shared
// constant that moved on both sides at once.

/// `fcitx::KeyState::Shift`.
pub const SHIFT: u32 = 1 << 0;
/// `fcitx::KeyState::Ctrl`.
pub const CTRL: u32 = 1 << 2;
/// `fcitx::KeyState::Alt`, the `Mod1` alias.
pub const ALT: u32 = 1 << 3;
/// `fcitx::KeyState::Hyper`, the `Mod3` alias.
pub const HYPER: u32 = 1 << 5;
/// `fcitx::KeyState::Super`, the `Mod4` alias.
pub const SUPER: u32 = 1 << 6;
/// `fcitx::KeyState::Super2`, GTK's virtual Super.
pub const SUPER2: u32 = 1 << 26;
/// `fcitx::KeyState::Meta`.
pub const META: u32 = 1 << 28;

/// The modifiers a desktop environment owns, which the plugin must never take.
///
/// Every one of them belongs to the session's window manager rather than to an input
/// method: a key carrying one of them has to reach the application whatever else it holds.
pub const DESKTOP_MODIFIERS: u32 = HYPER | SUPER | SUPER2 | META;

/// The input context every case routes keys for.
pub const IC: u64 = 1;

/// The timestamp every synthetic key carries.
///
/// The routing table reads nothing from it, and a test must not read a clock, so a
/// constant is both sufficient and the only admissible value.
pub const TIME_MS: u32 = 7;

// ── Strokes ──────────────────────────────────────────────────────────────────────

/// One key press of `sym` with `state` held.
pub fn press(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: TIME_MS,
    }
}

/// One key release of `sym`.
pub fn release(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: true,
        time_ms: TIME_MS,
    }
}

// ── The session contexts ─────────────────────────────────────────────────────────

/// Which session context a case is checked in.
///
/// The three the shortcut table distinguishes, and the three a key can arrive in. The
/// whole point of the table is that the same keysym means different things in each of
/// them: a `Space` with a composition live commits a candidate, and the same `Space` with
/// nothing composing is the application's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Situation {
    /// A session exists and holds nothing.
    Idle,
    /// A composition is live, with more than one page of candidates behind it.
    Composing,
    /// Temporary English is on, which hands every key to the application.
    TempEnglish,
}

/// The keystrokes that put a session into `situation`.
///
/// Typed as strokes rather than as actions so that the setup goes through the same
/// translation the case under test does; a setup that injected `KeyAction`s would be
/// exercising a path the product does not have.
fn setup_strokes(situation: Situation) -> &'static [(u32, u32)] {
    match situation {
        Situation::Idle => &[],
        Situation::Composing => &[(KEY_N, 0), (KEY_I, 0)],
        Situation::TempEnglish => &[(KEY_E, CTRL | SHIFT)],
    }
}

// ── The in-memory data ───────────────────────────────────────────────────────────

/// One entry of the in-memory dictionary.
pub struct Entry {
    /// The `'`-separated syllable string the entry answers to.
    pub key: &'static str,
    /// The word the entry holds.
    pub text: &'static str,
    /// Ranking weight within one key, higher first.
    pub weight: u32,
    /// How many syllables the word consumes.
    pub syl_count: u8,
}

/// The dictionary every case decodes against.
///
/// `ni` carries eight readings, which is more than a page holds, so the page keys have
/// somewhere to go and the digits have a boundary to fall off: `1` names a candidate and
/// `9` names none. A dictionary with fewer readings than a page holds would make every
/// page key inert and the matrix would be asserting nothing about them.
pub const DICTIONARY: &[(&str, &str, u32, u8)] = &[
    ("ni", "你", 900_000, 1),
    ("ni", "尼", 800_000, 1),
    ("ni", "泥", 700_000, 1),
    ("ni", "拟", 600_000, 1),
    ("ni", "逆", 500_000, 1),
    ("ni", "匿", 400_000, 1),
    ("ni", "腻", 300_000, 1),
    ("ni", "溺", 200_000, 1),
    ("ni'hao", "你好", 900_000, 2),
];

/// The dictionary the cases decode against, in the shape [`Lexicon`] wants.
pub struct TestLexicon {
    /// Every entry, in the order it was declared.
    pub entries: Vec<Entry>,
}

impl TestLexicon {
    /// Builds a dictionary from `(key, word, weight, syllable count)` rows.
    pub fn new(rows: &[(&'static str, &'static str, u32, u8)]) -> Self {
        let entries = rows
            .iter()
            .map(|&(key, text, weight, syl_count)| Entry {
                key,
                text,
                weight,
                syl_count,
            })
            .collect();
        Self { entries }
    }
}

impl Lexicon for TestLexicon {
    /// Answers with the entries stored under `key`, in the order they were declared.
    ///
    /// # Errors
    ///
    /// Never: the table is in memory and every key is answered, an unknown one with an
    /// empty list.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = self
            .entries
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

    /// Answers `Unsupported`: prefix enumeration is a Phase 2 capability.
    ///
    /// # Errors
    ///
    /// Always, with the code the frozen contract reserves for a capability this build
    /// phase does not have.
    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    /// Answers with no single-character fallback at all.
    ///
    /// Deliberate: a fallback would add a candidate the rows never declared, and the
    /// matrix's page arithmetic is stated in terms of the readings above.
    ///
    /// # Errors
    ///
    /// Never.
    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }
}

/// A user-frequency source that has learned nothing and records nothing.
///
/// The matrix drives keys and reads frames; nothing in it commits a candidate and then
/// asserts on the learned frequency, so a source that answers "never recorded" is the
/// whole of what the decode needs.
pub struct SilentUser;

impl UserFreqSource for SilentUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

// ── The host double ──────────────────────────────────────────────────────────────

/// The host boundary, keeping what it was handed.
///
/// A key the plugin keeps is a key whose text the user will not have to retype, so what a
/// case asserts on is what reached the host rather than what the routing layer decided
/// internally. The recorder therefore counts every call that is not a diagnostic, and
/// keeps the newest frame so a case can read the composing text and the page back.
pub struct RecordingHost {
    /// Every non-diagnostic call, counted.
    pub calls: usize,
    /// The text of every commit, in order.
    pub commits: Vec<String>,
    /// How many candidates the newest frame holds.
    pub newest_candidates: usize,
    /// The paging state of the newest frame.
    ///
    /// The frame carries the page on show rather than the whole candidate list, so the
    /// number of pages is what says whether a page key has anywhere to go.
    pub newest_page: Option<PageState>,
    /// The composing text of the newest frame.
    pub newest_preedit: Option<String>,
    /// How many times the host's input-method state was flipped.
    pub toggles: usize,
    /// Whether the host has the input method enabled.
    pub is_enabled: bool,
    /// The rendered code of every diagnostic, in order.
    pub diagnostics: Vec<String>,
}

impl Default for RecordingHost {
    /// A host whose input context is enabled, which is the state a context is activated
    /// in, and which has been handed nothing.
    fn default() -> Self {
        Self {
            calls: 0,
            commits: Vec::new(),
            newest_candidates: 0,
            newest_page: None,
            newest_preedit: None,
            toggles: 0,
            is_enabled: true,
            diagnostics: Vec::new(),
        }
    }
}

impl Host for RecordingHost {
    fn commit(&mut self, _ic: u64, text: &str) {
        self.calls += 1;
        self.commits.push(text.to_owned());
    }

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {
        self.calls += 1;
    }

    fn clear_preedit(&mut self, _ic: u64) {
        self.calls += 1;
    }

    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        self.calls += 1;
        if let UiCommand::Frame(frame) = command {
            self.newest_candidates = frame.candidates.len();
            self.newest_page = Some(frame.page);
            self.newest_preedit = Some(frame.preedit.text.clone());
        }
    }

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        self.calls += 1;
        self.toggles += 1;
        self.is_enabled = !self.is_enabled;
        self.is_enabled
    }

    fn diagnose(&mut self, _ic: u64, err: &ImeError) {
        self.diagnostics.push(err.to_string());
    }
}

// ── The fixture ──────────────────────────────────────────────────────────────────

/// What the host reports about an ordinary input context.
///
/// No program name, neither the password flag nor the sensitive one: the context a case
/// types into is a text field like any other. An unreported context would instead be
/// treated as a password box, and every activation would report that it suppresses
/// learning on the crash channel.
fn ordinary_report() -> ContextReport<'static> {
    ContextReport::Reported {
        program: None,
        password: false,
        sensitive: false,
    }
}

/// One in-memory dictionary with the decoder and the configuration a case routes under.
pub struct Fixture {
    /// The dictionary the decode reads.
    pub lexicon: TestLexicon,
    /// The user frequencies the commit path writes to.
    pub user: SilentUser,
    /// The language model the ranking is scored with.
    pub lm: InMemoryLm,
    /// The decoder under test, built from the shipped configuration.
    pub decoder: Decoder,
    /// The configuration in force, which a case may vary per key binding.
    pub config: RoutingConfig,
}

impl Default for Fixture {
    /// A fixture over [`DICTIONARY`] with the shipped configuration.
    fn default() -> Self {
        Self {
            lexicon: TestLexicon::new(DICTIONARY),
            user: SilentUser,
            lm: InMemoryLm::new(),
            decoder: Decoder::default(),
            config: RoutingConfig::default(),
        }
    }
}

impl Fixture {
    /// The sources as the environment a session step reads.
    pub fn env(&self) -> SessionEnv<'_> {
        SessionEnv {
            decoder: &self.decoder,
            lexicon: &self.lexicon,
            user_freq: &self.user,
            lm: &self.lm,
        }
    }

    /// A router over this fixture's sources, with `keys` in force.
    ///
    /// The context is activated, so the router can route a key the moment it is returned,
    /// and it is reported as an ordinary context rather than as an unreported one: an
    /// unreported context is fail-closed and reports that it suppresses learning, which
    /// would put a diagnostic on the crash channel for every case in this file. The
    /// matrix never asserts on a learned frequency, so which of the two answers the
    /// privacy policy gives is not part of what a case checks.
    pub fn router(&self, keys: KeyBindings) -> KeyRouter<'_> {
        let mut config = self.config;
        config.keys = keys;
        let privacy =
            ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default());
        let mut router = KeyRouter::new(self.env(), privacy, config);
        router.activate_reported(IC, ordinary_report());
        router
    }

    /// A router driven into `situation`, with `host` receiving the setup's effects.
    ///
    /// The setup is typed through the router rather than injected as actions, so a case
    /// begins from a state the production path actually reaches.
    pub fn router_in(
        &self,
        keys: KeyBindings,
        situation: Situation,
        host: &mut RecordingHost,
    ) -> KeyRouter<'_> {
        let mut router = self.router(keys);
        for &(sym, state) in setup_strokes(situation) {
            router.key_event(IC, &press(sym, state), host);
        }
        router
    }

    /// A session stepped into `situation`, for the layered bus.
    ///
    /// The setup goes through the routing table rather than naming the actions directly,
    /// so the session and the router reach the same state by the same route.
    pub fn session_in(&self, situation: Situation) -> Session {
        let env = self.env();
        let cfg = self.config.session;
        let mut session = Session::new();
        for &(sym, state) in setup_strokes(situation) {
            let action = translate_key(&press(sym, state), &KeyBindings::default());
            step(&mut session, SessionEvent::Key(action), &cfg, &env);
        }
        session
    }
}

// ── The deterministic source ─────────────────────────────────────────────────────

/// A 64-bit linear congruential generator, for the never-swallow corpus.
///
/// The constants are Knuth's. The corpus must be identical on every run and in every
/// process, which is what a seeded generator gives and what a random-number crate would
/// take away; the workspace forbids adding a dependency for a test's convenience in any
/// case.
pub struct Lcg {
    /// The generator's state.
    state: u64,
}

impl Lcg {
    /// Starts the generator at `seed`.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next value in the sequence.
    pub fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // The high bits of an LCG are the ones with a usable period; the low ones cycle
        // far too fast to make a corpus that covers the range.
        (self.state >> 33) as u32
    }
}
