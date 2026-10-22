//! What the router does with the configuration, with a phrase the session saves, and
//! with the badge the header's right-hand slot shows.
//!
//! Three things are tested here. The first is the label the status strip carries: it comes
//! from the configuration — the active layout's name, or the engine's own Chinese /
//! English label when the user turned the hint off — so the window shows what the document
//! says rather than a value the router decided on its own.
//!
//! The second is the phrase document and its format, which are tested beside the store, and
//! the state machine that decides *what* to save, which is tested in `ime-core`. What is
//! tested here is the arm that joins them: the entry the session handed over reaches the
//! user's document, the table the decode reads is rebuilt around it, the key is kept when
//! that worked, and a save that could not happen leaves the key to the application and
//! reports why.
//!
//! The third is the badge: the first-run key hint and the page indicator that share the
//! label's slot. The resolution runs where frames are posted, so these tests drive the
//! router and read the strip off the frames the host was handed.
//!
//! Everything is in memory except the document itself, which is the point of the test: a
//! phrase is saved by writing a file, so the assertion is made against what the file
//! holds. The document lives in a directory of the test's own under the system's
//! temporary directory, and no test here touches the environment, the clock or a display
//! server.

mod pool;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use ime_core::lm::InMemoryLm;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::{SessionEnv, SessionEvent, SessionState};
use ime_core::viterbi::Decoder;
use ime_types::{
    ImeError, KeyAction, Lexicon, StatusStrip, SyllableId, UiCommand, UserFreqSource, WordFlags,
    WordIter, WordRef,
};

use crate::engine::host::Host;
use crate::engine::router::phrases::{PhraseHandle, PhraseStore};
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::engine::{CTRL, KEY_SPACE, KEY_T, SHIFT};
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};

/// The input context every test uses.
const IC: u64 = 1;
/// A second input context, for the tests that hold two of them.
const IC2: u64 = 2;

/// A second input context, for the test that focus creates a session where none was.
const OTHER_IC: u64 = 2;

/// `FcitxKey_n`.
const KEY_N: u32 = 0x006e;

/// `FcitxKey_i`.
const KEY_I: u32 = 0x0069;

/// `FcitxKey_minus`, a page key of the shipped configuration.
const KEY_MINUS: u32 = 0x002d;

/// `FcitxKey_equal`, the other page key of the shipped configuration.
const KEY_EQUAL: u32 = 0x003d;

/// How long a test waits for the phrase writer thread to publish a row.
///
/// A save is answered before it is written: the host thread hands the row to the writer
/// and returns, and the document and the table are the writer's to update. A test that
/// read them straight afterwards would be asserting on a race rather than on the
/// contract, so it waits for the effect instead of for a duration.
const PHRASES_PATIENCE: Duration = Duration::from_secs(5);

/// Spins until `condition` holds or [`PHRASES_PATIENCE`] runs out.
///
/// Returns whether the condition was ever observed, so a caller can assert on it and
/// report the wait as a failure rather than hanging.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PHRASES_PATIENCE;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::yield_now();
    }
    condition()
}

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 7,
    }
}

/// The privacy state a fresh installation has: the shipped policy, no blacklist.
fn shipped_privacy() -> ContextPrivacy {
    ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
}

/// What the host reports about an ordinary context: no program name, neither the password
/// nor the sensitive flag.
fn ordinary_report() -> ContextReport<'static> {
    ContextReport::Reported {
        program: None,
        password: false,
        sensitive: false,
        // An ordinary application renders a client preedit: the negotiation
        // leaves the configuration's answer standing.
        preedit: true,
    }
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

/// The dictionary these tests decode against.
///
/// Two readings of `ni`: enough for a composition with a candidate highlighted, which is
/// what the session needs before it can save one. There is no single-character fallback,
/// so nothing here invents a candidate the rows did not declare.
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
/// The phrase path never reads it: saving a phrase is not a commit, so nothing is learned
/// from it and nothing is recorded.
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

/// A host that remembers the diagnostics, the newest frame's first candidate, and the
/// window commands that are not frames.
///
/// The phrase path produces nothing else. Saving a phrase writes a file and reports a
/// diagnostic, and the text it saves is the candidate the highlight was on -- which is
/// the first candidate of the newest frame, because nothing here moves the highlight. The
/// status strip of that frame is kept too, because it is what the window's header shows.
/// The hide count and the emptied-preedit count are what the focus lifecycle is asserted
/// against.
struct RecordingHost {
    /// The rendered code of every diagnostic, in order.
    diagnostics: Vec<String>,
    /// The text of the first candidate of the newest frame.
    highlighted: Option<String>,
    /// The status strip of the newest frame.
    status: Option<StatusStrip>,
    /// Whether the input context is enabled. A context is activated enabled, so the first
    /// toggle is the one that switches to English.
    is_enabled: bool,
    /// How many times the window was told to hide.
    hides: usize,
    /// How many times the application's preedit area was emptied.
    cleared: usize,
    /// How many frames the window was sent.
    frames: usize,
    /// The text of every commit, in order.
    commits: Vec<String>,
    /// The text of every client preedit write, in order.
    preedits: Vec<String>,
    /// How many times the application's preedit area was written.
    preedit_cleared: usize,
}

impl Default for RecordingHost {
    /// A host whose input context is enabled, which is the state a context is activated in.
    fn default() -> Self {
        Self {
            diagnostics: Vec::new(),
            highlighted: None,
            status: None,
            is_enabled: true,
            hides: 0,
            cleared: 0,
            frames: 0,
            commits: Vec::new(),
            preedits: Vec::new(),
            preedit_cleared: 0,
        }
    }
}

impl Host for RecordingHost {
    fn commit(&mut self, _ic: u64, text: &str) {
        self.commits.push(String::from(text));
    }

    fn set_preedit(&mut self, _ic: u64, text: &str, _caret: u32) {
        self.preedits.push(text.to_owned());
    }

    fn clear_preedit(&mut self, _ic: u64) {
        self.preedit_cleared = self.preedit_cleared.saturating_add(1);
        self.cleared += 1;
    }

    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        match command {
            UiCommand::Frame(frame) => {
                self.frames += 1;
                self.highlighted = frame.candidates.first().map(|held| held.text.clone());
                self.status = Some(frame.status.clone());
            }
            UiCommand::Hide { .. } => self.hides += 1,
            _ => {}
        }
    }

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        self.is_enabled = !self.is_enabled;
        self.is_enabled
    }

    fn diagnose(&mut self, _ic: u64, err: &ImeError) {
        self.diagnostics.push(err.to_string());
    }
}

/// The routing layer driven from in-memory data, with a phrase document of the test's.
struct Fixture {
    /// The dictionary the decode reads.
    lexicon: TestLexicon,
    /// The user frequencies the commit path writes to.
    user: Box<dyn UserFreqSource>,
    /// The language model the ranking is scored with.
    lm: InMemoryLm,
    /// The decoder, built with the shipped configuration.
    decoder: Decoder,
    /// The directory the phrase document is written to.
    dir: PathBuf,
}

impl Fixture {
    /// Builds the doubles and an empty directory for the phrase document.
    ///
    /// The directory is emptied rather than merely created, so that a second run of the
    /// suite does not read a document the first one left behind.
    fn new(label: &str) -> Self {
        Self::with_user(label, Box::new(SilentUser))
    }

    /// Builds the doubles with `user` answering the frequency path.
    ///
    /// The directory is emptied rather than merely created, so that a second run of the
    /// suite does not read a document the first one left behind.
    fn with_user(label: &str, user: Box<dyn UserFreqSource>) -> Self {
        let name = format!("rspinyin-router-{}-{label}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        Self {
            lexicon: TestLexicon::new(&[
                ("ni", "你", 900_000, 1),
                ("ni", "尼", 800_000, 1),
                ("ni'hao", "你好", 900_000, 2),
            ]),
            user,
            lm: InMemoryLm::new(),
            decoder: Decoder::default(),
            dir,
        }
    }

    /// The sources as the environment a session step reads.
    fn env(&self) -> SessionEnv<'_> {
        SessionEnv {
            decoder: &self.decoder,
            lexicon: &self.lexicon,
            user_freq: &*self.user,
            lm: &self.lm,
            english: None,
            confusables: None,
            symbols: None,
        }
    }

    /// A router over these sources with the shipped configuration and `phrases`.
    fn router(&self, phrases: PhraseHandle) -> KeyRouter<'_> {
        self.router_with(phrases, RoutingConfig::default())
    }

    /// A router over these sources with `config` in force and `phrases`.
    fn router_with(&self, phrases: PhraseHandle, config: RoutingConfig) -> KeyRouter<'_> {
        let mut router = KeyRouter::new(self.env(), shipped_privacy(), config);
        router.use_phrases(phrases);
        router
    }
}

/// Types `ni` in [`IC`], which is the shortest input the fixture decodes to a candidate.
fn type_ni(router: &mut KeyRouter<'_>, host: &mut RecordingHost) {
    assert!(
        router.key_event(IC, &press(KEY_N, 0), host),
        "a letter is the plugin's"
    );
    router.key_event(IC, &press(KEY_I, 0), host);
}

#[test]
fn test_add_phrase_appends_the_highlighted_candidate_to_the_document() {
    let fixture = Fixture::new("appends");
    let document = fixture.dir.join("phrases.tsv");
    let handle = PhraseHandle::new(PhraseStore::load(Some(document.clone()), 100, true));
    let mut router = fixture.router(handle.clone());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    let highlighted = host.highlighted.clone().expect("a frame with a candidate");

    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::AddPhrase), &mut host),
        "a saved phrase is something the plugin did, so the key is kept"
    );
    assert!(
        host.diagnostics.is_empty(),
        "saving a phrase is not a failure: {:?}",
        host.diagnostics
    );
    // The row is queued, not written: the write and the read-back that follows it belong
    // to the writer thread, so the assertion is made once that thread has published.
    let saved = highlighted.as_str();
    let published = wait_until(|| {
        fs::read_to_string(&document).is_ok_and(|written| written == format!("ni\t{saved}\n"))
    });
    assert!(published, "the queued row reaches the document");
    // The table is rebuilt from the document, so the phrase is live for the rest of the
    // session rather than only after a restart.
    let live = wait_until(|| {
        let table = handle.table();
        table
            .longest_match("ni", 0)
            .is_some_and(|hit| table.text(hit) == saved)
    });
    assert!(live, "the rebuilt table carries the saved phrase");
    assert!(
        handle.shutdown(PHRASES_PATIENCE),
        "the writer stops within the patience a test allows it"
    );
}

#[test]
fn test_add_phrase_without_a_document_reports_read_only_mode() {
    // The degradation ASM-15 names: with no document to write to the plugin stops
    // writing, and the key it could not act on reaches the application.
    let fixture = Fixture::new("no-document");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    let before = host.diagnostics.len();
    assert!(
        !router.step_session(IC, SessionEvent::Key(KeyAction::AddPhrase), &mut host),
        "a key the plugin did not act on must reach the application"
    );
    assert_eq!(host.diagnostics.len(), before + 1, "{:?}", host.diagnostics);
    let reported = host.diagnostics.last().cloned().unwrap_or_default();
    assert!(reported.starts_with("data/readonly-mode:"), "{reported}");
}

#[test]
fn test_add_phrase_accepts_a_row_the_document_cannot_take() {
    // A path whose directory does not exist. The save is answered before it is written,
    // so what the router owes the session is the answer it can give: the row was
    // accepted, and the key is kept. The failure itself belongs to the writer thread and
    // is reported on the diagnostic channel from there, which is why nothing shows up in
    // this host's diagnostics at this moment.
    let fixture = Fixture::new("unwritable");
    let document = fixture.dir.join("absent").join("phrases.tsv");
    let handle = PhraseHandle::new(PhraseStore::load(Some(document.clone()), 100, true));
    let mut router = fixture.router(handle.clone());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::AddPhrase), &mut host),
        "the row is accepted, so the key is the plugin's"
    );
    assert!(
        host.diagnostics.is_empty(),
        "the write happens on the writer thread, so nothing is reported here: {:?}",
        host.diagnostics
    );
    assert!(
        handle.shutdown(PHRASES_PATIENCE),
        "the writer stops within the patience a test allows it"
    );
    assert!(
        !document.exists(),
        "a write the document refused leaves no file behind"
    );
}

/// A user source that answers nothing and keeps the pin requests it is given, so the
/// router's pin branch can be asserted from the outside.
#[derive(Default)]
struct PinningUser {
    /// Every `(key, word)` pair the source was asked to pin, in order. Behind a shared
    /// handle because the fixture owns the source and the test reads the log.
    requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl PinningUser {
    /// Builds the source and the handle its requests land in.
    fn new() -> (
        Self,
        std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    ) {
        let requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> =
            std::sync::Arc::default();
        (
            Self {
                requests: requests.clone(),
            },
            requests,
        )
    }
}

impl UserFreqSource for PinningUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }

    fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
        let word = word.map(str::to_owned).unwrap_or_default();
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((String::from(key), word));
        Ok(())
    }
}

#[test]
fn test_pin_highlighted_asks_the_store_and_reranks_the_frame() {
    let (user, requests) = PinningUser::new();
    let fixture = Fixture::with_user("pin", Box::new(user));
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    let highlighted = host.highlighted.clone().expect("a frame with a candidate");
    let frames_before = host.frames;

    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::PinHighlighted), &mut host),
        "the pin was applied, so the key is the plugin's"
    );
    assert!(
        host.diagnostics.is_empty(),
        "a pin the store took is not a failure: {:?}",
        host.diagnostics
    );
    assert_eq!(
        requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        [(String::from("ni"), highlighted)],
        "the store is asked for the input key and the highlighted word"
    );
    // The executor looped `PinApplied` back into the session, which decoded again and
    // re-sent the window: the frame the user sees is the one that ranks the pin.
    assert_eq!(
        host.frames,
        frames_before + 1,
        "the frame is re-sent after the store changed what ranks"
    );
}

#[test]
fn test_pin_at_a_source_without_pin_storage_reports_and_releases_the_key() {
    // The default `set_pinned` answers `dict/unsupported` (ADR-0013): the store took
    // nothing, so nothing re-ranks, and a key the plugin did not act on reaches the
    // application -- the same discipline every other refused action follows.
    let fixture = Fixture::new("pin-unsupported");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    let before = host.diagnostics.len();
    assert!(
        !router.step_session(IC, SessionEvent::Key(KeyAction::PinHighlighted), &mut host),
        "a key the plugin could not act on must reach the application"
    );
    assert_eq!(host.diagnostics.len(), before + 1, "{:?}", host.diagnostics);
    let reported = host.diagnostics.last().cloned().unwrap_or_default();
    assert_eq!(reported, "dict/unsupported", "{reported}");
}

#[test]
fn test_step_session_of_a_context_with_no_session_does_nothing() {
    let fixture = Fixture::new("absent");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    let mut host = RecordingHost::default();

    assert!(!router.step_session(IC, SessionEvent::Key(KeyAction::AddPhrase), &mut host));
    assert!(host.diagnostics.is_empty());
}

#[test]
fn test_step_session_of_an_idle_mode_toggle_reports_only_the_flash() {
    // The flash is diagnostic-class: the executor reports the mode line on the crash
    // channel and makes no host call at all, so the step did nothing the plugin keeps a
    // key for. The mode switches are claimed by the engine layer that flips the bit,
    // before the session is ever stepped.
    let fixture = Fixture::new("idle-flash");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(
        !router.step_session(IC, SessionEvent::Key(KeyAction::ToggleFullWidth), &mut host),
        "the flash reaches no host method, so the step is not the plugin's doing"
    );
    assert!(
        host.status.is_none(),
        "no frame was sent: with nothing composing there is no window"
    );
    assert_eq!(host.hides, 0, "and nothing was hidden either");
}

// ── the label the window's header shows ────────────────────────────────────────────
//
// The label comes from the configuration rather than from the router: the layout's own name
// while the user has the hint on, the engine's Chinese / English label when they turned it
// off. A router built with no phrase document is enough to reach it -- nothing on this path
// saves a phrase -- so these tests reach the store the startup sequence would install.

#[test]
fn test_client_preedit_negotiates_the_application_capability() {
    // The user's configuration is the ceiling and the application's capability is
    // the floor: a context the host reported as unable to render a preedit is
    // never written one, while an ordinary context -- or one the host said
    // nothing about -- keeps the behaviour the configuration asks for.
    let fixture = Fixture::new("preedit-negotiation");
    let config = RoutingConfig {
        client_preedit: true,
        ..RoutingConfig::default()
    };
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), config);
    let mut host = RecordingHost::default();

    // An ordinary application: the inline preedit is written.
    router.activate_reported(IC, ordinary_report());
    type_ni(&mut router, &mut host);
    assert!(
        !host.preedits.is_empty(),
        "a capable client receives the inline preedit"
    );

    // The same configuration over an application the host reported as unable to
    // render one: the area is cleared, never written.
    let limited = ContextReport::Reported {
        program: None,
        password: false,
        sensitive: false,
        preedit: false,
    };
    router.reset(IC, &mut host);
    router.activate_reported(IC, limited);
    let mut host = RecordingHost::default();
    type_ni(&mut router, &mut host);
    assert!(
        host.preedits.is_empty(),
        "a client without the capability never sees an inline string"
    );
    assert!(
        host.preedit_cleared > 0,
        "the area is emptied instead, so no earlier text is left behind"
    );

    // A context the host reported nothing about keeps the mainline behaviour:
    // the negotiation narrows only when the host spoke.
    router.reset(IC, &mut host);
    router.activate(IC);
    let mut host = RecordingHost::default();
    type_ni(&mut router, &mut host);
    assert!(
        !host.preedits.is_empty(),
        "an unreported capability does not narrow the configuration"
    );
}

#[test]
fn test_resolve_client_preedit_and_consolidates_the_ceiling_and_the_floor() {
    // The truth table of the one-liner the negotiation runs, pinned on its own:
    // both sides must agree before an inline string is written.
    use super::context::resolve_client_preedit;
    assert!(resolve_client_preedit(true, true));
    assert!(!resolve_client_preedit(true, false), "the floor wins");
    assert!(!resolve_client_preedit(false, true), "the ceiling wins");
    assert!(!resolve_client_preedit(false, false));
}

#[test]
fn test_key_event_labels_the_window_with_the_layout_the_configuration_names() {
    let fixture = Fixture::new("scheme-hint");
    let config = RoutingConfig {
        scheme_hint: Some("小鹤"),
        ..RoutingConfig::default()
    };
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), config);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);

    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(
        status.mode_label, "小鹤",
        "the header names the layout the configuration in force declares"
    );
}

#[test]
fn test_key_event_falls_back_to_the_engine_label_without_a_scheme_hint() {
    // `scheme.show_hint` off, or a layout this build has no name for: the window shows what
    // it always did rather than an empty header.
    let fixture = Fixture::new("no-hint");
    let config = RoutingConfig {
        scheme_hint: None,
        ..RoutingConfig::default()
    };
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), config);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);

    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "中");
}

#[test]
fn test_key_event_label_survives_the_host_s_language_chord() {
    // `Ctrl+Space` is the host's own language switch now: the routing table claims no
    // chord for it, so the key reaches the application and the plugin's label — the
    // layout the configuration names — is exactly where it was. The English marker the
    // old chord produced was the read-back of a state this boundary cannot write, which
    // is why it is gone rather than wrong.
    let fixture = Fixture::new("english-label");
    let config = RoutingConfig {
        scheme_hint: Some("小鹤"),
        ..RoutingConfig::default()
    };
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), config);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(!router.key_event(IC, &press(KEY_SPACE, CTRL), &mut host));
    assert!(
        host.is_enabled,
        "the plugin never touches the host's input-method state"
    );
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(
        status.mode_label, "小鹤",
        "the layout's name is unchanged by a key the plugin declined"
    );
}

// ── the badge the header's right-hand slot shows ─────────────────────────────────
//
// The slot is `StatusStrip::mode_label`, and the engine resolves who shows in it where the
// frame is posted: the first-run key hint once per process, then the page indicator of a
// multi-page list, then the mode name the three tests above assert. The hint is resolved
// against the bindings in force, and its text is built from key names and numbers only, so
// every assertion below is an exact string.

/// The hint the shipped bindings spell out, the design's example text.
const FIRST_RUN_HINT: &str = "Tab 换词 · ↑↓ 翻页 · Ctrl+Shift+/ 全部";

#[test]
fn test_first_frame_of_a_session_carries_the_hint_and_the_next_carries_the_mode_name() {
    let fixture = Fixture::new("first-hint");
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), RoutingConfig::default());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    // The process's first frame with candidates borrows the slot for the hint, whatever
    // the configuration's layout name is.
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(
        status.mode_label, FIRST_RUN_HINT,
        "the first frame with candidates shows the key hint"
    );

    // The next frame gives the slot back: the hint shows once per process, not once per
    // composition, so the layout's name returns inside the same composition.
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "全拼", "the layout's name returns");

    // And the second composition never sees the hint again.
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "全拼");
}

#[test]
fn test_multi_page_frame_carries_the_page_indicator() {
    // A list that spans two pages at the shipped page size of nine (`max_candidates`
    // of the decode contract), so the frame's own paging state has something to report.
    let lexicon = TestLexicon::new(&[
        ("ni", "你", 110_000, 1),
        ("ni", "尼", 100_000, 1),
        ("ni", "泥", 90_000, 1),
        ("ni", "妮", 80_000, 1),
        ("ni", "倪", 70_000, 1),
        ("ni", "霓", 60_000, 1),
        ("ni", "鲵", 50_000, 1),
        ("ni", "坭", 40_000, 1),
        ("ni", "猊", 30_000, 1),
        ("ni", "伲", 20_000, 1),
        ("ni", "祢", 10_000, 1),
    ]);
    let user = SilentUser;
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let env = SessionEnv {
        decoder: &decoder,
        lexicon: &lexicon,
        user_freq: &user,
        lm: &lm,
        english: None,
        confusables: None,
        symbols: None,
    };
    let mut router = KeyRouter::new(env, shipped_privacy(), RoutingConfig::default());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    // The first frame — one passthrough candidate on one page — borrows the slot for
    // the hint, even though the list the next keystroke brings will span pages.
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(
        status.mode_label, FIRST_RUN_HINT,
        "the hint's one showing outranks the page indicator"
    );

    // The next frame shows the eleven-candidate list of two pages: the indicator takes
    // the slot the hint gave back, and reads the page the frame itself is showing.
    assert!(router.key_event(IC, &press(KEY_I, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "1/2");

    // Page forward: the current page moves with the frame.
    assert!(router.key_event(IC, &press(KEY_EQUAL, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "2/2");

    // And back, because the current page is the other half of the indicator.
    assert!(router.key_event(IC, &press(KEY_MINUS, 0), &mut host));
    let status = host.status.clone().expect("a frame reached the window");
    assert_eq!(status.mode_label, "1/2");
}

// ── the focus lifecycle the session host drives ─────────────────────────────────
//
// The host's focus callbacks land on these two methods, so the routing-side half of the
// contract is tested here: a focus loss is a step of `SessionEvent::FocusLost` — the
// preedit area emptied and the window hidden, committing nothing, the session and the
// privacy state kept — and a focus arrival ensures a session instead of replacing one.

#[test]
fn test_focus_out_takes_the_composition_back_and_keeps_the_context() {
    let fixture = Fixture::new("focus-out");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    type_ni(&mut router, &mut host);

    let cleared_before = host.cleared;
    router.focus_out(IC, &mut host);
    assert_eq!(host.hides, 1, "the window is hidden with the composition");
    assert!(
        host.cleared > cleared_before,
        "the application's preedit area is emptied with it"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the session is stepped, not dropped"
    );
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some(""),
        "and the composition it held is gone"
    );
    assert_eq!(
        router.observed_privacy_contexts(),
        1,
        "the privacy state of a kept context is kept with it"
    );
    // The next keystroke starts over rather than continuing what was taken back.
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("n"),
        "typing starts from the new key"
    );
}

#[test]
fn test_focus_in_ensures_a_context_instead_of_rebuilding_it() {
    let fixture = Fixture::new("focus-in");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    router.key_event(IC, &press(KEY_N, 0), &mut host);

    // A focus arriving at a context that is already here changes nothing: the session
    // is the one the context had, mid-composition included.
    router.focus_in(IC);
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("n"),
        "an existing session is reused, not rebuilt"
    );
    assert_eq!(host.hides, 0, "and ensuring a context posts nothing");

    // A focus arriving where there was no session creates one, exactly as an
    // activation would.
    router.focus_in(OTHER_IC);
    assert!(
        router.session(OTHER_IC).is_some(),
        "focus is what makes a session exist for a context"
    );
}

#[test]
fn test_script_toggle_converts_the_commit_and_toggles_back() {
    let fixture = Fixture::new("script");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.use_script(std::sync::Arc::new(
        ime_dict::script::ScriptIndex::from_pairs(&[("你", "妳")]),
    ));
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    // Simplified first: the shipped bit is off, and the commit borrows through.
    type_ni(&mut router, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(host.commits.last().map(String::as_str), Some("你"));

    // The toggle flips the bit, and the same input commits converted. The chord is
    // the real one the routing table names, so the key travels the route a user's
    // keystroke travels.
    assert!(
        router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host),
        "a toggle with a table is the plugin's key"
    );
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    type_ni(&mut router, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("妳"),
        "the traditional mode converts the commit"
    );

    // The second toggle is the way back, and the commit is plain again.
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    type_ni(&mut router, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(host.commits.last().map(String::as_str), Some("你"));
}

#[test]
fn test_script_toggle_without_a_table_reports_and_releases_the_key() {
    // A build whose assembly installed no table: the toggle reports the degradation
    // the code was registered for, the key reaches the application, and the next
    // toggle behaves the same -- the mode bit never moved.
    let fixture = Fixture::new("script-missing");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        !router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host),
        "a key the plugin did not act on must reach the application"
    );
    // The refusal is reported on the process diagnostic channel -- the registered code
    // has no frozen `ImeError` variant to travel in, which is why the reporter is the
    // emitter and not the host boundary. What the host can assert is the shape of the
    // refusal: no error was fabricated, the composition survived the refused toggle
    // untouched, and a second press refuses the same way -- the mode bit never moved.
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);

    assert!(!router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "the composition the refusal preserved commits unchanged"
    );
}

#[test]
fn test_output_projection_reaches_a_new_context_and_spares_one_in_flight() {
    // The document asks for full width and English marks: a context created under it
    // commits widened ASCII from its first keystroke, while the context created
    // before the reload keeps the bits its own creation gave it.
    let fixture = Fixture::new("output-projection");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut config = RoutingConfig::default();
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), config);
    router.use_phrases(handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    // Half width first: the shipped projection.
    type_ni(&mut router, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(host.commits.last().map(String::as_str), Some("你"));

    // The reload replaces the routing view whole; the live context keeps its bits.
    config.output.full_width = true;
    router.reload(config, &mut host);
    type_ni(&mut router, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "an in-flight session keeps its own output bits"
    );

    // A context created under the reloaded view starts where the document says.
    router.activate_reported(IC2, ordinary_report());
    // `ai` is a syllable of its own, so the composition commits as one widened word.
    let _ = router.key_event(IC2, &press(crate::engine::KEY_A, 0), &mut host);
    let _ = router.key_event(IC2, &press(0x0069, 0), &mut host);
    router.key_event(IC2, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("ａｉ"),
        "the new context starts full width, down to the widened letters"
    );
}
