//! The probe: which backend this session can host the candidate window in, and the surface
//! itself.
//!
//! Responsibility: walk the tier ladder once, construct the backend of the first tier that
//! can serve, and report the window's pre-created size. The ladder is walked in one
//! direction and a tier that cannot be constructed is not retried.
//!
//! Boundaries: the probe creates the window and stops there. It draws no pixel, places
//! nothing and knows no candidate; the renderer and the geometry pass own both. What it
//! does own is *whether* there is a surface at all, which is the question
//! `UserInterface::available()` is answered from.

use std::fmt;

use ime_types::SurfaceBackend;
use ime_ui::layout::{self, GridLayout, Metrics};
use ime_ui::platform::x11::X11Backend;

use super::environment::{Environment, SessionTier};

/// Candidates on the largest page the design allows.
///
/// The decoder's paging state caps a page at nine candidates. The number is restated here
/// because this crate does not depend on the decoder, and it only has to be an upper bound:
/// a page smaller than this one produces a shorter panel, which the surface draws into the
/// top of the window rather than clipping.
const LARGEST_PAGE: usize = 9;

/// The device pixel ratio the window is pre-created with.
///
/// Enumerating outputs -- and with them the ratio they run at -- is a seam no backend
/// implements yet (`crate::screen::ScreenEnumerator`), so the window is created at the
/// fallback ratio. On a scaled output the panel then appears at the wrong physical size
/// until a backend enumerates its outputs; the alternative, deriving a ratio from the
/// screen's millimetre size, is a guess with nothing to correct it against.
const PRE_CREATED_SCALE: f32 = crate::screen::FALLBACK_SCALE;

/// What the platform probe found.
///
/// The backend is carried rather than stored here, so the caller decides when the surface
/// becomes reachable to another thread; `super::install` is that caller.
pub enum ProbeOutcome {
    /// A backend was constructed and can host the candidate window.
    Ready {
        /// The surface the UI thread draws into.
        backend: Box<dyn SurfaceBackend>,
    },
    /// No backend can host the window, so the host keeps drawing the candidates.
    Unsupported {
        /// The tier the session offered.
        tier: SessionTier,
        /// What is missing, as a clause completing the diagnostic code.
        reason: &'static str,
    },
}

impl ProbeOutcome {
    /// The line this outcome records, or `None` when there is nothing to record.
    ///
    /// An unsupported session is reported under the frozen
    /// `platform/compositor/unsupported` code, which the platform layer, the takeover and
    /// the compositor checks all spell the same way, so one grep finds every place a
    /// session was found unable to host a window. A session that *can* host one records
    /// nothing here: which backend serves it is part of the lifecycle summary the addon
    /// writes, and a code of its own would be a second name for a condition that line
    /// already carries.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn diagnostic(&self) -> Option<String> {
        match self {
            Self::Ready { .. } => None,
            Self::Unsupported { tier, reason } => Some(format!(
                "{}: tier={} ({reason})",
                crate::ui_impl::NO_BACKEND_CODE,
                tier.name()
            )),
        }
    }

    /// The identifier of the backend, or `None` when the probe found none.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn backend_id(&self) -> Option<&'static str> {
        match self {
            Self::Ready { backend } => Some(backend.backend_id()),
            Self::Unsupported { .. } => None,
        }
    }

    /// The tier the session offered, or `None` when a backend was constructed.
    ///
    /// The complement of [`Self::backend_id`], for a caller that wants to report which rung
    /// of the ladder was missing rather than which one answered.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn tier(&self) -> Option<SessionTier> {
        match self {
            Self::Ready { .. } => None,
            Self::Unsupported { tier, .. } => Some(*tier),
        }
    }

    /// Takes the backend out of the outcome, if there is one.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn into_backend(self) -> Option<Box<dyn SurfaceBackend>> {
        match self {
            Self::Ready { backend } => Some(backend),
            Self::Unsupported { .. } => None,
        }
    }
}

impl fmt::Debug for ProbeOutcome {
    /// Renders the backend by name rather than by contents.
    ///
    /// A `Box<dyn SurfaceBackend>` has no `Debug`, and printing a whole connection would be
    /// useless anyway: what a diagnostic needs is which backend answered.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready { backend } => f
                .debug_struct("Ready")
                .field("backend", &backend.backend_id())
                .finish(),
            Self::Unsupported { tier, reason } => f
                .debug_struct("Unsupported")
                .field("tier", tier)
                .field("reason", reason)
                .finish(),
        }
    }
}

/// Probes `environment` and constructs the backend this session can host the window in.
///
/// The ladder is `ASM-13`'s: X11 first, then the Wayland tiers, then the fallback in which
/// no window is drawn and the host's own candidate list takes over. This build can
/// construct the first rung only — see the module documentation of `super` for why — so a
/// session that offers Wayland alone is answered with the fallback rather than with a
/// surface that does not exist.
///
/// # Parameters
///
/// * `environment` -- the display-server names of the session, from
///   [`Environment::from_process`] or from a caller that already knows them.
///
/// # Returns
///
/// The constructed backend, or the tier and the reason there is none. The window is
/// created at [`initial_window_size`]'s size and stays unmapped until the first `Show`, so
/// a probe that succeeds does not put anything on screen.
///
/// # Panics
///
/// Never panics.
pub fn probe(environment: &Environment) -> ProbeOutcome {
    match environment.tier() {
        SessionTier::X11 => connect_x11(environment),
        SessionTier::Wayland => ProbeOutcome::Unsupported {
            tier: SessionTier::Wayland,
            reason: WAYLAND_NOT_IN_BUILD,
        },
        SessionTier::None => ProbeOutcome::Unsupported {
            tier: SessionTier::None,
            reason: NO_DISPLAY,
        },
    }
}

/// The logical size the candidate window is pre-created at.
///
/// The window is created before the first frame exists, so its size is a policy rather than
/// a measurement: `layout::window_size` needs the panel a frame produces, and no frame has
/// been produced yet. The policy is the largest panel the design allows, and it is that way
/// round on purpose. **Nothing can resize the window afterwards**: `SurfaceBackend` has no
/// resize call, and an override-redirect window is resized by nobody else, so a window
/// smaller than the panel clips candidates off the screen, while a window larger than the
/// panel costs only the transparent reserve around it — which is click-through by
/// construction, because the interactive region is the panel and not the surface.
///
/// The width is the design's own cap on the panel ([`Metrics::max_width`]); the height is
/// what the largest page needs at the smallest number of candidates per row, computed by
/// the layout module rather than restated here.
///
/// # Parameters
///
/// * `metrics` -- the constants from [`ime_ui::layout::metrics`].
/// * `scale` -- device pixel ratio; a value that cannot describe a surface is corrected to
///   `1.0` rather than rejected, exactly as `layout::window_size` corrects it.
///
/// # Returns
///
/// `(width_dp, height_dp)` in logical pixels, each at least 1.
///
/// # Panics
///
/// Never panics.
pub fn initial_window_size(metrics: &Metrics, scale: f32) -> (u32, u32) {
    let grid = GridLayout {
        cols: metrics.max_per_row,
        rows: pre_created_rows(metrics),
        pages: 1,
        overflow: false,
    };
    // The layout module's own arithmetic for the height, so a change to the header, the
    // padding or the cell height moves this size with it. The width is the panel cap, which
    // `container_size` would only reach for a page of maximally wide cells.
    let cap = metrics.max_width;
    let measured = layout::container_size(&grid, metrics.cell_min_width, cap, metrics);
    layout::window_size(cap, measured.height, scale, metrics)
}

/// Rows the pre-created window leaves for candidates.
///
/// The largest page at the fewest candidates per row the design allows: a narrower grid
/// wraps the page into more rows, and this is the height that fits all of them.
fn pre_created_rows(metrics: &Metrics) -> u8 {
    let per_row = usize::from(metrics.min_per_row.max(1));
    let rows = LARGEST_PAGE.div_ceil(per_row);
    rows.min(usize::from(u8::MAX)) as u8
}

/// Constructs the X11 backend, or reports why it could not be.
fn connect_x11(environment: &Environment) -> ProbeOutcome {
    let Ok(metrics) = layout::metrics() else {
        return ProbeOutcome::Unsupported {
            tier: SessionTier::X11,
            reason: METRICS_MISSING,
        };
    };
    let (width_dp, height_dp) = initial_window_size(metrics, PRE_CREATED_SCALE);
    match X11Backend::connect(
        width_dp,
        height_dp,
        PRE_CREATED_SCALE,
        environment.display(),
    ) {
        Ok(backend) => ProbeOutcome::Ready {
            backend: Box::new(backend),
        },
        Err(_) => ProbeOutcome::Unsupported {
            tier: SessionTier::X11,
            reason: X_SERVER_UNREACHABLE,
        },
    }
}

/// Why a session that offers only Wayland gets no window from this build.
///
/// The reason names the build rather than the compositor: `ime_ui::platform::wayland` is not
/// part of that crate's module tree in this build, so a compositor that offered every tier
/// would be answered exactly the same way. Reporting the compositor would send an operator
/// looking for the wrong thing.
const WAYLAND_NOT_IN_BUILD: &str = concat!(
    "the session offers Wayland and this build can construct no Wayland surface; ",
    "ClassicUI keeps drawing the candidates"
);

/// Why a session with no display server gets no window.
const NO_DISPLAY: &str = concat!(
    "neither $DISPLAY nor $WAYLAND_DISPLAY names a display server; ",
    "ClassicUI keeps drawing the candidates"
);

/// Why an X11 session may still have no window.
const X_SERVER_UNREACHABLE: &str = concat!(
    "the X server $DISPLAY names could not be reached; ",
    "ClassicUI keeps drawing the candidates"
);

/// Why the window has no size to be created with.
const METRICS_MISSING: &str = concat!(
    "ui/candidate.slint declares no readable metrics block, so the window has no size; ",
    "ClassicUI keeps drawing the candidates"
);
