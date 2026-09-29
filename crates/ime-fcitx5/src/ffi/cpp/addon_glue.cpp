// Fcitx5 addon glue: the loadable-library entry points.
//
// This translation unit owns what only C++ can provide:
//
//   * `fcitx_addon_factory_instance`, expanded by FCITX_ADDON_FACTORY. Rust cannot
//     generate it, because the macro needs the complete `fcitx::AddonFactory` and
//     `fcitx::AddonManager` types.
//   * `rspinyin_register_vtable`, the handshake the Rust side calls to hand over its
//     callback table. The Rust half of the contract is `src/ffi/abi.rs`.
//   * the input-context identity the engine and UI glue use in their callbacks.
//
// The struct definitions below mirror the `#[repr(C)]` declarations in
// `src/ffi/abi.rs`. There is deliberately no shared header: the glue is compiled as
// part of this crate, and spelling the contract out at both definition sites is what
// makes an accidental drift visible. Field order is the ABI — append only, and bump
// the version constant on both sides when you do.

#include <cstddef>
#include <cstdint>
#include <mutex>

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

struct FcitxKeyEvent {
    std::uint32_t sym;
    std::uint32_t state;
    bool is_release;
    std::uint32_t time_ms;
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

struct RspinyinVtable {
    std::uint32_t abi_version;

    bool (*on_addon_init)(void *);
    void (*on_addon_destroy)(void *);

    bool (*on_activate)(void *, std::uint64_t);
    void (*on_deactivate)(void *, std::uint64_t);
    void (*on_reset)(void *, std::uint64_t);
    bool (*on_key_event)(void *, std::uint64_t, const FcitxKeyEvent *);

    bool (*on_input_panel_update)(void *, std::uint64_t, const UiPanelSnapshot *);
    void (*on_cursor_rect)(void *, std::uint64_t, FcitxCursorRect);
    void (*on_focus_in)(void *, std::uint64_t);
    void (*on_focus_out)(void *, std::uint64_t);

    void (*on_commit_string)(void *, std::uint64_t, const char *, std::size_t);
    void (*on_set_preedit)(void *, std::uint64_t, const char *, std::size_t, std::uint32_t);
    void (*on_clear_preedit)(void *, std::uint64_t);
};

// The single symbol the Rust side exports. It runs the ABI handshake and returns the
// opaque context the host passes back to every callback; a null return means the plugin
// must not be used and the addon stays in pure-engine mode.
extern "C" void *rspinyin_plugin_init();

namespace {

/// ABI version this glue was compiled against; must equal the Rust constant.
constexpr std::uint32_t kAbiVersion = 1;

/// Handshake state, filled in once while the addon instance is constructed.
struct HandshakeState {
    /// Callback table handed over by the Rust side; null until it is registered.
    const RspinyinVtable *vtable = nullptr;
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
bool isComplete(const RspinyinVtable *vt) {
    return vt->on_addon_init != nullptr && vt->on_addon_destroy != nullptr &&
           vt->on_activate != nullptr && vt->on_deactivate != nullptr &&
           vt->on_reset != nullptr && vt->on_key_event != nullptr &&
           vt->on_input_panel_update != nullptr && vt->on_cursor_rect != nullptr &&
           vt->on_focus_in != nullptr && vt->on_focus_out != nullptr &&
           vt->on_commit_string != nullptr && vt->on_set_preedit != nullptr &&
           vt->on_clear_preedit != nullptr;
}

/// Logs an ABI mismatch in the shape the contract prescribes.
void logAbiMismatch(std::uint32_t pluginVersion) {
    FCITX_ERROR() << "rspinyin: ABI mismatch (host=" << kAbiVersion
                  << ", plugin=" << pluginVersion << ")";
}

} // namespace

/// Validates the callback table and caches it for the lifetime of the process.
///
/// Called by the Rust entry point with the table it was compiled against. A table whose
/// `abi_version` differs from the version this glue was compiled with is refused: the
/// addon factory then never calls `on_addon_init`, and the plugin stays in pure-engine
/// mode instead of running against a layout it does not know.
extern "C" void rspinyin_register_vtable(const RspinyinVtable *vt) {
    if (vt == nullptr) {
        FCITX_ERROR() << "rspinyin: registration refused (null vtable)";
        return;
    }
    std::call_once(state().registration, [vt]() {
        if (vt->abi_version != kAbiVersion) {
            logAbiMismatch(vt->abi_version);
            return;
        }
        if (!isComplete(vt)) {
            FCITX_ERROR() << "rspinyin: registration refused (incomplete vtable)";
            return;
        }
        state().vtable = vt;
    });
}

namespace rspinyin {

/// The registered callback table, or null when registration did not happen.
const RspinyinVtable *vtable() {
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

} // namespace rspinyin

namespace {

/// The addon instance Fcitx5 creates through the factory below.
///
/// It owns the handshake: constructing it asks the Rust side for the callback table and
/// starts the engine, destroying it releases the engine. The input-method-engine and
/// user-interface roles live in `engine_glue.cpp` and `ui_glue.cpp`.
class RspinyinAddon : public fcitx::AddonInstance {
public:
    explicit RspinyinAddon(fcitx::AddonManager *) {
        void *context = rspinyin_plugin_init();
        const RspinyinVtable *vt = rspinyin::vtable();
        if (context == nullptr || vt == nullptr) {
            FCITX_WARN() << "rspinyin: plugin entry declined; pure engine mode";
            return;
        }
        state().context = context;
        if (!vt->on_addon_init(context)) {
            FCITX_WARN() << "rspinyin: addon init declined; pure engine mode";
            return;
        }
        FCITX_INFO() << "rspinyin: addon loaded";
    }

    ~RspinyinAddon() override {
        // Released even when the constructor declined. The lifecycle may have completed
        // part of its sequence before the step that failed, and the Rust destroy path is
        // idempotent, so calling it for an addon that never initialised costs nothing —
        // while skipping it would strand whatever the partial sequence had taken.
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_addon_destroy == nullptr) {
            return;
        }
        vt->on_addon_destroy(rspinyin::context());
    }
};

/// Creates the addon instance; Fcitx5 takes ownership of the returned pointer.
class RspinyinAddonFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new RspinyinAddon(manager);
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
extern "C" FCITXCORE_EXPORT ::fcitx::AddonFactory *rspinyin_addon_factory() {
    static RspinyinAddonFactory factory;
    return &factory;
}
