//! The character-selection route: what the composing marks the configuration names
//! do when the highlight sits on a candidate a character can be taken from.
//!
//! # Responsibility
//!
//! The two marks `keys.select_first_char` and `keys.select_last_char` name -- the
//! brackets of the shipped document -- reach the router as input characters: they are
//! marks of the punctuation row, and with nothing composing the passthrough policy
//! answers them exactly as it always has. While a composition is live, this module
//! upgrades the configured marks to [`KeyAction::SelectFirstChar`] and
//! [`KeyAction::SelectLastChar`] when the highlight sits on a candidate the
//! session can take a character from, steps the session with the action, and --
//! once the step has trimmed the input the character was spelled with -- stages
//! the commit of the character itself. Splitting is a [`char`] slice at either
//! end of the candidate's UTF-8 text, so it is constant time and cannot panic;
//! a candidate without a character at the asked-for end is not a selection, and
//! the key then keeps the old answer every mark has: it travels back to the
//! application. `Off` on a key leaves its mark to that old answer, so the gesture
//! can be taken away one half at a time.
//!
//! The association view answers through its own transition (the character is
//! taken from a hit's commit text, which only the session reads), so a
//! selection staged here carries no commit of its own for that case.
//!
//! # Boundary
//!
//! Plain Rust over the [`Host`] trait: no host object, no file, no clock, no
//! global state. The guards here mirror the session transition's; either side
//! failing answers "not ours", which is the safe direction -- a commit is only
//! staged after the session reported that it trimmed something.

use ime_config::keymap::KeyBindings;
use ime_core::state::{Session, SessionEvent, SessionState};
use ime_types::{CandidateSource, KeyAction};

use crate::engine::host::Host;

use super::StepCtx;
use super::context::Context;

/// Which end of the highlighted candidate a key takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    /// The first character, which the mark `keys.select_first_char` names asks for.
    First,
    /// The last character, which the mark `keys.select_last_char` names asks for.
    Last,
}

/// What a staged selection still waits for after the session has stepped.
#[derive(Debug, PartialEq)]
pub(super) enum Staged {
    /// The host commits `text` -- the character the candidate was split to.
    /// The session's step trimmed the input the character consumed and repainted
    /// the window; this is the commit that closes the gesture.
    Host(String),
    /// The session commits through its own effect, which is the association
    /// route's shape: the view holds no input to keep composing, so its
    /// transition pushes the commit itself.
    Session,
}

/// Returns the character of `text` at `which` end, or `None` for an empty one.
///
/// Both ends of a UTF-8 string are character boundaries, so either answer is
/// one slice and the work is the edge character's length alone -- constant
/// time. The unit is the scalar value, which is what a multibyte character and
/// an emoji tail are made of. The session layer keeps its own twin of this
/// helper beside the trimming it does; the two answer identically.
fn char_at_boundaries(text: &str, which: Which) -> Option<&str> {
    match which {
        Which::First => {
            let ch = text.chars().next()?;
            Some(&text[..ch.len_utf8()])
        }
        Which::Last => {
            let ch = text.chars().next_back()?;
            Some(&text[text.len() - ch.len_utf8()..])
        }
    }
}

/// Which end of the highlighted candidate the mark asks for, as the binding table
/// in force answers it.
///
/// The first-character key is asked before the last-character one, which is also the
/// order the projection settles a conflict in: a mark both keys name is resolved to
/// the first-character gesture there, so the second arm is the tiebreak the table can
/// no longer reach.
///
/// # Panics
///
/// Never.
fn which_of(keys: &KeyBindings, ch: char) -> Option<Which> {
    if keys.select_first_char.mark() == Some(ch) {
        Some(Which::First)
    } else if keys.select_last_char.mark() == Some(ch) {
        Some(Which::Last)
    } else {
        None
    }
}

/// Returns the action `which` is routed as.
fn action_of(which: Which) -> KeyAction {
    match which {
        Which::First => KeyAction::SelectFirstChar,
        Which::Last => KeyAction::SelectLastChar,
    }
}

/// Whether a candidate is a reading a character can be taken from.
///
/// Panel candidates are symbols -- already single characters, chosen rather
/// than spelled -- and pass-through and English ones are not pinyin readings,
/// so none of them has a syllable to give up. The session transition keeps its
/// own twin of this predicate beside the trimming it does; the two answer
/// identically, and each side's tests pin its own.
fn is_selectable_source(source: CandidateSource) -> bool {
    matches!(
        source,
        CandidateSource::Dict
            | CandidateSource::UserDict
            | CandidateSource::Learned
            | CandidateSource::Phrase
            | CandidateSource::Script
    )
}

/// Returns what answering `ch` as a character selection would take, or `None`
/// when the mark is not a selection here.
///
/// `None` is the old answer -- the mark belongs to the application, exactly as
/// it did before this route existed -- and it covers every guard: a session
/// that is not composing, a mark the configuration does not name, no highlight, a
/// candidate that is not a reading, and an empty text. An association candidate
/// stages the session's own commit, because the character is taken from the hit's
/// commit text there.
fn prepare(session: &Session, keys: &KeyBindings, ch: char) -> Option<(KeyAction, Staged)> {
    let which = which_of(keys, ch)?;
    if session.state != SessionState::Composing {
        return None;
    }
    let candidate = session.highlighted_candidate()?;
    if candidate.source == CandidateSource::Assoc {
        return Some((action_of(which), Staged::Session));
    }
    if !is_selectable_source(candidate.source) {
        return None;
    }
    let text = char_at_boundaries(&candidate.text, which)?;
    Some((action_of(which), Staged::Host(String::from(text))))
}

/// Answers a composing mark that may be a character selection.
///
/// The mark is staged, the session is stepped with the action -- which trims
/// the input the character was spelled with and repaints the window -- and the
/// commit of the character itself is handed to the host only after the step
/// reported that it acted. A step that produced nothing changed no input, and
/// committing over it would put the character on screen behind an input that
/// still spells the whole candidate, so the answer is `Some(false)`: the key is
/// handed back, the way a mark the guards refuse is.
///
/// # Arguments
///
/// * `ctx` — the context the frame context and the output mode bits are read
///   through; the commit leaves through the same rewrite an ordinary commit
///   takes, minus the hide, because the composition continues.
/// * `step` — the step context the event is stepped with.
/// * `ic` — the host's identity for the input context.
/// * `ch` — the mark the punctuation row delivered.
/// * `host` — the boundary the commit is executed against.
///
/// # Returns
///
/// `None` when the mark is not a selection here -- the caller keeps the old
/// answer and the key travels back to the application. `Some(acted)` otherwise,
/// reporting whether the route did anything the plugin keeps the key for.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn answer(
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    ic: u64,
    ch: char,
    host: &mut dyn Host,
) -> Option<bool> {
    let (action, staged) = prepare(&ctx.session, &step.config.keys, ch)?;
    // The frame context is written for the same reason the ordinary key path
    // writes it: the step's repaint should carry the mode bits as they stand.
    ctx.write_frame_context(step.config);
    let acted = ctx.run(SessionEvent::Key(action), step, host);
    if acted {
        if let Staged::Host(text) = staged {
            // The one rewrite every commit takes: the full-width, punctuation
            // and script switches answer before the host sees the text. The
            // composition continues, so there is no hide beside it.
            let text = ctx.modes.transform_output(&text, ctx.script.index());
            host.commit(ic, &text);
        }
    }
    Some(acted)
}

#[cfg(test)]
mod tests {
    use ime_config::schema::SelectCharKey;

    use super::{KeyBindings, Which, char_at_boundaries, prepare, which_of};

    #[test]
    fn test_char_at_boundaries_takes_the_first_scalar_of_each_shape() {
        assert_eq!(char_at_boundaries("abc", Which::First), Some("a"));
        assert_eq!(char_at_boundaries("你好", Which::First), Some("你"));
        assert_eq!(char_at_boundaries("你好😊", Which::First), Some("你"));
    }

    #[test]
    fn test_char_at_boundaries_takes_the_last_scalar_of_each_shape() {
        assert_eq!(char_at_boundaries("abc", Which::Last), Some("c"));
        assert_eq!(char_at_boundaries("你好", Which::Last), Some("好"));
        // The emoji tail is one scalar value, which is the unit this split names.
        assert_eq!(char_at_boundaries("你好😊", Which::Last), Some("😊"));
    }

    #[test]
    fn test_char_at_boundaries_rejects_an_empty_text_on_both_ends() {
        assert_eq!(char_at_boundaries("", Which::First), None);
        assert_eq!(char_at_boundaries("", Which::Last), None);
    }

    #[test]
    fn test_which_of_names_the_configured_marks_on_both_ends() {
        let shipped = KeyBindings::default();
        assert_eq!(which_of(&shipped, '['), Some(Which::First));
        assert_eq!(which_of(&shipped, ']'), Some(Which::Last));

        let custom = KeyBindings {
            select_first_char: SelectCharKey::Comma,
            select_last_char: SelectCharKey::Period,
            ..KeyBindings::default()
        };
        assert_eq!(which_of(&custom, ','), Some(Which::First));
        assert_eq!(which_of(&custom, '.'), Some(Which::Last));
        assert_eq!(
            which_of(&custom, '['),
            None,
            "a mark the configuration no longer names keeps the old answer"
        );
    }

    #[test]
    fn test_which_of_answers_nothing_for_an_off_key_or_an_untouched_mark() {
        let keys = KeyBindings {
            select_first_char: SelectCharKey::Off,
            select_last_char: SelectCharKey::Off,
            ..KeyBindings::default()
        };
        assert_eq!(which_of(&keys, '['), None);
        assert_eq!(which_of(&keys, ']'), None);
        assert_eq!(
            which_of(&keys, ','),
            None,
            "a mark no key names is not a selection"
        );
    }

    #[test]
    fn test_prepare_rejects_an_idle_session_and_an_unrouted_mark() {
        let session = ime_core::state::Session::new();
        let keys = KeyBindings::default();
        assert_eq!(prepare(&session, &keys, '['), None, "nothing is composing");
        assert_eq!(prepare(&session, &keys, ']'), None);
        assert_eq!(
            prepare(&session, &keys, ','),
            None,
            "only the configured marks are routed here"
        );
    }
}
