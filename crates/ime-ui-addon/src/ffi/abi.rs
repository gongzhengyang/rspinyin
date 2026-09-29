//! The user-interface addon's C ABI.
//!
//! The contract between the C++ glue in `src/ffi/cpp/` and this crate. It is a
//! *separate* table from the engine's (`ime-fcitx5::ffi::abi`): the two libraries are
//! `dlopen`'d independently as two Fcitx5 addons, so each owns its own handshake, its
//! own callback table and its own context. Nothing here is shared with the engine, and
//! nothing here changes when the engine's table changes.
//!
//! Struct field order is part of the ABI: fields may only be appended, and appending
//! one requires bumping [`RSPINYIN_ABI_VERSION`] and mirroring the change in the C++
//! glue, because a C struct has no other compatibility mechanism.
//!
//! # Who calls whom
//!
//! * The glue provides `rspinyin_register_ui_vtable`, which this side calls to hand the
//!   callback table over, and `rspinyin_ui_addon_factory`, which
//!   [`fcitx_addon_factory_instance`] forwards under the name Fcitx5 resolves after
//!   `dlopen`.
//! * [`rspinyin_ui_plugin_init`] is called by the glue while constructing the addon
//!   instance: it runs the ABI handshake and returns the opaque context the host passes
//!   back to every callback.
//! * This side calls back into the glue through [`activate_ui`] and [`current_ui`],
//!   which ask `UserInterfaceManager` which user interface is active and for a
//!   re-evaluation.
//!
//! # Why this is not the engine's table
//!
//! The engine's `RspinyinVtable` carried two slots for this role —
//! `on_input_panel_update` and `on_cursor_rect` — because both roles lived in one
//! library. The split moved them here and took them out of the engine's table, which
//! is why [`RSPINYIN_ABI_VERSION`] is 2 rather than 1. See
//! `docs/dev/adr/0004-ui-addon-crate-split.md`.
//!
//! # Panic safety
//!
//! Unwinding into C++ is undefined behaviour, so every `extern "C"` body runs inside
//! the panic guard and returns its documented fallback value instead of propagating a
//! panic. Panics are reported through the crash channel.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::str;

use ime_types::{ImeError, check_abi};

use super::{emit_diagnostic, guard_ffi};

/// ABI version of the Rust / C++ vtable contract.
///
/// Re-exported from the frozen contract crate so the workspace keeps a single
/// definition; the assertion below is what keeps this module honest if that
/// definition ever drifts from the version the C++ glue is compiled with.
pub use ime_types::RSPINYIN_ABI_VERSION;

// Compile-time guard on the frozen value. The C++ glue carries the same constant, so a
// change here without a matching change there is an ABI break, not a refactor.
//
// Version 2 is the split: the two user-interface slots left the engine's table and
// became this table's first two entries. A glue compiled against version 1 would find
// neither, so the handshake refuses it rather than running against a layout it does not
// know.
const _: () = assert!(RSPINYIN_ABI_VERSION == 2);

/// Caret rectangle of the focused client, in client coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FcitxCursorRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
    /// Client scale factor (1.0 for an unscaled client).
    pub scale: f64,
}

/// Input-panel snapshot, owned by the host for the duration of one callback.
///
/// Both buffers are UTF-8 and are only valid inside the `on_input_panel_update` call:
/// the host frees them when the callback returns, so anything this side keeps must be
/// copied out. `candidates_ptr` holds `candidate_count` texts separated by `'\n'`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UiPanelSnapshot {
    /// Preedit text; null when `preedit_len` is 0.
    pub preedit_ptr: *const u8,
    /// Length of `preedit_ptr` in bytes.
    pub preedit_len: usize,
    /// Caret position inside the preedit in bytes, 0 when the host reports none.
    pub caret: u32,
    /// `'\n'`-separated candidate texts; null when `candidates_len` is 0.
    pub candidates_ptr: *const u8,
    /// Length of `candidates_ptr` in bytes.
    pub candidates_len: usize,
    /// Number of entries in `candidates_ptr`, 0 for an empty list.
    pub candidate_count: u32,
    /// Highlighted candidate, or -1 when nothing is highlighted.
    pub cursor_index: i32,
    /// Current page index as the pageable candidate list reports it, 0 when the list
    /// is not pageable.
    pub page: u8,
    /// Number of pages, 0 when the list is not pageable.
    pub total_pages: u8,
    /// Number of candidates on the current page.
    pub page_size: u8,
}

/// What the host did with a request to become the active user interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiActivation {
    /// The host routes input-panel updates to this plugin now.
    Active,
    /// The host has no user interface of this plugin to switch to: either its addon
    /// instance does not derive from `fcitx::UserInterface`, or the addon is not
    /// registered under a user-interface category. Fcitx5 has no call that registers
    /// one at run time, so this is a packaging matter rather than a runtime failure.
    NotRegistered,
    /// The host kept another user interface active. That is the user's choice, not an
    /// error, and the plugin must not fight it.
    OtherUiActive,
    /// No host ABI is reachable: the pure-Rust build, or a host that reported a code
    /// this version does not know.
    Unavailable,
}

/// Maps the activation code `src/ffi/cpp/ui_glue.cpp` returns.
///
/// An unknown code maps to [`UiActivation::Unavailable`], so a host newer than this
/// build cannot make the plugin believe it took over when it did not.
pub(super) fn activation_from_code(code: u32) -> UiActivation {
    match code {
        0 => UiActivation::Active,
        1 => UiActivation::NotRegistered,
        2 => UiActivation::OtherUiActive,
        _ => UiActivation::Unavailable,
    }
}

/// Asks the host to re-evaluate which user interface is active.
///
/// The request is a re-evaluation rather than a forced switch: Fcitx5 picks the
/// available user interface with the highest priority, so the answer is whatever the
/// host decided. The name of the user interface that was active before is read with
/// [`current_ui`], before this call, by the caller.
pub fn activate_ui() -> UiActivation {
    // SAFETY: the glue reads no memory this side owns, blocks on nothing, and answers
    // with a plain code.
    activation_from_code(unsafe { rspinyin_ui_activate() })
}

/// The name of the user interface the host routes input-panel updates to.
///
/// Returns `None` when the host cannot answer, which is what the pure-Rust build does.
pub fn current_ui() -> Option<String> {
    // SAFETY: the glue returns either null or a pointer to a buffer it keeps alive
    // until the next call into the glue; this side copies it out before returning.
    let name = unsafe { rspinyin_ui_current() };
    if name.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per the glue's contract. A name that is not
    // UTF-8 is a broken host, and is reported as "no answer" rather than as a name.
    unsafe { CStr::from_ptr(name) }
        .to_str()
        .ok()
        .map(String::from)
}

/// The callback set the C++ glue caches for the process lifetime.
///
/// The first parameter of every callback is the opaque context returned by
/// [`rspinyin_ui_plugin_init`]; the host stores it and hands it back unchanged, and
/// never dereferences it. The `u64` parameter is the input-context id.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinUiVtable {
    /// Must equal [`RSPINYIN_ABI_VERSION`]; the host refuses registration otherwise.
    pub abi_version: u32,

    /// Addon construction; returns whether the addon initialized.
    pub on_addon_init: extern "C" fn(*mut c_void) -> bool,
    /// Addon destruction; runs before the instance is freed.
    pub on_addon_destroy: extern "C" fn(*mut c_void),

    /// The host updated the input panel; returns whether the frame must be drawn.
    pub on_input_panel_update: extern "C" fn(*mut c_void, u64, *const UiPanelSnapshot) -> bool,
    /// The client caret rectangle moved.
    pub on_cursor_rect: extern "C" fn(*mut c_void, u64, FcitxCursorRect),

    /// The host suspended this user interface; another one is active.
    pub on_host_suspend: extern "C" fn(*mut c_void),
    /// The host resumed this user interface; input-panel updates are routed here.
    pub on_host_resume: extern "C" fn(*mut c_void),

    /// The host's `UserInterface::available()` query.
    pub is_available: extern "C" fn() -> bool,
}

/// The one vtable this addon registers with its own glue.
///
/// A `static` rather than a `const`: the glue caches the address, so it has to stay
/// put for the process lifetime.
pub static RSPINYIN_UI_VTABLE: RspinyinUiVtable = RspinyinUiVtable {
    abi_version: RSPINYIN_ABI_VERSION,
    on_addon_init,
    on_addon_destroy,
    on_input_panel_update,
    on_cursor_rect,
    on_host_suspend,
    on_host_resume,
    is_available,
};

/// The opaque context handed to the glue and passed back to every callback.
///
/// A pointer to this marker rather than to any state: the callbacks reach their state
/// through `crate::ui_impl`, exactly as the engine's do, so the context is an identity
/// token Fcitx5 can hold and nothing more. It is `static` so the address is stable for
/// the process lifetime.
static UI_CONTEXT: u8 = 0;

/// Runs the ABI handshake and returns the opaque context.
///
/// Called by the glue while it constructs the addon instance, before any callback can
/// fire. A version mismatch leaves the table unregistered, so the glue's factory never
/// calls [`on_addon_init`] and the addon stays inert instead of running against a
/// layout it does not know.
///
/// Returns null when the handshake fails, which is what the glue checks for.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_ui_plugin_init() -> *mut c_void {
    guard_ffi(ptr::null_mut(), || {
        if let Err(err) = check_abi(RSPINYIN_ABI_VERSION) {
            emit_diagnostic(&abi_mismatch_code(&err));
            return ptr::null_mut();
        }
        #[cfg(fcitx5_host)]
        {
            // SAFETY: the table is a `static` that stays alive and unchanged for the
            // process lifetime, and every entry is a non-null `extern "C"` function.
            unsafe { rspinyin_register_ui_vtable(&RSPINYIN_UI_VTABLE) };
        }
        // `&UI_CONTEXT as *const u8 as *mut c_void` would discard the `const`; the
        // pointer is never written through, but it is handed to C as `void *`, so the
        // cast has to be explicit rather than implied.
        &UI_CONTEXT as *const u8 as *mut c_void
    })
}

/// The `platform/fcitx5/version-mismatch` diagnostic for a refused handshake.
fn abi_mismatch_code(err: &ImeError) -> String {
    format!("platform/fcitx5/version-mismatch: {err}")
}

/// The addon factory Fcitx5 resolves with `dlsym` after `dlopen`.
///
/// Defined in the C++ glue under a crate-private name and re-exported here: rustc
/// writes a version script for the cdylib that ends in `local: *`, listing only the
/// symbols Rust itself marks for export, so a factory defined in C++ would be linked
/// into the image and still invisible to `dlsym`.
#[unsafe(no_mangle)]
pub extern "C" fn fcitx_addon_factory_instance() -> *mut c_void {
    #[cfg(fcitx5_host)]
    {
        // SAFETY: the glue returns the address of a function-local static factory that
        // lives for the process lifetime. Fcitx5 owns the addon instances the factory
        // creates, not the factory itself.
        unsafe { rspinyin_ui_addon_factory() }
    }
    #[cfg(not(fcitx5_host))]
    {
        // No glue is linked, so there is no factory to forward to. Returning null is
        // what a host would see for a library that contains no addon.
        ptr::null_mut()
    }
}

/// Answers the host's `UserInterface::available()` query.
///
/// The host asks this every time it re-evaluates which user interface is active, so the
/// body reads a few atomics and records nothing: it must not allocate, block, or write
/// a log line per query. A `false` answer is what makes the host keep drawing the
/// candidates itself.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_ui_available() -> bool {
    // Wrapped in a closure: an `extern "C" fn` item does not implement `FnOnce()`, so
    // the guard cannot take it directly the way it takes `crate::addon`'s functions.
    guard_ffi(false, || is_available())
}
/// Reports that the host suspended this user interface.
///
/// Called from `UserInterface::suspend()` when another user interface became active,
/// including while the plugin is being unloaded.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_ui_suspend() {
    guard_ffi((), || on_host_suspend(ptr::null_mut()));
}

/// Reports that the host resumed this user interface.
///
/// Called from `UserInterface::resume()` when the input panel was routed here again.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_ui_resume() {
    guard_ffi((), || on_host_resume(ptr::null_mut()));
}

/// Addon construction: runs the user-interface start-up sequence.
///
/// Returns whether the addon initialised. `false` leaves the addon registered but not
/// available, so Fcitx5 keeps ClassicUI — the documented fallback rather than a broken
/// session.
extern "C" fn on_addon_init(_context: *mut c_void) -> bool {
    guard_ffi(false, crate::addon::on_addon_init)
}

/// Addon destruction: stops the UI thread and releases what the start-up took.
extern "C" fn on_addon_destroy(_context: *mut c_void) {
    guard_ffi((), crate::addon::on_addon_destroy);
}

/// The host updated the input panel.
///
/// Returns whether the frame must be drawn: `false` means this frame needs no drawing
/// and the UI keeps the previous one. The snapshot is validated and read into safe
/// values here, at the boundary; nothing on this path decodes, formats or resolves
/// geometry, because the host thread's budget for the callback is 100us.
extern "C" fn on_input_panel_update(
    _context: *mut c_void,
    ic_id: u64,
    snapshot: *const UiPanelSnapshot,
) -> bool {
    guard_ffi(false, || {
        if snapshot.is_null() {
            emit_diagnostic("ffi/null-panel-snapshot");
            return false;
        }
        // SAFETY: non-null and owned by the caller for the duration of this call;
        // `UiPanelSnapshot` is `repr(C)` and every bit pattern is a valid value.
        let panel = unsafe { *snapshot };
        // SAFETY: the host guarantees both buffers are valid for their lengths for the
        // duration of this call, and they are never mutated while borrowed here. An
        // empty panel is legitimate, so the empty-tolerant reader is used.
        let preedit = unsafe { bytes_from_raw_allow_empty(panel.preedit_ptr, panel.preedit_len) };
        // SAFETY: as above, for the candidate buffer.
        let candidates =
            unsafe { bytes_from_raw_allow_empty(panel.candidates_ptr, panel.candidates_len) };
        let (Some(preedit), Some(candidates)) = (preedit, candidates) else {
            emit_diagnostic("ffi/invalid-panel-snapshot");
            return false;
        };
        // The host sends UTF-8. A buffer that is not is a broken host rather than text,
        // and is refused instead of being converted lossily into something plausible.
        let (Ok(preedit), Ok(candidates)) = (str::from_utf8(preedit), str::from_utf8(candidates))
        else {
            emit_diagnostic("ffi/invalid-panel-snapshot");
            return false;
        };
        crate::ui_impl::on_input_panel_update(crate::ui_impl::PanelUpdate {
            ic: ic_id,
            preedit,
            caret: panel.caret,
            candidates,
            candidate_count: panel.candidate_count,
            cursor_index: panel.cursor_index,
            page: panel.page,
            total_pages: panel.total_pages,
            page_size: panel.page_size,
        })
    })
}

/// The client caret rectangle moved.
///
/// The rectangle is stored exactly as the host reported it: turning it into a screen
/// anchor is the caret ladder's job and runs where an anchor is needed, never inside a
/// host callback.
extern "C" fn on_cursor_rect(_context: *mut c_void, ic_id: u64, rect: FcitxCursorRect) {
    guard_ffi((), || {
        crate::ui_impl::on_cursor_rect(ic_id, rect);
    });
}

/// `UserInterface::suspend()`.
extern "C" fn on_host_suspend(_context: *mut c_void) {
    guard_ffi((), crate::ui_impl::on_host_suspend);
}

/// `UserInterface::resume()`.
extern "C" fn on_host_resume(_context: *mut c_void) {
    guard_ffi((), crate::ui_impl::on_host_resume);
}

/// `UserInterface::available()`.
extern "C" fn is_available() -> bool {
    crate::ui_impl::is_available()
}

/// Reads a host-owned byte buffer that may legitimately be empty.
///
/// A null pointer with a zero length is an empty buffer rather than an invalid one:
/// that is the shape the host sends for a panel with no preedit and for a hidden
/// candidate list. A non-zero length with a null pointer stays invalid, because the
/// host cannot have meant anything by it.
///
/// # Safety
///
/// When `len` is non-zero, `ptr` must point to `len` initialised bytes that stay valid
/// and unaliased for reads for the whole of `'a`.
unsafe fn bytes_from_raw_allow_empty<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller upholds the validity contract documented above.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Validates and caches the callback table for the lifetime of the process.
    ///
    /// Provided by `src/ffi/cpp/ui_addon_glue.cpp`.
    ///
    /// # Safety
    ///
    /// `vt` must point to a table that stays alive and unchanged for the process
    /// lifetime, with every function pointer non-null.
    fn rspinyin_register_ui_vtable(vt: *const RspinyinUiVtable);

    /// The addon factory, defined in `src/ffi/cpp/ui_addon_glue.cpp`.
    ///
    /// # Safety
    ///
    /// The returned pointer is the address of a function-local static that lives for
    /// the process lifetime; the caller must not free it.
    fn rspinyin_ui_addon_factory() -> *mut c_void;

    /// Asks the user-interface manager to re-evaluate which user interface is active
    /// and answers with one of the activation codes [`activation_from_code`] maps.
    ///
    /// Provided by `src/ffi/cpp/ui_glue.cpp`.
    ///
    /// # Safety
    ///
    /// Called from the host thread; the callee blocks on nothing and owns nothing the
    /// caller has to release.
    fn rspinyin_ui_activate() -> u32;

    /// The active user interface's name, or null when the host cannot name one.
    ///
    /// Provided by `src/ffi/cpp/ui_glue.cpp`. The returned pointer stays valid until
    /// the next call into the glue, and must not be freed.
    ///
    /// # Safety
    ///
    /// The caller must treat the returned pointer as host-owned and read it before the
    /// next call into the glue.
    fn rspinyin_ui_current() -> *const c_char;
}

/// Stand-in for the pure-Rust build, where no C++ glue is linked.
///
/// It answers "no host" rather than pretending to have asked one, so the activation
/// policy reports [`UiActivation::Unavailable`] instead of a refusal the host never
/// gave.
///
/// Declared `unsafe` to mirror the `extern "C"` declaration it stands in for, so the
/// call site needs no configuration-dependent `unsafe` block.
///
/// # Safety
///
/// Nothing is read or written; the caller has no obligations beyond the ones the real
/// function carries.
#[cfg(not(fcitx5_host))]
unsafe fn rspinyin_ui_activate() -> u32 {
    ACTIVATION_UNAVAILABLE
}

/// The activation code meaning "there is no manager to ask", matching
/// `kUiActivationUnavailable` in `src/ffi/cpp/ui_glue.cpp`.
#[cfg(not(fcitx5_host))]
const ACTIVATION_UNAVAILABLE: u32 = 3;

/// Stand-in for the pure-Rust build, where no C++ glue is linked.
///
/// Declared `unsafe` to mirror the `extern "C"` declaration it stands in for, so the
/// call site needs no configuration-dependent `unsafe` block.
///
/// # Safety
///
/// The returned pointer is always null and must not be dereferenced.
#[cfg(not(fcitx5_host))]
unsafe fn rspinyin_ui_current() -> *const c_char {
    ptr::null()
}

#[cfg(test)]
mod tests;
