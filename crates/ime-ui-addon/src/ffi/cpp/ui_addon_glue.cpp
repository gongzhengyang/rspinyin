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
#include <deque>
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

/// Looks an engine symbol up through probe `mechanism`.
///
/// Returns null when that mechanism finds nothing. `handle_out`, when non-null,
/// receives the handle the symbol came from (a `RTLD_NOLOAD` result, or null for
/// `RTLD_DEFAULT`); the caller has no reason to close a `RTLD_NOLOAD` handle.
void *find_engine_symbol(int mechanism, const char *name, void **handle_out) {
    switch (mechanism) {
    case 1:
        return dlsym(RTLD_DEFAULT, name);
    case 2: {
        Dl_info info{};
        void *self = reinterpret_cast<void *>(&find_engine_symbol);
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
        return dlsym(handle, name);
    }
    case 3: {
        void *handle = dlopen("librspinyin.so", RTLD_NOLOAD | RTLD_LAZY);
        if (handle == nullptr) {
            return nullptr;
        }
        if (handle_out != nullptr) {
            *handle_out = handle;
        }
        return dlsym(handle, name);
    }
    default:
        return nullptr;
    }
}

} // namespace
// ── Event return channel (ADR-0011) ─────────────────────────────────────────────
//
// The candidate window's events are produced on the drain thread, but the engine's
// session layer (`rspinyin_event_ingest`) runs on the Fcitx5 main loop and only
// there. This outlet is the marshalling point between the two: `post` queues one
// wire from the drain thread, and `flush` hands the queued events to the engine on
// the loop thread. The Rust half calls `flush` from the callback-table entries that
// the host dispatches on the loop (`on_input_panel_update`, `on_cursor_rect`), which
// is the delivery path that needs no facility of the host's own.
//
// What is deliberately not here yet is the *wake*: a queued event reaches the engine
// at the next loop dispatch, not at the moment it was queued, so a click on a window
// the host has nothing new to dispatch for waits for the next dispatch. Waking an
// idle loop needs one defer/post event source on the instance's event loop, whose
// signature must be taken from the installed host headers rather than from memory
// (the standard every other fcitx call in these files was held to). Until that lands,
// the outlet is the proven half: queue, drain, and the probes below. ADR-0011
// records this as the return channel's remaining piece.
//
// The engine's ingest is resolved with the same probe trio the frame handshake uses,
// and dropped again when the transport unregisters, so a pointer into an engine that
// unloaded first is never called.

/// Mirrors the `#[repr(C)]` event wire in `src/ffi/transport.rs`, which transcribes
/// the engine's reader. A plain integer struct: the whole wire crosses the drain
/// thread's boundary as a copy.
struct RspinyinEventWire {
    std::uint32_t kind;
    std::uint32_t revision;
    std::uint16_t index;
    std::uint32_t reason;
    std::int32_t anchor_x;
    std::int32_t anchor_y;
    std::uint32_t anchor_w;
    std::uint32_t anchor_h;
    std::int32_t anchor_screen;
    float anchor_scale;
    std::uint32_t anchor_placement;
};

// The layout is shared with both Rust transcriptions; a field added must move all
// three and this number.
static_assert(sizeof(RspinyinEventWire) == 44,
              "the event wire is the ABI: four words, a slot pair, and the anchor "
              "seven-tuple");

/// The engine's ingest, as the probes above hand it over.
using rspinyin_event_ingest_fn = bool (*)(std::uint64_t, const RspinyinEventWire *);

namespace {

/// One queued event: the context it belongs to, and the wire it travels on.
struct QueuedEvent {
    std::uint64_t ic_id;
    RspinyinEventWire wire;
};

/// How many events may wait for the main loop before a post is refused.
///
/// The rates are human-scale -- a click, a page turn, a throttled hover -- so the
/// ceiling exists to bound the outlet's memory, not to absorb a burst. A full
/// outbox refuses the new event, which the drain counts, because dropping an older
/// one would reorder what the engine is told.
constexpr std::size_t kEventOutboxCapacity = 64;

/// The outlet state: the resolved ingest, and the events waiting for the loop.
///
/// A function-local static rather than a namespace-scope object, like the handshake
/// state above: nothing in this library is initialised before Fcitx5 is ready for
/// it. One mutex guards both, and it is a leaf lock -- nothing else is taken while
/// it is held, and it is never held across the ingest call itself.
struct EventOutlet {
    std::mutex guard;
    rspinyin_event_ingest_fn ingest = nullptr;
    std::deque<QueuedEvent> outbox;
};

EventOutlet &event_outlet() {
    static EventOutlet outlet;
    return outlet;
}

} // namespace

/// Resolves the engine's ingest and opens the outbox (ADR-0011).
///
/// Returns 1 when the ingest was resolved, 0 when no mechanism found a live engine.
/// Arming again clears whatever a previous arm left queued: events encoded against
/// one engine incarnation must not be delivered into the next.
extern "C" std::uint32_t rspinyin_ui_event_outlet_arm() {
    for (int mechanism = 1; mechanism <= 3; ++mechanism) {
        void *handle = nullptr;
        void *symbol = find_engine_symbol(mechanism, "rspinyin_event_ingest", &handle);
        if (symbol == nullptr) {
            continue;
        }
        EventOutlet &outlet = event_outlet();
        {
            const std::lock_guard<std::mutex> guard(outlet.guard);
            outlet.ingest = reinterpret_cast<rspinyin_event_ingest_fn>(symbol);
            outlet.outbox.clear();
        }
        return 1;
    }
    return 0;
}

/// Drops the engine's ingest and empties the outbox.
///
/// Called when the transport unregisters: an ingest pointer into an engine that
/// unloaded first must never be called, and events queued behind a gone engine are
/// dropped rather than delivered into a teardown.
extern "C" void rspinyin_ui_event_outlet_disarm() {
    EventOutlet &outlet = event_outlet();
    const std::lock_guard<std::mutex> guard(outlet.guard);
    outlet.ingest = nullptr;
    outlet.outbox.clear();
}

/// Queues one event wire for the engine's main loop, from the drain thread.
///
/// Returns 1 when the wire was queued and 0 when it was not -- the outlet is
/// disarmed, or the outbox is at its ceiling. The wire is copied before this
/// returns: the drain thread's borrow ends with the call, and nothing queued
/// outlives the copy.
extern "C" std::uint32_t rspinyin_ui_event_outlet_post(std::uint64_t ic_id,
                                                       const RspinyinEventWire *wire) {
    if (wire == nullptr) {
        return 0;
    }
    EventOutlet &outlet = event_outlet();
    const std::lock_guard<std::mutex> guard(outlet.guard);
    if (outlet.ingest == nullptr || outlet.outbox.size() >= kEventOutboxCapacity) {
        return 0;
    }
    outlet.outbox.push_back(QueuedEvent{ic_id, *wire});
    return 1;
}

/// Hands the outbox's queued events to the engine, on the calling thread.
///
/// Must be called from the Fcitx5 main loop, which is the only thread the engine's
/// session layer runs on. The lock is released around each ingest call, so the
/// drain thread's posts are never delayed by the engine's own work; the order the
/// engine is told is still the order the events arrived in.
extern "C" std::uint32_t rspinyin_ui_event_outlet_flush() {
    std::uint32_t flushed = 0;
    for (;;) {
        rspinyin_event_ingest_fn ingest = nullptr;
        QueuedEvent item{};
        {
            EventOutlet &outlet = event_outlet();
            const std::lock_guard<std::mutex> guard(outlet.guard);
            if (outlet.ingest == nullptr || outlet.outbox.empty()) {
                return flushed;
            }
            item = outlet.outbox.front();
            outlet.outbox.pop_front();
            ingest = outlet.ingest;
        }
        if (ingest(item.ic_id, &item.wire)) {
            ++flushed;
        }
    }
}
