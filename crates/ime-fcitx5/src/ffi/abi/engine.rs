//! The input-method-engine callbacks: the host-to-engine direction of the ABI.
//!
//! Responsibility: validate at the boundary what the host hands over — a key event, a
//! commit or preedit buffer — and forward it to the crate module that owns the answer.
//! Every entry point runs under the panic guard and answers with its documented
//! fallback instead of unwinding into C++.
//!
//! Boundaries: nothing here decides what input means. A key is handed to
//! [`crate::session_host`], which routes it through the session of the context it arrived
//! for; what the plugin then does about it leaves through [`crate::effects`], whose
//! production implementation is the `host` module beside this one. The callbacks below
//! build that boundary per call and pass it down, so no callback on this path allocates
//! without bound, blocks or resolves geometry on the host thread.
//!
//! The candidate panel and the caret rectangle are **not** here. Both belong to the
//! user-interface role, which reads them from the `InputContext` the host hands it; the
//! engine writes only the client's own preedit area, and the panel the window draws from
//! is not this addon's to fill.

use std::ffi::{c_char, c_void};

use super::{FcitxKeyEvent, bytes_from_raw};

use crate::effects::EffectHost;
use crate::engine::host::Host;
use crate::ffi::{emit_diagnostic, guard_ffi};

mod host;

use self::host::FcitxHost;

/// Runs `call` against the production host boundary for `ic_id`.
///
/// The adapter is built per call because the two shapes meet here and nowhere else: one
/// engine routes for every input context and passes the id on each call, while the
/// boundary the effect executor speaks is built for one context. Building it allocates
/// nothing — it is an id and two references — so a keystroke pays no cost for the split.
fn with_host<T>(ic_id: u64, call: impl FnOnce(&mut dyn Host) -> T) -> T {
    let mut host = FcitxHost::new(ic_id);
    call(&mut EffectHost::new(ic_id, &mut host))
}

/// An input context switched to this input method.
///
/// Returns whether a session is live for the context afterwards. `false` means the plugin
/// has nothing to route for it — no session host is installed — which the glue reports as
/// pure-engine mode; it never means a session was dropped.
pub extern "C" fn on_activate(_context: *mut c_void, ic_id: u64) -> bool {
    guard_ffi(false, || crate::session_host::activate(ic_id))
}

/// An input context switched away from this input method.
///
/// The composition is discarded and never committed, and the window is hidden with it:
/// the host-side call sequence is the one [`on_reset`] produces, and the difference is
/// that the session goes away with the context rather than staying behind.
pub extern "C" fn on_deactivate(_context: *mut c_void, ic_id: u64) {
    guard_ffi((), || {
        with_host(ic_id, |host| crate::session_host::deactivate(ic_id, host));
    });
}

/// An input context was reset.
///
/// The host calls this when the input method is reset — a focus change, a mode switch —
/// without the context going away. What the user was composing is taken back and nothing
/// is committed, which is what keeps a half-typed word out of the application.
pub extern "C" fn on_reset(_context: *mut c_void, ic_id: u64) {
    guard_ffi((), || {
        with_host(ic_id, |host| crate::session_host::reset(ic_id, host));
    });
}

/// A key press or release.
///
/// Returns whether the engine consumed the key. `false` keeps the key in the normal
/// Fcitx5 pipeline so nothing is swallowed; a null event pointer is also answered
/// with `false`.
pub extern "C" fn on_key_event(
    _context: *mut c_void,
    ic_id: u64,
    event: *const FcitxKeyEvent,
) -> bool {
    guard_ffi(false, || {
        if event.is_null() {
            emit_diagnostic("ffi/null-key-event");
            return false;
        }
        // SAFETY: non-null and owned by the caller for the duration of this call;
        // `FcitxKeyEvent` is `repr(C)` and every bit pattern is a valid value. The copy
        // is what lets the borrow end here rather than travel into the routing layer.
        let key = unsafe { *event };
        with_host(ic_id, |host| crate::session_host::key_event(ic_id, &key, host))
    })
}

/// An input context gained focus.
pub extern "C" fn on_focus_in(_context: *mut c_void, _ic_id: u64) {
    guard_ffi((), || {
        // Stub: focus tracking belongs to the session lifecycle work.
    });
}

/// An input context lost focus.
pub extern "C" fn on_focus_out(_context: *mut c_void, _ic_id: u64) {
    guard_ffi((), || {
        // Stub: focus tracking belongs to the session lifecycle work. Losing focus is
        // the project's highest-severity defect, so the handling must be deliberate.
    });
}

/// Text to insert into the client.
///
/// A null pointer or a zero length is the host clearing a buffer, not text: it is
/// diagnosed as `ffi/invalid-commit` and ignored, never treated as an empty commit.
pub extern "C" fn on_commit_string(
    _context: *mut c_void,
    _ic_id: u64,
    text: *const c_char,
    len: usize,
) {
    guard_ffi((), || {
        // SAFETY: `c_char` and `u8` have the same size and alignment; the host
        // guarantees the buffer is valid for `len` bytes.
        match unsafe { bytes_from_raw(text.cast::<u8>(), len) } {
            Some(_bytes) => {
                // Stub: handing the text to the client belongs to the commit work.
            }
            None => emit_diagnostic("ffi/invalid-commit"),
        }
    });
}

/// Preedit text plus its caret position in bytes.
pub extern "C" fn on_set_preedit(
    _context: *mut c_void,
    _ic_id: u64,
    text: *const c_char,
    len: usize,
    _caret: u32,
) {
    guard_ffi((), || {
        // SAFETY: `c_char` and `u8` have the same size and alignment; the host
        // guarantees the buffer is valid for `len` bytes.
        match unsafe { bytes_from_raw(text.cast::<u8>(), len) } {
            Some(_bytes) => {
                // Stub: rendering the preedit belongs to the preedit work.
            }
            None => emit_diagnostic("ffi/invalid-preedit"),
        }
    });
}

/// Drop the preedit.
pub extern "C" fn on_clear_preedit(_context: *mut c_void, _ic_id: u64) {
    guard_ffi((), || {
        // Stub: clearing the preedit belongs to the preedit work.
    });
}
