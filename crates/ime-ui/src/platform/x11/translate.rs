//! The pure half of the X11 backend: X11 wire values onto the contract vocabulary.
//!
//! Everything here maps types the X server speaks -- events, setup structures, visual
//! declarations -- onto what the backend and the renderer speak, and touches neither the
//! connection nor the window. Keeping it apart from the backend is what lets it be covered
//! without a display server: the platform module's own suite drives every function here
//! against constructed wire values.
//!
//! Boundaries: nothing here sends a request or holds a socket, and nothing here decides
//! where the window goes. It is the vocabulary the backend's own half reads its orders in.

use ime_types::SurfaceEvent;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{EventMask, Screen, VisualClass, Visualid};

use super::ARGB_DEPTH;
use crate::platform::repack_scratch;

/// Alpha that makes the surface base fully opaque.
pub(crate) const OPAQUE_ALPHA: u8 = 255;

/// What one X11 event means to the backend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Decoded {
    /// An event the UI thread consumes.
    Surface(SurfaceEvent),
    /// The server lost part of the window's contents; repaint it from the retained frame.
    Repaint,
    /// The window changed size, in physical pixels.
    Resized { width_px: u32, height_px: u32 },
    /// An X protocol error; counted, never fatal.
    ProtocolError,
    /// Nothing this backend acts on.
    Ignored,
}

/// Translates one X11 event into what the backend should do about it.
pub(crate) fn classify_event(event: &Event, width_px: u32, height_px: u32) -> Decoded {
    match event {
        Event::ButtonPress(press) => {
            match scroll_axis(press.detail, press.event_x, press.event_y) {
                Some(axis) => Decoded::Surface(axis),
                None => Decoded::Surface(SurfaceEvent::PointerButton {
                    x: i32::from(press.event_x),
                    y: i32::from(press.event_y),
                    button: press.detail,
                    pressed: true,
                }),
            }
        }
        Event::ButtonRelease(release) => {
            // A wheel button sends a release as well; turning that into a second axis
            // event would scroll twice per notch.
            if scroll_axis(release.detail, release.event_x, release.event_y).is_some() {
                return Decoded::Ignored;
            }
            Decoded::Surface(SurfaceEvent::PointerButton {
                x: i32::from(release.event_x),
                y: i32::from(release.event_y),
                button: release.detail,
                pressed: false,
            })
        }
        Event::MotionNotify(motion) => Decoded::Surface(SurfaceEvent::PointerMotion {
            x: i32::from(motion.event_x),
            y: i32::from(motion.event_y),
        }),
        Event::EnterNotify(enter) => Decoded::Surface(SurfaceEvent::PointerEnter {
            x: i32::from(enter.event_x),
            y: i32::from(enter.event_y),
        }),
        Event::LeaveNotify(_) => Decoded::Surface(SurfaceEvent::PointerLeave),
        Event::Expose(_) => Decoded::Repaint,
        Event::ConfigureNotify(configure) => {
            let width = u32::from(configure.width);
            let height = u32::from(configure.height);
            if (width, height) == (width_px, height_px) {
                // The echo of a configure request we sent; there is nothing to do.
                Decoded::Ignored
            } else {
                Decoded::Resized {
                    width_px: width,
                    height_px: height,
                }
            }
        }
        Event::Error(_) => Decoded::ProtocolError,
        _ => Decoded::Ignored,
    }
}

/// Decodes an X11 wheel button into an axis event.
///
/// X11 reports the wheel as button presses: 4 and 5 are the vertical pair, 6 and 7 the
/// legacy horizontal pair, and clients that speak XInput2 use 8 and 9 for horizontal
/// movement. The sign is normalised here, once, so that the rest of the UI only ever sees
/// "positive means forward": button 5, the wheel turned down, pages forward, and button 4
/// pages back.
pub(crate) fn scroll_axis(button: u8, x: i16, y: i16) -> Option<SurfaceEvent> {
    let (delta, horizontal) = match button {
        4 => (-1, false),
        5 => (1, false),
        6 | 8 => (-1, true),
        7 | 9 => (1, true),
        _ => return None,
    };
    Some(SurfaceEvent::Axis {
        x: i32::from(x),
        y: i32::from(y),
        delta,
        horizontal,
    })
}

/// The alpha to paint the surface base with, given what the theme asked for.
pub(crate) fn effective_alpha(requested: u8, argb_visual: bool, composited: bool) -> u8 {
    if argb_visual && composited {
        requested
    } else {
        OPAQUE_ALPHA
    }
}

/// Picks the 32-bit TrueColor visual whose channel masks match the buffer format.
///
/// The ARGB path is taken only when the masks are exactly the ones an `Argb8888` buffer
/// assumes -- red in `0x00ff0000`, green in `0x0000ff00`, blue in `0x000000ff`. A depth of
/// 32 with a different order would render blue and red swapped, which is worse than the
/// opaque fallback the caller then chooses.
pub(crate) fn select_argb_visual(screen: &Screen) -> Option<(Visualid, u8)> {
    let depth = screen
        .allowed_depths
        .iter()
        .find(|depth| depth.depth == ARGB_DEPTH)?;
    depth
        .visuals
        .iter()
        .find(|visual| {
            visual.class == VisualClass::TRUE_COLOR
                && visual.red_mask == 0x00ff_0000
                && visual.green_mask == 0x0000_ff00
                && visual.blue_mask == 0x0000_00ff
        })
        .map(|visual| (visual.visual_id, depth.depth))
}

/// The events the window asks for.
///
/// `StructureNotify` is on the list because `ConfigureNotify` -- the only source of
/// [`SurfaceEvent::Resize`] -- is delivered through it and through nothing else.
pub(super) fn event_mask() -> EventMask {
    EventMask::EXPOSURE
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
        | EventMask::ENTER_WINDOW
        | EventMask::LEAVE_WINDOW
        | EventMask::VISIBILITY_CHANGE
        | EventMask::STRUCTURE_NOTIFY
}

/// The repacking scratch this window needs: nothing on the ARGB path.
pub(super) fn scratch_for(depth: u8, width_px: u32, height_px: u32) -> Vec<u8> {
    if depth == ARGB_DEPTH {
        Vec::new()
    } else {
        repack_scratch(width_px, height_px)
    }
}
