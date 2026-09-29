//! Unit tests for the frame mapping: `UiFrame` in, component properties out.
//!
//! Every scene here is a pure function of a frame and the component's constants, so none of
//! them needs a Slint platform: the component is exercised separately, in
//! `crate::adapter::tests`.

use ime_types::StatusStrip;

use super::*;
use crate::adapter::cell::VisualState;
use crate::adapter::tests::frame_with;

/// The metrics of the real `candidate.slint`, which every mapping test builds on.
fn parsed() -> Metrics {
    *layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// The state one frame maps to, with the configured panel ceiling.
fn mapped(frame: &UiFrame) -> DrawState {
    let metrics = parsed();
    draw_state(frame, container_cap(frame, &metrics), &metrics)
}

#[test]
fn test_draw_state_empty_frame_draws_only_the_header() {
    let frame = frame_with(1, "ni", 0);
    let state = mapped(&frame);
    assert_eq!(state.item_count, 0);
    assert_eq!(state.grid_rows, 0, "no candidate occupies no row");
    assert!(state.cells.is_empty(), "and the grid draws no cell");
    assert_eq!(state.preedit_text, "ni");
    // The compressed 28dp header, the rule and the two 8dp paddings.
    assert_eq!(state.header_height, 28.0);
    assert_eq!(state.container_height, 45.0);
    assert_eq!(state.container_width, 220.0, "the narrowest panel");
}

#[test]
fn test_draw_state_single_candidate_fills_one_cell() {
    let frame = frame_with(2, "ni", 1);
    let state = mapped(&frame);
    assert_eq!(state.item_count, 1);
    assert_eq!(state.grid_rows, 1);
    assert_eq!(state.max_per_row, 5);
    assert_eq!(state.header_height, 34.0);
    // 34dp header + 1dp rule + 2x8dp padding + one 36dp row.
    assert_eq!(state.container_height, 87.0);
}

#[test]
fn test_draw_state_full_page_wraps_into_two_rows() {
    let frame = frame_with(3, "ni'hao", 9);
    let state = mapped(&frame);
    assert_eq!(state.item_count, 9);
    assert_eq!(state.grid_rows, 2, "nine candidates at five per row");
    // 5 x 66dp cells + 4 x 6dp gaps + 2 x 8dp padding; the cells fit two glyphs.
    assert_eq!(state.cell_width, 66.0);
    assert_eq!(state.container_width, 370.0);
    // 34dp header + 1dp rule + 2x8dp padding + 2 x 36dp rows + 6dp row gap.
    assert_eq!(state.container_height, 129.0);
}

#[test]
fn test_draw_state_page_shorter_than_a_row_keeps_the_cells_it_has() {
    let frame = frame_with(4, "ni", 3);
    let state = mapped(&frame);
    assert_eq!(state.item_count, 3);
    assert_eq!(state.grid_rows, 1, "three candidates fit one row");
    assert_eq!(
        state.max_per_row, 5,
        "the row length stays the configured one; the component clamps its last row"
    );
    // 3 x 66dp cells + 2 x 6dp gaps + 2 x 8dp padding, which is past the 220dp floor.
    assert_eq!(state.container_width, 226.0);
}

#[test]
fn test_draw_state_long_preedit_keeps_the_newest_characters() {
    let text: String = "ni'hao".chars().cycle().take(200).collect();
    let frame = frame_with(5, &text, 1);
    let state = mapped(&frame);
    assert_eq!(state.preedit_text.chars().count(), PREEDIT_MAX_CHARS);
    assert!(
        text.ends_with(state.preedit_text.as_str()),
        "the tail of the input is what stays visible"
    );
    assert_ne!(state.preedit_text, text, "the head really was cut");
}

#[test]
fn test_draw_state_preedit_inside_the_bound_is_untouched() {
    let frame = frame_with(6, "ni'hao'ma", 1);
    let state = mapped(&frame);
    assert_eq!(state.preedit_text, "ni'hao'ma");
}

#[test]
fn test_draw_state_max_per_row_out_of_range_is_clamped() {
    let metrics = parsed();
    let mut frame = frame_with(7, "ni", 20);
    frame.layout.max_per_row = 0;
    assert_eq!(
        mapped(&frame).max_per_row,
        3,
        "the smallest grid 3.1.1 allows"
    );
    frame.layout.max_per_row = 200;
    assert_eq!(
        mapped(&frame).max_per_row,
        9,
        "the largest grid 3.1.1 allows"
    );
    // A corrupted pair of bounds cannot panic either.
    frame.layout.max_per_row = u8::MAX;
    assert!(mapped(&frame).max_per_row >= i32::from(metrics.min_per_row));
}

#[test]
fn test_per_row_clamp_agrees_with_the_layout_column_count() {
    let metrics = parsed();
    for configured in [0u8, 1, 3, 5, 9, 12, 200] {
        let columns = layout::grid(64, 1, configured, &metrics).cols;
        assert_eq!(
            per_row(configured, &metrics),
            columns,
            "configured {configured} must clamp the same way the layout does"
        );
    }
}

#[test]
fn test_draw_state_mode_label_follows_the_status_strip() {
    let mut frame = frame_with(8, "ni", 1);
    frame.status = StatusStrip {
        mode_label: String::from("拼音"),
        ..StatusStrip::default()
    };
    assert_eq!(mapped(&frame).mode_label, "拼音");
    frame.status.mode_label.clear();
    assert_eq!(mapped(&frame).mode_label, "");
}

#[test]
fn test_draw_state_cell_width_follows_the_text_length() {
    let metrics = parsed();
    let short = mapped(&frame_with(9, "ni", 1));
    let mut long_frame = frame_with(10, "ni", 1);
    long_frame.candidates[0].text = String::from("你好世界");
    let long = mapped(&long_frame);
    // Two glyphs at the cell font size plus the chrome every cell spends around its text.
    assert_eq!(
        short.cell_width,
        2.0 * metrics.font_size_cell + metrics.cell_chrome_width
    );
    assert!(
        long.cell_width > short.cell_width,
        "a longer candidate needs a wider cell: {} vs {}",
        long.cell_width,
        short.cell_width
    );
    assert!(
        long.cell_width <= metrics.max_text_width + metrics.cell_chrome_width,
        "3.1.3 stops a cell once its text reaches the text limit"
    );
}

#[test]
fn test_draw_state_unchanged_frame_reports_no_change_and_keeps_its_buffers() {
    let frame = frame_with(11, "ni'hao", 9);
    let metrics = parsed();
    let cap = container_cap(&frame, &metrics);
    let mut state = DrawState::default();
    let first = state.update(&frame, cap, &metrics);
    assert!(!first.is_empty(), "the first frame changes everything");
    let capacity = state.preedit_text.capacity();
    let second = state.update(&frame, cap, &metrics);
    assert!(
        second.is_empty(),
        "a frame that draws the same state changes nothing: {second:?}"
    );
    assert_eq!(
        state.preedit_text.capacity(),
        capacity,
        "the buffer is reused rather than reallocated"
    );
    // The same holds for a frame whose text changed but kept its length: the buffer is
    // refilled rather than replaced, so no allocation happens either.
    let other = frame_with(12, "ma'fan", 9);
    let third = state.update(&other, cap, &metrics);
    assert!(
        third.preedit_text,
        "different text is a change, so the property is written"
    );
    assert_eq!(state.preedit_text, "ma'fan");
    assert_eq!(state.preedit_text.capacity(), capacity);
}

#[test]
fn test_draw_state_reports_the_properties_a_frame_changed() {
    let metrics = parsed();
    let cap = 720.0;
    let mut state = DrawState::default();
    state.update(&frame_with(13, "ni", 1), cap, &metrics);
    let delta = state.update(&frame_with(14, "ni", 9), cap, &metrics);
    assert!(delta.item_count && delta.grid_rows);
    assert!(delta.container_width && delta.container_height);
    assert!(
        !delta.preedit_text
            && !delta.mode_label
            && !delta.header_height
            && !delta.cell_width
            && !delta.max_per_row,
        "a property whose value did not change is not written: {delta:?}"
    );
}

#[test]
fn test_draw_state_cells_carry_the_page_candidates() {
    let state = mapped(&frame_with(50, "ni'hao", 3));
    assert_eq!(state.cells.len(), 3);
    assert_eq!(state.cells[0].index, 1);
    assert_eq!(state.cells[0].label, "1");
    assert_eq!(state.cells[2].label, "3");
    assert_eq!(state.cells[0].text, "你好");
    assert_eq!(
        state.cells[0].display_text, "你好",
        "two glyphs fit the cell"
    );
    assert_eq!(
        state.cells[0].state,
        VisualState::FocusRing,
        "the first candidate of the page carries the highlight"
    );
    assert_eq!(state.cells[1].state, VisualState::Default);
}

#[test]
fn test_draw_state_tenth_candidate_has_no_label_but_keeps_its_index() {
    let state = mapped(&frame_with(60, "ni", 12));
    assert_eq!(
        state.cells.len(),
        12,
        "the grid draws every candidate it is given"
    );
    for position in 0..9 {
        assert_eq!(
            state.cells[position].label,
            (position + 1).to_string(),
            "candidate {} is a number key",
            position + 1
        );
    }
    for position in 9..12 {
        assert!(
            state.cells[position].label.is_empty(),
            "candidate {} has no key of its own, so it draws no label",
            position + 1
        );
        assert_eq!(
            state.cells[position].index,
            u16::try_from(position + 1).unwrap_or(u16::MAX),
            "and it stays in the grid with the index a click names"
        );
    }
}

#[test]
fn test_draw_state_resolves_the_pointer_into_the_cells() {
    let frame = frame_with(70, "ni", 4);
    let metrics = parsed();
    let cap = container_cap(&frame, &metrics);
    let mut state = DrawState::default();
    state.update(&frame, cap, &metrics);
    assert_eq!(
        state.cells[0].state,
        VisualState::FocusRing,
        "the default pointer highlights the first candidate"
    );
    state.pointer = PointerState {
        highlighted: Some(0),
        hovered: Some(1),
        pressed: Some(2),
    };
    assert!(state.resolve_states(), "the pointer moved the states");
    assert_eq!(state.cells[0].state, VisualState::FocusRing);
    assert_eq!(state.cells[1].state, VisualState::Hover);
    assert_eq!(state.cells[2].state, VisualState::Active);
    assert_eq!(state.cells[3].state, VisualState::Default);
    assert!(
        !state.resolve_states(),
        "resolving the same pointer state twice changes nothing"
    );
}

#[test]
fn test_draw_state_unchanged_frame_changes_no_cell() {
    let frame = frame_with(80, "ni'hao", 9);
    let metrics = parsed();
    let cap = container_cap(&frame, &metrics);
    let mut measure = Measure::default();
    let mut state = DrawState::default();
    let first = state.update_cached(&frame, cap, &metrics, &mut measure);
    assert!(first.cells, "the first frame writes the grid");
    let second = state.update_cached(&frame, cap, &metrics, &mut measure);
    assert!(
        !second.cells,
        "a frame that draws the same cells changes none"
    );
    assert!(
        second.is_empty(),
        "and it changes nothing else either: {second:?}"
    );
}

#[test]
fn test_draw_state_cell_width_reserves_room_for_the_annotation() {
    let metrics = parsed();
    let plain = mapped(&frame_with(40, "ni", 1));
    let mut annotated_frame = frame_with(41, "ni", 1);
    annotated_frame.candidates[0].annotation = Some(String::from("自造词"));
    let annotated = mapped(&annotated_frame);
    assert!(
        annotated.cell_width > plain.cell_width,
        "an annotation needs room: {} vs {}",
        annotated.cell_width,
        plain.cell_width
    );
    // Three glyphs at the annotation font size, plus the gap before them.
    assert_eq!(
        annotated.cell_width,
        plain.cell_width + 3.0 * metrics.font_size_small + metrics.annotation_gap
    );
    assert_eq!(annotated.cells[0].annotation, "自造词");
}

#[test]
fn test_draw_state_show_annotation_follows_the_layout_hint() {
    let mut frame = frame_with(42, "ni", 1);
    frame.candidates[0].annotation = Some(String::from("自造词"));
    frame.layout.show_annotation = false;
    let state = mapped(&frame);
    assert!(!state.show_annotation);
    assert_eq!(state.cells[0].annotation, "");
    assert_eq!(
        state.cell_width, 66.0,
        "a cell reserves no room for an annotation the layout turned off"
    );
}

#[test]
fn test_draw_state_truncated_cell_keeps_the_full_text() {
    let text: String = "你好世界".chars().cycle().take(32).collect();
    let mut frame = frame_with(90, "ni", 1);
    frame.candidates[0].text = text.clone();
    let state = mapped(&frame);
    let cell = &state.cells[0];
    assert_eq!(cell.text, text, "the text a selection commits is never cut");
    assert_ne!(cell.display_text, text, "the drawn text is cut");
    assert!(cell.display_text.ends_with('…'), "and marked as cut");
}

#[test]
fn test_draw_state_grid_boundaries_never_overflow() {
    // DoD 5's matrix: every combination of candidate count and row length the window can be
    // handed, including the counts a page cannot actually hold.
    let metrics = parsed();
    let cap = metrics.max_width;
    let mut measure = Measure::default();
    for count in [1usize, 9, 10, 45] {
        for per_row in [3u8, 5, 9] {
            let mut frame = frame_with(100, "ni'hao", count);
            frame.layout.max_per_row = per_row;
            let mut state = DrawState::default();
            state.update_cached(&frame, cap, &metrics, &mut measure);
            assert_eq!(
                state.cells.len(),
                count,
                "{count} candidates at {per_row}/row"
            );
            assert_eq!(
                state.grid_rows,
                i32::try_from(count.div_ceil(usize::from(per_row))).unwrap_or(i32::MAX),
                "{count} candidates at {per_row}/row wrap into rows"
            );
            assert!(state.container_width <= metrics.max_width);
            assert!(state.container_width >= metrics.min_width);
            assert_eq!(state.max_per_row, i32::from(per_row));
        }
    }
}

#[test]
fn test_update_cached_measures_each_distinct_text_once() {
    let metrics = parsed();
    let frame = frame_with(30, "ni'hao", 9);
    let cap = container_cap(&frame, &metrics);
    let mut measure = Measure::default();
    let mut state = DrawState::default();
    state.update_cached(&frame, cap, &metrics, &mut measure);
    assert_eq!(
        measure.misses(),
        1,
        "the nine candidates of a page share one text, so it is measured once"
    );
    for _ in 0..4 {
        state.update_cached(&frame, cap, &metrics, &mut measure);
    }
    assert_eq!(
        measure.misses(),
        1,
        "a repeated frame measures nothing new: this is what keeps a keystroke inside the \
         adapter's budget"
    );
    let mut other = frame_with(31, "ma", 9);
    for candidate in &mut other.candidates {
        candidate.text = String::from("世界");
    }
    state.update_cached(&other, cap, &metrics, &mut measure);
    assert_eq!(
        measure.misses(),
        2,
        "a frame whose text is new measures exactly that text"
    );
}

#[test]
fn test_revision_gate_accepts_the_first_frame_and_drops_an_older_one() {
    let mut gate = RevisionGate::default();
    assert_eq!(gate.current(), None);
    assert!(gate.accept(7), "the first frame is always drawn");
    assert_eq!(gate.current(), Some(7));
    assert!(
        !gate.accept(3),
        "a frame older than the drawn one is dropped"
    );
    assert_eq!(
        gate.current(),
        Some(7),
        "and it does not move the gate back"
    );
    assert!(gate.accept(7), "a redelivery of the same frame is harmless");
    assert!(gate.accept(8), "a newer frame is drawn");
    assert_eq!(gate.current(), Some(8));
}

#[test]
fn test_container_cap_of_a_degenerate_configuration_uses_the_component_maximum() {
    let metrics = parsed();
    let mut frame = frame_with(15, "ni", 1);
    frame.layout.max_width_dp = 400;
    assert_eq!(
        container_cap(&frame, &metrics),
        400.0,
        "the configured ceiling is the one that binds"
    );
    frame.layout.max_width_dp = 0;
    assert_eq!(
        container_cap(&frame, &metrics),
        metrics.max_width,
        "a ceiling that cannot describe a panel falls back to the component's"
    );
}

#[test]
fn test_truncation_start_stays_on_a_character_boundary() {
    let text = "拼音ni'hao";
    for max_chars in 0..=text.chars().count() + 2 {
        let start = truncation_start(text, max_chars);
        assert!(text.is_char_boundary(start), "max_chars {max_chars}");
    }
    assert_eq!(truncation_start(text, 0), text.len(), "an empty tail");
    assert_eq!(
        truncation_start(text, 2),
        text.len() - 2,
        "the last two bytes"
    );
    assert_eq!(truncation_start(text, 100), 0, "everything fits");
}
