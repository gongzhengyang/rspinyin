//! The production host boundary: what the effect executor's calls become.
//!
//! Responsibility: turn one input context's [`HostCtx`] calls into the calls that reach
//! Fcitx5, and report what the host refused. This is the only implementation of the
//! boundary that talks to a real host; every other one in this workspace is a test double.
//!
//! Boundaries: the module holds no state beyond the context id it was built for, and it
//! decides nothing. Whether text is committed, whether the preedit area is filled and
//! which command the window is handed are the session's decisions, made above this layer
//! and passed down as arguments. Nothing here blocks: every call is a function call into
//! the glue, which does the work against the objects Fcitx5 already owns.
//!
//! # The candidate window
//!
//! [`HostCtx::post`] has no channel yet. The engine addon reaches the window through the
//! host's own input panel — that is the interface ADR-0003 leaves the two addons to share,
//! since neither links the other — and writing that panel is the user-interface handover's
//! piece, not this boundary's. Until it lands the command is dropped and
//! [`UI_NOT_READY_CODE`] is recorded, which is the degradation `features.md` §2.2.4
//! registers for exactly this state: the frame is committed, the box is not drawn, and the
//! user keeps their text.
//!
//! # The language switch
//!
//! The language switch is Fcitx5's own hotkey (`ASM-02`): the host decides whether a key
//! reaches the engine at all, and an engine that intercepted the key would be taking over
//! a decision that is not its own. Fcitx5 5.1.7's public headers carry no per-context
//! input-method state for an engine to read or write, which matches that design — so
//! [`HostCtx::is_enabled`] answers the only state this boundary can observe, and
//! [`HostCtx::set_enabled`] has no call to make. The read-back is what keeps the status
//! strip truthful: a switch this boundary cannot perform is never reported as performed.
//!
//! # The pure-Rust build
//!
//! Without the `fcitx5-host` feature there is no glue to call, so every wrapper answers
//! its documented default and records `ffi/host-not-linked` once per window. The build
//! exists so the workspace's tests and audits need no Fcitx5 development package; the
//! plugin never runs in it.

// Only the `fcitx5-host` wrappers below spell a `c_char`; the dependency-free build has
// no glue to declare and would otherwise carry an unused import into `-D warnings`.
#[cfg(fcitx5_host)]
use std::ffi::c_char;

use ime_types::{ImeError, UiCommand};

use crate::effects::HostCtx;
use crate::ffi::emit_diagnostic;

/// Recorded when a command for the candidate window has no channel to travel on.
///
/// The code `features.md` §2.2.4 registers for "this frame is committed and not drawn",
/// and the user-interface addon reports the same one for the same state. It is deliberately
/// not a new spelling: the meaning is the registered one, and a second code for it would
/// make a grep for the degradation miss half of it.
pub const UI_NOT_READY_CODE: &str = "ui/not-ready";

/// Recorded when a host call is made in a build with no glue to call.
#[cfg(not(fcitx5_host))]
pub const HOST_NOT_LINKED_CODE: &str = "ffi/host-not-linked";

/// The Fcitx5 host, for one input context.
///
/// Built per callback from the id the host delivered, which is what keeps the production
/// boundary free of an id-to-pointer lookup on the key path: the pointer travels beside
/// the callback in the glue, and is resolved once per call.
pub struct FcitxHost {
    /// The host's identity for the input context these calls belong to.
    ic: u64,
}

impl FcitxHost {
    /// Builds the boundary for one input context.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// A boundary that names that context in every call it makes.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new(ic: u64) -> Self {
        Self { ic }
    }
}

impl HostCtx for FcitxHost {
    fn commit(&mut self, text: &str) -> Result<(), ImeError> {
        if host_commit(self.ic, text) {
            Ok(())
        } else {
            Err(ImeError::FfiInvalidCommit)
        }
    }

    fn set_client_preedit(&mut self, text: &str, caret: u32) -> Result<(), ImeError> {
        if host_set_preedit(self.ic, text, caret) {
            Ok(())
        } else {
            Err(ImeError::FfiInvalidCommit)
        }
    }

    fn clear_client_preedit(&mut self) -> Result<(), ImeError> {
        if host_clear_preedit(self.ic) {
            Ok(())
        } else {
            Err(ImeError::FfiInvalidCommit)
        }
    }

    fn is_enabled(&self) -> bool {
        // An engine is only asked about a key while its input method is the active one
        // for the context, so this is the answer the host's own routing already gave.
        // There is no separate per-context state to read; see the module documentation.
        true
    }

    fn set_enabled(&mut self, _enabled: bool) {
        // No host call exists for this and none is wanted: the switch belongs to Fcitx5's
        // own hotkey handling (`ASM-02`). The caller reads the state back, so a switch
        // that did not happen is never reported as one.
    }

    fn post(&mut self, _command: UiCommand) {
        // No channel yet; see the module documentation. The command is dropped rather than
        // queued: nothing in this process could drain a queue, and the frame the user is
        // waiting for is the next one anyway.
        emit_diagnostic(UI_NOT_READY_CODE);
    }
}

// ── the calls themselves ────────────────────────────────────────────────────────────
//
// One wrapper per host call, so that the boundary above is written once rather than once
// per build. Each is a single `unsafe` call whose safety argument is the same in every
// case: the bytes come from a live slice, and the callee copies them.

/// Inserts `text` into the client of `ic`, answering whether the host took it.
#[cfg(fcitx5_host)]
fn host_commit(ic: u64, text: &str) -> bool {
    // SAFETY: `text` is a live `&str` for the whole call, so the pointer and the length
    // describe an initialised buffer. The callee copies the bytes and keeps no pointer to
    // them; it never writes through the pointer, which is why a shared slice is sound.
    unsafe { rspinyin_host_commit(ic, text.as_ptr().cast::<c_char>(), text.len()) }
}

/// Writes `text` into the client's preedit area, answering whether the host took it.
#[cfg(fcitx5_host)]
fn host_set_preedit(ic: u64, text: &str, caret: u32) -> bool {
    // SAFETY: as `host_commit`: a live slice, read only, for the length of the call. The
    // caret is a byte offset the preedit builder guarantees to be on a character boundary.
    unsafe { rspinyin_host_set_preedit(ic, text.as_ptr().cast::<c_char>(), text.len(), caret) }
}

/// Empties the client's preedit area, answering whether the host took the call.
#[cfg(fcitx5_host)]
fn host_clear_preedit(ic: u64) -> bool {
    // SAFETY: no pointer crosses; the callee resolves the context itself.
    unsafe { rspinyin_host_clear_preedit(ic) }
}

/// Inserts `text` into the client of `ic`, in a build with no glue to call.
///
/// Answers `false` rather than pretending the text landed: the caller turns that into
/// [`ImeError::FfiInvalidCommit`], which is what a host that refused the text reports.
#[cfg(not(fcitx5_host))]
fn host_commit(_ic: u64, _text: &str) -> bool {
    emit_diagnostic(HOST_NOT_LINKED_CODE);
    false
}

/// Writes the preedit area in a build with no glue to call.
#[cfg(not(fcitx5_host))]
fn host_set_preedit(_ic: u64, _text: &str, _caret: u32) -> bool {
    emit_diagnostic(HOST_NOT_LINKED_CODE);
    false
}

/// Empties the preedit area in a build with no glue to call.
#[cfg(not(fcitx5_host))]
fn host_clear_preedit(_ic: u64) -> bool {
    emit_diagnostic(HOST_NOT_LINKED_CODE);
    false
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Inserts `text` into the client of `ic_id`. Defined in `src/ffi/cpp/engine_glue.cpp`.
    ///
    /// # Safety
    ///
    /// `text` must point to `len` initialised bytes that stay valid and unaliased for
    /// reads for the whole of the call. The callee copies them and keeps no pointer.
    fn rspinyin_host_commit(ic_id: u64, text: *const c_char, len: usize) -> bool;

    /// Writes the client's preedit area. Defined in `src/ffi/cpp/engine_glue.cpp`.
    ///
    /// # Safety
    ///
    /// As [`rspinyin_host_commit`]; `caret` must be a byte offset on a character boundary
    /// of that buffer.
    fn rspinyin_host_set_preedit(ic_id: u64, text: *const c_char, len: usize, caret: u32) -> bool;

    /// Empties the client's preedit area. Defined in `src/ffi/cpp/engine_glue.cpp`.
    ///
    /// # Safety
    ///
    /// No pointer crosses the boundary; the callee resolves the context itself.
    fn rspinyin_host_clear_preedit(ic_id: u64) -> bool;
}
