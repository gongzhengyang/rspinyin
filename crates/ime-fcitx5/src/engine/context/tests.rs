//! The key walk, layer by layer.
//!
//! Every layer is driven through both entry points: `dispatch_in` for the layer's own key
//! domain, and `dispatch` for the walk itself — the priority order, the first consumer
//! winning, and the fall-through to the host. The sweeps run over every keysym in the
//! ranges the routing table reads plus a deterministic pseudo-random tail, so a row added
//! to the table is exercised here without anybody remembering to add it.
//!
//! Two rules live ahead of the walk rather than in a layer — a release is never ours and
//! a modifier press is never ours — and so do the chords that ask for a panel, which are the
//! entry to the overlay layer and not a row of it. The per-layer tests assert what the layer
//! says about such a key while the walk tests assert what `dispatch` answers. The two are
//! different on purpose, and the difference is asserted rather than assumed.
//!
//! The layers that read the session answer through the arbitrator, so the sessions they are
//! asked about are real ones: a composition built by typing through the state machine, with
//! the candidates, the page and the highlight a live composition actually holds.

use ime_core::lm::InMemoryLm;
use ime_core::state::{Session, SessionConfig, SessionEnv, SessionState};
use ime_core::viterbi::Decoder;
use ime_types::{
    ImeError, KeyAction, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef,
};

use crate::engine::*;

use super::panel::{KEY_P, KEY_P_UPPER, KEY_QUESTION, KEY_SLASH, UI_NOT_IMPLEMENTED_CODE};

/// The modifier states the sweeps run over: a bare press and the four a user can hold.
const STATES: [u32; 5] = [0, SHIFT, CTRL, CTRL | SHIFT, CTRL | SHIFT | ALT];

/// `FcitxKey_F35`, a keysym no row of the table names.
const KEY_F35: u32 = 0xffbe;

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 7,
    }
}

/// A key release of `sym` with `state` held.
fn release(sym: u32, state: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: true,
        time_ms: 7,
    }
}

/// Every keysym the sweeps run over.
///
/// The two ranges cover every row the table has — the ASCII rows and the function-key
/// block — and the pseudo-random tail covers the keysyms it must never claim.
fn corpus() -> Vec<u32> {
    let mut syms: Vec<u32> = (0x20..=0x100).collect();
    syms.extend(0xff00..=0xffff);
    syms.extend(pseudo_random_syms(200));
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

/// The modifier set every chord that asks for a panel carries.
const CHORD_STATE: u32 = CTRL | SHIFT;

/// The chords that ask for a panel, as the host spells them.
///
/// Written out rather than read from the code under test: a keysym added to or dropped from
/// the matcher has to be changed here too before the sweeps below agree with it.
fn panel_chord_syms() -> [u32; 4] {
    [KEY_SLASH, KEY_QUESTION, KEY_P, KEY_P_UPPER]
}

/// The panel a chord names for a session that is (not) composing.
///
/// Written out rather than read from the code under test, for the same reason the syms are:
/// the slash entry's answer depends on the session state, and a fixture that mirrored the
/// matcher would agree with whatever the matcher did.
fn panel_for(sym: u32, composing: bool) -> Overlay {
    match (sym, composing) {
        (KEY_SLASH | KEY_QUESTION, true) => Overlay::CheatSheet,
        (KEY_SLASH | KEY_QUESTION, false) => Overlay::CommandPalette,
        _ => Overlay::Diagnostics,
    }
}

/// Whether this key is one of the chords that asks for a panel.
fn is_panel_chord(sym: u32, state: u32) -> bool {
    state == CHORD_STATE && panel_chord_syms().contains(&sym)
}

/// The layers in priority order, the host layer included.
///
/// Written out rather than read from the code under test, so that a change to the walk's
/// order has to be made in two places before this test agrees with it.
fn walk_order() -> [KeyContext; 4] {
    [
        KeyContext::ModalOverlay,
        KeyContext::Composition,
        KeyContext::Session,
        KeyContext::Host,
    ]
}

// ── The doubles ──────────────────────────────────────────────────────────────────
//
// Everything is in memory: a dictionary of eight rows, a language model, and a
// user-frequency source that answers nothing. No test here touches a dictionary file, the
// clock, the environment or a display server, and none of them sleeps.

/// The dictionary the composing setups decode against.
///
/// Eight readings of `ni`, in ranking order: more than one page of five, so the page keys,
/// the highlight moves and the digit row have something to act on — and so that a digit past
/// the end of the page and a page key with no page to turn to are reachable, which are the
/// keys the arbitration hands back while a composition is live.
struct TestLexicon;

impl TestLexicon {
    /// Every entry, as `(key, word, weight)`, in the order the ranking reads them.
    const WORDS: [(&'static str, &'static str, u32); 8] = [
        ("ni", "你", 900_000),
        ("ni", "尼", 800_000),
        ("ni", "泥", 700_000),
        ("ni", "拟", 600_000),
        ("ni", "逆", 500_000),
        ("ni", "腻", 400_000),
        ("ni", "妮", 300_000),
        ("ni", "霓", 200_000),
    ];
}

impl Lexicon for TestLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = Self::WORDS
            .iter()
            .filter(|(row, _, _)| *row == key)
            .map(|&(_, text, weight)| WordRef {
                text,
                weight,
                syl_count: 1,
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
/// Nothing the walk asks reaches the user's frequencies: the question is what a session
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

/// A live composition of `ni`, built the way a user builds one.
///
/// Driven through the state machine one character at a time rather than written together by
/// hand, so that the candidates, the page, the highlight and the caret are the ones a real
/// composition holds — which is exactly what the arbitration reads. The session owns
/// everything it decoded, so it outlives the sources it was built from.
fn composing_session() -> Session {
    let lexicon = TestLexicon;
    let user = SilentUser;
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let env = SessionEnv {
        decoder: &decoder,
        lexicon: &lexicon,
        user_freq: &user,
        lm: &lm,
    };
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "ni".chars() {
        let effects = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
        assert!(!effects.is_empty(), "typing {ch:?} composes");
    }
    assert_eq!(session.state, SessionState::Composing);
    session
}

// ── The setups ───────────────────────────────────────────────────────────────────

/// The state the walk is driven in, as the session and the overlay behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// A panel is open over a live composition.
    OverlayOverComposition,
    /// A composition is live.
    Composing,
    /// A session exists and nothing is composing.
    Idle,
    /// Temporary English is on, with a composition behind it.
    TempEnglish,
    /// The host delivered a key for a context that never had a session.
    Absent,
}

impl Setup {
    /// Every setup, for the sweeps that have to cover all of them.
    const ALL: [Setup; 5] = [
        Setup::OverlayOverComposition,
        Setup::Composing,
        Setup::Idle,
        Setup::TempEnglish,
        Setup::Absent,
    ];

    /// The session a setup runs against.
    ///
    /// The composing setups are real compositions decoded from the doubles, because the
    /// arbitration reads the candidate list and the paging state and a session written
    /// together by hand would hold neither. The other two are the states the walk reaches
    /// without one, and the arbitration answers them from the state alone.
    fn session(self) -> Option<Session> {
        match self {
            Setup::OverlayOverComposition | Setup::Composing => Some(composing_session()),
            Setup::TempEnglish => {
                let mut session = composing_session();
                session.temp_english = true;
                Some(session)
            }
            Setup::Idle => Some(Session::new()),
            Setup::Absent => None,
        }
    }
}

/// A dispatcher in the state a setup describes, with the shipped configuration.
fn dispatcher_in(setup: Setup) -> Dispatcher {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    if setup == Setup::OverlayOverComposition {
        dispatcher.open_overlay(Overlay::CheatSheet);
    }
    dispatcher
}

/// The view of a session that may not exist.
fn view_of(session: Option<&Session>) -> SessionView<'_> {
    match session {
        Some(session) => SessionView::new(session),
        None => SessionView::absent(),
    }
}

/// Runs one key through the walk in `setup`, against `session`.
fn dispatch_in_setup(setup: Setup, session: Option<&Session>, event: &KeyEvent) -> Consumed {
    let mut dispatcher = dispatcher_in(setup);
    dispatcher.dispatch(event, &view_of(session))
}

/// Asserts that one layer hands every key of the corpus back, in every modifier state.
///
/// The three layers that defer outside their own key domain are swept here rather than in a
/// copy of the loop each: a key a layer starts claiming is then visible in one place.
fn assert_layer_defers(setup: Setup, context: KeyContext, session: Option<&Session>) {
    let view = view_of(session);
    let mut dispatcher = dispatcher_in(setup);
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(context, &press(sym, state), &view),
                Consumed::Ignored,
                "{setup:?} layer {context:?} sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_key_context_priority_order_is_modal_first_and_host_last() {
    assert!(KeyContext::ModalOverlay < KeyContext::Composition);
    assert!(KeyContext::Composition < KeyContext::Session);
    assert!(KeyContext::Session < KeyContext::Host);
    let mut sorted = [
        KeyContext::Host,
        KeyContext::Session,
        KeyContext::Composition,
        KeyContext::ModalOverlay,
    ];
    sorted.sort();
    assert_eq!(sorted, walk_order());
}

#[test]
fn test_key_event_round_trips_through_the_host_event() {
    for sym in corpus() {
        for state in STATES {
            for is_release in [false, true] {
                let event = KeyEvent {
                    sym,
                    state,
                    is_release,
                    time_ms: 7,
                };
                assert_eq!(KeyEvent::from(event.as_host_event()), event);
            }
        }
    }
}

#[test]
fn test_action_for_matches_the_routing_table() {
    let bindings = KeyBindings::default();
    let dispatcher = Dispatcher::new(bindings);
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            assert_eq!(
                dispatcher.action_for(&event),
                translate_key(&event.as_host_event(), &bindings),
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatcher_overlay_tracks_what_is_open() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    assert_eq!(dispatcher.overlay(), None);
    dispatcher.open_overlay(Overlay::CommandPalette);
    assert_eq!(dispatcher.overlay(), Some(Overlay::CommandPalette));
    // Opening a second panel replaces the first rather than stacking two.
    dispatcher.open_overlay(Overlay::Diagnostics);
    assert_eq!(dispatcher.overlay(), Some(Overlay::Diagnostics));
    dispatcher.close_overlay();
    assert_eq!(dispatcher.overlay(), None);
    // Closing one that is already closed is not an error.
    dispatcher.close_overlay();
    assert_eq!(dispatcher.overlay(), None);
}

#[test]
fn test_dispatcher_set_bindings_changes_what_the_walk_claims() {
    let composing = Setup::Composing.session();
    let view = view_of(composing.as_ref());
    let zero = press(KEY_0, 0);

    let mut passthrough = Dispatcher::new(KeyBindings::default());
    assert_eq!(passthrough.dispatch(&zero, &view), Consumed::Ignored);

    let flipping = KeyBindings {
        digit_zero: DigitZero::Flip,
        ..KeyBindings::default()
    };
    let mut reloaded = Dispatcher::new(KeyBindings::default());
    reloaded.set_bindings(flipping);
    assert_eq!(reloaded.dispatch(&zero, &view), Consumed::Consumed);
}

#[test]
fn test_dispatcher_set_session_config_changes_what_the_walk_keeps() {
    // The arbitration measures a typed character against the length limit the session
    // configuration declares, so a reload that lowers it changes which keys the plugin
    // keeps. The composition holds two bytes and the limit below leaves room for neither.
    let composing = Setup::Composing.session();
    let view = view_of(composing.as_ref());
    let letter = press(KEY_A, 0);

    let mut shipped = Dispatcher::new(KeyBindings::default());
    assert_eq!(shipped.dispatch(&letter, &view), Consumed::Consumed);

    let mut limited = Dispatcher::new(KeyBindings::default());
    limited.set_session_config(SessionConfig::new(2, 5, true, 720));
    assert_eq!(
        limited.dispatch(&letter, &view),
        Consumed::Ignored,
        "a full input hands the character to the application"
    );
}

#[test]
fn test_session_view_reports_the_session_it_wraps() {
    let absent = SessionView::absent();
    assert!(absent.is_absent());
    assert_eq!(absent.state(), None);
    assert!(!absent.temp_english());
    assert_eq!(absent.candidate_count(), 0);

    let mut session = Session::new();
    let fresh = SessionView::new(&session);
    assert!(!fresh.is_absent());
    assert_eq!(fresh.state(), Some(SessionState::Idle));
    assert!(!fresh.temp_english());
    assert_eq!(fresh.candidate_count(), 0);

    // The view borrows the session, so the second one is built after the mutation.
    session.temp_english = true;
    let loaded = SessionView::new(&session);
    assert!(loaded.temp_english());
    assert_eq!(loaded.state(), Some(SessionState::Idle));

    // The count is the decode's own answer, and a live composition has candidates in it.
    let composing = composing_session();
    let view = SessionView::new(&composing);
    assert_eq!(view.state(), Some(SessionState::Composing));
    assert!(view.candidate_count() > 0, "the fixture composes something");
}

#[test]
fn test_session_view_arbitrate_keeps_only_what_the_session_can_act_on() {
    let cfg = SessionConfig::default();
    let composing = composing_session();
    let view = SessionView::new(&composing);
    // The first three are keys the composition acts on — the highlight, the digit that names
    // a candidate, and the engine's own mode bit. The last three it does not, though the
    // routing table names every one of them: that half of the decision is what the table
    // cannot see, and keeping such a key is the swallowed-key defect.
    let rows = [
        (KeyAction::CommitHighlighted, Consumed::Consumed),
        (KeyAction::SelectIndex(1), Consumed::Consumed),
        (KeyAction::ToggleLang, Consumed::Consumed),
        (KeyAction::SelectIndex(9), Consumed::Ignored),
        (KeyAction::PagePrev, Consumed::Ignored),
        (KeyAction::Ignore, Consumed::Ignored),
    ];
    for (action, expected) in rows {
        assert_eq!(view.arbitrate(action, &cfg), expected, "{action:?}");
    }

    // A view with no session behind it keeps nothing at all.
    let absent = SessionView::absent();
    for action in [
        KeyAction::CommitHighlighted,
        KeyAction::ToggleLang,
        KeyAction::InputChar('a'),
    ] {
        assert_eq!(
            absent.arbitrate(action, &cfg),
            Consumed::Ignored,
            "{action:?} has nowhere to go without a session"
        );
    }
}

#[test]
fn test_dispatch_in_host_layer_never_consumes_a_key() {
    // The host layer is the answer the walk falls through to, and it is written as a layer
    // so that the tree has a row for the case rather than an implicit branch.
    let session = Setup::Composing.session();
    assert_layer_defers(Setup::Composing, KeyContext::Host, session.as_ref());
}

#[test]
fn test_dispatch_in_composition_layer_answers_through_the_arbitrator() {
    // Inside a live composition the layer's answer is the arbitrator's: the table's meaning
    // of the key, and whether the session would act on the action it named. A routed key
    // nothing can act on is handed back, which is the half of the decision the table cannot
    // see and the reason this layer is not a keyboard grab.
    let cfg = SessionConfig::default();
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            let action = dispatcher.action_for(&event);
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Composition, &event, &view),
                arbitrate(action, session.as_ref(), &cfg),
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_composition_layer_hands_back_a_key_the_session_cannot_act_on() {
    // The other half of the same rule, and the defect the arbitration closes: every key
    // below is one the table names, and the composition has nothing to do with any of them.
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let cases = [
        (KEY_9, 0, "a digit past the end of the page"),
        (KEY_UP, 0, "the first page has nothing before it"),
        (KEY_MINUS, 0, "and no page to turn back to"),
        (KEY_TAB, SHIFT, "the highlight is on the first candidate"),
        (KEY_RIGHT, 0, "the caret is already at the end of the input"),
    ];
    for (sym, state, why) in cases {
        let event = press(sym, state);
        assert_ne!(
            dispatcher.action_for(&event),
            KeyAction::Ignore,
            "the table names {sym:#06x}"
        );
        assert_eq!(
            dispatcher.dispatch_in(KeyContext::Composition, &event, &view),
            Consumed::Ignored,
            "{why}: sym {sym:#06x} state {state:#x}"
        );
    }
}

#[test]
fn test_dispatch_in_composition_layer_is_entered_by_a_live_composition_alone() {
    // The layer's entry condition, and the reason a composition is not a keyboard grab: a
    // session that is composing and is not in temporary English. Every other state, and a
    // context with no session at all, falls through to the layers below.
    for setup in [Setup::Idle, Setup::TempEnglish, Setup::Absent] {
        let session = setup.session();
        assert_layer_defers(setup, KeyContext::Composition, session.as_ref());
    }
}

#[test]
fn test_dispatch_in_session_layer_takes_only_the_keys_an_idle_session_owns() {
    let session = Setup::Idle.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let cases = [
        // The two actions an idle session acts on.
        (KEY_A, 0, Consumed::Consumed),
        (KEY_Z, SHIFT, Consumed::Consumed),
        (KEY_E, CTRL | SHIFT, Consumed::Consumed),
        // The mode chords the engine owns.
        // The language chord belongs to the host: the routing table claims it no more,
        // so Ctrl+Space travels on untouched.
        (KEY_SPACE, CTRL, Consumed::Ignored),
        (KEY_SPACE, SHIFT, Consumed::Consumed),
        (KEY_PERIOD, CTRL, Consumed::Consumed),
        // Everything else is the application's: the keys the table names and the ones it
        // does not. These are the keys the plugin used to eat with nothing composing.
        (KEY_SPACE, 0, Consumed::Ignored),
        (KEY_0 + 3, 0, Consumed::Ignored),
        (KEY_RETURN, 0, Consumed::Ignored),
        (KEY_BACKSPACE, 0, Consumed::Ignored),
        (KEY_ESCAPE, 0, Consumed::Ignored),
        (KEY_TAB, 0, Consumed::Ignored),
        (KEY_TAB, SHIFT, Consumed::Ignored),
        (KEY_LEFT, 0, Consumed::Ignored),
        (KEY_MINUS, 0, Consumed::Ignored),
        (KEY_EQUAL, 0, Consumed::Ignored),
        (KEY_UP, 0, Consumed::Ignored),
        (KEY_F35, 0, Consumed::Ignored),
    ];
    for (sym, state, expected) in cases {
        assert_eq!(
            dispatcher.dispatch_in(KeyContext::Session, &press(sym, state), &view),
            expected,
            "sym {sym:#06x} state {state:#x}"
        );
    }
}

#[test]
fn test_dispatch_in_session_layer_defers_while_the_host_finishes_a_commit() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for state in [SessionState::Cancelling, SessionState::Committing] {
        let mut session = Session::new();
        session.state = state;
        let view = SessionView::new(&session);
        for sym in corpus() {
            for held in STATES {
                let event = press(sym, held);
                if is_engine_owned(&dispatcher, &event) {
                    continue;
                }
                assert_eq!(
                    dispatcher.dispatch_in(KeyContext::Session, &event, &view),
                    Consumed::Ignored,
                    "state {state:?} sym {sym:#06x} state {held:#x}"
                );
            }
        }
    }
}

/// Whether the engine owns this key outright, in every session state.
///
/// The three mode bits are Fcitx5's input-method state and the plugin's own output choices,
/// so a layer answers for them whether or not anything is composing — that is the design,
/// not an exception to it. The sweep below is about every other key.
fn is_engine_owned(dispatcher: &Dispatcher, event: &KeyEvent) -> bool {
    is_mode_chord(dispatcher.action_for(event))
}

/// Asserts the overlay layer hands one key back and leaves the panel as it was.
fn assert_overlay_defers(sym: u32, view: &SessionView<'_>) {
    let mut dispatcher = dispatcher_in(Setup::OverlayOverComposition);
    assert_eq!(
        dispatcher.dispatch_in(KeyContext::ModalOverlay, &press(sym, 0), view),
        Consumed::Ignored,
        "sym {sym:#06x} must reach the layers below"
    );
    assert_eq!(
        dispatcher.overlay(),
        Some(Overlay::CheatSheet),
        "sym {sym:#06x} must leave the panel open"
    );
}

#[test]
fn test_dispatch_in_overlay_layer_takes_escape_alone_while_the_panel_is_empty() {
    let session = Setup::OverlayOverComposition.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = dispatcher_in(Setup::OverlayOverComposition);
    assert_eq!(
        dispatcher.dispatch_in(KeyContext::ModalOverlay, &press(KEY_ESCAPE, 0), &view),
        Consumed::Consumed,
        "Escape is the one key an empty panel owns"
    );
    assert_eq!(dispatcher.overlay(), None, "and it closes the panel");
    // The keys a panel with content would navigate with are not an empty panel's either:
    // taking them for a panel that draws nothing is a key the user pressed and saw nothing
    // happen on, which is the defect this layer's design exists to avoid.
    for sym in [KEY_UP, KEY_DOWN, KEY_RETURN] {
        assert_overlay_defers(sym, &view);
    }
    // And neither is anything else: the composition behind the panel keeps working.
    for sym in [KEY_TAB, KEY_A, KEY_0, KEY_MINUS, KEY_SPACE, KEY_BACKSPACE] {
        assert_overlay_defers(sym, &view);
    }
}

#[test]
fn test_dispatch_in_overlay_layer_defers_when_no_overlay_is_open() {
    let session = Setup::Composing.session();
    assert_layer_defers(Setup::Composing, KeyContext::ModalOverlay, session.as_ref());
}

#[test]
fn test_dispatch_closes_the_overlay_on_escape() {
    // Every panel the bus can hold answers `Escape` the same way, the cheat sheet included,
    // and the key does not reach the composition behind it.
    for panel in [
        Overlay::CheatSheet,
        Overlay::CommandPalette,
        Overlay::Diagnostics,
    ] {
        let session = Setup::Composing.session();
        let view = view_of(session.as_ref());
        let mut dispatcher = Dispatcher::new(KeyBindings::default());
        dispatcher.open_overlay(panel);
        let escape = press(KEY_ESCAPE, 0);
        assert_eq!(
            dispatcher.dispatch(&escape, &view),
            Consumed::Consumed,
            "{panel:?} is the panel's key"
        );
        assert_eq!(dispatcher.overlay(), None, "{panel:?} is closed");
        // With the panel gone the same key is the layers' again, and it reaches the
        // composition.
        assert_eq!(dispatcher.dispatch(&escape, &view), Consumed::Consumed);
        assert_eq!(dispatcher.overlay(), None);
    }
}

#[test]
fn test_dispatch_opens_the_panel_each_chord_names() {
    // The two chords are the plugin's whether or not anything is composing: they open a
    // host-layer mode, not something a composition owns. Which panel the slash shapes name
    // is the session state's call — the cheat sheet mid-composition, the palette otherwise.
    for setup in [Setup::Composing, Setup::Idle] {
        let composing = matches!(setup, Setup::Composing);
        let session = setup.session();
        let view = view_of(session.as_ref());
        for sym in panel_chord_syms() {
            let mut dispatcher = dispatcher_in(setup);
            let event = press(sym, CHORD_STATE);
            assert_eq!(
                dispatcher.dispatch(&event, &view),
                Consumed::Consumed,
                "setup {setup:?} sym {sym:#06x}"
            );
            assert_eq!(
                dispatcher.overlay(),
                Some(panel_for(sym, composing)),
                "setup {setup:?} sym {sym:#06x}"
            );
            // The key is kept and the request is recorded, so the caller can report that
            // nothing draws the panel yet.
            assert_eq!(
                dispatcher.take_overlay_request(),
                Some(panel_for(sym, composing)),
                "setup {setup:?} sym {sym:#06x}"
            );
            assert_eq!(
                dispatcher.take_overlay_request(),
                None,
                "sym {sym:#06x}: a request is taken once"
            );
        }
    }
}

#[test]
fn test_dispatch_replaces_the_open_panel_with_the_one_the_next_chord_names() {
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    // The cheat sheet stands for a panel another gesture opened.
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.open_overlay(Overlay::CheatSheet);
    assert_eq!(
        dispatcher.dispatch(&press(KEY_P, CHORD_STATE), &view),
        Consumed::Consumed
    );
    assert_eq!(
        dispatcher.overlay(),
        Some(Overlay::Diagnostics),
        "panels are replaced rather than stacked"
    );
    let asked = dispatcher.take_overlay_request();
    assert_eq!(asked, Some(Overlay::Diagnostics));
}

#[test]
fn test_dispatch_hands_the_panel_chords_back_with_an_extra_modifier() {
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    // A chord is its whole modifier set: one extra modifier is another key, and one the
    // desktop environment owns.
    for sym in panel_chord_syms() {
        for extra in [ALT, SUPER, HYPER, META, SUPER2] {
            let mut dispatcher = dispatcher_in(Setup::Composing);
            let event = press(sym, CHORD_STATE | extra);
            assert_eq!(
                dispatcher.dispatch(&event, &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {:#x}",
                CHORD_STATE | extra
            );
            assert_eq!(dispatcher.overlay(), None, "sym {sym:#06x}");
            let asked = dispatcher.take_overlay_request();
            assert_eq!(asked, None, "sym {sym:#06x}");
        }
    }
    // `Ctrl` or `Shift` alone is not the chord either, and the slash shapes carry nothing
    // else on the routing table, so the whole key reaches the application.
    for sym in [KEY_SLASH, KEY_QUESTION] {
        for state in [CTRL, SHIFT, 0] {
            let mut dispatcher = dispatcher_in(Setup::Composing);
            assert_eq!(
                dispatcher.dispatch(&press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_hands_the_panel_chords_back_in_temporary_english() {
    // Temporary English hands every key to the application, the chords included: that is the
    // whole meaning of the mode, and a panel opening out of it would take a key the mode
    // promised to pass on.
    let session = Setup::TempEnglish.session();
    let view = view_of(session.as_ref());
    for sym in panel_chord_syms() {
        let mut dispatcher = dispatcher_in(Setup::TempEnglish);
        assert_eq!(
            dispatcher.dispatch(&press(sym, CHORD_STATE), &view),
            Consumed::Ignored,
            "sym {sym:#06x}"
        );
        assert_eq!(dispatcher.overlay(), None, "sym {sym:#06x}");
        let asked = dispatcher.take_overlay_request();
        assert_eq!(asked, None, "sym {sym:#06x}");
    }
}

#[test]
fn test_dispatch_hands_the_panel_chords_back_without_a_session() {
    // A context the plugin was never activated in is not one to open a panel in.
    let view = SessionView::absent();
    for sym in panel_chord_syms() {
        let mut dispatcher = Dispatcher::new(KeyBindings::default());
        assert_eq!(
            dispatcher.dispatch(&press(sym, CHORD_STATE), &view),
            Consumed::Ignored,
            "sym {sym:#06x}"
        );
        assert_eq!(dispatcher.overlay(), None, "sym {sym:#06x}");
        let asked = dispatcher.take_overlay_request();
        assert_eq!(asked, None, "sym {sym:#06x}");
    }
}

#[test]
fn test_dispatcher_take_overlay_request_answers_once_and_keeps_the_newest() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    // Nothing has asked for a panel yet.
    assert_eq!(dispatcher.take_overlay_request(), None);
    // Two chords before the caller takes the first: the panel asked for last is the one
    // worth opening, and a request is taken exactly once.
    dispatcher.open_panel(Overlay::CommandPalette);
    dispatcher.open_panel(Overlay::Diagnostics);
    let asked = dispatcher.take_overlay_request();
    assert_eq!(asked, Some(Overlay::Diagnostics));
    assert_eq!(dispatcher.take_overlay_request(), None);
}

#[test]
fn test_dispatch_marks_a_modifier_used_when_a_panel_chord_is_claimed() {
    // A modifier the user pressed a chord with is not one they meant to hold, so the chord
    // marks the hold the way a claimed key does and the release is a no-op.
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    assert!(dispatcher.arm_hold(&press(KEY_SHIFT_L, SHIFT), true));
    let chord = KeyEvent {
        sym: KEY_SLASH,
        state: CHORD_STATE,
        is_release: false,
        time_ms: 40,
    };
    assert_eq!(dispatcher.dispatch(&chord, &view), Consumed::Consumed);
    let release = KeyEvent {
        sym: KEY_SHIFT_L,
        state: 0,
        is_release: true,
        time_ms: 600,
    };
    assert_eq!(dispatcher.dispatch(&release, &view), Consumed::Ignored);
    assert_eq!(
        dispatcher.take_hold_outcome(),
        None,
        "the modifier was typed with, so the release means nothing"
    );
}

#[test]
fn test_dispatch_leaves_the_composition_behind_a_panel_untouched() {
    let session = Setup::Composing.session();
    let view = view_of(session.as_ref());
    let mut dispatcher = dispatcher_in(Setup::Composing);
    assert_eq!(
        dispatcher.dispatch(&press(KEY_SLASH, CHORD_STATE), &view),
        Consumed::Consumed
    );
    // The panel is a host-layer mode on top of the session, whatever it draws: the chord
    // must not have disturbed the composition, which is still live and still takes the
    // next letter once the panel is gone.
    assert_eq!(
        session.as_ref().map(|s| s.state),
        Some(SessionState::Composing)
    );
    assert_eq!(
        dispatcher.dispatch(&press(KEY_A, 0), &view),
        Consumed::Consumed
    );
}

#[test]
fn test_overlay_name_and_not_implemented_code_are_stable() {
    // The codes are matched by diagnostics and tests, so their spelling is pinned here.
    assert_eq!(UI_NOT_IMPLEMENTED_CODE, "ui/not-implemented");
    for (panel, name) in [
        (Overlay::CheatSheet, "cheat-sheet"),
        (Overlay::CommandPalette, "command-palette"),
        (Overlay::Diagnostics, "diagnostics"),
    ] {
        assert_eq!(panel.name(), name);
        assert_eq!(
            panel.not_implemented_code(),
            format!("{UI_NOT_IMPLEMENTED_CODE}: {name}")
        );
    }
}

#[test]
fn test_dispatch_returns_the_first_layer_that_claims_the_key() {
    for setup in Setup::ALL {
        let session = setup.session();
        let view = view_of(session.as_ref());
        for sym in corpus() {
            for state in STATES {
                let event = press(sym, state);
                if is_shift_press(&event.as_host_event()) || is_panel_chord(sym, state) {
                    // Both are rules ahead of the walk rather than layers in it — the
                    // modifier press and the panel chords — and each has a test of its own.
                    continue;
                }
                let mut probe = dispatcher_in(setup);
                let expected = walk_order()
                    .iter()
                    .map(|context| probe.dispatch_in(*context, &event, &view))
                    .find(|answer| *answer != Consumed::Ignored)
                    .unwrap_or(Consumed::Ignored);
                assert_eq!(
                    dispatch_in_setup(setup, session.as_ref(), &event),
                    expected,
                    "setup {setup:?} sym {sym:#06x} state {state:#x}"
                );
            }
        }
    }
}

#[test]
fn test_dispatch_hands_a_key_back_only_after_every_layer_declined_it() {
    let event = press(KEY_F35, 0);
    for setup in Setup::ALL {
        let session = setup.session();
        let view = view_of(session.as_ref());
        let mut probe = dispatcher_in(setup);
        for context in walk_order() {
            assert_eq!(
                probe.dispatch_in(context, &event, &view),
                Consumed::Ignored,
                "setup {setup:?} layer {context:?}"
            );
        }
        assert_eq!(
            dispatch_in_setup(setup, session.as_ref(), &event),
            Consumed::Ignored,
            "setup {setup:?}"
        );
    }
}

#[test]
fn test_dispatch_without_an_overlay_matches_the_arbitrator_key_by_key() {
    // The walk adds three rules ahead of the arbitration — a release and a modifier press
    // are never the plugin's, and the chords that ask for a panel are the bus's own rather
    // than a session's — and no decision of its own: for every other key it answers what the
    // arbitrator answers about the action the table gave that key. The walk and the
    // arbitration are therefore one answer, which is what keeps a second copy of the claim
    // decision from growing inside a layer.
    let cfg = SessionConfig::default();
    for setup in [
        Setup::Composing,
        Setup::Idle,
        Setup::TempEnglish,
        Setup::Absent,
    ] {
        let session = setup.session();
        let view = view_of(session.as_ref());
        let panels = !matches!(setup, Setup::Absent | Setup::TempEnglish);
        for sym in corpus() {
            for state in STATES {
                let event = press(sym, state);
                // A fresh dispatcher per key: a chord leaves a panel open behind it, and the
                // question here is what one key answers.
                let mut dispatcher = dispatcher_in(setup);
                let expected = if is_shift_press(&event.as_host_event()) {
                    Consumed::Ignored
                } else if panels && is_panel_chord(sym, state) {
                    Consumed::Consumed
                } else {
                    arbitrate(dispatcher.action_for(&event), session.as_ref(), &cfg)
                };
                assert_eq!(
                    dispatcher.dispatch(&event, &view),
                    expected,
                    "setup {setup:?} sym {sym:#06x} state {state:#x}"
                );
            }
        }
    }
}

#[test]
fn test_dispatch_with_an_empty_overlay_answers_what_the_bare_walk_answers() {
    // An overlay with nothing behind it changes no answer: the layer owns the `Escape` that
    // closes it and no other key, so every key reaches exactly the layer it would have
    // reached with no panel open. Taking a key for a panel that does not exist is the
    // swallowed-key defect, and this is the sweep that says so.
    let open = Setup::OverlayOverComposition.session();
    let bare = Setup::Composing.session();
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            assert_eq!(
                dispatch_in_setup(Setup::OverlayOverComposition, open.as_ref(), &event),
                dispatch_in_setup(Setup::Composing, bare.as_ref(), &event),
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_never_claims_a_release_or_a_modifier_press() {
    // The two rules ahead of the walk, which are properties of the key rather than of a
    // layer: the host delivers both edges of every key, and the held `Shift` is the host's
    // own temporary switch. Taking either would take half of a gesture from the
    // application.
    for setup in Setup::ALL {
        let session = setup.session();
        for state in STATES {
            for sym in corpus() {
                assert_eq!(
                    dispatch_in_setup(setup, session.as_ref(), &release(sym, state)),
                    Consumed::Ignored,
                    "setup {setup:?} release of {sym:#06x} state {state:#x}"
                );
            }
            for sym in [KEY_SHIFT_L, KEY_SHIFT_R] {
                assert_eq!(
                    dispatch_in_setup(setup, session.as_ref(), &press(sym, state)),
                    Consumed::Ignored,
                    "setup {setup:?} modifier press {sym:#06x} state {state:#x}"
                );
            }
        }
    }
}

#[test]
fn test_dispatch_answers_every_context_and_every_key() {
    // The totality of the walk: no combination of layer and key is left without an
    // answer, and the sweep is counted so that an empty run cannot pass.
    let keys = corpus();
    let (mut claimed, mut handed_back, mut chained) = (0usize, 0usize, 0usize);
    for setup in Setup::ALL {
        let session = setup.session();
        for sym in &keys {
            for state in STATES {
                match dispatch_in_setup(setup, session.as_ref(), &press(*sym, state)) {
                    Consumed::Consumed => claimed += 1,
                    Consumed::Ignored => handed_back += 1,
                    Consumed::ChainPending => chained += 1,
                }
            }
        }
    }
    assert!(claimed > 0, "some context must claim a key");
    assert!(handed_back > 0, "and some context must hand one back");
    assert_eq!(chained, 0, "no sequence is registered, so nothing chains");
    assert_eq!(
        claimed + handed_back + chained,
        Setup::ALL.len() * keys.len() * STATES.len(),
        "every combination of context, key and modifier state must be answered"
    );
}
