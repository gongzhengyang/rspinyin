//! The blur negotiation: the surface capability that asks a compositor to blur
//! behind the window, the answer it can give, and the alpha decision the answer
//! resolves into.

use ime_types::{PlatformError, RectI};

use super::OPAQUE_ALPHA;

/// Recorded when the compositor refuses the blur request: an expected degradation,
/// never shown to the user as an error.
pub const BLUR_UNAVAILABLE: &str = "ui/theme/blur-unavailable";

/// What the compositor answered when the window asked it to blur what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurNegotiation {
    /// The compositor took the request, so the base may stay translucent.
    Applied,
    /// The compositor refused it, or the round trip did not answer in time. Failure
    /// is the expected outcome on most compositors and must never block the UI thread.
    Refused,
    /// No request was made, because the user turned acrylic off.
    Disabled,
}

/// How opaque the base is painted, and whether the compositor was the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurDecision {
    /// The alpha to paint `surface.base` with.
    pub alpha: u8,
    /// The compositor refused the request, so [`BLUR_UNAVAILABLE`] belongs in the
    /// diagnostics. A user-disabled acrylic is not reported.
    pub unavailable: bool,
}

/// Decides how opaque the base has to be, given the acrylic negotiation.
///
/// # Parameters
///
/// - `outcome`: what the compositor answered, or that the user turned acrylic off.
/// - `requested`: `ui.base_alpha`, the alpha to use when blur is available.
///
/// # Returns
///
/// The alpha to paint with, and whether the refusal should be reported.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn resolve_base_alpha(outcome: BlurNegotiation, requested: u8) -> BlurDecision {
    match outcome {
        BlurNegotiation::Applied | BlurNegotiation::Disabled => BlurDecision {
            alpha: requested,
            unavailable: false,
        },
        BlurNegotiation::Refused => BlurDecision {
            alpha: OPAQUE_ALPHA,
            unavailable: true,
        },
    }
}

/// A surface that can ask its compositor to blur what is behind the window.
///
/// The frozen `SurfaceBackend` trait has no blur request and this crate may not
/// change it, so the compositor-side round trip arrives as this separate capability:
/// the backend that can speak it implements it -- setting
/// `_KDE_NET_WM_BLUR_BEHIND_REGION` on KWin, relying on the namespace rule on
/// Hyprland -- and a backend that cannot ask simply does not, which the caller
/// negotiates as [`BlurNegotiation::Refused`].
///
/// # Concurrency
///
/// Implementations must not block: a request the compositor does not answer within
/// the negotiation budget reports [`PlatformError`] instead of waiting, because this
/// call happens on the UI thread and a stalled compositor must not stall the window.
/// The trait is not `Send`, since a surface is owned by the thread that created it.
pub trait BlurSurface {
    /// Asks the compositor to blur `region`, in physical pixels relative to the
    /// window's top-left corner.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] when the request cannot be delivered, which the
    /// caller reads as [`BlurNegotiation::Refused`] and degrades to an opaque base.
    fn request_blur(&mut self, region: &[RectI]) -> Result<(), PlatformError>;
}

/// Asks a surface to blur the region behind the window.
///
/// # Parameters
///
/// - `surface`: the window's backend, behind the [`BlurSurface`] capability.
/// - `region`: the container rectangle, in physical pixels.
///
/// # Returns
///
/// [`BlurNegotiation::Applied`] when the compositor took the request and
/// [`BlurNegotiation::Refused`] when it did not. The error itself is not propagated:
/// a compositor without blur is a supported configuration, not a failure.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn request_blur(surface: &mut dyn BlurSurface, region: &[RectI]) -> BlurNegotiation {
    if surface.request_blur(region).is_ok() {
        BlurNegotiation::Applied
    } else {
        BlurNegotiation::Refused
    }
}
