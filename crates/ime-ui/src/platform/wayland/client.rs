//! The seam the Wayland protocol binding plugs into.
//!
//! Responsibility: name, in the backend's own vocabulary, everything it needs from a Wayland
//! connection. Boundaries: this module declares and documents the seam and implements nothing
//! -- the binding does, and the tests implement it with a script that answers from memory.
//!
//! # Why the seam exists
//!
//! The protocol crates (`wayland-client`, `wayland-protocols-wlr`, a `memfd` or
//! `XDG_RUNTIME_DIR` mapping for the `wl_shm` pool) are not dependencies of this crate yet, so
//! the backend is written against this trait rather than against them. That is what makes the
//! substance -- the tier ladder, the geometry, the buffer discipline and the event decoding --
//! testable with no compositor running, and it is what keeps the eventual binding a mechanical
//! translation of a small, closed set of operations.
//!
//! # What the binding must not do
//!
//! * It must never call `xdg_popup.grab`, which is the only request that would give the popup
//!   the keyboard. There is deliberately no method here for it.
//! * It must never bind a `wl_keyboard` to type through, and must report
//!   [`WireEvent::KeyboardEnter`] for our own surface only, so the ladder can fail a tier that
//!   took the keyboard.
//! * It must never block. Every method returns within itself; the UI thread calls them from
//!   its `poll(2)` loop, and a request that waited for a reply would stall the loop and the
//!   host thread's frames with it.
//! * It must never destroy a buffer the compositor still holds: a slot is only recycled after
//!   [`WireEvent::BufferReleased`], and [`ProtocolClient::resize_pool`] is only called when
//!   both slots are back.

use std::os::fd::RawFd;

use ime_types::{FrameToken, PlatformError, RectI};

use super::events::WireEvent;
use super::layer_shell::LayerRequest;
use super::popup::PopupRequest;
use super::probe::Global;
use super::{OutputInfo, SurfaceRect, Tier};

/// One frame to put on the surface.
///
/// The three values travel together because they are one operation: a buffer is attached, its
/// damage declared, and the surface committed in one request round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCommit<'a> {
    /// The pool slot holding the pixels.
    pub slot: usize,
    /// The buffer's size in physical pixels.
    pub size_px: (u32, u32),
    /// The damaged regions, in buffer coordinates -- which are physical pixels here, because
    /// the buffer scale is set to the output's ratio.
    pub damage: &'a [RectI],
}

/// Everything the backend needs from a Wayland connection.
///
/// # Concurrency
///
/// `Send` but not `Sync`: a Wayland connection may only be used from the thread that created
/// it, which is the UI thread that owns this backend. Every method is called from that thread.
pub trait ProtocolClient: Send {
    /// The registry globals seen so far, in arrival order.
    fn globals(&self) -> Vec<Global>;

    /// The outputs the connection knows about, in enumeration order.
    fn outputs(&self) -> Vec<OutputInfo>;

    /// The connection's file descriptor, for the UI thread's `poll(2)` loop.
    fn connection_fd(&self) -> RawFd;

    /// Creates the surface role for `tier`, replacing any surface left over from a previous
    /// tier.
    ///
    /// For [`Tier::LayerShell`] that is a `zwlr_layer_surface_v1` on `output`, which the
    /// binding binds to the output the window is placed on. For the popup tiers it is the
    /// parent toplevel plus the `xdg_popup`: the parent's window geometry must cover `output`,
    /// because that is the only parent position a client can know and the only one in which
    /// the positioner's anchor rectangle is expressible. `output` is `None` before anything
    /// has been placed, in which case the binding may bind the layer surface to whatever
    /// output it considers current. Nothing is mapped until
    /// [`ProtocolClient::set_visible`].
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when the interface the tier needs is not
    /// available, and [`PlatformError::Disconnected`] when the connection is gone.
    fn create_surface(
        &mut self,
        tier: Tier,
        output: Option<&OutputInfo>,
    ) -> Result<(), PlatformError>;

    /// Applies a tier 1 placement: size, anchor, margin, exclusive zone and keyboard
    /// interactivity, all in surface-local units.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the requests cannot be delivered.
    fn configure_layer(&mut self, request: &LayerRequest) -> Result<(), PlatformError>;

    /// Applies a popup placement: the positioner's anchor rectangle, anchor, gravity,
    /// constraint adjustment and size.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when the placement cannot be expressed -- the
    /// anchor rectangle outside the parent's window geometry, which the protocol refuses --
    /// and [`PlatformError::Disconnected`] when the connection is gone.
    fn configure_popup(&mut self, request: &PopupRequest) -> Result<(), PlatformError>;

    /// Sets the surface's buffer scale to the output's device pixel ratio.
    ///
    /// The buffers are then drawn at the output's physical resolution and the protocol's
    /// surface-local units are the logical pixels the window is laid out in, so nothing is
    /// rasterized at double size and nothing is upscaled by the compositor.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    fn set_buffer_scale(&mut self, scale: i32) -> Result<(), PlatformError>;

    /// Replaces the interactive region, in surface-local units. An empty set makes the
    /// surface click-through.
    ///
    /// A binding whose `wl_compositor` is older than the version that has
    /// `set_input_region` reports success and leaves the whole surface interactive: the
    /// window is small, so the degradation costs clicks near its edge rather than usability.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    fn set_input_region(&mut self, rects: &[SurfaceRect]) -> Result<(), PlatformError>;

    /// Maps or unmaps the surface.
    ///
    /// Unmapping is `attach(NULL)` followed by a commit, which is how a surface with no buffer
    /// becomes invisible on every tier.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError>;

    /// Attaches a buffer, declares its damage and commits the surface.
    ///
    /// The buffer stays the compositor's until it sends [`WireEvent::BufferReleased`] for the
    /// slot.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    fn attach_and_commit(&mut self, frame: FrameCommit<'_>) -> Result<(), PlatformError>;

    /// Recreates the buffer pool at a new physical size.
    ///
    /// Called only when both slots are back, so no buffer the compositor holds is destroyed.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when the pool cannot be created -- a `wl_shm`
    /// allocation failure, a full `/dev/shm`, or an unwritable runtime directory.
    fn resize_pool(&mut self, width_px: u32, height_px: u32) -> Result<(), PlatformError>;

    /// The pixels of one pool slot, or `None` when the pool has no such slot.
    fn buffer_mut(&mut self, slot: usize) -> Option<&mut [u8]>;

    /// Requests a frame callback carrying `token`.
    ///
    /// At most one callback may be outstanding per surface, so the caller must not request a
    /// second before the first has fired.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    fn request_frame(&mut self, token: FrameToken) -> Result<(), PlatformError>;

    /// Drains pending protocol events into `out`.
    ///
    /// Must not block: whatever has already arrived is decoded and the call returns.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the connection is gone, which is what a
    /// compositor restart looks like from here.
    fn dispatch(&mut self, out: &mut Vec<WireEvent>) -> Result<(), PlatformError>;

    /// Pushes queued requests to the compositor.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the connection is gone.
    fn flush(&mut self) -> Result<(), PlatformError>;
}
