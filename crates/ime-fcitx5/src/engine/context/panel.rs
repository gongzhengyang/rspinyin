//! The panel chords: the keys that ask for a panel, and the request they leave behind.
//!
//! # Responsibility
//!
//! Two of the design's shortcuts open a panel instead of acting on a composition:
//! `Ctrl+Shift+/` asks for the command palette and `Ctrl+Shift+P` for the diagnostics panel.
//! Neither is a row of the routing table, and neither produces a
//! [`KeyAction`](ime_types::KeyAction).
//!
//! A `KeyAction` is the session's vocabulary: it says what a session does with a key it was
//! given. Opening a panel is not something a session does — it is a host-layer mode, drawn in
//! the candidate window's own surface — and an action enum that carried it would make every
//! session state answer a question that is not its own. The chords are therefore answered by
//! the bus, ahead of the layer walk, and what they produce is an [`Overlay`].
//!
//! # The request a chord leaves behind
//!
//! No panel is drawn yet: this module is the reservation for the keys of two that are still to
//! come. A chord is the plugin's all the same — handing it to the application would leave the
//! user pressing a shortcut that does nothing and is not reported either — so the key is kept
//! and the request travels to the caller, which is the layer that can report
//! [`UI_NOT_IMPLEMENTED_CODE`] and, once a panel exists, draw it.
//!
//! The bus does not report it itself. It holds no host object, and the crate's diagnostic
//! channel is process-wide state that a module documented as free of global state must not
//! reach. What the bus does is state the fact precisely: [`Dispatcher::take_overlay_request`]
//! hands the caller the panel a chord asked for, once, and the caller decides what to do with
//! it.
//!
//! # Boundary
//!
//! Plain Rust, like the rest of the bus: a chord is a function of the key and of two facts
//! about the session, and the request is a value the caller takes.

use super::{Dispatcher, KeyEvent, SessionView};
use crate::engine::{CTRL, MODIFIER_MASK, SHIFT};

/// `FcitxKey_slash`, the key the command-palette chord names.
pub(super) const KEY_SLASH: u32 = 0x002f;

/// `FcitxKey_question`, the other shape the slash key can arrive in.
///
/// A host folds `Shift` into the symbol, exactly as it does for a letter: on a layout where
/// `?` is the shifted form of the key `/` sits on, `Ctrl+Shift+/` reaches the plugin as
/// `question` rather than as `slash`. The two shapes are one key on the keyboard, and a chord
/// that matched only one of them would work on some layouts and not on others. The letter
/// chords answer the same problem the same way, by matching both the lowercase and the
/// uppercase symbol.
pub(super) const KEY_QUESTION: u32 = 0x003f;

/// `FcitxKey_p`, the key the diagnostics chord names.
pub(super) const KEY_P: u32 = 0x0070;

/// `FcitxKey_P`, the uppercase shape a chord containing `Shift` may deliver.
pub(super) const KEY_P_UPPER: u32 = 0x0050;

/// The `domain/action/reason` code a chord reports while nothing draws the panel it asked for.
///
/// Stable, like every other code in the project: diagnostics and tests match on it. The
/// caller renders it with the panel's own name ([`Overlay::not_implemented_code`]), so one
/// line says which shortcut was pressed and that nothing happened behind it.
pub const UI_NOT_IMPLEMENTED_CODE: &str = "ui/not-implemented";

/// A modal overlay: a panel that owns the keyboard while it is open.
///
/// The three the design names. Which one is open changes nothing about *whether* a key is the
/// overlay's — the panel key domain is the same for all three — so the bus keeps the identity
/// for the layer that draws the panel rather than branching on it.
///
/// Two of the three are asked for by a chord; the cheat sheet is asked for by the
/// held-modifier gesture, which is why the enum is not simply a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// The keyboard cheat sheet.
    CheatSheet,
    /// The command palette.
    CommandPalette,
    /// The diagnostics panel.
    Diagnostics,
}

impl Overlay {
    /// The panel's name, as a diagnostic spells it.
    ///
    /// # Returns
    ///
    /// A lowercase, hyphenated name — `cheat-sheet`, `command-palette`, `diagnostics` — stable
    /// enough to match on and free of anything the user typed. It is the reason half of the
    /// code [`Overlay::not_implemented_code`] renders.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::CheatSheet => "cheat-sheet",
            Self::CommandPalette => "command-palette",
            Self::Diagnostics => "diagnostics",
        }
    }

    /// The diagnostic a chord reports while nothing draws this panel.
    ///
    /// # Returns
    ///
    /// [`UI_NOT_IMPLEMENTED_CODE`] with the panel's [`Overlay::name`] appended: one line that
    /// names the shortcut's target and says nothing came of it, which is what a key taken with
    /// no effect behind it has to say.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::engine::Overlay;
    ///
    /// assert_eq!(
    ///     Overlay::Diagnostics.not_implemented_code(),
    ///     "ui/not-implemented: diagnostics"
    /// );
    /// ```
    pub fn not_implemented_code(&self) -> String {
        format!("{UI_NOT_IMPLEMENTED_CODE}: {}", self.name())
    }
}

/// The panel one key asks for, when the plugin may open one.
///
/// # Arguments
///
/// * `event` — the key as the host delivered it.
/// * `session` — what the bus may read about the session of the input context the key arrived
///   in.
///
/// # Returns
///
/// The panel the chord names, or `None` for every other key. The modifier set is compared for
/// equality, like every other chord: `Ctrl+Shift+Alt+/` is a different key and belongs to the
/// desktop environment.
///
/// Two conditions beyond the chord itself, and both are about where the plugin is live. A
/// context with no session is one the plugin was never activated in, and a panel is not
/// something to open there; temporary English hands every key to the application, the chords
/// included, because that is the whole meaning of the mode.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn chord(event: &KeyEvent, session: &SessionView<'_>) -> Option<Overlay> {
    // The key first: the modifier set is one comparison, and it rejects almost every event
    // before anything at all is asked of the session.
    if (event.state & MODIFIER_MASK) != (CTRL | SHIFT) {
        return None;
    }
    let panel = match event.sym {
        KEY_SLASH | KEY_QUESTION => Overlay::CommandPalette,
        KEY_P | KEY_P_UPPER => Overlay::Diagnostics,
        _ => return None,
    };
    if session.is_absent() || session.temp_english() {
        return None;
    }
    Some(panel)
}

impl Dispatcher {
    /// The overlay that owns the keyboard, if one is open.
    ///
    /// # Returns
    ///
    /// The open overlay, or `None` when the keyboard belongs to the layers below.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    /// Gives the keyboard to a modal overlay.
    ///
    /// The layer that draws the panel is the one that opens it, and it is also the one that
    /// executes what the overlay layer decides: the bus only answers whether a key is the
    /// panel's.
    ///
    /// # Arguments
    ///
    /// * `overlay` — the panel that is now open.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn open_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
    }

    /// Takes the keyboard back from the overlay, if one is open.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    /// Opens a panel a chord asked for, and records the request.
    ///
    /// The chord path, as opposed to [`Dispatcher::open_overlay`], which is the panel layer
    /// opening what it has drawn. A chord takes a key from the application while there is
    /// nothing behind it yet, and that fact is what the request carries to the caller.
    ///
    /// # Arguments
    ///
    /// * `overlay` — the panel the chord named.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn open_panel(&mut self, overlay: Overlay) {
        self.open_overlay(overlay);
        self.pending_overlay = Some(overlay);
    }

    /// Takes the panel the last chord asked for, if one did.
    ///
    /// # Returns
    ///
    /// The panel a chord named, or `None` when no chord has been answered since the last call.
    /// Taking rather than reading is what keeps one request from being acted on twice, and a
    /// second chord before the first request is taken replaces it: the panel the user asked
    /// for last is the one worth opening.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn take_overlay_request(&mut self) -> Option<Overlay> {
        self.pending_overlay.take()
    }
}
