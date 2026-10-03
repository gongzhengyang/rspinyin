//! The placement arithmetic: the base output, the flip decision and the clamps.
//!
//! Everything here works in physical pixels and in `i64`, and everything here is private
//! to the geometry module: [`super::compute`] is the only caller. The split is by
//! responsibility rather than by layer -- this file owns *where the window goes*, and the
//! parent owns the public request and result types plus the hit map that is derived from
//! the answer.
//!
//! # The window origin against the panel edge
//!
//! Two vertical extents appear below and they must not be confused. The *window* is the
//! surface the compositor positions: the panel plus a transparent shadow reserve of
//! [`Px::shadow`] on all four sides. The *panel* is the visible rectangle inside it, and the
//! design's caret gap is measured to the panel, not to the surface.
//!
//! A decision that reads `caret.bottom + caret_gap` and treats it as the window origin
//! therefore lands the panel a whole reserve too far from the caret and floats the arrow in
//! transparent space. Every position here subtracts the reserve, and every fit test measures
//! the panel: a surface that fits always leaves the panel room, while a panel that fits may
//! still need the reserve pulled back inside the output, which is the clamp's business.

use ime_types::{Anchor, Placement, RectI};

use crate::layout::Metrics;
use crate::platform::physical_dimension;

use super::{Screen, even_up_scaled, to_i32, to_u32};

/// The metrics in physical pixels, for one ratio.
#[derive(Clone, Copy, Debug)]
pub(super) struct Px {
    pub(super) shadow: i64,
    pub(super) padding: i64,
    pub(super) cell_h: i64,
    pub(super) gap: i64,
    pub(super) margin: i64,
    header: i64,
    separator: i64,
    caret_gap: i64,
    arrow_w: i64,
    arrow_h: i64,
    /// The ratio every physical extent above was converted with.
    ///
    /// Kept because the two `fit` questions below are not asked in physical pixels. The
    /// component lays its cells out in *logical* ones and the rasterizer converts once, so
    /// a grid that fits at 1.0 fits at every ratio. Counting in physical pixels instead
    /// lets the per-metric rounding inflate the stride -- at 1.25 a 6dp gap rounds to 8px
    /// against a true 7.5 -- and a row of five 64dp cells that exactly fills its container
    /// is then read as four, which puts every hit rectangle of the page on the wrong cell.
    scale: f32,
    /// The same three metrics the fit test needs, in the units the component uses.
    cell_w_dp: f32,
    cell_h_dp: f32,
    gap_dp: f32,
}

impl Px {
    /// Converts the component's constants into physical pixels.
    ///
    /// The constants come from the component's own declaration, read back by the layout
    /// module, so a size cannot drift between what is drawn and what is hit tested; the
    /// cell width is the layout pass's own choice for this page rather than the minimum,
    /// because the cells on screen are the ones the hit map has to land on. Every extent
    /// goes through the platform layer's conversion, which keeps a degenerate constant from
    /// turning into a zero-sized cell that later arithmetic would divide by.
    pub(super) fn for_scale(scale: f32, cell_width: f32, metrics: &Metrics) -> Self {
        Self {
            shadow: metric_px(metrics.shadow_margin, scale),
            padding: metric_px(metrics.container_padding, scale),
            cell_h: metric_px(metrics.cell_height, scale),
            gap: metric_px(metrics.grid_gap, scale),
            margin: metric_px(super::EDGE_MARGIN_DP, scale),
            header: metric_px(metrics.header_height, scale),
            separator: metric_px(metrics.separator_height, scale),
            caret_gap: metric_px(super::CARET_GAP_DP, scale),
            arrow_w: metric_px(metrics.cursor_arrow_width, scale),
            arrow_h: metric_px(metrics.cursor_arrow_height, scale),
            scale,
            cell_w_dp: cell_width,
            cell_h_dp: metrics.cell_height,
            gap_dp: metrics.grid_gap,
        }
    }

    /// Height of the container block above the grid: padding, header and separator.
    ///
    /// The full header is used even for a window with no candidate, where the layout pass
    /// sizes a compressed one: with no candidate there is no cell to place, so the choice
    /// cannot move a hit rectangle.
    pub(super) fn top_block(&self) -> i64 {
        self.padding + self.header + self.separator
    }

    /// Container height that shows exactly `rows` rows of candidates.
    ///
    /// Rounded up to an even number, like the height the layout pass asked for, so a window
    /// built from it is even too. The formula is the layout pass's own: header, separator,
    /// padding on both sides, and the rows with their gaps.
    pub(super) fn container_height(&self, rows: i64) -> i64 {
        even_up_scaled(
            self.top_block() + self.grid_height(rows) + self.padding,
            self.scale,
        )
    }

    /// Height of a grid of `rows` rows.
    fn grid_height(&self, rows: i64) -> i64 {
        let rows = rows.max(1);
        rows * self.cell_h + (rows - 1) * self.gap
    }

    /// How many rows fit in a container of this height; never fewer than one.
    pub(super) fn rows_in(&self, container_h: i64) -> i64 {
        fit(
            self.cell_h_dp,
            self.gap_dp,
            self.logical(container_h - self.top_block() - self.padding),
        )
    }

    /// How many columns fit in a container of this width; never fewer than one.
    pub(super) fn columns_in(&self, container_w: i64) -> i64 {
        fit(
            self.cell_w_dp,
            self.gap_dp,
            self.logical(container_w - 2 * self.padding),
        )
    }

    /// One physical extent in the logical units the component lays its cells out in.
    fn logical(&self, px: i64) -> f32 {
        if self.scale > 0.0 {
            px as f32 / self.scale
        } else {
            px as f32
        }
    }
}

/// Converts one logical metric into physical pixels.
///
/// The metrics are whole dp by design and the parser has already refused anything that is
/// not a finite, non-negative number, so this only has to handle the fraction; rounding
/// rather than truncating keeps a metric from drifting a pixel low.
fn metric_px(dp: f32, scale: f32) -> i64 {
    let whole = if dp.is_finite() && dp > 0.0 {
        dp.round()
    } else {
        0.0
    };
    i64::from(physical_dimension(whole as u32, scale))
}

/// How many cells of `cell` plus `gap` fit in `available`; never fewer than one.
///
/// All three are logical extents, which is what makes the answer the same at every ratio:
/// the component lays the grid out in logical pixels and the rasterizer converts the whole
/// layout at once, so a cell never grows relative to the container. A ratio whose
/// per-metric rounding is not exact would break that if the arithmetic were done in the
/// converted extents instead.
///
/// The float-to-integer cast saturates rather than wrapping, so an absurd container turns
/// into `i64::MAX` columns rather than a negative count; the caller's own row/column cap is
/// what bounds the page.
fn fit(cell: f32, gap: f32, available: f32) -> i64 {
    let stride = cell + gap;
    if !available.is_finite() || !stride.is_finite() || stride <= 0.0 || available < cell {
        return 1;
    }
    ((available + gap) / stride).floor().max(1.0) as i64
}

/// The caret reduced to the numbers placement reads, widened to `i64`.
#[derive(Clone, Copy, Debug)]
struct Caret {
    top: i64,
    bottom: i64,
    centre_x: i64,
}

impl Caret {
    fn of(cursor: RectI) -> Self {
        let left = i64::from(cursor.x);
        let top = i64::from(cursor.y);
        Self {
            top,
            bottom: top + i64::from(cursor.h),
            centre_x: left + i64::from(cursor.w) / 2,
        }
    }
}

/// An output's rectangle in virtual-desktop physical pixels, widened to `i64`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    pub(super) left: i64,
    pub(super) top: i64,
    pub(super) right: i64,
    pub(super) bottom: i64,
}

impl Bounds {
    /// Computes an output's bounds in `i64`.
    ///
    /// Widening matters: an origin near `i32::MIN` combined with a size near `u32::MAX`
    /// overflows both `i32` and `u32`, and an output a backend reported wrongly must not be
    /// able to wrap the arithmetic into a plausible-looking position.
    pub(super) fn of(screen: &Screen) -> Self {
        let left = i64::from(screen.origin.0);
        let top = i64::from(screen.origin.1);
        Self {
            left,
            top,
            right: left + i64::from(screen.size.0),
            bottom: top + i64::from(screen.size.1),
        }
    }

    /// The output's width in physical pixels.
    pub(super) fn width(&self) -> i64 {
        self.right - self.left
    }
}

/// The window origin, in virtual-desktop physical pixels.
///
/// Only the origin is carried: the arrow is bounded by the panel's width rather than the
/// surface's, and the placement arithmetic works on the heights separately, so neither
/// extent belongs in a struct the arrow reads.
#[derive(Clone, Copy, Debug)]
pub(super) struct Window {
    pub(super) x: i64,
    pub(super) y: i64,
}

impl Window {
    /// Builds the window origin from the placement the pass settled on.
    pub(super) fn of(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

/// One placement pass's inputs, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pass {
    px: Px,
    caret: Caret,
    bounds: Option<Bounds>,
    forced: Option<Placement>,
}

impl Pass {
    /// Prepares a pass from the anchor and the base output's bounds.
    pub(super) fn of(px: Px, anchor: &Anchor, bounds: Option<Bounds>) -> Self {
        Self {
            px,
            caret: Caret::of(anchor.cursor),
            bounds,
            forced: forced_side(anchor.placement),
        }
    }

    /// The side, the visible row count and the container height that fit the output.
    ///
    /// The row count starts at what the caller's container can show and comes down until
    /// one side of the caret has room, which is the design's rule for a screen too short
    /// for the window: show fewer rows, never fewer than one. The search is bounded by the
    /// row count the container can show, so it cannot spin.
    ///
    /// Every test in the loop is on the panel, because the panel is what the user sees and
    /// what the caret gap is measured to; the shadow reserve around it is transparent and
    /// may be pulled back into the output afterwards. Row counts are therefore only given up
    /// when the visible rectangle genuinely does not fit.
    pub(super) fn vertical(&self, max_rows: i64, container_h: i64) -> (Placement, i64, i64) {
        let max_rows = max_rows.max(1);
        // The caller's height is the one its own layout pass built for the page, so it is
        // kept whenever it is the height the rows that fit actually need -- which is what
        // makes the panel exactly as tall as the frame it draws. It is *not* kept when the
        // output is so short that the fit reduced the page to fewer rows than the caller
        // sized for: there the caller's height is the taller page's, and keeping it would
        // report a panel that does not fit as the answer to "what fits".
        let mut rows = max_rows;
        let caller_fits = self.px.container_height(rows) >= container_h;
        loop {
            let height = if rows == max_rows && caller_fits {
                container_h
            } else {
                self.px.container_height(rows)
            };
            if self.has_room_below(height) || rows <= 1 {
                return (self.side(height), rows, height);
            }
            rows -= 1;
        }
    }

    /// Whether a panel of this height has room below the caret.
    ///
    /// This, and not [`Self::fits`], is what the row reduction tests. `side` prefers
    /// `Below`, so testing "fits on either side" would flip a three-row panel above a
    /// caret that a two-row panel sits comfortably under -- the panel would be thrown to
    /// the other side of the caret rather than shortened, which is the opposite of what
    /// the design asks for. Only when even one row has no room below does the reduction
    /// stop and let [`Self::side`] flip.
    ///
    /// A pinned side is never flipped, so its panel is measured against the output instead.
    fn has_room_below(&self, panel_h: i64) -> bool {
        let Some(bounds) = self.bounds else {
            return true;
        };
        match self.forced {
            Some(_) => panel_h <= bounds.bottom - bounds.top - 2 * self.px.margin,
            None => self.below_fits(bounds, panel_h),
        }
    }

    /// The side to place on, once the panel height is settled.
    fn side(&self, panel_h: i64) -> Placement {
        if let Some(side) = self.forced {
            return side;
        }
        let Some(bounds) = self.bounds else {
            return Placement::Below;
        };
        if self.below_fits(bounds, panel_h) {
            return Placement::Below;
        }
        if self.above_fits(bounds, panel_h) {
            return Placement::Above;
        }
        // Neither side has room even at one row: keep the roomier one and let the clamp
        // push the window back inside, which loses the least of the caret's neighbourhood.
        let below = bounds.bottom - self.px.margin - self.panel_top_below();
        let above = self.panel_bottom_above() - bounds.top - self.px.margin;
        if below >= above {
            Placement::Below
        } else {
            Placement::Above
        }
    }

    /// The panel's top edge when the panel goes below the caret.
    ///
    /// This is the design's rule -- one caret gap between the caret and the panel -- written
    /// once: the placement, the fit test and the flip all read the edge from here rather than
    /// repeating the sum, which is what keeps the window origin from drifting a shadow
    /// reserve away from the panel edge it is supposed to sit behind.
    fn panel_top_below(&self) -> i64 {
        self.caret.bottom + self.px.caret_gap
    }

    /// The panel's bottom edge when the panel goes above the caret.
    fn panel_bottom_above(&self) -> i64 {
        self.caret.top - self.px.caret_gap
    }

    /// Whether the panel fits below the caret, entirely inside the output.
    ///
    /// The panel's own top and bottom edges are the ones tested -- the caret gap above it and
    /// the edge margin below it -- because those are the edges the user sees. The reserve
    /// around the surface is transparent and is dealt with by the clamp instead.
    fn below_fits(&self, bounds: Bounds, panel_h: i64) -> bool {
        let top = self.panel_top_below();
        top >= bounds.top + self.px.margin && top + panel_h <= bounds.bottom - self.px.margin
    }

    /// Whether the panel fits above the caret, entirely inside the output.
    fn above_fits(&self, bounds: Bounds, panel_h: i64) -> bool {
        let bottom = self.panel_bottom_above();
        bottom - panel_h >= bounds.top + self.px.margin && bottom <= bounds.bottom - self.px.margin
    }

    /// The window's x, and whether it had to leave the caret-centred position.
    pub(super) fn horizontal(&self, window_w: i64) -> (i64, bool) {
        let ideal = self.caret.centre_x - window_w / 2;
        let Some(bounds) = self.bounds else {
            return (ideal, false);
        };
        let low = bounds.left + self.px.margin;
        let high = (bounds.right - window_w - self.px.margin).max(low);
        let x = ideal.clamp(low, high);
        (x, x != ideal)
    }

    /// The window origin that puts the panel one caret gap away from the caret.
    ///
    /// The origin is *not* where the panel's edge goes: the surface carries the transparent
    /// reserve on all four sides, so the reserve is subtracted from the panel's edge here.
    /// Returning the panel's edge as the origin -- which is what an earlier revision did --
    /// leaves the panel a whole reserve further from the caret and the arrow floating in
    /// empty space above it.
    fn ideal_y(&self, side: Placement, window_h: i64) -> i64 {
        match side {
            Placement::Above => self.panel_bottom_above() - window_h + self.px.shadow,
            Placement::Below | Placement::Auto => self.panel_top_below() - self.px.shadow,
        }
    }

    /// The window's y for a side, and whether it had to leave the ideal position.
    ///
    /// The clamp is on the surface, not on the panel: the surface is what the compositor
    /// positions, and it has to stay inside the output. Where the surface's reserve no longer
    /// fits below the caret, that clamp pulls the panel closer to the caret than the design
    /// gap -- `clamped_y` reports exactly that, and it is what drops the arrow.
    pub(super) fn vertical_pos(&self, side: Placement, window_h: i64) -> (i64, bool) {
        let ideal = self.ideal_y(side, window_h);
        let Some(bounds) = self.bounds else {
            return (ideal, false);
        };
        let low = bounds.top + self.px.margin;
        let high = (bounds.bottom - window_h - self.px.margin).max(low);
        let y = ideal.clamp(low, high);
        (y, y != ideal)
    }

    /// The caret indicator arrow, or `None` when the design says not to draw it.
    ///
    /// It appears only when the window sits exactly below the caret: a flipped or clamped
    /// window has no straight line back to the caret, and an arrow pointing the wrong way
    /// would be worse than none. The arrow fills the gap between the caret and the panel, so
    /// its base meets the panel's top edge and its tip meets the caret; that edge is one
    /// shadow reserve below the surface's, which is why the vertical anchor adds the reserve
    /// back on.
    ///
    /// The horizontal bound is measured in panel coordinates for the same reason: the arrow
    /// has to stay inside the panel, and an offset taken from the surface's left edge would
    /// be a whole reserve off. The arrow is `arrow_w` wide and is required to keep that much
    /// clearance from either panel edge, which also keeps it clear of the panel's rounded
    /// corner.
    pub(super) fn arrow(
        &self,
        side: Placement,
        window: Window,
        panel_w: i64,
        clamped_x: bool,
        clamped_y: bool,
    ) -> Option<RectI> {
        if side != Placement::Below || clamped_x || clamped_y {
            return None;
        }
        let centre = self.caret.centre_x - window.x - self.px.shadow;
        if centre < self.px.arrow_w || centre > panel_w - self.px.arrow_w {
            return None;
        }
        Some(RectI {
            x: to_i32(self.caret.centre_x - self.px.arrow_w / 2),
            y: to_i32(window.y + self.px.shadow - self.px.arrow_h),
            w: to_u32(self.px.arrow_w),
            h: to_u32(self.px.arrow_h),
        })
    }
}

/// The side a caller pinned the window to, or `None` when the pass may flip it.
fn forced_side(placement: Placement) -> Option<Placement> {
    match placement {
        Placement::Auto => None,
        side => Some(side),
    }
}
