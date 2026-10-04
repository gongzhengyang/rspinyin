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

use ime_dict::paths;
use ime_types::{KeyAction, StatusStrip};

use crate::engine::host::Host;

/// What the status strip shows in Chinese mode when the configuration names no layout.
const MODE_LABEL_CHINESE: &str = "中";

/// What the status strip shows in English mode, temporary English included.
const MODE_LABEL_ENGLISH: &str = "英";

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
}

#[cfg(test)]
mod tests;
