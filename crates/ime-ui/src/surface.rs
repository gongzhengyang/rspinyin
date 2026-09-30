//! The candidate window as the UI thread's surface.
//!
//! Responsibility: hold the Slint platform, the component binding, the placement result and
//! the frame being drawn, and drive them from the loop's calls. Input is routed rather than
//! interpreted here: a pointer event is handed to `crate::interaction::route::PointerRouter`,
//! which decides what it means and which channel it travels on, and the surface only applies
//! the pointer state it reports back. It decides nothing about what a candidate is worth.
//!
//! # Per-frame order
//!
//! A frame is written into the component the moment it arrives, and `render` is the only call
//! that puts anything on screen: the panel is placed and its interactive region applied, and
//! the rasterizer is asked for a frame if the scene is dirty. Nothing animates yet -- the
//! component declares no property for a motion output -- so `render` reports no deadline and
//! the loop blocks indefinitely whenever the scene is clean, which is the whole of the idle
//! CPU budget. A motion that did exist would be advanced here, before the rasterize call, so
//! that the frame it produces is the one that reaches the surface.
//!
//! # The `Send` bound, and why it is gone
//!
//! `UiSurface` used to require `Send`, which a surface owning the Slint platform cannot
//! provide: [`RspinyinPlatform`] holds an `Rc`, and the component it hands out is reference
//! counted as well. The bound has since been removed from the trait, because the boxed
//! surface never crosses a thread boundary -- the factory runs on the UI thread and the loop
//! consumes the box there -- so it bought nothing and cost the production implementation.
//! See `ui_thread::surface`'s module documentation for the full reasoning.
//!
//! # Module map
//!
//! This file owns the surface's state, the calls the loop makes on it and the trait that drives
//! them. Two submodules beside it own what is neither: `placement` decides where the panel goes
//! and what the pointer can hit, and `input` turns a backend event into the event the host is
//! posted. Nothing outside the module names either of them.

mod input;
mod placement;

#[cfg(test)]
mod tests;

use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use ime_types::ui::OverlayFrame;
use ime_types::{Anchor, ImeError, RectI, SurfaceBackend, SurfaceEvent, ThemeSpec, UiFrame};

use crate::adapter::Adapter;
use crate::channel::UiEventQueue;
use crate::geometry::Geometry;
use crate::interaction::PointerRouter;
use crate::layout::{self, Metrics};
use crate::slint_platform::RspinyinPlatform;
use crate::theme::{BlurNegotiation, ThemeResolution};
use crate::ui_thread::{SurfaceUpdate, UiSurface};

/// The candidate window, as the UI thread's event loop sees it.
pub struct CandidateSurface {
    /// The platform the window draws through, and the one place the display connection lives.
    platform: RspinyinPlatform,
    /// The component binding: every property write goes through it.
    adapter: Adapter,
    /// The component's constants, parsed once.
    metrics: &'static Metrics,
    /// The frame being drawn, moved in rather than copied.
    frame: Option<Box<UiFrame>>,
    /// Where the host asked the window to appear.
    anchor: Option<Anchor>,
    /// The placement of the last frame, with the hit map the pointer is tested against.
    geometry: Option<Geometry>,
    /// Backend events a call has not consumed yet; the loop drains them on its next pass.
    pending: Vec<SurfaceEvent>,
    /// The one place a backend event becomes a host event.
    ///
    /// The router owns the pointer state across the whole event stream, so a press is
    /// remembered until its release arrives and a hover is only reported when the candidate
    /// under the pointer changes.
    router: PointerRouter,
    /// Whether a placement has been produced that the router has not been told about.
    ///
    /// Placing runs on the `apply` path, which has no event queue, and re-adopting a frame
    /// can produce a hover. The flag defers that to the next `drain_events`, which does have
    /// one. It is a flag rather than a revision because the placement pass can produce a new
    /// geometry for the frame the surface already holds, and a revision-keyed check would
    /// miss exactly the change that moves the cells.
    adoption_pending: bool,
    /// The overlay the engine last asked for, or `None` when none is open.
    ///
    /// Retained rather than drawn: the component declares no overlay surface yet, so the
    /// window keeps showing the candidate panel instead of blanking itself. Holding the
    /// frame is what lets the overlay be drawn the moment the component can, and it keeps
    /// the value observable rather than silently discarded.
    overlay: Option<Box<OverlayFrame>>,
    /// The panel's rectangle, as the interactive region last applied.
    region: Option<RectI>,
    /// Failures of the interactive-region call, which are reported rather than fatal.
    region_failures: u64,
    /// The diagnostic codes of the last theme resolution.
    theme_codes: [Option<&'static str>; 2],
    /// The instant the previous frame was drawn at.
    ///
    /// The animation is driven by the difference between two of these rather than by a
    /// clock read of its own, so the frame loop stays the only thing that knows the time
    /// and a surface that is not rendered does not animate.
    last_frame: Option<Instant>,
}

/// How long after a frame the next one is due while something is animating.
///
/// The component's spring settles in tens of milliseconds, so this is a 144Hz frame: fast
/// enough that the motion reads as continuous, slow enough that it costs nothing to run.
/// It is returned only while something is actually animating -- a still window returns
/// `None` and the loop blocks indefinitely, which is what keeps `BUDGET-CPU-01` at zero.
const FRAME_INTERVAL: Duration = Duration::from_micros(6_944);

impl CandidateSurface {
    /// Builds the surface on the thread that will run the UI loop.
    ///
    /// The Slint platform is installed here and the component is created against it, so a
    /// window that cannot exist fails at construction rather than at the first frame. The
    /// calling thread must not have a Slint platform yet, and must not have created any
    /// component, window or timer of its own.
    ///
    /// # Parameters
    ///
    /// * `backend` -- the display backend the window is drawn into. Its geometry is the
    ///   surface's size and scale factor.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the thread already owns a Slint
    /// platform (`ui/slint/conflict`) or the component cannot be bound to it
    /// (`ui/slint/component`), and [`ImeError::ConfigInvalid`] when `ui/candidate.slint` no
    /// longer declares the metrics block the layout computes with. Every one of them means the
    /// same thing to the host: there is no self-drawn candidate window, so its own user
    /// interface has to take over.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(backend: Box<dyn SurfaceBackend>) -> Result<Self, ImeError> {
        let platform = RspinyinPlatform::new(backend);
        platform.install()?;
        let mut adapter = Adapter::new()?;
        // The family is probed once, here, and written before the first frame: the probe
        // rasterises a small frame per candidate family and caches its answer process-wide,
        // so a second surface pays nothing. An empty name -- no CJK family on this machine
        // -- leaves the view's own default in place, and the probe's status is what the
        // caller reports as `ui/font/missing-cjk`.
        // No family matched: the last entry is the generic fallback `fontdb` lands on, and
        // asking for it explicitly is what the probe's own doc says to do.
        let choice = crate::renderer::probe_font_choice();
        let family = crate::renderer::CJK_FAMILIES[choice
            .family_index
            .unwrap_or(crate::renderer::CJK_FAMILIES.len() - 1)];
        adapter.apply_font_family(family);
        Ok(Self {
            platform,
            adapter,
            metrics: layout::metrics()?,
            frame: None,
            anchor: None,
            geometry: None,
            pending: Vec::new(),
            router: PointerRouter::new(),
            adoption_pending: false,
            overlay: None,
            region: None,
            region_failures: 0,
            theme_codes: [None; 2],
            last_frame: None,
        })
    }

    /// Applies one state change.
    ///
    /// A frame older than the one on screen is dropped; a `Show` also re-places the panel
    /// against the anchor the host sent, so a window that appears before its first frame still
    /// ends up where the caret is.
    ///
    /// # Parameters
    ///
    /// * `update` -- the change to apply.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the surface cannot be mapped or
    /// unmapped, which is the only part of a state change that can fail.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError> {
        match update {
            SurfaceUpdate::Frame(frame) => {
                self.draw(frame);
                Ok(())
            }
            SurfaceUpdate::Theme(spec) => {
                self.apply_theme(&spec);
                Ok(())
            }
            // The overlay is a mode of the window rather than part of the frame, and the
            // component declares no surface to draw it into yet. The frame is therefore
            // retained and reported through [`Self::overlay`]; blanking the window instead
            // would take the candidate panel away from a user who is still typing.
            SurfaceUpdate::Overlay(frame) => {
                self.overlay = frame;
                Ok(())
            }
            SurfaceUpdate::Show { anchor, .. } => {
                self.anchor = Some(anchor);
                self.adapter.set_visible(true)?;
                self.place();
                Ok(())
            }
            SurfaceUpdate::Hide { .. } => self.adapter.set_visible(false),
        }
    }

    /// The descriptor the loop adds to its `poll` set, if the surface has one.
    ///
    /// Always `None`: the platform owns the display connection and hands out no descriptor of
    /// its own, so the loop waits on the wakeup counter and on nothing else. The consequence is
    /// that a pointer event is delivered on the next wake-up rather than waking the loop
    /// itself, which a platform accessor for the backend's connection would fix.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        None
    }

    /// Rasterizes the scene into the surface, if anything is dirty.
    ///
    /// # Parameters
    ///
    /// * `now` -- the instant the loop woke. It is what the animation advances by: the
    ///   difference from the previous call is the step, and a surface that is not rendered
    ///   does not animate.
    ///
    /// # Returns
    ///
    /// The instant the next frame is due, or `None` when nothing is animating. `None` is
    /// what lets the loop block indefinitely, so an idle window costs nothing at all.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the frame cannot be presented. A
    /// backend that still holds the previous buffer is not an error: the frame is skipped and
    /// stays dirty for the next call.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn render(&mut self, now: Instant) -> Result<Option<Instant>, ImeError> {
        // The first frame has no predecessor to measure against, so it steps by nothing and
        // draws the motion at its starting point rather than jumping to where it would have
        // been had the window been on screen all along.
        let step = match self.last_frame.replace(now) {
            Some(previous) => now.duration_since(previous).as_secs_f32(),
            None => 0.0,
        };
        // The properties are written before the rasterizer runs: `advance` is what moves the
        // scene, and asking for a frame first would draw the previous one.
        let animating = self.adapter.advance(step);
        self.platform.render_if_dirty().map_err(ImeError::from)?;
        Ok(animating.then(|| now + FRAME_INTERVAL))
    }

    /// Unmaps the surface and releases it.
    ///
    /// No disappearing motion runs: the component declares no property to draw one with, and
    /// the host's shutdown budget is 200ms, which a motion the user cannot see must not spend.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the surface cannot be unmapped.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn close(&mut self) -> Result<(), ImeError> {
        self.adapter.set_visible(false)
    }

    /// Frames committed to the surface since it was created.
    ///
    /// The idle-CPU budget is asserted on this counter: a window nothing is happening to must
    /// not advance it.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn committed_frames(&self) -> u64 {
        self.platform.committed_frames()
    }

    /// Frames skipped in a row because the backend had no free buffer.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn starvation_streak(&self) -> u32 {
        self.platform.starvation_streak()
    }

    /// The diagnostic codes of the last theme resolution, in the order the degradations
    /// happened.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn theme_diagnostics(&self) -> [Option<&'static str>; 2] {
        self.theme_codes
    }

    /// The overlay the engine last asked for, or `None` when none is open.
    ///
    /// The frame is carried rather than drawn: the component declares no overlay surface
    /// yet, so this is what a caller observes until it does. A closed overlay and one that
    /// was never opened are the same value, which is the contract's own reading of the
    /// latest-wins channel (`UiCommand::Overlay(None)` means "close it").
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn overlay(&self) -> Option<&OverlayFrame> {
        self.overlay.as_deref()
    }

    /// Draws one frame, dropping it when the window already draws something newer.
    fn draw(&mut self, frame: Box<UiFrame>) {
        if !self.adapter.apply_frame(&frame) {
            return;
        }
        self.frame = Some(frame);
        self.place();
    }

    /// Resolves a theme request and writes it into the component.
    ///
    /// The blur round trip belongs to the backend capability
    /// [`crate::theme::BlurSurface`], which the platform object does not expose here, so a
    /// request for acrylic resolves as refused: the base is painted opaque and
    /// `ui/theme/blur-unavailable` is reported through [`Self::theme_diagnostics`]. Passing
    /// the compositor's answer through is what makes the translucent tier reachable.
    fn apply_theme(&mut self, spec: &ThemeSpec) {
        let negotiation = if spec.acrylic {
            BlurNegotiation::Refused
        } else {
            BlurNegotiation::Disabled
        };
        let resolution = ThemeResolution::resolve(spec, negotiation);
        self.adapter.apply_theme(spec, &resolution.tokens);
        self.theme_codes = resolution.diagnostic_codes();
    }
}

/// The surface as the UI loop sees it.
///
/// Every method is a forward to the inherent one of the same name. The calls are written
/// as `CandidateSurface::apply(self, update)` rather than `self.apply(update)` on purpose:
/// an inherent method takes precedence over a trait method of the same name, so the
/// unqualified form would call the inherent method and read as if the delegation were
/// missing. Naming the inherent method explicitly is what makes this an impl rather than
/// an infinite regress.
impl UiSurface for CandidateSurface {
    fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        CandidateSurface::event_fd(self)
    }

    fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError> {
        CandidateSurface::apply(self, update)
    }

    fn drain_events(&mut self, events: &UiEventQueue, limit: usize) -> Result<(), ImeError> {
        CandidateSurface::drain_events(self, events, limit)
    }

    fn render(&mut self, now: Instant) -> Result<Option<Instant>, ImeError> {
        CandidateSurface::render(self, now)
    }

    fn close(&mut self) -> Result<(), ImeError> {
        CandidateSurface::close(self)
    }
}
