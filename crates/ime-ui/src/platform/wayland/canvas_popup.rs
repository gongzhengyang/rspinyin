//! Tier 3: the popup whose placement this backend computes for itself.
//!
//! Responsibility: turn a window position in desktop physical pixels into the anchor
//! rectangle tier 3 hands the positioner, keeping the window inside its output. Boundaries: no
//! connection and no protocol object; the result is plain data, and [`super::popup`] carries
//! it to the binding.
//!
//! # Why this tier exists at all
//!
//! Tiers 2 and 3 create the same popup over the same parent; they differ in who decides where
//! it lands. Tier 2 asks the compositor to flip and slide the popup and takes whatever
//! geometry comes back. Tier 3 switches that off and places the window itself, which is what
//! makes it work on a compositor whose constraint handling is missing, ignores the
//! positioner's adjustments, or clamps to the parent's bounds rather than the output's.
//!
//! With the adjustment switched off, the compositor does exactly what the anchor rectangle
//! says -- including placing the window off-screen if the rectangle says so. The check that
//! the rectangle is inside the output therefore moves here, and so does the avoidance.
//!
//! # The fullscreen parent with a sub-surface, and why this tier is not it
//!
//! The design considered giving the parent a fullscreen role and hanging a `wl_subsurface` on
//! it, positioning the sub-surface directly. It is not implemented, and the reason is the
//! focus rule. A mapped `xdg_toplevel` is focused by the compositor, and xdg-shell has no
//! request with which a client can refuse the keyboard; the parent would take the focus of
//! the application the user is typing into, which is the project's highest-severity defect. A
//! sub-surface inherits its parent's input behaviour rather than escaping it, so the
//! sub-surface arrangement inherits the defect with the placement problem solved.
//!
//! What tier 3 does instead is the parent the popup tiers share, whose focus behaviour the
//! ladder checks at run time: [`super::events::WireEvent::KeyboardEnter`] fails the tier at
//! once, so a compositor that focuses the parent costs the user a different window backend
//! rather than their input. Whether any compositor outside the wlroots family can be served
//! at all is the open question the spike has to settle; where the answer is no, the honest
//! outcome for that compositor is T4 and the host's own candidate list.

use ime_types::RectI;

use super::{OutputInfo, SurfaceRect};

/// Keeps a window of `size_px` inside `output`.
///
/// A position the geometry pass computed should already fit; this is the second line of
/// defence, because the first one can be wrong -- the output can have been reconfigured
/// between the layout and the commit. A window larger than the output starts at the output's
/// leading edge rather than being centred, since half a candidate window off the right edge
/// is worth more than half of it off the left.
pub fn clamp(position: (i32, i32), size_px: (u32, u32), output: &OutputInfo) -> (i32, i32) {
    let left = i64::from(output.origin.0);
    let top = i64::from(output.origin.1);
    let right = left + i64::from(output.size.0);
    let bottom = top + i64::from(output.size.1);
    let x = clamp_axis(i64::from(position.0), left, right, i64::from(size_px.0));
    let y = clamp_axis(i64::from(position.1), top, bottom, i64::from(size_px.1));
    (x as i32, y as i32)
}

/// The anchor rectangle that puts the popup's top-left corner on `position`.
///
/// The rectangle is one surface-local pixel: with `anchor = top-left` and
/// `gravity = bottom-right` the popup's top-left corner lands exactly on the rectangle's, so
/// a single point is all the positioner needs. The position is clamped into the output first,
/// which is what keeps the rectangle inside a parent whose window geometry covers that
/// output -- the condition the positioner refuses a request for.
pub fn anchor_rect(
    position: (i32, i32),
    size_px: (u32, u32),
    output: &OutputInfo,
    scale: f32,
) -> SurfaceRect {
    let (x, y) = clamp(position, size_px, output);
    let point = RectI {
        x,
        y,
        w: 1,
        h: 1,
    };
    SurfaceRect::from_physical(point, output.origin, scale)
}

/// Clamps one axis into `[start, end - size]`, collapsing to `start` when nothing fits.
fn clamp_axis(value: i64, start: i64, end: i64, size: i64) -> i64 {
    let furthest = (end - size).max(start);
    value.clamp(start, furthest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> OutputInfo {
        OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 1.0)
    }

    #[test]
    fn test_clamp_leaves_a_fitting_position_alone() {
        let placed = clamp((400, 340), (600, 140), &output());
        assert_eq!(placed, (400, 340));
    }

    #[test]
    fn test_clamp_pulls_a_window_back_inside_the_output() {
        // The caret is in the bottom-right corner, so the window as computed hangs off both
        // edges; tier 2 would let the compositor flip it, and tier 3 has to do it here.
        let placed = clamp((1700, 1000), (600, 140), &output());
        assert_eq!(placed, (1920 - 600, 1080 - 140));
        let placed = clamp((-50, -50), (600, 140), &output());
        assert_eq!(placed, (0, 0));
    }

    #[test]
    fn test_clamp_of_a_window_larger_than_the_output_starts_at_the_leading_edge() {
        let placed = clamp((100, 100), (4000, 2000), &output());
        assert_eq!(placed, (0, 0), "a window that cannot fit is not centred");
    }

    #[test]
    fn test_clamp_is_relative_to_the_outputs_own_origin() {
        let second = OutputInfo::new("DP-2", (1920, -200), (1920, 1080), 1.0);
        let placed = clamp((1900, -300), (600, 140), &second);
        assert_eq!(
            placed,
            (1920, -200),
            "the second output's top-left corner, not the desktop's"
        );
        let placed = clamp((3800, 900), (600, 140), &second);
        assert_eq!(placed, (3840 - 600, -200 + 1080 - 140));
    }

    #[test]
    fn test_clamp_survives_a_nonsense_output() {
        let broken = OutputInfo::new("?", (i32::MAX, i32::MIN), (u32::MAX, u32::MAX), 1.0);
        let (x, y) = clamp((0, 0), (600, 140), &broken);
        assert!(
            x >= 0 && y <= 0,
            "the result stays inside the output's range"
        );
    }

    #[test]
    fn test_anchor_rect_is_a_point_at_the_clamped_position() {
        let rect = anchor_rect((400, 340), (600, 140), &output(), 1.0);
        assert_eq!(rect, SurfaceRect::new(400, 340, 1, 1));
    }

    #[test]
    fn test_anchor_rect_stays_inside_a_covering_parent() {
        // The protocol refuses an anchor rectangle that leaves the parent's window geometry,
        // so the clamp is what keeps a tier 3 request legal at the output's edges.
        let output = output();
        let covering = SurfaceRect::new(0, 0, 1920, 1080);
        for position in [(400, 340), (1900, 1070), (-40, -40), (5000, 5000)] {
            let rect = anchor_rect(position, (600, 140), &output, 1.0);
            assert!(
                rect.is_inside(covering),
                "the anchor rectangle at {position:?} left the parent"
            );
        }
    }

    #[test]
    fn test_anchor_rect_is_rebased_and_scaled_for_the_parent() {
        let second = OutputInfo::new("DP-2", (1920, 0), (1920, 1080), 2.0);
        let rect = anchor_rect((2400, 600), (600, 140), &second, 2.0);
        assert_eq!(rect, SurfaceRect::new(240, 300, 1, 1));
    }
}
