//! The Wayland backend: one [`SurfaceBackend`] over whichever tier the ladder settled on.
//!
//! Responsibility: own the connection binding, the buffer pool and the ladder, and serve the
//! contract's eight methods on top of them. Boundaries: the protocol itself is behind
//! [`ProtocolClient`], the tier choice behind [`TierLadder`], and the buffer discipline behind
//! [`BufferPool`]; what is left here is the wiring between them, which is where the units and
//! the ordering are decided.
//!
//! # Units
//!
//! Two conversions meet here. A configure from the compositor arrives in surface-local units
//! and is reported to the caller as [`SurfaceEvent::Resize`] in physical pixels, exactly as
//! the X11 backend reports a `ConfigureNotify`; [`SurfaceBackend::geometry`] returns the
//! logical size and the scale. Pointer positions arrive in surface-local units and are
//! reported as physical pixels for the same reason. [`SurfaceBackend::set_input_region`] goes
//! the other way, physical to surface-local.
//!
//! # Ordering
//!
//! `acquire_buffer` marks a slot in flight; `commit` attaches it, and the slot stays in flight
//! until the compositor releases it. A frame that cannot be drawn -- both slots out, or the
//! pool not yet at the configured size -- is skipped with [`PlatformError::NoFreeBuffer`]
//! rather than waited for, because the caller is the UI thread and waiting there would hold up
//! the next keystroke.

use std::os::fd::{BorrowedFd, RawFd};
use std::time::{Duration, Instant};

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};

use crate::platform::{clamp_dimension, clip_rects, normalize_scale, physical_size};

use super::canvas_popup;
use super::client::{FrameCommit, ProtocolClient};
use super::events::{self, WireEvent};
use super::layer_shell::{self, LayerRequest};
use super::popup::{ParentSpace, PopupGeometry, PopupRequest};
use super::probe::{Capabilities, CompositorKind, LadderStep, TierAttempt, TierLadder};
use super::shm::BufferPool;
use super::{OutputInfo, SurfaceRect, Tier, WaylandDiagnostics, WindowPlacement, buffer_scale};

/// The Wayland candidate-window backend.
///
/// Construct one per UI thread. The connection it holds is its own -- never the host's -- and
/// is only ever used from the thread that created it, which is the thread that owns this
/// value.
pub struct WaylandBackend {
    client: Box<dyn ProtocolClient>,
    ladder: TierLadder,
    capabilities: Capabilities,
    pool: BufferPool,
    /// The window's size in surface-local units, which is the size the renderer lays out in.
    size_dp: (u32, u32),
    /// The window's size in physical pixels, which is the size of one buffer.
    size_px: (u32, u32),
    scale: f32,
    /// The window's top-left corner in desktop physical pixels.
    position: (i32, i32),
    /// The caret the window is anchored to, in desktop physical pixels.
    caret: RectI,
    /// The output the window is placed on, once it has been placed.
    output: Option<OutputInfo>,
    input_region: Vec<RectI>,
    /// The slot `acquire_buffer` handed out and `commit` has not attached yet.
    pending_slot: Option<usize>,
    next_token: u64,
    /// The frame callback that has been requested and has not fired.
    pending_frame: Option<FrameToken>,
    protocol_errors: u32,
    focus_events: u32,
    visible: bool,
    /// Whether the surface has been created, which happens on the first placement.
    created: bool,
    start: Instant,
    /// Scratch for the events of one poll, reused so a poll allocates nothing.
    wire: Vec<WireEvent>,
}

impl WaylandBackend {
    /// Builds the backend over an established connection.
    ///
    /// The tier is probed from the connection's registry, but nothing is created until
    /// [`WaylandBackend::place`] says where the window goes: a backend that is never placed
    /// costs nothing, and the tier's budget only starts once there is a surface to configure.
    /// A backend whose probe found no usable tier is returned with [`Tier::Fallback`]; the
    /// caller checks [`WaylandBackend::tier`] and, on the fallback, records
    /// [`super::probe::UNSUPPORTED_CODE`] and registers no user interface at all.
    pub fn new(width_dp: u32, height_dp: u32, scale: f32, client: Box<dyn ProtocolClient>) -> Self {
        let scale = normalize_scale(scale);
        let size_dp = (clamp_dimension(width_dp), clamp_dimension(height_dp));
        let size_px = physical_size(size_dp.0, size_dp.1, scale);
        let capabilities = Capabilities::from_globals(&client.globals());
        Self {
            client,
            ladder: TierLadder::begin(capabilities),
            capabilities,
            pool: BufferPool::new(size_px.0, size_px.1),
            size_dp,
            size_px,
            scale,
            position: (0, 0),
            caret: RectI {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            output: None,
            input_region: Vec::new(),
            pending_slot: None,
            next_token: 0,
            pending_frame: None,
            protocol_errors: 0,
            focus_events: 0,
            visible: false,
            created: false,
            start: Instant::now(),
            wire: Vec::new(),
        }
    }

    /// The tier in use, or [`Tier::Fallback`] when none is.
    pub fn tier(&self) -> Tier {
        self.ladder.tier()
    }

    /// The protocol capabilities the connection turned out to have.
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// The tiers tried and abandoned, in order.
    pub fn attempts(&self) -> &[TierAttempt] {
        self.ladder.attempts()
    }

    /// When the current tier's configure budget expires, as elapsed time since construction.
    ///
    /// `None` once a tier is confirmed. This is what the UI thread arms its `timerfd` with;
    /// there is no other timer, and a backend whose budget is unarmed -- one that has not been
    /// placed yet -- never times out.
    pub fn deadline(&self) -> Option<Duration> {
        self.ladder.deadline()
    }

    /// The connection's file descriptor, for the UI thread's `poll(2)` loop.
    pub fn connection_fd(&self) -> RawFd {
        self.client.connection_fd()
    }

    /// The outputs the connection knows about.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.client.outputs()
    }

    /// The detection results the caller reports as diagnostics.
    pub fn diagnostics(&self) -> WaylandDiagnostics {
        WaylandDiagnostics {
            tier: self.ladder.tier(),
            compositor: CompositorKind::from_capabilities(&self.capabilities),
            shaped_input_region: self.capabilities.input_region,
            pool_bytes: self.pool.pool_bytes(),
            starvation_episodes: self.pool.episodes(),
            worst_starvation: self.pool.worst_run(),
            protocol_errors: self.protocol_errors,
            focus_events: self.focus_events,
        }
    }

    /// Places the window, creating the surface the first time.
    ///
    /// The first call creates the tier's surface and starts the configure budget; every call
    /// applies the placement, so a caret that moves while the window is visible costs one
    /// request round rather than a rebuild.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when the ladder found no usable tier, and
    /// whatever [`ProtocolClient::create_surface`] and the placement report otherwise.
    pub fn place(&mut self, placement: WindowPlacement) -> Result<(), PlatformError> {
        if self.ladder.tier() == Tier::Fallback {
            return Err(PlatformError::Unavailable);
        }
        self.position = placement.top_left;
        self.caret = placement.caret;
        self.output = Some(placement.output);
        if !self.created {
            let tier = self.ladder.tier();
            let output = self.output.clone();
            self.client.create_surface(tier, output.as_ref())?;
            self.client.set_buffer_scale(buffer_scale(self.scale))?;
            self.client.resize_pool(self.size_px.0, self.size_px.1)?;
            self.created = true;
            // The budget starts now: until the surface existed there was nothing for a
            // configure to configure.
            self.ladder.arm(self.start.elapsed());
        }
        self.apply_placement()?;
        self.apply_input_region()?;
        self.client.flush()
    }

    /// The parent space the popup tiers place in, or a plain origin before anything is placed.
    fn parent_space(&self) -> ParentSpace {
        match &self.output {
            Some(output) => ParentSpace::for_output(output),
            None => ParentSpace::new((0, 0), self.scale),
        }
    }

    /// Adopts a new size, in surface-local units; reports whether anything changed.
    fn resize(&mut self, size_dp: (u32, u32)) -> bool {
        let size_dp = (clamp_dimension(size_dp.0), clamp_dimension(size_dp.1));
        let size_px = physical_size(size_dp.0, size_dp.1, self.scale);
        if size_dp == self.size_dp && size_px == self.size_px {
            return false;
        }
        self.size_dp = size_dp;
        self.size_px = size_px;
        // Recorded rather than applied: the pool recreates its buffers at the first moment
        // both slots are back, so nothing the compositor is still reading is torn down.
        self.pool.request_resize(size_px.0, size_px.1);
        true
    }

    /// Gives the connection the buffer size the pool has just adopted.
    ///
    /// The pool can only adopt once both slots are back, so a resize that arrives while a
    /// frame is in flight waits here for the next acquire or commit.
    fn sync_pool(&mut self) -> Result<(), PlatformError> {
        if !self.pool.has_pending() || !self.pool.adopt_pending() {
            return Ok(());
        }
        self.client
            .resize_pool(self.pool.width_px(), self.pool.height_px())
    }

    /// Pushes the placement for the current tier to the connection.
    fn apply_placement(&mut self) -> Result<(), PlatformError> {
        if !self.created {
            return Ok(());
        }
        let Some(output) = self.output.clone() else {
            return Ok(());
        };
        match self.ladder.tier() {
            Tier::LayerShell => {
                let request = LayerRequest::new(self.position, self.size_dp, &output, self.scale);
                self.client.configure_layer(&request)
            }
            Tier::Popup => {
                let space = ParentSpace::for_output(&output);
                let request = PopupRequest::below_caret(self.caret, space, self.size_dp);
                self.client.configure_popup(&request)
            }
            Tier::CanvasPopup => {
                // Tier 3 places the window itself, so the anchor rectangle carries the
                // position rather than the caret.
                let rect =
                    canvas_popup::anchor_rect(self.position, self.size_px, &output, self.scale);
                let request = PopupRequest::at(rect, self.size_dp);
                self.client.configure_popup(&request)
            }
            Tier::Fallback => Ok(()),
        }
    }

    /// Pushes the stored interactive region to the connection.
    fn apply_input_region(&mut self) -> Result<(), PlatformError> {
        if !self.created {
            return Ok(());
        }
        let clipped = clip_rects(&self.input_region, self.size_px.0, self.size_px.1);
        let rects: Vec<SurfaceRect> = clipped
            .iter()
            .map(|rect| SurfaceRect::from_physical(*rect, (0, 0), self.scale))
            .collect();
        self.client.set_input_region(&rects)
    }

    /// Hides the surface if it is showing.
    fn hide(&mut self) -> Result<(), PlatformError> {
        if !self.visible {
            return Ok(());
        }
        self.visible = false;
        self.client.set_visible(false)
    }

    /// Rebuilds the surface for a new tier.
    ///
    /// The buffers are not recreated: they belong to the connection rather than to the
    /// surface, so the new one attaches the same two. They are released first, because the
    /// surface they were attached to no longer exists and the compositor cannot still be
    /// reading them -- and because a release that never came would starve every later frame.
    fn rebuild(&mut self, tier: Tier) -> Result<(), PlatformError> {
        if !self.created {
            // Nothing has been placed yet, so there is no surface to rebuild; the next
            // placement creates one for the tier the ladder has moved to.
            return Ok(());
        }
        self.pool.release_all();
        let output = self.output.clone();
        self.client.create_surface(tier, output.as_ref())?;
        self.client.set_buffer_scale(buffer_scale(self.scale))?;
        self.apply_placement()?;
        self.apply_input_region()?;
        if self.visible {
            self.client.set_visible(true)?;
        }
        self.client.flush()
    }

    /// Acts on what the ladder decided.
    fn apply_step(&mut self, step: LadderStep) -> Result<(), PlatformError> {
        match step {
            LadderStep::Pending | LadderStep::Confirmed(_) => Ok(()),
            LadderStep::Promote { next, .. } => self.rebuild(next),
            LadderStep::Fallback { .. } => {
                // No tier answered. Take the window off screen and leave the tier on the
                // fallback, which is the caller's signal to register no user interface at all
                // and let the host draw the candidate list.
                self.hide()
            }
        }
    }

    /// Acts on one protocol event.
    fn react(
        &mut self,
        event: &WireEvent,
        out: &mut Vec<SurfaceEvent>,
    ) -> Result<(), PlatformError> {
        match *event {
            WireEvent::SurfaceEnter { scale } => self.adopt_scale(scale, out)?,
            WireEvent::LayerConfigure { width, height } => {
                let (width_dp, height_dp) =
                    layer_shell::adopt_configure(self.size_dp, (width, height));
                if self.resize((width_dp, height_dp)) {
                    // Reported in physical pixels, as the X11 backend reports a configure.
                    out.push(SurfaceEvent::Resize {
                        w: self.size_px.0,
                        h: self.size_px.1,
                    });
                }
            }
            WireEvent::PopupConfigure {
                x,
                y,
                width,
                height,
            } => {
                // The compositor's answer is the one that counts: it has applied the
                // constraint adjustment, so the geometry we asked for is discarded.
                let geometry = PopupGeometry::adopt(x, y, (width, height), self.parent_space());
                self.position = geometry.top_left;
                if self.resize(geometry.size_dp) {
                    out.push(SurfaceEvent::Resize {
                        w: self.size_px.0,
                        h: self.size_px.1,
                    });
                }
            }
            WireEvent::BufferReleased { slot } => self.pool.release(usize::from(slot)),
            WireEvent::FrameDone { data } => {
                if self.pending_frame == Some(FrameToken(data)) {
                    self.pending_frame = None;
                }
            }
            WireEvent::KeyboardEnter => {
                self.focus_events = self.focus_events.saturating_add(1);
            }
            WireEvent::DisplayError => {
                self.protocol_errors = self.protocol_errors.saturating_add(1);
            }
            WireEvent::LayerClosed | WireEvent::PopupDone => {
                // The compositor took the window away: the user clicked outside a popup, or
                // the output a layer surface was on has gone. The vocabulary has no separate
                // "dismissed" event, so it arrives as `CloseRequested` -- which for a
                // candidate window means "hide", never "quit".
                self.hide()?;
                out.push(SurfaceEvent::CloseRequested);
            }
            WireEvent::PointerEnter { .. }
            | WireEvent::PointerLeave
            | WireEvent::PointerMotion { .. }
            | WireEvent::PointerButton { .. }
            | WireEvent::PointerAxis { .. } => {
                if let Some(surface) = events::pointer_event(event, self.scale) {
                    out.push(surface);
                }
            }
        }
        Ok(())
    }

    /// Adopts the device pixel ratio of the output the surface moved onto.
    fn adopt_scale(
        &mut self,
        scale: f32,
        out: &mut Vec<SurfaceEvent>,
    ) -> Result<(), PlatformError> {
        let scale = normalize_scale(scale);
        if scale.to_bits() == self.scale.to_bits() {
            return Ok(());
        }
        self.scale = scale;
        self.client.set_buffer_scale(buffer_scale(scale))?;
        self.resize(self.size_dp);
        out.push(SurfaceEvent::Scale { factor: scale });
        Ok(())
    }

    /// Feeds one poll's events through the ladder and the backend.
    fn absorb(
        &mut self,
        now: Duration,
        wire: &[WireEvent],
        out: &mut Vec<SurfaceEvent>,
    ) -> Result<(), PlatformError> {
        let mut step = LadderStep::Pending;
        for event in wire {
            let outcome = self.ladder.on_event(event, now);
            if matches!(
                outcome,
                LadderStep::Promote { .. } | LadderStep::Fallback { .. }
            ) {
                step = outcome;
            }
            self.react(event, out)?;
        }
        // The rebuild happens after the batch, so an event that belonged to the surface being
        // torn down is applied to the surface that was current when it arrived.
        self.apply_step(step)
    }
}

impl SurfaceBackend for WaylandBackend {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        if !self.created {
            return Err(PlatformError::Unavailable);
        }
        self.sync_pool()?;
        let slot = self.pool.acquire()?;
        let (width, height, stride) = (
            self.pool.width_px(),
            self.pool.height_px(),
            self.pool.stride(),
        );
        self.pending_slot = Some(slot);
        match self.client.buffer_mut(slot) {
            Some(data) => Ok(PixelBufferMut {
                data,
                stride,
                width,
                height,
            }),
            None => {
                self.pending_slot = None;
                self.pool.release(slot);
                Err(PlatformError::NoFreeBuffer)
            }
        }
    }

    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
        self.sync_pool()?;
        // A buffer of the wrong size would be stretched into a blurry frame, so the frame is
        // skipped until the pool has caught up; `NoFreeBuffer` is the contract's "skip this
        // one" answer, and the next acquire adopts the size.
        if (self.pool.width_px(), self.pool.height_px()) != self.size_px {
            return Err(PlatformError::NoFreeBuffer);
        }
        if let Some(slot) = self.pending_slot.take() {
            let frame = FrameCommit {
                slot,
                size_px: self.size_px,
                damage,
            };
            self.client.attach_and_commit(frame)?;
        }
        self.client.flush()
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        self.input_region = rects.to_vec();
        self.apply_input_region()
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        if !visible {
            return self.hide();
        }
        if self.ladder.tier() == Tier::Fallback {
            return Err(PlatformError::Unavailable);
        }
        if !self.created {
            // There is no placement yet, and a surface with no position has nowhere to show.
            return Err(PlatformError::Unavailable);
        }
        if self.visible {
            return Ok(());
        }
        self.client.set_visible(true)?;
        self.visible = true;
        self.client.flush()
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        // Wayland does have frame callbacks, so the UI thread paces animation from the
        // compositor's own clock instead of a timer. At most one may be outstanding per
        // surface, so a second request before the first fires is refused rather than queued.
        if self.pending_frame.is_some() || !self.created {
            return None;
        }
        let token = FrameToken(self.next_token);
        self.next_token = self.next_token.wrapping_add(1);
        if self.client.request_frame(token).is_err() {
            // A callback that cannot be requested costs one animation frame and nothing else.
            // The contract's signature has no error channel for it, so it is counted instead.
            self.protocol_errors = self.protocol_errors.saturating_add(1);
            return None;
        }
        self.pending_frame = Some(token);
        Some(token)
    }

    fn connection_fd(&self) -> Option<BorrowedFd<'_>> {
        // The display's descriptor is what pointer and output events arrive on, so this is
        // the one the UI thread's poll set watches. Borrowed, not owned: the client keeps
        // the descriptor, and the borrow lives only for this call.
        Some(BorrowedFd::borrow_raw(WaylandBackend::connection_fd(self)))
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        let now = self.start.elapsed();
        let step = self.ladder.on_timeout(now);
        self.apply_step(step)?;

        // The scratch buffer is taken out so the events can be walked while `self` is borrowed
        // mutably, and put back so the next poll allocates nothing.
        let mut wire = std::mem::take(&mut self.wire);
        let dispatched = self.client.dispatch(&mut wire);
        let absorbed = self.absorb(now, &wire, out);
        self.wire = wire;
        dispatched?;
        absorbed
    }

    fn geometry(&self) -> (u32, u32, f32) {
        // The logical size the compositor configured, kept as it was reported rather than
        // divided back out of the buffer size, which would round it a second time.
        (self.size_dp.0, self.size_dp.1, self.scale)
    }

    fn backend_id(&self) -> &'static str {
        self.ladder.tier().backend_id()
    }
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows, and a `#[path]` child module keeps them inside `backend`, where
// the private fields they assert on are reachable.
#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
