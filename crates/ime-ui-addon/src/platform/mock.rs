//! A display-free stand-in for a window backend, for the tests of this module and of the
//! addon lifecycle.
//!
//! It exists so that the platform and window code can be driven on a machine with no
//! display server, which is what the project's test discipline requires: the tests here
//! must pass in a container with no `$DISPLAY` and no compositor.
//!
//! It is deliberately a *faithful* stand-in rather than a recording one. A frame reaches
//! the observation state the way it reaches a real surface — through
//! [`SurfaceBackend::acquire_buffer`] and [`SurfaceBackend::commit`] — so a test that reads
//! the published pixels is asserting what the renderer actually drew, not what a mock was
//! told. That is what makes "this frame differs from the previous one" a real assertion:
//! a window that draws once and then never again leaves the two frames identical.

use std::sync::{Arc, Mutex, MutexGuard};

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};

/// Bytes per pixel of the `Argb8888` format the contract fixes.
const BYTES_PER_PIXEL: u32 = 4;

/// What the surface did, as the test observes it.
///
/// Every field is read by a test: the pixels through [`Self::checksum`] and
/// [`Self::painted_pixels`], the rest directly.
#[derive(Debug)]
pub(crate) struct MockState {
    /// The last frame that was committed, `Argb8888`, `width * 4` bytes per row.
    pub(crate) pixels: Vec<u8>,
    /// How many frames were committed.
    pub(crate) commits: usize,
    /// Whether the surface is mapped.
    pub(crate) visible: bool,
    /// The interactive region last applied; the panel, never the shadow reserve around it.
    pub(crate) region: Vec<RectI>,
    /// Events the next `poll_events` hands back, as a compositor would.
    pub(crate) pending: Vec<SurfaceEvent>,
}

impl MockState {
    /// A fingerprint of the last committed frame.
    ///
    /// A checksum rather than the pixels themselves, so an assertion about "this frame
    /// differs from the last" does not have to hold two full surfaces in the test. The
    /// algorithm is FNV-1a, which is stable and needs no dependency.
    pub(crate) fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in &self.pixels {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// How many pixels of the last committed frame carry a non-zero alpha.
    ///
    /// The check that a frame was *painted* rather than merely committed: the window is
    /// mostly transparent reserve, so a count of zero means the surface was handed a frame
    /// and drew none of it.
    pub(crate) fn painted_pixels(&self) -> usize {
        self.pixels
            .chunks_exact(BYTES_PER_PIXEL as usize)
            .filter(|pixel| pixel[3] != 0)
            .count()
    }
}

/// A window backend that draws nowhere.
pub(crate) struct MockBackend {
    /// The buffer the caller writes; published on commit.
    back: Vec<u8>,
    /// What the test observes.
    state: Arc<Mutex<MockState>>,
    /// Logical size, which is what the surface is created with.
    width_dp: u32,
    /// Logical height.
    height_dp: u32,
    /// Device pixel ratio.
    scale: f32,
    /// Surface width in physical pixels.
    width_px: u32,
    /// Surface height in physical pixels.
    height_px: u32,
}

impl MockBackend {
    /// Builds a backend and the state the test reads it through.
    ///
    /// The two halves are handed out together because the backend is moved into the UI
    /// thread while the state stays with the test: that is the only way to observe a
    /// surface another thread owns.
    pub(crate) fn new(width_dp: u32, height_dp: u32, scale: f32) -> (Self, Arc<Mutex<MockState>>) {
        let width_px = physical(width_dp, scale);
        let height_px = physical(height_dp, scale);
        let len = width_px as usize * BYTES_PER_PIXEL as usize * height_px as usize;
        let state = Arc::new(Mutex::new(MockState {
            pixels: vec![0; len],
            commits: 0,
            visible: false,
            region: Vec::new(),
            pending: Vec::new(),
        }));
        let backend = Self {
            back: vec![0; len],
            state: Arc::clone(&state),
            width_dp,
            height_dp,
            scale,
            width_px,
            height_px,
        };
        (backend, state)
    }
}

/// Borrows a mock's state, recovering the contents of a poisoned lock.
///
/// A test that panicked while holding it has already failed; the state itself is a plain
/// buffer, so a later assertion reading it is better than a second panic from the lock.
pub(crate) fn lock_state(state: &Mutex<MockState>) -> MutexGuard<'_, MockState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One physical dimension of the surface, never zero.
fn physical(dp: u32, scale: f32) -> u32 {
    let px = (dp as f32 * scale).round();
    if px.is_finite() && px >= 1.0 {
        px as u32
    } else {
        1
    }
}

impl SurfaceBackend for MockBackend {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        Ok(PixelBufferMut {
            data: &mut self.back,
            stride: self.width_px as usize * BYTES_PER_PIXEL as usize,
            width: self.width_px,
            height: self.height_px,
        })
    }

    fn commit(&mut self, _damage: &[RectI]) -> Result<(), PlatformError> {
        let mut state = lock_state(&self.state);
        state.pixels.copy_from_slice(&self.back);
        state.commits = state.commits.saturating_add(1);
        Ok(())
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        lock_state(&self.state).region = rects.to_vec();
        Ok(())
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        lock_state(&self.state).visible = visible;
        Ok(())
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        // The X11 rung has no frame callback, and the tests here drive the window that rung
        // serves; a token would pace animation the surface does not ask for.
        None
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        let mut state = lock_state(&self.state);
        out.append(&mut state.pending);
        Ok(())
    }

    fn geometry(&self) -> (u32, u32, f32) {
        (self.width_dp, self.height_dp, self.scale)
    }

    fn backend_id(&self) -> &'static str {
        "mock"
    }
}
