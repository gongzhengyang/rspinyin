//! The three coordinate spaces the harness mixes, and the conversion between them.
//!
//! Responsibility: turn a point in the candidate window -- as the layout pass reports it --
//! into the absolute screen pixel the pointer needs, and refuse a point that is not on the
//! screen. Nothing here touches a connection, so the whole module is unit-testable.
//!
//! # The spaces
//!
//! | Space | Origin | Produced by |
//! |---|---|---|
//! | screen | the root window's top-left corner | [`container_to_screen`] |
//! | window | the candidate window's top-left corner, shadow reserve included | [`window_to_screen`] |
//! | container | the container's top-left corner, shadow reserve excluded | `ime-ui`'s hit map |
//!
//! The container is inset from the window by the shadow reserve, which is the transparent
//! margin the candidate box is drawn inside. A hit-map rectangle is container-relative, so
//! aiming at a cell means adding the reserve and then the window's own position.

use super::TestError;

/// Where the candidate window sits, in the three spaces the harness mixes.
///
/// The fields mirror what `ime-ui` reports: its placement pass produces a window position
/// in virtual-desktop physical pixels, a container offset holding the shadow reserve, and
/// a device pixel ratio. A probe that exports the real geometry fills this struct from it;
/// a run that has no probe reads the position and size from the server instead, which is
/// what `X11Session::window_placement` does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlacement {
    /// Top-left corner of the window, in screen physical pixels.
    pub origin: (i32, i32),
    /// Window size in physical pixels.
    pub size: (u32, u32),
    /// The container's offset inside the window: the shadow reserve on each side, in
    /// physical pixels.
    pub container_offset: (i32, i32),
    /// The device pixel ratio the window was rasterised with.
    pub scale: f32,
}

/// The shadow reserve the design keeps around the candidate container, in logical pixels.
///
/// It is `ime_ui::geometry::Metrics`'s `shadow_dp`. The harness mirrors the number rather
/// than importing it, because `xtask` does not link the renderer; a caller that has the
/// probe's exported geometry builds a [`WindowPlacement`] directly and never needs this.
pub const DEFAULT_SHADOW_DP: u16 = 32;

/// Converts a logical dimension to physical pixels the way the platform layer does.
///
/// The rule is `ime-ui`'s `physical_dimension`: multiply and round to nearest. It is
/// duplicated rather than imported because `xtask` must not link the renderer, and it is
/// pinned by the values the platform layer's own test uses, so the two cannot drift apart
/// unnoticed. Unlike a dimension, a coordinate may be zero, so nothing is clamped up to
/// one here.
pub fn dp_to_px(value: i32, scale: f32) -> i32 {
    let scaled = value as f32 * scale;
    if !scaled.is_finite() {
        return 0;
    }
    scaled.round() as i32
}

/// Converts a point in the container's own pixels into screen-absolute pixels.
///
/// The hit map the interaction layer reports is container-relative, so a cell's centre
/// needs this conversion before the pointer can be aimed at it: the container offset moves
/// the point into the window, and the window origin moves it onto the screen.
///
/// # Errors
///
/// Returns [`TestError::OffScreen`] when the result falls outside `screen`. The bounds are
/// half-open, the rule `ime-ui`'s placement uses: a point on the top or left edge is on the
/// screen, one on the bottom or right edge is not. A refusal here is the point of the
/// assertion -- a click outside the screen would land on whatever happens to be under the
/// pointer instead of on the cell the test named.
///
/// # Panics
///
/// Never. The sum is computed in `i64`, so no input can wrap it.
pub fn container_to_screen(
    placement: &WindowPlacement,
    point: (i32, i32),
    screen: (u16, u16),
) -> Result<(i16, i16), TestError> {
    let x = i64::from(placement.origin.0)
        .saturating_add(i64::from(placement.container_offset.0))
        .saturating_add(i64::from(point.0));
    let y = i64::from(placement.origin.1)
        .saturating_add(i64::from(placement.container_offset.1))
        .saturating_add(i64::from(point.1));
    on_screen(x, y, screen)
}

/// Checks a screen-absolute point against the screen and narrows it to the wire type.
///
/// # Errors
///
/// Returns [`TestError::OffScreen`] for a negative coordinate, one past the right or
/// bottom edge, or one that no longer fits the 16-bit field a pointer event carries.
pub fn on_screen(x: i64, y: i64, screen: (u16, u16)) -> Result<(i16, i16), TestError> {
    let outside = x < 0
        || y < 0
        || x >= i64::from(screen.0)
        || y >= i64::from(screen.1)
        || x > i64::from(i16::MAX)
        || y > i64::from(i16::MAX);
    if outside {
        return Err(TestError::OffScreen {
            x,
            y,
            width: screen.0,
            height: screen.1,
        });
    }
    Ok((x as i16, y as i16))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window at (100, 200) with a 32-pixel shadow reserve, as the design draws it.
    fn placement() -> WindowPlacement {
        WindowPlacement {
            origin: (100, 200),
            size: (424, 152),
            container_offset: (32, 32),
            scale: 1.0,
        }
    }

    #[test]
    fn test_dp_to_px_matches_the_platform_rounding() {
        // The same values `ime-ui`'s `physical_size` test pins, so the two agree.
        assert_eq!(dp_to_px(601, 1.25), 751);
        assert_eq!(dp_to_px(140, 1.25), 175);
        assert_eq!(dp_to_px(600, 2.0), 1200);
        assert_eq!(dp_to_px(600, 1.0), 600);
        // A coordinate may be zero, unlike a dimension.
        assert_eq!(dp_to_px(0, 2.0), 0);
        assert_eq!(dp_to_px(i32::from(DEFAULT_SHADOW_DP), 2.0), 64);
        assert_eq!(dp_to_px(10, f32::NAN), 0, "an unusable ratio cannot move a point");
        assert_eq!(dp_to_px(10, f32::INFINITY), 0);
    }

    #[test]
    fn test_container_to_screen_places_a_hit_point_at_both_scales() {
        let screen = (1920, 1080);
        assert_eq!(
            container_to_screen(&placement(), (10, 10), screen),
            Ok((142, 242)),
            "window origin plus the shadow reserve plus the hit point"
        );
        let doubled = WindowPlacement {
            container_offset: (64, 64),
            scale: 2.0,
            ..placement()
        };
        assert_eq!(
            container_to_screen(&doubled, (10, 10), screen),
            Ok((174, 274)),
            "the reserve doubles with the ratio, the origin does not"
        );
        assert_eq!(
            container_to_screen(&doubled, (0, 0), screen),
            Ok((164, 264)),
            "the container's own origin is the shadow reserve into the window"
        );
    }

    #[test]
    fn test_container_to_screen_refuses_a_point_off_the_screen() {
        let screen = (1920, 1080);
        let off_left = WindowPlacement {
            origin: (-40, 0),
            ..placement()
        };
        assert!(matches!(
            container_to_screen(&off_left, (0, 0), screen),
            Err(TestError::OffScreen { .. })
        ));
        let inside = WindowPlacement {
            origin: (0, 0),
            ..placement()
        };
        assert!(container_to_screen(&inside, (1920, 0), screen).is_err());
        assert!(container_to_screen(&inside, (0, 1080), screen).is_err());
        assert_eq!(
            container_to_screen(&inside, (1887, 1047), screen),
            Ok((1919, 1079)),
            "the last on-screen pixel is accepted"
        );
        // A point that no longer fits the 16-bit field a pointer event carries.
        assert!(on_screen(i64::from(i16::MAX) + 1, 0, (u16::MAX, u16::MAX)).is_err());
        assert_eq!(on_screen(0, 0, screen), Ok((0, 0)));
    }
}
