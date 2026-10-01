//! A display-free [`SurfaceBackend`] for this crate's tests.
//!
//! It exists so the renderer, the damage bookkeeping and the platform's geometry handling
//! can be asserted without a display server, and so the same call sequence the real
//! backends get can be replayed against a backend that records what it was asked to do.
//! Everything a test observes lives behind an `Arc<Mutex<_>>` the test keeps a handle on,
//! because the backend itself is moved into the platform and cannot be reached afterwards.

use std::sync::{Arc, Mutex, MutexGuard};

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};

use super::raster::BYTES_PER_PIXEL;

/// Everything a test can observe about a [`MockSurface`].
#[derive(Default)]
pub(crate) struct MockState {
    /// The last committed frame, copied out of the draw buffer.
    pub(crate) pixels: Vec<u8>,
    /// How many frames were committed.
    pub(crate) commits: usize,
    /// The damage every commit reported, concatenated in commit order.
    pub(crate) damage: Vec<RectI>,
    /// How many draw buffers were handed out.
    pub(crate) acquired: usize,
    /// Whether the surface is mapped.
    pub(crate) visible: bool,
    /// The last interactive region that was set.
    pub(crate) region: Vec<RectI>,
    /// Events the next `poll_events` call delivers.
    pub(crate) pending: Vec<SurfaceEvent>,
    /// How many of the next `acquire_buffer` calls fail with `NoFreeBuffer`.
    pub(crate) starve: usize,
}

impl MockState {
    /// One pixel of the last committed frame, as the bytes `[blue, green, red, alpha]`.
    ///
    /// `stride` is the row length in bytes, which is the surface width times
    /// [`BYTES_PER_PIXEL`].
    pub(crate) fn pixel(&self, stride: usize, x: usize, y: usize) -> [u8; 4] {
        let at = y * stride + x * BYTES_PER_PIXEL;
        let mut pixel = [0u8; 4];
        pixel.copy_from_slice(&self.pixels[at..at + BYTES_PER_PIXEL]);
        pixel
    }
}

/// A `SurfaceBackend` that needs no display server.
///
/// It double buffers like the real backends do, so a test can tell a copy that covers the
/// whole frame from one that only covers the region the frame redrew.
pub(crate) struct MockSurface {
    state: Arc<Mutex<MockState>>,
    /// The two draw buffers, `Argb8888`, `width_px * 4` bytes per row.
    buffers: [Vec<u8>; 2],
    /// Which buffer `acquire_buffer` hands out next.
    back: usize,
    width_px: u32,
    height_px: u32,
    width_dp: u32,
    height_dp: u32,
    scale: f32,
}

impl MockSurface {
    /// Creates a surface of `width_dp * scale` by `height_dp * scale` physical pixels, and
    /// hands back the observation state the test asserts on.
    pub(crate) fn new(width_dp: u32, height_dp: u32, scale: f32) -> (Self, Arc<Mutex<MockState>>) {
        let (width_px, height_px) = crate::platform::physical_size(width_dp, height_dp, scale);
        let state = Arc::new(Mutex::new(MockState::default()));
        let length = crate::platform::buffer_len(width_px, height_px);
        let surface = Self {
            state: Arc::clone(&state),
            buffers: [vec![0; length], vec![0; length]],
            back: 0,
            width_px,
            height_px,
            width_dp,
            height_dp,
            scale,
        };
        (surface, state)
    }

    /// Adopts a physical size, reallocating both buffers.
    ///
    /// The real backends do this when the compositor reports a configure -- `X11Backend`
    /// reallocates in `apply_size`, the Wayland pool adopts a deferred resize -- so the mock
    /// has to as well: a test that resizes the window and then renders would otherwise have
    /// the window and the surface describing two different sizes, and the copy would be
    /// refused by a buffer that is too small for the frame.
    ///
    /// A size the surface already has is not a resize, which is the rule the Wayland pool
    /// states for its own `request_resize`.
    fn adopt_size(&mut self, width_px: u32, height_px: u32) {
        let width_px = crate::platform::clamp_dimension(width_px);
        let height_px = crate::platform::clamp_dimension(height_px);
        if (width_px, height_px) == (self.width_px, self.height_px) {
            return;
        }
        self.width_px = width_px;
        self.height_px = height_px;
        self.width_dp = crate::platform::logical_dimension(width_px, self.scale);
        self.height_dp = crate::platform::logical_dimension(height_px, self.scale);
        let length = crate::platform::buffer_len(width_px, height_px);
        self.buffers = [vec![0; length], vec![0; length]];
        self.back = 0;
    }

    /// Adopts a device pixel ratio, reallocating both buffers around the same canvas.
    ///
    /// The real backends do this when the ratio moves -- `X11Backend` reconfigures its
    /// window in `apply_scale` -- so the mock has to as well: a test that re-scales the
    /// surface and then renders would otherwise have the surface describing a size the
    /// mock's buffers cannot hold, and the copy would be refused by a buffer that is too
    /// small for the frame.
    ///
    /// The logical size is the invariant the ratio only re-expresses: the buffers grow to
    /// the same canvas at the new ratio, exactly as the window itself does.
    ///
    /// Returns the ratio adopted, or `None` when the mock already runs at it.
    fn adopt_scale(&mut self, factor: f32) -> Option<f32> {
        let scale = crate::platform::normalize_scale(factor);
        if scale.to_bits() == self.scale.to_bits() {
            return None;
        }
        self.scale = scale;
        let (width_px, height_px) =
            crate::platform::physical_size(self.width_dp, self.height_dp, scale);
        self.width_px = width_px;
        self.height_px = height_px;
        let length = crate::platform::buffer_len(width_px, height_px);
        self.buffers = [vec![0; length], vec![0; length]];
        self.back = 0;
        Some(scale)
    }

    /// Locks the observation state, reporting a poisoned lock as an unusable backend.
    fn lock(&self) -> Result<MutexGuard<'_, MockState>, PlatformError> {
        self.state.lock().map_err(|_| PlatformError::Unavailable)
    }
}

impl SurfaceBackend for MockSurface {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        let starved = {
            let mut state = self.lock()?;
            if state.starve > 0 {
                state.starve -= 1;
                true
            } else {
                state.acquired += 1;
                false
            }
        };
        if starved {
            return Err(PlatformError::NoFreeBuffer);
        }
        let back = self.back;
        let data = self
            .buffers
            .get_mut(back)
            .map(|buffer| buffer.as_mut_slice())
            .ok_or(PlatformError::Unavailable)?;
        Ok(PixelBufferMut {
            data,
            stride: self.width_px as usize * BYTES_PER_PIXEL,
            width: self.width_px,
            height: self.height_px,
        })
    }

    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
        let back = self.back;
        {
            let mut state = self.lock()?;
            state.commits += 1;
            state.damage.extend_from_slice(damage);
            // Copied out so the test can sample the frame after the backend has been moved
            // into the platform and can no longer be reached.
            state.pixels.clear();
            if let Some(buffer) = self.buffers.get(back) {
                state.pixels.extend_from_slice(buffer);
            }
        }
        self.back = 1 - back;
        Ok(())
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        let mut state = self.lock()?;
        state.region = rects.to_vec();
        Ok(())
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        let mut state = self.lock()?;
        state.visible = visible;
        Ok(())
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        // Stands in for a compositor that has frame callbacks, so the token bookkeeping in
        // the renderer is exercised as well.
        Some(FrameToken(1))
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        // A scale change the surface synthesized from the anchor arrives in `out` itself:
        // adopt it before anything else, then report the ratio actually adopted, which is
        // the same order the X11 backend adopts one in.
        let mut adopted = None;
        for event in out.iter() {
            if let SurfaceEvent::Scale { factor } = *event {
                if let Some(ratio) = self.adopt_scale(factor) {
                    adopted = Some(ratio);
                }
            }
        }
        if let Some(ratio) = adopted {
            out.push(SurfaceEvent::Scale { factor: ratio });
        }
        let pending = {
            let mut state = self.lock()?;
            std::mem::take(&mut state.pending)
        };
        // A size the compositor reported is adopted before the events are handed on, which is
        // the order the real backends do it in: `poll_events` resizes itself first, then
        // reports the size it actually adopted. A ratio the compositor reported is adopted
        // the same way, and is handed on as it arrived: appended below the events the caller
        // already had, so the platform applies it to the window exactly once.
        for event in &pending {
            if let SurfaceEvent::Resize { w, h } = *event {
                self.adopt_size(w, h);
            }
            if let SurfaceEvent::Scale { factor } = *event {
                self.adopt_scale(factor);
            }
        }
        out.extend_from_slice(&pending);
        Ok(())
    }

    fn geometry(&self) -> (u32, u32, f32) {
        (self.width_dp, self.height_dp, self.scale)
    }

    fn backend_id(&self) -> &'static str {
        "mock"
    }
}

/// Runs a scene on a thread of its own and returns what it produced.
///
/// Slint installs its platform per thread and refuses a second one on the same thread, so a
/// scene that needs a platform gets a fresh thread: that keeps the tests independent of how
/// the runner schedules them, and of each other.
///
/// # Panics
///
/// Panics when the scene panics, after the default panic hook has already reported the
/// scene's own message.
pub(crate) fn on_own_thread<R: Send + 'static>(scene: impl FnOnce() -> R + Send + 'static) -> R {
    let thread = std::thread::Builder::new()
        .name(String::from("ime-ui-test"))
        .spawn(scene)
        .expect("the test thread can be spawned");
    thread.join().expect("the scene completes")
}
