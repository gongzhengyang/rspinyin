//! The mode bits the engine owns, and the status strip they paint.
//!
//! # Responsibility
//!
//! Three switches belong to the engine rather than to a session: the language switch, which
//! *is* Fcitx5's input-method state, and the full-width and punctuation switches, which are
//! the plugin's own output choices. A session holds none of them and never sees them; what
//! the user sees is the status strip the engine writes into every frame, and that is what
//! this module builds — the bits, the label the window shows, and the one place an action
//! that flips a bit is applied.
//!
//! # Boundary
//!
//! Plain Rust over values. The one call that leaves the process — flipping the host's input
//! state — goes through [`Host`], exactly as every other host effect does.

use ime_types::{KeyAction, StatusStrip};

use crate::engine::host::Host;

/// What the status strip shows in Chinese mode when the configuration names no layout.
const MODE_LABEL_CHINESE: &str = "中";

/// What the status strip shows in English mode, temporary English included.
const MODE_LABEL_ENGLISH: &str = "英";

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
    /// `has_user_dict_hit` and `readonly` are facts about the decode and the data
    /// directory: the layers that own them write them into the strip when they land, and
    /// the routing layer has nothing to say about either.
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
        StatusStrip {
            mode_label: String::from(self.mode_label(temp_english, hint)),
            full_width: self.is_full_width,
            punctuation_full: self.is_punct_full,
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
