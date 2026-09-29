//! Wayland candidate-window backends: the four-tier ladder over one `SurfaceBackend`.
//!
//! Responsibility: own a Wayland connection of our own, pick the highest tier the compositor
//! can serve, hand out premultiplied `Argb8888` draw buffers out of a two-slot `wl_shm` pool,
//! and drive the surface from protocol events. Boundaries: this layer owns the window and its
//! pixels and nothing else. It never rasterizes -- the caller writes every pixel -- and it
//! never decides where the window should go; the geometry pass upstream hands it a position.
//!
//! # The ladder
//!
//! Wayland gives an ordinary client no way to place a window at an absolute screen position,
//! so the design climbs down through four tiers and the first one that answers is kept:
//!
//! * **T1 `zwlr_layer_shell_v1`** -- `OVERLAY`, anchored `TOP | LEFT`, margins carry the
//!   position. Pixel-exact, and the compositor cannot give the surface the keyboard.
//! * **T2 `xdg_popup`** -- the positioner anchors the popup on the caret and the compositor
//!   flips and slides it to fit the output. The compositor's configured geometry wins.
//! * **T3 the same popup, positioned by us** -- the constraint adjustment is switched off and
//!   [`canvas_popup`] computes the rectangle itself.
//! * **T4 the fallback** -- no tier answered, so no `UserInterface` is registered and the
//!   host's own candidate list takes over (`ASM-13`). Input stays fully usable; only the
//!   window is someone else's.
//!
//! [`probe`] picks the starting tier from the registry, [`probe::TierLadder`] confirms it
//! with a configure and moves on when it does not arrive within `probe::CONFIGURE_TIMEOUT`.
//!
//! # Never takes keyboard focus
//!
//! Losing keyboard focus is the project's highest-severity defect, so the rule is structural
//! rather than a promise: T1 asks for `keyboard_interactivity = none`, which a compositor
//! cannot override; this backend never calls `xdg_popup.grab`, which is the only request that
//! would give the popup the keyboard; and it never binds a `wl_keyboard` to type through.
//! [`client::ProtocolClient`] has no method that could take focus, so the binding cannot
//! introduce one by accident. What a compositor does on its own -- a mapped `xdg_toplevel`
//! ancestor is focused by most of them -- cannot be refused in xdg-shell, which is why the
//! ladder watches for it: [`events::WireEvent::KeyboardEnter`] fails the current tier at once
//! rather than leaving a focus-stealing window on screen.
//!
//! # Units
//!
//! The contract counts physical pixels; the protocol counts surface-local units, which are
//! the surface's logical size, the buffer size divided by the buffer scale. Every crossing
//! between the two goes through [`surface_offset`], [`physical_offset`] or [`SurfaceRect`],
//! and nowhere else. The buffer scale is set to the output's device pixel ratio, so the
//! buffers are drawn at the output's physical resolution and no logical pixel is ever
//! upscaled by the compositor.
//!
//! # Threading
//!
//! The connection is created here and never borrowed from the host: a `wl_display` may only
//! be used from the thread that created it, and the host thread must not touch it. The
//! connection's file descriptor is handed to the UI thread's `poll(2)` loop through
//! [`backend::WaylandBackend::connection_fd`], and redraws are paced by the `wl_surface.frame`
//! callback -- there is no polling timer anywhere in this module.
//!
//! # What could not be verified on the development machine
//!
//! **No tier of this backend can be exercised on the machine it was written on.** That
//! machine runs WSLg, whose compositor is Weston -- which `ASM-13` names as lying outside all
//! four tiers -- and `wlr-protocols` is not installed, so `zwlr_layer_shell_v1` is neither
//! present nor testable. The consequence is recorded rather than papered over:
//!
//! * **Implemented and reasoned, not verified**: the tier ladder, every geometry conversion,
//!   the buffer discipline, the event decoding and the call sequence are covered by the tests
//!   in this module tree, which run with no display server at all. The protocol constants
//!   (anchor and gravity values, layer-shell enums, constraint adjustments) were checked
//!   against the published protocol descriptions, not against a compositor.
//! * **Unverified**: that any compositor accepts these requests, that a real popup lands
//!   where the positioner asks, that a parent toplevel can be created without taking the
//!   keyboard, and that T1's margins reach pixel-exactness. Those are the ladder's four
//!   acceptance items, and each needs a real Sway, Hyprland, KWin or Mutter session.
//! * **What a real session must confirm**: tier selection per compositor, the placement
//!   deviation against the caret, that no `wl_keyboard.enter` follows the first commit, and
//!   that the popup tiers are viable at all -- see the risk note below.
//!
//! The one open protocol question is the popup tiers' parent surface: the positioner's anchor
//! rectangle must lie inside the parent's window geometry, and no xdg-shell request lets a
//! client choose a toplevel's position, so the parent has to be a surface that covers the
//! output. Whether a compositor will map such a parent without giving it the keyboard is
//! exactly what the tier ladder's focus check and the spike exist to answer. Where the answer
//! is no, the honest outcome for that compositor is T4.
//!
//! # Layout
//!
//! [`backend`] holds the backend and its trait implementation, [`client`] the seam the
//! protocol binding plugs into, [`events`] the wire vocabulary, [`probe`] the tier ladder,
//! [`layer_shell`], [`popup`] and [`canvas_popup`] one tier each, and [`shm`] the buffer
//! bookkeeping. Everything but [`backend`] is free of state that needs a connection, which is
//! what lets the whole tree be tested without one.

use ime_types::RectI;

use crate::platform::{logical_dimension, normalize_scale, physical_dimension};
use probe::CompositorKind;

pub mod backend;
pub mod canvas_popup;
pub mod client;
pub mod events;
pub mod layer_shell;
pub mod popup;
pub mod probe;
pub mod shm;

/// Which of the four window backends is in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// T1: a `zwlr_layer_shell_v1` overlay surface, positioned by its margins.
    LayerShell,
    /// T2: an `xdg_popup` the compositor positions and adjusts.
    Popup,
    /// T3: an `xdg_popup` positioned by this backend, with the adjustment switched off.
    CanvasPopup,
    /// T4: no tier answered; the host's own candidate list draws instead.
    Fallback,
}

impl Tier {
    /// The identifier the contract's `backend_id` reports.
    ///
    /// `Fallback` never reaches the UI thread as a live backend -- it is the signal that no
    /// `UserInterface` may be registered -- but it needs a name all the same, because the
    /// diagnostics record it.
    pub fn backend_id(self) -> &'static str {
        match self {
            Self::LayerShell => "wlr-layer-shell",
            Self::Popup => "wlr-popup",
            Self::CanvasPopup => "wlr-canvas",
            Self::Fallback => "wlr-fallback",
        }
    }

    /// The label the diagnostics carry as `platform.wayland.tier`.
    pub fn label(self) -> &'static str {
        match self {
            Self::LayerShell => "T1",
            Self::Popup => "T2",
            Self::CanvasPopup => "T3",
            Self::Fallback => "T4",
        }
    }
}

/// What the backend detected about its environment.
///
/// The caller turns these into diagnostics: the tier and the compositor family identify the
/// window backend in a bug report, and the counters are what `ASM-13` asks for when a
/// compositor outside the four tiers is detected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaylandDiagnostics {
    /// The tier in use, or `Fallback` when none is.
    pub tier: Tier,
    /// The compositor family inferred from the registry.
    pub compositor: CompositorKind,
    /// Whether the interactive region can be shaped rather than covering the whole surface.
    pub shaped_input_region: bool,
    /// Bytes the `wl_shm` pool holds. Fixed by the window size, and the number the memory
    /// budget is stated in: it must not grow across frames or across show/hide cycles.
    pub pool_bytes: usize,
    /// Runs of consecutive frames skipped for want of a free buffer, each run counted once
    /// when it reaches `shm::STARVATION_FRAMES`.
    pub starvation_episodes: u32,
    /// The longest such run seen.
    pub worst_starvation: u32,
    /// Protocol errors the compositor raised; never fatal on their own.
    pub protocol_errors: u32,
    /// Times this surface was given the keyboard. Must stay zero: a candidate window that
    /// holds the keyboard is the project's highest-severity defect.
    pub focus_events: u32,
}

/// One output, as the connection's `wl_output` objects describe it.
///
/// The fields are the ones a placement needs: where the output starts in the virtual desktop,
/// how large it is in physical pixels, and the device pixel ratio relating its surface-local
/// units to those pixels. The name is carried for diagnostics only.
///
/// This mirrors the shape the cursor path enumerates its outputs in, so that a position
/// resolved there can be placed here without translation. It is not the same type: the
/// enumeration lives above this crate in the dependency order, and a backend may not reach
/// up for it.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    /// Output name as the compositor reports it, e.g. `eDP-1` or `DP-2`.
    pub name: String,
    /// Top-left corner in virtual-desktop physical pixels; negative for an output left of or
    /// above the primary one.
    pub origin: (i32, i32),
    /// Size in physical pixels.
    pub size: (u32, u32),
    /// Device pixel ratio of this output.
    pub scale: f32,
}

impl OutputInfo {
    /// Builds one output description.
    pub fn new(name: impl Into<String>, origin: (i32, i32), size: (u32, u32), scale: f32) -> Self {
        Self {
            name: name.into(),
            origin,
            size,
            scale: normalize_scale(scale),
        }
    }
}

/// Where the window should go, as the geometry pass computed it.
///
/// Both rectangles are in desktop physical pixels. `caret` is what the user is typing at;
/// `top_left` is where the candidate window's top-left corner belongs, avoidance and all.
/// The two are carried together because the popup tiers need the caret -- the compositor
/// positions those against it -- while the layer-shell tier needs the corner.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlacement {
    /// The caret rectangle in desktop physical pixels.
    pub caret: RectI,
    /// The window's top-left corner in desktop physical pixels.
    pub top_left: (i32, i32),
    /// The output the caret is on.
    pub output: OutputInfo,
}

impl WindowPlacement {
    /// Builds a placement.
    pub fn new(caret: RectI, top_left: (i32, i32), output: OutputInfo) -> Self {
        Self {
            caret,
            top_left,
            output,
        }
    }
}

/// A rectangle in the surface-local units the protocol speaks.
///
/// The protocol measures everything about a surface -- a configure size, an input region, a
/// positioner's anchor rectangle -- in the same units as the surface's logical size, which is
/// the buffer size divided by the buffer scale. The contract measures in physical pixels, so
/// every crossing goes through [`SurfaceRect::from_physical`] or [`SurfaceRect::to_physical`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width; zero is legal on the wire but means "nothing".
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl SurfaceRect {
    /// Builds a rectangle.
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Converts a physical rectangle into surface-local units, rebased onto `origin`.
    ///
    /// `origin` is the physical position of the coordinate space's top-left corner -- the
    /// output for a surface that covers it, the surface itself for an input region.
    pub fn from_physical(rect: RectI, origin: (i32, i32), scale: f32) -> Self {
        let scale = normalize_scale(scale);
        Self {
            x: surface_offset(rect.x.saturating_sub(origin.0), scale),
            y: surface_offset(rect.y.saturating_sub(origin.1), scale),
            w: logical_dimension(rect.w, scale),
            h: logical_dimension(rect.h, scale),
        }
    }

    /// Converts a surface-local rectangle back into physical pixels, offset by `origin`.
    pub fn to_physical(self, origin: (i32, i32), scale: f32) -> RectI {
        let scale = normalize_scale(scale);
        RectI {
            x: origin.0.saturating_add(physical_offset(self.x, scale)),
            y: origin.1.saturating_add(physical_offset(self.y, scale)),
            w: physical_dimension(self.w, scale),
            h: physical_dimension(self.h, scale),
        }
    }

    /// Whether this rectangle lies within `outer`.
    ///
    /// Widened to `i64`: a rectangle from a compositor is untrusted input and may carry
    /// coordinates whose sum overflows `i32`.
    pub fn is_inside(self, outer: SurfaceRect) -> bool {
        let left = i64::from(self.x);
        let top = i64::from(self.y);
        let right = left + i64::from(self.w);
        let bottom = top + i64::from(self.h);
        let outer_left = i64::from(outer.x);
        let outer_top = i64::from(outer.y);
        left >= outer_left
            && top >= outer_top
            && right <= outer_left + i64::from(outer.w)
            && bottom <= outer_top + i64::from(outer.h)
    }
}

/// The buffer scale a surface should carry for an output's device pixel ratio.
///
/// `wl_surface.set_buffer_scale` takes an integer, and a ratio below one would ask the
/// compositor to upscale a buffer that is already too small, so the value is clamped to at
/// least one.
pub fn buffer_scale(scale: f32) -> i32 {
    let scale = normalize_scale(scale).round();
    if scale < 1.0 {
        1
    } else if scale > f64::from(i32::MAX) as f32 {
        i32::MAX
    } else {
        scale as i32
    }
}

/// Converts a physical offset into surface-local units, rounding to the nearest one.
///
/// The rounding is a tier's whole positioning error: at a scale of 2 the margin is a whole
/// logical pixel, so a position can land half a logical pixel -- one physical pixel -- from
/// the one asked for, inside the two-pixel tolerance the design sets for T1.
pub fn surface_offset(offset_px: i32, scale: f32) -> i32 {
    let scale = f64::from(normalize_scale(scale));
    let value = (f64::from(offset_px) / scale).round();
    if !value.is_finite() {
        return 0;
    }
    value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Converts a surface-local offset into physical pixels, rounding to the nearest one.
pub fn physical_offset(offset_dp: i32, scale: f32) -> i32 {
    let scale = f64::from(normalize_scale(scale));
    let value = (f64::from(offset_dp) * scale).round();
    if !value.is_finite() {
        return 0;
    }
    value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_identifiers_are_distinct_and_stable() {
        let ids = [
            Tier::LayerShell.backend_id(),
            Tier::Popup.backend_id(),
            Tier::CanvasPopup.backend_id(),
            Tier::Fallback.backend_id(),
        ];
        assert_eq!(ids[0], "wlr-layer-shell");
        assert_eq!(ids[1], "wlr-popup");
        assert_eq!(ids[2], "wlr-canvas");
        for (index, id) in ids.iter().enumerate() {
            assert!(
                !ids.iter().skip(index + 1).any(|other| other == id),
                "two tiers share the identifier {id}"
            );
        }
        assert_eq!(Tier::LayerShell.label(), "T1");
        assert_eq!(Tier::Fallback.label(), "T4");
    }

    #[test]
    fn test_surface_rect_round_trips_physical_pixels_at_scale_two() {
        let rect = RectI {
            x: 100,
            y: 200,
            w: 600,
            h: 140,
        };
        let local = SurfaceRect::from_physical(rect, (0, 0), 2.0);
        assert_eq!(local, SurfaceRect::new(50, 100, 300, 70));
        assert_eq!(local.to_physical((0, 0), 2.0), rect);
    }

    #[test]
    fn test_surface_rect_rebases_onto_the_output_origin() {
        // A caret on an output that starts at (1920, 0): the anchor rectangle is measured
        // from the output, not from the desktop.
        let rect = RectI {
            x: 2000,
            y: 300,
            w: 4,
            h: 40,
        };
        let local = SurfaceRect::from_physical(rect, (1920, 0), 1.0);
        assert_eq!(local, SurfaceRect::new(80, 300, 4, 40));
        assert_eq!(local.to_physical((1920, 0), 1.0), rect);
    }

    #[test]
    fn test_surface_rect_degrades_safely_on_unusable_input() {
        let rect = RectI {
            x: i32::MIN,
            y: i32::MAX,
            w: 0,
            h: 0,
        };
        let local = SurfaceRect::from_physical(rect, (0, 0), 0.0);
        // A zero scale is replaced rather than divided by, a zero size becomes one pixel, and
        // an extreme coordinate saturates instead of wrapping.
        assert_eq!(local.w, 1);
        assert_eq!(local.h, 1);
        assert!(local.x <= 0);
        assert!(local.y >= 0);
    }

    #[test]
    fn test_surface_rect_containment_is_inclusive_at_the_edges() {
        let outer = SurfaceRect::new(0, 0, 100, 50);
        assert!(SurfaceRect::new(0, 0, 100, 50).is_inside(outer));
        assert!(SurfaceRect::new(90, 40, 10, 10).is_inside(outer));
        assert!(!SurfaceRect::new(91, 40, 10, 10).is_inside(outer));
        assert!(!SurfaceRect::new(-1, 0, 10, 10).is_inside(outer));
        // A rectangle whose extent overflows i32 is still judged correctly.
        assert!(
            !SurfaceRect::new(i32::MAX, 0, u32::MAX, 10).is_inside(outer),
            "a rectangle that runs off the far edge is outside"
        );
    }

    #[test]
    fn test_offsets_round_trip_and_stay_inside_i32() {
        assert_eq!(surface_offset(600, 2.0), 300);
        assert_eq!(surface_offset(-3, 2.0), -2, "rounds to the nearest unit");
        assert_eq!(physical_offset(300, 2.0), 600);
        assert_eq!(physical_offset(i32::MAX, 4.0), i32::MAX, "saturates");
        assert_eq!(surface_offset(i32::MIN, 0.5), i32::MIN, "saturates");
    }

    #[test]
    fn test_buffer_scale_never_asks_for_an_upscale() {
        assert_eq!(buffer_scale(1.0), 1);
        assert_eq!(buffer_scale(2.0), 2);
        assert_eq!(buffer_scale(1.25), 1);
        assert_eq!(buffer_scale(0.5), 1);
        assert_eq!(buffer_scale(f32::NAN), 1);
    }

    #[test]
    fn test_output_info_normalises_its_scale() {
        let output = OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 0.0);
        assert_eq!(output.scale.to_bits(), 1.0f32.to_bits());
        assert_eq!(output.name, "eDP-1");
        assert_eq!(output.size, (1920, 1080));
    }
}
