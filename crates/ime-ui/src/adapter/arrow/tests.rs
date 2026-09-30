//! Unit tests for the caret indicator's conversion.
//!
//! A pure function of a placement result, so these need no component, no platform and no
//! fixtures: the geometry is a plain value with public fields, and the arithmetic is
//! asserted on directly.

use ime_types::{Placement, RectI, ScreenId};

use crate::geometry::{Geometry, ScaleSnap};

use super::arrow_in_container;

/// A placement result whose arrow is `arrow`, computed at `scale`.
///
/// The panel sits `shadow-margin` inside the window, which is what `container_offset`
/// carries; the window itself is at `(100, 200)` at a scale of one and at twice that at a
/// scale of two, so the same panel lands on the same logical coordinates either way.
fn geometry(arrow: Option<RectI>, scale: f32) -> Geometry {
    let reserve = 32.0 * scale;
    Geometry {
        window_pos: ((100.0 * scale) as i32, (200.0 * scale) as i32),
        window_size: (600, 300),
        container_offset: (reserve as i32, reserve as i32),
        container_size: (536, 236),
        placement: Placement::Below,
        clamped_x: false,
        clamped_y: false,
        hit_map: Vec::new(),
        arrow,
        scale: ScaleSnap {
            value: scale,
            is_exact: true,
        },
        screen: ScreenId::new(0),
    }
}

#[test]
fn test_arrow_in_container_places_the_arrow_above_the_panels_top_edge() {
    // The panel's top edge is one shadow reserve below the window's origin, and the arrow
    // hangs off that edge: its base meets the panel and its tip reaches up into the caret
    // gap. In container coordinates the base is therefore at y = 0 and the tip at y = -6,
    // which is the negative the component adds the reserve back to.
    let arrow = RectI {
        x: 150,
        y: 226,
        w: 12,
        h: 6,
    };
    let placed = arrow_in_container(&geometry(Some(arrow), 1.0));

    assert_eq!(
        placed,
        Some((18.0, -6.0)),
        "18 = 150 - 100 - 32, and -6 = 226 - 200 - 32"
    );
}

#[test]
fn test_arrow_in_container_reports_no_arrow_when_the_pass_drew_none() {
    // A flipped or clamped placement produces no arrow at all, and the conversion has to
    // stay silent rather than invent one at the window's origin.
    assert_eq!(arrow_in_container(&geometry(None, 1.0)), None);
}

#[test]
fn test_arrow_in_container_scales_the_physical_rectangle_back_to_logical_pixels() {
    // The pass works in physical pixels, so at a scale of two every number it reports is
    // twice the logical one: the same panel must convert to the same logical coordinates.
    let arrow = RectI {
        x: 300,
        y: 452,
        w: 24,
        h: 12,
    };
    let placed = arrow_in_container(&geometry(Some(arrow), 2.0));

    assert_eq!(
        placed,
        Some((18.0, -6.0)),
        "(300 - 200 - 64) / 2 and (452 - 400 - 64) / 2"
    );
}
