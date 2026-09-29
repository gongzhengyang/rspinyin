//! Keyboard-to-semantics mapping.
//!
//! `KeyAction` is the only thing the session state machine consumes: the host
//! layer translates raw key events into these actions, and the state machine
//! never sees a keysym, a modifier mask or a host type. Freezing this enum is
//! what lets the key-routing layer and the session state machine be developed in
//! parallel.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

/// A semantic key action.
///
/// Variants carry payloads only where the payload is part of the action itself
/// (`InputChar`, `SelectIndex`, `MoveHighlight`, `MoveCaret`); everything else is
/// resolved from configuration at translation time, so the state machine never
/// performs a configuration lookup to decide what a key means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Insert one printable ASCII letter into the input buffer.
    InputChar(char),
    /// Delete the trailing syllable, or one character when the caret sits inside
    /// the buffer.
    Backspace,
    /// Commit the highlighted candidate.
    CommitHighlighted,
    /// Commit the raw input string instead of a candidate.
    CommitRaw,
    /// Commit the candidate displayed under the given digit, 1-based.
    SelectIndex(u8),
    /// Show the next page of candidates.
    PageNext,
    /// Show the previous page of candidates.
    PagePrev,
    /// Move the highlight by the given number of candidates.
    MoveHighlight(i8),
    /// Move the preedit caret by the given number of syllables.
    MoveCaret(i8),
    /// Toggle the Chinese / English input mode.
    ToggleLang,
    /// Toggle full-width output for characters this plugin commits.
    ToggleFullWidth,
    /// Toggle Chinese / English punctuation mode.
    TogglePunct,
    /// Enter temporary English mode, where every key passes through to the host.
    EnterTempEnglish,
    /// Cancel the current session without committing anything.
    Escape,
    /// Let the host handle the key; the plugin consumes nothing.
    Ignore,
    /// Switch the committed text between simplified and traditional Chinese.
    ///
    /// Appended by ADR-0005. The session is deliberately *not* reset: the
    /// transition table keeps a composing session composing and only recomputes
    /// the displayed text, because losing what the user has typed to a display
    /// toggle is the behaviour 0.4 rule 10 forbids.
    ToggleScript,
    /// Drop the highlighted word from the user's learned frequencies.
    ForgetHighlighted,
    /// Keep the highlighted word: exempt it from the frequency decay and eviction
    /// sweep that would otherwise eventually drop it.
    PinHighlighted,
    /// Save the highlighted candidate as a phrase the user typed on purpose.
    AddPhrase,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exhaustively names every variant, so adding or removing one breaks the
    // build instead of silently changing the frozen contract.
    fn label(action: KeyAction) -> &'static str {
        match action {
            KeyAction::InputChar(_) => "input-char",
            KeyAction::Backspace => "backspace",
            KeyAction::CommitHighlighted => "commit-highlighted",
            KeyAction::CommitRaw => "commit-raw",
            KeyAction::SelectIndex(_) => "select-index",
            KeyAction::PageNext => "page-next",
            KeyAction::PagePrev => "page-prev",
            KeyAction::MoveHighlight(_) => "move-highlight",
            KeyAction::MoveCaret(_) => "move-caret",
            KeyAction::ToggleLang => "toggle-lang",
            KeyAction::ToggleFullWidth => "toggle-full-width",
            KeyAction::TogglePunct => "toggle-punct",
            KeyAction::EnterTempEnglish => "enter-temp-english",
            KeyAction::Escape => "escape",
            KeyAction::Ignore => "ignore",
            KeyAction::ToggleScript => "toggle-script",
            KeyAction::ForgetHighlighted => "forget-highlighted",
            KeyAction::PinHighlighted => "pin-highlighted",
            KeyAction::AddPhrase => "add-phrase",
        }
    }

    #[test]
    fn test_key_action_payload_variants_round_trip() {
        assert_eq!(label(KeyAction::InputChar('n')), "input-char");
        assert_eq!(label(KeyAction::SelectIndex(9)), "select-index");
        assert_eq!(label(KeyAction::MoveHighlight(-1)), "move-highlight");
        assert_eq!(label(KeyAction::MoveCaret(1)), "move-caret");
    }

    #[test]
    fn test_key_action_unit_variants_are_distinct() {
        assert_ne!(KeyAction::PageNext, KeyAction::PagePrev);
        assert_ne!(KeyAction::Ignore, KeyAction::Escape);
        assert_eq!(KeyAction::Backspace, KeyAction::Backspace);
    }
}
