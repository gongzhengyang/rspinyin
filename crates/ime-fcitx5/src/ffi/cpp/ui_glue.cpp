// Fcitx5 user-interface glue.
//
// `RspinyinUi` derives from `fcitx::UserInterface` — the class Fcitx5 expects a UI addon
// to implement — and forwards the input-panel updates into the Rust callback table. The
// signatures below were taken from the installed 5.1.7 headers:
//
//   fcitx::UserInterface::update(UserInterfaceComponent, InputContext *)   [pure]
//   fcitx::UserInterface::available()                                      [pure]
//   fcitx::UserInterface::suspend() / resume()                             [pure]
//
// The struct definitions mirror the `#[repr(C)]` declarations in `src/ffi/abi.rs` and
// are identical to the copies in the other glue files; see `addon_glue.cpp` for why
// there is no shared header.

#include <algorithm>
#include <cstdint>
#include <memory>
#include <string>
#include <type_traits>

#include <fcitx/candidatelist.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputpanel.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

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

/// Candidate cap for a single panel update.
///
/// The snapshot is copied on the Fcitx5 main thread, so the work per update has to be
/// bounded; a page larger than this is truncated.
constexpr int kMaxCandidates = 64;

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
/// outlive whatever consumes the snapshot.
void fillSnapshot(const fcitx::InputContext &inputContext, std::string &preedit,
                  std::string &candidates, UiPanelSnapshot &snapshot) {
    const fcitx::InputPanel &panel = inputContext.inputPanel();
    preedit = panel.preedit().toString();
    snapshot.preedit_ptr = reinterpret_cast<const std::uint8_t *>(preedit.data());
    snapshot.preedit_len = preedit.size();
    const int caret = panel.preedit().cursor();
    snapshot.caret = caret > 0 ? static_cast<std::uint32_t>(caret) : 0U;

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
/// Nothing instantiates this class yet: Fcitx5 creates one addon instance per shared
/// library (see `addon_glue.cpp`), and how the engine and UI roles are handed out is
/// the next decision to make. The translation unit is compiled regardless, which is
/// what proves these overrides still match the installed headers.
class RspinyinUi : public fcitx::UserInterface {
public:
    /// The host updated one UI component for `inputContext`.
    void update(fcitx::UserInterfaceComponent component,
                fcitx::InputContext *inputContext) override {
        if (component != fcitx::UserInterfaceComponent::InputPanel) {
            // Stub: the status strip is built from the StatusArea component and belongs
            // to the status-strip work; nothing consumes it yet.
            return;
        }
        const RspinyinVtable *vt = rspinyin::vtable();
        if (vt == nullptr || vt->on_input_panel_update == nullptr || inputContext == nullptr) {
            return;
        }
        // These two own the bytes the snapshot points at; both outlive the call below.
        std::string preedit;
        std::string candidates;
        UiPanelSnapshot snapshot{};
        fillSnapshot(*inputContext, preedit, candidates, snapshot);
        vt->on_input_panel_update(rspinyin::context(), rspinyin::ic_id(inputContext), &snapshot);
    }

    /// Whether this UI can render right now.
    bool available() override {
        // Stub: the self-drawn window does not exist yet. Reporting availability would
        // make Fcitx5 route input-panel updates to a UI that cannot draw, so the honest
        // answer today is `false`, and ClassicUI keeps drawing the candidates.
        return false;
    }

    /// The host suspends this UI.
    void suspend() override {
        // Stub: suspending the surface belongs to the candidate-window work.
    }

    /// The host resumes this UI.
    void resume() override {
        // Stub: resuming the surface belongs to the candidate-window work.
    }
};

// Compile-time proof that every pure virtual of `fcitx::UserInterface` is implemented:
// an unimplemented one would leave the class abstract and fail this assertion.
static_assert(!std::is_abstract_v<RspinyinUi>,
              "RspinyinUi must implement every pure virtual of fcitx::UserInterface");

} // namespace
