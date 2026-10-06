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
// Focus is not on that list: the engine base class has no focus virtual, so focus
// reaches the addon through `Instance::watchEvent` and is forwarded into the table's
// `on_focus_in` / `on_focus_out` slots from there (`watchFocusEvents` below).
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
// # The other direction
//
// The callback table is what the host calls. Once a session has decided that text is
// committed, or that the client's preedit area should hold something, the plugin calls
// back — and those calls are the exported functions at the end of this file. They are
// exports rather than table slots because the table is frozen: appending a slot is an ABI
// break, adding an export is not (the same reasoning ADR-0003 records for the second
// addon's symbols). A call names its input context by the numeric id the table carries,
// so the pointer has to travel beside the callback; `rspinyin::CurrentContext` is what
// carries it, and it is installed by every override below that can lead to such a call.
//
// The struct definitions mirror the `#[repr(C)]` declarations in `src/ffi/abi.rs`, and
// only the engine's own table is mirrored here: the two payloads that left this table at
// ABI version 2 -- `FcitxCursorRect` and `UiPanelSnapshot` -- are the user-interface
// addon's and are mirrored in `crates/ime-ui-addon/src/ffi/cpp/ui_addon_glue.cpp`. A copy
// kept here would describe a layout nothing calls while claiming to be the contract, and
// the field order of the table below is the ABI: a stale slot in it would put every
// callback behind it at the wrong offset. See `addon_glue.cpp` for why there is no shared
// header.

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <type_traits>
#include <vector>

#include <fcitx-utils/key.h>
#include <fcitx-utils/log.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/text.h>

// ── Mirrored C ABI contract ──────────────────────────────────────────────────────
struct FcitxKeyEvent {
    std::uint32_t sym;
    std::uint32_t state;
    bool is_release;
    std::uint32_t time_ms;
};

struct RspinyinVtable {
    std::uint32_t abi_version;

    bool (*on_addon_init)(void *);
    void (*on_addon_destroy)(void *);

    bool (*on_activate)(void *, std::uint64_t);
    void (*on_deactivate)(void *, std::uint64_t);
    void (*on_reset)(void *, std::uint64_t);
    bool (*on_key_event)(void *, std::uint64_t, const FcitxKeyEvent *);

    void (*on_focus_in)(void *, std::uint64_t);
    void (*on_focus_out)(void *, std::uint64_t);

    void (*on_commit_string)(void *, std::uint64_t, const char *, std::size_t);
    void (*on_set_preedit)(void *, std::uint64_t, const char *, std::size_t, std::uint32_t);
    void (*on_clear_preedit)(void *, std::uint64_t);
};

// The layout is the ABI, so it is asserted rather than described: twelve words are the
// version field and eleven slots, and a slot added, removed or padded differently here
// would put every callback behind it at an offset the Rust side does not have. The two
// copies of this struct cannot share a header (see the file header), so this is what
// catches a drift between them at compile time instead of at the first key event.
static_assert(sizeof(RspinyinVtable) == 12 * sizeof(void *),
              "the engine's callback table is the ABI version plus eleven slots");

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

// The rest of this namespace is defined here rather than in `addon_glue.cpp`: it is the
// carrier the callbacks below install and the host calls at the end of this file resolve
// against, so it belongs beside them.

/// The input context the callback currently being served belongs to.
///
/// The calls Rust makes back into this library — committing text, filling the client's
/// preedit area — name the context by the numeric id the callback table carries, and that
/// table has no slot for the pointer. So the pointer travels *beside* the callback rather
/// than through it: every callback below installs the context it was given for the length
/// of its body, and a host call resolves the id against it.
///
/// A thread-local is exactly the right carrier. Every one of these callbacks runs on the
/// Fcitx5 main loop, the pointer is valid for the whole of the callback, and the guard
/// clears it on the way out — so a call that arrives outside a callback, or one naming a
/// different context, is refused rather than followed. It is also allocation-free and
/// lock-free, which the key path requires.
thread_local fcitx::InputContext *currentContext_ = nullptr;

/// Serves one callback with `inputContext` installed as the current one.
///
/// A guard rather than a pair of assignments: every exit path — the early returns that
/// skip the Rust call included — has to leave the thread-local as it found it.
class CurrentContext {
public:
    explicit CurrentContext(fcitx::InputContext *inputContext)
        : previous_(currentContext_) {
        currentContext_ = inputContext;
    }
    ~CurrentContext() { currentContext_ = previous_; }
    CurrentContext(const CurrentContext &) = delete;
    CurrentContext &operator=(const CurrentContext &) = delete;

private:
    /// The context installed by whatever called this callback, so that a nested call
    /// (Fcitx5 dispatching from inside a callback) restores the right one.
    fcitx::InputContext *previous_;
};

/// Resolves the context a host call names, or null when it does not match the callback
/// being served.
///
/// The id is checked rather than trusted: it comes across the ABI as a plain number, and
/// the digest is what the Rust side built its session key from, so a mismatch means the
/// call and the callback disagree about which context they are working on. Refusing is
/// the only safe answer — the alternative is committing text into somebody else's window.
///
/// The parameter is named `context_id` rather than `ic_id` because the digest function of
/// that name is what the body has to call: a parameter of the same name would shadow it.
fcitx::InputContext *resolveContext(std::uint64_t context_id) {
    fcitx::InputContext *inputContext = currentContext_;
    if (inputContext == nullptr || ic_id(inputContext) != context_id) {
        return nullptr;
    }
    return inputContext;
}

} // namespace rspinyin

// ── The Rust export the host's reload action travels through ────────────────────
//
// The addon's `reloadConfig` override below forwards into this function. It is linked
// directly, like `rspinyin_plugin_init` in `addon_glue.cpp`: both halves of the call
// live in one cdylib. The symbol travels the ADR-0011 append-only path — an added
// export, not a callback-table slot — so the ABI version does not move.
extern "C" bool rspinyin_config_reload();

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
    explicit RspinyinEngine(fcitx::AddonManager *manager) : manager_(manager) {
        watchFocusEvents();
    }

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
        // The pointer travels beside the callback: the table's slot carries the context's
        // numeric id, and the calls Rust makes back need the object. See
        // `rspinyin::CurrentContext`.
        rspinyin::CurrentContext current{event.inputContext()};
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
    ///
    /// The context is installed for the length of the call because the activation is what
    /// creates the session, and a session that is created has nothing to say to the host
    /// yet; the guard is here so that a later change to what activation does cannot reach
    /// a host call with no context installed.
    void activate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_activate == nullptr) {
            return;
        }
        rspinyin::CurrentContext current{event.inputContext()};
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
        // Ending a session hides the candidate window and empties the client's preedit
        // area, so the host calls Rust makes from inside this callback need the context.
        rspinyin::CurrentContext current{event.inputContext()};
        vt->on_deactivate(rspinyin::context(), rspinyin::ic_id(event.inputContext()));
    }

    /// An input context needs its state reset.
    ///
    /// Like a deactivation, this ends the composition without committing it, so the same
    /// host calls follow and the context is installed the same way.
    void reset(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override {
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_reset == nullptr) {
            return;
        }
        rspinyin::CurrentContext current{event.inputContext()};
        vt->on_reset(rspinyin::context(), rspinyin::ic_id(event.inputContext()));
    }

    /// The manager Fcitx5 handed to the factory, or null if it passed null.
    fcitx::AddonManager *manager() const { return manager_; }

private:
    /// Forwards the focus events into the callback table.
    ///
    /// The engine base class has no focus virtual: focus reaches an addon through the
    /// instance's event bus, so `Instance::watchEvent` is the forwarding point the
    /// `on_focus_in` / `on_focus_out` table slots hang from. The handlers read the table
    /// lazily, so installing them in the constructor — before `startPlugin` registers
    /// the table — is safe: an event that arrives before registration is dropped,
    /// exactly like one that arrives during shutdown. Each body installs the context it
    /// was given for its length, because the host calls Rust makes in answer (emptying
    /// the client's preedit area) resolve against it.
    ///
    /// Fcitx5 may observe the same transition twice — a focus out can also reach the
    /// `deactivate` virtual. The two slots stay independent: the table names what each
    /// means, and the session layer decides what the combination produces. The
    /// symmetric notification that does not exist anywhere — the host telling the plugin
    /// a context was destroyed — is a table slot the ADR-0011 symbol batch owns; the
    /// Rust side reclaims a context the host stopped focusing instead.
    void watchFocusEvents() {
        auto *instance = manager_->instance();
        focus_in_handler_ = instance->watchEvent(
            fcitx::EventType::InputContextFocusIn, fcitx::EventWatcherPhase::Default,
            [](fcitx::Event &event) {
                const RspinyinVtable *vt = rspinyin::vtable();
                if (vt == nullptr || vt->on_focus_in == nullptr) {
                    return;
                }
                auto &focus = static_cast<fcitx::FocusInEvent &>(event);
                rspinyin::CurrentContext current{focus.inputContext()};
                vt->on_focus_in(rspinyin::context(),
                                rspinyin::ic_id(focus.inputContext()));
            });
        focus_out_handler_ = instance->watchEvent(
            fcitx::EventType::InputContextFocusOut, fcitx::EventWatcherPhase::Default,
            [](fcitx::Event &event) {
                const RspinyinVtable *vt = rspinyin::vtable();
                if (vt == nullptr || vt->on_focus_out == nullptr) {
                    return;
                }
                auto &focus = static_cast<fcitx::FocusOutEvent &>(event);
                rspinyin::CurrentContext current{focus.inputContext()};
                vt->on_focus_out(rspinyin::context(),
                                 rspinyin::ic_id(focus.inputContext()));
            });
    }

    /// The plugin's only handle back to the host. Kept because the decoder's effects
    /// are applied through the host's own objects (`Instance`, the input-context
    /// manager), which are reachable from here and from nowhere else.
    fcitx::AddonManager *manager_;

    /// The registered focus handlers, kept so that destroying the engine disconnects
    /// them with it: a handler outliving the instance it reads would run on a dangling
    /// bus.
    // fcitx5 >= 5.1.19 renamed HandlerEntry to HandlerTableEntry; the table entry
    // type is what connect() has returned since, so spell the current name.
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> focus_in_handler_;
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> focus_out_handler_;
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

    /// Reloads the configuration, on the host's request.
    ///
    /// `fcitx::AddonInstance::reloadConfig()` is the slot Fcitx5 fires when its own
    /// reload action runs (`fcitx5-remote --reload`, or an apply from the
    /// configuration tools); overriding it here is what turns that action into a
    /// re-read instead of the base class's silent default. The virtual answers void,
    /// so the boolean the Rust entry returns is logged rather than mapped: `false`
    /// means the entry did not run to completion, and the condition it names is
    /// already on the diagnostic channel the Rust side keeps. The work is the re-read
    /// of one small TOML document on this thread — the same bounded read Fcitx5's own
    /// reload performs.
    void reloadConfig() override {
        if (rspinyin_config_reload()) {
            FCITX_INFO() << "rspinyin: configuration reloaded";
        } else {
            FCITX_WARN() << "rspinyin: configuration reload did not run to completion";
        }
    }
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

// ── Host calls made from Rust ────────────────────────────────────────────────────
//
// The other direction of the ABI. The table above is what the host calls; these are what
// the plugin calls back on, once a session has decided that text is committed or that the
// client's preedit area should hold something. They are plain exported functions rather
// than table slots because the table is frozen and these travel the other way: appending a
// slot would be an ABI break, adding an export is not.
//
// Each one resolves the context it was told to work on and refuses the call when the id
// does not match the callback being served — see `rspinyin::resolveContext`. A refusal is
// a `false` return, which the Rust side turns into a diagnostic rather than into a silent
// loss of the user's text.

/// Inserts `text` into the client's text field, at its caret.
///
/// The client owns the string it is given, so this is the one call in this section that
/// copies: Fcitx5's own signature takes a `std::string`. The buffer Rust hands over is a
/// live slice for the length of the call and is not retained.
extern "C" bool rspinyin_host_commit(std::uint64_t ic_id, const char *text, std::size_t len) {
    if (text == nullptr) {
        return false;
    }
    fcitx::InputContext *inputContext = rspinyin::resolveContext(ic_id);
    if (inputContext == nullptr) {
        return false;
    }
    inputContext->commitString(std::string(text, len));
    return true;
}

/// Writes `text` into the client's own preedit area, with its caret.
///
/// The preedit shown inside the client window, not the candidate window's header: which of
/// the two carries the composing text is the `[ui] client_preedit` policy, decided above
/// this layer.
extern "C" bool rspinyin_host_set_preedit(std::uint64_t ic_id, const char *text, std::size_t len,
                                          std::uint32_t caret) {
    if (text == nullptr) {
        return false;
    }
    fcitx::InputContext *inputContext = rspinyin::resolveContext(ic_id);
    if (inputContext == nullptr) {
        return false;
    }
    // Fcitx5 takes the cursor as a signed byte offset. An offset past the end of the text
    // is clamped to it rather than wrapped into a negative one, which Fcitx5 would read as
    // "at the end" — the same place, reached without a cast that could turn 4 GiB into a
    // cursor near the start of the line.
    const std::size_t bounded = caret < len ? static_cast<std::size_t>(caret) : len;
    fcitx::Text preedit(std::string(text, len));
    preedit.setCursor(static_cast<int>(bounded));
    inputContext->inputPanel().setPreedit(preedit);
    return true;
}

/// Empties the client's own preedit area.
///
/// Called when a composition ends and when the client-preedit policy is off, in which case
/// the area has to stay empty rather than hold what an earlier configuration put there.
extern "C" bool rspinyin_host_clear_preedit(std::uint64_t ic_id) {
    fcitx::InputContext *inputContext = rspinyin::resolveContext(ic_id);
    if (inputContext == nullptr) {
        return false;
    }
    inputContext->inputPanel().setPreedit(fcitx::Text());
    return true;
}
