//! Unit tests for the candidate grid's cell model.

use ime_types::{Candidate, CandidateSource, PageState};

use super::*;
use crate::layout;

/// The metrics of the real `ui/candidate.slint`, which every geometry here builds on.
fn parsed() -> Metrics {
    *layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// A cell geometry of the given width and position, drawing no annotation.
fn geometry(position: u16, width: f32) -> CellGeometry {
    CellGeometry {
        position,
        width,
        annotation_width: 0.0,
        show_annotation: true,
        metrics: parsed(),
    }
}

/// A candidate with `text` and no annotation.
fn candidate(text: &str) -> Candidate {
    Candidate {
        index: 1,
        text: String::from(text),
        annotation: None,
        source: CandidateSource::Dict,
        score: 0.0,
        consumed_syllables: 1,
    }
}

/// A page state of `count` candidates per page, showing the page `current` (one-based).
fn page(current: u8, page_size: u8) -> PageState {
    PageState {
        current,
        total: 5,
        page_size,
    }
}

#[test]
fn test_visual_state_resolve_follows_the_design_priority() {
    // The five rows of 3.4, in the order the table ranks them.
    assert_eq!(
        VisualState::resolve(false, false, false, true),
        VisualState::Disabled,
        "Disabled outranks everything"
    );
    assert_eq!(
        VisualState::resolve(true, true, true, false),
        VisualState::Active,
        "a press outranks the focus ring and the hover"
    );
    assert_eq!(
        VisualState::resolve(true, true, false, false),
        VisualState::FocusRing,
        "the keyboard highlight outranks the hover"
    );
    assert_eq!(
        VisualState::resolve(false, true, false, false),
        VisualState::Hover,
        "the hover outranks the default"
    );
    assert_eq!(
        VisualState::resolve(false, false, false, false),
        VisualState::Default,
        "nothing over the cell leaves it in the default state"
    );
    assert_eq!(
        VisualState::resolve(false, false, false, true),
        VisualState::Disabled,
        "a disabled cell is disabled whatever else is set"
    );
    assert_eq!(VisualState::default(), VisualState::Default);
}

#[test]
fn test_local_position_maps_a_global_index_to_the_page() {
    assert_eq!(local_position(0, &page(1, 5)), Some(0));
    assert_eq!(local_position(4, &page(1, 5)), Some(4));
    // The second page shows the global indices 5..10, so its first candidate is at 0.
    assert_eq!(local_position(5, &page(2, 5)), Some(0));
    assert_eq!(local_position(9, &page(2, 5)), Some(4));
}

#[test]
fn test_local_position_of_an_index_on_another_page_is_none() {
    assert_eq!(
        local_position(0, &page(2, 5)),
        None,
        "an index before the page does not wrap"
    );
    assert_eq!(local_position(4, &page(2, 5)), None);
    // An empty frame has no page at all, and the arithmetic must not underflow.
    assert_eq!(local_position(0, &page(0, 0)), Some(0));
    assert_eq!(local_position(1, &page(0, 0)), Some(1));
}

#[test]
fn test_pointer_state_default_highlights_the_first_candidate() {
    let pointer = PointerState::default();
    assert_eq!(pointer.highlighted, Some(0));
    assert_eq!(pointer.hovered, None);
    assert_eq!(pointer.pressed, None);
}

#[test]
fn test_pointer_state_for_page_re_anchors_all_three_indices() {
    let pointer = PointerState {
        highlighted: Some(6),
        hovered: Some(7),
        pressed: Some(0),
    }
    .for_page(&page(2, 5));
    assert_eq!(pointer.highlighted, Some(1));
    assert_eq!(pointer.hovered, Some(2));
    assert_eq!(
        pointer.pressed, None,
        "an index on the page the user left names no cell here"
    );
}

#[test]
fn test_cell_geometry_text_budget_subtracts_the_chrome_and_the_annotation() {
    let plain = geometry(0, 66.0);
    // 66dp of cell minus the 36dp every cell spends on its padding and number label.
    assert_eq!(plain.text_budget(), 30.0);
    let annotated = CellGeometry {
        annotation_width: 20.0,
        ..plain
    };
    assert_eq!(annotated.text_budget(), 10.0);
}

#[test]
fn test_cell_geometry_text_budget_stops_at_the_text_limit() {
    // 3.1.3 caps the text at 120dp however wide the cell is.
    let wide = geometry(0, 400.0);
    assert_eq!(wide.text_budget(), 120.0);
}

#[test]
fn test_cell_geometry_text_budget_of_a_cell_narrower_than_its_chrome_is_zero() {
    let narrow = geometry(0, 20.0);
    assert_eq!(narrow.text_budget(), 0.0, "a budget is never negative");
}

#[test]
fn test_cell_write_carries_the_candidate_and_its_number_label() {
    let mut cell = CellState::default();
    let candidate = candidate("你好");
    assert!(cell.write(&candidate, geometry(0, 66.0), &mut Measure::default()));
    assert_eq!(cell.index, 1);
    assert_eq!(cell.label, "1");
    assert_eq!(cell.text, "你好");
    assert_eq!(
        cell.display_text, "你好",
        "the text fits the cell untouched"
    );
    assert_eq!(cell.annotation, "");
    assert_eq!(cell.source, CandidateSource::Dict);
}

#[test]
fn test_cell_write_past_the_ninth_candidate_draws_no_label() {
    let mut cell = CellState::default();
    let mut measure = Measure::default();
    cell.write(&candidate("你好"), geometry(8, 66.0), &mut measure);
    assert_eq!(cell.label, "9", "the ninth candidate is still a number key");
    cell.write(&candidate("你好"), geometry(9, 66.0), &mut measure);
    assert_eq!(
        cell.label, "",
        "a tenth candidate has no key of its own, so it draws no label"
    );
    assert_eq!(cell.index, 1, "and it keeps the index a click needs");
}

#[test]
fn test_cell_write_hides_an_annotation_the_layout_turned_off() {
    let mut cell = CellState::default();
    let mut annotated = candidate("你好");
    annotated.annotation = Some(String::from("自造词"));
    let mut measure = Measure::default();
    let shown = CellGeometry {
        annotation_width: 40.0,
        ..geometry(0, 120.0)
    };
    cell.write(&annotated, shown, &mut measure);
    assert_eq!(cell.annotation, "自造词");
    let hidden = CellGeometry {
        show_annotation: false,
        ..shown
    };
    cell.write(&annotated, hidden, &mut measure);
    assert_eq!(cell.annotation, "");
}

#[test]
fn test_cell_write_of_an_unchanged_candidate_reports_no_change() {
    let mut cell = CellState::default();
    let candidate = candidate("你好");
    let mut measure = Measure::default();
    assert!(
        cell.write(&candidate, geometry(0, 66.0), &mut measure),
        "the first write changes everything"
    );
    assert!(
        !cell.write(&candidate, geometry(0, 66.0), &mut measure),
        "a cell that already draws the candidate changes nothing"
    );
}

#[test]
fn test_cell_resolve_marks_only_the_cell_the_pointer_names() {
    let mut cell = CellState::default();
    let pointer = PointerState {
        highlighted: Some(0),
        hovered: Some(1),
        pressed: Some(2),
    };
    assert!(cell.resolve(0, pointer));
    assert_eq!(cell.state, VisualState::FocusRing);
    cell.resolve(1, pointer);
    assert_eq!(cell.state, VisualState::Hover);
    cell.resolve(2, pointer);
    assert_eq!(cell.state, VisualState::Active);
    cell.resolve(3, pointer);
    assert_eq!(cell.state, VisualState::Default);
    assert!(
        !cell.resolve(3, pointer),
        "re-resolving the same cell changes nothing"
    );
}

#[test]
fn test_cell_resolve_press_outranks_the_highlight_on_one_cell() {
    let mut cell = CellState::default();
    let pointer = PointerState {
        highlighted: Some(0),
        hovered: None,
        pressed: Some(0),
    };
    cell.resolve(0, pointer);
    assert_eq!(
        cell.state,
        VisualState::Active,
        "3.4 ranks the press above the focus ring"
    );
}

#[test]
fn test_measure_caches_a_repeated_text() {
    let mut measure = Measure::default();
    assert!(measure.is_empty());
    let first = measure.width("你好", 15.0);
    assert_eq!(first, 30.0, "two CJK glyphs fill two ems");
    assert_eq!(measure.misses(), 1);
    let second = measure.width("你好", 15.0);
    assert_eq!(second, first);
    assert_eq!(measure.misses(), 1, "the second measurement is a hit");
    assert_eq!(measure.len(), 1);
    // The em width is cached rather than a pixel width, so the smaller font reads the same
    // entry and only the multiplication differs.
    assert_eq!(measure.width("你好", 11.0), 22.0);
    assert_eq!(measure.misses(), 1, "a second font size is still a hit");
}

#[test]
fn test_measure_width_follows_the_script_of_each_character() {
    let mut measure = Measure::default();
    assert_eq!(measure.width("abcd", 10.0), 20.0, "ASCII is half an em");
    assert_eq!(measure.width("你好", 10.0), 20.0, "CJK is one em");
    assert_eq!(
        measure.width("ni好", 10.0),
        20.0,
        "a mixed text adds the two"
    );
    assert_eq!(measure.width("", 10.0), 0.0, "an empty text is no width");
}

#[test]
fn test_measure_evicts_the_least_recently_used_entry() {
    let mut measure = Measure::default();
    for index in 0..MEASURE_CACHE_CAPACITY {
        measure.width(&format!("w{index}"), 15.0);
    }
    assert_eq!(measure.len(), MEASURE_CACHE_CAPACITY);
    let misses = measure.misses();
    // The entry of the very first text is the least recently used one, so it goes first.
    measure.width(&format!("w{}", MEASURE_CACHE_CAPACITY), 15.0);
    assert_eq!(
        measure.len(),
        MEASURE_CACHE_CAPACITY,
        "the cache stays bounded"
    );
    assert_eq!(measure.misses(), misses + 1);
    measure.width("w0", 15.0);
    assert_eq!(
        measure.misses(),
        misses + 2,
        "the least recently used text was the one evicted"
    );
}

#[test]
fn test_write_elided_text_that_fits_is_untouched() {
    let mut measure = Measure::default();
    let mut target = String::new();
    assert!(measure.write_elided(&mut target, "你好", geometry(0, 66.0)));
    assert_eq!(target, "你好");
    assert!(
        !measure.write_elided(&mut target, "你好", geometry(0, 66.0)),
        "the buffer already holds it"
    );
}

#[test]
fn test_write_elided_empty_text_writes_nothing() {
    let mut measure = Measure::default();
    let mut target = String::from("stale");
    assert!(measure.write_elided(&mut target, "", geometry(0, 66.0)));
    assert_eq!(target, "");
}

/// The four classes of 3.1.3 the truncation has to handle, each far wider than the cell.
const LONG_TEXTS: [&str; 4] = [
    "你好世界你好世界你好世界你好世界你好世界你好世界你好世界你好世界",
    "the quick brown fox jumps over the lazy dog",
    "ni好hao世界ni好hao世界ni好hao世界",
    "😀😀😀😀😀😀😀😀😀😀",
];

#[test]
fn test_write_elided_long_texts_all_end_with_the_ellipsis_and_fit() {
    let cell = geometry(0, 66.0);
    let budget = cell.text_budget();
    let size = cell.metrics.font_size_cell;
    let mut measure = Measure::default();
    for text in LONG_TEXTS {
        let mut target = String::new();
        measure.write_elided(&mut target, text, cell);
        assert!(
            target.ends_with('…'),
            "{text:?} is wider than the cell, so it is cut and marked: {target:?}"
        );
        assert!(
            measure.width(&target, size) <= budget,
            "{target:?} must fit the {budget}dp budget"
        );
        assert!(text.starts_with(&target[..target.len() - '…'.len_utf8()]));
        assert!(
            target.len() < text.len(),
            "{target:?} is shorter than its text"
        );
    }
}

#[test]
fn test_write_elided_cuts_on_a_character_boundary() {
    // Wide enough that every class really is cut rather than reduced to the ellipsis.
    let cell = geometry(0, 90.0);
    let mut measure = Measure::default();
    for text in LONG_TEXTS {
        let mut target = String::new();
        measure.write_elided(&mut target, text, cell);
        // A cut inside a multi-byte character would not have produced a valid `String` at
        // all; the assertion is that the prefix is one of the text's own boundaries.
        let prefix = &target[..target.len() - '…'.len_utf8()];
        assert!(text.is_char_boundary(prefix.len()), "{target:?}");
        assert!(text.starts_with(prefix));
    }
}

#[test]
fn test_write_elided_budget_too_small_for_the_text_shows_the_ellipsis_alone() {
    let mut measure = Measure::default();
    let mut target = String::new();
    // A cell narrower than its own chrome leaves no budget at all, and an empty cell would
    // read as a candidate with nothing to show.
    measure.write_elided(&mut target, "你好", geometry(0, 20.0));
    assert_eq!(target, "…");
}

#[test]
fn test_write_text_and_replace_report_a_change_only_when_there_is_one() {
    let mut buffer = String::from("ni");
    assert!(!write_text(&mut buffer, "ni"));
    assert!(write_text(&mut buffer, "hao"));
    assert_eq!(buffer, "hao");
    let mut value = 3u8;
    assert!(!replace(&mut value, 3));
    assert!(replace(&mut value, 4));
    assert_eq!(value, 4);
}
