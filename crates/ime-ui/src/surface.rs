//! The candidate window as the UI thread's surface.
//!
//! Responsibility: hold the Slint platform, the component binding, the placement result and
//! the frame being drawn, and drive them from the loop's calls. It owns no input state of its
//! own -- a pointer event becomes a hover or a selection and is posted straight back -- and it
//! decides nothing about what a candidate is worth.
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

use std::os::fd::BorrowedFd;
use std::time::Instant;

use ime_types::{
    Anchor, ImeError, RectI, SelectTrigger, SurfaceBackend, SurfaceEvent, ThemeSpec, UiEvent,
    UiFrame,
};

use crate::adapter::{Adapter, DrawState};
use crate::channel::UiEventQueue;
use crate::geometry::{self, Desktop, Geometry, Panel, PlacementRequest};
use crate::layout::{self, ContainerSize, Metrics};
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
    /// The panel's rectangle, as the interactive region last applied.
    region: Option<RectI>,
    /// Failures of the interactive-region call, which are reported rather than fatal.
    region_failures: u64,
    /// The diagnostic codes of the last theme resolution.
    theme_codes: [Option<&'static str>; 2],
}

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
        let adapter = Adapter::new()?;
        Ok(Self {
            platform,
            adapter,
            metrics: layout::metrics()?,
            frame: None,
            anchor: None,
            geometry: None,
            pending: Vec::new(),
            region: None,
            region_failures: 0,
            theme_codes: [None; 2],
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

    /// Drains pending input and compositor events, posting what they mean.
    ///
    /// Pointer events are tested against the hit map of the last placement and become a hover
    /// or a selection; a resize or a scale change was already applied to the window by the
    /// platform. Events beyond `limit` stay queued for the next call, so a burst of pointer
    /// motion cannot starve rendering and no input is lost either.
    ///
    /// # Parameters
    ///
    /// * `events` -- the queue the loop posts the engine's events to.
    /// * `limit` -- how many events this call may consume.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the display connection is gone, which
    /// means no further frame can be presented.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn drain_events(&mut self, events: &UiEventQueue, limit: usize) -> Result<(), ImeError> {
        self.platform
            .poll_events(&mut self.pending)
            .map_err(ImeError::from)?;
        let ready = self.pending.len().min(limit);
        let revision = self.frame.as_ref().map_or(0, |frame| frame.revision);
        let geometry = self.geometry.as_ref();
        for event in self.pending.drain(..ready) {
            deliver(event, events, geometry, revision);
        }
        Ok(())
    }

    /// Rasterizes the scene into the surface, if anything is dirty.
    ///
    /// # Parameters
    ///
    /// * `now` -- the instant the loop woke. It is not read: nothing animates, so a frame is
    ///   either drawn now or not at all.
    ///
    /// # Returns
    ///
    /// The instant the next frame is due, or `None` when nothing is animating -- which is
    /// always, until the component declares a property for a motion output. `None` is what
    /// lets the loop block indefinitely, so an idle window costs nothing at all.
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
    pub fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
        self.platform.render_if_dirty().map_err(ImeError::from)?;
        Ok(None)
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

    /// The panel's rectangle in window-relative physical pixels, once a frame has been placed.
    ///
    /// This is the region the pointer can hit; everything outside it is the transparent shadow
    /// reserve, which stays out of the interactive region so that a click there reaches the
    /// application underneath.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn input_region(&self) -> Option<RectI> {
        self.region
    }

    /// One entry per visible candidate of the last placement, in container-relative physical
    /// pixels, paired with the candidate's global index.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn hit_map(&self) -> &[(RectI, u16)] {
        match self.geometry.as_ref() {
            Some(geometry) => &geometry.hit_map,
            None => &[],
        }
    }

    /// How many times the interactive region could not be applied.
    ///
    /// A surface whose region cannot be shaped still draws, but the pointer falls through the
    /// whole window instead of only through the reserve around the panel, so the counter is
    /// what a diagnostic reports.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn region_failures(&self) -> u64 {
        self.region_failures
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

    /// Draws one frame, dropping it when the window already draws something newer.
    fn draw(&mut self, frame: Box<UiFrame>) {
        if !self.adapter.apply_frame(&frame) {
            return;
        }
        self.frame = Some(frame);
        self.place();
    }

    /// Places the window for the frame it is drawing and applies its interactive region.
    fn place(&mut self) {
        let Some(frame) = self.frame.as_deref() else {
            return;
        };
        let anchor = self.anchor.unwrap_or(frame.anchor);
        let panel = {
            let state = self.adapter.state();
            Panel {
                size: ContainerSize {
                    width: state.container_width,
                    height: state.container_height,
                },
                cell_width: state.cell_width,
                columns: columns(state),
            }
        };
        let request = PlacementRequest::new(
            &anchor,
            // The outputs of the virtual desktop are a platform capability this layer does not
            // have, so the pass falls back to the anchor's own output and keeps the window
            // where the caret puts it: without a known output it can neither flip the window
            // above the caret nor clamp it to the screen edge.
            Desktop {
                screens: &[],
                primary: anchor.screen,
            },
            panel,
            frame,
            self.metrics,
        );
        let geometry = geometry::compute(&request);
        let region = RectI {
            x: geometry.container_offset.0,
            y: geometry.container_offset.1,
            w: geometry.container_size.0,
            h: geometry.container_size.1,
        };
        self.geometry = Some(geometry);
        self.region = Some(region);
        if self.platform.set_input_region(&[region]).is_err() {
            self.region_failures = self.region_failures.saturating_add(1);
        }
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

/// Turns one backend event into what the host needs to know about it.
///
/// Pointer positions arrive relative to the window and are tested against the hit map of the
/// last placement. Everything else either has no meaning for an override-redirect panel -- a
/// close request, a wheel -- or was already applied to the window by the platform, which is
/// what a resize and a scale change are.
fn deliver(event: SurfaceEvent, events: &UiEventQueue, geometry: Option<&Geometry>, revision: u32) {
    match event {
        SurfaceEvent::PointerEnter { x, y } | SurfaceEvent::PointerMotion { x, y } => {
            let index = hit_test(geometry, x, y);
            events.post_hover(UiEvent::Hover { revision, index }, Instant::now());
        }
        SurfaceEvent::PointerLeave => {
            events.post_hover(
                UiEvent::Hover {
                    revision,
                    index: None,
                },
                Instant::now(),
            );
        }
        SurfaceEvent::PointerButton {
            x,
            y,
            button: 1,
            pressed: true,
        } => {
            let Some(index) = hit_test(geometry, x, y) else {
                return;
            };
            // A click that cannot be handed over within the queue's budget is abandoned rather
            // than retried: the queue counts it behind `ui/select/timeout`, and failing this
            // call would take the whole UI thread down.
            let _ = events.post_select(UiEvent::Select {
                revision,
                index,
                trigger: SelectTrigger::Mouse,
            });
        }
        _ => {}
    }
}

/// The candidate under a window-relative pointer position, if there is one.
///
/// The pointer arrives relative to the window and the hit map is expressed in the container's
/// own space, so the shadow reserve is subtracted before the test. A position that cannot be
/// expressed in that space -- the far edge of an `i32` -- is outside every cell.
fn hit_test(geometry: Option<&Geometry>, x: i32, y: i32) -> Option<u16> {
    let geometry = geometry?;
    let x = i64::from(x) - i64::from(geometry.container_offset.0);
    let y = i64::from(y) - i64::from(geometry.container_offset.1);
    geometry
        .hit_map
        .iter()
        .find(|(rect, _)| {
            let left = i64::from(rect.x);
            let top = i64::from(rect.y);
            (left..left + i64::from(rect.w)).contains(&x)
                && (top..top + i64::from(rect.h)).contains(&y)
        })
        .map(|(_, index)| *index)
}

/// Candidates per row as the component draws them, for the placement pass.
fn columns(state: &DrawState) -> u8 {
    state.max_per_row.clamp(1, i32::from(u8::MAX)) as u8
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ime_types::HideReason;

    use super::*;
    use crate::adapter::tests::{anchor, frame_with};
    use crate::channel::ChannelConfig;
    use crate::renderer::mock::{MockState, MockSurface, on_own_thread};

    /// The surface the tests draw into, in logical pixels at a scale of 1.0.
    const SURFACE_WIDTH_DP: u32 = 320;
    const SURFACE_HEIGHT_DP: u32 = 160;

    /// Bytes per row of the mock's buffer at a scale of 1.0.
    const STRIDE: usize = SURFACE_WIDTH_DP as usize * 4;

    /// Builds a surface on a fresh thread and runs `scene` against it.
    ///
    /// The scene runs on its own thread because Slint installs one platform per thread, and it
    /// returns plain values because the component the surface holds is not `Send`.
    fn with_surface<R: Send + 'static>(
        scene: impl FnOnce(&mut CandidateSurface, Arc<Mutex<MockState>>) -> R + Send + 'static,
    ) -> R {
        on_own_thread(move || {
            let (backend, state) = MockSurface::new(SURFACE_WIDTH_DP, SURFACE_HEIGHT_DP, 1.0);
            let mut surface = CandidateSurface::new(Box::new(backend)).expect("the surface starts");
            scene(&mut surface, state)
        })
    }

    /// Shows the window, draws one frame of `count` candidates and rasterizes it.
    fn show_and_draw(surface: &mut CandidateSurface, count: usize) {
        surface
            .apply(SurfaceUpdate::Show {
                revision: 1,
                anchor: anchor(),
            })
            .expect("the window can be shown");
        surface
            .apply(SurfaceUpdate::Frame(Box::new(frame_with(
                1, "ni'hao", count,
            ))))
            .expect("the frame is applied");
        surface.render(Instant::now()).expect("the frame is drawn");
    }

    /// Renders until the scene stops changing, so a sampled frame is the settled one.
    fn settle(surface: &mut CandidateSurface) {
        for _ in 0..4 {
            surface
                .render(Instant::now())
                .expect("a settling frame is drawn");
        }
    }

    #[test]
    fn test_surface_show_frame_hide_commits_and_unmaps() {
        let (committed, starved, shown, hidden) = with_surface(|surface, state| {
            show_and_draw(surface, 9);
            let committed = surface.committed_frames();
            let starved = surface.starvation_streak();
            let shown = state.lock().expect("the mock is not poisoned").visible;
            surface
                .apply(SurfaceUpdate::Hide {
                    revision: 2,
                    reason: HideReason::Committed,
                })
                .expect("the window can be hidden");
            surface.render(Instant::now()).expect("the hide is handled");
            let hidden = state.lock().expect("the mock is not poisoned").visible;
            (committed, starved, shown, hidden)
        });
        assert!(committed > 0, "the frame reaches the surface");
        assert_eq!(starved, 0, "the mock always has a free buffer");
        assert!(shown, "showing the window maps the surface");
        assert!(!hidden, "hiding it unmaps the surface");
    }

    #[test]
    fn test_surface_draws_a_non_empty_panel_and_leaves_the_reserve_transparent() {
        let (centre_alpha, reserve_alpha, region) = with_surface(|surface, state| {
            show_and_draw(surface, 1);
            settle(surface);
            let region = surface.input_region().expect("the panel was placed");
            let state = state.lock().expect("the mock is not poisoned");
            let centre = state.pixel(
                STRIDE,
                (region.x + region.w as i32 / 2) as usize,
                (region.y + region.h as i32 / 2) as usize,
            );
            let reserve = state.pixel(STRIDE, 1, 1);
            (centre[3], reserve[3], region)
        });
        assert!(
            centre_alpha > 0,
            "the panel is painted, and {region:?} is where it was placed"
        );
        assert_eq!(
            reserve_alpha, 0,
            "the shadow reserve is transparent, so nothing is drawn outside the panel"
        );
    }

    #[test]
    fn test_surface_applies_the_panel_as_the_interactive_region() {
        let (region, recorded) = with_surface(|surface, state| {
            show_and_draw(surface, 1);
            let region = surface.input_region().expect("the panel was placed");
            let recorded = state
                .lock()
                .expect("the mock is not poisoned")
                .region
                .clone();
            (region, recorded)
        });
        assert_eq!(
            region,
            RectI {
                x: 32,
                y: 32,
                w: 220,
                h: 88
            },
            "the panel is inset by the 32dp shadow reserve on every side, and the placement \
             rounds its extent up to an even number"
        );
        assert_eq!(
            recorded,
            vec![region],
            "the reserve stays out of the interactive region, so a click there reaches the \
             application underneath"
        );
    }

    #[test]
    fn test_surface_drops_a_frame_older_than_the_one_it_draws() {
        let (before, after) = with_surface(|surface, _state| {
            show_and_draw(surface, 1);
            let before = surface.input_region();
            surface
                .apply(SurfaceUpdate::Frame(Box::new(frame_with(0, "ni", 9))))
                .expect("the frame is handled");
            (before, surface.input_region())
        });
        assert_eq!(
            before, after,
            "an older frame does not resize the panel the window is drawing"
        );
    }

    #[test]
    fn test_surface_idle_render_reports_no_deadline_and_commits_nothing() {
        let (idle, before, after) = with_surface(|surface, state| {
            show_and_draw(surface, 9);
            settle(surface);
            let before = state.lock().expect("the mock is not poisoned").commits;
            let first = surface
                .render(Instant::now())
                .expect("an idle frame is handled");
            // The budget's probe watches ten seconds of a still window; what a unit test can
            // assert is the same claim over a window it can afford to wait out.
            std::thread::sleep(Duration::from_millis(50));
            let second = surface
                .render(Instant::now())
                .expect("an idle frame is handled");
            let after = state.lock().expect("the mock is not poisoned").commits;
            (first.is_none() && second.is_none(), before, after)
        });
        assert!(
            idle,
            "an idle surface reports no deadline, which is what lets the loop block"
        );
        assert_eq!(after, before, "an idle surface commits nothing");
    }

    #[test]
    fn test_surface_click_inside_a_cell_posts_a_select() {
        let (selected, from_header, index) = with_surface(|surface, state| {
            show_and_draw(surface, 3);
            let events = UiEventQueue::new(&ChannelConfig::default());
            let region = surface.input_region().expect("the panel was placed");
            let (cell, index) = surface.hit_map()[0];
            let centre_x = region.x + cell.x + cell.w as i32 / 2;
            let centre_y = region.y + cell.y + cell.h as i32 / 2;
            click(&state, centre_x, centre_y);
            surface
                .drain_events(&events, 8)
                .expect("the click is delivered");
            let selected = events.poll(Duration::ZERO);
            // The header carries the preedit, not a candidate.
            click(&state, region.x + 2, region.y + 2);
            surface
                .drain_events(&events, 8)
                .expect("the click is delivered");
            let from_header = events.poll(Duration::ZERO);
            (selected, from_header, index)
        });
        assert_eq!(
            selected,
            Some(UiEvent::Select {
                revision: 1,
                index,
                trigger: SelectTrigger::Mouse
            }),
            "a click on a cell selects the candidate the hit map names"
        );
        assert_eq!(from_header, None, "the header is not a candidate");
    }

    #[test]
    fn test_surface_pointer_motion_posts_a_hover() {
        let hovered = with_surface(|surface, state| {
            show_and_draw(surface, 3);
            let events = UiEventQueue::new(&ChannelConfig::default());
            let region = surface.input_region().expect("the panel was placed");
            let (cell, _) = surface.hit_map()[0];
            state
                .lock()
                .expect("the mock is not poisoned")
                .pending
                .push(SurfaceEvent::PointerMotion {
                    x: region.x + cell.x + 1,
                    y: region.y + cell.y + 1,
                });
            surface
                .drain_events(&events, 8)
                .expect("the motion is delivered");
            events.poll(Duration::ZERO)
        });
        assert!(
            matches!(hovered, Some(UiEvent::Hover { index: Some(0), .. })),
            "the pointer over a cell hovers that candidate, got {hovered:?}"
        );
    }

    #[test]
    fn test_surface_without_a_frame_has_no_region_and_no_hit_map() {
        let (region, hits, geometry_events) = with_surface(|surface, _state| {
            let events = UiEventQueue::new(&ChannelConfig::default());
            let geometry_events = surface
                .drain_events(&events, 8)
                .map(|()| events.poll(Duration::ZERO));
            (
                surface.input_region(),
                surface.hit_map().len(),
                geometry_events,
            )
        });
        assert_eq!(region, None, "nothing is placed before the first frame");
        assert_eq!(hits, 0, "and nothing can be hit");
        assert!(
            matches!(geometry_events, Ok(None)),
            "an empty backend posts nothing"
        );
    }

    /// Pushes a left button press at a window-relative position.
    fn click(state: &Arc<Mutex<MockState>>, x: i32, y: i32) {
        state
            .lock()
            .expect("the mock is not poisoned")
            .pending
            .push(SurfaceEvent::PointerButton {
                x,
                y,
                button: 1,
                pressed: true,
            });
    }
}
