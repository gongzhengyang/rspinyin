//! The doubles and fixtures the key-routing tests are driven from.
//!
//! Everything here is in memory: an in-memory dictionary, an in-memory language model, a
//! user-frequency source that remembers what it was asked to record, and a host that
//! remembers every call the engine made. No test in this module touches a dictionary
//! file, the clock, the environment or a display server, which is what makes the whole
//! routing layer verifiable without Fcitx5 present.

mod effects;
mod routing;

use std::sync::Mutex;

use ime_core::lm::InMemoryLm;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::{SessionEnv, SessionState};
use ime_core::viterbi::Decoder;
use ime_types::{
    Anchor, HideReason, ImeError, Lexicon, SyllableId, UiCommand, UiFrame, UserFreqSource,
    WordFlags, WordIter, WordRef,
};

use crate::engine::host::Host;
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};

/// The input context every test that needs only one uses.
const IC: u64 = 1;

/// A second input context, for the tests that check one context's state does not reach
/// another.
const IC2: u64 = 2;

/// `FcitxKey_n`.
const KEY_N: u32 = 0x006e;

/// `FcitxKey_i`.
const KEY_I: u32 = 0x0069;

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 7,
    }
}

/// A key release of `sym` with `state` held.
fn release(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: true,
        time_ms: 7,
    }
}

/// A deterministic keysym stream from a 32-bit LCG: reproducible, and needs no
/// dependency (`rand` is not one of this workspace's crates).
fn keysym_stream(count: usize) -> impl Iterator<Item = u32> {
    let mut seed: u32 = 0x1234_5678;
    (0..count).map(move |_| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed
    })
}

/// The privacy state a fresh installation has: the shipped policy, no blacklist.
fn shipped_privacy() -> ContextPrivacy {
    ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
}

/// What the host reports about an ordinary context: no program name, neither the password
/// nor the sensitive flag. Nothing is blacklisted and learning is permitted.
fn ordinary_report() -> ContextReport<'static> {
    ContextReport::Reported {
        program: None,
        password: false,
        sensitive: false,
    }
}

/// Activates `ic` as an ordinary, learnable context.
fn activate_ordinary(router: &mut KeyRouter<'_>, ic: u64) {
    router.activate_reported(ic, ordinary_report());
}

/// Starts a composition by typing `n` in `ic`, and asserts that it started.
///
/// Every test that needs a live composition begins here, so the setup and the assumption
/// it rests on stay in one place.
fn type_n(router: &mut KeyRouter<'_>, ic: u64, host: &mut RecordingHost) {
    assert!(
        router.key_event(ic, &press(KEY_N, 0), host),
        "a letter is the plugin's"
    );
    assert_eq!(
        router.session(ic).map(|session| session.state),
        Some(SessionState::Composing),
        "one letter starts a composition"
    );
}

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

/// The dictionary the routing tests decode against.
///
/// Three readings of `ni`, two of `hao` and two whole-input readings of `ni'hao`: enough
/// for a candidate list that is longer than one entry and shorter than a page, which is
/// what the selection and paging boundary tests need. There is no single-character
/// fallback, so nothing here invents a candidate the rows below did not declare.
struct TestLexicon {
    /// Every entry, in the order the tests declared them.
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

/// A user-frequency source that remembers every record it was given.
///
/// The trait's `record` takes `&self`, so the log needs interior mutability; a mutex is
/// used rather than a cell because the trait requires `Sync`.
#[derive(Default)]
struct RecordingUser {
    /// The commits the source was asked to record.
    commits: Mutex<Vec<(String, u16)>>,
}

impl RecordingUser {
    /// Everything recorded so far.
    fn recorded(&self) -> Vec<(String, u16)> {
        match self.commits.lock() {
            Ok(commits) => commits.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl UserFreqSource for RecordingUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, key: &str, weight_hint: u16) {
        let mut commits = match self.commits.lock() {
            Ok(commits) => commits,
            Err(poisoned) => poisoned.into_inner(),
        };
        commits.push((String::from(key), weight_hint));
    }

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// One call the engine made on the host.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    /// Text committed to the application.
    Commit(u64, String),
    /// Composing text written into the application's preedit area.
    SetPreedit(u64, String, u32),
    /// The application's preedit area emptied.
    ClearPreedit(u64),
    /// A command posted to the candidate window.
    Post(u64, UiCommand),
    /// The input context's enabled state flipped to the recorded value.
    Toggle(u64, bool),
    /// A diagnostic reported, as its rendered code.
    Diagnose(u64, String),
}

/// A host that remembers every call and answers the enable toggle with a real state.
struct RecordingHost {
    /// Every call, in the order the engine made it.
    calls: Vec<Call>,
    /// Whether the input context is enabled. A context is activated enabled, so the first
    /// toggle is the one that switches to English.
    is_enabled: bool,
}

impl Default for RecordingHost {
    /// A host whose input context is enabled, which is the state a context is activated
    /// in.
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            is_enabled: true,
        }
    }
}

impl RecordingHost {
    /// Everything committed so far.
    fn commits(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Commit(_, text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The candidate-window commands posted so far, named for readable assertions.
    fn ui_kinds(&self) -> Vec<&'static str> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Post(_, UiCommand::Frame(_)) => Some("frame"),
                Call::Post(_, UiCommand::Show { .. }) => Some("show"),
                Call::Post(_, UiCommand::Hide { .. }) => Some("hide"),
                Call::Post(_, UiCommand::Theme(_)) => Some("theme"),
                Call::Post(_, UiCommand::Shutdown) => Some("shutdown"),
                _ => None,
            })
            .collect()
    }

    /// The newest frame the window was handed, if any.
    fn last_frame(&self) -> Option<&UiFrame> {
        self.calls.iter().rev().find_map(|call| match call {
            Call::Post(_, UiCommand::Frame(frame)) => Some(&**frame),
            _ => None,
        })
    }

    /// Why the window was hidden, in the order the hides were posted.
    fn hides(&self) -> Vec<HideReason> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Post(_, UiCommand::Hide { reason, .. }) => Some(*reason),
                _ => None,
            })
            .collect()
    }

    /// Where the newest show put the window, if any.
    fn last_show(&self) -> Option<Anchor> {
        self.calls.iter().rev().find_map(|call| match call {
            Call::Post(_, UiCommand::Show { anchor, .. }) => Some(*anchor),
            _ => None,
        })
    }

    /// The diagnostics reported so far, as their rendered codes.
    fn diagnostics(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Diagnose(_, code) => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    /// The states the input context was left in by the toggles so far.
    fn toggles(&self) -> Vec<bool> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Toggle(_, enabled) => Some(*enabled),
                _ => None,
            })
            .collect()
    }

    /// The preedit calls so far: `Some((text, caret))` for a write, `None` for a clear.
    fn preedits(&self) -> Vec<Option<(String, u32)>> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::SetPreedit(_, text, caret) => Some(Some((text.clone(), *caret))),
                Call::ClearPreedit(_) => Some(None),
                _ => None,
            })
            .collect()
    }

    /// Whether anything other than a diagnostic reached the host.
    ///
    /// This is the test-side reading of "the plugin did something": an effect other than
    /// `Effect::Diagnose` is exactly a call other than `Call::Diagnose` here.
    fn acted(&self) -> bool {
        self.calls
            .iter()
            .any(|call| !matches!(call, Call::Diagnose(..)))
    }
}

impl Host for RecordingHost {
    fn commit(&mut self, ic: u64, text: &str) {
        self.calls.push(Call::Commit(ic, String::from(text)));
    }

    fn set_preedit(&mut self, ic: u64, text: &str, caret: u32) {
        self.calls
            .push(Call::SetPreedit(ic, String::from(text), caret));
    }

    fn clear_preedit(&mut self, ic: u64) {
        self.calls.push(Call::ClearPreedit(ic));
    }

    fn post_ui(&mut self, ic: u64, command: UiCommand) {
        self.calls.push(Call::Post(ic, command));
    }

    fn toggle_enabled(&mut self, ic: u64) -> bool {
        self.is_enabled = !self.is_enabled;
        self.calls.push(Call::Toggle(ic, self.is_enabled));
        self.is_enabled
    }

    fn diagnose(&mut self, ic: u64, err: &ImeError) {
        self.calls.push(Call::Diagnose(ic, err.to_string()));
    }
}

/// The routing layer driven from in-memory data.
struct Fixture {
    /// The dictionary the decode reads.
    lexicon: TestLexicon,
    /// The user frequencies the commit path writes to.
    user: RecordingUser,
    /// The language model the ranking is scored with.
    lm: InMemoryLm,
    /// The decoder, built with the shipped configuration.
    decoder: Decoder,
}

impl Fixture {
    /// Builds the doubles every routing test decodes against.
    fn new() -> Self {
        Self {
            lexicon: TestLexicon::new(&[
                ("ni", "你", 900_000, 1),
                ("ni", "尼", 800_000, 1),
                ("ni", "泥", 700_000, 1),
                ("hao", "好", 900_000, 1),
                ("hao", "号", 800_000, 1),
                ("ni'hao", "你好", 900_000, 2),
                ("ni'hao", "拟好", 800_000, 2),
            ]),
            user: RecordingUser::default(),
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

    /// A router over these sources with the shipped configuration.
    fn router(&self) -> KeyRouter<'_> {
        self.router_with(RoutingConfig::default())
    }

    /// A router over these sources with `config` in force.
    fn router_with(&self, config: RoutingConfig) -> KeyRouter<'_> {
        KeyRouter::new(self.env(), shipped_privacy(), config)
    }
}
