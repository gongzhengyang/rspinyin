//! Tier 1: the wlroots layer-shell surface.
//!
//! Responsibility: turn a window position in desktop physical pixels into the request the
//! layer shell speaks -- layer, anchor, margin, size, exclusive zone, keyboard interactivity
//! -- and adopt the size the compositor configures. Boundaries: no connection and no surface
//! object; the request is plain data, and the connection binding translates it into
//! `zwlr_layer_surface_v1` requests.
//!
//! # Positioning
//!
//! `anchor = TOP | LEFT` with `margin = (top = y, right = 0, bottom = 0, left = x)` pins the
//! surface's top-left corner to `(x, y)`. Both the margin and the size are surface-local
//! units, so they are the logical pixels the window is laid out in rather than the physical
//! pixels the contract counts in; the conversion happens here, once. The margin is measured
//! from the anchored output's own corner, so a position on a desktop that extends to the left
//! of or above the primary output is rebased first.
//!
//! The rounding of that conversion is this tier's entire positioning error: at a scale of 2 a
//! margin is a whole logical pixel, so a position can land one physical pixel away from the
//! one asked for, inside the two-pixel tolerance the design allows for this tier.
//!
//! # Focus
//!
//! `keyboard_interactivity = none` is what makes this tier structurally incapable of taking
//! the keyboard: the compositor will not give the surface focus whatever else it does.
//! [`LayerRequest::new`] is the only constructor and it hard-codes the value -- there is no
//! argument with which a caller could ask for another one.

use super::{OutputInfo, physical_offset, surface_offset};

/// The layer this window is placed in: above everything the compositor draws itself.
pub const LAYER_OVERLAY: u32 = 3;

/// Anchor the surface's top edge to the output's top edge.
pub const ANCHOR_TOP: u32 = 1;

/// Anchor the surface's left edge to the output's left edge.
pub const ANCHOR_LEFT: u32 = 4;

/// The exclusive-zone value that asks for no space of its own on the output.
///
/// A candidate window floats over the desktop; reserving space for it would reflow every
/// other window on the screen every time a keystroke shows it.
pub const EXCLUSIVE_ZONE_NONE: i32 = -1;

/// The keyboard-interactivity value that never takes the keyboard.
pub const KEYBOARD_INTERACTIVITY_NONE: u32 = 0;

/// Everything `zwlr_layer_surface_v1` needs to place and shape the window.
///
/// Sizes and margins are surface-local units, as the protocol requires; the connection
/// binding passes them straight through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerRequest {
    /// Requested width in surface-local units.
    pub width_dp: u32,
    /// Requested height in surface-local units.
    pub height_dp: u32,
    /// Which edges of the output the surface is anchored to.
    pub anchor: u32,
    /// Distance from the output's top edge, in surface-local units.
    pub margin_top: i32,
    /// Distance from the output's left edge, in surface-local units.
    pub margin_left: i32,
    /// Space to reserve on the output; [`EXCLUSIVE_ZONE_NONE`] reserves none.
    pub exclusive_zone: i32,
    /// Whether the surface may take the keyboard; [`KEYBOARD_INTERACTIVITY_NONE`] always.
    pub keyboard_interactivity: u32,
    /// Which layer the surface is placed in.
    pub layer: u32,
}

impl LayerRequest {
    /// Builds the request that pins a window of `size_dp` to `position` on `output`.
    ///
    /// `position` is the window's top-left corner in desktop physical pixels, and `size_dp`
    /// the window's logical size -- the size the renderer lays out in, which is what the
    /// protocol's surface-local units are.
    pub fn new(
        position: (i32, i32),
        size_dp: (u32, u32),
        output: &OutputInfo,
        scale: f32,
    ) -> Self {
        let (x, y) = output_local(position, output.origin);
        Self {
            // A zero-sized surface cannot be shown at all, and a configure of zero means
            // "choose one yourself", so a degenerate request is clamped rather than sent.
            width_dp: size_dp.0.max(1),
            height_dp: size_dp.1.max(1),
            anchor: ANCHOR_TOP | ANCHOR_LEFT,
            margin_top: surface_offset(y, scale),
            margin_left: surface_offset(x, scale),
            exclusive_zone: EXCLUSIVE_ZONE_NONE,
            keyboard_interactivity: KEYBOARD_INTERACTIVITY_NONE,
            layer: LAYER_OVERLAY,
        }
    }

    /// Where the request puts the surface's top-left corner, in desktop physical pixels.
    ///
    /// The inverse of the conversion [`LayerRequest::new`] applies, and the probe a test
    /// asserts the round trip with: a margin is a rounded logical offset, so this can differ
    /// from the requested position by up to one physical pixel.
    pub fn position_px(&self, output: &OutputInfo, scale: f32) -> (i32, i32) {
        (
            output
                .origin
                .0
                .saturating_add(physical_offset(self.margin_left, scale)),
            output
                .origin
                .1
                .saturating_add(physical_offset(self.margin_top, scale)),
        )
    }
}

/// Converts a desktop position into an output's own coordinate space.
///
/// The layer shell measures its margin from one output's edges, so a position on a desktop
/// that extends left of or above the primary output has to be rebased first. Saturating: an
/// origin from a compositor that reported nonsense must not wrap the coordinate into a value
/// that looks plausible.
pub fn output_local(position: (i32, i32), origin: (i32, i32)) -> (i32, i32) {
    (
        position.0.saturating_sub(origin.0),
        position.1.saturating_sub(origin.1),
    )
}

/// Adopts the size the compositor configured.
///
/// A zero on either axis means "choose it yourself", so the requested size is kept for that
/// axis. Anything else is the size the compositor will accept, and taking it is mandatory
/// rather than polite: it may be smaller than the request when the output cannot fit the
/// window, and the surface is clipped to it either way. The caller re-lays out at the size
/// this returns.
pub fn adopt_configure(current_dp: (u32, u32), configured_dp: (u32, u32)) -> (u32, u32) {
    (
        if configured_dp.0 == 0 {
            current_dp.0
        } else {
            configured_dp.0
        },
        if configured_dp.1 == 0 {
            current_dp.1
        } else {
            configured_dp.1
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> OutputInfo {
        OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 1.0)
    }

    #[test]
    fn test_layer_request_pins_the_top_left_corner_with_a_margin() {
        let request = LayerRequest::new((100, 200), (600, 140), &output(), 1.0);
        assert_eq!(request.anchor, ANCHOR_TOP | ANCHOR_LEFT);
        assert_eq!((request.margin_top, request.margin_left), (200, 100));
        assert_eq!((request.width_dp, request.height_dp), (600, 140));
        assert_eq!(request.layer, LAYER_OVERLAY);
        assert_eq!(request.exclusive_zone, EXCLUSIVE_ZONE_NONE);
        assert_eq!(
            request.position_px(&output(), 1.0),
            (100, 200),
            "the margin round-trips at a scale of one"
        );
    }

    #[test]
    fn test_layer_request_never_asks_for_the_keyboard() {
        // The project's highest-severity defect is a candidate window that takes focus, so
        // every request this module can build must ask for none.
        for scale in [1.0, 1.25, 2.0, 3.0] {
            let request = LayerRequest::new((10, 20), (600, 140), &output(), scale);
            assert_eq!(request.keyboard_interactivity, KEYBOARD_INTERACTIVITY_NONE);
        }
    }

    #[test]
    fn test_layer_request_is_surface_local_so_the_margin_divides_by_the_scale() {
        let request = LayerRequest::new((400, 800), (600, 140), &output(), 2.0);
        assert_eq!((request.margin_top, request.margin_left), (400, 200));
        assert_eq!(
            (request.width_dp, request.height_dp),
            (600, 140),
            "the size is already logical and is not scaled again"
        );
        assert_eq!(
            request.position_px(&output(), 2.0),
            (400, 800),
            "and the physical position round-trips"
        );
    }

    #[test]
    fn test_layer_request_rebases_onto_a_second_output() {
        let second = OutputInfo::new("DP-2", (1920, 0), (1920, 1080), 1.0);
        let request = LayerRequest::new((2000, 300), (600, 140), &second, 1.0);
        assert_eq!(
            (request.margin_top, request.margin_left),
            (300, 80),
            "the margin is measured from the output's own corner"
        );
        assert_eq!(request.position_px(&second, 1.0), (2000, 300));
    }

    #[test]
    fn test_output_local_saturates_on_a_nonsense_origin() {
        assert_eq!(
            output_local((10, 10), (i32::MAX, i32::MIN)),
            (10 - i32::MAX, i32::MAX),
            "a subtraction that would overflow saturates instead of wrapping"
        );
        assert_eq!(output_local((5, 5), (5, 5)), (0, 0));
    }

    #[test]
    fn test_adopt_configure_takes_the_compositors_size() {
        assert_eq!(adopt_configure((600, 140), (500, 120)), (500, 120));
        assert_eq!(
            adopt_configure((600, 140), (0, 0)),
            (600, 140),
            "zero means the client chooses"
        );
        assert_eq!(adopt_configure((600, 140), (400, 0)), (400, 140));
    }

    #[test]
    fn test_layer_request_clamps_a_degenerate_size() {
        let request = LayerRequest::new((0, 0), (0, 0), &output(), 1.0);
        assert_eq!((request.width_dp, request.height_dp), (1, 1));
    }
}
