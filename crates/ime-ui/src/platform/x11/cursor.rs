//! The pointer cursor of the X11 window.
//!
//! Responsibility: give the mapped candidate window the pointer shape a clickable panel
//! carries -- the cursor font's left-pointer arrow -- and install it no more than once.
//!
//! # Why one static shape is the whole feature
//!
//! The window's interactive region is *the panel*, because
//! `SurfaceBackend::set_input_region` shapes it with the SHAPE extension on every
//! placement. The shadow reserve is outside
//! that region, so the server never delivers a pointer event for it: the cursor there is
//! whatever the application underneath shows, which is exactly the see-through behaviour
//! the reserve is drawn for. Every point the pointer *can* reach on this window is
//! therefore candidate cell or container chrome, and one arrow covers all of it. There is
//! consequently nothing dynamic to manage -- no hit-test feedback, no contract event --
//! only a window attribute to set when the window is first mapped.
//!
//! # The glyph
//!
//! The shape comes from the cursor font, the core X convention `cursorfont.h` fixes:
//! one `OpenFont` on the always-present `cursor` font, one `CreateGlyphCursor`
//! taking the left-pointer glyph and its mask, and a `ChangeWindowAttributes` making it
//! the window's cursor. No pixmap is built, and nothing here draws. A failure is a dead
//! connection and reports as every other request failure does; a server that somehow
//! lacks the font answers with an asynchronous protocol error, which the event poll
//! counts rather than propagates.
//!
//! # Resource lifetime
//!
//! The glyph cursor copies the glyphs it needs, so the font is closed as soon as the
//! cursor exists. The cursor itself lives until the connection does: the X server
//! reclaims every resource of a client on disconnect, and a cursor is one `xid` held for
//! the process lifetime.

use ime_types::PlatformError;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// The name of the core cursor font every X server carries.
const CURSOR_FONT: &[u8] = b"cursor";

/// The cursor-font glyph of the arrow a clickable surface shows.
///
/// Glyph 68 is `XC_left_ptr` of `<X11/cursorfont.h>`, the left-pointing arrow every
/// toolkit draws over ordinary clickable content; its mask glyph follows it.
const ARROW_GLYPH: u16 = 68;

/// The pointer shapes this window can carry.
///
/// One shape today, because the shaped input region leaves the window only the clickable
/// panel to be pointed at. The enum exists so the dedupe state and the install path speak
/// of a shape rather than of the one value it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointerShape {
    /// The cursor font's left-pointer arrow.
    Arrow,
}

/// The cursor state of the window: which shape is installed.
///
/// Installing is queued requests, so a shape that is already current must not be sent
/// again -- a repeated `show` would otherwise push four requests the server answers with
/// nothing new. [`Self::begin_install`] is the one gate.
#[derive(Debug)]
pub(crate) struct CursorState {
    current: Option<PointerShape>,
}

impl CursorState {
    /// Creates the state of a window that has never been mapped.
    pub(crate) const fn new() -> Self {
        Self { current: None }
    }

    /// Records `shape` as the shape to install, answering whether it is a change.
    ///
    /// `true` means the caller has to send the install; `false` means the shape is
    /// already current and any further request would be waste.
    pub(crate) fn begin_install(&mut self, shape: PointerShape) -> bool {
        if self.current == Some(shape) {
            return false;
        }
        self.current = Some(shape);
        true
    }

    /// The shape currently installed, or `None` before the first install.
    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<PointerShape> {
        self.current
    }
}

/// Installs the arrow pointer cursor on `window`, if `state` does not have it yet.
///
/// Queues the whole exchange -- font, glyph cursor, window attribute, font close -- and
/// flushes once. Called when the window is mapped: an unmapped window draws no cursor,
/// and a re-mapped one still carries the attribute it was given the first time.
///
/// # Errors
///
/// Returns [`PlatformError::Disconnected`] when the connection cannot carry the
/// requests, which is the only way an established connection refuses them.
pub(crate) fn ensure(
    state: &mut CursorState,
    conn: &RustConnection,
    window: Window,
) -> Result<(), PlatformError> {
    if !state.begin_install(PointerShape::Arrow) {
        return Ok(());
    }
    let font = conn.generate_id().map_err(disconnected)?;
    let cursor = conn.generate_id().map_err(disconnected)?;
    conn.open_font(font, CURSOR_FONT).map_err(disconnected)?;
    // Black on white: the cursor font's glyphs are drawn for that pair, and it is what
    // the default arrow on any desktop reads as.
    conn.create_glyph_cursor(
        cursor,
        font,
        font,
        ARROW_GLYPH,
        ARROW_GLYPH + 1,
        0,
        0,
        0,
        0xffff,
        0xffff,
        0xffff,
    )
    .map_err(disconnected)?;
    conn.change_window_attributes(
        window,
        &ChangeWindowAttributesAux {
            cursor: Some(cursor),
            ..Default::default()
        },
    )
    .map_err(disconnected)?;
    // The glyph cursor keeps its own copy of the two glyphs, so the font is closed at
    // once and the server is left holding only the cursor.
    conn.close_font(font).map_err(disconnected)?;
    conn.flush().map_err(disconnected)
}

/// Maps a failure on an established connection, as the backend's own mapper does.
fn disconnected<T>(_: T) -> PlatformError {
    PlatformError::Disconnected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_state_installs_only_the_first_shape() {
        let mut state = CursorState::new();
        assert_eq!(state.current(), None, "an unmapped window has no cursor");
        assert!(
            state.begin_install(PointerShape::Arrow),
            "the first shape is a change and must be sent"
        );
        assert_eq!(state.current(), Some(PointerShape::Arrow));
        assert!(
            !state.begin_install(PointerShape::Arrow),
            "a re-shown window re-sends nothing"
        );
        assert_eq!(state.current(), Some(PointerShape::Arrow));
    }

    #[test]
    #[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
    fn test_x11_backend_installs_the_arrow_cursor_when_mapped() {
        use ime_types::surface::SurfaceBackend as _;

        let mut backend = super::super::X11Backend::connect(320, 80, 1.0, None)
            .expect("a live X server reachable through DISPLAY");
        assert_eq!(backend.cursor.current(), None);
        backend.set_visible(true).expect("the window can be shown");
        assert_eq!(
            backend.cursor.current(),
            Some(PointerShape::Arrow),
            "the mapped window carries the arrow the panel is pointed at with"
        );
        // Unmapping and mapping again must not push the install a second time: the
        // window attribute survived the unmap.
        backend
            .set_visible(false)
            .expect("the window can be hidden");
        backend
            .set_visible(true)
            .expect("the window can be shown again");
        assert_eq!(backend.cursor.current(), Some(PointerShape::Arrow));
    }
}
