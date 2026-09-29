//! The event-driven tests of the session state machine.
//!
//! Paging, the mouse events, the configuration reload and the frames the machine
//! emits, one test per behaviour. The design's transition table and the exhaustive
//! sweeps over the state and event spaces live next door, in the `sweep_tests`
//! module.

use ime_types::{KeyAction, UiEvent};

use super::SessionConfig;
use super::machine::{Session, SessionState, step};
use super::paging::{MAX_PAGES, Paging};
use super::tests::{
    Fixture, committed, composing, diagnosed, dismiss_event, every_key_action, frame_of, hidden,
    highlighted, hover_event, index_of, kinds, page_event, preedit_text, select_event,
};
use crate::viterbi::lattice::WORDS_PER_KEY;

#[test]
fn test_composing_page_next_turns_the_page_and_highlights_its_first_candidate() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::PageNext, &cfg, &env);

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.paging.page, 1);
    assert_eq!(
        session.paging.highlight,
        u16::from(session.paging.page_size)
    );
    assert_eq!(
        frame_of(&effects).map(|frame| frame.page.current),
        Some(2),
        "the frame reports the page one-based"
    );
    assert_eq!(session.buf.raw(), "ni", "paging never touches the input");
}

#[test]
fn test_composing_page_next_on_the_last_page_emits_nothing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let _ = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let _ = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let last = session.paging;

    let effects = session.handle_key(KeyAction::PageNext, &cfg, &env);

    assert!(effects.is_empty(), "the pages do not wrap");
    assert_eq!(session.paging, last);
}

#[test]
fn test_composing_tab_moves_the_highlight_without_touching_the_input() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::MoveHighlight(1), &cfg, &env);

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.paging.highlight, 1);
    assert_eq!(session.buf.raw(), "ni");
    assert_eq!(
        preedit_text(&effects),
        None,
        "the composing text did not change, so no preedit is emitted"
    );
}

#[test]
fn test_composing_move_caret_changes_only_the_preedit() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");

    let effects = session.handle_key(KeyAction::MoveCaret(-1), &cfg, &env);

    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    assert_eq!(session.buf.raw(), "nihao");
    assert_eq!(session.buf.caret(), 2);
}

#[test]
fn test_composing_hover_moves_the_highlight() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Hover {
            revision,
            index: Some(4),
        }),
        &cfg,
        &env,
    );

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.paging.highlight, 4);
    assert_eq!(session.buf.raw(), "ni");
}

#[test]
fn test_composing_hover_off_the_grid_changes_nothing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let before = session.paging;

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Hover {
            revision,
            index: None,
        }),
        &cfg,
        &env,
    );

    assert!(effects.is_empty());
    assert_eq!(session.paging, before);
}

#[test]
fn test_stale_ui_event_is_dropped_with_a_diagnostic() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let makers: [fn(u32) -> UiEvent; 4] = [select_event, hover_event, page_event, dismiss_event];

    for make in makers {
        let mut session = composing(&cfg, &env, "ni");
        let stale = session.revision.value().wrapping_add(1);
        assert_ne!(stale, session.revision.value());
        let event = make(stale);

        let effects = step(&mut session, super::SessionEvent::Ui(event), &cfg, &env);

        assert_eq!(kinds(&effects), ["diagnose"]);
        assert!(
            diagnosed(&effects).is_some_and(|code| code.starts_with("ui/stale-select")),
            "the diagnostic names the stale revision"
        );
        assert_eq!(session.state, SessionState::Composing);
        assert_eq!(session.buf.raw(), "ni");
        assert_eq!(committed(&effects), None);
    }
}

#[test]
fn test_select_with_a_matching_revision_commits_the_clicked_candidate() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let expected = session.decoded().candidates[3].text.clone();

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Select {
            revision,
            index: 3,
            trigger: ime_types::SelectTrigger::Mouse,
        }),
        &cfg,
        &env,
    );

    assert_eq!(committed(&effects), Some(expected.as_str()));
    assert_eq!(session.state, SessionState::Committing);
}

#[test]
fn test_select_repeated_for_the_same_revision_commits_once() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let click = super::SessionEvent::Ui(UiEvent::Select {
        revision,
        index: 1,
        trigger: ime_types::SelectTrigger::Mouse,
    });

    let first = step(&mut session, click.clone(), &cfg, &env);
    let second = step(&mut session, click, &cfg, &env);

    assert_eq!(kinds(&first), ["commit"]);
    assert!(
        second.is_empty(),
        "a click that raced its frame must not select twice"
    );
    assert_eq!(session.state, SessionState::Committing);
}

#[test]
fn test_select_naming_no_candidate_is_dropped() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Select {
            revision,
            index: u16::MAX,
            trigger: ime_types::SelectTrigger::Mouse,
        }),
        &cfg,
        &env,
    );

    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_dismiss_cancels_the_composition() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let reasons = [
        ime_types::DismissReason::OutsideClick,
        ime_types::DismissReason::Escape,
        ime_types::DismissReason::ScrollUpEmpty,
    ];
    for reason in reasons {
        let mut session = composing(&cfg, &env, "ni");
        let revision = session.revision.value();

        let effects = step(
            &mut session,
            super::SessionEvent::Ui(UiEvent::Dismiss { revision, reason }),
            &cfg,
            &env,
        );

        assert_eq!(
            kinds(&effects),
            ["set-client-preedit", "hide"],
            "{reason:?}"
        );
        assert_eq!(hidden(&effects), Some(ime_types::HideReason::Cancelled));
        assert_eq!(session.state, SessionState::Cancelling, "{reason:?}");
    }
}

#[test]
fn test_page_event_turns_the_page() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Page {
            revision,
            dir: ime_types::PageDir::Next,
        }),
        &cfg,
        &env,
    );

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.paging.page, 1);
}

#[test]
fn test_rendered_event_carries_no_business_meaning() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();

    let effects = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Rendered {
            revision,
            raster: core::time::Duration::from_micros(300),
            presented_at_unix_nanos: 0,
        }),
        &cfg,
        &env,
    );

    assert!(
        effects.is_empty(),
        "a render receipt must not raise a diagnostic"
    );
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_config_reload_keeps_the_input_and_the_candidates() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let candidates = session.decoded().candidates.clone();

    let next = SessionConfig {
        max_per_row: 9,
        ..cfg
    };
    let effects = step(
        &mut session,
        super::SessionEvent::ConfigReloaded(next),
        &cfg,
        &env,
    );

    assert_eq!(kinds(&effects), ["send-frame"]);
    assert_eq!(session.buf.raw(), "ni", "a reload never resets the input");
    assert_eq!(session.decoded().candidates, candidates);
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.paging.page_size, 9);
    assert_eq!(
        frame_of(&effects).map(|frame| frame.layout.max_per_row),
        Some(9)
    );
}

#[test]
fn test_config_reload_that_changes_nothing_the_session_reads_is_idempotent() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let before = session.paging;

    let effects = step(
        &mut session,
        super::SessionEvent::ConfigReloaded(cfg),
        &cfg,
        &env,
    );

    assert!(
        effects.is_empty(),
        "an unchanged configuration changes nothing"
    );
    assert_eq!(session.paging, before);
}

#[test]
fn test_config_reload_keeps_the_highlight_on_the_same_word() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let revision = session.revision.value();
    let _ = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Hover {
            revision,
            index: Some(7),
        }),
        &cfg,
        &env,
    );
    let word = highlighted(&session).map(String::from);

    let next = SessionConfig {
        max_per_row: 9,
        ..cfg
    };
    let _ = step(
        &mut session,
        super::SessionEvent::ConfigReloaded(next),
        &cfg,
        &env,
    );

    assert_eq!(session.paging.highlight, 7);
    assert_eq!(session.paging.page, 0, "seven is on the first page of nine");
    assert_eq!(highlighted(&session).map(String::from), word);
}

#[test]
fn test_config_reload_while_idle_emits_nothing() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let next = SessionConfig {
        max_per_row: 9,
        ..cfg
    };
    let effects = step(
        &mut session,
        super::SessionEvent::ConfigReloaded(next),
        &cfg,
        &env,
    );

    assert!(effects.is_empty(), "there is no window to repaint");
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_enter_temp_english_sets_the_flag_and_cancels_a_composition() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::EnterTempEnglish, &cfg, &env);

    assert!(session.temp_english);
    assert_eq!(session.state, SessionState::Cancelling);
    assert_eq!(hidden(&effects), Some(ime_types::HideReason::Cancelled));
}

#[test]
fn test_temp_english_passes_every_key_through() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::EnterTempEnglish, &cfg, &env);

    for action in every_key_action() {
        if matches!(
            action,
            KeyAction::Escape | KeyAction::CommitHighlighted | KeyAction::CommitRaw
        ) {
            continue;
        }
        let effects = session.handle_key(action, &cfg, &env);
        assert!(effects.is_empty(), "{action:?} must pass through");
        assert_eq!(
            session.buf.raw(),
            "",
            "{action:?} must not reach the input buffer"
        );
        assert!(session.temp_english, "{action:?}");
    }
}

#[test]
fn test_temp_english_leaves_on_enter_or_escape() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    for action in [KeyAction::Escape, KeyAction::CommitHighlighted] {
        let mut session = Session::new();
        let _ = session.handle_key(KeyAction::EnterTempEnglish, &cfg, &env);

        let effects = session.handle_key(action, &cfg, &env);

        assert!(!session.temp_english, "{action:?} leaves the mode");
        assert!(
            effects.is_empty(),
            "{action:?} is handed back to the application"
        );
    }
}

#[test]
fn test_frames_carry_only_the_page_on_show() {
    let cfg = SessionConfig {
        max_per_row: 5,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let total = session.decoded().candidates.len();

    let first = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let page = frame_of(&first);

    assert_eq!(
        page.map(|frame| frame.candidates.len()),
        Some(WORDS_PER_KEY - 5),
        "the second page carries only what is left of the list"
    );
    assert_eq!(page.map(|frame| frame.page.current), Some(2));
    assert_eq!(page.map(|frame| frame.page.total), Some(2));
    assert_eq!(page.map(|frame| frame.page.page_size), Some(5));
    assert_eq!(
        page.map(|frame| frame.candidates[0].text.clone()),
        session
            .decoded()
            .candidates
            .get(5)
            .map(|held| held.text.clone()),
        "the frame starts at the page's first candidate, not the list's"
    );
    assert_eq!(
        total, WORDS_PER_KEY,
        "the fixture lists twelve readings of `ni`, but the lattice keeps only the head \
         of a key's list, so the decode answers with eight"
    );
    assert_eq!(
        page.map(|frame| frame.preedit.text.clone()),
        Some(String::from("ni"))
    );
}

#[test]
fn test_frames_report_the_page_count_capped_at_five() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let total = u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX);
    let size = u16::from(cfg.max_per_row);
    let expected = u8::try_from(total.div_ceil(size))
        .unwrap_or(u8::MAX)
        .min(MAX_PAGES);

    let effects = session.handle_key(KeyAction::MoveHighlight(1), &cfg, &env);

    let page = frame_of(&effects).map(|frame| frame.page);
    assert_eq!(page.map(|state| state.total), Some(expected));
    assert_eq!(page.map(|state| state.page_size), Some(cfg.max_per_row));
    assert!(page.is_some_and(|state| state.total <= MAX_PAGES));
}

#[test]
fn test_frames_carry_the_context_the_engine_wrote() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();
    let anchor = ime_types::Anchor {
        cursor: ime_types::RectI {
            x: 40,
            y: 80,
            w: 2,
            h: 20,
        },
        screen: ime_types::ScreenId::new(1),
        scale: 1.5,
        placement: ime_types::Placement::Below,
    };
    let status = ime_types::StatusStrip {
        mode_label: String::from("中"),
        readonly: true,
        ..ime_types::StatusStrip::default()
    };
    session.set_frame_context(anchor, status.clone());

    let effects = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    let frame = frame_of(&effects);
    assert_eq!(frame.map(|held| held.anchor), Some(anchor));
    assert_eq!(
        frame.map(|held| held.status.clone()),
        Some(status),
        "the mode strip reaches the window unchanged"
    );
    assert_eq!(
        session.frame_context().status.mode_label,
        "中",
        "the session remembers what the engine wrote"
    );
}

#[test]
fn test_paging_state_is_rebuilt_with_the_configured_page_size() {
    let cfg = SessionConfig {
        max_per_row: 9,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert_eq!(session.paging.page_size, 9);
    assert_eq!(session.paging, Paging::with_page_size(9));
}

#[test]
fn test_paging_helpers_are_used_by_the_machine() {
    // A guard against the machine and the paging state drifting apart: the highlight
    // the machine reports is the one the paging state holds, and it is on the page
    // the frame shows.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    for _ in 0..7 {
        let _ = session.handle_key(KeyAction::MoveHighlight(1), &cfg, &env);
    }

    let effects = session.handle_key(KeyAction::Ignore, &cfg, &env);

    assert!(effects.is_empty());
    assert_eq!(session.paging.highlight, 7);
    assert_eq!(session.paging.page, 1);
    assert!(session.paging.local_index() < u16::from(session.paging.page_size));
    let expected = session.decoded().candidates[7].text.clone();
    assert_eq!(highlighted(&session), Some(expected.as_str()));
}

#[test]
fn test_diagnostics_never_carry_the_typed_input() {
    let cfg = SessionConfig {
        max_raw_len: 3,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nih");
    let mut codes = Vec::new();

    // The length cap is the one diagnostic a user can reach by typing, and it must
    // describe the limit without echoing what was typed.
    for action in [
        KeyAction::InputChar('a'),
        KeyAction::InputChar('o'),
        KeyAction::SelectIndex(0),
    ] {
        let effects = session.handle_key(action, &cfg, &env);
        if let Some(code) = diagnosed(&effects) {
            codes.push(code);
        }
    }

    assert!(!codes.is_empty(), "the cap is reached");
    for code in &codes {
        assert!(!code.contains("nih"), "{code}");
        assert!(!code.contains('\''), "{code}");
    }
}

#[test]
fn test_redecode_leaves_the_highlight_on_a_visible_candidate() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    let chosen = session.decoded().candidates[7].text.clone();
    let revision = session.revision.value();
    let _ = step(
        &mut session,
        super::SessionEvent::Ui(UiEvent::Hover {
            revision,
            index: Some(7),
        }),
        &cfg,
        &env,
    );

    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);

    assert!(
        highlighted(&session).is_some(),
        "the highlight never dangles"
    );
    assert!(usize::from(session.paging.highlight) < session.decoded().candidates.len());
    assert!(session.paging.local_index() < u16::from(session.paging.page_size));
    // Either the word the user had chosen is still offered and the highlight followed
    // it, or the list no longer holds it and the highlight went back to the top.
    match index_of(&session, &chosen) {
        Some(at) => assert_eq!(
            session.paging.highlight, at,
            "the highlight followed the word"
        ),
        None => assert_eq!(session.paging.highlight, 0, "the word is gone"),
    }
}
