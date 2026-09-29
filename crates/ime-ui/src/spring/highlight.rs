//! The floating highlight box and the damage it leaves behind.
//!
//! The highlight is not the background of a candidate cell: it is one rectangle
//! floating above the grid, driven by four independent springs, so that a cross-row
//! move animates `y` and `h` at the same time as `x`. The box therefore flies and
//! reshapes rather than sliding, and mid-flight it overlaps two cells -- which is
//! exactly what the motion is meant to look like.
//!
//! This module also owns the *damage* the motion produces. Redrawing the whole
//! window on every step of a 181ms slide would eat the raster budget several times
//! over, so a step reports the union of where the box was and where it is now, and
//! nothing else.

use ime_types::RectI;

use super::{Spring1D, SpringParams};

/// The largest coordinate an animated rectangle may carry, in dp.
///
/// The rectangle comes from candidate geometry, so a value past this is a caller
/// bug rather than a real layout. Clamping keeps the outward rounding below inside
/// the integer rectangle type, and it sits far outside any display.
const MAX_COORD_DP: f32 = 1.0e6;

/// A rectangle in device-independent pixels, with fractional components.
///
/// The springs integrate in dp because that is the unit the `.slint` properties and
/// the configured layout are expressed in; the conversion to physical pixels happens
/// once, in [`HighlightRect::bounds`], at the point where a damage region is needed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightRect {
    /// Left edge, in dp.
    pub x: f32,
    /// Top edge, in dp.
    pub y: f32,
    /// Width, in dp.
    pub w: f32,
    /// Height, in dp.
    pub h: f32,
}

impl HighlightRect {
    /// An empty rectangle at the origin: the state of a window with no candidates.
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    /// Builds a rectangle from its four components.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether every component is a finite number.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }

    /// The physical-pixel rectangle that covers this one.
    ///
    /// Both far edges are rounded outward, so the result always contains the whole
    /// box even when it sits on a fractional pixel; a damage region that rounded
    /// inward would leave a one-pixel smear behind the moving edge. A scale that
    /// cannot be used is replaced by `1.0` rather than propagated, on the same
    /// reasoning as the platform layer's own scale correction.
    pub fn bounds(self, scale: f32) -> RectI {
        let scale = usable_scale(scale);
        let x = clamp_coord(self.x);
        let y = clamp_coord(self.y);
        let w = clamp_coord(self.w).max(0.0);
        let h = clamp_coord(self.h).max(0.0);
        let x0 = clamp_coord((x * scale).floor());
        let y0 = clamp_coord((y * scale).floor());
        let x1 = clamp_coord(((x + w) * scale).ceil()).max(x0);
        let y1 = clamp_coord(((y + h) * scale).ceil()).max(y0);
        RectI {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        }
    }
}

/// The smallest physical-pixel rectangle covering both inputs.
///
/// The arithmetic is widened to `i64` before the edges are added, because a
/// rectangle may carry `i32::MIN` for its origin and would overflow when offset by
/// its width; the result is then saturated back into the unsigned width field rather
/// than truncated, so a nonsensical input produces a nonsensical rectangle instead of
/// a small one.
pub fn union_rects(a: RectI, b: RectI) -> RectI {
    let a_right = i64::from(a.x) + i64::from(a.w);
    let a_bottom = i64::from(a.y) + i64::from(a.h);
    let b_right = i64::from(b.x) + i64::from(b.w);
    let b_bottom = i64::from(b.y) + i64::from(b.h);
    let x0 = i64::from(a.x).min(i64::from(b.x));
    let y0 = i64::from(a.y).min(i64::from(b.y));
    let x1 = a_right.max(b_right);
    let y1 = a_bottom.max(b_bottom);
    let widest = i64::from(u32::MAX);
    RectI {
        x: x0 as i32,
        y: y0 as i32,
        w: (x1 - x0).clamp(0, widest) as u32,
        h: (y1 - y0).clamp(0, widest) as u32,
    }
}

/// What one step of the highlight produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightStep {
    /// The rectangle to draw this frame, in dp; this is what the `.slint` properties
    /// take.
    pub rect: HighlightRect,
    /// The region that must be redrawn, in physical pixels. `None` when the box did
    /// not move, so the previous frame's pixels still stand and there is nothing to
    /// do.
    pub damage: Option<RectI>,
    /// Whether all four springs are at rest after this step.
    pub settled: bool,
}

/// The floating highlight box: four springs, plus the visibility that gates them.
///
/// The four degrees of freedom are independent on purpose. A cross-row move changes
/// `y` and `h` together, and a page turn can change all four; driving them from one
/// interpolated scalar would force them to move in lockstep and lose the "fly and
/// reshape" character.
#[derive(Clone, Debug)]
pub struct HighlightAnim {
    x: Spring1D,
    y: Spring1D,
    w: Spring1D,
    h: Spring1D,
    visible: bool,
    /// The bounds drawn last frame, so the next step can report the union. `None`
    /// until the box has been drawn once.
    drawn: Option<RectI>,
}

impl HighlightAnim {
    /// Creates a box at rest on `rect`.
    ///
    /// The box starts hidden: a window that has not been shown yet has no highlight
    /// to draw, and starting hidden means the first appearance cannot fly in from
    /// wherever the previous session left the box.
    pub fn new(params: SpringParams, rect: HighlightRect) -> Self {
        let (omega0, zeta, mass) = (params.omega0, params.zeta, params.mass);
        Self {
            x: Spring1D::new(omega0, zeta, mass, rect.x),
            y: Spring1D::new(omega0, zeta, mass, rect.y),
            w: Spring1D::new(omega0, zeta, mass, rect.w),
            h: Spring1D::new(omega0, zeta, mass, rect.h),
            visible: false,
            drawn: None,
        }
    }

    /// Moves the box to `rect`, preserving the velocity of a box already in flight.
    ///
    /// This is the behaviour 3.3.1 calls out as the reason a spring is used instead
    /// of a bezier: a second arrow key pressed while the box is still travelling
    /// bends its path toward the new cell, because the velocity it already had is
    /// kept. A non-finite rectangle is ignored rather than allowed to poison the
    /// integrator.
    pub fn retarget(&mut self, rect: HighlightRect) {
        if !rect.is_finite() {
            return;
        }
        self.x.retarget(rect.x);
        self.y.retarget(rect.y);
        self.w.retarget(rect.w);
        self.h.retarget(rect.h);
        if !self.visible {
            // Nothing is on screen to fly. Leaving the springs mid-flight would make
            // the next appearance start from the previous cell and drift in.
            self.snap();
        }
    }

    /// Jumps the box onto its target and stops it.
    pub fn snap(&mut self) {
        self.x.snap();
        self.y.snap();
        self.w.snap();
        self.h.snap();
    }

    /// Jumps the box to `rect` without animating.
    pub fn snap_to(&mut self, rect: HighlightRect) {
        self.retarget(rect);
        self.snap();
    }

    /// Sets whether the box is drawn.
    ///
    /// Hiding snaps the box onto its target: the window is fading out anyway, so
    /// there is nothing left to animate, and the next appearance should begin where
    /// the highlight belongs rather than fly in from where it was left.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            self.snap();
            self.drawn = None;
        }
    }

    /// Whether the box is drawn.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The box as it should be drawn this frame, in dp.
    pub fn rect(&self) -> HighlightRect {
        HighlightRect::new(self.x.x, self.y.x, self.w.x.max(0.0), self.h.x.max(0.0))
    }

    /// The box's resting place, in dp.
    pub fn target(&self) -> HighlightRect {
        HighlightRect::new(
            self.x.target(),
            self.y.target(),
            self.w.target().max(0.0),
            self.h.target().max(0.0),
        )
    }

    /// Whether all four springs are at rest.
    pub fn is_settled(&self) -> bool {
        self.x.is_settled() && self.y.is_settled() && self.w.is_settled() && self.h.is_settled()
    }

    /// Advances the box by `dt` and reports what has to be redrawn.
    ///
    /// All four springs advance on every frame even when the first of them is
    /// already at rest: short-circuiting the sequence would freeze whichever
    /// components came after it, which is a box that slides horizontally while its
    /// height stays stuck at the previous cell's.
    pub fn step(&mut self, dt: f32, scale: f32) -> HighlightStep {
        if !self.visible {
            // A hidden box neither moves nor leaves a trail; the window-level fade
            // owns the pixels it used to occupy.
            self.drawn = None;
            return HighlightStep {
                rect: self.rect(),
                damage: None,
                settled: true,
            };
        }
        let settled_x = self.x.step(dt);
        let settled_y = self.y.step(dt);
        let settled_w = self.w.step(dt);
        let settled_h = self.h.step(dt);
        let rect = self.rect();
        let bounds = rect.bounds(scale);
        // Damage is the union of where the box was and where it is now: anything
        // smaller leaves a smear along the trailing edge, anything larger spends
        // raster time on pixels that did not change.
        let damage = if self.drawn == Some(bounds) {
            None
        } else {
            Some(match self.drawn {
                Some(previous) => union_rects(previous, bounds),
                None => bounds,
            })
        };
        self.drawn = Some(bounds);
        HighlightStep {
            rect,
            damage,
            settled: settled_x && settled_y && settled_w && settled_h,
        }
    }
}

/// Replaces a scale factor that cannot be used.
fn usable_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Clamps one coordinate into the range the integer rectangle type can hold.
fn clamp_coord(dp: f32) -> f32 {
    if dp.is_finite() {
        dp.clamp(-MAX_COORD_DP, MAX_COORD_DP)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use crate::spring::{DEFAULT_MASS, HIGHLIGHT_OMEGA0, HIGHLIGHT_ZETA};
    use super::*;

    const FRAME_S: f32 = 1.0 / 144.0;

    fn highlight_params() -> SpringParams {
        SpringParams::new(HIGHLIGHT_OMEGA0, HIGHLIGHT_ZETA, DEFAULT_MASS)
    }

    /// A candidate cell of the size the grid produces: 88dp wide, 30dp tall.
    fn cell(column: f32, row: f32) -> HighlightRect {
        HighlightRect::new(column * 88.0, row * 30.0, 88.0, 30.0)
    }

    /// Steps until the box is at rest, with a ceiling so a non-settling spring fails
    /// the test rather than hanging it.
    fn settle(anim: &mut HighlightAnim, scale: f32) -> HighlightStep {
        let mut step = anim.step(FRAME_S, scale);
        let mut frames = 0u32;
        while !step.settled {
            frames += 1;
            assert!(frames < 1_000, "the highlight must settle");
            step = anim.step(FRAME_S, scale);
        }
        step
    }

    #[test]
    fn test_highlight_retarget_preserves_velocity_in_flight() {
        let mut anim = HighlightAnim::new(highlight_params(), cell(0.0, 0.0));
        anim.set_visible(true);
        anim.retarget(cell(1.0, 0.0));
        for _ in 0..6 {
            anim.step(FRAME_S, 1.0);
        }
        let velocity = anim.x.v;
        assert!(velocity > 0.0, "the box is travelling toward the new cell");

        // The user presses the key again while the box is still flying.
        anim.retarget(cell(2.0, 0.0));
        assert_eq!(
            anim.x.v, velocity,
            "a redirect must keep the velocity it already had"
        );
        assert!(!anim.is_settled());
    }

    #[test]
    fn test_highlight_settles_on_target_from_both_sides() {
        for (from, to) in [(cell(0.0, 0.0), cell(3.0, 0.0)), (cell(3.0, 0.0), cell(0.0, 0.0))] {
            let mut anim = HighlightAnim::new(highlight_params(), from);
            anim.set_visible(true);
            anim.retarget(to);
            let step = settle(&mut anim, 1.0);
            assert!(step.settled);
            assert_eq!(anim.rect(), to, "rests exactly on the cell");
        }
    }

    #[test]
    fn test_highlight_step_reports_damage_covering_old_and_new_bounds() {
        let mut anim = HighlightAnim::new(highlight_params(), cell(0.0, 0.0));
        anim.set_visible(true);
        anim.retarget(cell(0.0, 1.0));
        let mut previous = anim.rect();
        let mut widest = 0u64;
        let mut frame = 0u32;
        loop {
            let step = anim.step(FRAME_S, 1.0);
            frame += 1;
            assert!(frame < 1_000, "the highlight must settle");
            // A frame whose sub-pixel movement stays inside the same integer bounds
            // reports no damage at all, which is the point of rounding outward: the
            // caller gets `None` rather than a one-pixel repaint.
            if let Some(damage) = step.damage {
                let union = union_rects(previous.bounds(1.0), step.rect.bounds(1.0));
                let damaged_area = u64::from(damage.w) * u64::from(damage.h);
                let union_area = u64::from(union.w) * u64::from(union.h);
                assert!(
                    damaged_area as f64 <= union_area as f64 * 1.2,
                    "damage {damaged_area}px^2 exceeds the union {union_area}px^2 by over 20%"
                );
                widest = widest.max(damaged_area);
            }
            previous = step.rect;
            if step.settled {
                break;
            }
        }
        assert!(widest > 0, "a box that moved must have been damaged");
        // One cell tall plus one cell of travel, not the whole 600x140dp window.
        assert!(
            widest <= 88 * 62,
            "the worst damage was {widest}px^2, which is more than the box's path"
        );
    }

    #[test]
    fn test_highlight_settled_box_reports_no_damage() {
        let mut anim = HighlightAnim::new(highlight_params(), cell(0.0, 0.0));
        anim.set_visible(true);
        anim.retarget(cell(1.0, 0.0));
        settle(&mut anim, 1.0);
        let idle = anim.step(FRAME_S, 1.0);
        assert_eq!(idle.damage, None, "a resting box redraws nothing");
        assert!(idle.settled);
    }

    #[test]
    fn test_highlight_hidden_box_neither_moves_nor_damages() {
        let mut anim = HighlightAnim::new(highlight_params(), cell(0.0, 0.0));
        let step = anim.step(FRAME_S, 1.0);
        assert_eq!(step.damage, None);
        assert!(step.settled, "a hidden box is never animating");

        // While hidden a redirect is a jump, so the next appearance cannot drift in
        // from the cell that was highlighted last time.
        anim.retarget(cell(4.0, 2.0));
        assert_eq!(anim.rect(), cell(4.0, 2.0));
    }

    #[test]
    fn test_highlight_retarget_ignores_non_finite_rect() {
        let mut anim = HighlightAnim::new(highlight_params(), cell(0.0, 0.0));
        anim.set_visible(true);
        anim.retarget(cell(1.0, 0.0));
        anim.step(FRAME_S, 1.0);
        let before = anim.target();
        anim.retarget(HighlightRect::new(f32::NAN, 0.0, 88.0, 30.0));
        assert_eq!(anim.target(), before, "a poisoned target is refused");
        settle(&mut anim, 1.0);
        assert!(anim.rect().is_finite());
    }

    #[test]
    fn test_highlight_rect_bounds_rounds_outward_at_fractional_scale() {
        let rect = HighlightRect::new(10.5, 4.25, 20.5, 9.5);
        let bounds = rect.bounds(1.5);
        // left = floor(15.75) = 15, right = ceil(46.5) = 47, top = floor(6.375) = 6,
        // bottom = ceil(20.625) = 21.
        assert_eq!((bounds.x, bounds.y, bounds.w, bounds.h), (15, 6, 32, 15));
    }

    #[test]
    fn test_highlight_rect_bounds_of_degenerate_input_stays_usable() {
        let nan = HighlightRect::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0);
        let bounds = nan.bounds(f32::NAN);
        assert_eq!(bounds.x, 0, "a non-finite origin falls back to zero");
        assert_eq!(
            (bounds.w, bounds.h),
            (0, 0),
            "a non-finite size collapses instead of wrapping the unsigned field"
        );
        assert!(bounds.y.abs() <= 1_000_000);
        // A negative width is a caller bug; the rectangle collapses rather than
        // wrapping around the unsigned field.
        let inverted = HighlightRect::new(5.0, 5.0, -10.0, -10.0);
        let bounds = inverted.bounds(1.0);
        assert_eq!((bounds.w, bounds.h), (0, 0));
        // A zero or negative scale is replaced rather than dividing the window away.
        let rect = HighlightRect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(rect.bounds(0.0), rect.bounds(1.0));
        assert_eq!(rect.bounds(-2.0), rect.bounds(1.0));
    }

    #[test]
    fn test_union_rects_covers_both_inputs() {
        let a = RectI {
            x: -4,
            y: 10,
            w: 20,
            h: 5,
        };
        let b = RectI {
            x: 100,
            y: 2,
            w: 4,
            h: 40,
        };
        let union = union_rects(a, b);
        assert_eq!((union.x, union.y), (-4, 2));
        assert_eq!((union.w, union.h), (108, 40));
        assert_eq!(union_rects(a, a), a);
    }
}
