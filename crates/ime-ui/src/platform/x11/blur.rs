//! The KDE blur-behind negotiation for the X11 backend.
//!
//! Responsibility: detect at connection time whether this window manager takes a
//! `_KDE_NET_WM_BLUR_BEHIND_REGION` request at all -- the atom must already exist and
//! the window manager must be KWin, the compositor that reads the property -- and carry
//! a request from the surface to the window, either directly on the backend through the
//! [`BlurSurface`] capability or through the shared [`BlurHandle`] a caller keeps after
//! the backend has been boxed for the platform.
//!
//! Boundaries: this module speaks one property and nothing else. It does not decide
//! whether the window wants blur -- the theme's acrylic flag and the degradation ladder
//! in `crate::theme` own that -- and it never blocks: the applied-or-refused answer
//! comes from the support detected at construction, not from a round trip, because the
//! request runs on the UI thread and a window manager that will not answer must not
//! stall it.
//!
//! # The protocol
//!
//! KWin reads the region from a `CARDINAL` property on the client window: a flat list
//! of `x`, `y`, `width`, `height` quadruples in window-relative physical pixels, one
//! per rectangle. An empty request deletes the property, which is the protocol's own
//! "no blur". The write itself is fire-and-forget -- one queued `change_property` and a
//! flush -- so a request costs no reply wait.
//!
//! # Why the capability is a handle and not the backend
//!
//! The frozen `SurfaceBackend` trait has no blur request and the backend lives behind
//! the platform's `RefCell` once it has been boxed, so the probe cannot lend a second
//! mutable borrow of it to the surface. The request therefore travels through a slot
//! the two share: the handle answers at once and leaves the region for the backend's
//! next event poll to write, which keeps the one-connection ownership intact.

use std::sync::{Arc, Mutex};

use ime_types::{PlatformError, RectI};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

use crate::theme::BlurSurface;

/// The property KWin reads the blur region from.
const BLUR_BEHIND_REGION: &[u8] = b"_KDE_NET_WM_BLUR_BEHIND_REGION";

/// The EWMH property naming the window whose `_NET_WM_NAME` is the manager's own.
const SUPPORTING_WM_CHECK: &[u8] = b"_NET_SUPPORTING_WM_CHECK";

/// The EWMH property the manager window carries its name in.
const WM_NAME: &[u8] = b"_NET_WM_NAME";

/// The type `_NET_WM_NAME` is written as.
const UTF8_STRING: &[u8] = b"UTF8_STRING";

/// The name of the one window manager that reads [`BLUR_BEHIND_REGION`].
const KWIN: &[u8] = b"KWin";

/// How much of a window manager's name is read to identify it: any real name is
/// shorter, and the identification only needs the first bytes anyway.
const WM_NAME_LIMIT: u32 = 64;

/// The blur state of one X11 window.
///
/// Detection happens once, in [`Self::open`], while the backend is being built: the
/// atom and the manager's name are read there, and everything afterwards -- the
/// handle's answer, the property writes -- works from that answer.
pub(crate) struct BlurChannel {
    /// The interned blur atom, present only when this window manager takes requests.
    atom: Option<Atom>,
    /// The request slot shared with [`BlurHandle`].
    slot: Arc<Mutex<BlurSlot>>,
}

/// The latest blur request a handle has taken, behind a mutex.
struct BlurSlot {
    /// Whether the compositor takes the request at all, as detected at construction.
    available: bool,
    /// The region of the latest request, until the backend writes it to the window.
    pending: Option<Vec<RectI>>,
}

impl BlurChannel {
    /// Detects this window manager's blur support and opens the request slot.
    ///
    /// A window manager that is not KWin -- and one whose name cannot be read at all --
    /// is a compositor without this blur, not a failure: the answer is recorded in the
    /// slot and every request on it is refused, which is exactly the degradation the
    /// theme's ladder exists to handle. A read error means the connection is going
    /// away anyway, and the backend's next call reports that through its own error.
    pub(crate) fn open(conn: &RustConnection, root: Window) -> Self {
        // The atom must already exist: KWin interns it at start-up, so its absence is
        // a compositor that will never read the property, and creating the name here
        // would turn that fact into a lie.
        let detected = intern_atom(conn, BLUR_BEHIND_REGION, true).unwrap_or(None);
        let atom = if detected.is_some() && is_kwin(conn, root).unwrap_or(false) {
            detected
        } else {
            None
        };
        Self {
            atom,
            slot: Arc::new(Mutex::new(BlurSlot {
                available: atom.is_some(),
                pending: None,
            })),
        }
    }

    /// The capability a caller keeps after the backend has been boxed for the platform.
    pub(crate) fn handle(&self) -> BlurHandle {
        BlurHandle {
            slot: Arc::clone(&self.slot),
        }
    }

    /// Writes the latest request a handle has taken, if there is one and this window
    /// manager takes requests at all.
    ///
    /// Called from the backend's event poll, which the UI thread drives anyway: the
    /// write is one queued `change_property` and a flush, never a round trip. A slot
    /// whose lock was poisoned by a panicking writer loses the request, which is the
    /// degradation a window decoration is allowed.
    pub(crate) fn drain(&self, conn: &RustConnection, window: Window) -> Result<(), PlatformError> {
        let Some(atom) = self.atom else {
            return Ok(());
        };
        let pending = self
            .slot
            .lock()
            .ok()
            .and_then(|mut slot| slot.pending.take());
        match pending {
            Some(region) => write_region(conn, window, atom, &region),
            None => Ok(()),
        }
    }

    /// The direct request path, for a caller that still holds the backend.
    fn request(
        &self,
        conn: &RustConnection,
        window: Window,
        region: &[RectI],
    ) -> Result<(), PlatformError> {
        let atom = self.atom.ok_or(PlatformError::Unavailable)?;
        write_region(conn, window, atom, region)
    }
}

/// The blur capability of an X11 window, shared between the backend and its caller.
///
/// The backend owns the connection and the window, and once it has been boxed for the
/// Slint platform this handle is the only way left to ask for blur. A request on the
/// handle is answered at once, from the support detected when the backend connected --
/// so a window manager that will not answer cannot stall the caller -- and the region
/// it names reaches the window on the backend's next event poll.
///
/// # Concurrency
///
/// The handle is `Send` and `Sync`: the slot is a mutex over a flag and the latest
/// region, held for the length of one request and nothing else. It is not a queue --
/// the newest request replaces the pending one, which is the latest-wins shape the
/// surface's placement updates want.
pub struct BlurHandle {
    slot: Arc<Mutex<BlurSlot>>,
}

impl BlurSurface for BlurHandle {
    fn request_blur(&mut self, region: &[RectI]) -> Result<(), PlatformError> {
        let mut slot = self.slot.lock().map_err(|_| PlatformError::Unavailable)?;
        if !slot.available {
            return Err(PlatformError::Unavailable);
        }
        slot.pending = Some(region.to_vec());
        Ok(())
    }
}

/// Hands the backend's own connection to the negotiation: the synchronous path for a
/// caller that still holds the [`super::X11Backend`], writing the property inside the
/// call instead of leaving it for the next event poll.
impl BlurSurface for super::X11Backend {
    fn request_blur(&mut self, region: &[RectI]) -> Result<(), PlatformError> {
        self.blur.request(&self.conn, self.window, region)
    }
}

/// Interns an atom, answering `None` when `only_if_exists` is set and nobody has the
/// name interned.
fn intern_atom(
    conn: &RustConnection,
    name: &[u8],
    only_if_exists: bool,
) -> Result<Option<Atom>, PlatformError> {
    let cookie = conn
        .intern_atom(only_if_exists, name)
        .map_err(super::unavailable)?;
    let atom = cookie.reply().map_err(super::unavailable)?.atom;
    Ok((atom != 0).then_some(atom))
}

/// The first 32-bit element of a property, or `0` when it carries none.
fn property_word(
    conn: &RustConnection,
    window: Window,
    property: Atom,
    kind: impl Into<Atom>,
) -> Result<u32, PlatformError> {
    let cookie = conn
        .get_property(false, window, property, kind, 0, 1)
        .map_err(super::unavailable)?;
    let reply = cookie.reply().map_err(super::unavailable)?;
    Ok(reply
        .value32()
        .and_then(|mut words| words.next())
        .unwrap_or(0))
}

/// The bytes of an 8-bit property, up to [`WM_NAME_LIMIT`].
fn property_bytes(
    conn: &RustConnection,
    window: Window,
    property: Atom,
    kind: impl Into<Atom>,
) -> Result<Vec<u8>, PlatformError> {
    // The length is counted in 32-bit units whatever the format, so 8-bit data comes
    // back at four bytes per unit.
    let cookie = conn
        .get_property(false, window, property, kind, 0, WM_NAME_LIMIT / 4)
        .map_err(super::unavailable)?;
    Ok(cookie.reply().map_err(super::unavailable)?.value)
}

/// Whether the window manager running is KWin, by the EWMH identification.
///
/// The root's [`SUPPORTING_WM_CHECK`] names a child window the manager owns, and that
/// child's `_NET_WM_NAME` is the manager's own name. A root without the check, a
/// manager window without a name, or a name that is not KWin's all read as a window
/// manager that will never take the request.
fn is_kwin(conn: &RustConnection, root: Window) -> Result<bool, PlatformError> {
    let Some(check) = intern_atom(conn, SUPPORTING_WM_CHECK, true)? else {
        return Ok(false);
    };
    let manager = property_word(conn, root, check, AtomEnum::WINDOW)?;
    if manager == 0 {
        return Ok(false);
    }
    let name = intern_atom(conn, WM_NAME, false)?.unwrap_or(0);
    let utf8 = intern_atom(conn, UTF8_STRING, false)?.unwrap_or(0);
    if name == 0 || utf8 == 0 {
        return Ok(false);
    }
    Ok(is_kwin_name(&property_bytes(conn, manager, name, utf8)?))
}

/// Whether a window manager's name identifies KWin.
///
/// The manager writes its own name, `KWin`, so the match is on that exact prefix and
/// is case-sensitive: a name that merely contains the letters is some other project.
fn is_kwin_name(name: &[u8]) -> bool {
    name.starts_with(KWIN)
}

/// Writes the blur region to the window, or deletes the property when it is empty.
///
/// An empty region is the protocol's own "no blur", which is what a caller that has
/// nothing to name asks for. The coordinates go out through the cast that keeps the
/// bit pattern: they are signed, and KWin decodes the `CARDINAL` property as signed
/// again, so the round trip is exact.
fn write_region(
    conn: &RustConnection,
    window: Window,
    atom: Atom,
    region: &[RectI],
) -> Result<(), PlatformError> {
    if region.is_empty() {
        conn.delete_property(window, atom)
            .map_err(super::disconnected)?;
    } else {
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atom,
            AtomEnum::CARDINAL,
            &region_payload(region),
        )
        .map_err(super::disconnected)?;
    }
    conn.flush().map_err(super::disconnected)
}

/// The flat `x, y, width, height` quadruples KWin reads the region as.
fn region_payload(region: &[RectI]) -> Vec<u32> {
    region
        .iter()
        .flat_map(|rect| [rect.x as u32, rect.y as u32, rect.w, rect.h])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{BlurNegotiation, request_blur};

    /// A channel whose slot the test filled in by hand, for the display-free half.
    fn channel(available: bool) -> BlurChannel {
        BlurChannel {
            atom: None,
            slot: Arc::new(Mutex::new(BlurSlot {
                available,
                pending: None,
            })),
        }
    }

    #[test]
    fn test_region_payload_lists_one_xywh_quadruple_per_rectangle() {
        let region = [
            RectI {
                x: -8,
                y: 4,
                w: 120,
                h: 36,
            },
            RectI {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            },
        ];
        // The signed coordinates keep their bit pattern through the `CARDINAL` type,
        // which is what KWin decodes back as signed.
        assert_eq!(
            region_payload(&region),
            vec![(-8i32) as u32, 4, 120, 36, 1, 2, 3, 4]
        );
    }

    #[test]
    fn test_region_payload_of_no_rectangles_is_empty() {
        assert!(region_payload(&[]).is_empty());
    }

    #[test]
    fn test_kwin_name_matches_the_prefix_the_manager_writes() {
        assert!(is_kwin_name(b"KWin"));
        assert!(
            is_kwin_name(b"KWin (X11)"),
            "what follows the name is not the name"
        );
        assert!(
            !is_kwin_name(b"kwin"),
            "the name is the manager's own, case included"
        );
        assert!(
            !is_kwin_name(b"Kwi"),
            "a prefix of the name is not the name"
        );
        assert!(!is_kwin_name(b"i3"));
        assert!(!is_kwin_name(b""));
    }

    #[test]
    fn test_blur_handle_refuses_when_the_compositor_has_no_blur() {
        let mut handle = channel(false).handle();
        let region = [RectI {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        }];
        assert!(
            handle.request_blur(&region).is_err(),
            "a compositor detected as unable to take the request refuses it"
        );
        assert!(
            handle
                .slot
                .lock()
                .expect("the slot is not poisoned")
                .pending
                .is_none(),
            "a refused request leaves nothing for the backend to write"
        );
    }

    #[test]
    fn test_blur_handle_accepts_and_keeps_only_the_latest_region() {
        let mut handle = channel(true).handle();
        let first = [RectI {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        }];
        let second = [RectI {
            x: 4,
            y: 4,
            w: 20,
            h: 20,
        }];
        assert!(handle.request_blur(&first).is_ok());
        assert!(handle.request_blur(&second).is_ok());
        // The backend side drains through `take`: the newest request wins and the
        // slot is empty again afterwards.
        let pending = handle
            .slot
            .lock()
            .expect("the slot is not poisoned")
            .pending
            .take();
        assert_eq!(pending, Some(second.to_vec()));
        let again = handle
            .slot
            .lock()
            .expect("the slot is not poisoned")
            .pending
            .take();
        assert_eq!(again, None, "a drained slot stays drained");
    }

    #[test]
    fn test_blur_channel_handle_shares_the_slot_with_the_backend() {
        let channel = channel(true);
        let mut handle = channel.handle();
        let region = [RectI {
            x: 0,
            y: 0,
            w: 6,
            h: 6,
        }];
        assert!(handle.request_blur(&region).is_ok());
        let pending = channel
            .slot
            .lock()
            .expect("the slot is not poisoned")
            .pending
            .take();
        assert_eq!(
            pending,
            Some(region.to_vec()),
            "what the handle took is what the backend's drain finds"
        );
    }

    #[test]
    #[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
    fn test_x11_backend_blur_request_answers_inside_the_call() {
        let mut backend = super::super::X11Backend::connect(320, 80, 1.0, None)
            .expect("a live X server reachable through DISPLAY");
        let region = [RectI {
            x: 1,
            y: 1,
            w: 10,
            h: 10,
        }];
        // Either answer is a working negotiation: KWin takes the request, any other
        // window manager refuses it. What must never happen is a block or a panic --
        // the answer comes from the detection made at construction, not from a
        // round trip.
        let outcome = request_blur(&mut backend, &region);
        assert!(matches!(
            outcome,
            BlurNegotiation::Applied | BlurNegotiation::Refused
        ));
    }
}
