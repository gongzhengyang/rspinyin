//! The mode bits the engine owns, and the status strip they paint.
//!
//! # Responsibility
//!
//! Three switches belong to the engine rather than to a session: the language switch, which
//! *is* Fcitx5's input-method state, and the full-width and punctuation switches, which are
//! the plugin's own output choices. A session holds none of them and never sees them; what
//! the user sees is the status strip the engine writes into every frame, and that is what
//! this module builds — the bits, the label the window shows, the degradation notice that
//! may occupy the label's slot, and the one place an action that flips a bit is applied.
//!
//! The two output switches have one reader each, and both live here so the bits cannot
//! gain a meaning without it being written next to them: [`Modes::transform_output`] is
//! the rewrite a commit goes through on its way to the application, and
//! [`Modes::mode_flash_line`] is the line a switch reports when it is flipped with no
//! composition on screen — the one state where the strip that normally carries the mode
//! does not exist.
//!
//! # The notice slot
//!
//! The strip's label slot doubles as the degradation notice slot: when the process carries
//! a degradation the user must hear about, its sentence takes the label's place for that
//! frame. The mapping between a stable diagnostic code and the sentence, and the priority
//! that decides which sentence wins when several hold at once, is the frozen table below —
//! the copy is user-visible Chinese and the priorities are part of the contract, so neither
//! is reworded casually. A notice never *adds* a second line: the mode's own text yields
//! the slot and comes back when the facts clear.
//!
//! The facts are read live rather than stored. The read-only flag is sticky but can be set
//! by any data step at any time — including after the router was built — so a notice copied
//! into `Modes` at construction could outlive the truth; the strip asks the process on
//! every frame instead, which costs a few atomic loads off the decode path the frame is
//! already on.
//!
//! # Boundary
//!
//! Plain Rust over values. The one call that leaves the process — flipping the host's input
//! state — goes through [`Host`], exactly as every other host effect does.

use std::borrow::Cow;

use ime_core::passthrough::{PassthroughFlags, PunctMode, transform_committed};
use ime_core::state::effects::ModeBit;
use ime_dict::paths;
use ime_types::{KeyAction, StatusStrip};

use crate::engine::host::Host;

/// What the status strip shows in Chinese mode when the configuration names no layout.
const MODE_LABEL_CHINESE: &str = "中";

/// What the status strip shows in English mode, temporary English included.
const MODE_LABEL_ENGLISH: &str = "英";

/// The line an idle full-width switch reports when it switched on.
const FULL_WIDTH_FLASH_ON: &str = "mode/full-width: on";

/// The line an idle full-width switch reports when it switched off.
const FULL_WIDTH_FLASH_OFF: &str = "mode/full-width: off";

/// The line an idle punctuation switch reports when it switched to Chinese punctuation.
const PUNCT_FLASH_ON: &str = "mode/punct-full: on";

/// The line an idle punctuation switch reports when it switched to English punctuation.
const PUNCT_FLASH_OFF: &str = "mode/punct-full: off";

/// One row of the frozen notice table: a degradation, the sentence the user reads, and the
/// priority that decides which sentence wins when several hold at once. Lower numbers win.
struct Notice {
    /// The stable `domain/action/reason` code the fact is known by.
    ///
    /// The runtime answers with the text alone and never logs the notice, so no
    /// production read of this field exists; it is kept because the code is the row's
    /// frozen identity and the frozen mapping is code-to-copy, so a grep in either
    /// direction has to land on this table rather than on two files that can drift
    /// apart.
    #[allow(dead_code)]
    code: &'static str,
    /// The user-visible sentence that takes the label slot. Chinese by the language rules.
    text: &'static str,
    /// Priority; `1` is the highest, and it is what the selection orders by.
    priority: u8,
}

/// The frozen notice table, highest priority first.
///
/// A notice is shown only while its fact is set, so a row whose fact source has not been
/// wired yet is a dark row rather than a wrong one. The two engine-side facts are read
/// live from the process ([`NoticeFacts::current`]); the two user-interface facts are
/// owned by the other addon — a separate `dlopen`'d library whose statics this one
/// cannot read — and reach the engine only once the cross-addon wire carries a fact
/// channel back; they land as diagnostics on that side in the meantime, under the same
/// codes.
const NOTICE_TABLE: [Notice; 4] = [
    Notice {
        code: "dict/unavailable",
        text: "词典不可用，仅直通输入",
        priority: 1,
    },
    Notice {
        code: "data/readonly-mode",
        text: "用户词库只读，学习已暂停",
        priority: 2,
    },
    Notice {
        code: "ui/font/missing-cjk",
        text: "未找到中文字体，显示可能异常",
        priority: 3,
    },
    Notice {
        code: "ui/theme/blur-unavailable",
        text: "桌面不支持模糊，已切换不透明",
        priority: 4,
    },
];

/// The degradation facts a frame's notice is selected from.
///
/// A plain value rather than a set of reads so that [`select_notice`] stays a pure function
/// every branch of which a test can reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NoticeFacts {
    /// No dictionary is mapped: decoding degrades to pass-through.
    dict_unavailable: bool,
    /// The data directory is not writable: learning is paused.
    readonly: bool,
    /// The font probe found no family that draws CJK.
    font_missing_cjk: bool,
    /// The compositor refused the blur the theme asked for.
    blur_unavailable: bool,
}

impl NoticeFacts {
    /// The process's degradation facts as this frame sees them.
    ///
    /// The read-only flag is read live: it is process-wide and sticky. The
    /// dictionary-unavailable fact reaches this module through the addon's re-export of
    /// the session layer's reader — the one visibility bridge the private module layout
    /// allows — and is sticky the same way, because a failed load is never retried. The
    /// two interface-side facts wait for the cross-addon channel — see [`NOTICE_TABLE`].
    ///
    /// # Panics
    ///
    /// Never.
    fn current() -> Self {
        Self {
            dict_unavailable: crate::addon::dictionary_unavailable(),
            readonly: paths::is_readonly_mode(),
            ..Self::default()
        }
    }

    /// Whether the table row of `index` holds for these facts.
    ///
    /// The index is the row's position in [`NOTICE_TABLE`], which is priority order.
    ///
    /// # Panics
    ///
    /// Never.
    fn holds(self, index: usize) -> bool {
        match index {
            0 => self.dict_unavailable,
            1 => self.readonly,
            2 => self.font_missing_cjk,
            3 => self.blur_unavailable,
            _ => false,
        }
    }
}

/// Selects the notice the label slot shows, or `None` when no degradation holds.
///
/// The selection orders by the rows' own priority rather than by table position, so a row
/// inserted out of order still loses to a higher-priority fact: a `dict/unavailable` start
/// beats a read-only data directory no matter when either was detected. Equal priorities
/// keep the earlier row, and the test suite pins the frozen table to unique priorities so
/// the tie case stays one the table cannot produce.
///
/// # Panics
///
/// Never.
fn select_notice(facts: NoticeFacts) -> Option<&'static str> {
    NOTICE_TABLE
        .iter()
        .enumerate()
        .filter(|(index, _)| facts.holds(*index))
        .min_by_key(|(_, notice)| notice.priority)
        .map(|(_, notice)| notice.text)
}

/// What the label slot shows: the notice when one holds, the mode's own text otherwise.
///
/// # Panics
///
/// Never.
fn strip_label(facts: NoticeFacts, mode_label: &'static str) -> &'static str {
    select_notice(facts).unwrap_or(mode_label)
}

/// The mode bits the engine owns.
///
/// The session does not hold them: the language switch *is* Fcitx5's input-method state,
/// and the full-width and punctuation switches are the plugin's own output choices. They
/// reach the window through the status strip the engine writes into every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Modes {
    /// Whether the host has the input method enabled for this context, which is the
    /// Chinese mode. The engine mirrors the host's answer rather than keeping a flag of
    /// its own.
    is_chinese: bool,
    /// Whether the punctuation the plugin commits is written full width.
    is_full_width: bool,
    /// Whether the punctuation the plugin commits is Chinese.
    is_punct_full: bool,
}

impl Default for Modes {
    /// A freshly activated context: Chinese, half width, Chinese punctuation — the three
    /// values the shipped configuration declares.
    ///
    /// The two output bits are the `[engine]` section's own defaults (`punct_mode`
    /// "chinese", `full_width` false). They live here rather than in a second projection
    /// because the switches are runtime state — the user flips them while typing — and
    /// the configuration only says where they start; this constructor is therefore the
    /// one place the document's defaults and the engine's switches can drift, which is
    /// why both sides of the alignment are named in one sentence.
    fn default() -> Self {
        Self {
            is_chinese: true,
            is_full_width: false,
            is_punct_full: true,
        }
    }
}

impl Modes {
    /// The status strip the window draws.
    ///
    /// The label slot shows the mode's own text unless a degradation notice holds, in which
    /// case the notice takes the slot for this frame (see the module documentation). The
    /// `readonly` bit and the notice come from one live read of the process facts, so the
    /// lock and the sentence always agree; `chinese` is the mode bit itself, which the view
    /// reads instead of inferring anything from the label's text.
    ///
    /// `has_user_dict_hit` and `script` have no producer on this side in v1 and stay at
    /// their defaults; the deferral is registered on the fields themselves.
    ///
    /// # Arguments
    ///
    /// * `temp_english` — whether the session is in temporary English, which the label
    ///   follows.
    /// * `hint` — the active layout's label from the configuration, or `None` when the
    ///   user turned the hint off.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn status(&self, temp_english: bool, hint: Option<&'static str>) -> StatusStrip {
        let facts = NoticeFacts::current();
        StatusStrip {
            mode_label: String::from(strip_label(facts, self.mode_label(temp_english, hint))),
            full_width: self.is_full_width,
            punctuation_full: self.is_punct_full,
            chinese: self.is_chinese,
            readonly: facts.readonly,
            ..StatusStrip::default()
        }
    }

    /// The label the status strip shows.
    ///
    /// Temporary English shows as English whatever the persistent state is, because that
    /// is the mode the keys are in. Chinese mode shows the layout the configuration names
    /// (`scheme.show_hint`) — a user who switched to a double-pinyin layout can see which
    /// one is answering their keys — and falls back to the engine's own label when the
    /// hint is off.
    ///
    /// # Arguments
    ///
    /// * `temp_english` — whether the session is in temporary English.
    /// * `hint` — the active layout's label, or `None` when there is none to show.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn mode_label(
        &self,
        temp_english: bool,
        hint: Option<&'static str>,
    ) -> &'static str {
        if self.is_chinese && !temp_english {
            hint.unwrap_or(MODE_LABEL_CHINESE)
        } else {
            MODE_LABEL_ENGLISH
        }
    }

    /// Applies the keys the engine owns rather than the session.
    ///
    /// The three mode keys change bits the engine holds: the persistent language switch
    /// goes to the host, because switching to English means Fcitx5 hands the keyboard
    /// back to the application, and the full-width and punctuation switches are the
    /// plugin's own output choices. The session repaints from the frame context the
    /// caller wrote, so nothing here emits an effect.
    ///
    /// # Arguments
    ///
    /// * `action` — what the routing table made of the key.
    /// * `ic` — the host's identity for the input context the key arrived in.
    /// * `host` — the boundary the language switch is executed against.
    ///
    /// # Returns
    ///
    /// Whether `action` was one of them.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn apply(&mut self, action: KeyAction, ic: u64, host: &mut dyn Host) -> bool {
        match action {
            KeyAction::ToggleLang => {
                // No chord of the routing table produces this action any more — the
                // language switch is the host's own hotkey — so the arm runs only when a
                // future host hands the key over. The read-back is what keeps it honest
                // when that day comes: the bit is the state the host reports, never the
                // state the switch was hoped to produce.
                self.is_chinese = host.toggle_enabled(ic);
                true
            }
            KeyAction::ToggleFullWidth => {
                self.is_full_width = !self.is_full_width;
                true
            }
            KeyAction::TogglePunct => {
                self.is_punct_full = !self.is_punct_full;
                true
            }
            _ => false,
        }
    }

    /// Rewrites committed text by the output half of the mode bits.
    ///
    /// This is the reader the full-width and punctuation switches existed for: with the
    /// switches on, the marks and ASCII characters a commit carries come out the way the
    /// switches promise, and with them off the text comes back borrowed and untouched.
    /// The one call sits at the commit door — see the effect executor — because that is
    /// the only place text leaves for the application, which is what makes the switches
    /// true switches: they change what the user's next commit looks like, not only an
    /// icon.
    ///
    /// The per-character work is `ime-core`'s passthrough policy, which owns the
    /// substitution table and the widening map; this method is the engine's one call into
    /// it, so the two ends of the mapping cannot drift.
    ///
    /// # Arguments
    ///
    /// * `text` — the text a commit effect is about to hand to the host.
    ///
    /// # Returns
    ///
    /// The text to commit. It borrows `text` whenever no character changes — the whole
    /// hot path, a commit of Chinese candidates included — so a commit the mode bits
    /// cannot alter allocates nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn transform_output<'a>(&self, text: &'a str) -> Cow<'a, str> {
        transform_committed(text, self.is_punct_full, self.is_full_width)
    }

    /// The passthrough policy's view of this context, as one value for `classify`.
    ///
    /// The two output switches are the context's runtime state and the two input flags
    /// are the document's, so neither side can build the policy's flags alone; this
    /// constructor is the one place they meet. `temp_english` is deliberately absent:
    /// the router answers a context in that mode before the policy is ever asked, so a
    /// value that reaches here is always `false`, and carrying a lie would let the
    /// policy's rule order drift away from the router's.
    ///
    /// # Arguments
    ///
    /// * `auto_english_on_uppercase` — `[engine] auto_english_on_uppercase`, as the
    ///   configuration projects it.
    /// * `passthrough_url` — `[engine] passthrough_url`, as the configuration projects
    ///   it.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn passthrough_flags(
        &self,
        auto_english_on_uppercase: bool,
        passthrough_url: bool,
    ) -> PassthroughFlags {
        PassthroughFlags {
            auto_english_on_uppercase,
            passthrough_url,
            punct_mode: if self.is_punct_full {
                PunctMode::Chinese
            } else {
                PunctMode::English
            },
            full_width: self.is_full_width,
            temp_english: false,
        }
    }

    /// The diagnostic line an idle mode switch reports, named for the bit and the state
    /// it took.
    ///
    /// The bits are applied before the session is stepped, so by the time the flash
    /// effect is executed the fields already hold the value the switch produced: the
    /// line is the truth the user now owns, never the state they left. The four spellings
    /// are constants because the codes are matched by tests and by a reader's eye, and a
    /// formatted string would make both guess.
    ///
    /// # Arguments
    ///
    /// * `bit` — the mode bit the flash announces.
    ///
    /// # Returns
    ///
    /// The stable line the executor reports on the diagnostic channel.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn mode_flash_line(&self, bit: ModeBit) -> &'static str {
        match (bit, self.is_full_width, self.is_punct_full) {
            (ModeBit::FullWidth, true, _) => FULL_WIDTH_FLASH_ON,
            (ModeBit::FullWidth, false, _) => FULL_WIDTH_FLASH_OFF,
            (ModeBit::PunctFull, _, true) => PUNCT_FLASH_ON,
            (ModeBit::PunctFull, _, false) => PUNCT_FLASH_OFF,
        }
    }
}

#[cfg(test)]
mod tests;
