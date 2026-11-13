//! The C ABI's entry points onto the session slot: the free functions the FFI
//! layer and the addon's tests drive.
//!
//! Responsibility: turn one host callback — an activation, a focus change, a
//! reset, a key, a window event, a configuration reload or the unload sweep —
//! into the same-named call on the session host, through the one process-wide
//! slot. Every entry answers its documented safe default when nothing is
//! installed, which is why none of them can fail. The slot itself, its
//! installation and the [`SessionHost`](super::SessionHost) the calls run on
//! stay in the parent module; this submodule is only the surface they are named
//! through, re-exported there so the callers keep spelling `session_host::key_event`
//! and its siblings on that module.
//!
//! # Threading
//!
//! Everything here runs on the Fcitx5 host thread, one call at a time; the slot
//! lock is held for the length of one callback, which is the guarantee the parent
//! module documents.

use ime_types::{Anchor, UiEvent};

use crate::engine::host::Host;
use crate::engine::router::RoutingConfig;
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::ContextReport;

use super::{lock_sessions, with_sessions};

/// Creates the session of an input context the host switched to this input method.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
///
/// # Returns
///
/// Whether a session is live for the context afterwards, which is what the C ABI's
/// activation callback reports. `false` means nothing is installed, so the host was told
/// nothing was set up — it never means a session was dropped.
///
/// # Panics
///
/// Never.
pub fn activate(ic: u64) -> bool {
    with_sessions(false, |sessions| {
        sessions.activate(ic);
        true
    })
}

/// Creates the session of an input context the host has described.
///
/// The C ABI currently delivers an activation as the context id alone, so the plain
/// [`activate`] observes every context as unreported; this entry is the shape the FFI
/// layer calls once the vtable forwards the capability flags and the program name, and
/// the shape the tests use to stand in for a host that reported an ordinary context.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `report` — what the host read off the context, or [`ContextReport::Unreported`].
///
/// # Returns
///
/// Whether a session is live for the context afterwards, as [`activate`] answers.
///
/// # Panics
///
/// Never.
pub fn activate_reported(ic: u64, report: ContextReport<'_>) -> bool {
    with_sessions(false, |sessions| {
        sessions.activate_reported(ic, report);
        true
    })
}

/// Drops the session of an input context the host switched away from.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `host` — the boundary the effects of ending the session are executed against.
///
/// # Panics
///
/// Never.
pub fn deactivate(ic: u64, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.deactivate(ic, host));
}

/// Takes back the composition of an input context that stays active.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `host` — the boundary the effects are executed against.
///
/// # Panics
///
/// Never.
pub fn reset(ic: u64, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.reset(ic, host));
}

/// Ensures a session exists for an input context that gained focus.
///
/// A session that is already there is reused as it stands — the host reusing an id, or a
/// focus coming back to a context a focus loss left behind, must not rebuild the session
/// under it. One that is not there is created, exactly as [`activate`] would create it.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
///
/// # Panics
///
/// Never.
pub fn focus_in(ic: u64) {
    with_sessions((), |sessions| sessions.focus_in(ic));
}

/// Takes the composition of an input context that lost focus, and keeps the session.
///
/// Nothing is committed, the window is hidden and the application's preedit area is
/// emptied; the session and the privacy state stay, so a focus that comes back finds the
/// context it left. The loss also counts towards reclamation: the
/// `FOCUS_OUTS_BEFORE_RECLAIM`-th loss in a row for a context no focus came back to
/// reclaims it, its session and its privacy state with it.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `host` — the boundary the effects are executed against.
///
/// # Panics
///
/// Never.
pub fn focus_out(ic: u64, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.focus_out(ic, host));
}

/// Routes one key event and answers whether the key was consumed.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context the key arrived in.
/// * `key` — the key as the host delivered it.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Returns
///
/// Whether the plugin acted on the key. `false` for a key that must keep travelling, and
/// `false` when no session host is installed.
///
/// # Panics
///
/// Never.
pub fn key_event(ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
    with_sessions(false, |sessions| sessions.key_event(ic, key, host))
}

/// Routes one event from the candidate window.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context the event belongs to.
/// * `event` — what the user did to the window.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Returns
///
/// Whether anything other than a diagnostic reached the host.
///
/// # Panics
///
/// Never.
pub fn ui_event(ic: u64, event: UiEvent, host: &mut dyn Host) -> bool {
    with_sessions(false, |sessions| sessions.ui_event(ic, event, host))
}

/// Records where the host reports the caret, for the frames that follow.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `anchor` — the cursor rectangle, the screen it is on and the scale factor.
///
/// # Panics
///
/// Never.
pub fn set_anchor(ic: u64, anchor: Anchor) {
    with_sessions((), |sessions| sessions.set_anchor(ic, anchor));
}

/// Adopts a reloaded configuration without disturbing a composition in progress.
///
/// # Arguments
///
/// * `config` — the configuration that replaces the one in force.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Panics
///
/// Never.
pub fn reload(config: RoutingConfig, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.reload(config, host));
}

/// Takes the session host out of the process and reports how many sessions were live.
///
/// The sweep the addon runs at unload. It drops the sessions rather than stepping each
/// one with `SessionEvent::Reset`, and the difference is the host: a reset's effects — a
/// hide posted to the candidate window — have nowhere to go at unload, because the
/// lifecycle module reaches neither the C ABI nor the window's addon, and the window's
/// addon is being torn down with this one. Nothing is committed either way, which is what
/// the unload criterion asks for: no session is left behind and no candidate can still be
/// taken. [`SessionHost::shutdown`](super::SessionHost::shutdown) is the same sweep for
/// a caller that does have a host.
///
/// # Returns
///
/// The number of sessions that were live, for the line the caller reports.
///
/// # Panics
///
/// Never.
pub fn shutdown() -> usize {
    match lock_sessions().take() {
        Some(sessions) => sessions.live_contexts(),
        None => 0,
    }
}
