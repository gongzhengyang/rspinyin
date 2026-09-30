//! Where the highlight box rests, in the candidate grid's own coordinates.
//!
//! Responsibility: turn the candidate the pointer state names into the rectangle the
//! springs are sent to. It is the grid's arithmetic and nothing else -- the grid's origin,
//! its cell width, its columns and its gaps -- which is what keeps the box the user sees on
//! the cell the pointer hits.
//!
//! The same arithmetic is the hit map's (`crate::geometry`), restated in logical pixels
//! because the two sides work in different units: the hit map answers in physical pixels of
//! the container, the springs integrate in logical pixels of the grid.

#[cfg(test)]
mod tests;

use crate::layout::Metrics;
use crate::spring::HighlightRect;

use super::frame::DrawState;

/// The rectangle the highlight box rests on, in the candidate grid's own coordinates.
///
/// The grid's origin is the candidate area's top-left corner, inside the container padding
/// and below the header: the window adds both when it draws the box, which is the only
/// place the two spaces meet. The arithmetic is the hit map's (`crate::geometry`), restated
/// in logical pixels, so the box the user sees and the cell the pointer hits cannot drift
/// apart.
///
/// # Parameters
///
/// * `state` -- the state the component is drawing, which fixes the cell width, the columns
///   per row and the gaps the grid draws with.
/// * `position` -- the candidate's zero-based position within the page, which is what
///   [`PointerState`](super::cell::PointerState) holds and what the grid numbers its cells
///   by.
/// * `metrics` -- the component's constants, from [`crate::layout::metrics`].
///
/// # Returns
///
/// The cell's rectangle in logical pixels, relative to the candidate grid's origin. A
/// position past the last cell of the page still lands on the grid, which is what keeps a
/// stale highlight visible instead of collapsing it onto the origin.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics: the column count is kept away from zero and the arithmetic is `f32`.
pub fn highlight_rect(state: &DrawState, position: u16, metrics: &Metrics) -> HighlightRect {
    // A page with no column -- an empty state -- still has one, which is what the grid
    // itself clamps to when it lays its rows out.
    let columns = state.max_per_row.max(1);
    let position = i32::from(position);
    let column = position % columns;
    let row = position / columns;
    HighlightRect::new(
        column as f32 * (state.cell_width + metrics.grid_gap),
        row as f32 * (metrics.cell_height + metrics.grid_gap),
        state.cell_width.max(0.0),
        metrics.cell_height.max(0.0),
    )
}
