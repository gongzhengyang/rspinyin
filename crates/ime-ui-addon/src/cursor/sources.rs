//! The platform seam tier 2 reads the focused window's origin through.
//!
//! Responsibility: name where the focused client window sits in the virtual desktop, and
//! carry the implementations the resolver is built over -- one that never answers, and a
//! deterministic in-memory one for tests.
//!
//! Boundaries: this module owns the seam and nothing else. It knows nothing about the
//! ladder, the cache or the diagnostics, and it never enumerates an output. A backend
//! that cannot report absolute positions answers `None` rather than failing, which is
//! what sends the resolution to the fallback.

use super::IcId;

/// Where the focused client window sits in the virtual desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowOrigin {
    /// Left edge of the client window, in screen physical pixels.
    pub x: i32,
    /// Top edge of the client window, in screen physical pixels.
    pub y: i32,
}

/// The seam the platform backends implement to answer where the focused window is.
///
/// X11 answers with `_NET_ACTIVE_WINDOW` plus `xcb_translate_coordinates`; Wayland answers from the
/// coordinate space of its tier (see `features.md` 2.5.2) -- a layer-shell tier maps the client
/// rectangle directly, a fullscreen-parent tier adds the client window's offset inside that parent.
/// Implementations run on the host thread and must not block: the tier 2 budget is 500 us, which is
/// one round trip.
pub trait WindowGeometrySource: Send {
    /// The focused window's screen-absolute origin, or `None` when the backend cannot say.
    ///
    /// `None` is the honest answer for a platform that cannot report absolute window positions, and
    /// it is what sends the resolution to the fallback. It is not an error: nothing is delayed by it
    /// and no key is lost.
    fn window_origin(&mut self, ic: IcId) -> Option<WindowOrigin>;
}

/// A geometry source that never answers, so that tier 2 is never taken.
///
/// What the pure-Rust build has until the X11 and Wayland backends land, and what the tests use to
/// reach the fallback.
pub struct UnknownWindowGeometry;

impl WindowGeometrySource for UnknownWindowGeometry {
    fn window_origin(&mut self, _ic: IcId) -> Option<WindowOrigin> {
        None
    }
}

/// A deterministic in-memory [`WindowGeometrySource`].
///
/// A context that is not in the list has no answer, exactly as a real backend that lost its window
/// does.
pub struct FixedGeometry {
    origins: Vec<(IcId, WindowOrigin)>,
}

impl FixedGeometry {
    /// Builds a source that answers for the listed contexts only.
    pub fn new(origins: impl IntoIterator<Item = (IcId, WindowOrigin)>) -> Self {
        Self {
            origins: origins.into_iter().collect(),
        }
    }
}

impl WindowGeometrySource for FixedGeometry {
    fn window_origin(&mut self, ic: IcId) -> Option<WindowOrigin> {
        for (id, origin) in &self.origins {
            if *id == ic {
                return Some(*origin);
            }
        }
        None
    }
}
