//! What one key does to a session, and whether the plugin keeps it.
//!
//! The routing table's rows are pinned next door; these are the rows that reach a
//! session, plus the boundary cases the shortcut table cares about: a key the table does
//! not name, a key that arrives with nothing composing, a key that arrives while a
//! composition is live, and a key release.

use ime_core::state::{SessionConfig, SessionState};
use ime_types::{Anchor, HideReason, KeyAction, Placement, RectI, ScreenId, SpanKind};

use crate::engine::*;

use super::{
    Fixture, IC, IC2, KEY_I, KEY_N, RecordingHost, activate_ordinary, keysym_stream, press,
    release, type_n,
};

/// `FcitxKey_h`.
const KEY_H: u32 = 0x0068;

/// `FcitxKey_o`.
const KEY_O: u32 = 0x006f;

#[test]
fn test_key_event_letter_while_idle_starts_a_composition_and_claims_the_key() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "the key was acted on, so it stops here"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Composing)
    );
    // The show precedes the frame: the window drops a frame that arrives while it is
    // hidden.
    assert_eq!(host.ui_kinds(), ["show", "frame"]);
    let frame = host.last_frame().expect("a frame reached the window");
    assert!(
        !frame.candidates.is_empty(),
        "a composition always has something to show"
    );
    assert!(host.commits().is_empty(), "nothing is committed yet");
}

#[test]
fn test_key_event_letter_while_composing_extends_the_input() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    assert!(router.key_event(IC, &press(KEY_I, 0), &mut host));
    let session = router.session(IC).expect("the context is active");
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.buf.raw(), "ni", "both letters are in the input");
    assert_eq!(host.ui_kinds(), ["show", "frame", "frame"]);
}

#[test]
fn test_key_event_digit_while_idle_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    for digit in KEY_1..=KEY_9 {
        assert!(
            !router.key_event(IC, &press(digit, 0), &mut host),
            "a digit with nothing to select belongs to the application"
        );
    }
    assert!(
        !host.acted(),
        "a key that is handed back must not have changed anything"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_named_keys_while_idle_are_handed_back() {
    // Every row of the table that needs a composition behind it, pressed with nothing
    // composing. The letters are the only rows an idle session acts on, and they are
    // covered above.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    let bare_presses = [
        KEY_SPACE,
        KEY_APOSTROPHE,
        KEY_0,
        KEY_MINUS,
        KEY_EQUAL,
        KEY_TAB,
        KEY_LEFT,
        KEY_RIGHT,
        KEY_UP,
        KEY_DOWN,
        KEY_RETURN,
        KEY_ESCAPE,
        KEY_BACKSPACE,
    ];
    for sym in bare_presses {
        assert!(
            !router.key_event(IC, &press(sym, 0), &mut host),
            "sym {sym:#06x} has nothing to act on while idle"
        );
    }
    assert!(!host.acted());
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_apostrophe_while_idle_is_handed_back() {
    // The separator is the one key of the composing keymap the table names in every context
    // and the plugin still must not take: the input alphabet accepts it, so a session with
    // nothing composing would read it as the first character of a new composition and open a
    // candidate window over a character the user typed for the application.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        !router.key_event(IC, &press(KEY_APOSTROPHE, 0), &mut host),
        "an apostrophe with nothing composing belongs to the application"
    );
    assert!(!host.acted(), "the window must not appear");
    assert!(
        host.ui_kinds().is_empty(),
        "nothing reached the candidate window"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "no composition was started"
    );
}

#[test]
fn test_key_event_apostrophe_while_composing_extends_the_input() {
    // Inside a composition the same key pins a syllable boundary, which is the only way to
    // spell an input the segmenter would otherwise cut somewhere else.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    for sym in [KEY_N, KEY_I] {
        assert!(
            router.key_event(IC, &press(sym, 0), &mut host),
            "sym {sym:#06x} is the plugin's inside a composition"
        );
    }
    assert!(
        router.key_event(IC, &press(KEY_APOSTROPHE, 0), &mut host),
        "the separator is the plugin's inside a composition"
    );
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("ni'"),
        "the separator reaches the tail of the input"
    );

    for sym in [KEY_H, KEY_A, KEY_O] {
        assert!(router.key_event(IC, &press(sym, 0), &mut host));
    }
    let session = router.session(IC).expect("the context is active");
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.buf.raw(), "ni'hao", "the whole input is kept");

    // The window draws the boundary the user spelled out rather than the one the segmenter
    // would have chosen, which is what makes the character visible in the header.
    let frame = host.last_frame().expect("the window has a frame");
    assert_eq!(frame.preedit.text, "ni'hao");
    assert!(
        frame
            .preedit
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::Separator),
        "the boundary is segmented as a separator"
    );
    // And the boundary reached the decode: the two-syllable reading the fixture declares for
    // this input is on the list, which it could not be for an input the segmenter had cut
    // anywhere else. The whole list rather than the page on show, so the assertion does not
    // depend on where the ranking put the word.
    assert!(
        session
            .decoded()
            .candidates
            .iter()
            .any(|held| held.text == "你好"),
        "the pinned boundary reaches the decode"
    );
}

#[test]
fn test_key_event_unrouted_key_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    // F5, the keypad's Enter, the slash — a mark the policy row does not name — and a
    // bare modifier that is not Shift.
    for sym in [0xffbe, 0xff8d, 0x002f, 0xffe3] {
        assert!(
            !router.key_event(IC, &press(sym, 0), &mut host),
            "sym {sym:#06x} is not the plugin's"
        );
    }
    assert!(!host.acted());
}

#[test]
fn test_key_event_key_release_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    for (sym, state) in [(KEY_N, 0), (KEY_SPACE, 0), (KEY_RETURN, 0)] {
        assert!(
            !router.key_event(IC, &release(sym, state), &mut host),
            "the application's key-up is never ours"
        );
    }
    assert!(!host.acted());
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "a release does not start a composition"
    );
}

#[test]
fn test_key_event_unknown_context_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    let mut host = RecordingHost::default();

    // No context was activated, so there is no session to act on the key. The
    // `ffi/stale-ic` diagnostic goes to the crash channel, which is stderr, and is not
    // something the host boundary sees.
    assert!(!router.key_event(IC, &press(KEY_N, 0), &mut host));
    assert!(!host.acted());
}

#[test]
fn test_key_event_space_commits_the_highlighted_candidate() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let highlighted = host
        .last_frame()
        .expect("the window has a frame")
        .candidates[0]
        .text
        .clone();

    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits(), [highlighted]);
    assert_eq!(host.hides().last(), Some(&HideReason::Committed));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the session is ready for the next word"
    );
}

#[test]
fn test_key_event_digit_while_composing_selects_the_candidate_it_names() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let candidates = host.last_frame().expect("a frame").candidates.clone();
    assert!(candidates.len() >= 2, "the fixture offers a second choice");

    // `2` names the second candidate on the page.
    assert!(router.key_event(IC, &press(KEY_0 + 2, 0), &mut host));
    assert_eq!(host.commits(), [candidates[1].text.clone()]);
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_digit_past_the_last_candidate_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let count = host.last_frame().expect("a frame").candidates.len();
    assert!(count < 9, "the fixture must not fill a whole page");

    let mut host = RecordingHost::default();
    assert!(
        !router.key_event(IC, &press(KEY_0 + 9, 0), &mut host),
        "the digit names nothing on this page"
    );
    assert!(host.commits().is_empty());
    assert!(!host.acted());
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Composing),
        "the composition survives a key that was handed back"
    );
}

#[test]
fn test_key_event_escape_cancels_the_composition_without_committing() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert!(host.commits().is_empty(), "a cancellation commits nothing");
    assert_eq!(host.hides().last(), Some(&HideReason::Cancelled));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the cancellation is closed, not left waiting"
    );
}

#[test]
fn test_key_event_backspace_that_empties_the_input_ends_the_composition() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(host.hides().last(), Some(&HideReason::EmptyInput));
    assert!(host.commits().is_empty());
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_enter_commits_the_raw_input_when_the_configuration_asks() {
    let fixture = Fixture::new();
    let config = RoutingConfig {
        keys: KeyBindings {
            enter_commit_raw: true,
            ..KeyBindings::default()
        },
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    assert!(router.key_event(IC, &press(KEY_RETURN, 0), &mut host));
    assert_eq!(host.commits(), [String::from("ni")]);
}

#[test]
fn test_key_event_input_past_the_limit_is_diagnosed_and_handed_back() {
    let fixture = Fixture::new();
    let config = RoutingConfig {
        session: SessionConfig::new(1, 5, true, 720),
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    let mut host = RecordingHost::default();

    // The input is one byte long and the limit is one byte, so the second letter cannot
    // be taken: the key reaches the application and the host is told why.
    assert!(!router.key_event(IC, &press(KEY_I, 0), &mut host));
    assert_eq!(host.diagnostics(), ["decode/too-long: len=2 max=1"]);
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("n"),
        "the input the limit allowed is kept"
    );
}

#[test]
fn test_key_event_in_temporary_english_hands_every_key_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    // The chord is the plugin's: it enters the mode.
    assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));
    assert!(
        router.session(IC).expect("the context").temp_english,
        "the mode is on"
    );

    // Every key now reaches the application, letters included.
    for sym in [KEY_N, KEY_I, KEY_SPACE] {
        assert!(
            !router.key_event(IC, &press(sym, 0), &mut host),
            "temporary English passes every key through"
        );
    }
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "nothing was composed while the mode was on"
    );

    // Escape leaves the mode, and the key that left it is handed back too.
    assert!(!router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert!(!router.session(IC).expect("the context").temp_english);
    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "the plugin is back in Chinese"
    );
}

#[test]
fn test_key_event_full_width_and_punctuation_toggle_the_status_strip() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);

    // Shift+Space switches full width, and the window repaints with it.
    assert!(router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host));
    let frame = host.last_frame().expect("a frame");
    assert!(frame.status.full_width, "the switch reached the frame");
    assert_eq!(
        frame.status.mode_label, "全拼",
        "the header names the layout the shipped configuration declares"
    );

    // Ctrl+. switches the punctuation, the other way round from the shipped default.
    assert!(router.key_event(IC, &press(KEY_PERIOD, CTRL), &mut host));
    let frame = host.last_frame().expect("a frame");
    assert!(!frame.status.punctuation_full);
    assert!(
        frame.status.full_width,
        "one switch does not undo the other"
    );
}

#[test]
fn test_key_event_idle_full_width_toggle_is_claimed_without_reaching_the_host() {
    // With nothing composing there is no window to repaint: the switch claims its key —
    // the user asked for a change of mode, and the next composition will show it — but
    // the only thing it produces is the diagnostic flash, which is no host call at all.
    // Keeping the claim without pretending work happened is the honest shape of a mode
    // switch in the idle state.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host),
        "the switch is the plugin's key whatever is composing"
    );
    assert!(
        !host.acted(),
        "the idle switch reaches the diagnostic channel, never the host"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "and the session was not stepped into a composition by it"
    );
}

#[test]
fn test_key_event_ctrl_space_is_handed_back_to_the_host() {
    // The language switch is the host's own hotkey, and the plugin can neither read nor
    // write the state behind it — so the routing table claims no chord for it. The key
    // reaches the application unclaimed, nothing is toggled, and the plugin's own mode
    // is exactly where it was.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    // A second keystroke, so the frame on record is the mode's own label rather than the
    // first-run hint that borrows the slot on a process's first frame.
    assert!(router.key_event(IC, &press(KEY_I, 0), &mut host));

    // A recorder of its own, so "nothing reached the host" is about this key and not
    // about the composition typed before it.
    let mut chord_host = RecordingHost::default();
    assert!(
        !router.key_event(IC, &press(KEY_SPACE, CTRL), &mut chord_host),
        "the language chord travels on to the host"
    );
    assert!(
        chord_host.toggles().is_empty(),
        "the plugin never touches the host's input-method state: {:?}",
        chord_host.toggles()
    );
    assert!(
        !chord_host.acted(),
        "a chord the plugin did not act on is nobody's to keep"
    );
    let frame = host.last_frame().expect("the composing frame is on record");
    assert_eq!(
        frame.status.mode_label, "全拼",
        "the mode the user sees is unchanged by a key the plugin declined"
    );
    assert_eq!(frame.preedit.text, "ni", "and the composition survives it");
    assert!(
        router.key_event(IC, &press(KEY_H, 0), &mut host),
        "the next letter is still the plugin's"
    );
}

#[test]
fn test_key_event_shift_press_leaves_the_temporary_switch_to_the_host() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    // The table names a Shift press, but the press itself is the application's: an input
    // method that kept it would take the first half of every capital letter.
    for sym in [KEY_SHIFT_L, KEY_SHIFT_R] {
        assert!(!router.key_event(IC, &press(sym, SHIFT), &mut host));
    }
    assert!(host.toggles().is_empty(), "a modifier is not a mode switch");
    assert!(!host.acted());
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_two_contexts_keep_separate_sessions() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    activate_ordinary(&mut router, IC2);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    type_n(&mut router, IC2, &mut host);

    let first = router.session(IC).expect("the first context");
    let second = router.session(IC2).expect("the second context");
    assert_eq!(first.buf.raw(), "n");
    assert_eq!(second.buf.raw(), "n");
    assert_ne!(
        first.id, second.id,
        "each context counts its sessions in a range of its own"
    );
}

#[test]
fn test_set_anchor_places_the_window_where_the_host_reported() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    let anchor = Anchor {
        cursor: RectI {
            x: 100,
            y: 200,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(1),
        scale: 1.5,
        placement: Placement::Auto,
    };
    router.set_anchor(IC, anchor);

    type_n(&mut router, IC, &mut host);
    assert_eq!(
        host.last_show(),
        Some(anchor),
        "the show carries the anchor"
    );
    assert_eq!(
        host.last_frame().expect("a frame").anchor,
        anchor,
        "and so does the frame"
    );
}

#[test]
fn test_key_event_never_claims_a_key_that_did_nothing() {
    // The "never swallow" rule, over the 200 keysyms the shortcut table asks for: an
    // answer of `true` means the key stopped here, so something other than a diagnostic
    // must have reached the host. The mode chords are left to their own rows above --
    // they are claimed for a mode switch rather than for an effect.
    let states = [0, SHIFT, CTRL, CTRL | SHIFT | ALT];
    let mut checked = 0;
    for sym in keysym_stream(200) {
        for state in states {
            let key = press(sym, state);
            let action = translate_key(&key, &KeyBindings::default());
            if is_mode_chord(action) {
                continue;
            }
            let fixture = Fixture::new();
            let mut router = fixture.router();
            activate_ordinary(&mut router, IC);
            let mut host = RecordingHost::default();

            let claimed = router.key_event(IC, &key, &mut host);
            assert!(
                !claimed || host.acted(),
                "sym {sym:#010x} state {state:#x} was claimed as {action:?} but did nothing"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 800, "the sweep must have run");
}

#[test]
fn test_key_event_never_claims_a_key_that_did_nothing_while_composing() {
    // The same rule with a composition behind the key, which is the state where most of
    // the table's rows can act and where the ones that cannot must still be handed back.
    let states = [0, SHIFT, CTRL, CTRL | SHIFT | ALT];
    let mut checked = 0;
    for sym in keysym_stream(200) {
        for state in states {
            let key = press(sym, state);
            let action = translate_key(&key, &KeyBindings::default());
            if is_mode_chord(action) {
                continue;
            }
            let fixture = Fixture::new();
            let mut router = fixture.router();
            activate_ordinary(&mut router, IC);
            let mut setup = RecordingHost::default();
            router.key_event(IC, &press(KEY_N, 0), &mut setup);
            let mut host = RecordingHost::default();

            let claimed = router.key_event(IC, &key, &mut host);
            assert!(
                !claimed || host.acted(),
                "sym {sym:#010x} state {state:#x} was claimed as {action:?} but did nothing"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 800, "the sweep must have run");
}

/// Whether `action` is one of the mode switches the engine owns.
///
/// Those are claimed for a change to a mode bit rather than for an effect, so the
/// sweeps above leave them to the rows that assert what they do.
fn is_mode_chord(action: KeyAction) -> bool {
    matches!(action, KeyAction::ToggleLang | KeyAction::ToggleFullWidth)
        || matches!(action, KeyAction::TogglePunct | KeyAction::EnterTempEnglish)
}
