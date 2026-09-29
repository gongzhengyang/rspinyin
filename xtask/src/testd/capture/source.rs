//! The server side of a capture: what a drawable is, how its pixels are read, and the
//! retry that keeps a frame read across a move from being written as a baseline.
//!
//! Responsibility: turn a [`CaptureTarget`] into a drawable plus the rectangle to read from
//! it, and read that rectangle one chunk at a time. The arithmetic of the frame lives in
//! [`super::plan`], the byte order in [`super::pixels`], and the PNG in [`super::encode`].
//!
//! # Why the read is checked twice
//!
//! A window can be moved or resized between the moment its geometry is read and the moment
//! its last row arrives. The pixels would still be a real rendering of the window -- but of
//! a rectangle that is no longer where the capture says it is, which is exactly the kind of
//! misaligned baseline a later audit would compare against. So the footprint is read before
//! and after, a single retry covers a window manager that was still placing the window, and
//! a drawable that moved twice fails the capture instead.
//!
//! # What the read loop is generic over
//!
//! [`read_stable`] takes anything that can report a footprint and fill a row range, so the
//! retry and the refusal are exercised by the tests beside this module with a double that
//! moves on demand -- no display server, no window manager, and no timing.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as XprotoExt, Format, GetGeometryReply, ImageFormat, Screen, Window,
};
use x11rb::rust_connection::RustConnection;

use super::pixels::PixelFormat;
use super::plan::{ChunkRange, Footprint, FramePlan};
use super::{CaptureError, CaptureTarget};
use crate::testd::x11::X11Session;

/// The X11 `AllPlanes` mask: every bit of a pixel is wanted.
const ALL_PLANES: u32 = u32::MAX;

/// What a capture reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Anchor {
    /// The root window, whose coordinates are screen coordinates.
    Root(Window),
    /// A window, whose coordinates are its own.
    Window(Window),
}

impl Anchor {
    /// The drawable the requests name.
    fn drawable(self) -> Window {
        match self {
            Self::Root(window) | Self::Window(window) => window,
        }
    }
}

/// A drawable, resolved: where to read from it, how much, and in which layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Resolved {
    /// The drawable and what its coordinates mean.
    pub(super) anchor: Anchor,
    /// Top-left of the capture inside the drawable, in drawable coordinates.
    pub(super) origin: (i32, i32),
    /// Size of the capture in physical pixels.
    pub(super) size: (u32, u32),
    /// The drawable's pixel layout, already checked to be one this channel unpacks.
    pub(super) format: PixelFormat,
}

/// One drawable a frame is read from.
///
/// The trait exists so the read loop can be driven without a display server: a double that
/// answers a footprint and fills a row range exercises the retry and the refusal, which are
/// the parts of a capture that a live run can only reach by luck.
pub(super) trait FrameSource {
    /// The target this source reads, so a refusal can name it.
    fn target(&self) -> CaptureTarget;

    /// Where the drawable is now, in screen physical pixels.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Request`] when the server cannot answer.
    fn footprint(&self) -> Result<Footprint, CaptureError>;

    /// Reads `chunk`'s rows into `out`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Request`] when the server refuses the request, and
    /// [`CaptureError::ShortRead`] when fewer bytes arrive than the geometry promised.
    fn read_rows(
        &self,
        chunk: ChunkRange,
        row_bytes: usize,
        out: &mut [u8],
    ) -> Result<(), CaptureError>;
}

/// A live X11 drawable.
pub(super) struct X11Drawable<'a> {
    /// The connection the requests go out on.
    conn: &'a RustConnection,
    /// The root window, which a window's position is translated against.
    root: Window,
    /// What is being read.
    anchor: Anchor,
    /// The target the caller named, carried for the refusal.
    target: CaptureTarget,
    /// Top-left of the capture inside the drawable.
    origin: (i32, i32),
    /// Width of the capture, in physical pixels.
    width: u32,
}

impl<'a> X11Drawable<'a> {
    /// Binds a source to a resolved drawable.
    pub(super) fn new(session: &'a X11Session, target: CaptureTarget, resolved: &Resolved) -> Self {
        Self {
            conn: session.connection(),
            root: session.root(),
            anchor: resolved.anchor,
            target,
            origin: resolved.origin,
            width: resolved.size.0,
        }
    }
}

impl FrameSource for X11Drawable<'_> {
    fn target(&self) -> CaptureTarget {
        self.target
    }

    fn footprint(&self) -> Result<Footprint, CaptureError> {
        let geometry = geometry(self.conn, self.anchor.drawable())?;
        let size = (u32::from(geometry.width), u32::from(geometry.height));
        let origin = match self.anchor {
            // The root is the origin of screen coordinates by definition.
            Anchor::Root(_) => (0, 0),
            Anchor::Window(window) => {
                let reply = self
                    .conn
                    .translate_coordinates(window, self.root, 0, 0)
                    .map_err(request)?
                    .reply()
                    .map_err(request)?;
                (i32::from(reply.dst_x), i32::from(reply.dst_y))
            }
        };
        Ok(Footprint { origin, size })
    }

    fn read_rows(
        &self,
        chunk: ChunkRange,
        row_bytes: usize,
        out: &mut [u8],
    ) -> Result<(), CaptureError> {
        let x = wire(i16::try_from(self.origin.0), "x", i64::from(self.origin.0))?;
        let row = i64::from(self.origin.1) + i64::from(chunk.y);
        let y = wire(i16::try_from(row), "y", row)?;
        let width = wire(u16::try_from(self.width), "width", i64::from(self.width))?;
        let height = wire(u16::try_from(chunk.rows), "height", i64::from(chunk.rows))?;
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.anchor.drawable(),
                x,
                y,
                width,
                height,
                ALL_PLANES,
            )
            .map_err(request)?
            .reply()
            .map_err(request)?;
        let expected = row_bytes * chunk.rows as usize;
        let Some(slot) = out.get_mut(..expected) else {
            return Err(CaptureError::ShortRead {
                expected,
                actual: out.len(),
            });
        };
        let Some(data) = reply.data.get(..expected) else {
            return Err(CaptureError::ShortRead {
                expected,
                actual: reply.data.len(),
            });
        };
        slot.copy_from_slice(data);
        Ok(())
    }
}

/// Reads a whole frame, refusing a drawable that moved while it was being read.
///
/// The frame is read, the drawable's footprint is compared across the reading, and a
/// drawable that moved is read once more. Every chunk rewrites its own rows, so nothing of
/// a rejected reading survives in the buffer.
///
/// # Errors
///
/// Returns whatever the source reports, and [`CaptureError::WindowMoved`] when the
/// drawable's footprint differs across both readings.
///
/// # Panics
///
/// Never.
pub(super) fn read_stable<S: FrameSource>(
    source: &S,
    plan: &FramePlan,
    frame: &mut [u8],
) -> Result<(), CaptureError> {
    let (before, after) = attempt(source, plan, frame)?;
    if before == after {
        return Ok(());
    }
    let (before, after) = attempt(source, plan, frame)?;
    if before == after {
        return Ok(());
    }
    Err(CaptureError::WindowMoved {
        target: source.target(),
        before: before.quad(),
        after: after.quad(),
    })
}

/// Reads the whole frame once, reporting where the drawable was before and after.
fn attempt<S: FrameSource>(
    source: &S,
    plan: &FramePlan,
    frame: &mut [u8],
) -> Result<(Footprint, Footprint), CaptureError> {
    let before = source.footprint()?;
    read_frame(source, plan, frame)?;
    let after = source.footprint()?;
    Ok((before, after))
}

/// Reads every chunk of the plan into the frame.
fn read_frame<S: FrameSource>(
    source: &S,
    plan: &FramePlan,
    frame: &mut [u8],
) -> Result<(), CaptureError> {
    for chunk in plan.chunks() {
        let Some(slot) = plan.chunk_slice(frame, *chunk) else {
            return Err(CaptureError::ShortRead {
                expected: plan.buffer_len(),
                actual: frame.len(),
            });
        };
        source.read_rows(*chunk, plan.row_bytes(), slot)?;
    }
    Ok(())
}

/// Resolves a target into a drawable, a rectangle inside it, and its pixel layout.
///
/// # Errors
///
/// Returns [`CaptureError::Request`] when the geometry or the window attributes cannot be
/// answered -- which is also what a window that has gone away reports --
/// [`CaptureError::RegionOffScreen`] for a region that is not on the screen, and
/// [`CaptureError::UnknownVisual`] or [`CaptureError::UnsupportedVisual`] when the
/// drawable's pixel layout is not one this channel can unpack.
///
/// # Panics
///
/// Never.
pub(super) fn resolve(
    session: &X11Session,
    target: CaptureTarget,
) -> Result<Resolved, CaptureError> {
    let conn = session.connection();
    let screen = screen_of(session)?;
    let formats = &conn.setup().pixmap_formats;
    match target {
        CaptureTarget::FullScreen => {
            let root = session.root();
            let geometry = geometry(conn, root)?;
            Ok(Resolved {
                anchor: Anchor::Root(root),
                origin: (0, 0),
                size: (u32::from(geometry.width), u32::from(geometry.height)),
                format: root_format(screen, formats)?,
            })
        }
        CaptureTarget::Region(region) => {
            super::plan::check_region(region, session.screen())?;
            Ok(Resolved {
                anchor: Anchor::Root(session.root()),
                origin: (region.x, region.y),
                size: (region.w, region.h),
                format: root_format(screen, formats)?,
            })
        }
        CaptureTarget::Window(window) => {
            let geometry = geometry(conn, window)?;
            let attributes = conn
                .get_window_attributes(window)
                .map_err(request)?
                .reply()
                .map_err(request)?;
            let format = PixelFormat::of_visual(screen, formats, attributes.visual, geometry.depth)
                .ok_or(CaptureError::UnknownVisual {
                    visual: attributes.visual,
                })?
                .checked()?;
            Ok(Resolved {
                anchor: Anchor::Window(window),
                // A window's own coordinates start at its top-left corner, so the whole
                // window is read from its own origin however it is placed on the screen.
                origin: (0, 0),
                size: (u32::from(geometry.width), u32::from(geometry.height)),
                format,
            })
        }
    }
}

/// The screen the session's root window belongs to.
///
/// # Errors
///
/// Returns [`CaptureError::Request`] when the setup lists no screen with that root, which is
/// a server describing a window it has not described the screen of.
fn screen_of(session: &X11Session) -> Result<&Screen, CaptureError> {
    session
        .connection()
        .setup()
        .roots
        .iter()
        .find(|screen| screen.root == session.root())
        .ok_or_else(|| CaptureError::Request {
            detail: format!(
                "the server's setup lists no screen whose root is {:#x}",
                session.root()
            ),
        })
}

/// The root window's pixel layout, checked to be one this channel unpacks.
fn root_format(screen: &Screen, formats: &[Format]) -> Result<PixelFormat, CaptureError> {
    PixelFormat::of_screen(screen, formats)
        .ok_or(CaptureError::UnknownVisual {
            visual: screen.root_visual,
        })?
        .checked()
}

/// The geometry of a drawable.
fn geometry(conn: &RustConnection, drawable: Window) -> Result<GetGeometryReply, CaptureError> {
    conn.get_geometry(drawable)
        .map_err(request)?
        .reply()
        .map_err(request)
}

/// Narrows a value to the width a request field carries.
///
/// A capture whose origin or size does not fit is refused here rather than truncated into
/// the request: a wrapped field would ask the server for a rectangle nobody named.
fn wire<T>(
    narrowed: Result<T, std::num::TryFromIntError>,
    field: &str,
    value: i64,
) -> Result<T, CaptureError> {
    narrowed.map_err(|_| CaptureError::Request {
        detail: format!("the capture's {field} {value} does not fit the request's field"),
    })
}

/// Maps an X11 failure onto the channel's error vocabulary.
fn request<E: std::fmt::Display>(error: E) -> CaptureError {
    CaptureError::Request {
        detail: error.to_string(),
    }
}
