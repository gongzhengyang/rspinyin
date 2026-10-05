//! The plugin's own life: the context the host is handed, the handshake that gates it,
//! the symbols that construct and destroy the addon, and the reload slot the host's
//! configuration reload action fires.
//!
//! Responsibility: own the registration sequence. [`rspinyin_plugin_init`] validates the
//! ABI version, hands the callback table to the glue, and returns the opaque context the
//! host passes back to every callback; [`fcitx_addon_factory_instance`] is the symbol
//! Fcitx5 resolves after `dlopen`; [`on_addon_init`] and [`on_addon_destroy`] are the two
//! vtable slots the addon's own lifetime runs through; [`rspinyin_config_reload`] is the
//! one export the glue calls after load, when the host asks for a configuration reload
//! (ADR-0011's append-only symbol path — no vtable slot, no version bump).
//!
//! Boundaries: this module never decodes, never renders and never touches the user
//! interface. Everything it hands over is either a pointer the host owns or the address of
//! a `static` that outlives the process.

use std::ffi::c_void;
use std::ptr;

use crate::ffi::{emit_diagnostic, guard_ffi};

use super::{RSPINYIN_VTABLE, RspinyinHandshake, RspinyinVtable};

/// Plugin state behind the opaque context pointer handed to the host.
///
/// The host only stores and returns this pointer; every access goes through a
/// callback that received it, which is what keeps the callbacks testable without a
/// process-wide singleton. The handshake uses an atomic, so reaching it through a
/// shared reference is sound even though the host hands the pointer back as `*mut`.
pub(super) struct PluginContext {
    /// Visible to the module's tests, which drive the handshake through a context
    /// pointer the way the host does.
    pub(super) handshake: RspinyinHandshake,
}

impl PluginContext {
    /// Creates a context whose handshake is still rejected.
    pub(super) const fn new() -> Self {
        Self {
            handshake: RspinyinHandshake::new(),
        }
    }
}

/// Process-wide plugin state; one plugin instance per process is the host's model.
static PLUGIN_CONTEXT: PluginContext = PluginContext::new();

/// One of the symbols this crate exports to the C++ glue.
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
        match register(&PLUGIN_CONTEXT, &RSPINYIN_VTABLE) {
            Registration::Accepted(context) => context,
            Registration::Refused(line) => {
                // Rejected: no registration, no `on_addon_init`, pure-engine mode.
                emit_diagnostic(&line);
                ptr::null_mut()
            }
        }
    })
}

/// What one registration attempt decided.
///
/// A value rather than a pair of side effects, so that the refusal is reachable from a
/// test: the production context accepts the production table on the first call and could
/// never take the other branch.
#[derive(Debug)]
pub(super) enum Registration {
    /// The host may use the plugin; this is the context to hand it.
    Accepted(*mut c_void),
    /// The table was refused, and this is the line the refusal is reported with.
    ///
    /// The line carries the stable `platform/fcitx5/version-mismatch` code, which
    /// diagnostics and tests match on, and it is reported by the caller rather than here:
    /// the entry point is the only place that owns the crash channel.
    Refused(String),
}

/// Validates `vt` and, when it passes, registers it with the host.
///
/// The body of [`rspinyin_plugin_init`], over the context and the table it is given. The
/// returned context points at `plugin`, which the caller must therefore keep alive for as
/// long as the host holds it — the production entry point passes the process-wide static,
/// whose address outlives everything.
pub(super) fn register(plugin: &PluginContext, vt: &RspinyinVtable) -> Registration {
    match plugin.handshake.accept(vt) {
        Ok(()) => {
            register_with_host();
            Registration::Accepted(ptr::from_ref(plugin).cast_mut().cast::<c_void>())
        }
        Err(err) => Registration::Refused(err.to_string()),
    }
}

/// Hands the table to the C++ glue, which validates the ABI version again.
#[cfg(fcitx5_host)]
fn register_with_host() {
    // SAFETY: `RSPINYIN_VTABLE` is a `static`, so its address is valid for the whole
    // process lifetime; the callee only reads it and caches the pointer.
    unsafe { super::rspinyin_register_vtable(&RSPINYIN_VTABLE) }
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

/// The input-context id the reload's host boundary is built for.
///
/// `0` is the reserved "no input context" value, and a reload is the one callback that
/// belongs to no context: it re-reads a document and tells every live session at once.
/// The effects a reload produces are window commands, which carry no input-context
/// identity of their own, so the id is never resolved against a context; a per-context
/// call arriving on this path would be a defect above this layer, and a boundary built
/// for id 0 refuses to resolve it rather than sending it to a wrong one.
const RELOAD_CONTEXT: u64 = 0;

/// Re-reads the configuration and hands the result to every live session.
///
/// The production trigger the `config-watch` step records: the glue's `reloadConfig`
/// override forwards the host's reload request here — the glue links this export by
/// name, the way it links [`rspinyin_plugin_init`] — and this entry is the caller
/// [`on_config_reload`] was written for. The sequence is the reload handler's own:
/// re-read the document, report what it found, adopt the routing table it projects;
/// the broadcast through [`session_host::reload`](crate::session_host::reload) is what
/// makes the adoption reach the sessions that are already alive. A composition in
/// progress survives all of it (`AGENTS.md` prohibition 23).
///
/// # Returns
///
/// Whether the sequence ran to completion: the document was re-read — a missing store
/// or an unparsable one is reported on the diagnostic channel, and the values in force,
/// possibly the built-in defaults, are what the broadcast hands on — and a live session
/// host, if one is installed, was told the values now in force. `false` means the entry
/// panicked and the guard answered its fallback; every lesser degradation still answers
/// `true`, because the reload itself happened.
///
/// # Panics
///
/// Never: the body runs under the panic guard, and a panic becomes the `false`
/// fallback.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_config_reload() -> bool {
    guard_ffi(false, || {
        let routing = crate::addon::on_config_reload();
        super::engine::with_host(RELOAD_CONTEXT, |host| {
            crate::session_host::reload(routing, host);
        });
        true
    })
}
