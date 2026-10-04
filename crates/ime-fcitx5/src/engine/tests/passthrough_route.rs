//! The passthrough policy's answers on the idle key path.
//!
//! A text-producing key pressed with nothing composing is the policy's to answer before
//! the session sees it: an uppercase letter is typed English, a URL keystroke belongs to
//! the application, and a punctuation mark is committed as the mode bits spell it. These
//! tests pin each decision at the routing boundary, where the observable facts are what
//! reached the host and whether the plugin kept the key. The composing path is
//! deliberately absent: while a composition is live the session answers every key, so
//! these keys behave exactly as they did before the policy was wired.

use ime_core::state::SessionState;

use crate::engine::{CTRL, KEY_PERIOD, KEY_SPACE, SHIFT};

use super::{Fixture, IC, RecordingHost, RoutingConfig, activate_ordinary, press};

/// `FcitxKey_N`, the shifted letter's own keysym, which the routing table folds before
/// its rows read it.
const KEY_N_UPPER: u32 = 0x004e;

/// `FcitxKey_comma`.
const KEY_COMMA: u32 = 0x002c;

/// `FcitxKey_at`, the one URL marker a single keystroke can carry.
const KEY_AT: u32 = 0x0040;

#[test]
fn test_key_event_idle_uppercase_is_committed_as_english() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        router.key_event(IC, &press(KEY_N_UPPER, SHIFT), &mut host),
        "the policy committed the letter, so the key stops here"
    );
    assert_eq!(host.commits(), ["N"], "the letter reaches the application");
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "typed English opens no composition"
    );
    assert!(
        host.ui_kinds().is_empty(),
        "no candidate window opens for typed English"
    );
}

#[test]
fn test_key_event_idle_uppercase_widens_under_full_width() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();
    // The full-width chord, so the commit the policy produces comes out widened like
    // every other commit the switches rewrite.
    router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host);

    let mut host = RecordingHost::default();
    assert!(router.key_event(IC, &press(KEY_N_UPPER, SHIFT), &mut host));
    assert_eq!(
        host.commits(),
        ["Ｎ"],
        "the full-width switch is a switch, not an icon"
    );
}

#[test]
fn test_key_event_auto_english_off_lets_the_session_answer_the_uppercase() {
    // The switch off, the policy has no answer for the capital and the key falls
    // through to the session — which reads the case the table folded away, so what
    // starts composing is the lowercase spelling. The configuration declines the
    // commitment, not the keystroke.
    let fixture = Fixture::new();
    let config = RoutingConfig {
        auto_english_on_uppercase: false,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        router.key_event(IC, &press(KEY_N_UPPER, SHIFT), &mut host),
        "the session took the key and opened a composition"
    );
    assert!(
        host.commits().is_empty(),
        "the policy never committed what the configuration declined"
    );
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("n"),
        "the composing input carries the folded letter"
    );
}

#[test]
fn test_key_event_idle_comma_commits_the_chinese_mark() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_COMMA, 0), &mut host));
    assert_eq!(
        host.commits(),
        ["，"],
        "the punctuation switch's output half answers the mark"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_key_event_idle_comma_in_english_punct_mode_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();
    // The punctuation chord, which flips the mode the mark is answered in.
    router.key_event(IC, &press(KEY_PERIOD, CTRL), &mut host);

    let mut host = RecordingHost::default();
    assert!(
        !router.key_event(IC, &press(KEY_COMMA, 0), &mut host),
        "half-width punctuation belongs to the application"
    );
    assert!(host.commits().is_empty());
    assert!(!host.acted(), "a handed-back key changed nothing");
}

#[test]
fn test_key_event_idle_url_marker_is_handed_back() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        !router.key_event(IC, &press(KEY_AT, 0), &mut host),
        "the keystroke a URL context owns travels on to the application"
    );
    assert!(host.commits().is_empty());
    assert!(!host.acted());
}

#[test]
fn test_key_event_url_passthrough_off_feeds_the_session() {
    let fixture = Fixture::new();
    let config = RoutingConfig {
        passthrough_url: false,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(
        !router.key_event(IC, &press(KEY_AT, 0), &mut host),
        "the key travels on: a swallowed character is a report, not a claim"
    );
    assert!(host.commits().is_empty());
    assert!(
        host.diagnostics()
            .iter()
            .any(|line| line.contains("invalid-char")),
        "the decoder's refusal is still reported, not silent"
    );
}
