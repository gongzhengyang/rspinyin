//! X11 ARGB candidate-window backend.
//!
//! Responsibility: own one X11 connection and one override-redirect window, hand out
//! premultiplied `Argb8888` draw buffers, upload them with `PutImage`, shape the
//! interactive region through the SHAPE extension, and drive the window from X11 events.
//!
//! Boundaries: this module owns X11 detail and nothing else. It does not rasterize --
//! the caller writes the pixels -- and it does not decide where the window goes. The pure
//! translation from X11 events and geometry onto the contract vocabulary sits in the
//! `translate` child module, where it is covered without a display server.
//!
//! # Never takes keyboard focus
//!
//! The window is `override_redirect`, its `WM_HINTS` carries `input = False`, and this
//! module never calls `SetInputFocus`. Losing keyboard focus is the project's
//! highest-severity defect, so no grab request appears here either: neither
//! `grab_keyboard` nor `grab_pointer` is ever sent.
//!
//! # Uploads and pixel format
//!
//! Frames go out with `PutImage`, the fallback the design allows where MIT-SHM is
//! unavailable: a shared segment needs `shmget`/`shmat` (or `mmap` over a `/dev/shm`
//! file), and both need `libc` plus an `unsafe` block, neither of which this crate may
//! use. The buffer format is fixed at `Argb8888` premultiplied, which is the layout of a
//! 32-bit TrueColor visual on a little-endian host, so such a buffer is uploaded
//! verbatim. With no such visual the backend falls back to the root visual, drops the
//! alpha channel while repacking, and reports `platform/x11/no-argb-visual` through
//! [`X11Diagnostics`].

use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};
// x11rb's stream implements the fd trait under this name, and `as_fd` is how the
// connection's descriptor is borrowed safely -- `BorrowedFd::borrow_raw` would be an
// `unsafe` this crate may not carry.
use rustix::fd::AsFd;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shape::{ConnectionExt as ShapeExt, SK, SO};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as XprotoExt,
    CreateGCAux, CreateWindowAux, Gcontext, ImageFormat, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

use super::{
    BYTES_PER_PIXEL, buffer_len, clamp_dimension, clip_rects, logical_dimension, normalize_scale,
    pack_rgb24, physical_size, x11_rectangles,
};

mod translate;

pub(crate) use self::translate::{Decoded, classify_event, effective_alpha, select_argb_visual};
// The event translator's scale-step and alpha constants have no production reader in
// this module; only the platform tests assert on them.
#[cfg(test)]
pub(crate) use self::translate::{OPAQUE_ALPHA, scroll_axis};
use self::translate::{event_mask, scratch_for};

/// Depth of the ARGB visual, and of the buffers uploaded to it.
pub(crate) const ARGB_DEPTH: u8 = 32;

/// Events one `poll_events` call drains; the next poll picks up whatever is left.
const MAX_EVENTS_PER_POLL: usize = 64;

/// The SHAPE extension, used to restrict the interactive region.
const SHAPE_EXTENSION: &str = "SHAPE";

/// The EWMH properties the window carries, in the order [`intern_atoms`] resolves them.
const EWMH_ATOMS: [&[u8]; 9] = [
    b"_NET_WM_NAME",
    b"_NET_WM_PID",
    b"_NET_WM_WINDOW_TYPE",
    b"_NET_WM_WINDOW_TYPE_DOCK",
    b"_NET_WM_STATE",
    b"_NET_WM_STATE_ABOVE",
    b"_NET_WM_STATE_SKIP_TASKBAR",
    b"_NET_WM_STATE_SKIP_PAGER",
    b"UTF8_STRING",
];

/// `WM_CLASS`: instance name, then class name, both `"rspinyin"`, each NUL-terminated.
const WM_CLASS: &[u8] = b"rspinyin\0rspinyin\0";

/// `_NET_WM_NAME`, in UTF-8.
const TITLE: &[u8] = b"rspinyin";

/// `WM_HINTS` with `InputHint` set and `input = False`.
///
/// `input = False` is what tells a window manager this window must not get the keyboard.
const WM_HINTS: [u32; 9] = [1, 0, 0, 0, 0, 0, 0, 0, 0];

/// What the backend detected about its environment.
///
/// The flags are what the caller reports as `platform/x11/no-argb-visual` and
/// `platform/x11/no-compositor`; `protocol_errors` counts X errors, which are never fatal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X11Diagnostics {
    /// The window was created with a 32-bit ARGB visual.
    pub argb_visual: bool,
    /// An active compositor owned `_NET_WM_CM_S<n>` when the window was created.
    pub composited: bool,
    /// The SHAPE extension is available, so the input region can be shaped.
    pub shape_available: bool,
    /// X protocol errors observed so far.
    pub protocol_errors: u32,
}

/// The EWMH atoms the window needs, interned once at construction.
///
/// The field names are abbreviated because the property calls that use them are long;
/// [`EWMH_ATOMS`] spells every atom out, and [`intern_atoms`] fills them in that order.
struct Atoms {
    name: Atom,
    pid: Atom,
    kind: Atom,
    dock: Atom,
    state: Atom,
    above: Atom,
    skip_taskbar: Atom,
    skip_pager: Atom,
    utf8: Atom,
    cm: Atom,
}

/// The X11 window backend.
pub struct X11Backend {
    conn: RustConnection,
    window: Window,
    gc: Gcontext,
    /// Depth of the window: 32 on the ARGB path, the root depth otherwise.
    depth: u8,
    /// The two draw buffers, `Argb8888` premultiplied, `width_px * 4` bytes per row.
    buffers: [Vec<u8>; 2],
    /// Index of the buffer `acquire_buffer` hands out next.
    back: usize,
    /// Index of the buffer uploaded last.
    ///
    /// The X server keeps no backing store for this window, so when it asks for an
    /// exposure the frame has to come from somewhere: it comes from here.
    front: usize,
    /// Repacking scratch for the 24-bit fallback; empty on the ARGB path.
    packed: Vec<u8>,
    width_px: u32,
    height_px: u32,
    width_dp: u32,
    height_dp: u32,
    /// The device pixel ratio the window runs at: the ratio [`Self::apply_scale`] adopts
    /// and [`Self::apply_size`] back-computes the logical canvas with.
    scale: f32,
    /// Last position requested, so a repeated move costs nothing.
    position: (i32, i32),
    input_region: Vec<RectI>,
    composited: bool,
    shape_available: bool,
    protocol_errors: u32,
    mapped: bool,
}

impl X11Backend {
    /// Connects to the X server and creates the window, unmapped.
    ///
    /// `width_dp` and `height_dp` are the logical size, `scale` the device pixel ratio,
    /// and `display` the X display name or `None` to take it from `$DISPLAY`. The window
    /// is fully prepared but not mapped, because mapping it before the first frame would
    /// flash an opaque rectangle on a server without a compositor;
    /// [`SurfaceBackend::set_visible`] maps it. Everything the first key needs therefore
    /// already exists by the time this returns.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when `$DISPLAY` is unset, the display name
    /// does not parse, or the connection or window creation fails; the caller then picks
    /// another backend or falls back to the host's own candidate list.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn connect(
        width_dp: u32,
        height_dp: u32,
        scale: f32,
        display: Option<&str>,
    ) -> Result<Self, PlatformError> {
        let (conn, screen_num) = x11rb::connect(display).map_err(unavailable)?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or(PlatformError::Unavailable)?;
        let root = screen.root;
        let scale = normalize_scale(scale);
        let (width_px, height_px) = physical_size(width_dp, height_dp, scale);
        let (visual, depth) =
            select_argb_visual(screen).unwrap_or((screen.root_visual, screen.root_depth));

        let atoms = intern_atoms(&conn, screen_num)?;
        conn.prefetch_extension_information(SHAPE_EXTENSION)
            .map_err(unavailable)?;
        let shape_available = conn
            .extension_information(SHAPE_EXTENSION)
            .map_err(unavailable)?
            .is_some();
        let composited = selection_owner(&conn, atoms.cm)? != 0;

        let window = conn.generate_id().map_err(unavailable)?;
        let gc = conn.generate_id().map_err(unavailable)?;
        let colormap = conn.generate_id().map_err(unavailable)?;
        // A window whose visual is not the root's must carry a colormap for that visual,
        // or the server rejects the window with `BadMatch`.
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)
            .map_err(unavailable)?;
        conn.create_window(
            depth,
            window,
            root,
            0,
            0,
            width_px as u16,
            height_px as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux {
                // A transparent background: the frame the caller writes replaces it, and
                // on the ARGB path an all-zero pixel is invisible rather than black.
                background_pixel: Some(0),
                colormap: Some(colormap),
                override_redirect: Some(1),
                event_mask: Some(event_mask()),
                ..Default::default()
            },
        )
        .map_err(unavailable)?;
        set_window_properties(&conn, window, &atoms)?;
        conn.create_gc(
            gc,
            window,
            &CreateGCAux {
                // The window has no children of its own to obscure it, so
                // graphics-exposure events would be pure noise.
                graphics_exposures: Some(0),
                ..Default::default()
            },
        )
        .map_err(unavailable)?;

        let buffers = [
            vec![0; buffer_len(width_px, height_px)],
            vec![0; buffer_len(width_px, height_px)],
        ];
        Ok(Self {
            conn,
            window,
            gc,
            depth,
            buffers,
            back: 0,
            front: 0,
            packed: scratch_for(depth, width_px, height_px),
            width_px,
            height_px,
            width_dp: logical_dimension(width_px, scale),
            height_dp: logical_dimension(height_px, scale),
            scale,
            position: (0, 0),
            input_region: Vec::new(),
            composited,
            shape_available,
            protocol_errors: 0,
            mapped: false,
        })
    }

    /// The file descriptor of the X connection, for the UI thread's `poll(2)` loop.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn connection_fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    /// The detection results the caller reports as diagnostics.
    pub fn diagnostics(&self) -> X11Diagnostics {
        X11Diagnostics {
            argb_visual: self.depth == ARGB_DEPTH,
            composited: self.composited,
            shape_available: self.shape_available,
            protocol_errors: self.protocol_errors,
        }
    }

    /// The alpha the renderer should paint the surface base with.
    ///
    /// `requested` is the theme's `base_alpha`. It survives only when the window can
    /// carry alpha and a compositor will blend it; otherwise the base is forced opaque,
    /// because an unblended alpha channel shows as black.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn effective_base_alpha(&self, requested: u8) -> u8 {
        effective_alpha(requested, self.depth == ARGB_DEPTH, self.composited)
    }

    /// Moves the window to a position in screen physical pixels.
    ///
    /// The window is moved, never destroyed and recreated: recreating it would flash. A
    /// move to the position it already has is dropped, because the geometry module
    /// recomputes the position on every frame.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the request cannot be delivered.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), PlatformError> {
        if (x, y) == self.position {
            return Ok(());
        }
        self.conn
            .configure_window(
                self.window,
                &ConfigureWindowAux {
                    x: Some(x),
                    y: Some(y),
                    ..Default::default()
                },
            )
            .map_err(disconnected)?;
        self.position = (x, y);
        self.flush()
    }

    /// The window the X server currently considers focused.
    ///
    /// Read-only, and the probe the focus rule is verified with: the value must be the
    /// same before and after the candidate window is shown.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the query cannot be answered.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn input_focus(&self) -> Result<Window, PlatformError> {
        let cookie = self.conn.get_input_focus().map_err(disconnected)?;
        let reply = cookie.reply().map_err(disconnected)?;
        Ok(reply.focus)
    }

    /// Adopts a new physical size: new buffers, new scratch, input region re-applied.
    ///
    /// The logical size is back-computed with the ratio currently adopted -- the one
    /// [`Self::apply_scale`] maintains -- and with nothing else. A configure and a scale
    /// change can interleave within one poll batch, and a back-computation against any
    /// other ratio would drift the logical canvas away from the size the placement pass
    /// addresses; the adopted ratio is what keeps the two the same.
    fn apply_size(&mut self, width_px: u32, height_px: u32) -> Result<(), PlatformError> {
        // The server can report any size at all, so the value is clamped before it is
        // used to allocate: a bogus configure must not ask for gigabytes of buffers.
        let width_px = clamp_dimension(width_px);
        let height_px = clamp_dimension(height_px);
        self.width_px = width_px;
        self.height_px = height_px;
        self.width_dp = logical_dimension(width_px, self.scale);
        self.height_dp = logical_dimension(height_px, self.scale);
        let len = buffer_len(width_px, height_px);
        self.buffers = [vec![0; len], vec![0; len]];
        self.packed = scratch_for(self.depth, width_px, height_px);
        self.back = 0;
        self.front = 0;
        self.apply_input_region()
    }

    /// Adopts a device pixel ratio: reconfigures the window, reallocates the buffers.
    ///
    /// The window's logical size is the invariant -- the pre-created size is the canvas
    /// the panel draws into, and it is that canvas the placement pass addresses -- so
    /// only the physical size follows the ratio: the window is reconfigured to the same
    /// canvas at the new ratio, the buffers grow around it, and the interactive region is
    /// re-clipped against the new size. The server answers with a configure echo, which
    /// `classify_event` then finds equal to the size adopted here and drops.
    ///
    /// Returns the ratio adopted, or `None` when the backend already runs at it: a
    /// repeated report re-scales nothing.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Disconnected`] when the reconfiguration cannot be
    /// delivered.
    fn apply_scale(&mut self, factor: f32) -> Result<Option<f32>, PlatformError> {
        let scale = normalize_scale(factor);
        if scale.to_bits() == self.scale.to_bits() {
            return Ok(None);
        }
        let (width_px, height_px) = physical_size(self.width_dp, self.height_dp, scale);
        self.conn
            .configure_window(
                self.window,
                &ConfigureWindowAux {
                    width: Some(width_px),
                    height: Some(height_px),
                    ..Default::default()
                },
            )
            .map_err(disconnected)?;
        self.scale = scale;
        self.width_px = width_px;
        self.height_px = height_px;
        let len = buffer_len(width_px, height_px);
        self.buffers = [vec![0; len], vec![0; len]];
        self.packed = scratch_for(self.depth, width_px, height_px);
        self.back = 0;
        self.front = 0;
        self.apply_input_region()?;
        self.flush()?;
        Ok(Some(scale))
    }

    /// Applies the stored interactive region with `SHAPE_INPUT`.
    ///
    /// Without the SHAPE extension the window keeps the region it was created with -- all
    /// of it -- which is the documented degradation: the window is small, so the worst
    /// case is the shadow reserve swallowing clicks near its edge.
    fn apply_input_region(&mut self) -> Result<(), PlatformError> {
        if !self.shape_available {
            return Ok(());
        }
        let region = clip_rects(&self.input_region, self.width_px, self.height_px);
        let rectangles = x11_rectangles(&region);
        self.conn
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                self.window,
                0,
                0,
                &rectangles,
            )
            .map_err(disconnected)?;
        self.flush()
    }

    /// Uploads one of the two buffers to the window.
    fn upload(&mut self, index: usize) -> Result<(), PlatformError> {
        let (width, height) = (self.width_px as u16, self.height_px as u16);
        let Some(buffer) = self.buffers.get(index) else {
            return Err(PlatformError::NoFreeBuffer);
        };
        // A 24-bit surface cannot take an ARGB buffer as it is, so the frame is packed
        // into the scratch first; on the ARGB path it goes out verbatim.
        let (depth, data) = if self.depth == ARGB_DEPTH {
            (ARGB_DEPTH, &buffer[..])
        } else {
            pack_rgb24(buffer, &mut self.packed, self.width_px, self.height_px);
            (self.depth, &self.packed[..])
        };
        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                width,
                height,
                0,
                0,
                0,
                depth,
                data,
            )
            .map_err(disconnected)?;
        Ok(())
    }

    /// Pushes every queued request to the server.
    fn flush(&self) -> Result<(), PlatformError> {
        self.conn.flush().map_err(disconnected)
    }
}

impl SurfaceBackend for X11Backend {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        // `PutImage` copies the frame into the server, so no buffer is ever held by a
        // compositor and this backend never reports `NoFreeBuffer`: the caller is always
        // handed the back buffer, and the commit that follows swaps the two.
        let stride = self.width_px as usize * BYTES_PER_PIXEL as usize;
        let (width, height) = (self.width_px, self.height_px);
        let back = self.back;
        let data = self
            .buffers
            .get_mut(back)
            .ok_or(PlatformError::NoFreeBuffer)?;
        Ok(PixelBufferMut {
            data,
            stride,
            width,
            height,
        })
    }

    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
        // An empty damage set means nothing changed, so there is nothing to upload; the
        // buffer is still committed and the swap still happens, which keeps the two
        // buffers in step with the caller's frame count.
        if !damage.is_empty() {
            let back = self.back;
            self.upload(back)?;
        }
        self.front = self.back;
        self.back = 1 - self.back;
        self.flush()
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        self.input_region = rects.to_vec();
        self.apply_input_region()
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        if visible == self.mapped {
            return Ok(());
        }
        if visible {
            self.conn.map_window(self.window).map_err(disconnected)?;
        } else {
            self.conn.unmap_window(self.window).map_err(disconnected)?;
        }
        self.mapped = visible;
        self.flush()
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        // X11 has no frame callback: without a compositor implementing
        // `_NET_WM_SYNC_REQUEST` there is no "next frame" to be notified about, so the UI
        // thread paces animation with its own timer instead.
        None
    }

    fn connection_fd(&self) -> Option<BorrowedFd<'_>> {
        // The X connection's socket is what a pointer event arrives on, so this is the
        // descriptor the UI thread's poll set has to watch. Borrowed, not owned: the
        // connection keeps the descriptor, and the borrow lives only for this call.
        // `DefaultStream: AsFd` hands the borrow over safely, so no raw-descriptor
        // constructor is needed in this crate.
        Some(self.conn.stream().as_fd())
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        // A scale change can arrive from the surface itself, synthesized from the anchor
        // the host sent: X11 has no scale event source of its own, so this is the only
        // way the ratio the placement runs at reaches the window. Adopting before the
        // queue is decoded is what makes the configure echo of the reconfiguration -- and
        // any echo still queued from an earlier one -- read against the size actually
        // adopted, and the event reported below carries that ratio, exactly as a resize
        // reports the size actually adopted.
        let mut adopted = None;
        for event in out.iter() {
            if let SurfaceEvent::Scale { factor } = *event {
                if let Some(ratio) = self.apply_scale(factor)? {
                    adopted = Some(ratio);
                }
            }
        }
        if let Some(ratio) = adopted {
            out.push(SurfaceEvent::Scale { factor: ratio });
        }
        for _ in 0..MAX_EVENTS_PER_POLL {
            let event = match self.conn.poll_for_event().map_err(disconnected)? {
                Some(event) => event,
                None => return Ok(()),
            };
            match classify_event(&event, self.width_px, self.height_px) {
                Decoded::Surface(surface) => out.push(surface),
                Decoded::Repaint => {
                    let front = self.front;
                    self.upload(front)?;
                }
                Decoded::Resized {
                    width_px,
                    height_px,
                } => {
                    self.apply_size(width_px, height_px)?;
                    // The event reports the size actually adopted, not the one requested.
                    out.push(SurfaceEvent::Resize {
                        w: self.width_px,
                        h: self.height_px,
                    });
                }
                Decoded::ProtocolError => {
                    self.protocol_errors = self.protocol_errors.saturating_add(1);
                }
                Decoded::Ignored => {}
            }
        }
        Ok(())
    }

    fn geometry(&self) -> (u32, u32, f32) {
        (self.width_dp, self.height_dp, self.scale)
    }

    fn backend_id(&self) -> &'static str {
        "x11"
    }
}

/// Interns every EWMH atom the window needs, plus the compositor selection.
///
/// All the requests are queued before the first reply is awaited, so the whole set costs
/// one round trip instead of ten.
fn intern_atoms(conn: &RustConnection, screen_num: usize) -> Result<Atoms, PlatformError> {
    let cm_selection = format!("_NET_WM_CM_S{screen_num}");
    let cookies = EWMH_ATOMS
        .iter()
        .copied()
        .chain(std::iter::once(cm_selection.as_bytes()))
        .map(|name| conn.intern_atom(false, name).map_err(unavailable))
        .collect::<Result<Vec<_>, _>>()?;
    let mut replies = cookies.into_iter();
    let mut next = || -> Result<Atom, PlatformError> {
        let cookie = replies.next().ok_or(PlatformError::Unavailable)?;
        Ok(cookie.reply().map_err(unavailable)?.atom)
    };
    // Bound one at a time, in the order [`EWMH_ATOMS`] lists them, so the mapping from
    // name to field is spelled out here rather than implied by an evaluation order.
    let name = next()?;
    let pid = next()?;
    let kind = next()?;
    let dock = next()?;
    let state = next()?;
    let above = next()?;
    let skip_taskbar = next()?;
    let skip_pager = next()?;
    let utf8 = next()?;
    let cm = next()?;
    Ok(Atoms {
        name,
        pid,
        kind,
        dock,
        state,
        above,
        skip_taskbar,
        skip_pager,
        utf8,
        cm,
    })
}

/// Sets the window's identity and stacking hints.
///
/// The window is override-redirect, so a window manager is not managing it and will
/// mostly ignore these; they are set anyway, because a compositor reads the window type to
/// decide how to treat the window, and because a window with no `WM_CLASS` is
/// indistinguishable from a bug in any diagnostic dump.
fn set_window_properties(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
) -> Result<(), PlatformError> {
    let mode = PropMode::REPLACE;
    let pid = [std::process::id()];
    let dock = [atoms.dock];
    let states = [atoms.above, atoms.skip_taskbar, atoms.skip_pager];
    let class = WM_CLASS;
    let hints = WM_HINTS;
    conn.change_property8(mode, window, atoms.name, atoms.utf8, TITLE)
        .map_err(unavailable)?;
    conn.change_property32(mode, window, atoms.pid, AtomEnum::CARDINAL, &pid)
        .map_err(unavailable)?;
    conn.change_property32(mode, window, atoms.kind, AtomEnum::ATOM, &dock)
        .map_err(unavailable)?;
    conn.change_property32(mode, window, atoms.state, AtomEnum::ATOM, &states)
        .map_err(unavailable)?;
    conn.change_property8(mode, window, AtomEnum::WM_CLASS, AtomEnum::STRING, class)
        .map_err(unavailable)?;
    conn.change_property32(mode, window, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &hints)
        .map_err(unavailable)?;
    Ok(())
}

/// The owner of a selection, or zero when nobody owns it.
fn selection_owner(conn: &RustConnection, selection: Atom) -> Result<Window, PlatformError> {
    let cookie = conn.get_selection_owner(selection).map_err(unavailable)?;
    let reply = cookie.reply().map_err(unavailable)?;
    Ok(reply.owner)
}

/// Maps a failure that happens while the backend is being created.
fn unavailable<T>(_: T) -> PlatformError {
    PlatformError::Unavailable
}

/// Maps a failure on an established connection.
///
/// A request fails either because the connection is gone or because the server rejected
/// it; both leave the display unusable, so both collapse onto `Disconnected`. A mere X
/// protocol error arrives as an `Event::Error` instead, and is counted rather than fatal.
fn disconnected<T>(_: T) -> PlatformError {
    PlatformError::Disconnected
}

#[cfg(test)]
mod tests {
    use super::*;

    // The display-free half of this file's never-takes-focus policy is pinned by
    // the source scans in `crates/ime-ui/tests/focus_policy.rs`: this file sits at
    // its line budget, so those tests live beside the platform module's own suite.

    #[test]
    fn test_connect_with_unparsable_display_reports_unavailable() {
        // A display name without a colon cannot be parsed, so the call fails before any
        // socket is opened and no display server is needed to test it.
        let result = X11Backend::connect(320, 80, 1.0, Some("rspinyin-invalid"));
        assert!(matches!(result, Err(PlatformError::Unavailable)));
    }

    #[test]
    #[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
    fn test_x11_backend_adopts_the_scale_the_surface_synthesizes() {
        let mut backend = X11Backend::connect(320, 80, 1.0, None)
            .expect("a live X server reachable through DISPLAY");
        // The surface hands the synthesized event in through the poll it already drives;
        // the backend adopts it and reports the ratio it actually runs at.
        let mut out = vec![SurfaceEvent::Scale { factor: 2.0 }];
        backend
            .poll_events(&mut out)
            .expect("polling does not fail");
        assert_eq!(
            backend.geometry(),
            (320, 80, 2.0),
            "the ratio is adopted and the logical canvas is the invariant it re-expresses"
        );
        assert!(
            out.contains(&SurfaceEvent::Scale { factor: 2.0 }),
            "the ratio actually adopted is reported back for the window to adopt"
        );
        assert_eq!(
            backend.apply_scale(2.0).expect("the request is delivered"),
            None,
            "a repeated report re-scales nothing"
        );
        // A configure at the adopted ratio back-computes the same canvas: the two cannot
        // drift.
        backend.apply_size(640, 160).expect("the size is adopted");
        assert_eq!(
            backend.geometry(),
            (320, 80, 2.0),
            "the canvas the echo reports is the one the adopted ratio describes"
        );
    }
}
