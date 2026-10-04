//! What the engine does with the effects a step returns, and how a session's life ends.
//!
//! The effects themselves belong to `ime-core`; these tests are about the host side of
//! them: text reaching the application, the preedit area following the configuration, the
//! user's frequencies going through the privacy gate, and the three ways a session ends --
//! a deactivation, a reset, and a configuration reload that must not disturb a composition
//! in progress.

use ime_core::state::{SessionConfig, SessionState};
use ime_types::{DismissReason, HideReason, SelectTrigger, UiEvent};

use crate::engine::*;

use super::{
    Fixture, IC, IC2, KEY_I, KEY_N, RecordingHost, activate_ordinary, ordinary_report, press,
    type_n,
};

#[test]
fn test_key_event_commit_records_the_word_in_an_ordinary_context() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let frame = host.last_frame().expect("a frame");
    let highlighted = frame.candidates[0].text.clone();
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));

    assert_eq!(host.commits(), std::slice::from_ref(&highlighted));
    let recorded = fixture.user.recorded();
    assert_eq!(recorded.len(), 1, "one commit, one record");
    assert_eq!(recorded[0].0, highlighted);
    assert!(
        recorded[0].1 >= 1,
        "the hint counts the syllables the word spans"
    );
    assert_eq!(
        host.preedits().last(),
        Some(&None),
        "the preedit area is emptied once the commit landed"
    );
}

#[test]
fn test_key_event_commit_in_a_suppressed_context_records_nothing() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    // `activate` observes the context as unreported, which every policy treats as
    // sensitive: the host's capability flags do not reach Rust through the C ABI yet, and
    // an unreported context fails closed.
    router.activate(IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let frame = host.last_frame().expect("a frame");
    let highlighted = frame.candidates[0].text.clone();
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));

    assert_eq!(
        host.commits(),
        [highlighted],
        "the text still reaches the application: suppression is about learning, not input"
    );
    assert!(
        fixture.user.recorded().is_empty(),
        "a context that must not be learned from records nothing"
    );
}

#[test]
fn test_activate_observes_an_unreported_context_as_sensitive() {
    let fixture = Fixture::new();
    let mut router = fixture.router();

    let unreported = router.activate(IC);
    assert!(!unreported.is_reported);
    assert!(unreported.forbids_learning());

    let reported = router.activate_reported(IC2, ordinary_report());
    assert!(reported.is_reported);
    assert!(!reported.forbids_learning());
}

#[test]
fn test_ui_event_select_commits_the_clicked_candidate() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let frame = host.last_frame().expect("a frame").clone();
    assert!(frame.candidates.len() >= 2, "the fixture offers a choice");

    let mut host = RecordingHost::default();
    let click = UiEvent::Select {
        revision: frame.revision,
        index: 1,
        trigger: SelectTrigger::Mouse,
    };
    assert!(router.ui_event(IC, click, &mut host));
    assert_eq!(host.commits(), [frame.candidates[1].text.clone()]);
}

#[test]
fn test_ui_event_stale_revision_is_dropped_and_diagnosed() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let revision = host.last_frame().expect("a frame").revision;

    let mut host = RecordingHost::default();
    let stale = revision + 1;
    let click = UiEvent::Select {
        revision: stale,
        index: 0,
        trigger: SelectTrigger::Mouse,
    };
    assert!(
        !router.ui_event(IC, click, &mut host),
        "a click on a frame the session has replaced is dropped"
    );
    assert!(host.commits().is_empty());
    let expected = format!("ui/stale-select: revision={stale} current={revision}");
    assert_eq!(host.diagnostics(), [expected]);
}

#[test]
fn test_ui_event_dismiss_cancels_the_composition() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let revision = host.last_frame().expect("a frame").revision;

    let mut host = RecordingHost::default();
    let dismiss = UiEvent::Dismiss {
        revision,
        reason: DismissReason::OutsideClick,
    };
    assert!(router.ui_event(IC, dismiss, &mut host));
    assert!(host.commits().is_empty(), "a dismissal commits nothing");
    assert_eq!(host.hides().last(), Some(&HideReason::Cancelled));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_deactivate_ends_the_composition_and_forgets_the_context() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    let mut host = RecordingHost::default();
    router.deactivate(IC, &mut host);
    assert!(host.commits().is_empty(), "switching away commits nothing");
    assert_eq!(host.hides().last(), Some(&HideReason::FocusLost));
    assert!(router.session(IC).is_none(), "the context is gone");
    assert!(
        !router.key_event(IC, &press(KEY_N, 0), &mut host),
        "and its keys reach the application again"
    );
}

#[test]
fn test_deactivate_of_a_context_that_was_never_activated_does_nothing() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    let mut host = RecordingHost::default();

    router.deactivate(IC, &mut host);
    assert!(!host.acted());
    assert!(router.session(IC).is_none());
}

#[test]
fn test_reset_ends_the_composition_but_keeps_the_context() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    let mut host = RecordingHost::default();
    router.reset(IC, &mut host);
    assert_eq!(host.hides().last(), Some(&HideReason::FocusLost));
    let session = router.session(IC).expect("the context stays active");
    assert_eq!(session.state, SessionState::Idle);
    assert!(
        session.buf.raw().is_empty(),
        "the input went with the composition"
    );
}

#[test]
fn test_reload_keeps_the_composition_and_repaints() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    let mut host = RecordingHost::default();
    let next = RoutingConfig {
        session: SessionConfig::new(64, 9, true, 720),
        ..RoutingConfig::default()
    };
    router.reload(next, &mut host);

    assert_eq!(
        host.ui_kinds(),
        ["frame"],
        "the window repaints under the new page size"
    );
    assert_eq!(host.last_frame().expect("a frame").layout.max_per_row, 9);
    let session = router.session(IC).expect("the context is active");
    assert_eq!(
        session.state,
        SessionState::Composing,
        "a reload never resets a composition in progress"
    );
    assert_eq!(session.buf.raw(), "ni", "and never disturbs the input");
    assert!(host.commits().is_empty());
}

#[test]
fn test_reload_with_unchanged_values_produces_nothing() {
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);

    let mut host = RecordingHost::default();
    router.reload(RoutingConfig::default(), &mut host);
    assert!(!host.acted(), "reloading an unchanged file is idempotent");
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some("ni")
    );
}

#[test]
fn test_key_event_client_preedit_policy_follows_the_configuration() {
    let fixture = Fixture::new();

    // Off, which is the shipped default: the application's area is emptied and the
    // candidate window's header is what shows the pinyin.
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    assert_eq!(host.preedits(), [None]);

    // On: the composing text goes into the application's own preedit area.
    let config = RoutingConfig {
        client_preedit: true,
        ..RoutingConfig::default()
    };
    let mut router = fixture.router_with(config);
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    let preedits = host.preedits();
    assert_eq!(preedits.len(), 1, "one composition, one preedit write");
    let (text, caret) = preedits[0].clone().expect("the composing text was written");
    assert_eq!(text, "n");
    assert!(caret <= text.len() as u32, "the caret is inside the text");
}

#[test]
fn test_key_event_full_width_rewrites_the_raw_commit_the_host_receives() {
    // The full-width switch's one reader is the commit door: a raw commit is the
    // plugin's own ASCII output, so it is the commit the switch rewrites, character
    // for character, before the application sees any of it.
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

    assert!(
        router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host),
        "the full-width chord is the plugin's"
    );
    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    // `enter_commit_raw` turns Return into the raw commit, so what reaches the
    // application is the input the user typed, widened.
    assert!(router.key_event(IC, &press(KEY_RETURN, 0), &mut host));

    assert_eq!(
        host.commits(),
        ["ｎｉ"],
        "the raw input commits full width, every typed ASCII character widened"
    );
}

#[test]
fn test_key_event_commit_under_the_default_modes_reaches_the_host_unchanged() {
    // The shipped switches are half width with Chinese punctuation — the defaults —
    // and the rewrite exists to serve them, never to alter a commit they have no
    // opinion on: a candidate the modes cannot change is handed over byte for byte.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let frame = host.last_frame().expect("a frame");
    let highlighted = frame.candidates[0].text.clone();
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));

    assert_eq!(host.commits(), [highlighted.as_str()]);
}

#[test]
fn test_key_event_full_width_learn_key_is_the_word_the_user_chose() {
    // The switch rewrites the text the application receives; it must not rewrite the
    // word the dictionary learns. The two travel side by side through one commit, and
    // a widened spelling in the user's frequencies would teach the decoder a word
    // nothing can type.
    let fixture = Fixture::new();
    let mut router = fixture.router();
    activate_ordinary(&mut router, IC);
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host));
    type_n(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    let frame = host.last_frame().expect("a frame");
    let highlighted = frame.candidates[0].text.clone();
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));

    assert_eq!(host.commits(), [highlighted.as_str()]);
    let recorded = fixture.user.recorded();
    assert_eq!(recorded.len(), 1, "one commit, one record");
    assert_eq!(
        recorded[0].0, highlighted,
        "the record carries the word the user chose, not a transformed one"
    );
}
