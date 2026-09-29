// Fcitx5 input-method-engine glue.
//
// `RspinyinEngine` derives from `fcitx::InputMethodEngineV2` — the engine class Fcitx5
// 5.1 expects an input-method addon to implement — and forwards each virtual into the
// Rust callback table. The signatures below were taken from the installed 5.1.7
// headers, not from memory:
//
//   fcitx::InputMethodEngine::keyEvent(const InputMethodEntry &, KeyEvent &)   [pure]
//   fcitx::InputMethodEngine::activate/deactivate/reset(..., InputContextEvent &)
//   fcitx::InputMethodEngine::listInputMethods()
//
// # Why the addon instance lives in this translation unit
//
// Fcitx5 never asks an addon for its input-method engine. `InputMethodManager` collects
// the addons whose description declares `Category=InputMethod` and treats the
// `AddonInstance` of each one as an `InputMethodEngine` (5.1.7
// `inputmethodmanager.cpp`, the dynamic-entry loader). An instance that does not derive
// from that class is dispatched through a vtable slot the object does not have, which
// kills the process with SIGSEGV while the host enumerates input methods.
//
// The object the factory returns therefore has to *be* the engine, and `RspinyinAddon`
// below is that object. It is defined here rather than in `addon_glue.cpp` because a
// class may only be derived from a complete base type, and this is the translation unit
// that defines the engine. `addon_glue.cpp` keeps the loadable-library entry points, the
// handshake state and the lifecycle sequence; this file reaches that sequence through
// the free functions declared below.
//
// The struct definitions mirror the `#[repr(C)]` declarations in `src/ffi/abi.rs` and
// are identical to the copies in the other glue files; see `addon_glue.cpp` for why
// there is no shared header.

#include <cstdint>
#include <type_traits>
#include <vector>

#include <fcitx-utils/key.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputmethodentry.h>

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

// Defined in `addon_glue.cpp`.
namespace rspinyin {

/// The registered callback table, or null when registration did not happen.
const RspinyinVtable *vtable();

/// The opaque Rust context handed back to every callback.
void *context();

/// Stable 64-bit identity of an input context.
std::uint64_t ic_id(const fcitx::InputContext *inputContext);

/// Runs the plugin handshake and the addon initialisation sequence.
void startPlugin();

/// Releases everything `startPlugin` took.
void stopPlugin();

} // namespace rspinyin

namespace {

/// The input-method-engine role of the plugin.
///
/// Every override below reads at most a few fields and forwards into the Rust callback
/// table: this class runs on the Fcitx5 main loop, so nothing here may block, allocate
/// without bound or hold a lock.
class RspinyinEngine : public fcitx::InputMethodEngineV2 {
public:
    /// Takes the manager Fcitx5 handed to the factory.
    ///
    /// `AddonInstance` exposes no way back to the host — it has `reloadConfig`, `save`
    /// and the configuration accessors and nothing else — so the handle an addon needs
    /// for anything beyond the events it is handed has to be captured while it is
    /// created. This engine is not the only role that needs it: the user-interface role
    /// reaches `UserInterfaceManager` the same way.
    explicit RspinyinEngine(fcitx::AddonManager *manager) : manager_(manager) {}

    /// The input methods this engine provides.
    ///
    /// Returning an entry here is what makes the input method selectable in
    /// fcitx5-configtool; without it the plugin loads but is never offered, and the
    /// acceptance criterion "visible in the input method list and addable" cannot be met.
    ///
    /// The unique name is what a user's profile stores, so it is a stable identifier and
    /// must not change across releases. `addon` must match the addon name Fcitx5 loaded
    /// this library under (see packaging/fcitx5/rspinyin.conf).
    ///
    /// Offering it before the decoder exists is safe: a key the engine cannot act on is
    /// not consumed — `keyEvent` only calls `filterAndAccept` when Rust reports the key
    /// handled — so selecting this input method degrades to pass-through rather than
    /// swallowing the user's typing.
    std::vector<fcitx::InputMethodEntry> listInputMethods() override {
        std::vector<fcitx::InputMethodEntry> entries;
        entries.emplace_back("rspinyin", "Rust Pinyin", "zh_CN", "rspinyin");
        entries.back()
            .setNativeName("拼音")
            .setIcon("fcitx-rspinyin")
            .setLabel("拼")
            .setConfigurable(false);
        return entries;
    }

    /// A key press or release for this input method.
    ///
    /// The engine consumes a key only when the Rust side reports that it acted on it.
    /// Everything else — every key no subsystem can act on yet, every key release, and
    /// every key arriving before the callback table is registered — keeps travelling
    /// down the Fcitx5 pipeline, which is what "not consumed" means for this callback.
    /// Answering anything else would take the user's typing away from the application.
    void keyEvent(const fcitx::InputMethodEntry &, fcitx::KeyEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_key_event == nullptr) {
            return;
        }
        FcitxKeyEvent key{};
        key.sym = static_cast<std::uint32_t>(event.key().sym());
        key.state = static_cast<std::uint32_t>(event.key().states().toInteger());
        key.is_release = event.isRelease();
        key.time_ms = event.time() > 0 ? static_cast<std::uint32_t>(event.time()) : 0U;
        if (vt->on_key_event(rspinyin::context(), rspinyin::ic_id(event.inputContext()),
                             &key)) {
            // The engine consumed the key: stop it here so nothing else handles it.
            event.filterAndAccept();
        }
        // Otherwise the key keeps travelling down the Fcitx5 pipeline, which is what
        // "not consumed" means for this callback.
    }

    /// An input context switched to this input method.
    void activate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_activate == nullptr) {
            return;
        }
        // The virtual returns void, so a `false` answer from the engine can only mean
        // "no session is active"; there is nothing for this layer to decline.
        vt->on_activate(rspinyin::context(), rspinyin::ic_id(event.inputContext()));
    }

    /// An input context switched away from this input method.
    ///
    /// The base class implementation calls `reset()`; the engine decides that itself
    /// through its own callback, so nothing is reset implicitly here.
    void deactivate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_deactivate == nullptr) {
            return;
        }
        vt->on_deactivate(rspinyin::context(), rspinyin::ic_id(event.inputContext()));
    }

    /// An input context needs its state reset.
    void reset(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_reset == nullptr) {
            return;
        }
        vt->on_reset(rspinyin::context(), rspinyin::ic_id(event.inputContext()));
    }

    /// The manager Fcitx5 handed to the factory, or null if it passed null.
    fcitx::AddonManager *manager() const { return manager_; }

private:
    /// The plugin's only handle back to the host. Kept because the decoder's effects
    /// are applied through the host's own objects (`Instance`, the input-context
    /// manager), which are reachable from here and from nowhere else.
    fcitx::AddonManager *manager_;
};

// Compile-time proof that every pure virtual of the engine chain is implemented: an
// unimplemented one would leave the class abstract and fail this assertion.
static_assert(!std::is_abstract_v<RspinyinEngine>,
              "RspinyinEngine must implement every pure virtual of fcitx::InputMethodEngineV2");

/// The addon instance Fcitx5 creates through the factory in `addon_glue.cpp`.
///
/// It is the engine *and* the addon: the host reaches the engine by treating the
/// instance it was handed as an `InputMethodEngine`, so the two cannot be separate
/// objects (see the file header). The lifecycle around the engine — the Rust handshake
/// and the addon init/destroy sequence — stays in `addon_glue.cpp` and is entered from
/// here.
class RspinyinAddon final : public RspinyinEngine {
public:
    /// Starts the plugin lifecycle around a freshly created engine.
    explicit RspinyinAddon(fcitx::AddonManager *manager) : RspinyinEngine(manager) {
        rspinyin::startPlugin();
    }

    /// Releases the plugin lifecycle. The Rust destroy path is idempotent, so an addon
    /// whose initialisation declined part-way is released exactly like one that
    /// completed.
    ~RspinyinAddon() override { rspinyin::stopPlugin(); }
};

// The invariant the whole file exists for: an instance that is not an
// `InputMethodEngine` crashes the host while it enumerates input methods, so a future
// refactor that drops the base class fails the build instead of the user's session.
static_assert(std::is_base_of_v<fcitx::InputMethodEngine, RspinyinAddon>,
              "the addon instance must be an fcitx::InputMethodEngine: that is how "
              "Fcitx5 reaches the engine");

} // namespace

namespace rspinyin {

/// Creates the addon instance; Fcitx5 takes ownership of the returned pointer.
///
/// Declared in `addon_glue.cpp` and called from the factory there. It has to be defined
/// in this translation unit because the instance is also the engine, and the engine
/// class is complete only here.
fcitx::AddonInstance *createAddonInstance(fcitx::AddonManager *manager) {
    return new RspinyinAddon(manager);
}

} // namespace rspinyin
