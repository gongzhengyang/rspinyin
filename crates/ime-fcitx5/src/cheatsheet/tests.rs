//! The cheat sheet's tests: the generated content, and the caller seam that opens and
//! closes the panel.
//!
//! The content tests assert that the panel is a view of the tables the router answers
//! from — the `[keys]` bindings and the engine's chord table — rather than a spelling
//! of this file's own. The seam tests drive the real walk (`Dispatcher::dispatch` with
//! the chord or the release that reaches the panel path) and assert on what the host
//! boundary was handed, with the diagnostic sink recorded instead of the process-wide
//! throttle, which two tests running side by side would otherwise suppress each other
//! through.

use ime_core::state::Session;
use ime_types::ui::OverlayKind;
use ime_types::{ImeError, UiCommand};

use crate::engine::host::Host;
use crate::engine::{Consumed, Dispatcher, KeyEvent, Overlay, RoutingConfig, SessionView};

use super::*;

/// `FcitxKey_p`, the diagnostics chord's key.
const KEY_P: u32 = 0x0070;
/// `FcitxKey_Escape`.
const KEY_ESCAPE: u32 = 0xff1b;
/// `FcitxKey_Shift_L`.
const KEY_SHIFT_L: u32 = 0xffe1;
/// The modifier set both panel chords carry.
const CHORD_STATE: u32 = CTRL | SHIFT;

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 0,
    }
}

/// A key release of `sym` at a host timestamp.
fn release(sym: u32, at_ms: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state: 0,
        is_release: true,
        time_ms: at_ms,
    }
}

/// A host that records the commands it was posted and nothing else.
struct Recorder {
    commands: Vec<UiCommand>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl Host for Recorder {
    fn commit(&mut self, _ic: u64, _text: &str) {}

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

    fn clear_preedit(&mut self, _ic: u64) {}

    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        self.commands.push(command);
    }

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        true
    }

    fn diagnose(&mut self, _ic: u64, _err: &ImeError) {}
}

/// The overlay path over the shipped configuration and a fresh recorder.
fn stage<'a>(config: &'a RoutingConfig, recorder: &'a mut Recorder) -> OverlayStage<'a> {
    OverlayStage {
        config,
        ic: 1,
        host: recorder,
    }
}

/// The overlay commands a recorder was posted, unwrapped from their boxes.
fn overlay_commands(commands: &[UiCommand]) -> Vec<Option<OverlayFrame>> {
    commands
        .iter()
        .map(|command| match command {
            UiCommand::Overlay(frame) => frame.as_deref().cloned(),
            _ => panic!("a non-overlay command reached the panel path"),
        })
        .collect()
}

/// The entries of the section the frame holds under `title`.
fn section_entries<'a>(frame: &'a OverlayFrame, title: &str) -> &'a [OverlayEntry] {
    frame
        .sections
        .iter()
        .find(|section| section.title == title)
        .map(|section| section.entries.as_slice())
        .unwrap_or(&[])
}

#[test]
fn test_build_default_config_yields_three_groups_with_a_fixed_entry_count() {
    let frame = build(&KeyBindings::default(), "全拼");
    assert_eq!(frame.kind, OverlayKind::CheatSheet);
    assert_eq!(frame.title, "按键速查 · 全拼");
    assert_eq!(frame.selected, None, "the cheat sheet highlights nothing");
    assert!(frame.query.is_empty(), "and searches nothing");
    let titles: Vec<&str> = frame
        .sections
        .iter()
        .map(|section| section.title.as_str())
        .collect();
    assert_eq!(
        titles,
        ["组字", "模式", "编辑"],
        "the design's three groups"
    );
    let counts: Vec<usize> = frame
        .sections
        .iter()
        .map(|section| section.entries.len())
        .collect();
    assert_eq!(
        counts,
        [3, 6, 4],
        "组字: highlight, page, enter; 模式: four chords and two panels; 编辑: four rows"
    );
}

#[test]
fn test_build_without_a_scheme_hint_names_no_scheme_in_the_title() {
    let frame = build(&KeyBindings::default(), "");
    assert_eq!(frame.title, "按键速查");
}

#[test]
fn test_build_follows_paging_moved_onto_the_page_keys() {
    let keys = KeyBindings {
        flip_keys: FlipSet::PAGE_UP | FlipSet::PAGE_DOWN,
        ..KeyBindings::default()
    };
    let frame = build(&keys, "");
    let page = section_entries(&frame, "组字")
        .iter()
        .find(|entry| entry.label == "候选列表翻页")
        .expect("the page row is still listed");
    assert_eq!(
        page.keys, "page_up / page_down",
        "the row names the keys the configuration actually bound"
    );
    // The untouched group keeps its own spelling, which is what makes the panel a view
    // of the table rather than of this test.
    let highlight = section_entries(&frame, "组字")
        .iter()
        .find(|entry| entry.label == "移动候选高亮")
        .expect("the highlight row is untouched");
    assert_eq!(highlight.keys, "tab / shift_tab");
}

#[test]
fn test_build_drops_the_highlight_row_when_no_highlight_key_is_bound() {
    // The documented choice: the row is dropped whole, not drawn with a `-` in the key
    // column, because a row that names no key tells the user nothing about a binding
    // the table does not have.
    let keys = KeyBindings {
        highlight_keys: HighlightSet::empty(),
        ..KeyBindings::default()
    };
    let frame = build(&keys, "");
    let composition = section_entries(&frame, "组字");
    assert!(
        composition
            .iter()
            .all(|entry| entry.label != "移动候选高亮"),
        "an unbound list contributes no row"
    );
    assert_eq!(composition.len(), 2, "the page and enter rows remain");
}

#[test]
fn test_build_lists_the_mode_chords_the_engine_table_declares() {
    let frame = build(&KeyBindings::default(), "");
    let named: Vec<(&str, &str)> = section_entries(&frame, "模式")
        .iter()
        .map(|entry| (entry.keys.as_str(), entry.label.as_str()))
        .collect();
    assert_eq!(
        &named[..4],
        &[
            ("Ctrl+Space", "切换中英"),
            ("Shift+Space", "全角 / 半角"),
            ("Ctrl+.", "中英标点"),
            ("Ctrl+Shift+E", "临时英文"),
        ],
        "the chords are read out of the engine's table, in its order"
    );
    assert_eq!(
        &named[4..],
        &[("Ctrl+Shift+/", "命令面板"), ("Ctrl+Shift+P", "诊断面板")],
        "the two fixed panel chords close the group"
    );
}

#[test]
fn test_build_names_every_chord_the_engine_table_holds() {
    // The pin between the two tables: a chord added to the chord table without a name
    // or a label here fails this test rather than vanishing from the panel.
    let unnamed = CHORDS
        .iter()
        .filter(|chord| {
            chord_name(chord.sym, chord.mask).is_none() || chord_label(chord.action).is_none()
        })
        .count();
    assert_eq!(unnamed, 0, "every mode chord is nameable and labelled");
}

#[test]
fn test_build_follows_the_digit_zero_and_enter_settings() {
    let frame = build(&KeyBindings::default(), "");
    assert!(
        section_entries(&frame, "组字")
            .iter()
            .all(|entry| entry.keys != "0"),
        "passthrough is the application's digit, and no row claims it"
    );
    assert!(
        section_entries(&frame, "组字")
            .iter()
            .any(|entry| entry.keys == "Enter" && entry.label == "上屏高亮候选"),
        "the shipped Enter spelling commits the highlighted candidate"
    );

    let keys = KeyBindings {
        digit_zero: DigitZero::Flip,
        enter_commit_raw: true,
        ..KeyBindings::default()
    };
    let frame = build(&keys, "");
    let composition = section_entries(&frame, "组字");
    assert!(
        composition
            .iter()
            .any(|entry| entry.keys == "0" && entry.label == "数字 0 翻到下一页"),
        "the flip spelling of `0` is listed as the routing row answers it"
    );
    assert!(
        composition
            .iter()
            .any(|entry| entry.keys == "Enter" && entry.label == "上屏原始输入"),
        "and the raw-commit spelling of Enter follows its setting"
    );
}

#[test]
fn test_open_requested_panel_opens_the_cheat_sheet_and_posts_the_frame() {
    // The walk's chord matcher names the command palette and the diagnostics panel; the
    // cheat sheet arrives on the same request path, so its arm is driven at the seam
    // directly rather than through a chord the bus does not record yet.
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    assert!(
        open_requested_panel(&mut dispatcher, Overlay::CheatSheet, &mut path, &mut |_| {}),
        "an open reaches the host"
    );
    let frames = overlay_commands(&recorder.commands);
    assert_eq!(
        frames.len(),
        1,
        "exactly one open command travels to the window, got {:?}",
        recorder.commands
    );
    let frame = frames
        .first()
        .and_then(Option::as_ref)
        .expect("the panel was opened");
    assert_eq!(frame.kind, OverlayKind::CheatSheet);
    assert_eq!(frame.title, "按键速查 · 全拼");
    assert_eq!(
        dispatcher.overlay(),
        Some(Overlay::CheatSheet),
        "the bus holds the panel open, so the walk gives it the Escape"
    );
}

#[test]
fn test_apply_overlay_outcome_records_exactly_one_dismissal_when_escape_closes() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.open_overlay(Overlay::CheatSheet);
    let before = dispatcher.overlay();
    assert_eq!(
        dispatcher.dispatch(&press(KEY_ESCAPE, 0), &SessionView::absent()),
        Consumed::Consumed,
        "the walk closes the panel on the Escape"
    );
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    let mut lines: Vec<String> = Vec::new();
    assert!(
        apply_overlay_outcome_with(&mut dispatcher, before, &mut path, |line| {
            lines.push(String::from(line));
        }),
        "the close posts a command"
    );
    assert_eq!(
        lines,
        [String::from(UI_CHEATSHEET_DISMISSED_CODE)],
        "exactly one dismissal line, and it names the dismissal"
    );
    assert_eq!(
        overlay_commands(&recorder.commands),
        [None],
        "the window is told to close the panel"
    );
    assert_eq!(dispatcher.overlay(), None, "and the bus holds nothing open");
}

#[test]
fn test_apply_overlay_outcome_reports_a_dismissal_once_per_close() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.open_overlay(Overlay::CheatSheet);
    let before = dispatcher.overlay();
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    assert_eq!(
        dispatcher.dispatch(&press(KEY_ESCAPE, 0), &SessionView::absent()),
        Consumed::Consumed
    );
    let mut lines: Vec<String> = Vec::new();
    assert!(apply_overlay_outcome_with(
        &mut dispatcher,
        before,
        &mut path,
        |line| {
            lines.push(String::from(line));
        }
    ));
    assert_eq!(
        lines,
        [String::from(UI_CHEATSHEET_DISMISSED_CODE)],
        "one close, one line"
    );
    // A second application for the same closed panel must not repeat the line: the
    // before-snapshot is the caller's half of the exactly-once guarantee.
    let mut lines_after: Vec<String> = Vec::new();
    let overlay_before = dispatcher.overlay();
    assert!(!apply_overlay_outcome_with(
        &mut dispatcher,
        overlay_before,
        &mut path,
        |line| lines_after.push(String::from(line)),
    ));
    assert!(
        lines_after.is_empty(),
        "a panel that was already closed reports nothing"
    );
}

#[test]
fn test_apply_overlay_outcome_dismisses_a_foreign_panel_without_the_cheat_sheet_line() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.open_overlay(Overlay::Diagnostics);
    let before = dispatcher.overlay();
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    assert_eq!(
        dispatcher.dispatch(&press(KEY_ESCAPE, 0), &SessionView::absent()),
        Consumed::Consumed
    );
    let mut lines: Vec<String> = Vec::new();
    assert!(apply_overlay_outcome_with(
        &mut dispatcher,
        before,
        &mut path,
        |line| {
            lines.push(String::from(line));
        }
    ));
    assert!(
        lines.is_empty(),
        "the dismissal code names the cheat sheet, and no cheat sheet was open"
    );
    assert_eq!(
        overlay_commands(&recorder.commands),
        [None],
        "the window's slot is cleared whatever the panel was"
    );
}

#[test]
fn test_apply_overlay_outcome_keeps_the_not_implemented_report_for_the_other_panels() {
    let session = Session::new();
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    assert_eq!(
        dispatcher.dispatch(&press(KEY_P, CHORD_STATE), &view),
        Consumed::Consumed
    );
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    let mut lines: Vec<String> = Vec::new();
    let overlay_before = dispatcher.overlay();
    assert!(!apply_overlay_outcome_with(
        &mut dispatcher,
        overlay_before,
        &mut path,
        |line| lines.push(String::from(line)),
    ));
    assert_eq!(
        lines,
        [String::from("ui/not-implemented: diagnostics")],
        "today's behaviour, unchanged"
    );
    assert!(
        recorder.commands.is_empty(),
        "nothing is drawn for a panel that is not implemented"
    );
}

#[test]
fn test_apply_overlay_outcome_writes_and_clears_the_panel_on_a_long_press_release() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    assert!(
        dispatcher.arm_hold(&press(KEY_SHIFT_L, 0), true),
        "the hold is armed by the modifier press"
    );
    // The release that ends the hold: past the threshold, with nothing typed.
    assert_eq!(
        dispatcher.dispatch(&release(KEY_SHIFT_L, 300), &SessionView::absent()),
        Consumed::Consumed
    );
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    let mut lines: Vec<String> = Vec::new();
    let overlay_before = dispatcher.overlay();
    assert!(apply_overlay_outcome_with(
        &mut dispatcher,
        overlay_before,
        &mut path,
        |line| lines.push(String::from(line)),
    ));
    let frames = overlay_commands(&recorder.commands);
    assert_eq!(
        frames.len(),
        2,
        "the gesture writes the panel and clears it, got {frames:?}"
    );
    assert!(
        frames.first().is_some_and(|frame| frame.is_some()),
        "written first"
    );
    assert!(
        frames.last().is_some_and(|frame| frame.is_none()),
        "then cleared by the same release"
    );
    assert!(
        lines.is_empty(),
        "the long-press path records nothing: a hold is part of typing"
    );
    assert_eq!(
        dispatcher.overlay(),
        None,
        "and the bus is left with no panel the user did not ask for"
    );
}

#[test]
fn test_apply_overlay_outcome_ignores_a_short_release_that_was_a_mode_switch() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.arm_hold(&press(KEY_SHIFT_L, 0), true);
    assert_eq!(
        dispatcher.dispatch(&release(KEY_SHIFT_L, 10), &SessionView::absent()),
        Consumed::Consumed
    );
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    let overlay_before = dispatcher.overlay();
    assert!(
        !apply_overlay_outcome(&mut dispatcher, overlay_before, &mut path),
        "a hold under the threshold is a mode switch, not the panel's gesture"
    );
    assert!(
        recorder.commands.is_empty(),
        "and it posts nothing to the window"
    );
}

#[test]
fn test_apply_overlay_outcome_costs_nothing_when_the_walk_left_nothing() {
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let config = RoutingConfig::default();
    let mut recorder = Recorder::new();
    let mut path = stage(&config, &mut recorder);
    let overlay_before = dispatcher.overlay();
    assert!(
        !apply_overlay_outcome(&mut dispatcher, overlay_before, &mut path),
        "no outcome, no work"
    );
    assert!(recorder.commands.is_empty());
}
