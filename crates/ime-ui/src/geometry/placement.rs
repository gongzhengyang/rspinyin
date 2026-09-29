//! The placement arithmetic: the base output, the flip decision and the clamps.
//!
//! Everything here works in physical pixels and in `i64`, and everything here is private
//! to the geometry module: [`super::compute`] is the only caller. The split is by
//! responsibility rather than by layer -- this file owns *where the window goes*, and the
//! parent owns the public request and result types plus the hit map that is derived from
//! the answer.

use ime_types::{Anchor, Placement, RectI};

use crate::layout::Metrics;
use crate::platform::physical_dimension;

use super::{Screen, even_up, to_i32, to_u32};

/// The metrics in physical pixels, for one ratio.
#[derive(Clone, Copy, Debug)]
pub(super) struct Px {
    pub(super) shadow: i64,
    pub(super) padding: i64,
    pub(super) cell_w: i64,
    pub(super) cell_h: i64,
    pub(super) gap: i64,
    pub(super) margin: i64,
    header: i64,
    separator: i64,
    caret_gap: i64,
    arrow_w: i64,
    arrow_h: i64,
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
            cell_w: metric_px(cell_width, scale),
            cell_h: metric_px(metrics.cell_height, scale),
            gap: metric_px(metrics.grid_gap, scale),
            margin: metric_px(super::EDGE_MARGIN_DP, scale),
            header: metric_px(metrics.header_height, scale),
            separator: metric_px(metrics.separator_height, scale),
            caret_gap: metric_px(super::CARET_GAP_DP, scale),
            arrow_w: metric_px(metrics.cursor_arrow_width, scale),
            arrow_h: metric_px(metrics.cursor_arrow_height, scale),
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
        even_up(self.top_block() + self.grid_height(rows) + self.padding)
    }

    /// Height of a grid of `rows` rows.
    fn grid_height(&self, rows: i64) -> i64 {
        let rows = rows.max(1);
        rows * self.cell_h + (rows - 1) * self.gap
    }

    /// How many rows fit in a container of this height; never fewer than one.
    pub(super) fn rows_in(&self, container_h: i64) -> i64 {
        fit(
            self.cell_h,
            self.gap,
            container_h - self.top_block() - self.padding,
        )
    }

    /// How many columns fit in a container of this width; never fewer than one.
    pub(super) fn columns_in(&self, container_w: i64) -> i64 {
        fit(self.cell_w, self.gap, container_w - 2 * self.padding)
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
fn fit(cell: i64, gap: i64, available: i64) -> i64 {
    if available < cell {
        1
    } else {
        (available + gap) / (cell + gap)
    }
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

/// The window rectangle, in virtual-desktop physical pixels.
///
/// Only the numbers the arrow needs are carried: the placement arithmetic works on the
/// window's height separately, so a height field here would never be read.
#[derive(Clone, Copy, Debug)]
pub(super) struct Window {
    pub(super) x: i64,
    pub(super) y: i64,
    pub(super) w: i64,
}

impl Window {
    /// Builds the window rectangle from the placement the pass settled on.
    pub(super) fn of(x: i64, y: i64, w: i64) -> Self {
        Self { x, y, w }
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
    pub(super) fn vertical(&self, max_rows: i64, container_h: i64) -> (Placement, i64, i64) {
        let max_rows = max_rows.max(1);
        let mut rows = max_rows;
        loop {
            let height = if rows == max_rows {
                container_h
            } else {
                self.px.container_height(rows)
            };
            let window_h = height + 2 * self.px.shadow;
            if self.fits(window_h) || rows <= 1 {
                return (self.side(window_h), rows, height);
            }
            rows -= 1;
        }
    }

    /// Whether a window of this height has somewhere to go.
    fn fits(&self, window_h: i64) -> bool {
        let Some(bounds) = self.bounds else {
            return true;
        };
        match self.forced {
            // A pinned side is never flipped, but it still has to fit the output.
            Some(_) => window_h <= bounds.bottom - bounds.top - 2 * self.px.margin,
            None => self.below_fits(bounds, window_h) || self.above_fits(bounds, window_h),
        }
    }

    /// The side to place on, once the window height is settled.
    fn side(&self, window_h: i64) -> Placement {
        if let Some(side) = self.forced {
            return side;
        }
        let Some(bounds) = self.bounds else {
            return Placement::Below;
        };
        if self.below_fits(bounds, window_h) {
            return Placement::Below;
        }
        if self.above_fits(bounds, window_h) {
            return Placement::Above;
        }
        // Neither side has room even at one row: keep the roomier one and let the clamp
        // push the window back inside, which loses the least of the caret's neighbourhood.
        let below = bounds.bottom - self.px.margin - self.caret.bottom - self.px.caret_gap;
        let above = self.caret.top - self.px.caret_gap - bounds.top - self.px.margin;
        if below >= above {
            Placement::Below
        } else {
            Placement::Above
        }
    }

    /// Whether the window fits below the caret, entirely inside the output.
    fn below_fits(&self, bounds: Bounds, window_h: i64) -> bool {
        let y = self.caret.bottom + self.px.caret_gap;
        y >= bounds.top + self.px.margin && y + window_h <= bounds.bottom - self.px.margin
    }

    /// Whether the window fits above the caret, entirely inside the output.
    fn above_fits(&self, bounds: Bounds, window_h: i64) -> bool {
        let y = self.caret.top - self.px.caret_gap - window_h;
        y >= bounds.top + self.px.margin && y + window_h <= bounds.bottom - self.px.margin
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

    /// The window's y for a side, and whether it had to leave the ideal position.
    pub(super) fn vertical_pos(&self, side: Placement, window_h: i64) -> (i64, bool) {
        let ideal = match side {
            Placement::Above => self.caret.top - self.px.caret_gap - window_h,
            Placement::Below | Placement::Auto => self.caret.bottom + self.px.caret_gap,
        };
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
    /// It appears only under a window that sits exactly below the caret: a flipped or
    /// clamped window has no straight line back to the caret, and an arrow pointing the
    /// wrong way would be worse than none. The window is placed one gap below the caret and
    /// the arrow is one gap tall, so it fills that gap at the top of the window, centred on
    /// the caret; the horizontal bound keeps the whole arrow inside the window's width.
    pub(super) fn arrow(
        &self,
        side: Placement,
        window: Window,
        clamped_x: bool,
        clamped_y: bool,
    ) -> Option<RectI> {
        if side != Placement::Below || clamped_x || clamped_y {
            return None;
        }
        let distance = self.caret.centre_x - window.x;
        if distance < self.px.arrow_w || distance > window.w - self.px.arrow_w {
            return None;
        }
        Some(RectI {
            x: to_i32(self.caret.centre_x - self.px.arrow_w / 2),
            y: to_i32(window.y),
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
