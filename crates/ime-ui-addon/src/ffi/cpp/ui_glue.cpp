// Fcitx5 user-interface glue.
//
// `RspinyinUi` is the class that takes the candidate window away from ClassicUI. It
// derives from `fcitx::UserInterface` — the role Fcitx5 expects a UI addon to implement
// — and forwards the host's calls into the Rust callback table. The signatures below
// were taken from the installed 5.1.7 headers:
//
//   fcitx::UserInterface::update(UserInterfaceComponent, InputContext *)   [pure]
//   fcitx::UserInterface::available()                                      [pure]
//   fcitx::UserInterface::suspend() / resume()                             [pure]
//
// # How Fcitx5 picks the active user interface
//
// There is no call that registers a `UserInterface` object: 5.1.7 has only
// `UserInterfaceManager::load()`, which selects by name among the addons Fcitx5
// discovered with `Category=UI` in their addon description. `updateAvailability()` then
// walks that list in priority order (`UIPriority`, default 0; a tie goes to the
// lexicographically greater name) and takes the first addon whose `available()` answers
// true and whose `UIType` matches the current input method mode.
//
// ClassicUI is suppressed as a consequence rather than by an action: the manager hands
// `InputPanel` updates to the active user interface alone, so once this class is active
// the host's own window receives nothing to draw. Nothing here disables another addon,
// which is what keeps a hard switch away from a user's own UI addon (Kimpanel, for
// instance) out of the picture.
//
// The takeover is therefore: be an addon the manager lists as a user interface, answer
// `available()` honestly, and ask for a re-evaluation when that answer changes.
// `rspinyin_ui_activate` and `rspinyin_ui_current` below are the two calls that do it.
//
// # The wiring this file does not own
//
// Fcitx5 creates one addon instance per addon description through the factory in
// `addon_glue.cpp`, and a `dynamic_cast<fcitx::UserInterface *>` only succeeds on an
// instance that derives from this class. So the factory has to construct this class —
// `rspinyin::createUiInstance` below is that entry point — and the library has to be
// described with `Category=UI` for `addonNames(AddonCategory::UI)` to contain it. Until
// both hold, `rspinyin_ui_activate` answers "not registered" and the candidates stay
// with ClassicUI, which is the documented fallback rather than a failure.
//
// # The caret
//
// 5.1.7's `fcitx::UserInterface` has no `updateCursor` virtual, so the caret rectangle
// arrives the way Fcitx5's own UI addon receives it: an `InputContextCursorRectChanged`
// watcher, installed while this user interface is resumed and dropped while it is
// suspended. `InputContext::cursorRect()` is in client coordinates and
// `InputContext::scaleFactor()` is the client's ratio; turning the pair into a screen
// position is the Rust side's job, and this file only moves the numbers across.
//
// The struct definitions mirror the `#[repr(C)]` declarations in `src/ffi/abi.rs` and
// are identical to the copies in the other glue files; see `addon_glue.cpp` for why
// there is no shared header.

#include <algorithm>
#include <cstdint>
#include <memory>
#include <string>
#include <type_traits>
#include <unordered_set>

#include <fcitx-utils/handlertable.h>
#include <fcitx-utils/rect.h>
#include <fcitx/addoninfo.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include <fcitx/userinterfacemanager.h>

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

/// The user-interface addon's callback table, mirrored from `src/ffi/abi.rs`.
///
/// It is this addon's own table, not the engine's: the two libraries are `dlopen`'d
/// independently and each registers with its own glue. The first two entries are the
/// slots that lived in the engine's table before the split.
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

// Defined in `ui_addon_glue.cpp`, which owns the handshake state and the lifecycle.
namespace rspinyin {

/// The registered callback table, or null when registration did not happen.
const RspinyinUiVtable *vtable();

/// The opaque Rust context handed back to every callback.
void *context();

/// Stable 64-bit identity of an input context.
std::uint64_t ic_id(const fcitx::InputContext *inputContext);

/// Runs the plugin handshake and the addon initialisation sequence.
void startPlugin();

/// Releases everything `startPlugin` took.
void stopPlugin();

} // namespace rspinyin

// Defined in this file, for the addon factory in `ui_addon_glue.cpp` to call. It is the
// one seam the user-interface role needs on the C++ side: Fcitx5 creates the addon
// instance through that factory, and an instance that does not derive from
// `fcitx::UserInterface` can never be selected as the active user interface.
namespace rspinyin {

/// Creates the user-interface addon instance and returns it; the caller owns it.
fcitx::AddonInstance *createUiInstance(fcitx::AddonManager *manager);

} // namespace rspinyin

namespace {

/// The addon name this library is registered under, which is what
/// `UserInterfaceManager` matches against. It must equal the addon description's file
/// name (`rspinyin-ui.conf`) — the engine addon is `rspinyin.conf`, and reading this
/// constant off the engine's name is why the takeover answered "not registered"
/// forever: `addonNames(AddonCategory::UI)` contains the descriptor stems.
constexpr const char *kUiAddonName = "rspinyin-ui";

/// Candidate cap for a single panel update.
///
/// The snapshot is copied on the Fcitx5 main thread, so the work per update has to be
/// bounded; a page larger than this is truncated.
constexpr int kMaxCandidates = 64;

/// The codes `rspinyin_ui_activate` answers with; the Rust side maps them in
/// `activation_from_code`. A code this side does not know must never be read as a
/// takeover, so the "no host" code is the one a caller sees when it is unsure.
enum RspinyinUiActivation : std::uint32_t {
    /// This plugin is the active user interface now.
    kUiActivationActive = 0,
    /// The manager has no user interface of this plugin to switch to.
    kUiActivationNotRegistered = 1,
    /// The manager kept another user interface active.
    kUiActivationOtherUiActive = 2,
    /// There is no manager to ask.
    kUiActivationUnavailable = 3,
};

/// Saturates a value Fcitx5 reports as an `int` — often -1 for "unknown" — into one of
/// the snapshot's byte fields.
std::uint8_t toByte(int value) {
    if (value <= 0) {
        return 0;
    }
    return static_cast<std::uint8_t>(std::min(value, 255));
}

/// Copies the input panel of `inputContext` into `snapshot`.
///
/// `preedit` and `candidates` own the bytes the snapshot points at, so both have to
/// outlive whatever consumes the snapshot. Both are cleared first and reused by the
/// caller's members, so a steady-state update allocates nothing: the host thread's
/// budget for this callback is 100us, and an allocation per keystroke is the easy way
/// to miss it.
void fillSnapshot(const fcitx::InputContext &inputContext, std::string &preedit,
                  std::string &candidates, UiPanelSnapshot &snapshot) {
    const fcitx::InputPanel &panel = inputContext.inputPanel();
    preedit.clear();
    preedit.append(panel.preedit().toString());
    snapshot.preedit_ptr = reinterpret_cast<const std::uint8_t *>(preedit.data());
    snapshot.preedit_len = preedit.size();
    const int caret = panel.preedit().cursor();
    snapshot.caret = caret > 0 ? static_cast<std::uint32_t>(caret) : 0U;

    candidates.clear();
    const std::shared_ptr<fcitx::CandidateList> list = panel.candidateList();
    if (list == nullptr) {
        return;
    }
    const int count = list->size();
    if (count <= 0) {
        return;
    }
    const int capped = std::min(count, kMaxCandidates);
    for (int index = 0; index < capped; ++index) {
        if (index != 0) {
            candidates.push_back('\n');
        }
        candidates.append(list->candidate(index).text().toString());
    }
    snapshot.candidates_ptr = reinterpret_cast<const std::uint8_t *>(candidates.data());
    snapshot.candidates_len = candidates.size();
    snapshot.candidate_count = static_cast<std::uint32_t>(capped);
    snapshot.cursor_index = list->cursorIndex();
    if (const fcitx::PageableCandidateList *pageable = list->toPageable()) {
        snapshot.page = toByte(pageable->currentPage());
        snapshot.total_pages = toByte(pageable->totalPages());
    }
    snapshot.page_size = toByte(count);
}

/// The user-interface role of the plugin.
///
/// The host creates one of these through the addon factory (see
/// `rspinyin::createUiInstance`) and calls it from the Fcitx5 main loop. Every override
/// below does nothing but read a few fields and call into Rust, because it runs on that
/// loop: the budget is 100us and the two `std::string` members are reused for exactly
/// that reason.
class RspinyinUi : public fcitx::UserInterface {
public:
    /// Takes the manager the factory was handed.
    ///
    /// The manager is the only way to reach `UserInterfaceManager` from an addon:
    /// 5.1.7 has no `fcitx::instance()` free function and no accessor on
    /// `AddonInstance`, so the handle has to come in through the factory.
    explicit RspinyinUi(fcitx::AddonManager *manager) : manager_(manager) {}

    /// The host updated one UI component for `inputContext`.
    void update(fcitx::UserInterfaceComponent component,
                fcitx::InputContext *inputContext) override {
        if (component != fcitx::UserInterfaceComponent::InputPanel) {
            // The status strip is built by this plugin's own frame builder from the
            // decoder's state, so the host's status area is not a source for it. That
            // is the same decision as the candidate list: this plugin draws from its own
            // model rather than from what the host holds.
            return;
        }
        const RspinyinUiVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_input_panel_update == nullptr || inputContext == nullptr) {
            return;
        }
        UiPanelSnapshot snapshot{};
        // -1 is "nothing is highlighted"; a zero-initialised field would claim the first
        // candidate is, and `fillSnapshot` only overwrites it for a real list.
        snapshot.cursor_index = -1;
        // These two own the bytes the snapshot points at and are reused across updates.
        fillSnapshot(*inputContext, preedit_, candidates_, snapshot);
        // The answer ("does this frame need drawing") is not a redraw request here: this
        // plugin's frames are posted by the engine, and the host has no drawing left to
        // do for a user interface that draws itself.
        vt->on_input_panel_update(rspinyin::context(), rspinyin::ic_id(inputContext), &snapshot);
    }

    /// Whether this user interface can draw right now.
    ///
    /// Answered by the Rust side, which knows whether the self-drawn window exists and
    /// whether a platform backend can host it. Answering `true` without both would
    /// suppress ClassicUI while nothing draws in its place — the user would see no
    /// candidates at all — so this never guesses.
    bool available() override {
        const RspinyinUiVtable *vt = rspinyin::vtable();
        // No table means the handshake did not happen, so there is nothing that could
        // draw. Answering `true` would suppress ClassicUI while nothing replaced it.
        return vt != nullptr && vt->is_available != nullptr && vt->is_available();
    }

    /// The host suspended this user interface; another one is active.
    void suspend() override {
        // Another user interface owns the candidate window now, so this plugin's caret
        // tracking would only be watching somebody else's cursor.
        watcher_.reset();
        const RspinyinUiVtable *vt = rspinyin::vtable();
        if (vt != nullptr && vt->on_host_suspend != nullptr) {
            vt->on_host_suspend(rspinyin::context());
        }
    }

    /// The host resumed this user interface; input-panel updates are routed here.
    void resume() override {
        watchCursor();
        const RspinyinUiVtable *vt = rspinyin::vtable();
        if (vt != nullptr && vt->on_host_resume != nullptr) {
            vt->on_host_resume(rspinyin::context());
        }
    }

    /// The manager Fcitx5 handed to the factory, or null before it is created.
    fcitx::AddonManager *manager() const { return manager_; }

private:
    /// Starts forwarding caret rectangles, once.
    void watchCursor() {
        if (watcher_ != nullptr || manager_ == nullptr) {
            return;
        }
        fcitx::Instance *instance = manager_->instance();
        if (instance == nullptr) {
            return;
        }
        // The handle owns the registration: dropping it unregisters, which is what
        // `suspend` and the instance's own destruction rely on.
        watcher_ = instance->watchEvent(
            fcitx::EventType::InputContextCursorRectChanged, fcitx::EventWatcherPhase::Default,
            [this](fcitx::Event &event) { forwardCursor(event); });
    }

    /// Forwards one caret rectangle to the Rust side.
    void forwardCursor(fcitx::Event &event) {
        const RspinyinUiVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_cursor_rect == nullptr || !event.isInputContextEvent()) {
            return;
        }
        auto &contextEvent = static_cast<fcitx::InputContextEvent &>(event);
        const fcitx::InputContext *inputContext = contextEvent.inputContext();
        if (inputContext == nullptr) {
            return;
        }
        const fcitx::Rect rect = inputContext->cursorRect();
        const FcitxCursorRect out{rect.left(), rect.top(), rect.width(), rect.height(),
                                  inputContext->scaleFactor()};
        vt->on_cursor_rect(rspinyin::context(), rspinyin::ic_id(inputContext), out);
    }

    /// The manager the factory was handed; null only if the factory passed null.
    fcitx::AddonManager *manager_;
    /// Reused preedit buffer, so a panel update allocates nothing.
    std::string preedit_;
    /// Reused candidate buffer, so a panel update allocates nothing.
    std::string candidates_;
    /// The caret watcher while this user interface is active; declared last so it is
    /// dropped first, before the buffers it does not touch.
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> watcher_;
};

// Compile-time proof that every pure virtual of `fcitx::UserInterface` is implemented:
// an unimplemented one would leave the class abstract and fail this assertion.
static_assert(!std::is_abstract_v<RspinyinUi>,
              "RspinyinUi must implement every pure virtual of fcitx::UserInterface");

/// The addon instance Fcitx5 creates through the factory in `ui_addon_glue.cpp`.
///
/// It is the user interface *and* the addon: `UserInterfaceManager::updateAvailability`
/// walks the addons Fcitx5 discovered with `Category=UI` and dynamic-casts each instance
/// to `fcitx::UserInterface`, so an instance that does not derive from that class can
/// never be selected. The lifecycle around it — the Rust handshake and the addon
/// init/destroy sequence — stays in `ui_addon_glue.cpp` and is entered from here.
class RspinyinUiAddon final : public RspinyinUi {
public:
    /// Starts the plugin lifecycle around a freshly created user interface.
    explicit RspinyinUiAddon(fcitx::AddonManager *manager) : RspinyinUi(manager) {
        rspinyin::startPlugin();
    }

    /// Releases the plugin lifecycle. The Rust destroy path is idempotent, so an addon
    /// whose initialisation declined part-way is released exactly like one that
    /// completed.
    ~RspinyinUiAddon() override { rspinyin::stopPlugin(); }
};

// The invariant the whole file exists for: an instance that is not a
// `fcitx::UserInterface` is invisible to `updateAvailability()`, so the candidates would
// silently stay with ClassicUI. A future refactor that drops the base class fails the
// build instead of the user's session.
static_assert(std::is_base_of_v<fcitx::UserInterface, RspinyinUiAddon>,
              "the addon instance must be an fcitx::UserInterface: that is how Fcitx5 "
              "selects the active user interface");

/// The one user-interface instance, or null before the addon factory creates it.
RspinyinUi *&uiSlot() {
    static RspinyinUi *instance = nullptr;
    return instance;
}

} // namespace

namespace rspinyin {

/// Creates the user-interface addon instance.
///
/// Called by the addon factory in `ui_addon_glue.cpp`, which owns the returned instance.
/// The manager is required: it is the only handle that reaches
/// `UserInterfaceManager`, and without it the plugin could never ask to be the active
/// user interface.
///
/// Creating the instance twice returns the first one, so a host that creates the addon
/// again after a reload keeps a single registration rather than a second one that
/// outlives nothing.
fcitx::AddonInstance *createUiInstance(fcitx::AddonManager *manager) {
    if (uiSlot() == nullptr) {
        uiSlot() = new RspinyinUiAddon(manager);
    }
    return uiSlot();
}

} // namespace rspinyin

/// Asks the manager to re-evaluate which user interface is active.
///
/// Answers with one of `RspinyinUiActivation`. A library that the manager does not list
/// as a user interface cannot be switched to, so it is reported before anything is
/// asked: calling `updateAvailability()` in that state would only re-pick whatever is
/// already active, and a log line claiming a takeover attempt would be a lie.
extern "C" std::uint32_t rspinyin_ui_activate() {
    RspinyinUi *ui = uiSlot();
    if (ui == nullptr) {
        return kUiActivationNotRegistered;
    }
    fcitx::AddonManager *manager = ui->manager();
    if (manager == nullptr) {
        return kUiActivationUnavailable;
    }
    fcitx::Instance *instance = manager->instance();
    if (instance == nullptr) {
        return kUiActivationUnavailable;
    }
    const std::unordered_set<std::string> uiNames = manager->addonNames(fcitx::AddonCategory::UI);
    if (uiNames.count(kUiAddonName) == 0) {
        return kUiActivationNotRegistered;
    }
    fcitx::UserInterfaceManager &uis = instance->userInterfaceManager();
    uis.updateAvailability();
    return uis.currentUI() == kUiAddonName ? kUiActivationActive : kUiActivationOtherUiActive;
}

/// The name of the user interface the manager routes input-panel updates to.
///
/// The pointer is valid until the next call into this function: it refers to a static
/// buffer that the caller (the Rust side, on the host thread) copies out immediately.
/// Null means there is no manager to ask.
extern "C" const char *rspinyin_ui_current() {
    RspinyinUi *ui = uiSlot();
    if (ui == nullptr || ui->manager() == nullptr) {
        return nullptr;
    }
    fcitx::Instance *instance = ui->manager()->instance();
    if (instance == nullptr) {
        return nullptr;
    }
    static std::string name;
    name = instance->userInterfaceManager().currentUI();
    return name.c_str();
}
