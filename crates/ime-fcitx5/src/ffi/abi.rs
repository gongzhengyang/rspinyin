//! Frozen C ABI between the C++ glue in `src/ffi/cpp/` and the Rust engine.
//!
//! Everything here is a transcription of the contract frozen in
//! `docs/dev/features.md` §2.2.3. Struct field order is part of the ABI: fields may
//! only be appended, and appending one requires bumping [`RSPINYIN_ABI_VERSION`] and
//! mirroring the change in the C++ glue, because a C struct has no other
//! compatibility mechanism.
//!
//! # This is the engine addon's table only
//!
//! The user-interface role has its own table in `crates/ime-ui-addon`. The two
//! libraries are `dlopen`'d independently as two Fcitx5 addons, so each owns its own
//! handshake, its own callback table and its own context, and neither changes when the
//! other does. Version 2 of [`RSPINYIN_ABI_VERSION`] is the split: the two slots that
//! used to carry the user-interface role here — `on_input_panel_update` and
//! `on_cursor_rect` — are the first two entries of that table now.
//!
//! # Who calls whom
//!
//! * The glue provides `rspinyin_register_vtable`, which this side calls to hand the
//!   callback table over, and `rspinyin_addon_factory`, which
//!   [`fcitx_addon_factory_instance`] forwards under the name Fcitx5 resolves after
//!   `dlopen`.
//! * This module exports the entry points the host resolves by name.
//!   [`rspinyin_plugin_init`] is called by the glue while constructing the addon
//!   instance: it runs the ABI handshake and returns the opaque context the host passes
//!   back to every callback. [`fcitx_addon_factory_instance`] is the addon factory.
//!
//! # Layout
//!
//! The callback table itself — [`RspinyinVtable`] and the one [`RSPINYIN_VTABLE`]
//! instance the host caches — stays in this root, together with the reader that
//! turns a host-owned buffer into a slice. The rest is split by the role it serves and
//! re-exported below, so that every path into this module is unchanged by the split:
//!
//! * `types` — the frozen ABI version, the registration handshake and the `#[repr(C)]`
//!   payload mirrors.
//! * `lifecycle` — the plugin context, the handshake's registration with the glue, the
//!   addon factory, and the addon construction and destruction callbacks.
//! * `engine` — the input-method-engine callbacks, which are the host-to-engine
//!   direction.
//!
//! # Panic safety
//!
//! Unwinding into C++ is undefined behaviour, so every `extern "C"` body runs inside
//! the panic guard and returns its documented fallback value instead of propagating a
//! panic. Panics are reported through the crash channel.
//!
//! # Skeleton markers
//!
//! Callbacks whose logic belongs to a later card return their documented safe
//! default and carry a `Stub:` comment naming the work that replaces the body.
//! Nothing else in this module is a placeholder.

// The module itself is public so the transport's post-path seam (`transport::dispatch`)
// is reachable from the benchmark target; every callback that matters was already
// re-exported at this level, so nothing newly public carries behaviour that was not
// reachable before.
pub mod engine;
mod lifecycle;
mod types;

use std::ffi::{c_char, c_void};

pub use self::engine::{
    on_activate, on_clear_preedit, on_commit_string, on_deactivate, on_focus_in, on_focus_out,
    on_key_event, on_reset, on_set_preedit,
};
pub use self::lifecycle::{
    fcitx_addon_factory_instance, on_addon_destroy, on_addon_init, rspinyin_plugin_init,
};
pub use self::types::{FcitxKeyEvent, RSPINYIN_ABI_VERSION, RspinyinHandshake};

/// Callback set implemented on this side; the C++ glue takes the table once while
/// constructing the addon instance and caches the pointer.
///
/// The first parameter of every callback is the opaque context returned by
/// [`rspinyin_plugin_init`]; the host stores it and hands it back unchanged, and
/// never dereferences it. The `u64` parameter is the input-context id.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinVtable {
    /// Must equal [`RSPINYIN_ABI_VERSION`]; the host refuses registration otherwise.
    pub abi_version: u32,

    /// Addon construction; returns whether the addon initialized.
    pub on_addon_init: extern "C" fn(*mut c_void) -> bool,
    /// Addon destruction; runs before the instance is freed.
    pub on_addon_destroy: extern "C" fn(*mut c_void),

    /// An input context switched to this input method.
    pub on_activate: extern "C" fn(*mut c_void, u64) -> bool,
    /// An input context switched away from this input method.
    pub on_deactivate: extern "C" fn(*mut c_void, u64),
    /// An input context was reset (focus change, mode switch).
    pub on_reset: extern "C" fn(*mut c_void, u64),
    /// A key press or release; returns whether the engine consumed the key.
    pub on_key_event: extern "C" fn(*mut c_void, u64, *const FcitxKeyEvent) -> bool,

    /// An input context gained focus.
    pub on_focus_in: extern "C" fn(*mut c_void, u64),
    /// An input context lost focus.
    pub on_focus_out: extern "C" fn(*mut c_void, u64),

    /// Text to insert into the client.
    pub on_commit_string: extern "C" fn(*mut c_void, u64, *const c_char, usize),
    /// Preedit text plus its caret position in bytes.
    pub on_set_preedit: extern "C" fn(*mut c_void, u64, *const c_char, usize, u32),
    /// Drop the preedit.
    pub on_clear_preedit: extern "C" fn(*mut c_void, u64),
}

/// The one vtable this plugin registers with the host.
///
/// A `static` rather than a `const`: the host caches the address, so it has to stay
/// put for the process lifetime.
pub static RSPINYIN_VTABLE: RspinyinVtable = RspinyinVtable {
    abi_version: RSPINYIN_ABI_VERSION,
    on_addon_init,
    on_addon_destroy,
    on_activate,
    on_deactivate,
    on_reset,
    on_key_event,
    on_focus_in,
    on_focus_out,
    on_commit_string,
    on_set_preedit,
    on_clear_preedit,
};

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Validates and caches the callback table for the lifetime of the process.
    ///
    /// Provided by `src/ffi/cpp/addon_glue.cpp`.
    ///
    /// # Safety
    ///
    /// `vt` must point to a table that stays alive and unchanged for the process
    /// lifetime, with every function pointer non-null.
    pub fn rspinyin_register_vtable(vt: *const RspinyinVtable);
}

/// Reads a host-owned byte buffer as a slice.
///
/// Returns `None` for the two shapes the ABI forbids: a null pointer, and a zero
/// length (the host uses those to clear a buffer rather than to send text).
///
/// # Safety
///
/// When `ptr` is non-null it must point to `len` initialised bytes that stay valid
/// and unaliased for reads for the whole of `'a`.
pub(crate) unsafe fn bytes_from_raw<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() || len == 0 {
        return None;
    }
    // SAFETY: the caller upholds the validity contract documented above.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[cfg(test)]
mod tests;
