//! The input-method-engine callbacks: the host-to-engine direction of the ABI.
//!
//! Responsibility: validate at the boundary what the host hands over — a key event, a
//! commit or preedit buffer — and forward it to the crate module that owns the answer.
//! Every entry point runs under the panic guard and answers with its documented
//! fallback instead of unwinding into C++.
//!
//! Boundaries: nothing here decides what input means. The key router lives in `ime-core`,
//! so no callback on this path allocates without bound, blocks or resolves geometry on
//! the host thread.
//!
//! The input panel and the caret rectangle are **not** here. Both belong to the
//! user-interface role and are read by `crates/ime-ui-addon` from the `InputContext`
//! the host hands it; the engine addon never sees them.

use std::ffi::{c_char, c_void};

use crate::ffi::{emit_diagnostic, guard_ffi};

use super::{FcitxKeyEvent, bytes_from_raw};

/// An input context switched to this input method.
pub extern "C" fn on_activate(_context: *mut c_void, _ic_id: u64) -> bool {
    guard_ffi(false, || {
        // Stub: starting the composing session for this input context belongs to the
        // session lifecycle work; `false` reports that no session is active yet.
        false
    })
}

/// An input context switched away from this input method.
pub extern "C" fn on_deactivate(_context: *mut c_void, _ic_id: u64) {
    guard_ffi((), || {
        // Stub: discarding the composing session belongs to the session lifecycle work.
    });
}

/// An input context was reset.
pub extern "C" fn on_reset(_context: *mut c_void, _ic_id: u64) {
    guard_ffi((), || {
        // Stub: clearing preedit and candidates belongs to the session lifecycle work.
    });
}

/// A key press or release.
///
/// Returns whether the engine consumed the key. `false` keeps the key in the normal
/// Fcitx5 pipeline so nothing is swallowed; a null event pointer is also answered
/// with `false`.
pub extern "C" fn on_key_event(
    _context: *mut c_void,
    _ic_id: u64,
    event: *const FcitxKeyEvent,
) -> bool {
    guard_ffi(false, || {
        if event.is_null() {
            emit_diagnostic("ffi/null-key-event");
            return false;
        }
        // SAFETY: non-null and owned by the caller for the duration of this call;
        // `FcitxKeyEvent` is `repr(C)` and every bit pattern is a valid value.
        let _key = unsafe { *event };
        // Stub: `sym`, `state`, `is_release` and `time_ms` drive the session once key
        // routing exists. Until then the key must not be swallowed, so the answer
        // stays `false`.
        false
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
