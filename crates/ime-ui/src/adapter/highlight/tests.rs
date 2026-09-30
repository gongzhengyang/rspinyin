//! Unit tests for the highlight box's resting place.
//!
//! In a file of their own rather than in `adapter/tests.rs`, and the split is by what a test
//! needs: these cover a pure function of the drawn state, so they need no component, no
//! platform and no fixtures. The tests that drive a live window stay in `adapter/tests.rs`,
//! where the fixtures they share live.

use crate::layout::{self, Metrics};

use super::{DrawState, highlight_rect};

/// The width of one candidate cell: two CJK glyphs and the chrome around them, which is what
/// the frames the adapter maps carry.
fn cell_width(metrics: &Metrics) -> f32 {
    2.0 * metrics.font_size_cell + metrics.cell_chrome_width
}

/// A drawn state of one full row of five cells.
fn row_state(metrics: &Metrics) -> DrawState {
    DrawState {
        max_per_row: 5,
        cell_width: cell_width(metrics),
        ..DrawState::default()
    }
}

#[test]
fn test_highlight_rect_lands_on_the_named_cell() {
    let metrics = layout::metrics().expect("ui/candidate.slint declares its constants");
    let state = row_state(metrics);
    let step_x = cell_width(metrics) + metrics.grid_gap;
    let step_y = metrics.cell_height + metrics.grid_gap;

    // The seventh candidate of a five-per-row page: second column, second row. The box is
    // one cell wide and one cell tall, which is what makes it cover the cell it names.
    let seventh = highlight_rect(&state, 6, metrics);
    assert_eq!(
        (seventh.x, seventh.y),
        (step_x, step_y),
        "the seventh candidate sits one column right and one row down"
    );
    assert_eq!(seventh.w, cell_width(metrics), "and it is one cell wide");
    assert_eq!(seventh.h, metrics.cell_height, "and one cell tall");

    let first = highlight_rect(&state, 0, metrics);
    assert_eq!(
        (first.x, first.y),
        (0.0, 0.0),
        "the first candidate rests on the grid's own origin"
    );
}

#[test]
fn test_highlight_rect_of_a_degenerate_state_stays_on_the_grid() {
    let metrics = layout::metrics().expect("ui/candidate.slint declares its constants");
    // An empty state has no column count and no cell width, which is what a page drawn
    // before its first frame holds. The column count is kept away from zero and the
    // position is still placed on the grid rather than dropped.
    let empty = highlight_rect(&DrawState::default(), 3, metrics);
    assert_eq!(empty.x, 0.0, "a page with no column must not divide by zero");
    assert_eq!(empty.w, 0.0, "and a cell with no width draws nothing");
    assert_eq!(
        empty.y,
        3.0 * (metrics.cell_height + metrics.grid_gap),
        "the position is placed on the grid, one row step per row"
    );

    // A position past the last cell of the page still lands on the grid rather than off it.
    let past = highlight_rect(&row_state(metrics), 12, metrics);
    let step_x = cell_width(metrics) + metrics.grid_gap;
    let step_y = metrics.cell_height + metrics.grid_gap;
    assert_eq!(
        (past.x, past.y),
        (2.0 * step_x, 2.0 * step_y),
        "the thirteenth candidate is two columns right and two rows down"
    );
}
