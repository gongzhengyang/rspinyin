//! The platform boundary: `SurfaceBackend` and its pixel / event vocabulary.
//!
//! The trait is the seam that keeps rendering free of platform detail. It hands
//! out a raw pixel buffer plus a handful of primitives, and it knows nothing
//! about Slint: the window backends implement it and the renderer consumes it.
//! That is what lets the renderer be tested against a mock backend with no
//! display server present.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use crate::error::PlatformError;
use crate::ui::RectI;

/// A writable pixel buffer for one frame.
///
/// The format is fixed at Argb8888 with premultiplied alpha, which matches both
/// the shared-memory buffer format and the 32-bit visual used on X11. Only
/// `Debug` is derived: the buffer is an exclusive borrow of the frame memory, so
/// it can be neither copied nor compared.
#[derive(Debug)]
pub struct PixelBufferMut<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

/// Token for a requested frame callback.
///
/// Returned by `request_frame` where the backend supports frame callbacks, and
/// redeemed when the matching frame event arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameToken(pub u64);

/// One input or compositor event delivered by a backend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceEvent {
    PointerEnter {
        x: i32,
        y: i32,
    },
    PointerLeave,
    PointerMotion {
        x: i32,
        y: i32,
    },
    PointerButton {
        x: i32,
        y: i32,
        button: u8,
        pressed: bool,
    },
    Axis {
        x: i32,
        y: i32,
        delta: i32,
        horizontal: bool,
    },
    Resize {
        w: u32,
        h: u32,
    },
    Scale {
        factor: f32,
    },
    CloseRequested,
}

/// A drawable surface on one platform backend.
///
/// # Concurrency
///
/// The trait is `Send` because the backend is moved onto the UI thread and then
/// owned by it, but not `Sync`: every method takes `&mut self`, and a surface is
/// deliberately single-threaded. Implementations must not block beyond the call
/// itself, and must never take keyboard focus -- a candidate window that steals
/// focus is the project's highest-severity defect.
pub trait SurfaceBackend: Send {
    /// Acquires a writable pixel buffer; the format is fixed at Argb8888 with
    /// premultiplied alpha.
    ///
    /// The returned buffer is at least `width * height * 4` bytes long, and the
    /// caller is responsible for writing every pixel it intends to display.
    ///
    /// # Errors
    ///
    /// Returns `PlatformError::NoFreeBuffer` while the compositor still holds the
    /// previous buffer; the caller skips the frame rather than blocking.
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError>;

    /// Commits the buffer and declares the damaged regions, in physical pixels
    /// relative to the window's top-left corner.
    ///
    /// # Errors
    ///
    /// Returns a `PlatformError` when the commit cannot be delivered.
    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError>;

    /// Sets the interactive region. An empty set makes the whole window
    /// click-through.
    ///
    /// # Errors
    ///
    /// Returns a `PlatformError` when the region cannot be applied.
    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError>;

    /// Maps or unmaps the surface. After `true` the window is visible.
    ///
    /// # Errors
    ///
    /// Returns a `PlatformError` when the surface cannot be mapped or unmapped.
    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError>;

    /// Requests a callback for the next frame, used to pace animation frames.
    ///
    /// Returns `None` when the backend has no such notion, which is the case on
    /// X11 without a compositor frame callback.
    fn request_frame(&mut self) -> Option<FrameToken>;

    /// The file descriptor the connection multiplexes, if the backend has one.
    ///
    /// Appended by the 2026-10-01 increment recorded in `ADR-0005`: the event loop
    /// needs the connection fd in its poll set so pointer events are not queued behind
    /// the host's next post. The default answers `None`, which is what a test double
    /// wants -- the caller then falls back to its eventfd-only wait, the shape every
    /// hermetic test already exercises. X11 answers the X connection's descriptor,
    /// Wayland the display's.
    fn connection_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        None
    }

    /// Polls pending events into `out`: compositor events, pointer events, size
    /// and scale changes.
    ///
    /// # Errors
    ///
    /// Returns `PlatformError::Disconnected` when the display connection is gone,
    /// and other `PlatformError` values for backend-specific failures.
    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError>;

    /// Window geometry in logical pixels, plus the scale factor.
    fn geometry(&self) -> (u32, u32, f32);

    /// Backend identifier for diagnostics: `"x11"`, `"wlr-layer-shell"`,
    /// `"wlr-popup"`, `"wlr-canvas"` or `"mock"`.
    fn backend_id(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_token_round_trips_value() {
        let token = FrameToken(42);
        assert_eq!(token.0, 42);
        assert_eq!(token, FrameToken(42));
        assert_ne!(token, FrameToken(43));
    }

    #[test]
    fn test_surface_event_copy_and_equality() {
        let motion = SurfaceEvent::PointerMotion { x: 3, y: 4 };
        let copied = motion;
        assert_eq!(motion, copied);
        assert_ne!(motion, SurfaceEvent::PointerLeave);
        let resize = SurfaceEvent::Resize { w: 1200, h: 280 };
        assert_eq!(resize, SurfaceEvent::Resize { w: 1200, h: 280 });
        assert_ne!(resize, SurfaceEvent::Resize { w: 1200, h: 281 });
    }

    #[test]
    fn test_pixel_buffer_mut_exposes_writable_data() {
        let mut pixels = vec![0u8; 16];
        {
            let buffer = PixelBufferMut {
                data: &mut pixels,
                stride: 8,
                width: 2,
                height: 2,
            };
            buffer.data[0] = 255;
            // The caller writes `stride * height` bytes; assert the slice can hold them.
            assert_eq!(buffer.data.len(), buffer.stride * buffer.height as usize);
            assert_eq!(buffer.width, 2);
        }
        assert_eq!(pixels[0], 255);
    }
}
