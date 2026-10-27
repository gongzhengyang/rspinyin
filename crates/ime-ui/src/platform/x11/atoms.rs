//! The free X11 helpers the backend's setup path uses: atom interning, window
//! properties, the selection and the two error collapses.
//!
//! Split out of `x11.rs` to keep that file inside the line limit. None of them touch
//! backend state -- each takes the connection it works on as an argument -- which is
//! what keeps them free functions rather than methods.

use super::*;

/// Interns every EWMH atom the window needs, plus the compositor selection.
///
/// All the requests are queued before the first reply is awaited, so the whole set costs
/// one round trip instead of ten.
pub(super) fn intern_atoms(
    conn: &RustConnection,
    screen_num: usize,
) -> Result<Atoms, PlatformError> {
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
pub(super) fn set_window_properties(
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
pub(super) fn selection_owner(
    conn: &RustConnection,
    selection: Atom,
) -> Result<Window, PlatformError> {
    let cookie = conn.get_selection_owner(selection).map_err(unavailable)?;
    let reply = cookie.reply().map_err(unavailable)?;
    Ok(reply.owner)
}

/// Maps a failure that happens while the backend is being created.
pub(super) fn unavailable<T>(_: T) -> PlatformError {
    PlatformError::Unavailable
}

/// Maps a failure on an established connection.
///
/// A request fails either because the connection is gone or because the server rejected
/// it; both leave the display unusable, so both collapse onto `Disconnected`. A mere X
/// protocol error arrives as an `Event::Error` instead, and is counted rather than fatal.
pub(super) fn disconnected<T>(_: T) -> PlatformError {
    PlatformError::Disconnected
}
