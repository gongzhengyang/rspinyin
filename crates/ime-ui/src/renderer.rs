//! Software rasterization of the candidate window onto a [`SurfaceBackend`].
//!
//! Slint's software renderer draws a scene into a typed pixel slice; a surface backend
//! hands out bytes. This module is the bridge between the two: it owns the adapter Slint
//! renders through, the scratch a frame is rasterized into, and the damage bookkeeping
//! that decides what is copied to the surface and what is reported to the compositor.
//!
//! # The render loop
//!
//! Nothing here runs an event loop. The UI thread owns the loop (2.1), calls
//! `render_if_dirty` after `poll(2)` returns, and is the only caller. A frame that cannot
//! be delivered is skipped, never waited for, and a frame that is not dirty never touches
//! the backend at all -- which is what keeps an idle window at zero compositor traffic
//! (`BUDGET-CPU-01`).
//!
//! # Damage and the two surface buffers
//!
//! A backend double buffers, so the buffer `acquire_buffer` hands out holds the frame
//! *before* the one on screen, while the scratch holds the frame that *is* on screen. The
//! copy therefore has to cover two regions -- everything rendered into the scratch since
//! the last successful commit, and the damage of the frame the surface is showing -- while
//! only the former is reported as damage. A frame skipped because no buffer was free keeps
//! accumulating into the first list, which is what makes the copy correct again once a
//! buffer comes back.
//!
//! The two regions are folded into one bounding box and copied in a single pass. They
//! overlap heavily in the steady state -- a keystroke moves the highlight within the cells
//! the previous keystroke damaged -- so copying them one after the other copied the overlap
//! twice. The box is a superset of both, a superset is always safe for a copy, and that is
//! the same argument the pending-list collapse already relies on. Only the copy is merged:
//! what the compositor is told is still this frame's damage alone.
//!
//! Within one frame, the regions the renderer reports are merged as they are recorded, so a
//! frame's damage is a short list of disjoint places rather than one entry per rectangle the
//! renderer happened to emit. An animation is where this pays: the highlight damages the
//! same neighbourhood every frame, and the merge keeps that a handful of rectangles instead
//! of a list that has to be folded -- with a bounding box that is far larger than the
//! motion -- every time it grows past [`PENDING_COLLAPSE_LIMIT`].
//!
//! # The scratch
//!
//! A frame is rasterized into a scratch this crate owns -- the pixel format and the reason
//! for the extra buffer are in `raster` -- and the scratch grows to the largest surface
//! the window has had. That allocation is given back only once the window has stayed small
//! long enough for the new size to be a settled one rather than a drag, and the frame that
//! gives it back is a full repaint, because the smaller buffer no longer holds what the
//! surface is showing.
//!
//! # The licence boundary
//!
//! `SlintWindowAdapter` implements Slint's `WindowAdapter` and is therefore private: an
//! `impl` of a Slint trait is part of a type's public surface, and `ime-ui` may not export
//! any Slint type (Slint Royalty-free 2.0, obligation `OB-4`). [`RenderOutcome`] is the
//! crate's own vocabulary for what a frame produced, and the only thing this module hands
//! across the crate boundary besides the font probe.

mod probe;
mod raster;

#[cfg(test)]
pub(crate) mod mock;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ime_types::{FrameToken, PlatformError, RectI, SurfaceBackend, SurfaceEvent};
use slint::platform::software_renderer::{PhysicalRegion, RepaintBufferType, SoftwareRenderer};
use slint::platform::{Renderer, WindowAdapter, WindowEvent};
use slint::{PhysicalSize, PlatformError as SlintError, Window};

use self::raster::{BYTES_PER_PIXEL, PixelScratch, clip_rect, overlaps, union_pair, union_rect};

pub use self::probe::{CJK_FAMILIES, FontChoice, FontStatus, probe_font_choice, probe_fonts};

/// How many rectangles the pending list may hold before it is collapsed.
///
/// A starvation streak accumulates one region per skipped frame, and copying hundreds of
/// small rectangles costs more than copying their bounding box once. The box is a
/// superset, which is always safe for a copy.
///
/// This is also the point the damage bookkeeping saturates at: past it a frame's pending
/// list is one rectangle however long the streak runs, which is what bounds the fold the
/// copy performs. With the merge below, a list this long is a frame whose damage does not
/// share a pixel with itself -- a scene that changed all over the surface -- because
/// overlapping damage is folded on insert and never accumulates.
pub const PENDING_COLLAPSE_LIMIT: usize = 8;

/// What one call to `render_if_dirty` did.
///
/// The distinction that matters to the UI thread is whether a frame reached the
/// compositor: `Idle` and `Skipped` both leave the surface as it was, and only `Rendered`
/// advances it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    /// Nothing had to be drawn: no buffer was acquired and nothing was committed.
    Idle,
    /// A frame was rasterized and committed.
    Rendered {
        /// The bounding box of the damage, in physical pixels relative to the surface's
        /// top-left corner.
        ///
        /// This is what the frame *changed*, which is what the compositor is told; it is not
        /// the region the copy covered, which is the union of this damage with the previous
        /// frame's.
        bounding: RectI,
        /// How many rectangles the damage was reported as.
        ///
        /// This counts the damage *after* it was merged on insert: regions that share a
        /// pixel are folded into their bounding box as they are recorded, so the number
        /// describes how many separate places the frame changed rather than how many
        /// rectangles the renderer happened to hand over. It is therefore lower than the
        /// count an append-only list reported for the same frame -- a frame's shape, not a
        /// fixed property of the renderer's output -- and a budget that pins it has to name
        /// the frame it is pinning.
        rectangles: u32,
        /// How many copies of the scratch into the surface buffer this frame cost: one for a
        /// frame with anything to carry over, zero for one that had nothing.
        ///
        /// A frame copies at most one region, because the two lists the copy covers are
        /// folded into their bounding box first. The count is what the frame-copy budget is
        /// asserted on: a steady-state keystroke pays one copy rather than the two the
        /// two-list form charged for the same overlapping damage.
        copies: u32,
        /// How many bytes of the surface buffer this frame's copy wrote.
        ///
        /// Zero for a frame that had nothing to carry over, and otherwise the area of the
        /// region the copy covered in pixels times the four bytes of an `Argb8888` pixel.
        /// Where the `copies` field counts the passes and `rectangles` counts the
        /// rectangles the damage was reported as, this is the memory traffic a frame costs,
        /// which is the quantity the merge is about: folding the two lists into one region
        /// is what keeps a steady-state keystroke from paying for their overlap twice.
        copy_bytes: u64,
    },
    /// The backend had no free buffer. The frame stays dirty and is retried on the next
    /// wake-up; the surface is unchanged.
    Skipped,
}

/// The size and scale factor of the surface a window draws into.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SurfaceGeometry {
    /// Logical width in device-independent pixels.
    width_dp: u32,
    /// Logical height in device-independent pixels.
    height_dp: u32,
    /// Device pixel ratio, always finite and positive.
    scale: f32,
}

impl SurfaceGeometry {
    /// The surface size in physical pixels.
    fn physical(self) -> (u32, u32) {
        crate::platform::physical_size(self.width_dp, self.height_dp, self.scale)
    }
}

/// The rasterization state of one window.
struct FrameState {
    /// The buffer a frame is rasterized into before it is copied to the surface.
    scratch: PixelScratch,
    /// Regions rendered into the scratch since the last successful commit, merged as they
    /// are recorded.
    pending: Vec<RectI>,
    /// Regions of the last committed frame: what the surface is showing.
    shown: Vec<RectI>,
    /// Copy and report the whole surface on the next frame.
    full: bool,
}

impl FrameState {
    /// Creates a state that repaints everything on its first frame.
    fn new() -> Self {
        Self {
            scratch: PixelScratch::new(),
            pending: Vec::new(),
            shown: Vec::new(),
            full: true,
        }
    }

    /// Grows or shrinks the scratch to the surface size, scheduling the full repaint a
    /// reallocation needs.
    ///
    /// # Returns
    ///
    /// Whether the scratch was reallocated, which is what tells the caller to hand Slint a
    /// `NewBuffer` for this frame.
    ///
    /// The `full` flag is set here rather than at the call site so that the two cannot
    /// drift apart. A reallocation means the scratch no longer holds the frame the surface
    /// is showing, and a frame that then copied only its own damage would write a *partial*
    /// frame out of content that is no longer valid -- the defect this method exists to make
    /// impossible, and the one a shrink would introduce if it were treated as a mere
    /// bookkeeping change.
    fn resize_scratch(&mut self, width_px: u32, height_px: u32) -> bool {
        let reallocated = self.scratch.ensure(width_px, height_px);
        if reallocated {
            self.full = true;
        }
        reallocated
    }

    /// Records what this frame changed, merging into the list as it goes.
    ///
    /// The previous form appended every region and only folded the list once it grew past
    /// [`PENDING_COLLAPSE_LIMIT`], which left an animation -- where the motion damages the
    /// same neighbourhood frame after frame -- carrying a list of separate rectangles into
    /// both the copy and the report to the compositor. Merging on insert keeps the list
    /// short by construction, and the bounding box of two overlapping regions is a superset,
    /// so the copy stays correct.
    ///
    /// # Why the region still comes from the renderer
    ///
    /// The motion layer knows how far the highlight moved, so it could in principle predict
    /// the damage without asking the renderer. It deliberately does not: the renderer's
    /// region is what was actually drawn, and a prediction that disagreed with it would be a
    /// correctness risk taken in exchange for understanding Slint's own repaint boundaries,
    /// which are a moving target. What this method bounds is the *cost* of the region the
    /// renderer reports, not where that region comes from.
    fn record_damage(&mut self, region: &PhysicalRegion, width_px: u32, height_px: u32) {
        let rects = region
            .iter()
            .map(|(position, size)| RectI {
                x: position.x,
                y: position.y,
                w: size.width,
                h: size.height,
            })
            .collect::<Vec<_>>();
        self.record_damage_rects(&rects, width_px, height_px);
    }

    /// Keeps the damage list short, given the rectangles the renderer reported.
    ///
    /// Split from [`Self::record_damage`] because a [`PhysicalRegion`] cannot be built
    /// outside the renderer -- its fields are private and the only constructor is
    /// `SoftwareRenderer::render`. That makes the *source* of the region untestable by
    /// construction and the *bookkeeping* over it entirely testable, and the bookkeeping is
    /// where the merging rules live. The split is the difference between a rule that can be
    /// pinned by a test and one that can only be checked by watching a window.
    fn record_damage_rects(&mut self, rects: &[RectI], width_px: u32, height_px: u32) {
        if self.full {
            self.pending.clear();
            let whole = RectI {
                x: 0,
                y: 0,
                w: width_px,
                h: height_px,
            };
            if let Some(rect) = clip_rect(whole, width_px, height_px) {
                self.pending.push(rect);
            }
            return;
        }
        for rect in rects {
            if let Some(clipped) = clip_rect(*rect, width_px, height_px) {
                self.merge_into_pending(clipped);
            }
        }
        self.collapse_pending();
    }

    /// Adds `rect` to the pending list, merging it into an overlapping entry when one
    /// exists.
    ///
    /// Merging is a bounding-box union rather than a rectangle subtraction: the result is a
    /// superset, which is safe for a copy, and it keeps the list short without allocating,
    /// which the per-frame path may not do.
    ///
    /// Only the first overlap is merged. Growing an entry can make it overlap a later one,
    /// and that is left to [`Self::collapse_pending`]: rescanning the list after every
    /// insert would cost more than the extra rectangle it might remove, and the list is
    /// bounded either way.
    fn merge_into_pending(&mut self, rect: RectI) {
        for slot in self.pending.iter_mut() {
            if overlaps(*slot, rect) {
                *slot = union_pair(*slot, rect);
                return;
            }
        }
        self.pending.push(rect);
    }

    /// Folds the pending list into its bounding box once it has grown past
    /// [`PENDING_COLLAPSE_LIMIT`].
    ///
    /// Kept apart from [`Self::record_damage`] so that the saturation point can be asserted
    /// without a Slint region to feed it: the collapse is what bounds the fold the copy
    /// performs, however many rectangles a starvation streak has accumulated. With the merge
    /// above, a list this long means the frame damaged that many regions which do not share
    /// a pixel -- a streak of skipped frames, or a scene that changed all over.
    fn collapse_pending(&mut self) {
        if self.pending.len() > PENDING_COLLAPSE_LIMIT {
            let bounding = union_rect(&self.pending);
            self.pending.clear();
            self.pending.push(bounding);
        }
    }
}

/// Copies everything one committed frame has to carry over into its surface buffer.
///
/// The two lists are folded into a single bounding box, so a frame costs one copy however
/// many rectangles it damaged. The box is a superset of both lists, which is safe for a copy
/// because every pixel outside them is the same in the scratch and in the buffer being
/// written -- that is what the induction the module documentation describes rests on.
///
/// # Parameters
///
/// * `state` -- the frame state whose scratch holds the frame that is on screen and whose
///   two damage lists say what the acquired buffer is missing.
/// * `dst`, `dst_stride` -- the acquired surface buffer and its row length in bytes.
/// * `width_px`, `height_px` -- the surface size the region is clipped to.
///
/// # Returns
///
/// The region that was copied, or `None` when neither list held anything inside the surface.
///
/// # Errors
///
/// Returns [`PlatformError::Unavailable`] when the destination buffer is too short for the
/// region, which is a backend defect: the copy is refused rather than clamped.
fn copy_frame(
    state: &FrameState,
    dst: &mut [u8],
    dst_stride: usize,
    width_px: u32,
    height_px: u32,
) -> Result<Option<RectI>, PlatformError> {
    let Some(bounds) = copy_bounds(&state.pending, &state.shown, width_px, height_px) else {
        return Ok(None);
    };
    state.scratch.blit_into(dst, dst_stride, bounds)?;
    Ok(Some(bounds))
}

/// The bounding box of everything one committed frame has to copy.
///
/// Written as an explicit fold over both lists rather than by building a combined one: the
/// per-frame path may not allocate, and both lists are already in the state the caller owns.
/// Every rectangle is clipped first, so the box is inside the surface and
/// [`PixelScratch::blit_into`] is handed a region it can refuse rather than clamp.
fn copy_bounds(pending: &[RectI], shown: &[RectI], width_px: u32, height_px: u32) -> Option<RectI> {
    let mut bounds: Option<RectI> = None;
    for rect in pending.iter().chain(shown.iter()) {
        let Some(clipped) = clip_rect(*rect, width_px, height_px) else {
            continue;
        };
        bounds = Some(match bounds {
            None => clipped,
            Some(current) => union_pair(current, clipped),
        });
    }
    bounds
}

/// How many bytes one copy region costs: its area in pixels times the four bytes of an
/// `Argb8888` pixel.
///
/// This is the per-frame figure the frame-copy budget is measured on. It is derived from the
/// region the copy was handed rather than accumulated inside the copy loop, which would be a
/// per-pixel addition on the hot path; the two agree because [`PixelScratch::blit_into`]
/// writes exactly the region it is given and refuses one it cannot cover in full rather than
/// writing part of it.
fn region_bytes(rect: RectI) -> u64 {
    u64::from(rect.w) * u64::from(rect.h) * BYTES_PER_PIXEL as u64
}

/// The `WindowAdapter` Slint draws the candidate window through.
///
/// It is `pub(crate)` and never exported: the trait it implements belongs to Slint, and a
/// public `impl` of a Slint trait would put the Slint API into `ime-ui`'s public surface.
pub(crate) struct SlintWindowAdapter {
    window: Window,
    renderer: SoftwareRenderer,
    /// The one surface this window draws into, shared with the platform so the UI thread
    /// can poll it and shape it without going through the window.
    backend: Rc<RefCell<Box<dyn SurfaceBackend>>>,
    /// Rasterization target and damage bookkeeping. Interior mutability because every
    /// `WindowAdapter` method takes `&self`.
    frame: RefCell<FrameState>,
    /// The frame callback token of the last committed frame.
    pending_frame: Cell<Option<FrameToken>>,
    /// Physical size and scale of the surface.
    geometry: Cell<SurfaceGeometry>,
    /// Frames skipped in a row because no buffer was free.
    starved: Cell<u32>,
    /// Frames committed since the window was created.
    committed: Cell<u64>,
}

impl SlintWindowAdapter {
    /// Creates the adapter that draws into `backend`.
    ///
    /// The surface size comes from the backend and is told to Slint right away, because
    /// attaching a component derives the root item's geometry from the window size and the
    /// scale factor.
    ///
    /// # Errors
    ///
    /// Returns the Slint error of a failed initial window-event dispatch, which in
    /// practice means the window could not be prepared for the scene at all.
    pub(crate) fn new(
        backend: Rc<RefCell<Box<dyn SurfaceBackend>>>,
    ) -> Result<Rc<Self>, SlintError> {
        let (width_dp, height_dp, scale) = backend.borrow().geometry();
        let geometry = SurfaceGeometry {
            width_dp: crate::platform::clamp_dimension(width_dp),
            height_dp: crate::platform::clamp_dimension(height_dp),
            scale: crate::platform::normalize_scale(scale),
        };
        // Both the binding and the closure parameter are annotated: `Window::new` wants a
        // `Weak<dyn WindowAdapter>`, and without pinning `new_cyclic`'s type parameter the
        // compiler satisfies that coercion by making the parameter itself the trait object,
        // which cannot be unsized into a value.
        let adapter: Rc<SlintWindowAdapter> =
            Rc::new_cyclic(move |weak: &std::rc::Weak<SlintWindowAdapter>| {
                let window_weak: std::rc::Weak<dyn WindowAdapter> = weak.clone();
                Self {
                    window: Window::new(window_weak),
                    renderer: SoftwareRenderer::new_with_repaint_buffer_type(
                        RepaintBufferType::ReusedBuffer,
                    ),
                    backend,
                    frame: RefCell::new(FrameState::new()),
                    pending_frame: Cell::new(None),
                    geometry: Cell::new(geometry),
                    starved: Cell::new(0),
                    committed: Cell::new(0),
                }
            });
        adapter
            .window
            .try_dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: geometry.scale,
            })?;
        Ok(adapter)
    }

    /// Rasterizes the scene into the surface, if anything is dirty.
    ///
    /// This is the whole render loop as the UI thread sees it: it does nothing when Slint
    /// has no dirty state, and it never waits for the compositor.
    ///
    /// # Errors
    ///
    /// Propagates a backend failure. A backend with no free buffer is *not* an error: the
    /// frame is reported as [`RenderOutcome::Skipped`] and stays dirty.
    pub(crate) fn render_if_dirty(&self) -> Result<RenderOutcome, PlatformError> {
        // Rasterizes unconditionally, because Slint's own repaint buffer is the dirty
        // oracle: a property the scene reads reaches the renderer without anyone calling
        // `request_redraw` -- that is the component-level path the highlight animation
        // drives -- and `render` answers an empty region for a clean scene, which the
        // empty damage list below turns into `Idle`. What `full` on the frame state
        // still owns are the repaints Slint cannot see -- the remap and the resize --
        // and those commit the whole surface.
        let (width_px, height_px) = self.geometry.get().physical();
        let mut frame = self.frame.borrow_mut();
        let state = &mut *frame;
        let region = self.rasterize(state, width_px, height_px);
        state.record_damage(&region, width_px, height_px);
        if state.pending.is_empty() {
            // Slint marked the window dirty but nothing on it changed.
            return Ok(RenderOutcome::Idle);
        }
        self.commit(state, width_px, height_px)
    }

    /// Copies what the frame has to carry over into a surface buffer and commits it.
    ///
    /// `width_px` and `height_px` are the surface the frame was rasterized for, which is what
    /// the copy region is clipped to.
    ///
    /// # Errors
    ///
    /// Propagates a backend failure other than "no buffer is free".
    fn commit(
        &self,
        state: &mut FrameState,
        width_px: u32,
        height_px: u32,
    ) -> Result<RenderOutcome, PlatformError> {
        let mut backend = self.backend.borrow_mut();
        let buffer = match backend.acquire_buffer() {
            Ok(buffer) => buffer,
            Err(PlatformError::NoFreeBuffer) => {
                // The frame stays dirty so the next wake-up retries it; what wakes the UI
                // thread is the compositor releasing the buffer it is holding, and until
                // then Slint's own repaint buffer still considers the scene undrawn.
                self.starved.set(self.starved.get().saturating_add(1));
                return Ok(RenderOutcome::Skipped);
            }
            Err(error) => return Err(error),
        };
        // One copy over the union of both lists rather than one per rectangle: the lists
        // overlap in the steady state, and the union is a superset of what either needs.
        let copied = copy_frame(state, buffer.data, buffer.stride, width_px, height_px)?;
        // Only this frame's damage is new to the compositor; the previous frame's was
        // reported when it was committed, so the union must not be sent in its place.
        backend.commit(&state.pending)?;
        std::mem::swap(&mut state.shown, &mut state.pending);
        state.pending.clear();
        state.full = false;
        self.starved.set(0);
        self.committed.set(self.committed.get().saturating_add(1));
        if let Some(token) = backend.request_frame() {
            self.pending_frame.set(Some(token));
        }
        Ok(RenderOutcome::Rendered {
            bounding: union_rect(&state.shown),
            rectangles: state.shown.len().min(u32::MAX as usize) as u32,
            copies: u32::from(copied.is_some()),
            copy_bytes: copied.map_or(0, region_bytes),
        })
    }

    /// Rasterizes the scene into the scratch and returns the region that changed.
    fn rasterize(&self, state: &mut FrameState, width_px: u32, height_px: u32) -> PhysicalRegion {
        // A reallocation in either direction -- a growth, or the shrink a window that has
        // stayed small long enough is given back -- invalidates the frame the surface is
        // showing, and `resize_scratch` has already scheduled the full repaint that goes
        // with it.
        let reallocated = state.resize_scratch(width_px, height_px);
        if reallocated {
            // `NewBuffer` tells the renderer to ignore its partial-rendering cache for
            // exactly this frame.
            self.renderer
                .set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        }
        let stride = state.scratch.stride;
        let region = self.renderer.render(&mut state.scratch.pixels[..], stride);
        if reallocated {
            self.renderer
                .set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
        }
        region
    }

    /// Applies a surface event that changes the window's size or scale.
    ///
    /// Returns whether the event was one of those. Pointer events belong to the UI thread
    /// and are left untouched.
    ///
    /// # Errors
    ///
    /// Returns the Slint error of a failed window-event dispatch.
    pub(crate) fn apply_geometry_event(&self, event: SurfaceEvent) -> Result<bool, SlintError> {
        match event {
            SurfaceEvent::Resize { w, h } => {
                self.apply_resize(w, h)?;
                Ok(true)
            }
            SurfaceEvent::Scale { factor } => {
                self.apply_scale(factor)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Adopts a surface size reported in physical pixels.
    ///
    /// The backend reports what the compositor allowed, which may be smaller than what the
    /// layout asked for, so the window follows the surface rather than the other way
    /// round.
    fn apply_resize(&self, width_px: u32, height_px: u32) -> Result<(), SlintError> {
        let mut geometry = self.geometry.get();
        geometry.width_dp = crate::platform::logical_dimension(width_px, geometry.scale);
        geometry.height_dp = crate::platform::logical_dimension(height_px, geometry.scale);
        self.adopt_geometry(geometry);
        let logical = PhysicalSize::new(width_px, height_px).to_logical(geometry.scale);
        self.window
            .try_dispatch_event(WindowEvent::Resized { size: logical })
    }

    /// Adopts a scale factor reported by the surface.
    fn apply_scale(&self, factor: f32) -> Result<(), SlintError> {
        let mut geometry = self.geometry.get();
        geometry.scale = crate::platform::normalize_scale(factor);
        self.adopt_geometry(geometry);
        self.window
            .try_dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: geometry.scale,
            })
    }

    /// Records a new surface geometry and schedules a full repaint.
    fn adopt_geometry(&self, geometry: SurfaceGeometry) {
        self.geometry.set(geometry);
        let mut frame = self.frame.borrow_mut();
        frame.full = true;
    }

    /// The frame callback token of the last committed frame, until it is redeemed.
    pub(crate) fn pending_frame(&self) -> Option<FrameToken> {
        self.pending_frame.get()
    }

    /// Frames committed since the window was created.
    pub(crate) fn committed(&self) -> u64 {
        self.committed.get()
    }

    /// Frames skipped in a row because no draw buffer was free.
    pub(crate) fn starvation_streak(&self) -> u32 {
        self.starved.get()
    }
}

impl WindowAdapter for SlintWindowAdapter {
    fn window(&self) -> &Window {
        &self.window
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn size(&self) -> PhysicalSize {
        let (width_px, height_px) = self.geometry.get().physical();
        PhysicalSize::new(width_px, height_px)
    }

    fn set_visible(&self, visible: bool) -> Result<(), SlintError> {
        self.backend
            .borrow_mut()
            .set_visible(visible)
            .map_err(slint_error)?;
        if visible {
            // The surface was unmapped, so what its buffers hold is undefined: the next
            // frame repaints all of it.
            let mut frame = self.frame.borrow_mut();
            frame.full = true;
        }
        Ok(())
    }

    fn request_redraw(&self) {
        // Nothing to do here, by design. The UI thread rasterizes after `poll(2)` returns
        // whether or not anyone asked, because `render_if_dirty` trusts Slint's own
        // repaint buffer to answer an empty region for a clean scene; a request need not
        // block, wake anything or set a flag for a render that is already scheduled.
    }
}

/// Renders a backend failure as the Slint platform error of the same meaning.
///
/// The stable `domain/action/reason` code travels in the message, so a diagnostic that
/// records the Slint error still carries the code the tests and probes match on.
fn slint_error(error: PlatformError) -> SlintError {
    SlintError::Other(error.to_string())
}

#[cfg(test)]
// Inline rather than in `renderer/tests.rs`, and the split is by what a test needs: this one
// covers the damage bookkeeping on its own -- no component, no platform, no fixture -- so it
// needs nothing the scene tests bring with them.
mod collapse_tests {
    use ime_types::RectI;

    use super::{FrameState, PENDING_COLLAPSE_LIMIT, union_rect};

    /// The `step`-th rectangle of a starvation streak: two pixels wide, four apart, so a
    /// bounding box of them is a shape the assertions can state by hand.
    fn streak_rect(step: usize) -> RectI {
        RectI {
            x: step as i32 * 4,
            y: 0,
            w: 2,
            h: 2,
        }
    }

    #[test]
    fn test_collapse_pending_past_the_limit_folds_the_list_to_its_bounding_box() {
        let mut state = FrameState::new();
        for step in 0..PENDING_COLLAPSE_LIMIT {
            state.pending.push(streak_rect(step));
        }
        state.collapse_pending();
        assert_eq!(
            state.pending.len(),
            PENDING_COLLAPSE_LIMIT,
            "a list at the limit is left as the streak left it"
        );
        state.pending.push(streak_rect(PENDING_COLLAPSE_LIMIT));
        let bounding = union_rect(&state.pending);
        state.collapse_pending();
        assert_eq!(
            state.pending,
            vec![bounding],
            "one rectangle past the limit folds the list into its bounding box"
        );
        assert_eq!(
            bounding,
            RectI {
                // The streak runs to the ninth rectangle, whose right edge is at 8 * 4 + 2.
                x: 0,
                y: 0,
                w: 34,
                h: 2
            },
            "and the box covers every rectangle the streak accumulated"
        );
    }

    #[test]
    fn test_collapse_pending_inside_the_limit_leaves_the_list_alone() {
        // The boundary the fold must not trip over: a frame that damaged nothing has no box
        // to fold into, and inventing one would report damage the frame never produced.
        let mut state = FrameState::new();
        state.collapse_pending();
        assert!(state.pending.is_empty(), "an empty list stays empty");
        state.pending.push(streak_rect(0));
        state.collapse_pending();
        assert_eq!(
            state.pending,
            vec![streak_rect(0)],
            "a single rectangle is its own bounding box"
        );
    }
}
