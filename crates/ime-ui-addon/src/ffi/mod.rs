//! FFI infrastructure between this crate and the Fcitx5 C++ host.
//!
//! This module is the crate's only `unsafe` boundary and the only place allowed to
//! cross it: the C ABI contract in [`abi`], the panic guard every entry point runs
//! under, and the crash channel those entry points report through.
//!
//! # Layout
//!
//! * [`abi`] — the `#[repr(C)]` contract between the UI glue and this crate, the
//!   callback table the glue caches, and the entry points it resolves by name.
//! * `guard_ffi` / `catch_ffi` — the panic guard. Unwinding into C++ is undefined
//!   behaviour, so a panic has to become a value at the boundary; the workspace
//!   profile deliberately keeps unwinding enabled, without which the guard could not
//!   catch anything.
//! * `write_stderr_line` — the crash channel. It bypasses the diagnostics layer on
//!   purpose: the addon can be rejected or panic before that layer is initialised.
//!
//! # Why this duplicates `ime-fcitx5::ffi`
//!
//! The two libraries are `dlopen`'d independently as separate Fcitx5 addons and share
//! no state, so a common crate holding this code would put a link-time relationship
//! between them — the thing the split exists to remove. The C++ side accepts the same
//! trade: the `#[repr(C)]` struct definitions are repeated in each glue translation
//! unit, and `addon_glue.cpp` records why there is deliberately no shared header.
//!
//! # Threading
//!
//! Everything here runs on the Fcitx5 host thread, inside a host callback. No entry
//! point blocks, allocates without bound, or takes a lock, so this layer never holds
//! up the host's main loop.
// The FFI boundary is the one place in this crate allowed to use `unsafe`.
// The callbacks take raw pointers from the host and read through them. They cannot be
// `unsafe fn`: the C ABI contract (and the C++ glue) stores plain function pointers, and
// the pointer validity is the host's guarantee, documented in each `# Safety` section.
// The `unsafe_code` allowance for this module lives on the `pub mod ffi;` declaration in
// `lib.rs`, deliberately not here: a crate-level `#![allow]` would also permit raw
// pointers anywhere else in the crate, which the unsafe audit rejects.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub mod abi;

pub use abi::{
    FcitxCursorRect, RSPINYIN_ABI_VERSION, RSPINYIN_UI_VTABLE, RspinyinUiVtable, UiActivation,
    UiPanelSnapshot, activate_ui, current_ui, rspinyin_ui_plugin_init,
};

/// A panic caught at the FFI boundary, ready for the crash channel.
#[derive(Debug)]
pub(crate) struct PanicReport {
    /// Message taken from the panic payload, or a placeholder when the payload was
    /// not a string.
    message: String,
}

impl PanicReport {
    /// Extracts the message a `catch_unwind` payload carries.
    fn from_payload(payload: &(dyn Any + Send)) -> Self {
        if let Some(message) = payload.downcast_ref::<&'static str>() {
            return Self {
                message: (*message).to_owned(),
            };
        }
        if let Some(message) = payload.downcast_ref::<String>() {
            return Self {
                message: message.clone(),
            };
        }
        Self {
            message: String::from("<non-string panic payload>"),
        }
    }

    /// The line this report contributes to the crash channel.
    ///
    /// The `ffi/panic` code follows the `domain/action/reason` shape the project uses
    /// for every cross-boundary condition, so a crash is greppable next to the other
    /// FFI diagnostics.
    fn crash_line(&self) -> String {
        format!("rspinyin: ffi/panic: {}", self.message)
    }
}

/// Runs `body` and turns a panic into a [`PanicReport`].
///
/// The FFI entry points use `guard_ffi`, which maps the same case onto their
/// documented fallback value instead of surfacing the report.
pub(crate) fn catch_ffi<R>(body: impl FnOnce() -> R) -> Result<R, PanicReport> {
    catch_unwind(AssertUnwindSafe(body)).map_err(|payload| PanicReport::from_payload(&*payload))
}

/// Runs `body` under the FFI panic guard, returning `fallback` if it panics.
///
/// Every `extern "C"` body must run inside this: a panic that unwinds into C++ is
/// undefined behaviour, so the boundary turns it into an ordinary return value and
/// records it on the crash channel.
pub(crate) fn guard_ffi<R>(fallback: R, body: impl FnOnce() -> R) -> R {
    match catch_ffi(body) {
        Ok(value) => value,
        Err(report) => {
            write_stderr_line(&report.crash_line());
            fallback
        }
    }
}

/// Reports a `domain/action/reason` condition on the crash channel.
pub(crate) fn emit_diagnostic(code: &str) {
    write_stderr_line(&format!("rspinyin: {}", code));
}

/// Writes one line to stderr.
///
/// Deliberately not `tracing`: a crash channel must not depend on another subsystem
/// being healthy, and this one is reached before the diagnostics layer is
/// initialised or while a panic is being handled. Fcitx5 redirects the process's
/// stderr into its own log, which is where an operator looks after a failed load.
fn write_stderr_line(line: &str) {
    // A failure to write is not actionable: there is no second channel to report it
    // on, and every caller is already on an error path.
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catch_ffi_passes_a_successful_body_through() {
        assert!(matches!(catch_ffi(|| 7), Ok(7)));
    }

    #[test]
    fn test_emit_diagnostic_does_not_panic_on_an_empty_code() {
        // The crash channel is reached from panic paths, so it must never be the
        // thing that fails; an empty code is the degenerate input.
        let outcome = catch_unwind(|| emit_diagnostic(""));
        assert!(outcome.is_ok(), "the crash channel must not panic");
    }
}
