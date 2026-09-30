//! The caret indicator's geometry, as the component draws it.
//!
//! Responsibility: convert the rectangle the placement pass computed -- in virtual-desktop
//! physical pixels -- into the container-relative logical coordinates `ui/candidate.slint`
//! positions the arrow with. Nothing here decides *whether* an arrow is drawn: that is the
//! placement pass's answer, and `None` from it is `None` here.
//!
//! # The two coordinate spaces
//!
//! The pass works in physical pixels of the virtual desktop and reports the window's own
//! origin with them; the component draws in logical pixels relative to the container's
//! top-left corner, which is the shadow reserve inside the window. The conversion is the
//! two subtractions below and one division by the scale factor, and doing it here keeps the
//! component free of coordinate arithmetic -- which is what makes the whole conversion a
//! pure function of the placement result.

#[cfg(test)]
mod tests;

use crate::geometry::Geometry;

/// Converts the placement pass's arrow rectangle into container-relative logical pixels.
///
/// The scale factor is read from `geometry` rather than taken as a parameter, so the
/// conversion cannot be asked to divide by a factor the rectangle was not computed with.
///
/// # Parameters
///
/// * `geometry` -- the placement result. Its `arrow` is `None` when the pass declined to
///   draw one, and its `container_offset` is where the panel sits inside the window.
///
/// # Returns
///
/// The arrow's top-left corner in container-relative logical pixels, or `None` when the
/// pass drew no arrow. The component adds the shadow reserve back to place it.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics: the two subtractions are `i32` saturating and the division is by a factor
/// the placement pass snapped to one of the supported ratios.
pub fn arrow_in_container(geometry: &Geometry) -> Option<(f32, f32)> {
    let arrow = geometry.arrow?;
    let (offset_x, offset_y) = geometry.container_offset;
    let scale = geometry.scale.value;
    Some((
        (arrow.x - geometry.window_pos.0 - offset_x) as f32 / scale,
        (arrow.y - geometry.window_pos.1 - offset_y) as f32 / scale,
    ))
}
