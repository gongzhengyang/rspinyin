//! What a step asks the host to do, and the vocabulary the frame machinery shares.
//!
//! Responsibility: the effect list a [`crate::state::machine::step`] returns, the anchor
//! hint a show carries, and the inline capacity the list is sized to.
//!
//! Boundaries: pure declarations. Nothing here reads a session, a file or a clock, and
//! nothing here decides anything -- the machine decides, this module names.

use smallvec::SmallVec;

use ime_types::{ImeError, Placement, Preedit, Revision, UiFrame};

/// Most effects one [`step`] produces.
///
/// The returned `SmallVec` holds this many inline, so a step never allocates for its
/// effects. A step that puts a session on screen emits three -- the preedit, the
/// show and the frame -- and a step that also reports what a scheme rewrite refused
/// emits the fourth; no path in this module emits more.
pub const MAX_EFFECTS: usize = 4;

/// The set of effects one step produces.
pub type Effects = SmallVec<[Effect; MAX_EFFECTS]>;

/// What the host must do after a step.
///
/// The machine never does any of this itself: it describes the work so that the
/// caller can execute it on the thread that owns the channel, and so that a test can
/// assert on it without a host.
#[derive(Debug)]
pub enum Effect {
    /// Replace the composing text. The executor applies the client-preedit policy:
    /// with it on, the text goes to the application's preedit area, and with it off
    /// the area is cleared.
    UpdatePreedit(Preedit),
    /// Show the window with a new frame.
    SendFrame(Box<UiFrame>),
    /// Show the window, with the anchor the host resolves from [`AnchorHint`].
    Show(AnchorHint),
    /// Hide the window and say why.
    Hide(ime_types::HideReason),
    /// Commit this text to the application.
    Commit(String),
    /// Record one commit in the user's frequencies.
    RecordUserFreq {
        /// The word the user chose.
        key: String,
        /// How strongly to weigh it.
        weight_hint: u16,
    },
    /// Drop one word from the user's learned frequencies.
    ///
    /// The session has already taken the word out of the candidate list it is showing: the
    /// user must see it go on the keystroke, and the store cannot answer until this effect
    /// has run. What is left is the removal itself, which belongs to the layer that owns the
    /// store -- the session touches no file (0.4 rule 4). That layer batches the removal the
    /// way it batches a record, so executing this effect does not put a write transaction on
    /// the thread that handles the keystroke (`ASM-04`, `ASM-20`).
    ForgetUserWord {
        /// The word the user asked to forget: the text of the highlighted candidate, which
        /// is the key its frequency was recorded under.
        key: String,
    },
    /// Save the highlighted candidate as a phrase the user defined on purpose.
    ///
    /// The session holds no phrase table and writes no file, so the pair leaves as an
    /// effect and the host layer appends it to the user's phrase document, where the
    /// next load of that document picks it up. The composition is not touched: the
    /// candidate list and the preedit stay exactly as they were, and nothing is
    /// re-sent to the window.
    AddPhrase {
        /// The input the user typed, folded to the lower-case key alphabet of a phrase.
        key: String,
        /// The text of the candidate the highlight was on, which becomes the phrase.
        text: String,
    },
    /// Report a diagnostic.
    ///
    /// The payload is built from the stable `domain/action/reason` codes and from
    /// lengths and counts; it never carries the characters the user typed, which is
    /// what keeps a diagnostic log out of the user's input.
    Diagnose(ImeError),
    /// Set or clear the application's preedit area directly.
    ///
    /// This module emits it only to clear the area when a composition ends; the
    /// engine also uses it when a mode change outside a composition invalidates what
    /// the area shows.
    SetClientPreedit(Option<(String, u32)>),
}

/// Where the window should appear, as far as the session can say.
///
/// The session does not know where the caret is -- that is the host's -- so it asks
/// for a side of the cursor and leaves the rectangle to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorHint {
    /// Revision of the frame this window shows, so that a show and the frame it
    /// belongs to can be matched up.
    pub revision: Revision,
    /// Requested side of the cursor.
    pub placement: Placement,
}
