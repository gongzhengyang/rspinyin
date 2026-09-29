//! Screen space: output enumeration, geometry, hit testing and anchor arithmetic.
//!
//! The candidate window is placed in the physical pixel space of the virtual desktop, so
//! something has to say where the outputs are and turn a caret rectangle into an [`Anchor`]
//! on one of them. This module owns both: the description of the desktop ([`ScreenLayout`],
//! [`ScreenInfo`]), the operations the cursor path needs from it (a centre-point hit test,
//! the device-pixel-ratio rule, rounding to physical pixels), and the anchor arithmetic
//! itself.
//!
//! # The enumeration seam
//!
//! Enumerating outputs belongs to the platform backends -- RandR under X11, `wl_output`
//! under Wayland -- and neither backend exists yet. Enumeration is therefore a trait
//! ([`ScreenEnumerator`]) with a deterministic in-memory implementation
//! ([`ScriptedScreens`]) that the pure-Rust build and the tests use today; a backend
//! implements the same trait without any other module changing.
//!
//! A backend whose connection lives on the UI thread cannot be pulled from the host thread.
//! The Wayland tiers own their `wl_display` there, because a Wayland connection may only be
//! used from the thread that created it, so those backends enumerate on their own thread and
//! hand the result over instead; `CursorResolver::adopt_layout` is that entry point.
//!
//! # Coordinates
//!
//! Every coordinate here is a **physical pixel** in the virtual desktop. An output left of or
//! above the primary one therefore has a negative origin, and nothing here clamps it: such a
//! desktop is normal, and the avoidance pass owns what to do about a window that would fall
//! off an edge.
//!
//! # Invalidation
//!
//! Nothing here polls. A layout is enumerated when a caller asks for one, which is the
//! hot-plug path: the backends ask again from the RandR or `wl_output` event that told them
//! something changed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ime_types::{Anchor, Placement, PlatformError, RectI, ScreenId};

/// The lowest device pixel ratio an output or a client may report.
pub const MIN_SCALE: f32 = 1.0;

/// The highest device pixel ratio an output or a client may report.
pub const MAX_SCALE: f32 = 3.0;

/// The ratio substituted for a value outside `[MIN_SCALE, MAX_SCALE]`.
pub const FALLBACK_SCALE: f32 = 1.0;

/// Where the fallback caret sits horizontally, as a fraction of the screen width.
const FALLBACK_X_FRACTION: f32 = 0.5;

/// Where the fallback caret sits vertically, as a fraction of the screen height.
const FALLBACK_Y_FRACTION: f32 = 0.6;

/// Estimated caret height in logical pixels, for a client that reports a degenerate rectangle.
const ESTIMATED_LINE_HEIGHT_DP: f32 = 20.0;

/// A caret coordinate beyond this magnitude is treated as garbage rather than as a position.
pub(crate) const MAX_PLAUSIBLE_COORDINATE: i32 = 100_000;

/// One output in the virtual desktop.
///
/// `origin` and `size` are physical pixels and `scale` is the output's device pixel ratio: a
/// 3840x2160 output running at 2.0 reports `size = (3840, 2160)` together with `scale = 2.0`.
/// The size is never multiplied by the ratio -- the UI rasterises with the ratio, it does not
/// re-derive the geometry from it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenInfo {
    /// Output id, as the platform's enumeration reports it.
    pub id: ScreenId,
    /// Top-left corner in virtual-desktop physical pixels; negative when the desktop extends
    /// left of or above the primary output.
    pub origin: (i32, i32),
    /// Size in physical pixels.
    pub size: (u32, u32),
    /// Device pixel ratio of this output.
    pub scale: f32,
    /// Output name as the platform reports it, e.g. `eDP-1`, `DP-2` or `:0.0`.
    pub name: String,
}

impl ScreenInfo {
    /// Builds one output description.
    pub fn new(
        id: ScreenId,
        origin: (i32, i32),
        size: (u32, u32),
        scale: f32,
        name: impl Into<String>,
    ) -> Self {
        Self {
            id,
            origin,
            size,
            scale,
            name: name.into(),
        }
    }

    /// Whether `point` lies inside this output.
    ///
    /// The bounds are half-open: a point on the top or left edge is inside, a point on the
    /// bottom or right edge is not. Two outputs that share an edge therefore resolve to
    /// exactly one of them, which is what keeps the hit test deterministic on a tiled desktop.
    pub fn contains(&self, point: (i32, i32)) -> bool {
        let (left, top, right, bottom) = self.bounds();
        let (x, y) = (i64::from(point.0), i64::from(point.1));
        (left..right).contains(&x) && (top..bottom).contains(&y)
    }

    /// The point `x_fraction` across and `y_fraction` down this output.
    ///
    /// Fractions are not clamped to `0.0..=1.0`: a caller asking for a position outside the
    /// output means it, and the avoidance pass is where that is judged. The result is rounded
    /// to a physical pixel.
    pub fn point_at(&self, x_fraction: f32, y_fraction: f32) -> (i32, i32) {
        let (left, top, _, _) = self.bounds();
        let x = left as f64 + f64::from(self.size.0) * f64::from(x_fraction);
        let y = top as f64 + f64::from(self.size.1) * f64::from(y_fraction);
        (to_i32(x.round()), to_i32(y.round()))
    }

    /// The anchor for a caret that is already in screen physical pixels.
    ///
    /// Returns the anchor and whether this output's own scale factor was usable. The anchor's
    /// `scale` is the **output's** ratio, not the client's: a mixed-DPI desktop has to
    /// rasterise with the ratio of the output the window lands on.
    pub fn anchor_for(&self, cursor: RectI) -> (Anchor, bool) {
        let scale = check_scale(self.scale);
        let anchor = Anchor {
            cursor,
            screen: self.id,
            scale: scale.value,
            placement: Placement::Auto,
        };
        (anchor, scale.is_valid)
    }

    /// The fallback caret position on this output: its horizontal centre, 60% down.
    ///
    /// Returns the anchor and whether this output's scale factor was usable, as
    /// [`ScreenInfo::anchor_for`] does. The caret is one pixel wide and one estimated line
    /// tall, so a UI that centres the window on the caret centres it on the output.
    pub fn fallback_anchor(&self) -> (Anchor, bool) {
        let scale = check_scale(self.scale);
        let (x, y) = self.point_at(FALLBACK_X_FRACTION, FALLBACK_Y_FRACTION);
        let h = estimated_line_height(scale.value);
        let anchor = Anchor {
            cursor: RectI { x, y, w: 1, h },
            screen: self.id,
            scale: scale.value,
            placement: Placement::Auto,
        };
        (anchor, scale.is_valid)
    }

    /// This output's rectangle as `(left, top, right, bottom)`.
    ///
    /// `i64` because an origin near `i32::MIN` combined with a size near `u32::MAX` overflows
    /// both `i32` and `u32`: widening is what keeps the hit test panic-free on geometry a
    /// backend reported wrongly.
    fn bounds(&self) -> (i64, i64, i64, i64) {
        let left = i64::from(self.origin.0);
        let top = i64::from(self.origin.1);
        let right = left + i64::from(self.size.0);
        let bottom = top + i64::from(self.size.1);
        (left, top, right, bottom)
    }
}

/// Every output the platform currently reports.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenLayout {
    /// Outputs in enumeration order; that order is what breaks a tie when two outputs overlap.
    pub screens: Vec<ScreenInfo>,
    /// The output the platform calls primary.
    pub primary: ScreenId,
}

impl ScreenLayout {
    /// Builds a layout from its parts.
    pub fn new(screens: Vec<ScreenInfo>, primary: ScreenId) -> Self {
        Self { screens, primary }
    }

    /// The output containing `point`.
    ///
    /// The first match wins, so an overlapping pair resolves deterministically. `None` means
    /// the point is outside every output, which is what a gap in the desktop -- an output that
    /// is switched off, or a coordinate from a layout that has since changed -- looks like.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_types::ScreenId;
    /// use rspinyin_ui::screen::{ScreenInfo, ScreenLayout};
    ///
    /// let layout = ScreenLayout::new(
    ///     vec![ScreenInfo::new(ScreenId::new(0), (0, 0), (1920, 1080), 1.0, "eDP-1")],
    ///     ScreenId::new(0),
    /// );
    /// assert!(layout.screen_at((10, 10)).is_some());
    /// assert!(layout.screen_at((1920, 10)).is_none());
    /// ```
    pub fn screen_at(&self, point: (i32, i32)) -> Option<&ScreenInfo> {
        self.screens.iter().find(|screen| screen.contains(point))
    }

    /// The output with this id.
    pub fn screen(&self, id: ScreenId) -> Option<&ScreenInfo> {
        self.screens.iter().find(|screen| screen.id == id)
    }

    /// The output a window should use when the caret's output is unknown.
    ///
    /// This is `primary` when the enumeration contains it and the first enumerated output
    /// otherwise: a layout whose `primary` names an output that is not in the list is still
    /// usable, and refusing to place the window at all would be worse than placing it on the
    /// first output.
    pub fn primary_screen(&self) -> Option<&ScreenInfo> {
        self.screen(self.primary).or_else(|| self.screens.first())
    }
}

/// The seam the platform backends implement.
///
/// Implementations run on the host thread, inside the callback that noticed a change, and must
/// not block: a refresh has a 5 ms budget. `Send` matches the convention the frozen
/// `SurfaceBackend` sets, and is what lets the glue keep a resolver in a process-wide `Mutex`;
/// the trait is deliberately not `Sync`, because every method takes `&mut self`.
pub trait ScreenEnumerator: Send {
    /// Reports the outputs as they are right now.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when no display connection exists or the
    /// connection cannot answer, and [`PlatformError::Disconnected`] when the connection was
    /// lost. A caller that cannot enumerate keeps the layout it already has.
    fn enumerate(&mut self) -> Result<ScreenLayout, PlatformError>;
}

/// A deterministic in-memory [`ScreenEnumerator`].
///
/// It is what the pure-Rust build and the unit tests use: no display server is touched, so a
/// layout is exactly what the caller scripted, in the order it was scripted. The platform
/// backends replace it without any other module changing.
pub struct ScriptedScreens {
    script: Script,
    enumerations: Arc<AtomicUsize>,
}

/// What a scripted source answers with.
enum Script {
    /// Each layout in turn, repeating the last one once the script is exhausted.
    Layouts {
        layouts: Vec<ScreenLayout>,
        next: usize,
    },
    /// Always the same failure, as a backend with no display connection reports.
    Unavailable,
}

impl ScriptedScreens {
    /// A source that always reports `layout`.
    pub fn fixed(layout: ScreenLayout) -> Self {
        Self::scripted(vec![layout])
    }

    /// A source that reports each layout in turn and then repeats the last one.
    ///
    /// Repeating rather than failing is what makes a hot-plug test readable: the first
    /// enumeration is the layout before a monitor was unplugged, the second is the layout
    /// after, and every later call stays consistent with the second.
    ///
    /// An empty script behaves as [`ScriptedScreens::unavailable`]: a source with nothing to
    /// report cannot be told apart from one that cannot report.
    pub fn scripted(layouts: Vec<ScreenLayout>) -> Self {
        if layouts.is_empty() {
            return Self::unavailable();
        }
        Self {
            script: Script::Layouts { layouts, next: 0 },
            enumerations: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A source that always fails, as a backend without a display connection does.
    pub fn unavailable() -> Self {
        Self {
            script: Script::Unavailable,
            enumerations: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A handle that reports how often this source has been asked.
    ///
    /// A shared counter rather than a reader on the source itself, because the caller hands
    /// ownership of the source to the resolver: this is how a test or a probe asserts that a
    /// refresh enumerated exactly once, and that `resolve` enumerated not at all.
    pub fn handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.enumerations)
    }
}

impl ScreenEnumerator for ScriptedScreens {
    fn enumerate(&mut self) -> Result<ScreenLayout, PlatformError> {
        self.enumerations.fetch_add(1, Ordering::SeqCst);
        match &mut self.script {
            Script::Layouts { layouts, next } => {
                let last = layouts.len().saturating_sub(1);
                let index = (*next).min(last);
                *next = next.saturating_add(1);
                // `index` is bounded by the last element and the script is never empty, so the
                // lookup cannot fail; it is expressed as an error rather than as an index that
                // would panic if that invariant ever broke.
                match layouts.get(index) {
                    Some(layout) => Ok(layout.clone()),
                    None => Err(PlatformError::Unavailable),
                }
            }
            Script::Unavailable => Err(PlatformError::Unavailable),
        }
    }
}

/// A device pixel ratio after validation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleCheck {
    /// The ratio to use: the input when it is in range, [`FALLBACK_SCALE`] otherwise.
    pub value: f32,
    /// Whether the input was inside `[MIN_SCALE, MAX_SCALE]`.
    pub is_valid: bool,
}

/// Validates a device pixel ratio.
///
/// The rule is the one the cursor path applies to a client's scale factor and to an output's
/// own scale: anything outside `[MIN_SCALE, MAX_SCALE]` -- including `0.0`, a negative value,
/// `NaN` and the infinities -- becomes [`FALLBACK_SCALE`], and the caller records
/// `platform/scale/invalid`. Substituting rather than refusing keeps a client that reports a
/// broken ratio usable.
pub fn check_scale(scale: f32) -> ScaleCheck {
    let is_valid = (MIN_SCALE..=MAX_SCALE).contains(&scale);
    let value = if is_valid { scale } else { FALLBACK_SCALE };
    ScaleCheck { value, is_valid }
}

/// Rounds a coordinate and casts it to `i32`; the cast saturates rather than panicking.
pub(crate) fn to_i32(value: f64) -> i32 {
    value.round() as i32
}

/// Rounds a size and casts it to `u32`; a negative value saturates to zero.
pub(crate) fn to_u32(value: f64) -> u32 {
    value.round() as u32
}

/// Clamps a widened coordinate back into `i32`.
///
/// Integer casts truncate rather than saturate, so the clamp is explicit: a centre point
/// computed in `i64` must not wrap into a plausible-looking coordinate.
fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Whether a client-reported coordinate is inside the plausible desktop range.
pub(crate) fn is_plausible(coordinate: i32) -> bool {
    i64::from(coordinate).abs() <= i64::from(MAX_PLAUSIBLE_COORDINATE)
}

/// The centre point of a rectangle, computed in `i64` so that a very wide rectangle cannot
/// overflow `i32`.
pub(crate) fn centre_of(rect: RectI) -> (i32, i32) {
    let x = i64::from(rect.x) + i64::from(rect.w) / 2;
    let y = i64::from(rect.y) + i64::from(rect.h) / 2;
    (clamp_i32(x), clamp_i32(y))
}

/// One estimated caret line height, in physical pixels.
pub(crate) fn estimated_line_height(scale: f32) -> u32 {
    let height = f64::from(ESTIMATED_LINE_HEIGHT_DP) * f64::from(scale);
    to_u32(height.max(1.0))
}

/// The output a point is on, as an id.
pub(crate) fn screen_id_at(layout: &ScreenLayout, point: (i32, i32)) -> Option<ScreenId> {
    layout.screen_at(point).map(|screen| screen.id)
}

/// Whether two anchors are close enough that the newer one is jitter.
///
/// Only the position is compared: a caret that moved by at most one physical pixel on both
/// axes is the same caret as far as the eye is concerned. A different output or a different
/// scale is never jitter.
pub(crate) fn is_jitter(cached: &Anchor, candidate: &Anchor) -> bool {
    let dx = i64::from(cached.cursor.x) - i64::from(candidate.cursor.x);
    let dy = i64::from(cached.cursor.y) - i64::from(candidate.cursor.y);
    cached.screen == candidate.screen
        && cached.scale == candidate.scale
        && dx.abs() <= JITTER_TOLERANCE_PX
        && dy.abs() <= JITTER_TOLERANCE_PX
}

/// How far a caret may move and still count as jitter, in physical pixels.
const JITTER_TOLERANCE_PX: i64 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// One output.
    fn screen(id: u32, origin: (i32, i32), size: (u32, u32), scale: f32, name: &str) -> ScreenInfo {
        ScreenInfo::new(ScreenId::new(id), origin, size, scale, name)
    }

    #[test]
    fn test_screen_contains_uses_half_open_bounds() {
        let output = screen(0, (100, 200), (300, 400), 1.0, "DP-1");
        assert!(output.contains((100, 200)), "the top-left corner is inside");
        assert!(output.contains((399, 599)), "the last pixel is inside");
        assert!(
            !output.contains((400, 600)),
            "the far edge belongs to the next output"
        );
        assert!(!output.contains((99, 200)));
    }

    #[test]
    fn test_screen_contains_is_false_for_an_empty_output() {
        let output = screen(0, (0, 0), (0, 0), 1.0, "DP-1");
        assert!(!output.contains((0, 0)));
    }

    #[test]
    fn test_screen_contains_survives_a_size_that_overflows_i32() {
        // A backend that reported nonsense must not be able to panic the hit test: the far edge
        // is computed in `i64`, so it does not wrap around.
        let output = screen(0, (i32::MAX - 10, 0), (100, 100), 1.0, "DP-1");
        assert!(output.contains((i32::MAX, 50)));
        assert!(!output.contains((i32::MIN, 50)));
    }

    #[test]
    fn test_screen_at_picks_the_output_under_the_point() {
        let layout = ScreenLayout::new(
            vec![
                screen(0, (-1920, 0), (1920, 1080), 1.0, "DP-1"),
                screen(1, (0, 0), (1920, 1080), 1.0, "eDP-1"),
            ],
            ScreenId::new(1),
        );
        let on_left = layout.screen_at((-1, 500)).map(|s| s.id);
        assert_eq!(on_left, Some(ScreenId::new(0)));
        assert_eq!(
            layout.screen_at((0, 500)).map(|s| s.id),
            Some(ScreenId::new(1)),
            "the shared edge belongs to the output that starts there"
        );
        assert_eq!(layout.screen_at((-1921, 500)), None);
    }

    #[test]
    fn test_screen_at_returns_none_in_a_gap_between_outputs() {
        let layout = ScreenLayout::new(
            vec![screen(0, (1920, 0), (1920, 1080), 1.0, "DP-2")],
            ScreenId::new(0),
        );
        assert_eq!(layout.screen_at((100, 100)), None);
        let inside = layout.screen_at((2880, 540)).map(|s| s.id);
        assert_eq!(inside, Some(ScreenId::new(0)));
    }

    #[test]
    fn test_primary_screen_falls_back_to_the_first_output() {
        let layout = ScreenLayout::new(
            vec![screen(3, (0, 0), (800, 600), 1.0, "HDMI-1")],
            ScreenId::new(0),
        );
        assert_eq!(
            layout.primary_screen().map(|s| s.id),
            Some(ScreenId::new(3))
        );
        assert_eq!(layout.screen(ScreenId::new(0)), None);
        assert!(
            ScreenLayout::new(Vec::new(), ScreenId::new(0))
                .primary_screen()
                .is_none()
        );
    }

    #[test]
    fn test_point_at_rounds_to_physical_pixels() {
        let right = screen(0, (1920, 0), (1920, 1080), 1.0, "DP-2");
        assert_eq!(right.point_at(0.5, 0.6), (2880, 648));
        let left = screen(0, (-1920, -100), (1920, 1080), 1.0, "DP-1");
        assert_eq!(left.point_at(0.5, 0.6), (-960, 548));
    }

    #[test]
    fn test_fallback_anchor_sits_at_the_screen_centre_sixty_percent_down() {
        let output = screen(0, (1920, 0), (1920, 1080), 2.0, "DP-2");
        let (anchor, scale_valid) = output.fallback_anchor();
        let expected = Anchor {
            cursor: RectI {
                x: 2880,
                y: 648,
                w: 1,
                h: 40,
            },
            screen: ScreenId::new(0),
            scale: 2.0,
            placement: Placement::Auto,
        };
        assert!(scale_valid);
        assert_eq!(anchor, expected);
    }

    #[test]
    fn test_anchor_for_reports_an_unusable_output_scale() {
        let output = screen(0, (0, 0), (1920, 1080), 0.0, "DP-1");
        let cursor = RectI {
            x: 10,
            y: 20,
            w: 2,
            h: 24,
        };
        let (anchor, scale_valid) = output.anchor_for(cursor);
        assert!(!scale_valid, "0.0 is outside the accepted range");
        assert_eq!(anchor.scale, FALLBACK_SCALE);
        assert_eq!(anchor.cursor, cursor);
        assert_eq!(anchor.screen, ScreenId::new(0));
    }

    #[test]
    fn test_is_jitter_compares_the_position_only() {
        let base = Anchor {
            cursor: RectI {
                x: 100,
                y: 200,
                w: 2,
                h: 20,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Auto,
        };
        let mut nudged = base;
        nudged.cursor.x = 101;
        assert!(is_jitter(&base, &nudged));
        nudged.cursor.x = 102;
        assert!(!is_jitter(&base, &nudged), "two pixels is a real move");
        nudged = base;
        nudged.screen = ScreenId::new(1);
        assert!(!is_jitter(&base, &nudged), "another output is never jitter");
        nudged = base;
        nudged.scale = 2.0;
        assert!(!is_jitter(&base, &nudged), "another scale is never jitter");
    }

    #[test]
    fn test_centre_of_survives_a_rectangle_wider_than_i32() {
        let huge = RectI {
            x: -100,
            y: 0,
            w: u32::MAX,
            h: u32::MAX,
        };
        assert_eq!(centre_of(huge), (i32::MAX - 100, i32::MAX));
    }

    #[test]
    fn test_estimated_line_height_is_at_least_one_pixel() {
        assert_eq!(estimated_line_height(1.0), 20);
        assert_eq!(estimated_line_height(2.0), 40);
        assert_eq!(estimated_line_height(0.0), 1, "never zero pixels tall");
    }

    #[test]
    fn test_check_scale_accepts_the_range_and_replaces_the_rest() {
        for valid in [1.0_f32, 1.25, 1.5, 2.0, 3.0] {
            let check = check_scale(valid);
            assert_eq!(check.value, valid);
            assert!(check.is_valid, "{valid} is inside the accepted range");
        }
        for invalid in [0.0_f32, 0.5, 3.5, -1.0, f32::NAN, f32::INFINITY] {
            let check = check_scale(invalid);
            assert_eq!(check.value, FALLBACK_SCALE);
            assert!(!check.is_valid, "{invalid} is outside the accepted range");
        }
    }

    #[test]
    fn test_scripted_screens_repeats_the_last_layout_after_the_script_ends() {
        let first = ScreenLayout::new(
            vec![screen(0, (0, 0), (1920, 1080), 1.0, "eDP-1")],
            ScreenId::new(0),
        );
        let second = ScreenLayout::new(
            vec![screen(0, (0, 0), (1280, 720), 1.0, "eDP-1")],
            ScreenId::new(0),
        );
        let mut screens = ScriptedScreens::scripted(vec![first.clone(), second.clone()]);
        assert_eq!(screens.enumerate().ok(), Some(first));
        assert_eq!(screens.enumerate().ok(), Some(second.clone()));
        assert_eq!(screens.enumerate().ok(), Some(second));
    }

    #[test]
    fn test_scripted_screens_fixed_reports_the_same_layout_every_time() {
        let layout = ScreenLayout::new(
            vec![screen(0, (0, 0), (800, 600), 1.0, "HDMI-1")],
            ScreenId::new(0),
        );
        let mut screens = ScriptedScreens::fixed(layout.clone());
        let calls = screens.handle();
        assert_eq!(screens.enumerate().ok(), Some(layout.clone()));
        assert_eq!(screens.enumerate().ok(), Some(layout));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the probe counts every ask"
        );
    }

    #[test]
    fn test_scripted_screens_with_an_empty_script_is_unavailable() {
        let mut empty = ScriptedScreens::scripted(Vec::new());
        assert!(matches!(empty.enumerate(), Err(PlatformError::Unavailable)));
        let mut unavailable = ScriptedScreens::unavailable();
        assert!(matches!(
            unavailable.enumerate(),
            Err(PlatformError::Unavailable)
        ));
    }
}
