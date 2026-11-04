//! The negative-feedback transitions: penalizing the highlighted candidate's user
//! frequency, and hiding it for the rest of the session.
//!
//! Responsibility: the `ReduceFrequency` and `HideCandidate` rows of the transition
//! table, and the filter that keeps a hidden candidate out of every list the session
//! decodes afterwards. The two actions answer the same highlight with different
//! questions -- trust and presence -- and the shapes of their transitions differ the
//! way the answers do: the penalty leaves the list exactly as it is, because the rank
//! moves only on the re-decode the host owes the session once the store has taken the
//! receipt; the hide takes the candidate out here, on the keystroke, because the
//! suppression is the session's own memory and needs no store at all.
//!
//! Boundaries: nothing here touches the host or the store. The penalty leaves as an
//! [`Effect::ReduceUserFreq`] receipt -- the session touches no file (0.4 rule 4), and
//! it does not read the store either, so it cannot know whether the word was ever
//! learned; the host reports the not-found diagnostic from the store's own answer. The
//! hide writes the pair into [`Session::hidden`], which dies with the session: hiding
//! across sessions is the forget action's job, and the two do not duplicate one
//! mechanism.
//!
//! # The filter
//!
//! [`Session::filter_hidden_candidates`] runs at the end of every decode-bearing
//! refresh -- the per-keystroke one and the paging expansion alike -- so a hidden
//! candidate can return through neither. The empty set costs one branch, which is the
//! whole cost for a session that never used the action; a non-empty one is scanned
//! rather than hashed, because the set is a handful of pairs and a hash of borrowed
//! `(str, str)` keys is not something a `HashSet<(Box<str>, Box<str>)>` can serve
//! without allocating on the hot path.

use std::collections::HashSet;

use crate::state::machine::{Ctx, Effect, Session};
use crate::state::outcome::renumber;
use crate::viterbi::DecodeScratch;

impl Session {
    /// Answers the penalty action on the highlighted candidate.
    ///
    /// What leaves is the receipt alone: the list is untouched in this step, because the
    /// store has not been told yet -- the re-decode that moves the candidate down is the
    /// host's sequel to executing the effect, the same sequel a pin toggle rides. A
    /// session with no highlight answers nothing, which is what hands the key back to
    /// the application, and the symbol panel's candidates answer nothing for the same
    /// reason they commit nothing: a symbol is chosen, not spelled, and no frequency
    /// record stands behind it.
    pub(super) fn reduce_highlighted(&mut self, ctx: &mut Ctx<'_>) {
        if self.symbol_mode {
            return;
        }
        let Some(candidate) = self.highlighted_candidate() else {
            return;
        };
        let key = String::from(candidate.text.as_str());
        ctx.push(Effect::ReduceUserFreq { key });
    }

    /// Answers the hide action on the highlighted candidate.
    ///
    /// The candidate leaves the list on the keystroke -- the same removal the forget
    /// transition performs -- and the pair of the input and the text enters the hide
    /// set, which is what keeps every later decode of the same input from producing it
    /// again this session. Nothing is written anywhere else: no effect leaves the step,
    /// because no host work is owed. A composition without a highlight, and the symbol
    /// panel's list -- rebuilt from its table on every refresh, so a hide could not
    /// hold -- answer nothing.
    pub(super) fn hide_highlighted(&mut self, ctx: &mut Ctx<'_>) {
        if self.symbol_mode {
            return;
        }
        let Some(candidate) = self.highlighted_candidate() else {
            return;
        };
        // The input key is folded to the alphabet the records are keyed in, the way the
        // pin toggle and the phrase save fold it: the buffer accepts nothing outside
        // that alphabet, so no character can be lost by folding here.
        let key = self.buf.raw().to_ascii_lowercase();
        let text = String::from(candidate.text.as_str());
        if key.is_empty() {
            return;
        }
        if self.drop_candidate_at(self.paging.highlight).is_none() {
            return;
        }
        self.hidden
            .insert((Box::from(key.as_str()), Box::from(text.as_str())));
        self.emit_frame(ctx.cfg, ctx);
    }
}

/// Removes the candidates this session has hidden from the freshly decoded list.
///
/// A pair is named by the input it was hidden under as well as by its text, so the
/// filter only touches the list of the input the user was looking at when they hid
/// it: another input that spells the same text, or the same text behind a different
/// reading, is not the candidate the user sent away. The removals renumber the list,
/// because the decode's invariant -- a candidate's display number is its position
/// plus one, and the number keys and the click indices read against it -- does not
/// survive gaps.
///
/// This is a function of the three fields rather than a method on the session, and
/// the reason is the refresh's own: the text the highlight follows is held as a
/// borrow of [`Session::previous`] across the decode, and a method taking the whole
/// session would collide with it. The three fields the filter reads -- the workspace
/// to rewrite, the set to read, the input to name the pairs by -- are disjoint from
/// that borrow, which is exactly what the split signature borrows.
pub(in crate::state) fn filter_hidden_candidates(
    scratch: &mut DecodeScratch,
    hidden: &HashSet<(Box<str>, Box<str>)>,
    raw: &str,
) {
    if hidden.is_empty() {
        return;
    }
    let key = raw.to_ascii_lowercase();
    let mut result = scratch.take_result();
    let before = result.candidates.len();
    result.candidates.retain(|candidate| {
        !hidden.iter().any(|(hidden_key, hidden_text)| {
            hidden_key.as_ref() == key.as_str() && hidden_text.as_ref() == candidate.text.as_str()
        })
    });
    if result.candidates.len() != before {
        renumber(&mut result.candidates);
    }
    scratch.recycle(result);
}

#[cfg(test)]
mod tests {
    use ime_types::KeyAction;

    use crate::lm::InMemoryLm;
    use crate::state::machine::{Session, SessionEvent, SessionState};
    use crate::state::tests::{Fixture, composing, hidden, index_of, kinds};
    use crate::state::{SessionConfig, SessionEnv, step};
    use crate::viterbi::Decoder;
    use crate::viterbi::lattice::testing::{MockLexicon, MockUserFreq};

    use super::Effect;

    /// The user frequency the rank-flip fixture answers with before the penalty: large
    /// enough that its user term outranks the one-Q8 unigram deficit the fixture gives
    /// the penalized word, which is what puts that word first.
    const BOOSTED_FREQ: u32 = 65_535;

    /// The two answers the store gives either side of a penalty, as the environments a
    /// session is stepped with: the compose-time environment answers `BOOSTED_FREQ` for
    /// the penalized word and nothing for its rival, and the re-decode after the host
    /// applied the penalty answers nothing for either. The session never calls the store
    /// itself -- that is the point the two environments make.
    /// Assembles the environment the rank test steps with, over the user source the
    /// caller owns: the mock has to outlive the environment it answers through, so it
    /// is never built inside.
    fn env_over<'a>(
        decoder: &'a Decoder,
        lexicon: &'a MockLexicon,
        lm: &'a InMemoryLm,
        user: &'a MockUserFreq,
    ) -> SessionEnv<'a> {
        SessionEnv {
            decoder,
            lexicon,
            user_freq: user,
            lm,
            english: None,
            confusables: None,
            symbols: None,
            assoc: None,
        }
    }

    /// Returns the word the step's penalty receipt names, if one left.
    fn penalized(effects: &[Effect]) -> Option<&str> {
        effects.iter().find_map(|effect| match effect {
            Effect::ReduceUserFreq { key } => Some(key.as_str()),
            _ => None,
        })
    }

    /// The texts of the candidate list, in display order.
    fn texts_of(session: &Session) -> Vec<&str> {
        session
            .decoded()
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect()
    }

    /// Distinct single-character readings, so the wide fixture's list holds no
    /// duplicate text for one key.
    const WIDE_TEXTS: [&str; 30] = [
        "你", "尼", "泥", "拟", "逆", "妮", "匿", "腻", "溺", "倪", "霓", "呢", "馁", "内", "嫩",
        "能", "嗯", "唔", "屋", "乌", "吴", "无", "五", "伍", "午", "舞", "武", "雾", "物", "误",
    ];

    /// A dictionary that holds more candidates than the first supply rung, so that the
    /// expansion hook of the hide filter is reachable from a page request.
    fn wide_lexicon() -> MockLexicon {
        let rows: Vec<(&'static str, &'static str)> =
            WIDE_TEXTS.iter().map(|text| ("ni", *text)).collect();
        MockLexicon::with(&rows)
    }

    #[test]
    fn test_reduce_highlighted_emits_the_receipt_and_leaves_the_list_alone() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        let position = index_of(&session, "你");

        let effects = session.handle_key(KeyAction::ReduceFrequency, &cfg, &env);

        // The receipt alone: no frame and no removal, because the rank moves on the
        // host's re-decode, not in this step.
        assert_eq!(kinds(&effects), ["reduce-user-freq"]);
        assert_eq!(penalized(&effects), Some("你"));
        assert_eq!(index_of(&session, "你"), position, "the list is untouched");
        assert_eq!(session.state, SessionState::Composing);
    }

    #[test]
    fn test_reduce_highlighted_without_a_highlight_is_inert() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = Session::new();

        let effects = session.handle_key(KeyAction::ReduceFrequency, &cfg, &env);

        assert!(effects.is_empty(), "there is no highlight to penalize");
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_reduce_highlighted_on_a_panel_candidate_is_inert() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let table = crate::symbols::SymbolTable::parse("1\t数字\t０１\n").expect("rows are usable");
        let env = fixture.env_with_symbols(&table);
        let mut session = composing(&cfg, &env, "v1");
        assert!(session.is_symbol_mode(), "the panel is open");

        let effects = session.handle_key(KeyAction::ReduceFrequency, &cfg, &env);

        assert!(
            effects.is_empty(),
            "a symbol is chosen, not spelled: no frequency record stands behind it"
        );
        assert_eq!(session.state, SessionState::Composing, "the panel stays");
    }

    #[test]
    fn test_hide_highlighted_removes_the_candidate_and_keeps_it_hidden() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        assert!(index_of(&session, "你").is_some());

        let effects = session.handle_key(KeyAction::HideCandidate, &cfg, &env);

        // The removal is the session's own: one frame, no store effect at all.
        assert_eq!(kinds(&effects), ["send-frame"]);
        assert_eq!(hidden(&effects), None);
        assert_eq!(session.state, SessionState::Composing);
        assert!(
            index_of(&session, "你").is_none(),
            "the candidate left the list"
        );

        // A later decode of the same input cannot produce it again.
        let _ = step(&mut session, SessionEvent::PinApplied, &cfg, &env);
        assert!(
            index_of(&session, "你").is_none(),
            "the re-decode kept it out"
        );
    }

    #[test]
    fn test_hide_survives_the_composition_and_dies_with_the_session() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        let _ = session.handle_key(KeyAction::HideCandidate, &cfg, &env);

        // The composition ends and a new one begins in the same session: the hide set
        // is the session's memory, not the composition's. The escape leaves the session
        // waiting for the preedit confirmation, so the same event closes it before the
        // next composition starts.
        let _ = session.handle_key(KeyAction::Escape, &cfg, &env);
        let _ = step(&mut session, SessionEvent::PreeditCleared, &cfg, &env);
        assert_eq!(session.state, SessionState::Idle);
        let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('i'), &cfg, &env);
        assert!(
            index_of(&session, "你").is_none(),
            "the same session keeps the hide"
        );

        // A restarted session knows nothing of it -- hiding across sessions is the
        // forget action's job, and this set does not duplicate that mechanism.
        let mut restarted = Session::new();
        let _ = restarted.handle_key(KeyAction::InputChar('n'), &cfg, &env);
        let _ = restarted.handle_key(KeyAction::InputChar('i'), &cfg, &env);
        assert!(
            index_of(&restarted, "你").is_some(),
            "a restarted session offers the candidate again"
        );
    }

    #[test]
    fn test_hide_is_scoped_to_the_input_it_was_hidden_under() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        let _ = session.handle_key(KeyAction::HideCandidate, &cfg, &env);

        // Another input that spells the same text behind a different reading is not the
        // candidate the user sent away: the pair is named by the input as well. The
        // escape is closed the way the machine closes it, before the next composition.
        let _ = session.handle_key(KeyAction::Escape, &cfg, &env);
        let _ = step(&mut session, SessionEvent::PreeditCleared, &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('i'), &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('a'), &cfg, &env);
        let _ = session.handle_key(KeyAction::InputChar('o'), &cfg, &env);
        assert_eq!(session.buf.raw(), "nihao");
        assert!(
            index_of(&session, "你好").is_some(),
            "the two-character reading of another input is untouched"
        );
    }

    #[test]
    fn test_hidden_candidate_cannot_return_through_a_page_expansion() {
        let cfg = SessionConfig::default();
        let lexicon = wide_lexicon();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let user = MockUserFreq::with(&[]);
        let env = SessionEnv {
            decoder: &decoder,
            lexicon: &lexicon,
            user_freq: &user,
            lm: &lm,
            english: None,
            confusables: None,
            symbols: None,
            assoc: None,
        };
        let mut session = composing(&cfg, &env, "ni");
        let _ = session.handle_key(KeyAction::HideCandidate, &cfg, &env);
        assert!(index_of(&session, "你").is_none());

        // Paging past the materialized list widens the decode; the wider rung filters
        // through the same set the narrow one did.
        let _ = session.handle_key(KeyAction::PageNext, &cfg, &env);
        assert!(
            index_of(&session, "你").is_none(),
            "the wider decode kept the hidden candidate out"
        );
        assert!(
            session.candidate_count() > 5,
            "the wider list itself is on show"
        );
    }

    #[test]
    fn test_hide_without_a_highlight_is_inert() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = Session::new();

        let effects = session.handle_key(KeyAction::HideCandidate, &cfg, &env);

        assert!(effects.is_empty(), "there is no highlight to hide");
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_hide_on_a_panel_candidate_is_inert() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let table = crate::symbols::SymbolTable::parse("1\t数字\t０１\n").expect("rows are usable");
        let env = fixture.env_with_symbols(&table);
        let mut session = composing(&cfg, &env, "v1");

        let effects = session.handle_key(KeyAction::HideCandidate, &cfg, &env);

        assert!(
            effects.is_empty(),
            "the panel rebuilds its list every refresh, so a hide could not hold"
        );
        // The row is a list of symbols, one candidate each: the fixture's single row
        // spells two of them, and both stay.
        assert_eq!(texts_of(&session).len(), 2, "the panel list is untouched");
    }

    #[test]
    fn test_negative_feedback_routes_are_distinct_from_the_forget() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();

        // The penalty: the list stands, the receipt leaves.
        let mut penalized_session = composing(&cfg, &env, "ni");
        let penalized_effects =
            penalized_session.handle_key(KeyAction::ReduceFrequency, &cfg, &env);
        assert_eq!(kinds(&penalized_effects), ["reduce-user-freq"]);
        assert!(index_of(&penalized_session, "你").is_some());

        // The hide: the list shortens, and nothing reaches the store.
        let mut hidden_session = composing(&cfg, &env, "ni");
        let hidden_effects = hidden_session.handle_key(KeyAction::HideCandidate, &cfg, &env);
        assert_eq!(kinds(&hidden_effects), ["send-frame"]);
        assert!(index_of(&hidden_session, "你").is_none());

        // The forget: the list shortens, and the store is told.
        let mut forgotten_session = composing(&cfg, &env, "ni");
        let forgotten_effects =
            forgotten_session.handle_key(KeyAction::ForgetHighlighted, &cfg, &env);
        assert_eq!(
            kinds(&forgotten_effects),
            ["forget-user-word", "send-frame"]
        );
        assert!(index_of(&forgotten_session, "你").is_none());
    }

    #[test]
    fn test_pin_applied_redecode_moves_the_penalized_candidate_down() {
        let cfg = SessionConfig::default();
        // The penalized word starts one Q8 unigram step below its rival, so the boosted
        // user term is what holds it up; the store answering zero after the penalty is
        // what drops it back -- down, not out: the word stays offered.
        let lexicon = MockLexicon::with(&[("ni", "你"), ("ni", "尼")]);
        let mut lm = InMemoryLm::new();
        lm.insert_unigram("你", -256);
        lm.insert_unigram("尼", 0);
        let decoder = Decoder::default();
        let user_before = MockUserFreq::with(&[("你", BOOSTED_FREQ)]);
        let user_after = MockUserFreq::with(&[]);
        let env_before = env_over(&decoder, &lexicon, &lm, &user_before);
        let env_after = env_over(&decoder, &lexicon, &lm, &user_after);

        let mut session = composing(&cfg, &env_before, "ni");
        assert_eq!(
            session.decoded().candidates[0].text,
            "你",
            "the boost holds it up"
        );

        // The host executed the penalty and sent the sequel; the re-decode of the
        // unchanged input reads the store's new answer and moves the rank. The
        // composition text is unchanged, so the frame is the only effect the step owes.
        let effects = step(&mut session, SessionEvent::PinApplied, &cfg, &env_after);

        assert_eq!(kinds(&effects), ["send-frame"]);
        assert_eq!(
            session.decoded().candidates[0].text,
            "尼",
            "the penalized word lost its place"
        );
        assert!(
            index_of(&session, "你").is_some(),
            "it moved down rather than out: that is the forget action's answer"
        );
        assert_eq!(session.state, SessionState::Composing);
    }

    #[test]
    fn test_the_hide_set_starts_empty_and_costs_one_branch() {
        let cfg = SessionConfig::default();
        let fixture = Fixture::new();
        let env = fixture.env();
        let mut session = composing(&cfg, &env, "ni");
        let position = index_of(&session, "你");

        // A session that never used the action filters nothing: the refreshes below run
        // against an empty set and the list is the decode's own, byte for byte.
        let _ = step(&mut session, SessionEvent::PinApplied, &cfg, &env);
        let _ = session.handle_key(KeyAction::PageNext, &cfg, &env);
        assert_eq!(index_of(&session, "你"), position);
        assert_eq!(session.state, SessionState::Composing);
    }
}
