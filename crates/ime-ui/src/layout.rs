//! Candidate-window layout arithmetic.
//!
//! The window's geometry has exactly one source of truth: the `CandidateMetrics` global
//! in `ui/candidate.slint`. The constants are read back out of that file by
//! [`fn@metrics`] rather than restated here, so a size cannot drift between what is drawn
//! and what is computed.
//!
//! The flow is one pass, each step feeding the next:
//!
//! ```text
//! cell_width  -> the shared cell width the grid can afford, before the panel is settled
//! grid        -> rows, columns and paging for the candidates on screen
//! panel_and_cells -> the panel's logical size, then the cells stretched to fill it
//! window_size -> the surface's physical size, shadow reserve included
//! container_rect -> the panel's physical rectangle, which is also the input region
//! ```
//!
//! Everything is in logical pixels, which this project calls `dp`. The scale factor enters
//! only in [`window_size`] and [`container_rect`], and those two are where physical pixels
//! begin.

use ime_types::RectI;

use crate::platform::{normalize_scale, physical_dimension};

mod metrics;

pub use metrics::{Metrics, metrics, parse_metrics};

/// Cell width the grid should use, and whether that width forces the text to be elided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellWidth {
    /// Width of one cell, in logical pixels; every cell in the grid shares it.
    pub width: f32,
    /// True when at least one candidate was wider than the cell, so its text must be
    /// elided (3.1.3). The full text is never lost: it is what gets committed.
    pub truncated: bool,
}

/// Rows, columns and paging of the candidate area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridLayout {
    /// Candidates on a full row, which is what the container's width is built from.
    pub cols: u8,
    /// Rows the current page occupies.
    pub rows: u8,
    /// Pages the window shows, capped by the five-page limit of 3.1.3.
    pub pages: u8,
    /// True when the candidate list runs past the five-page limit.
    pub overflow: bool,
}

/// Size of the panel, shadow reserve excluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerSize {
    /// Panel width in logical pixels.
    pub width: f32,
    /// Panel height in logical pixels.
    pub height: f32,
}

/// Physical size of the whole window surface, shadow reserve included.
///
/// The surface covers the panel plus the transparent margin, because the shadow is drawn
/// into that margin (3.1.1); the geometry task places the window with this size.
///
/// # Parameters
///
/// * `container_width`, `container_height` -- panel size in logical pixels, as returned by
///   [`container_size`].
/// * `scale` -- device pixel ratio; a value that cannot describe a surface is corrected to
///   `1.0` rather than rejected.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// `(width, height)` in physical pixels, each at least `1`.
///
/// # Panics
///
/// Never panics: a negative, zero or non-finite size collapses to the smallest surface
/// instead of overflowing.
pub fn window_size(
    container_width: f32,
    container_height: f32,
    scale: f32,
    metrics: &Metrics,
) -> (u32, u32) {
    let scale = normalize_scale(scale);
    let margin = metrics.shadow_margin;
    (
        physical_dimension(whole_dp(container_width + 2.0 * margin), scale),
        physical_dimension(whole_dp(container_height + 2.0 * margin), scale),
    )
}

/// Physical rectangle of the panel inside the window surface.
///
/// This is the region the pointer must be able to hit; everything outside it is the
/// transparent shadow reserve, which stays out of the backend's input region so that a
/// click there reaches the application underneath (3.1.1).
///
/// # Parameters
///
/// * `window_width`, `window_height` -- surface size in physical pixels, as returned by
///   [`window_size`].
/// * `scale` -- device pixel ratio, the same one the surface was created with.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// The panel's rectangle in physical pixels, always at least 1x1 and always inside the
/// surface.
///
/// # Panics
///
/// Never panics: the subtraction saturates, so a surface smaller than the shadow reserve
/// yields the smallest rectangle inside it instead of underflowing.
pub fn container_rect(
    window_width: u32,
    window_height: u32,
    scale: f32,
    metrics: &Metrics,
) -> RectI {
    let scale = normalize_scale(scale);
    let margin = physical_dimension(whole_dp(metrics.shadow_margin), scale);
    let x = margin.min(window_width.saturating_sub(1));
    let y = margin.min(window_height.saturating_sub(1));
    RectI {
        x: x as i32,
        y: y as i32,
        w: window_width.saturating_sub(margin.saturating_mul(2)).max(1),
        h: window_height
            .saturating_sub(margin.saturating_mul(2))
            .max(1),
    }
}

/// Width the candidate cells should use, and whether the text has to be elided.
///
/// Three limits meet here. Every cell is at least [`Metrics::cell_min_width`] wide; a cell
/// stops growing once its text reaches [`Metrics::max_text_width`] (3.1.3); and the row
/// still has to fit the width the screen allows, which is what
/// [`Metrics::cell_chrome_width`] and the grid gaps are subtracted for. When the grid
/// budget is the binding limit, the caller may relax it by lowering `max_per_row`, which
/// is exactly what 3.1.3 prescribes for a very narrow screen.
///
/// # Parameters
///
/// * `natural_widths` -- the cells' unconstrained widths in logical pixels, measured by
///   the adapter; an empty slice means "no candidate", which yields the minimum cell.
/// * `max_per_row` -- configured candidates per row, clamped into the range 3.1.1 allows.
/// * `max_container_width` -- widest panel the screen allows.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// The shared cell width and whether it cuts any candidate's text short.
///
/// # Panics
///
/// Never panics: the column count is clamped to at least one before it is divided by, and
/// a non-finite measurement is ignored by `f32::max`.
pub fn cell_width(
    natural_widths: &[f32],
    max_per_row: u8,
    max_container_width: f32,
    metrics: &Metrics,
) -> CellWidth {
    let columns = f32::from(per_row(max_per_row, metrics));
    let chrome = 2.0 * metrics.container_padding + (columns - 1.0) * metrics.grid_gap;
    let budget = (max_container_width - chrome) / columns;
    let text_cap = metrics.max_text_width + metrics.cell_chrome_width;
    let cap = budget.min(text_cap).max(metrics.cell_min_width);
    let natural = natural_widths
        .iter()
        .copied()
        .fold(metrics.cell_min_width, f32::max);
    let width = natural.min(cap).max(metrics.cell_min_width);
    // Exact comparison on purpose: equal widths mean the text fits, and the flag is a
    // "must elide" decision rather than a measurement.
    CellWidth {
        width,
        truncated: natural > width,
    }
}

/// Rows, columns and paging for the candidates of one page.
///
/// # Parameters
///
/// * `item_count` -- candidates on the page being shown, which is what decides the
///   window's height; a page that is not full therefore gets a shorter window.
/// * `total_pages` -- pages the candidate list needs, from `PageState::total`; capped at
///   the five-page display limit and reported through [`GridLayout::overflow`].
/// * `max_per_row` -- configured candidates per row, clamped into the range 3.1.1 allows.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// The grid geometry, with `pages` never below `1` so that the status strip always has a
/// page to show.
///
/// # Panics
///
/// Never panics: the column count is clamped to at least one, so the row count is never a
/// division by zero.
pub fn grid(
    item_count: usize,
    total_pages: usize,
    max_per_row: u8,
    metrics: &Metrics,
) -> GridLayout {
    let columns = usize::from(per_row(max_per_row, metrics));
    let shown = item_count.min(usize::from(u8::MAX));
    let limit = usize::from(metrics.max_pages);
    GridLayout {
        cols: to_u8(shown.min(columns)),
        rows: to_u8(shown.div_ceil(columns)),
        pages: to_u8(total_pages.min(limit).max(1)),
        overflow: total_pages > limit,
    }
}

/// Logical size of the panel the current page needs.
///
/// # Parameters
///
/// * `grid` -- the geometry from [`grid`].
/// * `cell_width` -- the shared cell width from [`cell_width`].
/// * `max_container_width` -- widest panel the screen allows; a screen narrower than
///   [`Metrics::min_width`] wins over the minimum, which is 3.1.3's extreme-small-screen
///   rule.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// The panel size in logical pixels, including the padding around the candidate area and
/// the compressed header of a candidate-less window.
///
/// # Panics
///
/// Never panics: the clamp's lower bound is derived from its upper bound, so it can never
/// exceed it.
pub fn container_size(
    grid: &GridLayout,
    cell_width: f32,
    max_container_width: f32,
    metrics: &Metrics,
) -> ContainerSize {
    let columns = f32::from(grid.cols);
    let rows = f32::from(grid.rows);
    let cells = columns * cell_width + (columns - 1.0).max(0.0) * metrics.grid_gap;
    let cap = max_container_width.max(1.0);
    let floor = metrics.min_width.min(cap);
    let header = if grid.rows == 0 {
        metrics.header_height_compact
    } else {
        metrics.header_height
    };
    let body = if grid.rows == 0 {
        0.0
    } else {
        rows * metrics.cell_height + (rows - 1.0) * metrics.grid_gap
    };
    ContainerSize {
        width: (cells + 2.0 * metrics.container_padding).clamp(floor, cap),
        height: header + metrics.separator_height + 2.0 * metrics.container_padding + body,
    }
}

/// The one-pass form the adapter calls: sizes the panel, then stretches the cells into it.
///
/// The container's width and the cell width depend on each other: the cells decide how wide
/// the panel wants to be, and the panel's minimum width decides how much room the cells have
/// to share. Sizing the panel from stretched cells would never terminate, and stretching
/// before the minimum-width floor is applied would leave the floor's own void unfilled --
/// the panel would read as "not full" precisely on the pages that lifted it to
/// [`Metrics::min_width`]. The cycle is therefore broken in one direction only: the panel is
/// sized from the *natural* cells, the floor is applied, and the cells are then stretched to
/// share whatever the settled panel leaves, up to the 3.1.3 text limit plus the cell's own
/// chrome. What the stretch cannot absorb (a page wider than the text cap allows) is centred
/// by the component, which is why the width this returns is the one the highlight box and
/// the hit map consume too.
///
/// # Parameters
///
/// * `natural_widths` -- the cells' unconstrained widths in logical pixels, measured by
///   the adapter; an empty slice means "no candidate", which yields the minimum cell.
/// * `on_row` -- the column count the natural budget protects: the cells the widest row
///   actually holds, so a page shorter than a row spends the room its empty slots leave.
/// * `grid` -- the geometry from [`grid`], whose `cols` is also the denominator the stretch
///   shares the settled panel across.
/// * `max_container_width` -- widest panel the screen allows.
/// * `metrics` -- the constants from [`fn@metrics`].
///
/// # Returns
///
/// The panel size and the cell width that fills it, so the caller cannot use one without
/// the other.
///
/// # Panics
///
/// Never panics: the column counts are clamped to at least one before anything is divided
/// by them, and a non-finite measurement is ignored by `f32::max`.
pub fn panel_and_cells(
    natural_widths: &[f32],
    on_row: u8,
    grid: &GridLayout,
    max_container_width: f32,
    metrics: &Metrics,
) -> (ContainerSize, CellWidth) {
    let natural = cell_width(natural_widths, on_row, max_container_width, metrics);
    let container = container_size(grid, natural.width, max_container_width, metrics);
    // An empty page draws no row, so there is nothing to stretch and the width stays the
    // minimum the grid would use had it a cell: a phantom stretch would only churn the
    // property the next real page has to write back.
    if grid.cols == 0 {
        return (container, natural);
    }
    // The widest text as it was measured, before any cap: the elide flag has to be judged
    // against the width the cell finally draws at, and `cell_width`'s own answer carries
    // its already-capped width, from which the measurement can no longer be recovered.
    let measured = natural_widths
        .iter()
        .copied()
        .fold(metrics.cell_min_width, f32::max);
    let columns = f32::from(grid.cols.max(1));
    let chrome = 2.0 * metrics.container_padding + (columns - 1.0) * metrics.grid_gap;
    // The share one cell has of the settled panel. Only the per-cell content limit caps it:
    // the row budget the other cap comes from was already paid when the panel was sized,
    // and re-subtracting it here would shrink the cells below the width the panel is
    // already committed to.
    let text_cap = metrics.max_text_width + metrics.cell_chrome_width;
    let share = ((container.width - chrome) / columns).clamp(0.0, text_cap);
    let width = natural
        .width
        .max(share)
        .min(text_cap)
        .max(metrics.cell_min_width);
    let stretched = CellWidth {
        width,
        truncated: measured > width,
    };
    (container, stretched)
}

/// Candidates per row, clamped into the range 3.1.1 allows and never zero.
///
/// `max_per_row` comes from configuration, so a value outside the range is corrected here
/// rather than rejected: a grid with the wrong number of columns is usable, a division by
/// zero is not. The bounds are applied with `max`/`min` instead of `clamp` so that a
/// corrupted pair of bounds cannot panic.
fn per_row(max_per_row: u8, metrics: &Metrics) -> u8 {
    max_per_row
        .max(metrics.min_per_row)
        .min(metrics.max_per_row_limit)
        .max(1)
}

/// Narrows a count to `u8`, saturating.
fn to_u8(value: usize) -> u8 {
    value.min(usize::from(u8::MAX)) as u8
}

/// Rounds a logical size to whole logical pixels, refusing values that cannot describe one.
fn whole_dp(dp: f32) -> u32 {
    if dp.is_finite() && dp > 0.0 {
        dp as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The metrics of the real `candidate.slint`, which every geometry test builds on.
    fn parsed() -> Metrics {
        *metrics().expect("ui/candidate.slint declares a readable metrics block")
    }

    /// Natural cell widths for `count` candidates of the same measured width.
    fn uniform_widths(count: usize, width: f32) -> Vec<f32> {
        vec![width; count]
    }

    /// The panel the given page of candidates needs, in one call.
    fn panel(item_count: usize, total_pages: usize, per_row: u8, measured: f32) -> ContainerSize {
        let metrics = parsed();
        let grid = grid(item_count, total_pages, per_row, &metrics);
        let cell = cell_width(
            &uniform_widths(item_count, measured),
            per_row,
            720.0,
            &metrics,
        );
        container_size(&grid, cell.width, metrics.max_width, &metrics)
    }

    #[test]
    fn test_grid_one_candidate_is_a_single_cell() {
        let grid = grid(1, 1, 5, &parsed());
        assert_eq!(grid.cols, 1);
        assert_eq!(grid.rows, 1);
        assert_eq!(grid.pages, 1);
        assert!(!grid.overflow);
    }

    #[test]
    fn test_grid_nine_candidates_wrap_into_two_rows_of_five() {
        let grid = grid(9, 1, 5, &parsed());
        assert_eq!(grid.cols, 5);
        assert_eq!(grid.rows, 2);
    }

    #[test]
    fn test_grid_forty_five_candidates_across_five_pages_is_not_overflowing() {
        let grid = grid(9, 5, 5, &parsed());
        assert_eq!(grid.cols, 5);
        assert_eq!(grid.rows, 2);
        assert_eq!(grid.pages, 5);
        assert!(!grid.overflow);
    }

    #[test]
    fn test_grid_more_than_five_pages_is_capped_and_flagged() {
        let grid = grid(5, 9, 5, &parsed());
        assert_eq!(grid.pages, 5);
        assert!(grid.overflow);
    }

    #[test]
    fn test_grid_empty_candidate_list_occupies_no_row() {
        let grid = grid(0, 1, 5, &parsed());
        assert_eq!(grid.cols, 0);
        assert_eq!(grid.rows, 0);
        assert_eq!(grid.pages, 1);
    }

    #[test]
    fn test_grid_max_per_row_is_clamped_into_the_configured_range() {
        let metrics = parsed();
        assert_eq!(grid(20, 1, 0, &metrics).cols, 3);
        assert_eq!(grid(20, 1, 12, &metrics).cols, 9);
    }

    #[test]
    fn test_cell_width_average_candidate_stays_at_its_natural_width() {
        let measured = cell_width(&uniform_widths(9, 80.0), 5, 720.0, &parsed());
        assert_eq!(measured.width, 80.0);
        assert!(!measured.truncated);
    }

    #[test]
    fn test_cell_width_very_long_candidate_is_capped_and_flagged() {
        let measured = cell_width(&[600.0], 5, 720.0, &parsed());
        // The row budget binds before the 120dp text limit does: five columns have to fit
        // in 720dp, so a cell stops at (720 - 2x8 - 4x6) / 5.
        assert_eq!(measured.width, 136.0);
        assert!(measured.truncated);
    }

    #[test]
    fn test_cell_width_never_grows_past_the_text_limit() {
        // Three columns, so the grid budget is wide and the 120dp text limit is what stops
        // the cell: 120dp of text plus the 36dp of chrome around it.
        let measured = cell_width(&[600.0], 3, 720.0, &parsed());
        assert_eq!(measured.width, 156.0);
        assert!(measured.truncated);
    }

    #[test]
    fn test_cell_width_empty_measurement_falls_back_to_the_minimum() {
        let measured = cell_width(&[], 5, 720.0, &parsed());
        assert_eq!(measured.width, 64.0);
        assert!(!measured.truncated);
    }

    #[test]
    fn test_cell_width_narrow_screen_keeps_the_minimum_cell() {
        // 3.1.3: below 220dp the window follows the screen and drops to three columns.
        let measured = cell_width(&[600.0], 3, 204.0, &parsed());
        assert_eq!(measured.width, 64.0);
        assert!(measured.truncated);
    }

    /// The one-pass form the adapter calls, with the same arguments it passes.
    fn panel_and(
        item_count: usize,
        per_row: u8,
        measured: f32,
        max_container_width: f32,
    ) -> (ContainerSize, CellWidth) {
        let metrics = parsed();
        let grid = grid(item_count, 1, per_row, &metrics);
        let on_row = item_count
            .min(usize::from(u8::MAX))
            .min(usize::from(per_row))
            .max(1);
        let on_row = u8::try_from(on_row).unwrap_or(u8::MAX);
        panel_and_cells(
            &uniform_widths(item_count, measured),
            on_row,
            &grid,
            max_container_width,
            &metrics,
        )
    }

    #[test]
    fn test_panel_and_cells_stretches_a_short_page_to_fill_the_minimum_width_floor() {
        let (container, cell) = panel_and(1, 5, 64.0, 720.0);
        // The floor lifts the single 64dp cell's panel to the 220dp minimum; the stretch
        // then shares that panel across the row: (220 - 2x8 - 0) / 1 = 204, capped at the
        // 156dp the 120dp text limit plus its 36dp chrome allows. What the cap cannot
        // absorb -- 220 - 16 - 156 = 48dp -- is what the component centres.
        assert_eq!(container.width, 220.0);
        assert_eq!(cell.width, 156.0);
        // The stretch never elides: a wider cell has more room than the text needs.
        assert!(!cell.truncated);
    }

    #[test]
    fn test_panel_and_cells_leaves_a_full_natural_row_unchanged() {
        let (container, cell) = panel_and(9, 5, 80.0, 720.0);
        // Nine candidates size the panel from their own width: 5 x 80 + 4 x 6 gaps + 2 x 8
        // padding = 440dp, above the floor, so the share one cell has of it --
        // (440 - 16 - 24) / 5 = 80dp -- is exactly the natural width and the stretch is a
        // no-op. The floor is what makes the stretch engage, and here it did not.
        assert_eq!(container.width, 440.0);
        assert_eq!(cell.width, 80.0);
        assert!(!cell.truncated);
    }

    #[test]
    fn test_panel_and_cells_keeps_the_elide_flag_of_a_capped_page() {
        let (_, cell) = panel_and(9, 5, 200.0, 720.0);
        // The natural pass caps the 200dp text at the row budget (720 - 16 - 24) / 5 =
        // 136dp; the panel is then exactly full, the share is that same 136dp, and the
        // elide flag is computed against the stretched width, which is the width the cell
        // really draws at -- still shorter than the text, so still elided.
        assert_eq!(cell.width, 136.0);
        assert!(cell.truncated);
    }

    #[test]
    fn test_panel_and_cells_of_an_empty_page_keeps_the_minimum_cell() {
        let (container, cell) = panel_and(0, 5, 80.0, 720.0);
        // No cells, nothing to stretch: the panel is the floor's 220dp and the width stays
        // the minimum rather than being inflated by a phantom share.
        assert_eq!(container.width, 220.0);
        assert_eq!(cell.width, 64.0);
        assert!(!cell.truncated);
    }

    #[test]
    fn test_panel_and_cells_on_a_narrow_screen_stops_at_the_text_cap() {
        let (container, cell) = panel_and(1, 3, 64.0, 180.0);
        // 3.1.3: the window follows the 180dp screen, the floor is min(220, 180) = 180dp,
        // and the share (180 - 16) / 1 = 164dp is capped at the 156dp text limit -- a cell
        // wider than its text plus chrome would be mostly empty padding.
        assert_eq!(container.width, 180.0);
        assert_eq!(cell.width, 156.0);
        assert!(!cell.truncated);
    }

    #[test]
    fn test_container_size_single_candidate_uses_the_minimum_width() {
        let container = panel(1, 1, 5, 80.0);
        assert_eq!(container.width, 220.0);
        // 34dp header + 1dp rule + 2x8dp padding + one 36dp row.
        assert_eq!(container.height, 87.0);
    }

    #[test]
    fn test_container_size_nine_candidates_span_two_rows() {
        let container = panel(9, 1, 5, 80.0);
        // 5 x 80dp cells + 4 x 6dp gaps + 2 x 8dp padding.
        assert_eq!(container.width, 440.0);
        // 34dp header + 1dp rule + 2x8dp padding + 2 x 36dp rows + 6dp row gap.
        assert_eq!(container.height, 129.0);
    }

    #[test]
    fn test_container_size_forty_five_candidates_keeps_a_page_sized_window() {
        let container = panel(9, 5, 5, 80.0);
        assert_eq!(container.width, 440.0);
        assert_eq!(container.height, 129.0);
    }

    #[test]
    fn test_container_size_empty_candidate_list_shows_only_the_header() {
        let container = panel(0, 1, 5, 80.0);
        assert_eq!(container.width, 220.0);
        // The compressed 28dp header, the rule and the padding, with no candidate row.
        assert_eq!(container.height, 45.0);
    }

    #[test]
    fn test_container_size_long_candidates_stay_within_the_maximum_width() {
        let container = panel(5, 1, 5, 600.0);
        assert_eq!(container.width, 720.0);
        assert_eq!(container.height, 87.0);
    }

    #[test]
    fn test_container_size_screen_narrower_than_the_minimum_follows_the_screen() {
        let metrics = parsed();
        let grid = grid(3, 1, 3, &metrics);
        let cell = cell_width(&uniform_widths(3, 80.0), 3, 204.0, &metrics);
        let container = container_size(&grid, cell.width, 204.0, &metrics);
        assert_eq!(container.width, 204.0);
    }

    #[test]
    fn test_container_size_leaves_the_candidate_area_exactly_the_grid_it_needs() {
        // `candidate.slint` sizes the candidate area as `container-height - header-height -
        // separator-height`, so the panel's height has to be the header, the rule and the grid
        // and nothing else. A padding or a margin added here would shorten the area the cells
        // are drawn in, and the grid would be clipped rather than the panel growing to hold
        // it -- which is the failure the component's subtraction cannot report on its own.
        let metrics = parsed();
        for (count, per_row) in [(0usize, 5u8), (1, 5), (5, 5), (9, 5), (20, 9)] {
            let grid = grid(count, 1, per_row, &metrics);
            let cell = cell_width(&uniform_widths(count, 80.0), per_row, 720.0, &metrics);
            let container = container_size(&grid, cell.width, metrics.max_width, &metrics);
            let header = if grid.rows == 0 {
                metrics.header_height_compact
            } else {
                metrics.header_height
            };
            let rows = f32::from(grid.rows);
            let gaps = (rows - 1.0).max(0.0) * metrics.grid_gap;
            let area = container.height - header - metrics.separator_height;
            let expected = 2.0 * metrics.container_padding + rows * metrics.cell_height + gaps;
            assert_eq!(
                area, expected,
                "{count} candidates in rows of {per_row} leave the grid its own height"
            );
        }
    }

    #[test]
    fn test_window_size_includes_the_shadow_reserve() {
        let metrics = parsed();
        assert_eq!(window_size(220.0, 87.0, 1.0, &metrics), (284, 151));
    }

    #[test]
    fn test_window_size_doubles_with_the_scale_factor() {
        let metrics = parsed();
        let single = window_size(220.0, 87.0, 1.0, &metrics);
        let double = window_size(220.0, 87.0, 2.0, &metrics);
        assert_eq!(double, (single.0 * 2, single.1 * 2));
    }

    #[test]
    fn test_window_size_unusable_input_collapses_to_one_pixel() {
        let metrics = parsed();
        assert_eq!(window_size(f32::NAN, f32::NAN, 0.0, &metrics), (1, 1));
    }

    #[test]
    fn test_window_size_unusable_scale_is_treated_as_one() {
        let metrics = parsed();
        assert_eq!(window_size(220.0, 87.0, f32::NAN, &metrics), (284, 151));
    }

    #[test]
    fn test_container_rect_insets_the_panel_by_the_shadow_margin() {
        let rect = container_rect(284, 151, 1.0, &parsed());
        let expected = RectI {
            x: 32,
            y: 32,
            w: 220,
            h: 87,
        };
        assert_eq!(rect, expected);
    }

    #[test]
    fn test_container_rect_scales_with_the_surface() {
        let rect = container_rect(568, 302, 2.0, &parsed());
        let expected = RectI {
            x: 64,
            y: 64,
            w: 440,
            h: 174,
        };
        assert_eq!(rect, expected);
    }

    #[test]
    fn test_container_rect_surface_smaller_than_the_reserve_stays_inside() {
        let rect = container_rect(4, 4, 1.0, &parsed());
        assert!(rect.x < 4 && rect.y < 4);
        assert_eq!((rect.w, rect.h), (1, 1));
    }
}
