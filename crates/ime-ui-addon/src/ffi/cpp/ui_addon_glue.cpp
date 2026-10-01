// Fcitx5 user-interface addon glue: entry points, handshake and the addon factory.
//
// This translation unit is the user-interface addon's counterpart to the engine's
// `addon_glue.cpp`, and it is deliberately a separate copy rather than a shared
// library. Fcitx5 `dlopen`s the two addons independently, so a link-time relationship
// between them is exactly what the split exists to remove.
//
// What it owns:
//
//   * `rspinyin_ui_addon_factory`, expanded from FCITX_ADDON_FACTORY by hand. Rust
//     cannot generate it, because the macro needs the complete `fcitx::AddonFactory`
//     and `fcitx::AddonManager` types.
//   * `rspinyin_register_ui_vtable`, the handshake the Rust side calls to hand over its
//     callback table. The Rust half of the contract is `src/ffi/abi.rs`.
//   * the input-context identity the UI glue uses in its callbacks.
//   * the addon lifecycle: the sequence `startPlugin` runs at load and `stopPlugin`
//     runs at unload.
//
// The addon *instance* is not defined here — it has to derive from
// `fcitx::UserInterface`, and only `ui_glue.cpp` can define a class derived from that
// type; this file keeps the sequence and the entry points.
//
// The struct definitions below mirror the `#[repr(C)]` declarations in
// `src/ffi/abi.rs`. There is deliberately no shared header: the glue is compiled as
// C++17 and the Rust side is the authority on the layout, so a header would be a third
// copy to keep in step rather than a single source of truth.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <cstdint>
#include <mutex>
#include <string>

#include <fcitx-utils/log.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>

// ── Mirrored C ABI contract ──────────────────────────────────────────────────────
struct FcitxCursorRect {
    std::int32_t x;
    std::int32_t y;
    std::int32_t w;
    std::int32_t h;
    double scale;
};

struct UiPanelSnapshot {
    const std::uint8_t *preedit_ptr;
    std::size_t preedit_len;
    std::uint32_t caret;
    const std::uint8_t *candidates_ptr;
    std::size_t candidates_len;
    std::uint32_t candidate_count;
    std::int32_t cursor_index;
    std::uint8_t page;
    std::uint8_t total_pages;
    std::uint8_t page_size;
};

/// The user-interface addon's callback table.
///
/// Smaller than the engine's: it carries only what a candidate window needs. The first
/// two entries are the slots that used to live in the engine's table before the split
/// (ABI version 2).
struct RspinyinUiVtable {
    std::uint32_t abi_version;

    bool (*on_addon_init)(void *);
    void (*on_addon_destroy)(void *);

    bool (*on_input_panel_update)(void *, std::uint64_t, const UiPanelSnapshot *);
    void (*on_cursor_rect)(void *, std::uint64_t, FcitxCursorRect);

    void (*on_host_suspend)(void *);
    void (*on_host_resume)(void *);

    bool (*is_available)();
};

// The symbol the Rust side exports. It runs the ABI handshake and returns the opaque
// context the host passes back to every callback; a null return means the addon must
// not be used and the candidates stay with ClassicUI.
extern "C" void *rspinyin_ui_plugin_init();

namespace {

/// ABI version this glue was compiled against; must equal the Rust constant.
constexpr std::uint32_t kAbiVersion = 2;

/// Handshake state, filled in once while the addon instance is constructed.
struct HandshakeState {
    /// Callback table handed over by the Rust side; null until it is registered.
    const RspinyinUiVtable *vtable = nullptr;
    /// Opaque Rust context, passed back to every callback unchanged.
    void *context = nullptr;
    /// Serialises registration so a second attempt cannot replace the table.
    std::once_flag registration;
};

/// The process-wide handshake state.
///
/// A function-local static rather than a namespace-scope object, so that nothing in
/// this library is initialised before Fcitx5 is ready for it.
HandshakeState &state() {
    static HandshakeState instance;
    return instance;
}

/// Whether every callback in `vt` is set.
///
/// The Rust side cannot build a table with a null entry, so this only catches a foreign
/// or hand-written table — which is exactly the case worth refusing.
bool isComplete(const RspinyinUiVtable *vt) {
    return vt->on_addon_init != nullptr && vt->on_addon_destroy != nullptr &&
           vt->on_input_panel_update != nullptr && vt->on_cursor_rect != nullptr &&
           vt->on_host_suspend != nullptr && vt->on_host_resume != nullptr &&
           vt->is_available != nullptr;
}

/// Logs an ABI mismatch in the shape the contract prescribes.
void logAbiMismatch(std::uint32_t pluginVersion) {
    FCITX_ERROR() << "rspinyin-ui: ABI mismatch (host=" << kAbiVersion
                  << ", plugin=" << pluginVersion << ")";
}

} // namespace

/// Validates the callback table and caches it for the lifetime of the process.
///
/// Called by the Rust entry point with the table it was compiled against. A table whose
/// `abi_version` differs from the version this glue was compiled with is refused: the
/// addon factory then never calls `on_addon_init`, and the addon stays inert instead of
/// running against a layout it does not know.
extern "C" void rspinyin_register_ui_vtable(const RspinyinUiVtable *vt) {
    if (vt == nullptr) {
        FCITX_ERROR() << "rspinyin-ui: registration refused (null vtable)";
        return;
    }
    std::call_once(state().registration, [vt]() {
        if (vt->abi_version != kAbiVersion) {
            logAbiMismatch(vt->abi_version);
            return;
        }
        if (!isComplete(vt)) {
            FCITX_ERROR() << "rspinyin-ui: registration refused (incomplete vtable)";
            return;
        }
        state().vtable = vt;
    });
}

namespace rspinyin {

/// The registered callback table, or null when registration did not happen.
const RspinyinUiVtable *vtable() {
    return state().vtable;
}

/// The opaque Rust context, or null when registration did not happen.
void *context() {
    return state().context;
}

/// Stable 64-bit identity of an input context.
///
/// Fcitx5 exposes a 16-byte `ICUUID` rather than a number, and the ABI needs a number.
/// A digest of the uuid is stable for the lifetime of the context and needs no registry,
/// no allocation and no lock on the host thread. Collisions would merge two sessions,
/// which is why the digest is 64-bit FNV-1a over all 16 bytes; if a session registry
/// ever replaces it, this function is the only place that changes.
std::uint64_t ic_id(const fcitx::InputContext *inputContext) {
    if (inputContext == nullptr) {
        return 0;
    }
    std::uint64_t hash = 14695981039346656037ULL; // FNV-1a 64-bit offset basis
    for (const std::uint8_t byte : inputContext->uuid()) {
        hash ^= byte;
        hash *= 1099511628211ULL; // FNV-1a 64-bit prime
    }
    // 0 is reserved for "no input context", so a digest that lands on it is nudged.
    return hash == 0 ? 1 : hash;
}

/// Creates the addon instance; Fcitx5 takes ownership of the returned pointer.
///
/// Defined in `ui_glue.cpp`, because the instance derives from `fcitx::UserInterface`
/// and a class may only be derived from a complete base type. See that file's header for
/// why the addon has to be that class.
fcitx::AddonInstance *createUiInstance(fcitx::AddonManager *manager);

/// Runs the plugin handshake and the addon initialisation sequence.
///
/// Called by the addon instance in `ui_glue.cpp` while it is constructed. The handshake
/// comes first: it asks the Rust side for the callback table and hands that table to
/// this side, and every callback the glue makes is reached through it.
void startPlugin() {
    void *context = rspinyin_ui_plugin_init();
    const RspinyinUiVtable *vt = vtable();
    if (context == nullptr || vt == nullptr) {
        FCITX_WARN() << "rspinyin-ui: plugin entry declined; candidates stay with ClassicUI";
        return;
    }
    state().context = context;
    if (!vt->on_addon_init(context)) {
        FCITX_WARN() << "rspinyin-ui: addon init declined; candidates stay with ClassicUI";
        return;
    }
    FCITX_INFO() << "rspinyin-ui: addon loaded";
}

/// Releases everything `startPlugin` took.
///
/// Called by the addon instance before it is freed, and unconditionally. The lifecycle
/// may have completed part of its sequence before the step that failed, and the Rust
/// destroy path is idempotent, so calling it for an addon that never initialised costs
/// nothing — while skipping it would strand whatever the partial sequence had taken.
void stopPlugin() {
    const RspinyinUiVtable *vt = vtable();
    if (vt == nullptr || vt->on_addon_destroy == nullptr) {
        return;
    }
    vt->on_addon_destroy(context());
}

} // namespace rspinyin

namespace {

/// Creates the addon instance through the UI glue, which owns the class.
class RspinyinUiAddonFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return rspinyin::createUiInstance(manager);
    }
};

} // namespace

// The factory Fcitx5 resolves with dlsym after dlopen. Its canonical name
// (`fcitx_addon_factory_instance`) is exported from the Rust side instead — see
// `fcitx_addon_factory_instance` in src/ffi/abi.rs for why.
//
// FCITX_ADDON_FACTORY cannot be used verbatim here. rustc writes a version script for
// the cdylib that ends in `local: *`, listing only the symbols Rust itself marks for
// export; that script is authoritative, and neither --export-dynamic,
// --export-dynamic-symbol nor --dynamic-list can re-expose a name it has made local.
// A factory defined in this translation unit would therefore be linked into the image
// and still invisible to dlsym, which Fcitx5 reports as a missing addon.
//
// So the macro is expanded by hand under a crate-private name, and Rust re-exports it
// under the canonical one. The factory object is still constructed here, where the
// complete fcitx::AddonFactory and fcitx::AddonManager types are available.
extern "C" void *rspinyin_ui_addon_factory() {
    static RspinyinUiAddonFactory factory;
    return &factory;
}

// ── Cross-addon transport handshake (ADR-0011) ──────────────────────────────────
//
// The engine addon holds an atomic sink slot whose `post()` consults per command;
// registering means handing it the pointer to the Rust side's `rspinyin_ui_frame_sink()`
// static. The engine exports the registration symbol; this glue finds it and calls it
// once, during the addon's `ui-registration` step.
//
// Two probe mechanisms, in order, and a third fallback:
//   1. `dlsym(RTLD_DEFAULT, ...)`: answers when fcitx5 loaded the engine addon
//      `RTLD_GLOBAL`.
//   2. `dladdr` on this translation unit's own code gives the path this addon was
//      loaded from; the engine sits beside it under `librspinyin.so`, and a `RTLD_NOLOAD`
//      open of that exact path matches the loaded object by name.
//   3. the bare soname `librspinyin.so` under `RTLD_NOLOAD`, for a host that loads by
//      soname rather than by path.
// A `RTLD_NOLOAD` open never loads anything: it either answers an already-loaded object
// or fails, which is why the fallbacks cost nothing and risk nothing.
//
// The registration function's type is spelled as a plain `int (*)(const void *)`: the
// sink this side passes is an opaque pointer the engine never dereferences, and
// duplicating the struct layout here would be a third transcription of a shape this
// file does not otherwise need.

#include <dlfcn.h>

typedef int (*rspinyin_register_sinks_fn)(const void *);

// The sink pointer the Rust side of this addon exports (ADR-0011). Declared at file
// scope: a linkage specification is not valid inside a function body, and the symbol
// resolves within this image at link time.
extern "C" const void *rspinyin_ui_frame_sink();

namespace {

/// Looks the engine's registration function up through probe `mechanism`.
///
/// Returns null when that mechanism finds nothing. `handle_out`, when non-null,
/// receives the handle the symbol came from (a `RTLD_NOLOAD` result, or null for
/// `RTLD_DEFAULT`); the caller has no reason to close a `RTLD_NOLOAD` handle.
rspinyin_register_sinks_fn find_engine_register(int mechanism, void **handle_out) {
    switch (mechanism) {
    case 1:
        return reinterpret_cast<rspinyin_register_sinks_fn>(
            dlsym(RTLD_DEFAULT, "rspinyin_engine_register_ui_sinks"));
    case 2: {
        Dl_info info{};
        void *self = reinterpret_cast<void *>(&find_engine_register);
        if (dladdr(self, &info) == 0 || info.dli_fname == nullptr) {
            return nullptr;
        }
        std::string path(info.dli_fname);
        auto slash = path.find_last_of('/');
        std::string sibling = (slash == std::string::npos)
                                  ? std::string("librspinyin.so")
                                  : path.substr(0, slash + 1) + "librspinyin.so";
        void *handle = dlopen(sibling.c_str(), RTLD_NOLOAD | RTLD_LAZY);
        if (handle == nullptr) {
            return nullptr;
        }
        if (handle_out != nullptr) {
            *handle_out = handle;
        }
        return reinterpret_cast<rspinyin_register_sinks_fn>(
            dlsym(handle, "rspinyin_engine_register_ui_sinks"));
    }
    case 3: {
        void *handle = dlopen("librspinyin.so", RTLD_NOLOAD | RTLD_LAZY);
        if (handle == nullptr) {
            return nullptr;
        }
        if (handle_out != nullptr) {
            *handle_out = handle;
        }
        return reinterpret_cast<rspinyin_register_sinks_fn>(
            dlsym(handle, "rspinyin_engine_register_ui_sinks"));
    }
    default:
        return nullptr;
    }
}

} // namespace

/// Registers the frame sink with the engine addon (ADR-0011).
///
/// Returns the probe mechanism that fired: 1 `RTLD_DEFAULT`, 2 the sibling-path
/// `RTLD_NOLOAD`, 3 the bare-soname `RTLD_NOLOAD`, 0 when no mechanism found a live
/// engine. The Rust caller records the answer; a 0 is not retried until the next addon
/// load.
extern "C" std::uint32_t rspinyin_ui_transport_register() {
    const void *sink = rspinyin_ui_frame_sink();
    if (sink == nullptr) {
        return 0;
    }
    for (int mechanism = 1; mechanism <= 3; ++mechanism) {
        void *handle = nullptr;
        rspinyin_register_sinks_fn register_sinks = find_engine_register(mechanism, &handle);
        if (register_sinks == nullptr) {
            continue;
        }
        if (register_sinks(sink) == 1) {
            return static_cast<std::uint32_t>(mechanism);
        }
    }
    return 0;
}

/// Clears the engine's sink slot on unload, by the same probes (ADR-0011).
///
/// Nothing is cached between the register and unregister calls on purpose: an engine
/// that unloaded first must not leave this side holding a dangling function pointer,
/// and a fresh probe costs one `dlsym` on a path that runs once per unload.
extern "C" void rspinyin_ui_transport_unregister() {
    for (int mechanism = 1; mechanism <= 3; ++mechanism) {
        void *handle = nullptr;
        rspinyin_register_sinks_fn register_sinks = find_engine_register(mechanism, &handle);
        if (register_sinks == nullptr) {
            continue;
        }
        void *clear = dlsym(handle != nullptr ? handle : RTLD_DEFAULT,
                            "rspinyin_engine_clear_ui_sinks");
        if (clear != nullptr) {
            reinterpret_cast<void (*)()>(clear)();
            return;
        }
    }
}
