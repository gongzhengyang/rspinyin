//! The compose fallback's behaviour as the router delivers it.
//!
//! The host boundary owns dead-key composing when it can: the router asks it first and
//! only falls back to the built-in table when the host answers unsupported. These tests
//! pin that dispatch as a matrix. On the host-available side a scripted host answers
//! `Committed` and `Swallowed`, and the assertions check that the fallback neither
//! commits nor holds state behind the host's answers. On the host-missing side the
//! recording host answers the trait's default, unsupported, and the assertions walk the
//! built-in table's hit, miss and mark-replacement outcomes through the whole routing
//! path, including the per-context isolation of the pending state.
//!
//! Everything is in memory: no dictionary file, no clock, no display server.

use std::collections::VecDeque;

use ime_core::state::SessionState;
use ime_types::{ImeError, UiCommand};

use crate::engine::host::{ComposeOutcome, Host};
use crate::ffi::FcitxKeyEvent;

use super::{Fixture, IC, IC2, RecordingHost, RoutingConfig, activate_ordinary, press};

/// `FcitxKey_dead_acute`, the acute dead mark of a compose-capable layout.
const DEAD_ACUTE: u32 = 0xb4;

/// `FcitxKey_dead_grave`, the grave dead mark.
const DEAD_GRAVE: u32 = 0x60;

/// `FcitxKey_dead_diaeresis`, the diaeresis dead mark.
const DEAD_DIAERESIS: u32 = 0xa8;

/// A host whose compose boundary answers from a script, for the host-available side of
/// the matrix.
///
/// Every other boundary action is recorded as a commit or dropped: the compose step's
/// observable effects are the commits it makes and the keys it keeps, and a scripted
/// answer that claims an action must show up as exactly that.
struct ScriptedComposeHost {
    /// The answers the host gives, in the order the keys ask for them; an exhausted
    /// script answers `Unsupported`, which is what a host without compose support
    /// answers for every key.
    answers: VecDeque<ComposeOutcome>,
    /// The text of every commit, in order.
    commits: Vec<String>,
}

impl ScriptedComposeHost {
    /// A host that answers each queued outcome once and `Unsupported` afterwards.
    fn scripted(answers: &[ComposeOutcome]) -> Self {
        Self {
            answers: answers.iter().copied().collect(),
            commits: Vec::new(),
        }
    }
}

impl Host for ScriptedComposeHost {
    fn process_compose(&mut self, _ic: u64, _key: &FcitxKeyEvent) -> ComposeOutcome {
        self.answers
            .pop_front()
            .unwrap_or(ComposeOutcome::Unsupported)
    }

    fn commit(&mut self, _ic: u64, text: &str) {
        self.commits.push(String::from(text));
    }

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

    fn clear_preedit(&mut self, _ic: u64) {}

    fn post_ui(&mut self, _ic: u64, _command: UiCommand) {}

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        true
    }

    fn diagnose(&mut self, _ic: u64, _err: &ImeError) {}
}

/// The router every compose test drives: the step switched on, one ordinary context.
fn compose_router(fixture: &Fixture) -> crate::engine::router::KeyRouter<'_> {
    let config = RoutingConfig {
        compose: true,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    router
}

#[test]
fn test_key_event_host_committed_answer_commits_and_leaves_no_fallback_state() {
    let fixture = Fixture::new();
    let mut router = compose_router(&fixture);
    let mut host = ScriptedComposeHost::scripted(&[ComposeOutcome::Committed('å')]);

    assert!(
        router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host),
        "the host's commit keeps the key from the application"
    );
    assert_eq!(
        host.commits,
        ["å"],
        "the host's answer is what reaches the app"
    );
    // The fallback must hold nothing behind the host's answer: the next letter asks
    // the host again, is left unsupported, and routes as an ordinary composition with
    // no compose commit of its own.
    assert!(router.key_event(IC, &press(b'e' as u32, 0), &mut host));
    assert_eq!(
        host.commits,
        ["å"],
        "a dead key the host answered leaves no pending mark behind"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Composing),
        "the letter routes into its own composition"
    );
}

#[test]
fn test_key_event_host_swallowed_answer_hides_the_key_and_resets_the_fallback() {
    let fixture = Fixture::new();
    let mut router = compose_router(&fixture);
    let mut host = ScriptedComposeHost::scripted(&[ComposeOutcome::Swallowed]);

    assert!(
        router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host),
        "a key the host is composing is the host's, so the plugin keeps it"
    );
    assert!(
        host.commits.is_empty(),
        "a swallowed sequence shows the application nothing"
    );
    // The reset matters: had the fallback kept the acute pending, the next letter
    // would compose against it instead of routing on its own.
    assert!(router.key_event(IC, &press(b'e' as u32, 0), &mut host));
    assert!(host.commits.is_empty());
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("e"),
        "the letter after a host-swallowed key starts its own composition"
    );
}

#[test]
fn test_key_event_host_missing_hit_commits_without_opening_the_window() {
    let fixture = Fixture::new();
    let mut router = compose_router(&fixture);
    // The recording host never overrides the compose boundary, so every key is
    // answered unsupported and the built-in table is the only composer.
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host));
    assert!(router.key_event(IC, &press(b'e' as u32, 0), &mut host));
    assert_eq!(
        host.commits(),
        ["é"],
        "the opening mark stays invisible; the composed letter is the only output"
    );
    assert!(
        host.ui_kinds().is_empty(),
        "a compose commit passes straight through: no candidate window joins it"
    );
}

#[test]
fn test_key_event_host_missing_miss_letter_still_reaches_its_own_route() {
    let fixture = Fixture::new();
    let mut router = compose_router(&fixture);
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(DEAD_DIAERESIS, 0), &mut host));
    assert!(router.key_event(IC, &press(b'x' as u32, 0), &mut host));
    assert_eq!(
        host.commits(),
        ["¨"],
        "a failed sequence leaves exactly one mark behind, and the letter keeps its route"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Composing),
        "the unmatched letter keeps its own route into the composition"
    );
}

#[test]
fn test_key_event_host_missing_second_dead_key_commits_the_previous_mark() {
    let fixture = Fixture::new();
    let mut router = compose_router(&fixture);
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host));
    assert!(router.key_event(IC, &press(DEAD_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(b'a' as u32, 0), &mut host));
    assert_eq!(
        host.commits(),
        ["´", "à"],
        "the replaced mark is committed once when it hands over, and the new mark composes"
    );
}

#[test]
fn test_key_event_fallback_state_is_per_input_context() {
    let fixture = Fixture::new();
    let config = RoutingConfig {
        compose: true,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    activate_ordinary(&mut router, IC2);
    let mut host = RecordingHost::default();

    // Each context opens its own sequence, with a different mark on purpose: the
    // keys interleave, and neither context's letter may resolve against the
    // other's pending mark.
    assert!(router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host));
    assert!(router.key_event(IC2, &press(DEAD_GRAVE, 0), &mut host));
    assert!(router.key_event(IC2, &press(b'e' as u32, 0), &mut host));
    assert!(router.key_event(IC, &press(b'e' as u32, 0), &mut host));
    assert_eq!(
        host.commits(),
        ["è", "é"],
        "the second context composed its own grave, and the first's acute was still pending"
    );
}

#[test]
fn test_key_event_switch_off_hands_dead_keys_to_the_application() {
    let fixture = Fixture::new();
    // The shipped configuration: the compose step is off, so no key is ever asked and
    // the built-in table never opens a sequence.
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        !router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host),
        "with the step off the dead key is nobody's, and travels to the application"
    );
    assert!(host.commits().is_empty(), "the plugin committed nothing");
    // A later letter composes on its own, which is the plain pre-switch behaviour.
    assert!(router.key_event(IC, &press(b'e' as u32, 0), &mut host));
    assert!(
        host.commits().is_empty(),
        "the switch off adds no compose commit"
    );
}

#[test]
fn test_key_event_unactivated_context_never_opens_a_fallback_sequence() {
    let fixture = Fixture::new();
    let config = RoutingConfig {
        compose: true,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    // No activate_reported call: the context has no session yet, so the step has
    // nowhere to hold a pending mark even if it ran.
    let mut host = RecordingHost::default();

    assert!(
        !router.key_event(IC, &press(DEAD_ACUTE, 0), &mut host),
        "a context the plugin has not reported keeps every key with the application"
    );
    assert!(host.commits().is_empty());
}
