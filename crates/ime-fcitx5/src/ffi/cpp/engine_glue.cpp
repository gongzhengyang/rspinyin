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
// The struct definitions mirror the `#[repr(C)]` declarations in `src/ffi/abi.rs` and
// are identical to the copies in the other glue files; see `addon_glue.cpp` for why
// there is no shared header.

#include <cstdint>
#include <type_traits>
#include <vector>

#include <fcitx-utils/key.h>
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

} // namespace rspinyin

namespace {

/// The input-method-engine role of the plugin.
///
/// Nothing instantiates this class yet: Fcitx5 creates one addon instance per shared
/// library (see `addon_glue.cpp`), and how the engine and UI roles are handed out is
/// the next decision to make. The translation unit is compiled regardless, which is
/// what proves these overrides still match the installed headers.
class RspinyinEngine : public fcitx::InputMethodEngineV2 {
public:
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
};

// Compile-time proof that every pure virtual of the engine chain is implemented: an
// unimplemented one would leave the class abstract and fail this assertion.
static_assert(!std::is_abstract_v<RspinyinEngine>,
              "RspinyinEngine must implement every pure virtual of fcitx::InputMethodEngineV2");

} // namespace
