//! The custom Slint platform: one surface, one window, and no event loop of its own.
//!
//! Slint is driven here, not the other way round. The UI thread owns the `poll(2)` loop
//! (2.1) and calls [`RspinyinPlatform::render_if_dirty`] after each wake-up, so the
//! platform refuses to run an event loop rather than pretending to have one. That is also
//! why nothing in this module blocks: every call returns within itself.
//!
//! # The licence boundary
//!
//! Slint installs its platform per thread and hands out windows through the
//! `slint::platform::Platform` and `slint::platform::WindowAdapter` traits. Implementing
//! those traits puts the Slint API into a type's public surface, and `ime-ui` may not
//! export any Slint type (Slint Royalty-free 2.0, obligation `OB-4`), so the trait
//! implementations live on private types and [`RspinyinPlatform`] is the crate's own
//! handle to them.
//!
//! # Threading
//!
//! A platform belongs to the thread that installed it. The surface, the window and the
//! renderer all live on that thread and none of them is `Sync`: the only way into the
//! platform from elsewhere is the UI thread's own command queue (2.5.1).

use std::cell::RefCell;
use std::os::fd::OwnedFd;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use ime_types::{FrameToken, ImeError, PlatformError, RectI, SurfaceBackend, SurfaceEvent};
use rustix::io::dup;
use slint::PlatformError as SlintError;
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};

use crate::renderer::{RenderOutcome, SlintWindowAdapter};

/// Diagnostic code recorded when the process already owns a Slint platform.
pub const SLINT_CONFLICT_CODE: &str = "ui/slint/conflict";

/// Diagnostic code recorded when a second window is asked of the platform.
pub const SECOND_WINDOW_CODE: &str = "ui/slint/second-window";

/// Diagnostic code recorded when the backend starves for [`BUFFER_STARVATION_FRAMES`]
/// frames in a row.
pub const BUFFER_STARVATION_CODE: &str = "ui/buffer/starvation";

/// How many consecutive skipped frames count as starvation.
pub const BUFFER_STARVATION_FRAMES: u32 = 3;

/// Why the platform could not be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The thread already has a Slint platform: the host, or another part of the process,
    /// got there first.
    AlreadySet,
}

impl InstallError {
    /// The stable diagnostic code to record for this failure.
    pub const fn code(self) -> &'static str {
        SLINT_CONFLICT_CODE
    }
}

impl From<InstallError> for ImeError {
    fn from(_: InstallError) -> Self {
        // The frozen list has no variant for "Slint is already owned", and the closest one
        // is also the decision this failure forces: without a platform of our own there is
        // no self-drawn candidate window, so the host's own UI takes over.
        ImeError::CompositorUnsupported {
            detail: String::from(SLINT_CONFLICT_CODE),
        }
    }
}

/// The state the platform and its window share.
struct PlatformInner {
    /// The one surface the candidate window is drawn into.
    backend: Rc<RefCell<Box<dyn SurfaceBackend>>>,
    /// The adapter of that surface, once Slint has asked for a window.
    window: RefCell<Option<Weak<SlintWindowAdapter>>>,
    /// When the platform was created, which is what Slint's monotonic clock counts from.
    start: Instant,
}

/// The Slint platform of the candidate window.
///
/// One platform owns one surface, so a process with two candidate windows is not a case
/// this type serves: the second `create_window_adapter` is refused rather than silently
/// sharing the pixels of the first.
pub struct RspinyinPlatform {
    inner: Rc<PlatformInner>,
}

impl RspinyinPlatform {
    /// Creates the platform for `backend`.
    ///
    /// The backend is not touched until Slint asks for a window, so this may run before the
    /// surface is ready to be drawn into.
    pub fn new(backend: Box<dyn SurfaceBackend>) -> Self {
        Self {
            inner: Rc::new(PlatformInner {
                backend: Rc::new(RefCell::new(backend)),
                window: RefCell::new(None),
                start: Instant::now(),
            }),
        }
    }

    /// Installs this platform on the calling thread.
    ///
    /// Slint keeps its platform per thread and refuses a second one, so this has to be the
    /// first Slint call on the thread: no window, component or timer may exist yet.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::AlreadySet`] when the thread already has a Slint platform.
    /// The caller records [`SLINT_CONFLICT_CODE`] and leaves the candidate window to the
    /// host's own UI.
    pub fn install(&self) -> Result<(), InstallError> {
        let platform = SlintPlatform {
            inner: Rc::clone(&self.inner),
        };
        slint::platform::set_platform(Box::new(platform)).map_err(|_| InstallError::AlreadySet)
    }

    /// Rasterizes the scene into the surface if anything is dirty.
    ///
    /// This is what the UI thread calls after `poll(2)` returns, once per wake-up. It never
    /// blocks and never waits for the compositor.
    ///
    /// # Errors
    ///
    /// Propagates a backend failure. A backend with no free draw buffer is reported as
    /// [`RenderOutcome::Skipped`] instead, with the frame left dirty for the next call.
    pub fn render_if_dirty(&self) -> Result<RenderOutcome, PlatformError> {
        match self.window() {
            Some(adapter) => adapter.render_if_dirty(),
            // No window yet: nothing has been asked of the platform, so nothing is dirty.
            None => Ok(RenderOutcome::Idle),
        }
    }

    /// Polls the surface and applies the events that change the window's geometry.
    ///
    /// Events are appended to `out` for the caller, which translates the pointer ones into
    /// its own vocabulary; the resize and scale events are applied to the Slint window here
    /// and are left in `out` as well, because the UI thread re-lays-out the candidate
    /// container from them.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the display connection is gone, and
    /// whatever else the backend reports.
    pub fn poll_events(&self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        let first = out.len();
        self.inner.backend.borrow_mut().poll_events(out)?;
        let Some(adapter) = self.window() else {
            return Ok(());
        };
        for event in &out[first..] {
            adapter
                .apply_geometry_event(*event)
                .map_err(backend_error)?;
        }
        Ok(())
    }

    /// Duplicates the backend's connection descriptor, if it has one.
    ///
    /// The platform keeps the backend behind a `RefCell` shared with the window adapter,
    /// so a descriptor borrowed from it cannot outlive this call — which is why the copy
    /// is made here instead: the duplicate is an owned descriptor over the same open file
    /// description, so a poll set watching it observes exactly the readiness the
    /// connection itself reports. A backend with no descriptor (`None`) and a `dup` that
    /// failed (descriptor-table exhaustion) both answer `None`, which puts the caller
    /// back on its eventfd-only wait — the loop's documented fallback, not a lost
    /// correctness property.
    pub fn dup_connection_fd(&self) -> Option<OwnedFd> {
        let backend = self.inner.backend.borrow();
        let fd = backend.connection_fd()?;
        dup(fd).ok()
    }

    /// Sets the interactive region of the surface, in physical pixels.
    ///
    /// An empty set makes the whole window click-through, which is how the shadow reserve
    /// around the candidate container lets the pointer through to the application.
    ///
    /// # Errors
    ///
    /// Propagates a backend failure.
    pub fn set_input_region(&self, rects: &[RectI]) -> Result<(), PlatformError> {
        self.inner.backend.borrow_mut().set_input_region(rects)
    }

    /// The frame callback token of the last committed frame, until the backend redeems it.
    ///
    /// The UI thread uses this to pace animation frames: a token means the compositor will
    /// tell us when it is ready for the next one.
    pub fn pending_frame(&self) -> Option<FrameToken> {
        self.window().and_then(|adapter| adapter.pending_frame())
    }

    /// Frames committed since the platform was created.
    ///
    /// This is the counter the idle-CPU budget (`BUDGET-CPU-01`) is asserted on: a window
    /// nothing is happening to must not advance it.
    pub fn committed_frames(&self) -> u64 {
        self.window().map_or(0, |adapter| adapter.committed())
    }

    /// Frames skipped in a row because no draw buffer was free.
    ///
    /// The caller records [`BUFFER_STARVATION_CODE`] once this reaches
    /// [`BUFFER_STARVATION_FRAMES`].
    pub fn starvation_streak(&self) -> u32 {
        self.window()
            .map_or(0, |adapter| adapter.starvation_streak())
    }

    /// The backend's identifier, for diagnostics.
    pub fn backend_id(&self) -> &'static str {
        self.inner.backend.borrow().backend_id()
    }

    /// The surface geometry as `(width_dp, height_dp, scale)`.
    pub fn geometry(&self) -> (u32, u32, f32) {
        self.inner.backend.borrow().geometry()
    }

    /// The window adapter, if Slint has asked for a window yet.
    fn window(&self) -> Option<Rc<SlintWindowAdapter>> {
        self.inner.window.borrow().as_ref().and_then(Weak::upgrade)
    }
}

/// The Slint-facing half of the platform.
///
/// It is private on purpose: implementing `slint::platform::Platform` makes the Slint API
/// part of a type's surface, and `ime-ui` may not export any Slint type (`OB-4`).
/// [`RspinyinPlatform`] is the crate's own handle to it.
struct SlintPlatform {
    inner: Rc<PlatformInner>,
}

impl Platform for SlintPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, SlintError> {
        if self
            .inner
            .window
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some()
        {
            // One surface, one window: a second component would fight the first for the
            // pixels, so it is refused while the caller can still report it.
            return Err(SlintError::Other(String::from(SECOND_WINDOW_CODE)));
        }
        let adapter = SlintWindowAdapter::new(Rc::clone(&self.inner.backend))?;
        *self.inner.window.borrow_mut() = Some(Rc::downgrade(&adapter));
        let adapter: Rc<dyn WindowAdapter> = adapter;
        Ok(adapter)
    }

    fn duration_since_start(&self) -> Duration {
        // Monotonic by construction: Slint uses this for animations and timers, which must
        // not see the clock jump.
        self.inner.start.elapsed()
    }

    fn run_event_loop(&self) -> Result<(), SlintError> {
        // The UI thread owns the poll loop and drives rendering through
        // `RspinyinPlatform::render_if_dirty`; Slint's own loop is never run. Reporting
        // that as an error is better than a silent no-op, which would look like a hang.
        Err(SlintError::NoEventLoopProvider)
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        // A proxy would post tasks onto the UI thread, and the UI thread's own command
        // queue is the one path into it. A second one would be a second source of ordering
        // bugs, and `None` is the documented way of saying the platform has none.
        None
    }
}

/// Maps a Slint platform failure onto the backend error vocabulary.
///
/// The only Slint call on this path that can fail is a window-event dispatch, and a failure
/// there means the scene can no longer be driven at all, which is what `Unavailable`
/// states. The Slint text is dropped because nothing downstream can act on it.
fn backend_error(_: SlintError) -> PlatformError {
    PlatformError::Unavailable
}

#[cfg(test)]
mod tests {
    use slint::PhysicalSize;

    use super::*;
    use crate::renderer::mock::{MockSurface, on_own_thread};

    #[test]
    fn test_install_refuses_a_second_platform_on_the_same_thread() {
        let (first, second) = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(160, 64, 1.0).0));
            let first = platform.install();
            let second = platform.install();
            (first, second)
        });
        assert_eq!(first, Ok(()));
        assert_eq!(
            second,
            Err(InstallError::AlreadySet),
            "Slint allows one platform per thread"
        );
        assert_eq!(InstallError::AlreadySet.code(), "ui/slint/conflict");
    }

    #[test]
    fn test_install_error_carries_the_compositor_code() {
        let error = ImeError::from(InstallError::AlreadySet);
        assert_eq!(
            error.to_string(),
            "platform/compositor/unsupported: ui/slint/conflict",
            "the frozen code and the precise one both survive the conversion"
        );
    }

    #[test]
    fn test_create_window_adapter_serves_one_window_only() {
        let (first, second) = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(160, 64, 1.0).0));
            let slint_platform = SlintPlatform {
                inner: Rc::clone(&platform.inner),
            };
            // `first` has to stay alive across the second call. The platform keeps only a
            // `Weak` to the adapter it handed out, so collapsing the first call to a
            // `bool` -- as this test used to -- drops the last strong reference and the
            // guard then sees no window at all and serves a second one.
            let first = slint_platform.create_window_adapter();
            let second = slint_platform.create_window_adapter();
            (first.is_ok(), second.is_err())
        });
        assert!(first, "the first window is served");
        assert!(
            second,
            "a second window is refused instead of sharing the first one's pixels"
        );
    }

    #[test]
    fn test_duration_since_start_is_monotonic() {
        let (first, second) = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(160, 64, 1.0).0));
            let slint_platform = SlintPlatform {
                inner: Rc::clone(&platform.inner),
            };
            (
                slint_platform.duration_since_start(),
                slint_platform.duration_since_start(),
            )
        });
        assert!(
            second >= first,
            "Slint's clock must never go backwards: {first:?} then {second:?}"
        );
    }

    #[test]
    fn test_run_event_loop_reports_no_provider() {
        let refused = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(160, 64, 1.0).0));
            let slint_platform = SlintPlatform {
                inner: Rc::clone(&platform.inner),
            };
            slint_platform.run_event_loop().is_err()
        });
        assert!(
            refused,
            "the UI thread owns the loop, so Slint has no event loop here"
        );
    }

    #[test]
    fn test_render_if_dirty_without_a_window_is_idle() {
        let (outcome, commits) = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(160, 64, 1.0).0));
            let outcome = platform
                .render_if_dirty()
                .expect("there is nothing to render before a window exists");
            (outcome, platform.committed_frames())
        });
        assert_eq!(outcome, RenderOutcome::Idle);
        assert_eq!(commits, 0, "no window means no committed frames");
    }

    #[test]
    fn test_poll_events_applies_a_resize_to_the_window() {
        let (events, size) = on_own_thread(|| {
            let (backend, state) = MockSurface::new(160, 64, 1.0);
            let platform = RspinyinPlatform::new(Box::new(backend));
            let slint_platform = SlintPlatform {
                inner: Rc::clone(&platform.inner),
            };
            let adapter = slint_platform
                .create_window_adapter()
                .expect("the first window is served");
            state
                .lock()
                .expect("the mock is not poisoned")
                .pending
                .push(SurfaceEvent::Resize { w: 320, h: 128 });
            let mut events = Vec::new();
            platform
                .poll_events(&mut events)
                .expect("polling the mock succeeds");
            (events, adapter.size())
        });
        assert_eq!(events, vec![SurfaceEvent::Resize { w: 320, h: 128 }]);
        assert_eq!(
            size,
            PhysicalSize::new(320, 128),
            "the window follows the size the compositor allowed"
        );
    }

    #[test]
    fn test_set_input_region_reaches_the_surface() {
        let region = on_own_thread(|| {
            let (backend, state) = MockSurface::new(160, 64, 1.0);
            let platform = RspinyinPlatform::new(Box::new(backend));
            let rect = RectI {
                x: 2,
                y: 3,
                w: 4,
                h: 5,
            };
            platform
                .set_input_region(&[rect])
                .expect("the mock accepts a region");
            let region = state
                .lock()
                .expect("the mock is not poisoned")
                .region
                .clone();
            platform
                .set_input_region(&[])
                .expect("the mock accepts an empty region");
            let empty = state
                .lock()
                .expect("the mock is not poisoned")
                .region
                .clone();
            assert!(empty.is_empty(), "an empty region stays empty");
            region
        });
        assert_eq!(
            region,
            vec![RectI {
                x: 2,
                y: 3,
                w: 4,
                h: 5
            }]
        );
    }

    #[test]
    fn test_the_platform_reports_the_surface_it_owns() {
        let (id, geometry) = on_own_thread(|| {
            let platform = RspinyinPlatform::new(Box::new(MockSurface::new(120, 40, 2.0).0));
            (platform.backend_id(), platform.geometry())
        });
        assert_eq!(id, "mock");
        assert_eq!(geometry, (120, 40, 2.0));
    }
}
