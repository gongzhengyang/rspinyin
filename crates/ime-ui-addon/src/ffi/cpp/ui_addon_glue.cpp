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
