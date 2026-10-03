//! What the session host does with the host's callbacks.
//!
//! Two groups. The first drives [`SessionHost`] directly, over an in-memory dictionary and
//! a recording host, so the whole lifecycle — activate, type, reset, deactivate, unload —
//! is asserted without Fcitx5, a display server or a dictionary file. The second drives
//! the process-wide slot the C ABI reaches, which is the only place the free functions
//! can be exercised; it installs, walks the callbacks and sweeps, and leaves the slot
//! empty so a later test in the same process starts from the same state.
//!
//! Nothing here reads the clock, the environment or the filesystem. The fixtures are
//! leaked on purpose: the session host borrows its sources for the life of the process,
//! which is exactly the constraint the real startup step has to meet.

use ime_core::lm::InMemoryLm;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::SessionEnv;
use ime_core::viterbi::Decoder;
use ime_types::{
    Anchor, DismissReason, ImeError, Lexicon, Placement, RectI, ScreenId, SyllableId, UiCommand,
    UiEvent, UserFreqSource, WordFlags, WordIter, WordRef,
};

use crate::engine::host::Host;
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy};

use super::{
    SessionHost, activate, deactivate, focus_in, focus_out, install, is_installed, key_event,
    lock_slot_for_tests, reload, reset, set_anchor, shutdown, ui_event,
};

/// The input context most tests use.
const IC: u64 = 1;

/// A second input context, for the test that two must not disturb each other.
const OTHER_IC: u64 = 2;

/// `FcitxKey_n`.
const KEY_N: u32 = 0x006e;

/// `FcitxKey_i`.
const KEY_I: u32 = 0x0069;

/// `FcitxKey_space`, which commits the highlighted candidate.
const KEY_SPACE: u32 = 0x0020;

/// A key press of `sym` with no modifier held.
fn press(sym: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state: 0,
        is_release: false,
        time_ms: 7,
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
/// Two readings of `ni`, so a composition has a candidate to commit and a second one to
/// move the highlight onto. There is no single-syllable fallback, so nothing here invents
/// a candidate the rows did not declare.
struct TestLexicon {
    /// Every entry, in the order it was declared.
    words: Vec<Entry>,
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
/// Nothing here commits through the store: an unreported context is fail-closed, so the
/// learning path is suppressed before it reaches a source at all.
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

/// The sources a session decodes against, leaked for the process.
///
/// The leak is the constraint the type carries rather than a shortcut: a session host
/// lives in a process-wide slot and outlives every caller, so its sources have to as well.
fn sources() -> SessionEnv<'static> {
    let lexicon: &'static TestLexicon = Box::leak(Box::new(TestLexicon {
        words: vec![
            Entry {
                key: "ni",
                text: "你",
                weight: 900_000,
                syl_count: 1,
            },
            Entry {
                key: "ni",
                text: "尼",
                weight: 800_000,
                syl_count: 1,
            },
        ],
    }));
    let decoder: &'static Decoder = Box::leak(Box::new(Decoder::default()));
    let user: &'static SilentUser = Box::leak(Box::new(SilentUser));
    let lm: &'static InMemoryLm = Box::leak(Box::new(InMemoryLm::new()));
    SessionEnv {
        decoder,
        lexicon,
        user_freq: user,
        lm,
    }
}

/// The privacy state a fresh installation has.
fn privacy() -> ContextPrivacy {
    ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
}

/// A session host over the in-memory sources, with `config` in force.
fn host_with(config: RoutingConfig) -> SessionHost {
    SessionHost::new(KeyRouter::new(sources(), privacy(), config))
}

/// What the newest frame the host was handed showed.
#[derive(Debug, Default)]
struct FrameNote {
    /// The preedit line the window's header draws.
    preedit: String,
    /// The candidate texts of the page, best first.
    candidates: Vec<String>,
    /// Where the window was told to appear.
    anchor: Option<Anchor>,
    /// The revision the frame carries.
    revision: u32,
}

/// A host boundary that remembers what it was asked to do.
#[derive(Debug)]
struct RecordingHost {
    /// Every text handed to [`Host::commit`], in order.
    commits: Vec<String>,
    /// Every composing text written into the application's own preedit area.
    preedits: Vec<String>,
    /// How many times that area was emptied.
    cleared: usize,
    /// Every command posted, by kind.
    commands: Vec<&'static str>,
    /// The newest frame, as the window would draw it.
    frame: FrameNote,
    /// The state the host reports for every context.
    enabled: bool,
    /// Every diagnostic, rendered.
    diagnostics: Vec<String>,
}

impl Default for RecordingHost {
    /// A host whose input context is enabled, which is the state one is activated in.
    fn default() -> Self {
        Self {
            commits: Vec::new(),
            preedits: Vec::new(),
            cleared: 0,
            commands: Vec::new(),
            frame: FrameNote::default(),
            enabled: true,
            diagnostics: Vec::new(),
        }
    }
}

/// Names a command by its kind, so an assertion can compare without cloning a frame.
fn command_kind(command: &UiCommand) -> &'static str {
    match command {
        UiCommand::Frame(_) => "frame",
        UiCommand::Show { .. } => "show",
        UiCommand::Hide { .. } => "hide",
        UiCommand::Theme(_) => "theme",
        UiCommand::Overlay(_) => "overlay",
        UiCommand::Shutdown => "shutdown",
    }
}

impl Host for RecordingHost {
    fn commit(&mut self, _ic: u64, text: &str) {
        self.commits.push(text.to_owned());
    }

    fn set_preedit(&mut self, _ic: u64, text: &str, _caret: u32) {
        self.preedits.push(text.to_owned());
    }

    fn clear_preedit(&mut self, _ic: u64) {
        self.cleared += 1;
    }

    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        self.commands.push(command_kind(&command));
        if let UiCommand::Frame(frame) = &command {
            self.frame = FrameNote {
                preedit: frame.preedit.text.clone(),
                candidates: frame
                    .candidates
                    .iter()
                    .map(|candidate| candidate.text.clone())
                    .collect(),
                anchor: Some(frame.anchor),
                revision: frame.revision,
            };
        }
    }

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }

    fn diagnose(&mut self, _ic: u64, err: &ImeError) {
        self.diagnostics.push(err.to_string());
    }
}

/// Types `ni` in `ic`, which is the shortest input the fixture decodes to a candidate.
fn type_ni(sessions: &mut SessionHost, ic: u64, host: &mut RecordingHost) {
    assert!(
        sessions.key_event(ic, &press(KEY_N), host),
        "a letter that starts a composition is the plugin's key"
    );
    assert!(
        sessions.key_event(ic, &press(KEY_I), host),
        "a letter that extends a composition is the plugin's key"
    );
}

/// An anchor a test can recognise, at the given x position.
fn anchor(x: i32) -> Anchor {
    Anchor {
        cursor: RectI {
            x,
            y: 40,
            w: 2,
            h: 18,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Below,
    }
}

// ── the session host, driven directly ───────────────────────────────────────────────

#[test]
fn test_session_host_key_event_commits_the_highlighted_candidate() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    assert_eq!(
        host.frame.preedit, "ni",
        "the composing text reaches the window as the user typed it"
    );
    assert_eq!(
        host.frame.candidates,
        [String::from("你"), String::from("尼")],
        "the dictionary's words reach the window in weight order"
    );
    assert!(
        sessions.key_event(IC, &press(KEY_SPACE), &mut host),
        "space with a composition in flight commits the highlighted candidate"
    );
    assert_eq!(
        host.commits,
        [String::from("你")],
        "the application receives the candidate the highlight was on"
    );
    assert_eq!(
        host.commands.last().copied(),
        Some("hide"),
        "the window goes away once the text has landed"
    );
}

#[test]
fn test_session_host_idle_key_is_handed_back_to_the_application() {
    // The defect the arbitration exists to close: with nothing composing, a space, a
    // digit, a return or a backspace is the application's, not the plugin's.
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    for sym in [KEY_SPACE, 0x0031, 0xff0d, 0xff08] {
        assert!(
            !sessions.key_event(IC, &press(sym), &mut host),
            "an idle session must not claim {sym:#x}"
        );
    }
    assert!(
        host.commits.is_empty() && host.commands.is_empty(),
        "and it must do nothing about it either"
    );
}

#[test]
fn test_session_host_deactivate_drops_the_composition_without_committing() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    sessions.deactivate(IC, &mut host);
    assert!(
        host.commits.is_empty(),
        "switching the input method away must not commit the candidate under the caret"
    );
    assert_eq!(
        host.commands.last().copied(),
        Some("hide"),
        "the window goes away with the composition"
    );
    assert_eq!(sessions.live_contexts(), 0, "the context is dropped");
}

#[test]
fn test_session_host_reset_and_deactivate_produce_the_same_host_calls() {
    // The two host callbacks differ in what they leave behind — a reset keeps the
    // context, a deactivation drops it — and in nothing else: the sequence the host sees
    // has to be the same one.
    let mut reset_calls = Vec::new();
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    let before = host.commands.len();
    sessions.reset(IC, &mut host);
    reset_calls.extend(host.commands[before..].iter().copied());
    assert_eq!(
        sessions.live_contexts(),
        1,
        "a reset leaves the session in place"
    );

    let mut deactivate_calls = Vec::new();
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    let before = host.commands.len();
    sessions.deactivate(IC, &mut host);
    deactivate_calls.extend(host.commands[before..].iter().copied());

    assert_eq!(
        reset_calls, deactivate_calls,
        "a reset and a deactivation must reach the host identically"
    );
    assert_eq!(reset_calls, ["hide"], "and both hide the window");
}

#[test]
fn test_session_host_activate_after_deactivate_starts_a_clean_session() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    sessions.deactivate(IC, &mut host);
    sessions.activate(IC);
    sessions.key_event(IC, &press(KEY_N), &mut host);
    assert_eq!(
        host.frame.preedit, "n",
        "the second session starts from the first character, not from what the first typed"
    );
}

#[test]
fn test_session_host_two_contexts_do_not_disturb_each_other() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    sessions.activate(OTHER_IC);
    type_ni(&mut sessions, IC, &mut host);
    sessions.key_event(OTHER_IC, &press(KEY_N), &mut host);
    assert_eq!(
        host.frame.preedit, "n",
        "the second context's frame shows its own input"
    );
    sessions.deactivate(IC, &mut host);
    assert_eq!(
        sessions.live_contexts(),
        1,
        "dropping one context leaves the other's session alone"
    );
}

#[test]
fn test_session_host_focus_storms_stay_bounded_and_reclaim_lazily() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);

    // Below the limit a focus loss is a composition reset that keeps the context.
    for _ in 1..super::FOCUS_OUTS_BEFORE_RECLAIM {
        sessions.focus_out(IC, &mut host);
    }
    assert_eq!(
        host.commands.last().copied(),
        Some("hide"),
        "the window is hidden"
    );
    assert!(host.commits.is_empty(), "a focus loss never commits");
    assert_eq!(sessions.live_contexts(), 1, "the session is kept");

    // The next loss reclaims the context, privacy state included.
    sessions.focus_out(IC, &mut host);
    assert_eq!(
        sessions.live_contexts(),
        0,
        "the limit reclaims the context"
    );
    assert_eq!(
        sessions.router.observed_privacy_contexts(),
        0,
        "and the privacy state goes with it"
    );

    // A storm of losses for a context that never comes back finds nothing further to
    // reclaim, and the table stays empty rather than growing back.
    for _ in 0..1000 {
        sessions.focus_out(IC, &mut host);
    }
    assert_eq!(
        sessions.live_contexts(),
        0,
        "the storm reclaims nothing twice"
    );
    // Focus that keeps coming back keeps exactly the active one: every arrival
    // restarts the count the limit is measured in.
    sessions.focus_in(IC);
    for _ in 0..1000 {
        sessions.focus_out(IC, &mut host);
        sessions.focus_in(IC);
    }
    // The bound: the table never holds more than the active contexts plus the limit.
    assert!(
        sessions.live_contexts() <= 1 + super::FOCUS_OUTS_BEFORE_RECLAIM,
        "the context table stays bounded under the storm"
    );
    assert_eq!(
        sessions.live_contexts(),
        1,
        "a focus that comes back finds its session"
    );
}

#[test]
fn test_session_host_ui_event_anchor_and_reload_reach_the_session() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    let typed_revision = host.frame.revision;

    sessions.set_anchor(IC, anchor(120));
    assert!(
        sessions.ui_event(
            IC,
            UiEvent::Hover {
                revision: typed_revision,
                index: Some(1),
            },
            &mut host,
        ),
        "a hover that moves the highlight is something the plugin did"
    );
    assert_eq!(
        host.frame.anchor.map(|anchor| anchor.cursor.x),
        Some(120),
        "the frame the hover repainted carries the anchor the host reported"
    );

    let mut config = RoutingConfig::default();
    config.session.max_per_row = 7;
    sessions.reload(config, &mut host);
    assert_eq!(
        host.frame.candidates.len(),
        2,
        "a reload repaints the composition under the new page size"
    );

    assert!(
        !sessions.ui_event(
            IC,
            UiEvent::Dismiss {
                revision: typed_revision,
                reason: DismissReason::Escape,
            },
            &mut host,
        ),
        "an event naming a frame the session has moved past is dropped"
    );
}

#[test]
fn test_session_host_highlight_moved_by_the_window_decides_what_is_committed() {
    // The keyboard path and the window's path have to name the same candidate: a hover
    // that moves the highlight changes what the next space commits.
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    sessions.ui_event(
        IC,
        UiEvent::Hover {
            revision: host.frame.revision,
            index: Some(1),
        },
        &mut host,
    );
    sessions.key_event(IC, &press(KEY_SPACE), &mut host);
    assert_eq!(
        host.commits,
        [String::from("尼")],
        "the candidate the window put the highlight on is the one that commits"
    );
}

#[test]
fn test_session_host_shutdown_ends_every_session() {
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    sessions.activate(OTHER_IC);
    type_ni(&mut sessions, IC, &mut host);
    let before = host.commands.len();
    sessions.shutdown(&mut host);
    assert_eq!(sessions.live_contexts(), 0, "the table is empty afterwards");
    assert!(
        host.commands[before..].contains(&"hide"),
        "the window of every live session is hidden"
    );
    assert!(
        host.commits.is_empty(),
        "an unload commits nothing, whatever was composing"
    );
}

#[test]
fn test_session_host_with_client_preedit_writes_the_composing_text() {
    // `[ui] client_preedit` on is the configuration that makes the application's own
    // preedit area a carrier of the composing text; with it off the area is emptied
    // instead, which is what every other test here exercises.
    let config = RoutingConfig {
        client_preedit: true,
        ..RoutingConfig::default()
    };
    let mut sessions = host_with(config);
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    assert_eq!(
        host.preedits.last().map(String::as_str),
        Some("ni"),
        "the composing text is written into the application's preedit area"
    );
}

#[test]
fn test_session_host_never_claims_a_key_release() {
    // The host delivers both edges of every key, and taking a release would eat the
    // application's key-up. Nothing the routing layer does may claim one.
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    for sym in [
        KEY_N, KEY_I, KEY_SPACE, 0xff0d, 0xff08, 0xff1b, 0xff09, 0xff51,
    ] {
        let mut key = press(sym);
        key.is_release = true;
        assert!(
            !sessions.key_event(IC, &key, &mut host),
            "a release is never the plugin's key: {sym:#x}"
        );
    }
}

#[test]
fn test_session_host_never_claims_a_key_it_did_nothing_about() {
    // The invariant the arbitration exists to keep, walked over a wide sweep of keysyms
    // with a composition in flight: a key the plugin claims must have produced something
    // the host could see. A claimed key that produced nothing is a swallowed keystroke.
    let mut sessions = host_with(RoutingConfig::default());
    let mut host = RecordingHost::default();
    sessions.activate(IC);
    type_ni(&mut sessions, IC, &mut host);
    for sym in 0x20u32..0x7f {
        let before = (host.commits.len(), host.commands.len(), host.cleared);
        if sessions.key_event(IC, &press(sym), &mut host) {
            let after = (host.commits.len(), host.commands.len(), host.cleared);
            assert_ne!(
                before, after,
                "the plugin claimed {sym:#x} and then did nothing about it"
            );
        }
    }
}

// ── the process-wide slot the C ABI reaches ─────────────────────────────────────────
//
// The slot's guard lives on the module itself (`lock_slot_for_tests`), because the FFI
// entry-point tests in `ffi::abi` take the same one: a focus callback driven with a null
// context still lands in the free functions below, and those touch the shared slot.

#[test]
fn test_install_refuses_a_second_session_host() {
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    assert!(
        !is_installed(),
        "the sweep before the test emptied the slot"
    );
    assert!(
        install(sources(), privacy(), RoutingConfig::default()),
        "the first install takes the slot"
    );
    assert!(is_installed());
    assert!(
        !install(sources(), privacy(), RoutingConfig::default()),
        "a second install is refused: the router already there may be running a key"
    );
    assert_eq!(shutdown(), 0, "nothing was activated, so nothing was live");
    assert!(!is_installed(), "the sweep empties the slot");
}

#[test]
fn test_installed_host_routes_a_key_and_reports_what_was_live() {
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    assert!(install(sources(), privacy(), RoutingConfig::default()));
    let mut host = RecordingHost::default();
    assert!(activate(IC), "activation reports a live session");
    key_event(IC, &press(KEY_N), &mut host);
    key_event(IC, &press(KEY_I), &mut host);
    set_anchor(IC, anchor(64));
    ui_event(
        IC,
        UiEvent::Hover {
            revision: host.frame.revision,
            index: Some(1),
        },
        &mut host,
    );
    reload(RoutingConfig::default(), &mut host);
    key_event(IC, &press(KEY_SPACE), &mut host);
    assert_eq!(
        host.commits,
        [String::from("尼")],
        "the callbacks the host delivers reach the session through the slot"
    );
    assert_eq!(shutdown(), 1, "the sweep reports the session it ended");
    assert!(!is_installed());
}

#[test]
fn test_callbacks_without_an_installed_host_claim_nothing() {
    // The degradation: a callback that arrives before the sources exist must leave every
    // key to the application rather than swallowing it.
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    let mut host = RecordingHost::default();
    assert!(
        !key_event(IC, &press(KEY_N), &mut host),
        "a letter is not claimed with no session to act on it"
    );
    assert!(!activate(IC), "and no session is reported as set up");
    deactivate(IC, &mut host);
    reset(IC, &mut host);
    focus_in(IC);
    focus_out(IC, &mut host);
    set_anchor(IC, anchor(0));
    reload(RoutingConfig::default(), &mut host);
    assert!(
        !ui_event(
            IC,
            UiEvent::Dismiss {
                revision: 0,
                reason: DismissReason::Escape,
            },
            &mut host,
        ),
        "a window event is not acted on either"
    );
    assert!(
        host.commits.is_empty() && host.commands.is_empty(),
        "nothing reaches the host while no session host is installed"
    );
    assert_eq!(shutdown(), 0, "and the sweep finds nothing to end");
}

#[test]
fn test_deactivate_through_the_slot_ends_the_composition() {
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    assert!(install(sources(), privacy(), RoutingConfig::default()));
    let mut host = RecordingHost::default();
    activate(IC);
    key_event(IC, &press(KEY_N), &mut host);
    key_event(IC, &press(KEY_I), &mut host);
    let before = host.commands.len();
    deactivate(IC, &mut host);
    assert_eq!(
        host.commands[before..],
        ["hide"],
        "switching the input method away hides the window"
    );
    assert!(
        host.commits.is_empty(),
        "and commits nothing that was composing"
    );
    let _ = shutdown();
}

#[test]
fn test_reset_through_the_slot_keeps_the_session() {
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    assert!(install(sources(), privacy(), RoutingConfig::default()));
    let mut host = RecordingHost::default();
    activate(IC);
    key_event(IC, &press(KEY_N), &mut host);
    reset(IC, &mut host);
    assert_eq!(
        shutdown(),
        1,
        "a reset ends the composition and leaves the session where it was"
    );
}

#[test]
fn test_focus_through_the_slot_ends_the_composition_and_keeps_the_context() {
    let _slot = lock_slot_for_tests();
    let _ = shutdown();
    assert!(install(sources(), privacy(), RoutingConfig::default()));
    let mut host = RecordingHost::default();
    activate(IC);
    key_event(IC, &press(KEY_N), &mut host);
    key_event(IC, &press(KEY_I), &mut host);
    focus_out(IC, &mut host);
    assert!(
        host.commits.is_empty(),
        "a focus loss through the slot commits nothing"
    );
    focus_in(IC);
    key_event(IC, &press(KEY_N), &mut host);
    assert_eq!(
        host.frame.preedit, "n",
        "focus arriving again finds the context it left and starts a clean composition"
    );
    assert_eq!(shutdown(), 1, "the context focus brought back reports live");
}
