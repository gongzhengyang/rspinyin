//! The quick-input panel, driven end to end through the router.
//!
//! Seven domains, one fixture: a date-family trigger, the calculator, the amount
//! trigger and the codepoint trigger each open a panel whose candidates are pinned
//! against one injected clock snapshot, a selection commits through the ordinary
//! door, and the ways out — backspace, escape, a reload — hand control back to the
//! session without swallowing a keystroke or committing a surprise. Everything is in
//! memory; the clock is installed once with the same snapshot every test reads, so
//! installation order cannot change an assertion.

use ime_core::state::SessionState;
use ime_types::{CandidateSource, SelectTrigger, UiEvent};

use crate::engine::host::{ClockSnapshot, ClockSource};
use crate::engine::router::effects::quick::install_clock;

use super::*;

/// Whether the newest frame on the host is a quick-panel frame.
///
/// The fixture loads no symbol table, so the panel's `Symbol`-source candidates are
/// the only candidates of that source any frame can carry — which makes the source
/// the panel's signature, and this helper the honest answer to "is the panel up"
/// that does not depend on what the session's own decode offered beside it.
fn panel_open(host: &RecordingHost) -> bool {
    host.last_frame
        .as_ref()
        .is_some_and(|frame| {
            frame
                .candidates
                .iter()
                .any(|candidate| candidate.source == CandidateSource::Symbol)
        })
}

/// `FcitxKey_r`.
const KEY_R: u32 = 0x0072;
/// `FcitxKey_q`.
const KEY_Q: u32 = 0x0071;
/// `FcitxKey_s`.
const KEY_S: u32 = 0x0073;
/// `FcitxKey_j`.
const KEY_J: u32 = 0x006a;
/// `FcitxKey_x`.
const KEY_X: u32 = 0x0078;
/// `FcitxKey_c`.
const KEY_C: u32 = 0x0063;
/// `FcitxKey_u`.
const KEY_U: u32 = 0x0075;
/// `FcitxKey_d`.
const KEY_D: u32 = 0x0064;
/// `FcitxKey_1`.
const KEY_1: u32 = 0x0031;
/// `FcitxKey_2`.
const KEY_2: u32 = 0x0032;
/// `FcitxKey_5`.
const KEY_5: u32 = 0x0035;
/// `FcitxKey_7`.
const KEY_7: u32 = 0x0037;
/// `FcitxKey_9`.
const KEY_9: u32 = 0x0039;
/// `FcitxKey_BackSpace`.
const KEY_BACKSPACE: u32 = 0xff08;
/// `FcitxKey_Escape`.
const KEY_ESCAPE: u32 = 0xff1b;

/// The one civil-time snapshot every quick-input test pins its formats against.
const SNAPSHOT: ClockSnapshot = ClockSnapshot {
    year: 2026,
    month: 10,
    day: 28,
    hour: 12,
    minute: 34,
    second: 56,
    weekday: 2,
};

/// A clock that always answers [`SNAPSHOT`].
struct FixedClock;

impl ClockSource for FixedClock {
    fn now(&self) -> ClockSnapshot {
        SNAPSHOT
    }
}

/// Installs the pinned clock, once per test process.
///
/// Every test reads the same snapshot, so which test's call wins the once-only slot
/// cannot change what any of them asserts.
fn pinned_clock() {
    let _ = install_clock(std::sync::Arc::new(FixedClock));
}

/// The routing configuration a quick-input test runs under: the feature on, the
/// five date-family spellings live, the calculator per `calc_enabled`.
fn quick_config(calc_enabled: bool) -> RoutingConfig {
    let mut document = ime_config::Config::default();
    document.quick_input.enabled = true;
    document.quick_input.calc_enabled = calc_enabled;
    document.quick_input.date_prefixes = vec![
        String::from("rq"),
        String::from("sj"),
        String::from("xq"),
        String::from("ts"),
        String::from("dt"),
    ];
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// A router with quick input on and the pinned clock installed.
fn quick_router(fixture: &Fixture, calc_enabled: bool) -> KeyRouter<'_> {
    pinned_clock();
    fixture.router_with(PhraseHandle::new(PhraseStore::load(None, 100, true)), quick_config(calc_enabled))
}

#[test]
fn test_quick_date_trigger_opens_panel_and_commits_selected_format() {
    let fixture = Fixture::new("quick-date");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_R, 0), &mut host));
    assert!(
        router.key_event(IC, &press(KEY_Q, 0), &mut host),
        "the key that completes a trigger is the panel's"
    );
    assert_eq!(
        host.highlighted.as_deref(),
        Some("2026-10-28"),
        "the panel's first candidate is the pinned date"
    );
    // The digit keys commit on a date panel: format two is the Chinese numeric one.
    assert!(router.key_event(IC, &press(KEY_2, 0), &mut host));
    assert_eq!(host.commits, ["2026年10月28日"]);
    // The composition underneath is taken back with the commit; the session is
    // idle and the window hidden.
    assert_eq!(router.session(IC).map(|session| session.state), Some(SessionState::Idle));
    assert!(host.hides >= 1);
}

#[test]
fn test_quick_disabled_leaves_the_buffer_and_selection_untouched() {
    let fixture = Fixture::new("quick-off");
    // The shipped configuration: `[quick_input]` off, which is what every build
    // before the panel shipped with.
    let mut router = fixture.router(PhraseHandle::new(PhraseStore::load(None, 100, true)));
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_R, 0), &mut host);
    router.key_event(IC, &press(KEY_Q, 0), &mut host);
    assert!(
        !panel_open(&host),
        "no panel opened: the buffer composes as ordinary input"
    );
    router.key_event(IC, &press(KEY_2, 0), &mut host);
    assert!(
        host.commits.is_empty(),
        "a digit after a dead composition selects nothing: {:?}",
        host.commits
    );
}

#[test]
fn test_quick_time_weekday_and_iso_triggers_render_the_injected_clock() {
    for (first, second, first_candidate) in [
        (KEY_S, KEY_J, "12:34"),
        (KEY_X, KEY_Q, "星期三"),
        (KEY_T, KEY_S, "2026-10-28 12:34:56"),
        (KEY_D, KEY_T, "2026-10-28T12:34:56Z"),
    ] {
        let label = format!("trigger {first:x}{second:x}");
        let fixture = Fixture::new(&format!("quick-family-{first:x}{second:x}"));
        let mut router = quick_router(&fixture, false);
        router.activate_reported(IC, ordinary_report());
        let mut host = RecordingHost::default();
        assert!(router.key_event(IC, &press(first, 0), &mut host), "{label}");
        assert!(router.key_event(IC, &press(second, 0), &mut host), "{label}");
        assert_eq!(host.highlighted.as_deref(), Some(first_candidate), "{label}");
    }
}

#[test]
fn test_quick_calc_trigger_builds_an_expression_from_digit_keys() {
    let fixture = Fixture::new("quick-calc");
    let mut router = quick_router(&fixture, true);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_C, 0), &mut host);
    router.key_event(IC, &press(KEY_C, 0), &mut host);
    assert!(
        !panel_open(&host),
        "the bare trigger composes: with no expression there is nothing to evaluate"
    );
    // The digits are payload here, not selection: `1` then `2` build `12`.
    assert!(router.key_event(IC, &press(KEY_1, 0), &mut host));
    assert_eq!(host.highlighted.as_deref(), Some("1"));
    assert!(router.key_event(IC, &press(KEY_2, 0), &mut host));
    assert_eq!(host.highlighted.as_deref(), Some("12"));
    // Space commits the highlighted candidate, the value.
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["12"]);
    assert_eq!(router.session(IC).map(|session| session.state), Some(SessionState::Idle));
}

#[test]
fn test_quick_calc_disabled_leaves_the_trigger_dead() {
    let fixture = Fixture::new("quick-calc-off");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    for key in [KEY_C, KEY_C, KEY_1] {
        router.key_event(IC, &press(key, 0), &mut host);
    }
    assert!(
        !panel_open(&host),
        "with the calculator switched off the trigger never opens a panel"
    );
    // The digit was an ordinary selection key over the composition's own list: the
    // raw spelling went out, and no quick rendering was ever offered.
    assert_eq!(host.commits, ["cc"], "{:?}", host.commits);
}

#[test]
fn test_quick_amount_trigger_renders_four_styles_and_commits_the_first() {
    let fixture = Fixture::new("quick-amount");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_R, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_1, 0), &mut host));
    assert_eq!(host.highlighted.as_deref(), Some("一"), "the plain reading leads");
    assert!(router.key_event(IC, &press(KEY_2, 0), &mut host));
    assert_eq!(host.highlighted.as_deref(), Some("十二"));
    // The capitals candidate carries the accounting rule the card pins.
    let frame = host.last_frame.as_ref().expect("the panel's frame");
    assert_eq!(frame.candidates[1].text, "壹拾贰");
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["十二"]);
}

#[test]
fn test_quick_codepoint_trigger_renders_the_char_and_sixteen_successors() {
    let fixture = Fixture::new("quick-codepoint");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_U, 0), &mut host));
    // `u597d` names 好; the letters are payload here, the digits content.
    assert!(router.key_event(IC, &press(KEY_5, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_9, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_7, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_D, 0), &mut host));
    assert_eq!(host.highlighted.as_deref(), Some("好"));
    let frame = host.last_frame.as_ref().expect("the panel's frame");
    // The whole list is the character plus sixteen successors; the frame carries
    // the page of five the window shows, and the page state names the rest.
    assert_eq!(frame.candidates.len(), 5);
    assert_eq!(frame.page.total, 4, "17 candidates fill four pages of five");
    assert_eq!(frame.candidates[0].annotation.as_deref(), Some("U597D"));
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["好"]);
}

#[test]
fn test_quick_panel_select_by_click_commits_the_clicked_candidate() {
    let fixture = Fixture::new("quick-click");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    router.key_event(IC, &press(KEY_R, 0), &mut host);
    router.key_event(IC, &press(KEY_Q, 0), &mut host);
    let revision = host
        .last_frame
        .as_ref()
        .expect("the panel's frame")
        .revision;

    let acted = router.ui_event(
        IC,
        UiEvent::Select {
            revision,
            index: 1,
            trigger: SelectTrigger::Mouse,
        },
        &mut host,
    );
    assert!(acted, "a click on a live panel candidate is the panel's");
    assert_eq!(host.commits, ["2026年10月28日"]);

    // A click carrying any other revision is stale: the session drops it, and the
    // answer claims nothing.
    let stale = router.ui_event(
        IC,
        UiEvent::Select {
            revision,
            index: 0,
            trigger: SelectTrigger::Mouse,
        },
        &mut host,
    );
    assert!(!stale);
    assert_eq!(host.commits.len(), 1, "{:?}", host.commits);
}

#[test]
fn test_quick_backspace_hands_control_back_to_the_session() {
    let fixture = Fixture::new("quick-backspace");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    router.key_event(IC, &press(KEY_R, 0), &mut host);
    router.key_event(IC, &press(KEY_Q, 0), &mut host);
    assert!(host.highlighted.is_some(), "the panel is open");

    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert!(
        !panel_open(&host),
        "one character short of the trigger, the panel is gone"
    );
    // The session's own buffer took the same backspace: it holds what is left of
    // the spelling, and a second backspace ends the composition.
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw().to_owned()),
        Some(String::from("r"))
    );
    router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host);
    assert_eq!(router.session(IC).map(|session| session.state), Some(SessionState::Idle));
}

#[test]
fn test_quick_escape_cancels_the_panel_and_the_composition() {
    let fixture = Fixture::new("quick-escape");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    router.key_event(IC, &press(KEY_R, 0), &mut host);
    router.key_event(IC, &press(KEY_Q, 0), &mut host);
    assert!(host.highlighted.is_some(), "the panel is open");

    // Escape closes the panel and cancels the composition underneath: nothing is
    // committed, the window hides, and the session is idle when the next key comes.
    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert!(host.commits.is_empty(), "{:?}", host.commits);
    assert!(host.hides >= 1);
    assert_eq!(router.session(IC).map(|session| session.state), Some(SessionState::Idle));
}

#[test]
fn test_quick_reload_closes_the_panel_and_disables_the_triggers() {
    let fixture = Fixture::new("quick-reload");
    let mut router = quick_router(&fixture, false);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();
    router.key_event(IC, &press(KEY_R, 0), &mut host);
    router.key_event(IC, &press(KEY_Q, 0), &mut host);
    assert!(host.highlighted.is_some(), "the panel is open");

    // A reload that switches quick input off closes the panel; the composition the
    // session holds survives it, per the reload rule, and the next digit is an
    // ordinary selection key over a list that has no candidates.
    router.reload(RoutingConfig::default(), &mut host);
    router.key_event(IC, &press(KEY_2, 0), &mut host);
    assert!(
        host.commits.is_empty(),
        "a closed panel commits nothing: {:?}",
        host.commits
    );
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw().to_owned()),
        Some(String::from("rq")),
        "the composition the session holds survives the reload"
    );
}
