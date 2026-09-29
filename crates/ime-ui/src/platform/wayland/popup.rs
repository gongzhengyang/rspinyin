//! Tier 2: an `xdg_popup` that the compositor positions and adjusts.
//!
//! Responsibility: describe the popup's placement to `xdg_positioner` and adopt the geometry
//! the compositor configures. Boundaries: no connection and no protocol object; the request is
//! plain data the connection binding translates into `xdg_positioner` and `xdg_popup` calls.
//!
//! # How the popup lands where it does
//!
//! The positioner anchors a rectangle rather than a point: the anchor selects a corner of the
//! anchor rectangle, the gravity selects the direction the surface grows from that corner,
//! and the constraint adjustment says what the compositor may do when the result does not
//! fit. Tier 2 asks for `anchor = bottom-left` on the caret and `gravity = bottom-right`,
//! which puts the popup's top-left corner on the caret's bottom-left corner -- directly below
//! the caret, left-aligned with it -- and lets the compositor flip it above the caret and
//! slide it sideways when the output edge is in the way.
//!
//! # The parent's coordinate space
//!
//! The anchor rectangle is expressed in the parent surface's window geometry, and the
//! protocol requires that it lie inside it. No xdg-shell request lets a client choose a
//! toplevel's position, so the only parent whose origin this backend can know is one that
//! covers an output; [`ParentSpace`] is that surface's coordinate space, and
//! [`PopupRequest::anchor_rect_fits`] is the check that the placement stays expressible in
//! it. Whether a compositor will map such a parent without giving it the keyboard is the
//! question the tier ladder's focus check exists to answer, and the one a real session has to
//! settle.
//!
//! # The compositor's answer wins
//!
//! A configure carries the position and size the compositor settled on, after it applied the
//! constraint adjustment and its own policy. Those values are used as they are; the ones this
//! module asked for are only a request, and a client that insisted on them would put the
//! window under the output edge.

use ime_types::RectI;

use super::{OutputInfo, SurfaceRect};

/// Anchor the popup against the top-left corner of the anchor rectangle.
pub const ANCHOR_TOP_LEFT: u32 = 5;

/// Anchor the popup against the bottom-left corner of the anchor rectangle.
pub const ANCHOR_BOTTOM_LEFT: u32 = 6;

/// Grow the popup down and to the right from the anchor point.
pub const GRAVITY_BOTTOM_RIGHT: u32 = 8;

/// Ask the compositor to leave the popup where it was placed, even if that is off-screen.
pub const CONSTRAINT_NONE: u32 = 0;

/// Let the compositor move the popup sideways until it fits.
pub const CONSTRAINT_SLIDE_X: u32 = 1;

/// Let the compositor put the popup on the other side of the anchor point vertically.
pub const CONSTRAINT_FLIP_Y: u32 = 8;

/// The adjustment tier 2 asks for: above the caret when there is no room below, and sideways
/// when the popup would leave the output. The compositor applies flip before slide, so a
/// window that fits after flipping is not also slid.
pub const ADJUST_FLIP_SLIDE: u32 = CONSTRAINT_FLIP_Y | CONSTRAINT_SLIDE_X;

/// The parent surface's coordinate space: where its origin is and how its units map to
/// physical pixels.
///
/// Both fields are needed together and neither means anything alone, which is why they travel
/// as one value: the anchor rectangle is rebased by the origin and scaled by the ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParentSpace {
    /// The parent's top-left corner in desktop physical pixels.
    pub origin: (i32, i32),
    /// The parent's device pixel ratio, relating its units to physical pixels.
    pub scale: f32,
}

impl ParentSpace {
    /// Builds a parent space.
    pub fn new(origin: (i32, i32), scale: f32) -> Self {
        Self {
            origin,
            scale: crate::platform::normalize_scale(scale),
        }
    }

    /// The space of a parent whose window geometry covers `output`.
    ///
    /// That is the only parent position a client can know: a toplevel's position is the
    /// compositor's decision, and a surface covering an output starts at the output's origin.
    pub fn for_output(output: &OutputInfo) -> Self {
        Self::new(output.origin, output.scale)
    }
}

/// What `xdg_positioner` and `xdg_popup` need to place the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupRequest {
    /// The rectangle the popup is anchored against, in the parent's units.
    pub anchor_rect: SurfaceRect,
    /// Which corner of the anchor rectangle the popup is anchored to.
    pub anchor: u32,
    /// Which direction the popup grows from the anchor point.
    pub gravity: u32,
    /// What the compositor may do when the popup does not fit.
    pub constraint_adjustment: u32,
    /// Requested width in surface-local units.
    pub width_dp: u32,
    /// Requested height in surface-local units.
    pub height_dp: u32,
}

impl PopupRequest {
    /// The tier 2 placement: the popup hangs below the caret and the compositor adjusts it.
    pub fn below_caret(caret: RectI, space: ParentSpace, size_dp: (u32, u32)) -> Self {
        Self {
            anchor_rect: SurfaceRect::from_physical(caret, space.origin, space.scale),
            anchor: ANCHOR_BOTTOM_LEFT,
            gravity: GRAVITY_BOTTOM_RIGHT,
            constraint_adjustment: ADJUST_FLIP_SLIDE,
            width_dp: size_dp.0.max(1),
            height_dp: size_dp.1.max(1),
        }
    }

    /// The tier 3 placement: the popup's top-left corner lands on the anchor rectangle's.
    ///
    /// With the adjustment switched off the compositor does exactly what the rectangle says,
    /// so the rectangle carries a position this backend computed rather than the caret.
    pub fn at(anchor_rect: SurfaceRect, size_dp: (u32, u32)) -> Self {
        Self {
            anchor_rect,
            anchor: ANCHOR_TOP_LEFT,
            gravity: GRAVITY_BOTTOM_RIGHT,
            constraint_adjustment: CONSTRAINT_NONE,
            width_dp: size_dp.0.max(1),
            height_dp: size_dp.1.max(1),
        }
    }

    /// Whether the anchor rectangle lies inside the parent's window geometry.
    ///
    /// The protocol requires it, and a request that breaks it is refused with `invalid_input`
    /// rather than adjusted. The caller checks before committing so that a placement mistake
    /// is a failed tier -- which the ladder can act on -- instead of a protocol error.
    pub fn anchor_rect_fits(&self, parent_geometry: SurfaceRect) -> bool {
        self.anchor_rect.is_inside(parent_geometry)
    }
}

/// Where the popup ended up, as the compositor configured it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupGeometry {
    /// The popup's top-left corner in desktop physical pixels.
    pub top_left: (i32, i32),
    /// The popup's size in surface-local units, which is the size the renderer lays out in.
    pub size_dp: (u32, u32),
}

impl PopupGeometry {
    /// Adopts one `xdg_popup.configure`.
    ///
    /// `x` and `y` are relative to the parent's window geometry and `size_dp` is the window
    /// geometry the compositor settled on; both are in the parent's units, so the position is
    /// rebased and scaled and the size is taken as it is.
    pub fn adopt(x: i32, y: i32, size_dp: (u32, u32), space: ParentSpace) -> Self {
        Self {
            top_left: (
                space
                    .origin
                    .0
                    .saturating_add(super::physical_offset(x, space.scale)),
                space
                    .origin
                    .1
                    .saturating_add(super::physical_offset(y, space.scale)),
            ),
            // A zero-sized popup is not a window; the caller's re-layout needs something to
            // lay out, and a compositor sending zero means it has no opinion.
            size_dp: (size_dp.0.max(1), size_dp.1.max(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> OutputInfo {
        OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 1.0)
    }

    fn caret() -> RectI {
        RectI {
            x: 400,
            y: 300,
            w: 4,
            h: 40,
        }
    }

    #[test]
    fn test_below_caret_anchors_on_the_carets_bottom_left() {
        let space = ParentSpace::for_output(&output());
        let request = PopupRequest::below_caret(caret(), space, (600, 140));
        assert_eq!(request.anchor_rect, SurfaceRect::new(400, 300, 4, 40));
        assert_eq!(request.anchor, ANCHOR_BOTTOM_LEFT);
        assert_eq!(request.gravity, GRAVITY_BOTTOM_RIGHT);
        assert_eq!(request.constraint_adjustment, ADJUST_FLIP_SLIDE);
        assert_eq!(
            (request.width_dp, request.height_dp),
            (600, 140),
            "the positioner's size is surface-local, not physical"
        );
    }

    #[test]
    fn test_below_caret_rebases_and_scales_into_the_parent_space() {
        let second = OutputInfo::new("DP-2", (1920, 0), (1920, 1080), 2.0);
        let space = ParentSpace::for_output(&second);
        let caret = RectI {
            x: 2020,
            y: 300,
            w: 4,
            h: 40,
        };
        let request = PopupRequest::below_caret(caret, space, (600, 140));
        assert_eq!(
            request.anchor_rect,
            SurfaceRect::new(50, 150, 2, 20),
            "the caret is measured from the parent's corner, in the parent's units"
        );
    }

    #[test]
    fn test_the_anchor_rectangle_must_fit_the_parent() {
        let space = ParentSpace::for_output(&output());
        let request = PopupRequest::below_caret(caret(), space, (600, 140));
        let covering = SurfaceRect::new(0, 0, 1920, 1080);
        assert!(request.anchor_rect_fits(covering));
        let one_pixel = SurfaceRect::new(0, 0, 1, 1);
        assert!(
            !request.anchor_rect_fits(one_pixel),
            "a 1x1 parent cannot express a caret anchored across the desktop"
        );
    }

    #[test]
    fn test_at_places_the_popup_on_the_rectangle_it_is_given() {
        let request = PopupRequest::at(SurfaceRect::new(120, 340, 600, 140), (600, 140));
        assert_eq!(request.anchor, ANCHOR_TOP_LEFT);
        assert_eq!(request.gravity, GRAVITY_BOTTOM_RIGHT);
        assert_eq!(
            request.constraint_adjustment, CONSTRAINT_NONE,
            "tier 3 does its own avoidance instead of asking the compositor"
        );
    }

    #[test]
    fn test_adopt_configure_takes_the_compositors_position() {
        let space = ParentSpace::for_output(&output());
        let geometry = PopupGeometry::adopt(400, 340, (600, 140), space);
        assert_eq!(geometry.top_left, (400, 340));
        assert_eq!(geometry.size_dp, (600, 140));
    }

    #[test]
    fn test_adopt_configure_scales_a_position_on_a_scaled_output() {
        let space = ParentSpace::new((1920, 0), 2.0);
        let geometry = PopupGeometry::adopt(100, 50, (600, 140), space);
        assert_eq!(geometry.top_left, (1920 + 200, 100));
        assert_eq!(
            geometry.size_dp,
            (600, 140),
            "the configured size is already surface-local"
        );
    }

    #[test]
    fn test_adopt_configure_never_yields_a_zero_sized_popup() {
        let space = ParentSpace::for_output(&output());
        let geometry = PopupGeometry::adopt(0, 0, (0, 0), space);
        assert_eq!(geometry.size_dp, (1, 1));
    }

    #[test]
    fn test_a_degenerate_caret_still_produces_a_usable_anchor_rectangle() {
        let space = ParentSpace::for_output(&output());
        let degenerate = RectI {
            x: 10,
            y: 20,
            w: 0,
            h: 0,
        };
        let request = PopupRequest::below_caret(degenerate, space, (600, 140));
        assert_eq!(
            request.anchor_rect,
            SurfaceRect::new(10, 20, 1, 1),
            "a zero-sized anchor rectangle is not a placement"
        );
    }
}
