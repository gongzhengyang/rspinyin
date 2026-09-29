//! The key walk, layer by layer.
//!
//! Every layer is driven through both entry points: `dispatch_in` for the layer's own key
//! domain, and `dispatch` for the walk itself — the priority order, the first consumer
//! winning, and the fall-through to the host. The sweeps run over every keysym in the
//! ranges the routing table reads plus a deterministic pseudo-random tail, so a row added
//! to the table is exercised here without anybody remembering to add it.
//!
//! Two rules live ahead of the walk rather than in a layer — a release is never ours and
//! a modifier press is never ours — so the per-layer tests assert what the layer says
//! about such a key while the walk tests assert what `dispatch` answers. The two are
//! different on purpose, and the difference is asserted rather than assumed.

use ime_core::state::{Session, SessionState};

use crate::engine::*;

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

/// Whether the overlay layer owns this keysym: the four keys a panel's domain names.
fn is_panel_key(sym: u32) -> bool {
    matches!(sym, KEY_ESCAPE | KEY_UP | KEY_DOWN | KEY_RETURN)
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
}

/// The session a setup runs against, with its state set directly.
///
/// The walk reads the session's state and its temporary-English flag and nothing else, so
/// the states are set on a fresh session rather than reached through the machine: a
/// composition built by stepping would need a decoder and a dictionary for facts no layer
/// looks at.
fn session_of(setup: Setup) -> Option<Session> {
    let state = match setup {
        Setup::OverlayOverComposition | Setup::Composing | Setup::TempEnglish => {
            SessionState::Composing
        }
        Setup::Idle => SessionState::Idle,
        Setup::Absent => return None,
    };
    let mut session = Session::new();
    session.state = state;
    session.temp_english = setup == Setup::TempEnglish;
    Some(session)
}

/// A dispatcher in the state a setup describes, with the shipped bindings.
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

/// Runs one key through the walk in `setup`.
fn dispatch_in_setup(setup: Setup, event: &KeyEvent) -> Consumed {
    let mut dispatcher = dispatcher_in(setup);
    let session = session_of(setup);
    let view = view_of(session.as_ref());
    dispatcher.dispatch(event, &view)
}

/// What the routing table alone says about one key.
///
/// This is the composition layer's answer, without the modifier-press rule that `dispatch`
/// applies ahead of the walk.
fn table_verdict(event: &KeyEvent, bindings: &KeyBindings) -> Consumed {
    if claims_key(translate_key(&event.as_host_event(), bindings)) {
        Consumed::Consumed
    } else {
        Consumed::Ignored
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
    let composing = session_of(Setup::Composing);
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
    // `candidate_count` delegates to the session's own decode result. A fresh session
    // holds no candidates and there is no public way to seed one -- the buffers belong to
    // the decode workspace and `Session::decoded` hands them out read-only -- so the
    // assertion is on the delegation rather than on a number. The non-empty case is
    // covered where the decoder actually runs, in `ime-core`'s state tests.
    assert_eq!(loaded.candidate_count(), session.decoded().candidates.len());
}

#[test]
fn test_dispatch_in_host_layer_never_consumes_a_key() {
    let session = session_of(Setup::Composing);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Host, &press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_composition_layer_follows_the_routing_table() {
    let bindings = KeyBindings::default();
    let mut dispatcher = Dispatcher::new(bindings);
    let session = session_of(Setup::Composing);
    let view = view_of(session.as_ref());
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Composition, &event, &view),
                table_verdict(&event, &bindings),
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_composition_layer_defers_when_nothing_is_composing() {
    let session = session_of(Setup::Idle);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Composition, &press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_composition_layer_defers_in_temporary_english() {
    let session = session_of(Setup::TempEnglish);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Composition, &press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_composition_layer_defers_without_a_session() {
    let view = SessionView::absent();
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Composition, &press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_session_layer_takes_only_the_keys_an_idle_session_owns() {
    let session = session_of(Setup::Idle);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let cases = [
        // The two actions an idle session acts on.
        (KEY_A, 0, Consumed::Consumed),
        (KEY_Z, SHIFT, Consumed::Consumed),
        (KEY_E, CTRL | SHIFT, Consumed::Consumed),
        // The mode chords the engine owns.
        (KEY_SPACE, CTRL, Consumed::Consumed),
        (KEY_SPACE, SHIFT, Consumed::Consumed),
        (KEY_PERIOD, CTRL, Consumed::Consumed),
        // Everything else is the application's: the keys the table names and the ones it
        // does not.
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
fn test_dispatch_in_session_layer_defers_without_a_session() {
    let view = SessionView::absent();
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            if is_engine_owned(&dispatcher, &event) {
                continue;
            }
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Session, &event, &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_in_session_layer_defers_in_temporary_english() {
    // The mode hands every key to the application, the two keys that leave it included:
    // what ends the mode changes no other state and produces no effect.
    let session = session_of(Setup::TempEnglish);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            if is_engine_owned(&dispatcher, &event) {
                continue;
            }
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Session, &event, &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

/// Whether the engine owns this key outright, in every session state.
///
/// The three mode bits are Fcitx5's input-method state and the plugin's own output choices,
/// so the session layer answers for them whether or not anything is composing -- that is
/// the design, not an exception to it. The "defers" tests below are about every other key.
fn is_engine_owned(dispatcher: &Dispatcher, event: &KeyEvent) -> bool {
    super::is_mode_chord(dispatcher.action_for(event))
}

#[test]
fn test_dispatch_in_session_layer_defers_while_composing() {
    // A live composition is the layer above's; reaching this one means the walk already
    // gave the key up there.
    let session = session_of(Setup::Composing);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            if is_engine_owned(&dispatcher, &event) {
                continue;
            }
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::Session, &event, &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
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

#[test]
fn test_dispatch_in_overlay_layer_takes_the_panel_keys_and_defers_the_rest() {
    let session = session_of(Setup::OverlayOverComposition);
    let view = view_of(session.as_ref());
    // A fresh dispatcher per key: `Escape` closes the panel, and the layer below an open
    // panel is a different layer.
    for sym in [KEY_ESCAPE, KEY_UP, KEY_DOWN, KEY_RETURN] {
        let mut dispatcher = dispatcher_in(Setup::OverlayOverComposition);
        assert_eq!(
            dispatcher.dispatch_in(KeyContext::ModalOverlay, &press(sym, 0), &view),
            Consumed::Consumed,
            "sym {sym:#06x} is the panel's"
        );
    }
    // The panel's domain is its four keys: a letter, a digit and the page keys reach the
    // layers below, so a composition behind the panel keeps working.
    for sym in [KEY_A, KEY_0, KEY_MINUS, KEY_SPACE, KEY_BACKSPACE] {
        let mut dispatcher = dispatcher_in(Setup::OverlayOverComposition);
        assert_eq!(
            dispatcher.dispatch_in(KeyContext::ModalOverlay, &press(sym, 0), &view),
            Consumed::Ignored,
            "sym {sym:#06x} must reach the layers below"
        );
    }
}

#[test]
fn test_dispatch_in_overlay_layer_defers_when_no_overlay_is_open() {
    let session = session_of(Setup::Composing);
    let view = view_of(session.as_ref());
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in corpus() {
        for state in STATES {
            assert_eq!(
                dispatcher.dispatch_in(KeyContext::ModalOverlay, &press(sym, state), &view),
                Consumed::Ignored,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_closes_the_overlay_on_escape() {
    let session = session_of(Setup::OverlayOverComposition);
    let view = view_of(session.as_ref());
    let mut dispatcher = dispatcher_in(Setup::OverlayOverComposition);
    let escape = press(KEY_ESCAPE, 0);
    assert_eq!(dispatcher.dispatch(&escape, &view), Consumed::Consumed);
    assert_eq!(dispatcher.overlay(), None, "Escape closes the panel");
    // With the panel gone the same key is the layers' again, and it reaches the
    // composition.
    assert_eq!(dispatcher.dispatch(&escape, &view), Consumed::Consumed);
    assert_eq!(dispatcher.overlay(), None);
}

#[test]
fn test_dispatch_returns_the_first_layer_that_claims_the_key() {
    for setup in Setup::ALL {
        let session = session_of(setup);
        let view = view_of(session.as_ref());
        for sym in corpus() {
            for state in STATES {
                let event = press(sym, state);
                if is_shift_press(&event.as_host_event()) {
                    // The modifier-press rule is ahead of the walk, not in a layer; the
                    // walk's answer for a modifier press is asserted on its own.
                    continue;
                }
                let mut probe = dispatcher_in(setup);
                let expected = walk_order()
                    .iter()
                    .map(|context| probe.dispatch_in(*context, &event, &view))
                    .find(|answer| *answer != Consumed::Ignored)
                    .unwrap_or(Consumed::Ignored);
                assert_eq!(
                    dispatch_in_setup(setup, &event),
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
        let session = session_of(setup);
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
            dispatch_in_setup(setup, &event),
            Consumed::Ignored,
            "setup {setup:?}"
        );
    }
}

#[test]
fn test_dispatch_without_an_overlay_matches_the_routing_table_key_by_key() {
    // The acceptance criterion: with no panel open the walk answers exactly what the
    // routing table answered before the layers existed, for every key of the corpus, plus
    // the one rule the walk adds ahead of the table — a modifier press is the
    // application's.
    let bindings = KeyBindings::default();
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            let expected = if is_shift_press(&event.as_host_event()) {
                Consumed::Ignored
            } else {
                table_verdict(&event, &bindings)
            };
            assert_eq!(
                dispatch_in_setup(Setup::Composing, &event),
                expected,
                "sym {sym:#06x} state {state:#x}"
            );
        }
    }
}

#[test]
fn test_dispatch_with_an_overlay_open_is_transparent_outside_the_panel_keys() {
    for sym in corpus() {
        for state in STATES {
            let event = press(sym, state);
            let open = dispatch_in_setup(Setup::OverlayOverComposition, &event);
            if is_panel_key(sym) {
                assert_eq!(
                    open,
                    Consumed::Consumed,
                    "sym {sym:#06x} is the panel's while it is open"
                );
            } else {
                assert_eq!(
                    open,
                    dispatch_in_setup(Setup::Composing, &event),
                    "sym {sym:#06x} state {state:#x} must reach the composition"
                );
            }
        }
    }
}

#[test]
fn test_dispatch_never_claims_a_key_release() {
    for setup in Setup::ALL {
        for sym in corpus() {
            for state in STATES {
                assert_eq!(
                    dispatch_in_setup(setup, &release(sym, state)),
                    Consumed::Ignored,
                    "setup {setup:?} sym {sym:#06x} state {state:#x}"
                );
            }
        }
    }
}

#[test]
fn test_dispatch_never_claims_a_modifier_press() {
    for setup in Setup::ALL {
        for sym in [KEY_SHIFT_L, KEY_SHIFT_R] {
            for state in STATES {
                assert_eq!(
                    dispatch_in_setup(setup, &press(sym, state)),
                    Consumed::Ignored,
                    "setup {setup:?} sym {sym:#06x} state {state:#x}"
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
    let mut claimed = 0usize;
    let mut handed_back = 0usize;
    let mut chained = 0usize;
    for setup in Setup::ALL {
        for sym in &keys {
            for state in STATES {
                match dispatch_in_setup(setup, &press(*sym, state)) {
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
