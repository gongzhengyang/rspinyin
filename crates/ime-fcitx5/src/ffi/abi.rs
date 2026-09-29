//! Frozen C ABI between the C++ glue in `src/ffi/cpp/` and the Rust engine.
//!
//! Everything here is a transcription of the contract frozen in
//! `docs/dev/features.md` §2.2.3. Struct field order is part of the ABI: fields may
//! only be appended, and appending one requires bumping [`RSPINYIN_ABI_VERSION`] and
//! mirroring the change in the C++ glue, because a C struct has no other
//! compatibility mechanism.
//!
//! # Who calls whom
//!
//! * The glue exports the addon factory symbol Fcitx5 resolves after `dlopen`
//!   (`fcitx_addon_factory_instance`, expanded by `FCITX_ADDON_FACTORY`) and
//!   `rspinyin_register_vtable`, which this side calls to hand the callback table
//!   over.
//! * This module exports exactly one symbol, [`rspinyin_plugin_init`], which the
//!   glue calls while constructing the addon instance. It runs the ABI handshake and
//!   returns the opaque context the host passes back to every callback.
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

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use ime_types::{ImeError, check_abi};

use crate::ffi::{emit_diagnostic, guard_ffi};

/// ABI version of the Rust / C++ vtable contract.
///
/// Re-exported from the frozen contract crate so the workspace keeps a single
/// definition; the assertion below is what keeps this module honest if that
/// definition ever drifts from the version the C++ glue is compiled with.
pub use ime_types::RSPINYIN_ABI_VERSION;

// Compile-time guard on the frozen value. The C++ glue carries the same constant,
// so a change here without a matching change there is an ABI break, not a refactor.
const _: () = assert!(RSPINYIN_ABI_VERSION == 1);

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

/// A key press or release as Fcitx5 delivers it to the input method engine.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FcitxKeyEvent {
    /// XKB keysym, passed through unchanged from `fcitx::Key::sym()`.
    pub sym: u32,
    /// Modifier bit mask, passed through unchanged from
    /// `fcitx::Key::states().toInteger()`.
    pub state: u32,
    /// `true` for a key release.
    pub is_release: bool,
    /// Frontend timestamp in milliseconds, 0 when the frontend reports none.
    pub time_ms: u32,
}

/// Input-panel snapshot, owned by the host for the duration of one callback.
///
/// Both buffers are UTF-8 and are only valid inside the `on_input_panel_update`
/// call: the host frees them when the callback returns, so anything the engine keeps
/// must be copied out. `candidates_ptr` holds `candidate_count` texts separated by
/// `'\n'`.
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

    /// The host updated the input panel; returns whether the frame must be drawn.
    pub on_input_panel_update: extern "C" fn(*mut c_void, u64, *const UiPanelSnapshot) -> bool,
    /// The client caret rectangle moved.
    pub on_cursor_rect: extern "C" fn(*mut c_void, u64, FcitxCursorRect),
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

/// Registration state of the vtable handshake.
///
/// Both sides validate the version they receive: the C++ glue refuses to cache a
/// table whose `abi_version` differs from the version it was compiled with, and this
/// side refuses to hand the table over in the first place. Either rejection leaves
/// the addon in pure-engine mode instead of letting it run against a layout it does
/// not know.
#[derive(Debug)]
pub struct RspinyinHandshake {
    accepted: AtomicBool,
}

impl RspinyinHandshake {
    /// Creates a handshake that has not accepted a table yet.
    pub const fn new() -> Self {
        Self {
            accepted: AtomicBool::new(false),
        }
    }

    /// Validates `vt` and, when it passes, marks the handshake accepted.
    ///
    /// # Errors
    ///
    /// Returns `ImeError::Fcitx5VersionMismatch` when the table's `abi_version`
    /// differs from [`RSPINYIN_ABI_VERSION`]. The handshake stays rejected, so the
    /// callbacks that depend on it keep reporting the safe default.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::ffi::{RSPINYIN_VTABLE, RspinyinHandshake};
    ///
    /// let handshake = RspinyinHandshake::new();
    /// assert!(handshake.accept(&RSPINYIN_VTABLE).is_ok());
    /// assert!(handshake.is_accepted());
    /// ```
    pub fn accept(&self, vt: &RspinyinVtable) -> Result<(), ImeError> {
        check_abi(vt.abi_version)?;
        self.accepted.store(true, Ordering::Release);
        Ok(())
    }

    /// Whether a table with a matching ABI version has been accepted.
    pub fn is_accepted(&self) -> bool {
        self.accepted.load(Ordering::Acquire)
    }
}

impl Default for RspinyinHandshake {
    fn default() -> Self {
        Self::new()
    }
}

/// Plugin state behind the opaque context pointer handed to the host.
///
/// The host only stores and returns this pointer; every access goes through a
/// callback that received it, which is what keeps the callbacks testable without a
/// process-wide singleton. The handshake uses an atomic, so reaching it through a
/// shared reference is sound even though the host hands the pointer back as `*mut`.
struct PluginContext {
    handshake: RspinyinHandshake,
}

impl PluginContext {
    /// Creates a context whose handshake is still rejected.
    const fn new() -> Self {
        Self {
            handshake: RspinyinHandshake::new(),
        }
    }
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
    on_input_panel_update,
    on_cursor_rect,
    on_focus_in,
    on_focus_out,
    on_commit_string,
    on_set_preedit,
    on_clear_preedit,
};

/// Process-wide plugin state; one plugin instance per process is the host's model.
static PLUGIN_CONTEXT: PluginContext = PluginContext::new();

/// The single symbol this crate exports to the C++ glue.
///
/// The glue calls it once while constructing the addon instance, before anything
/// touches the vtable. It validates the ABI version, hands the table to the host
/// through `rspinyin_register_vtable`, and returns the opaque context the host must
/// pass back to every callback.
///
/// # Returns
///
/// A non-null context when the plugin may be used, or null when it must not be: the
/// host ABI is not linked into this build, the ABI version does not match, or the
/// entry itself panicked. The glue treats null as pure-engine mode and logs why.
///
/// # Safety
///
/// Called by the host from the thread that loads the addon. The returned pointer
/// must be passed back unchanged and must never be dereferenced by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_plugin_init() -> *mut c_void {
    guard_ffi(ptr::null_mut(), || {
        if let Err(err) = PLUGIN_CONTEXT.handshake.accept(&RSPINYIN_VTABLE) {
            // Rejected: no registration, no `on_addon_init`, pure-engine mode.
            emit_diagnostic(&err.to_string());
            return ptr::null_mut();
        }
        register_with_host();
        ptr::from_ref(&PLUGIN_CONTEXT).cast_mut().cast::<c_void>()
    })
}

/// Hands the table to the C++ glue, which validates the ABI version again.
#[cfg(fcitx5_host)]
fn register_with_host() {
    // SAFETY: `RSPINYIN_VTABLE` is a `static`, so its address is valid for the whole
    // process lifetime; the callee only reads it and caches the pointer.
    unsafe { rspinyin_register_vtable(&RSPINYIN_VTABLE) }
}

/// The addon factory Fcitx5 resolves with `dlsym` after `dlopen`.
///
/// # Why this is exported from Rust rather than defined in C++
///
/// `FCITX_ADDON_FACTORY` would normally define this symbol in `addon_glue.cpp`. That
/// does not work for a rustc-built cdylib: rustc emits a version script ending in
/// `local: *` that lists only the symbols rustc itself marks for export, and that
/// script is authoritative. `--export-dynamic`, `--export-dynamic-symbol` and
/// `--dynamic-list` were each tried and none of them can re-expose a name the script
/// has made local. A factory defined in C++ is therefore linked into the image and
/// still invisible to the host, which Fcitx5 reports as a missing addon.
///
/// So the glue expands the macro by hand under the private name
/// `rspinyin_addon_factory`, and this function re-exports it under the canonical name.
/// The factory object is still constructed in C++, where the complete
/// `fcitx::AddonFactory` and `fcitx::AddonManager` types are available; Rust only
/// forwards the pointer.
///
/// # Safety
///
/// Called by the host from the thread that loads the addon. The returned pointer is
/// owned by a C++ function-local static and must not be freed by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn fcitx_addon_factory_instance() -> *mut c_void {
    guard_ffi(ptr::null_mut(), || {
        // SAFETY: defined in `src/ffi/cpp/addon_glue.cpp` and linked into this cdylib
        // whenever the host ABI is present. It returns the address of a function-local
        // static, which stays valid for the process lifetime.
        unsafe { rspinyin_addon_factory() }
    })
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Constructs the addon factory. Defined in `src/ffi/cpp/addon_glue.cpp`.
    fn rspinyin_addon_factory() -> *mut c_void;
}

/// Stand-in for the pure-Rust build, where no C++ glue is linked.
///
/// Returning null rather than omitting the symbol keeps `fcitx_addon_factory_instance`
/// present in every build, so a library built without the host ABI fails the way the
/// host expects (no addon) instead of failing to resolve the symbol at all.
///
/// Declared `unsafe` to mirror the `extern "C"` declaration it stands in for, so the
/// call site needs no configuration-dependent `unsafe` block.
///
/// # Safety
///
/// The caller must treat the returned pointer as host-owned and must not free it. It is
/// always null in this build.
#[cfg(not(fcitx5_host))]
unsafe fn rspinyin_addon_factory() -> *mut c_void {
    emit_diagnostic("ffi/host-not-linked: addon factory requested");
    ptr::null_mut()
}

/// Reports that this build has no host to register with.
#[cfg(not(fcitx5_host))]
fn register_with_host() {
    // Without the `fcitx5-host` feature the glue is not compiled at all, so there is
    // no factory symbol, no Fcitx5 to load this library, and nothing to register.
    emit_diagnostic("ffi/host-not-linked: vtable registration skipped");
}

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

/// Reads a host-owned byte buffer that may legitimately be empty.
///
/// Unlike [`bytes_from_raw`], a null pointer with a zero length is an empty buffer
/// rather than an invalid one: that is the shape the host sends for a panel with no
/// preedit and for a hidden candidate list. A non-null length with a null pointer stays
/// invalid, because the host cannot have meant anything by it.
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

/// Reads the context pointer the host hands back.
///
/// # Safety
///
/// `context` must be null or the pointer returned by [`rspinyin_plugin_init`] of this
/// process, which stays alive for the process lifetime.
unsafe fn context_ref<'a>(context: *mut c_void) -> Option<&'a PluginContext> {
    if context.is_null() {
        return None;
    }
    // SAFETY: the caller upholds the provenance contract documented above.
    Some(unsafe { &*context.cast::<PluginContext>() })
}

/// Addon construction.
///
/// Returns whether the addon initialized. `false` puts the plugin in pure-engine
/// mode: it may decode and commit, but it does not register a `UserInterface` and
/// Fcitx5 keeps drawing the candidate window itself.
///
/// The handshake is checked here, at the boundary, because it is this table's own
/// precondition; the lifecycle sequence that follows is
/// [`crate::addon::on_addon_init`], and a rejected handshake never reaches it.
pub extern "C" fn on_addon_init(context: *mut c_void) -> bool {
    guard_ffi(false, || {
        // SAFETY: the host passes back the pointer from `rspinyin_plugin_init`.
        match unsafe { context_ref(context) } {
            Some(plugin) if plugin.handshake.is_accepted() => crate::addon::on_addon_init(context),
            _ => false,
        }
    })
}

/// Addon destruction, before the instance is freed.
///
/// The host calls this once per addon instance whose construction succeeded; the
/// lifecycle sequence itself is [`crate::addon::on_addon_destroy`], which is safe to
/// run when initialisation declined part-way.
pub extern "C" fn on_addon_destroy(context: *mut c_void) {
    guard_ffi((), || {
        crate::addon::on_addon_destroy(context);
    });
}

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

/// The host updated the input panel.
///
/// Returns whether the frame must be drawn: `false` means this frame needs no
/// drawing and the UI keeps the previous one.
pub extern "C" fn on_input_panel_update(
    _context: *mut c_void,
    _ic_id: u64,
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
        match (preedit, candidates) {
            (Some(_preedit), Some(_candidates)) => {
                // Stub: the frame is built and posted to the UI thread by the
                // candidate-panel work. `false` reports "no drawing needed", which is
                // also the documented answer for a hidden panel.
            }
            _ => emit_diagnostic("ffi/invalid-panel-snapshot"),
        }
        false
    })
}

/// The client caret rectangle moved.
pub extern "C" fn on_cursor_rect(_context: *mut c_void, _ic_id: u64, _rect: FcitxCursorRect) {
    guard_ffi((), || {
        // Stub: positioning the candidate window at the caret belongs to the
        // candidate-panel work.
    });
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

#[cfg(test)]
mod tests {
    use std::panic::panic_any;

    use super::*;

    use crate::ffi::{PanicReport, catch_ffi};

    /// Builds the production table with a different ABI version.
    fn vtable_with_abi_version(abi_version: u32) -> RspinyinVtable {
        RspinyinVtable {
            abi_version,
            ..RSPINYIN_VTABLE
        }
    }

    /// Points at `context` the way the host does.
    fn context_pointer(context: &PluginContext) -> *mut c_void {
        ptr::from_ref(context).cast_mut().cast::<c_void>()
    }

    #[test]
    fn test_vtable_declares_the_frozen_abi_version() {
        assert_eq!(RSPINYIN_VTABLE.abi_version, RSPINYIN_ABI_VERSION);
    }

    #[test]
    fn test_handshake_accepts_vtable_with_matching_abi_version() {
        let handshake = RspinyinHandshake::new();
        assert!(!handshake.is_accepted());
        assert!(handshake.accept(&RSPINYIN_VTABLE).is_ok());
        assert!(handshake.is_accepted());
    }

    #[test]
    fn test_handshake_rejects_vtable_with_mismatched_abi_version() {
        let handshake = RspinyinHandshake::new();
        let newer = vtable_with_abi_version(RSPINYIN_ABI_VERSION + 1);
        let result = handshake.accept(&newer);
        assert!(result.is_err(), "a mismatched ABI version must be rejected");
        assert!(!handshake.is_accepted());
        if let Err(err) = result {
            assert!(matches!(err, ImeError::Fcitx5VersionMismatch { .. }));
            assert!(
                err.to_string()
                    .starts_with("platform/fcitx5/version-mismatch")
            );
        }
    }

    #[test]
    fn test_on_addon_init_returns_false_after_mismatched_abi_version() {
        let context = PluginContext::new();
        let older = vtable_with_abi_version(RSPINYIN_ABI_VERSION.wrapping_sub(1));
        assert!(context.handshake.accept(&older).is_err());
        assert!(!on_addon_init(context_pointer(&context)));
    }

    #[test]
    fn test_on_addon_init_returns_true_after_accepted_handshake() {
        let context = PluginContext::new();
        assert!(context.handshake.accept(&RSPINYIN_VTABLE).is_ok());
        assert!(on_addon_init(context_pointer(&context)));
    }

    #[test]
    fn test_on_addon_init_returns_false_for_null_context() {
        assert!(!on_addon_init(ptr::null_mut()));
    }

    #[test]
    fn test_guard_ffi_returns_the_body_value_when_it_succeeds() {
        assert!(guard_ffi(false, || true));
        assert_eq!(guard_ffi(0, || 7), 7);
    }

    #[test]
    fn test_guard_ffi_returns_fallback_when_the_body_panics() {
        let mut body_ran = false;
        let result = guard_ffi(false, || {
            body_ran = true;
            panic_any("engine blew up");
        });
        assert!(body_ran, "the guarded body must have run");
        assert!(!result, "a panicking body must yield the fallback value");
    }

    #[test]
    fn test_guard_ffi_reports_string_panic_payloads() {
        // Both string shapes a panic payload can take must reach the crash log.
        let static_str: Result<(), PanicReport> = catch_ffi(|| panic_any("boom"));
        assert!(static_str.is_err());
        if let Err(report) = static_str {
            assert_eq!(report.message, "boom");
        }

        let owned: Result<(), PanicReport> = catch_ffi(|| panic_any(String::from("bang")));
        assert!(owned.is_err());
        if let Err(report) = owned {
            assert_eq!(report.message, "bang");
        }
    }

    #[test]
    fn test_guard_ffi_reports_non_string_panic_payload() {
        let report: Result<(), PanicReport> = catch_ffi(|| panic_any(42u32));
        assert!(report.is_err());
        if let Err(report) = report {
            assert_eq!(report.message, "<non-string panic payload>");
            assert!(report.crash_line().starts_with("rspinyin: ffi/panic:"));
        }
    }

    #[test]
    fn test_bytes_from_raw_rejects_null_pointer() {
        // SAFETY: a null pointer with a zero length is the invalid shape under test;
        // no bytes are read.
        let bytes = unsafe { bytes_from_raw(ptr::null(), 0) };
        assert!(bytes.is_none());
    }

    #[test]
    fn test_bytes_from_raw_rejects_zero_length() {
        let host_buffer = *b"ab";
        // SAFETY: the pointer is valid for the two bytes that exist; the length is
        // what makes the shape invalid, so nothing is read.
        let bytes = unsafe { bytes_from_raw(host_buffer.as_ptr(), 0) };
        assert!(bytes.is_none());
    }

    #[test]
    fn test_bytes_from_raw_borrows_host_bytes() {
        let host_buffer = *b"abc";
        // SAFETY: the pointer and length describe exactly the live array above, and
        // the borrow does not outlive it.
        let bytes = unsafe { bytes_from_raw(host_buffer.as_ptr(), host_buffer.len()) };
        assert_eq!(bytes, Some(&host_buffer[..]));
    }

    #[test]
    fn test_bytes_from_raw_allow_empty_accepts_an_empty_buffer() {
        // SAFETY: the empty shape is the case under test and reads no bytes.
        let bytes = unsafe { bytes_from_raw_allow_empty(ptr::null(), 0) };
        assert!(bytes.is_some());
        if let Some(bytes) = bytes {
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn test_bytes_from_raw_allow_empty_rejects_null_pointer_with_length() {
        // SAFETY: the invalid shape is the case under test; the null pointer is never
        // dereferenced because the length check rejects it first.
        let bytes = unsafe { bytes_from_raw_allow_empty(ptr::null(), 4) };
        assert!(bytes.is_none());
    }

    #[test]
    fn test_bytes_from_raw_allow_empty_borrows_host_bytes() {
        let host_buffer = *b"xy";
        // SAFETY: the pointer and length describe exactly the live array above.
        let bytes = unsafe { bytes_from_raw_allow_empty(host_buffer.as_ptr(), host_buffer.len()) };
        assert_eq!(bytes, Some(&host_buffer[..]));
    }

    #[test]
    fn test_on_key_event_returns_false_so_the_key_is_not_swallowed() {
        let event = FcitxKeyEvent {
            sym: 0x61,
            state: 0,
            is_release: false,
            time_ms: 5,
        };
        assert!(!on_key_event(ptr::null_mut(), 7, ptr::from_ref(&event)));
    }

    #[test]
    fn test_on_key_event_returns_false_for_null_event() {
        assert!(!on_key_event(ptr::null_mut(), 7, ptr::null()));
    }

    #[test]
    fn test_on_input_panel_update_returns_false_for_null_snapshot() {
        assert!(!on_input_panel_update(ptr::null_mut(), 7, ptr::null()));
    }

    #[test]
    fn test_on_input_panel_update_reads_a_valid_snapshot() {
        let preedit = b"ni hao";
        let candidates = b"ni\nhao";
        let snapshot = UiPanelSnapshot {
            preedit_ptr: preedit.as_ptr(),
            preedit_len: preedit.len(),
            caret: 6,
            candidates_ptr: candidates.as_ptr(),
            candidates_len: candidates.len(),
            candidate_count: 2,
            cursor_index: 0,
            page: 0,
            total_pages: 1,
            page_size: 2,
        };
        // The stub answers "no drawing needed", but the snapshot has to be read without
        // diagnosing an invalid buffer.
        assert!(!on_input_panel_update(
            ptr::null_mut(),
            7,
            ptr::from_ref(&snapshot)
        ));
    }

    #[test]
    fn test_on_commit_string_ignores_invalid_buffer_without_panicking() {
        let commit = || on_commit_string(ptr::null_mut(), 7, ptr::null(), 0);
        let outcome = std::panic::catch_unwind(commit);
        assert!(
            outcome.is_ok(),
            "an invalid commit must be diagnosed, not panicked"
        );
    }

    #[test]
    fn test_on_set_preedit_ignores_invalid_buffer_without_panicking() {
        let set_preedit = || on_set_preedit(ptr::null_mut(), 7, ptr::null(), 0, 0);
        let outcome = std::panic::catch_unwind(set_preedit);
        assert!(
            outcome.is_ok(),
            "an invalid preedit must be diagnosed, not panicked"
        );
    }

    #[cfg(fcitx5_host)]
    #[test]
    fn test_plugin_init_registers_the_vtable_with_the_host_glue() {
        let context = rspinyin_plugin_init();
        assert!(
            !context.is_null(),
            "the host glue must accept a table with the frozen ABI version"
        );
        assert!(on_addon_init(context));
    }
}
