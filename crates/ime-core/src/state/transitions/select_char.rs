//! The character-selection transitions: taking one character of the highlighted
//! candidate, in a composition and in an association view.
//!
//! Responsibility: the two `SelectFirstChar`/`SelectLastChar` rows of the
//! transition table. In a composition the character itself is committed by the
//! host layer -- which splits the candidate's text and asks for this step after
//! -- and what this module owns is the input the character was spelled with:
//! one syllable of it leaves the buffer and the composition keeps running. In
//! an association view there is no input to keep, so the character leaves
//! through the ordinary commit door and the session ends the way a whole-hit
//! selection ends.
//!
//! Boundaries: nothing here touches the host, and nothing here commits on the
//! composition path -- the commit is the caller's, gated on this step having
//! acted, so a guard that fails here hands the key back instead of leaving the
//! text on screen and the input behind it telling different stories.

use smallvec::SmallVec;

use ime_types::{CandidateSource, HideReason, SchemeId};

use crate::segment::HINT_INLINE_BOUNDARIES;
use crate::state::boundaries::map_to_raw;
use crate::state::machine::{Ctx, Effect, Session, SessionState};

impl Session {
    /// Takes one character of the highlighted candidate: `first` selects the
    /// leading character and `false` the trailing one.
    ///
    /// The character itself is committed by the host layer, which asked for this
    /// step after splitting the candidate's text; this transition owns the input
    /// the character was spelled with. One syllable of it leaves the buffer --
    /// the pinyin reading of one character is one syllable, so the leading
    /// character owns the first syllable and the trailing one the last syllable
    /// the candidate consumed -- the session stays [`SessionState::Composing`],
    /// and the window is repainted with what is left: `nihao` reading `你好`
    /// keeps `hao` after the leading `你` goes, and the input right of the
    /// removed syllable is left exactly as it was. When nothing is left the
    /// composition ends the way the last Backspace ends it. A guard that fails
    /// produces nothing, which is what hands the key back to the application.
    pub(super) fn select_char(&mut self, first: bool, ctx: &mut Ctx<'_>) {
        let Some((start, end)) = self.char_range(first) else {
            return;
        };
        let raw = String::from(self.buf.raw());
        let mut keep = String::with_capacity(raw.len());
        keep.push_str(&raw[..start]);
        keep.push_str(&raw[end..]);
        if keep.is_empty() {
            // The character was the whole reading, so nothing is left to
            // compose: the same end the last Backspace takes.
            self.end_session(HideReason::EmptyInput, ctx);
            return;
        }
        self.retype_input(&keep);
        self.refresh(ctx.env);
        self.emit_update(ctx);
    }

    /// Takes one character of the highlighted association hit's commit text.
    ///
    /// A hit's display text and its insert text can differ, so the character is
    /// taken from the commit text -- the part that reaches the application when
    /// the whole hit is chosen. A view holds no input, so there is nothing to
    /// keep composing: the character leaves through the ordinary commit door
    /// and the session ends the way a whole-hit selection ends, recording
    /// nothing -- an association selection is continuation text, not evidence
    /// about a spelling.
    pub(super) fn select_assoc_char(&mut self, first: bool, ctx: &mut Ctx<'_>) {
        let index = usize::from(self.paging.highlight);
        // The character is copied out before the view is touched: the borrow of
        // the hit ends here, and the mutation below cannot overlap it.
        let char_text = self
            .assoc_view
            .as_ref()
            .and_then(|view| view.hits.get(index))
            .and_then(|hit| split_char(hit.commit.as_str(), first))
            .map(String::from);
        let Some(char_text) = char_text else {
            return;
        };
        self.clear_assoc_view();
        self.clear_input();
        self.state = SessionState::Committing;
        ctx.push(Effect::Commit(char_text));
    }

    /// Returns the `(start, end)` byte range of the input the selected character
    /// was spelled with, or `None` when the selection cannot act.
    ///
    /// The guards mirror the host layer's, and one is stricter on purpose: a
    /// double-pinyin input aligns keystrokes and syllables through the layout's
    /// own grid, which this transition cannot reach, so it is left alone rather
    /// than trimmed by a guess. The syllable grid is the one the last refresh
    /// derived from the graph, recovered through the same mapping the refresh
    /// used, so a guard here can only disagree with the host's where the host
    /// would hand the key back anyway.
    fn char_range(&self, first: bool) -> Option<(usize, usize)> {
        let candidate = self.highlighted_candidate()?;
        if self.symbol_mode || !is_selectable_source(candidate.source) {
            return None;
        }
        if self.request().scheme != SchemeId::FULL {
            return None;
        }
        let raw = self.buf.raw();
        let mut hint = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        if !self.dag().best_segmentation_hint(&mut hint) {
            return None;
        }
        let mut grid = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        if !map_to_raw(raw, &hint, &mut grid) {
            return None;
        }
        let total = grid.len().saturating_sub(1);
        let last = usize::from(candidate.consumed_syllables).clamp(1, total);
        let (start, end) = if first { (0, 1) } else { (last - 1, last) };
        Some((usize::from(grid[start]), usize::from(grid[end])))
    }

    /// Replaces the input with `keep`, re-typed through the buffer's own push
    /// path so the caret ends where a keystroke would leave it and the syllable
    /// grid is repaired the same way.
    ///
    /// The remainder is a slice of what the buffer itself held, so every push
    /// succeeds; the loop stops anyway rather than resting on that, and the
    /// refresh that follows ranks whatever the buffer then holds.
    fn retype_input(&mut self, keep: &str) {
        self.buf.clear();
        for ch in keep.chars() {
            if self.buf.push_char(ch).is_err() {
                break;
            }
        }
    }
}

/// Whether a candidate is a reading a character can be taken from.
///
/// Panel candidates are symbols -- already single characters, chosen rather
/// than spelled -- and pass-through and English ones are not pinyin readings,
/// so none of them has a syllable to give up. The host layer keeps its own twin
/// of this predicate beside the commit it stages; the two answer identically,
/// and each side's tests pin its own.
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

/// Returns the first or last character of `text`, or `None` when it is empty.
///
/// Both ends of a UTF-8 string are character boundaries, so either answer is
/// one slice, and the work is the edge character's length alone. The unit is
/// the scalar value, which is what a multibyte character and an emoji tail are
/// made of. The host layer keeps its own twin of this helper beside the commit
/// it stages, and the two answer identically.
fn split_char(text: &str, first: bool) -> Option<&str> {
    if first {
        let ch = text.chars().next()?;
        Some(&text[..ch.len_utf8()])
    } else {
        let ch = text.chars().next_back()?;
        Some(&text[text.len() - ch.len_utf8()..])
    }
}

#[cfg(test)]
mod tests {
    use ime_types::{CandidateSource, HideReason, KeyAction, SchemeId};

    use super::split_char;
    use crate::assoc::AssocSourceKind;
    use crate::state::machine::Session;
    use crate::state::tests::assoc::AssocProviderStub;
    use crate::state::tests::{Fixture, committed, composing, hidden, index_of, kinds};
    use crate::state::{
        AssocSessionConfig, SessionConfig, SessionEnv, SessionEvent, SessionState, step,
    };

    /// The association configuration the view tests run under.
    fn cfg_assoc() -> SessionConfig {
        SessionConfig {
            assoc: AssocSessionConfig {
                enabled: true,
                max_candidates: 5,
                history_depth: 8,
            },
            ..SessionConfig::default()
        }
    }

    /// Lands a commit of the top candidate and lets the association view open,
    /// the way a real session reaches the view: the host confirms the commit.
    fn with_assoc_view(env: &SessionEnv<'_>) -> Session {
        let cfg = cfg_assoc();
        let mut session = composing(&cfg, env, "ni");
        assert_eq!(session.decoded().candidates[0].text, "你");
        let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, env);
        let _ = step(&mut session, SessionEvent::CommitDone, &cfg, env);
        assert_eq!(session.state, SessionState::Composing, "the view opened");
        session
    }

    #[test]
    fn test_split_char_takes_the_first_scalar_of_ascii_multibyte_and_emoji() {
        assert_eq!(split_char("abc", true), Some("a"));
        assert_eq!(split_char("你好", true), Some("你"));
        assert_eq!(split_char("你好😊", true), Some("你"));
    }

    #[test]
    fn test_split_char_takes_the_last_scalar_of_ascii_multibyte_and_emoji() {
        assert_eq!(split_char("abc", false), Some("c"));
        assert_eq!(split_char("你好", false), Some("好"));
        // The emoji tail is one scalar value; a family ZWJ sequence splits at
        // its last scalar, which is the unit this contract names.
        assert_eq!(split_char("你好😊", false), Some("😊"));
        assert_eq!(split_char("你好🧑‍🚀", false), Some("🚀"));
    }

    #[test]
    fn test_split_char_rejects_an_empty_text_on_both_ends() {
        assert_eq!(split_char("", true), None);
        assert_eq!(split_char("", false), None);
    }

    #[test]
    fn test_select_char_first_takes_the_leading_syllable_and_stays_composing() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "nihao");
        assert_eq!(session.decoded().candidates[0].text, "你好");

        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        // The commit is the host's, so no commit effect leaves the step: what
        // leaves is the repaint of what remains.
        assert_eq!(
            kinds(&effects),
            ["update-preedit", "send-frame"],
            "the step repaints, the host commits"
        );
        assert_eq!(session.buf.raw(), "hao", "the first syllable left");
        assert_eq!(session.state, SessionState::Composing);
        assert_eq!(committed(&effects), None);
    }

    #[test]
    fn test_select_char_last_takes_the_trailing_syllable_and_stays_composing() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "nihao");
        assert_eq!(session.decoded().candidates[0].text, "你好");

        let effects = session.handle_key(KeyAction::SelectLastChar, &cfg, &env);

        assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
        assert_eq!(session.buf.raw(), "ni", "the last syllable left");
        assert_eq!(session.state, SessionState::Composing);
    }

    #[test]
    fn test_select_char_keeps_the_input_right_of_the_removed_syllable() {
        // `nihaoma` read as 你好 plus a syllable still to come: taking the last
        // character of the prefix candidate removes its own last syllable and
        // leaves the unprocessed `ma` in place, exactly where it was.
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[
            ("ni", "你"),
            ("hao", "好"),
            ("ni'hao", "你好"),
            ("ma", "吗"),
        ]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "nihaoma");
        let prefix = index_of(&session, "你好").expect("the prefix candidate is offered");
        session.paging.highlight = prefix;

        let effects = session.handle_key(KeyAction::SelectLastChar, &cfg, &env);

        assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
        assert_eq!(session.buf.raw(), "nima", "the right side stayed");
        assert_eq!(session.state, SessionState::Composing);
    }

    #[test]
    fn test_select_char_on_a_whole_reading_single_syllable_ends_the_composition() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[("ni", "你")]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        assert_eq!(session.decoded().candidates[0].text, "你");

        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        // Nothing is left to compose, so the session ends the way the last
        // Backspace ends it: preedit cleared, window hidden.
        assert_eq!(kinds(&effects), ["set-client-preedit", "hide"]);
        assert_eq!(hidden(&effects), Some(HideReason::EmptyInput));
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_select_char_without_a_candidate_is_inert() {
        // The idle arm has no highlight to act on and produces nothing, which is
        // what hands the key back to the application.
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[("ni", "你")]);
        let env = fixture.env();
        let mut session = Session::new();

        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        assert!(effects.is_empty(), "nothing to act on, nothing produced");
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_select_char_on_a_passthrough_candidate_is_inert() {
        // An input no reading cuts degrades to a pass-through candidate, which
        // is not a pinyin reading: the selection has no syllable to give up and
        // produces nothing.
        let cfg = SessionConfig::default();
        let fixture = Fixture::with_dictionary(&[]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "vvv");
        assert_eq!(
            session.decoded().candidates[0].source,
            CandidateSource::Passthrough,
            "the degraded list offers the raw input"
        );

        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        assert!(
            effects.is_empty(),
            "a pass-through candidate answers nothing"
        );
        assert_eq!(session.buf.raw(), "vvv", "the input is untouched");
    }

    #[test]
    fn test_select_char_under_a_double_pinyin_scheme_is_inert() {
        // The layout's own grid is what aligns its keystrokes with syllables,
        // and this transition cannot reach it, so the selection is left alone
        // rather than trimming by a guess.
        let cfg = SessionConfig {
            scheme: SchemeId::XIAOHE,
            ..SessionConfig::default()
        };
        let fixture = Fixture::with_dictionary(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "nihao");
        assert_eq!(session.request().scheme, SchemeId::XIAOHE);

        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        assert!(effects.is_empty(), "a layout input is not trimmed here");
        assert_eq!(session.buf.raw(), "nihao", "the input is untouched");
    }

    #[test]
    fn test_select_assoc_char_commits_the_first_character_of_the_commit_text() {
        let fixture = Fixture::with_dictionary(&[("ni", "你")]);
        let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("陈列室", "陈列")]);
        let env = fixture.env_assoc(&provider, false);
        let mut session = with_assoc_view(&env);
        assert_eq!(session.decoded().candidates[0].text, "陈列室");

        let cfg = cfg_assoc();
        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        // The character comes from the commit text, not the display text: the
        // display is 陈列室 and the insert is 陈列, so the leading character is
        // 陈 either way -- the trailing one is what tells the two apart.
        assert_eq!(committed(&effects), Some("陈"));
        assert_eq!(session.state, SessionState::Committing);
        assert!(!session.is_assoc_active(), "the view closed");
    }

    #[test]
    fn test_select_assoc_char_commits_the_last_character_of_the_commit_text() {
        let fixture = Fixture::with_dictionary(&[("ni", "你")]);
        let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("陈列室", "陈列")]);
        let env = fixture.env_assoc(&provider, false);
        let mut session = with_assoc_view(&env);
        assert_eq!(session.decoded().candidates[0].text, "陈列室");

        let cfg = cfg_assoc();
        let effects = session.handle_key(KeyAction::SelectLastChar, &cfg, &env);

        // The trailing character of the *commit* text is 列, which is what
        // distinguishes the commit text from the display text here.
        assert_eq!(committed(&effects), Some("列"));
        assert_eq!(session.state, SessionState::Committing);
    }

    #[test]
    fn test_select_assoc_char_with_a_highlight_past_the_list_is_inert() {
        let fixture = Fixture::with_dictionary(&[("ni", "你")]);
        let provider = AssocProviderStub::new(AssocSourceKind::Bigram, &[("陈列室", "陈列")]);
        let env = fixture.env_assoc(&provider, false);
        let mut session = with_assoc_view(&env);
        session.paging.highlight = 99;

        let cfg = cfg_assoc();
        let effects = session.handle_key(KeyAction::SelectFirstChar, &cfg, &env);

        assert!(
            effects.is_empty(),
            "a highlight past the list answers nothing"
        );
        assert_eq!(session.state, SessionState::Composing, "nothing closed");
    }
}
