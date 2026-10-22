//! The transition arms the routing actions reach one at a time: dropping the
//! highlighted word from the user's frequencies, saving a phrase, the page jumps, and
//! the properties a configuration reload owes a live session.
//!
//! The rest of the table is covered by the table-driven suites (`sweep_tests` and
//! `table_tests`); this file holds the arms whose per-action contract — the exact
//! effects and the state left behind — is worth pinning on its own. It moved out of
//! `transitions.rs` beside its siblings here once the module outgrew the
//! next-to-the-code size, per the workspace test-layout rule.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{CandidateSource, DecodeFlags, KeyAction};

use crate::confusable::ConfusableTable;
use crate::fuzzy;
use crate::state::machine::{
    CONTEXT_KEY_BUDGET, Effect, Session, SessionEvent, SessionState, step,
};
use crate::state::{ImSwitchPolicy, SessionConfig};
use crate::symbols::SymbolTable;

use super::{Fixture, committed, composing, frame_of, highlighted, kinds, pin_toggle};

/// A live composition on `raw`, with the fixture that decoded it kept alive beside it.
fn composing_on(raw: &str) -> (Session, Fixture) {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let session = composing(&cfg, &fixture.env(), raw);
    assert_eq!(session.state, SessionState::Composing);
    (session, fixture)
}

#[test]
fn test_forget_highlighted_preserves_highlight() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    // Off the first candidate, so that "the highlight followed the words" and "the
    // highlight was reset" are different outcomes rather than the same one.
    let _ = session.handle_key(KeyAction::MoveHighlight(2), &cfg, &env);
    assert_eq!(
        session.paging.highlight, 2,
        "the fixture pages enough to move"
    );
    let dropped = highlighted(&session).unwrap_or_default().to_owned();
    let next = session
        .decoded()
        .candidates
        .get(3)
        .map(|held| held.text.clone())
        .expect("a candidate behind the highlighted one");

    let effects = session.handle_key(KeyAction::ForgetHighlighted, &cfg, &env);

    assert_eq!(
        session.state,
        SessionState::Composing,
        "the composition survives the removal"
    );
    assert_eq!(session.buf.raw(), "ni", "and so does the input");
    assert_eq!(
        effects.len(),
        2,
        "the store is told and the window is re-sent"
    );
    match effects.first() {
        Some(Effect::ForgetUserWord { key }) => {
            assert_eq!(key.as_str(), dropped.as_str());
        }
        other => panic!("expected a forget effect first, got {other:?}"),
    }
    assert!(
        matches!(effects.last(), Some(Effect::SendFrame(_))),
        "the window draws the list without the word"
    );
    assert!(
        !session
            .decoded()
            .candidates
            .iter()
            .any(|held| held.text == dropped),
        "the word is gone from the candidate list"
    );
    assert_eq!(
        session.paging.highlight, 2,
        "the highlight stayed where the user was looking"
    );
    assert_eq!(
        highlighted(&session),
        Some(next.as_str()),
        "on the word that took the removed one's place"
    );
    for (position, candidate) in session.decoded().candidates.iter().enumerate() {
        assert_eq!(
            usize::from(candidate.index),
            position + 1,
            "the display numbers still match the positions"
        );
    }
}

#[test]
fn test_forget_highlighted_at_the_end_keeps_a_neighbour() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let last = u16::try_from(session.decoded().candidates.len())
        .expect("a candidate count that fits")
        .saturating_sub(1);
    assert!(last > 1, "the fixture offers a list to stand at the end of");
    session.paging.highlight = last;
    let before = session
        .decoded()
        .candidates
        .get(usize::from(last) - 1)
        .map(|held| held.text.clone())
        .expect("a candidate in front of the last one");

    let effects = session.handle_key(KeyAction::ForgetHighlighted, &cfg, &env);

    assert!(matches!(
        effects.first(),
        Some(Effect::ForgetUserWord { .. })
    ));
    assert_eq!(
        highlighted(&session),
        Some(before.as_str()),
        "the highlight falls back onto the word in front of the one removed"
    );
    assert!(
        session.paging.highlight > 0,
        "and not back to the top of the list"
    );
}

#[test]
fn test_forget_highlighted_arm_does_nothing_without_a_composition() {
    let fixture = Fixture::new();
    let mut session = Session::new();
    let effects = session.handle_key(
        KeyAction::ForgetHighlighted,
        &SessionConfig::default(),
        &fixture.env(),
    );
    assert!(effects.is_empty(), "there is no highlight to forget");
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_add_phrase_arm_records_the_highlighted_candidate() {
    let (mut session, fixture) = composing_on("ni");
    let expected = highlighted(&session).unwrap_or_default().to_owned();
    assert!(
        !expected.is_empty(),
        "a composition has a candidate to save"
    );

    let effects = session.handle_key(
        KeyAction::AddPhrase,
        &SessionConfig::default(),
        &fixture.env(),
    );
    assert_eq!(effects.len(), 1, "the arm emits exactly one effect");
    assert!(
        matches!(effects.first(), Some(Effect::AddPhrase { .. })),
        "the arm saves the phrase rather than reporting it unsupported"
    );
    if let Some(Effect::AddPhrase { key, text }) = effects.first() {
        assert_eq!(key.as_str(), "ni", "the key is the input the user typed");
        assert_eq!(text.as_str(), expected);
    }
}

#[test]
fn test_add_phrase_arm_leaves_the_composition_alone() {
    let (mut session, fixture) = composing_on("ni");
    let before = session.decoded().clone();
    let raw = String::from(session.buf.raw());
    let id = session.id;

    let effects = session.handle_key(
        KeyAction::AddPhrase,
        &SessionConfig::default(),
        &fixture.env(),
    );
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.id, id, "the composition is the same one");
    assert_eq!(session.buf.raw(), raw.as_str());
    assert_eq!(
        session.decoded(),
        &before,
        "the candidate list is untouched"
    );
    assert_eq!(effects.len(), 1, "nothing is re-sent to the window");
}

#[test]
fn test_add_phrase_arm_does_nothing_without_a_composition() {
    let fixture = Fixture::new();
    let mut session = Session::new();
    let effects = session.handle_key(
        KeyAction::AddPhrase,
        &SessionConfig::default(),
        &fixture.env(),
    );
    assert!(effects.is_empty(), "there is no highlight to save");
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_page_last_jumps_to_the_last_candidate_of_the_last_page() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let total = u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX);
    let pages = session.paging.page_count(total);
    assert!(pages > 1, "the fixture offers a last page worth jumping to");

    let effects = session.handle_key(KeyAction::PageLast, &cfg, &env);

    assert_eq!(
        kinds(&effects),
        ["send-frame"],
        "the jump repaints the grid"
    );
    assert_eq!(session.paging.page, pages - 1, "the last page is on show");
    assert_eq!(
        usize::from(session.paging.highlight),
        usize::from(total) - 1,
        "the highlight is on the list's last candidate"
    );
    let frame = frame_of(&effects).expect("the frame the jump sent");
    assert_eq!(frame.page.current, pages, "and the frame says so");
}

#[test]
fn test_page_first_returns_to_the_first_candidate_of_the_first_page() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let _ = session.handle_key(KeyAction::PageNext, &cfg, &env);
    assert_eq!(session.paging.page, 1, "the fixture pages forward");

    let effects = session.handle_key(KeyAction::PageFirst, &cfg, &env);

    assert_eq!(
        kinds(&effects),
        ["send-frame"],
        "the jump repaints the grid"
    );
    assert_eq!(session.paging.page, 0);
    assert_eq!(session.paging.highlight, 0);

    // On the first page already, the jump is the boundary event and not a repaint: the
    // key is handed back with the grid unchanged.
    let effects = session.handle_key(KeyAction::PageFirst, &cfg, &env);
    assert!(effects.is_empty(), "the grid is already on the first page");
    assert_eq!(
        session.state,
        SessionState::Composing,
        "and the composition survives the inert key"
    );
}

#[test]
fn test_page_last_twice_leaves_the_grid_at_the_end() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();

    let _ = session.handle_key(KeyAction::PageLast, &cfg, &env);
    let at_the_end = (session.paging.page, session.paging.highlight);
    let effects = session.handle_key(KeyAction::PageLast, &cfg, &env);

    assert!(effects.is_empty(), "the last page is already on show");
    assert_eq!(
        (session.paging.page, session.paging.highlight),
        at_the_end,
        "neither the page nor the highlight moved"
    );
}

#[test]
fn test_page_jumps_do_nothing_without_a_composition() {
    let fixture = Fixture::new();
    let mut session = Session::new();
    let cfg = SessionConfig::default();
    let effects = session.handle_key(KeyAction::PageFirst, &cfg, &fixture.env());
    assert!(effects.is_empty(), "there is no grid to jump");
    let effects = session.handle_key(KeyAction::PageLast, &cfg, &fixture.env());
    assert!(effects.is_empty());
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_reload_preserves_active_session() {
    // A reload of the configuration -- the phrase table included, which is swapped
    // in the layer that owns it -- never resets a composition in progress (0.4
    // rule 10): the input, the candidates and the session's own identity all
    // survive it, and only the frame is re-sent under the new values.
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let before = session.decoded().clone();
    let raw = String::from(session.buf.raw());
    let id = session.id;

    let next = SessionConfig {
        max_per_row: 7,
        ..SessionConfig::default()
    };
    let effects = step(
        &mut session,
        SessionEvent::ConfigReloaded(next),
        &cfg,
        &fixture.env(),
    );
    assert_eq!(session.state, SessionState::Composing);
    assert_eq!(session.id, id, "the composition is not restarted");
    assert_eq!(session.buf.raw(), raw.as_str());
    assert_eq!(session.decoded(), &before);
    assert_eq!(effects.len(), 1, "only the frame is re-sent");
    assert!(matches!(effects.first(), Some(Effect::SendFrame(_))));
}

#[test]
fn test_pin_highlighted_arm_toggles_the_highlighted_word_for_the_input() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let _ = session.handle_key(KeyAction::MoveHighlight(2), &cfg, &env);
    let expected = highlighted(&session).unwrap_or_default().to_owned();

    let effects = session.handle_key(KeyAction::PinHighlighted, &cfg, &env);

    assert_eq!(effects.len(), 1, "the arm emits exactly one effect");
    assert_eq!(
        kinds(&effects),
        ["toggle-user-pin"],
        "the arm asks the store for a pin, not a diagnostic"
    );
    let (key, word) = pin_toggle(&effects).expect("the toggle the arm emitted");
    assert_eq!(key, "ni", "the pin belongs to the input the user typed");
    assert_eq!(word, expected, "and names the highlighted candidate");
    assert_eq!(
        session.state,
        SessionState::Composing,
        "the composition survives the pin"
    );
    assert_eq!(
        session.buf.raw(),
        "ni",
        "and so does the input the pin is keyed by"
    );
}

#[test]
fn test_pin_highlighted_arm_does_nothing_without_a_composition() {
    let fixture = Fixture::new();
    let mut session = Session::new();
    let effects = session.handle_key(
        KeyAction::PinHighlighted,
        &SessionConfig::default(),
        &fixture.env(),
    );
    assert!(effects.is_empty(), "there is no highlight to pin");
    assert_eq!(session.state, SessionState::Idle);
}

#[test]
fn test_pin_applied_event_refreshes_a_composing_session() {
    let (mut session, fixture) = composing_on("ni");
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let before = session.decoded().clone();

    let effects = step(&mut session, SessionEvent::PinApplied, &cfg, &env);

    assert_eq!(
        session.state,
        SessionState::Composing,
        "the refresh keeps the composition"
    );
    assert_eq!(
        kinds(&effects),
        ["send-frame"],
        "the store changed what ranks, so the window is re-sent"
    );
    assert_eq!(
        session.buf.raw(),
        "ni",
        "the input is untouched by the re-rank"
    );
    // The refresh decodes again, so the list is a new answer rather than the old one
    // echoed back: equal here because the fixture's store recorded no pin, which is
    // exactly the identity the host loop relies on when a toggle changes nothing.
    assert_eq!(session.decoded(), &before);
}

#[test]
fn test_fuzzy_classes_latched_from_the_config_widen_the_lattice() {
    // The dictionary holds the word under its canonical spelling only: `zong` typed
    // reaches `总` only when the zh_z class is latched, because the class is what
    // widens the input's syllable to the `zhong` family.
    let cfg = SessionConfig {
        fuzzy: fuzzy::projection(false, DecodeFlags::FUZZY_ZH_Z),
        ..SessionConfig::default()
    };
    let fixture = Fixture::with_dictionary(&[("zhong", "总"), ("zong", "宗")]);
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "zong".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts.first(),
        Some(&"宗"),
        "the exact spelling still ranks first"
    );
    assert!(
        texts.contains(&"总"),
        "the widened family is reachable: {texts:?}"
    );

    // Everything off: the same input reaches its own family alone, which is the
    // byte-identical-to-the-mainline decode the default promises.
    let plain = Fixture::with_dictionary(&[("zhong", "总"), ("zong", "宗")]);
    let env = plain.env();
    let mut session = Session::new();
    for ch in "zong".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &SessionConfig::default(), &env);
    }
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["宗"],
        "without a class the variant spelling is unreachable: {texts:?}"
    );
}

#[test]
fn test_fuzzy_reload_latches_at_the_next_composition() {
    // The classes latch beside the scheme: a reload mid composition changes nothing
    // about the composition on screen, and answers the next one (0.4 rule 10).
    let off = SessionConfig::default();
    let on = SessionConfig {
        fuzzy: fuzzy::projection(false, DecodeFlags::FUZZY_ZH_Z),
        ..SessionConfig::default()
    };
    let fixture = Fixture::with_dictionary(&[("zhong", "总"), ("zong", "宗")]);
    let env = fixture.env();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::InputChar('z'), &on, &env);
    let _ = session.handle_key(KeyAction::InputChar('o'), &on, &env);
    let _ = session.handle_key(KeyAction::InputChar('n'), &on, &env);
    let _ = session.handle_key(KeyAction::InputChar('g'), &on, &env);
    let mid: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(
        mid.contains(&"总"),
        "the composition typed under the class reaches the widened family: {mid:?}"
    );

    // Back to idle with the master switch off, then composing again: the next
    // composition reads the new latch, not the one it was typed under.
    let _ = session.handle_key(KeyAction::Escape, &off, &env);
    // The host confirms the preedit is gone, which is what completes the cancel.
    let _ = step(&mut session, SessionEvent::PreeditCleared, &off, &env);
    assert_eq!(session.state, SessionState::Idle);
    for ch in "zong".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &off, &env);
    }
    let after: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        after,
        ["宗"],
        "the reload answered the next composition: {after:?}"
    );
}

#[test]
fn test_abbrev_switch_expands_the_initial_reading() {
    // The fixture carries the abbreviation key a reader of `nh` asks for: the walk
    // spells the initials as `n'h` and answers the prefix range with the word.
    let fixture = Fixture::with_dictionary(&[("n'h", "你好"), ("ni'hao", "你好")]);
    let on = SessionConfig {
        abbrev: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::InputChar('n'), &on, &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &on, &env);
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(
        texts.contains(&"你好"),
        "the abbreviation reaches the word: {texts:?}"
    );

    // The shipped switch is off, and the same input reaches nothing: no syllable
    // spells `nh`, and without the expansion the input is the pass-through
    // candidate alone.
    let env = fixture.env();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::InputChar('n'), &SessionConfig::default(), &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &SessionConfig::default(), &env);
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["nh"],
        "with the switch off the input is echoed, not expanded: {texts:?}"
    );
}

#[test]
fn test_context_rerank_moves_the_next_composition_toward_the_committed_word() {
    // 银行 ends the previous sentence; `yh` in the next one reads as 银行 and 预算
    // at the same weight, and the context is what tells them apart: the candidate
    // the model continues from the committed word moves to the front of the page.
    let rows: &'static [(&'static str, &'static str)] = &[
        ("yin'hang", "银行"),
        ("yu'suan", "预算"),
        ("y'h", "银行"),
        ("y'h", "预算"),
    ];
    let fixture = Fixture::with_dictionary_and_bigram(
        rows,
        &[("银行", -256), ("预算", -256)],
        "银行",
        "银行",
        -64,
    );
    let cfg = SessionConfig {
        abbrev: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    // First composition: `yinhang`, one reading, committed as it stands.
    for ch in "yinhang".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    assert_eq!(session.decoded().candidates[0].text, "银行");
    let effects = session.handle_key(KeyAction::SelectIndex(1), &cfg, &env);
    assert_eq!(committed(&effects), Some("银行"), "the commit lands");
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(session.context(), Some("银行"));

    // Second composition: `yh`, the two candidates tied, the context decides.
    let _ = session.handle_key(KeyAction::InputChar('y'), &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts.first(),
        Some(&"银行"),
        "the continuation of the committed word leads: {texts:?}"
    );
    assert!(
        texts.contains(&"预算"),
        "the other reading stays on the page: {texts:?}"
    );
}

#[test]
fn test_context_rerank_leaves_a_list_no_band_can_hold_alone() {
    // The same committed word, but the two readings are a whole band apart in the
    // base scores: the near-tie run is one candidate long, and context does not
    // reach past it. The base ranking's own first choice stays first.
    let rows: &'static [(&'static str, &'static str)] = &[
        ("yin'hang", "银行"),
        ("yu'suan", "预算"),
        ("y'h", "银行"),
        ("y'h", "预算"),
    ];
    let fixture = Fixture::with_dictionary_and_bigram(
        rows,
        &[("银行", -2048), ("预算", -256)],
        "银行",
        "银行",
        -64,
    );
    let cfg = SessionConfig {
        abbrev: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "yinhang".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let effects = session.handle_key(KeyAction::SelectIndex(1), &cfg, &env);
    assert_eq!(committed(&effects), Some("银行"));
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('y'), &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts.first(),
        Some(&"预算"),
        "a real score gap is not context's to override: {texts:?}"
    );
}

#[test]
fn test_context_rerank_retires_after_its_key_budget() {
    // Five keystrokes are served by the context; the sixth finds it gone. A pure
    // session has no clock, so staleness is counted in the currency the input
    // arrives in.
    let mut session = Session::new();
    session.set_context(String::from("银行"));
    for _ in 0..CONTEXT_KEY_BUDGET {
        session.age_context();
        assert_eq!(
            session.context(),
            Some("银行"),
            "the context serves its budget"
        );
    }
    session.age_context();
    assert_eq!(session.context(), None, "the budget spent, it retires");
}

#[test]
fn test_context_rerank_dies_with_a_cancelled_composition() {
    // The context survives the composition that set it -- that is its point -- but
    // a composition the user took back says nothing about what they mean next, and
    // Escape takes the context with it.
    let rows: &'static [(&'static str, &'static str)] = &[("yin'hang", "银行")];
    let fixture = Fixture::with_dictionary(rows);
    let cfg = SessionConfig::default();
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "yinhang".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let _ = session.handle_key(KeyAction::SelectIndex(1), &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    assert_eq!(session.context(), Some("银行"));
    // A new composition, then Escape: the composition is taken back and the
    // context with it.
    let _ = session.handle_key(KeyAction::InputChar('y'), &cfg, &env);
    assert_eq!(session.context(), Some("银行"));
    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);
    assert_eq!(session.context(), None, "a cancelled composition takes it");
}

/// A two-category panel table, the shape the walkthroughs run against.
fn symbol_fixture() -> SymbolTable {
    SymbolTable::parse("1\t数字\t０１２３４５\nx\t箭头\t←↑→\n").expect("rows are usable")
}

#[test]
fn test_v_prefix_serves_the_category_as_candidates() {
    // `v1` opens the digit category: the symbols arrive as ordinary candidates,
    // labelled with the category's name, and the preedit is the keystrokes as
    // typed.
    let words: &'static [(&'static str, &'static str)] = &[("zhishi", "知识")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "v1".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let decoded = session.decoded();
    let texts: Vec<&str> = decoded
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["０", "１", "２", "３", "４", "５"],
        "file order is display order"
    );
    assert_eq!(decoded.candidates[0].source, CandidateSource::Symbol);
    assert_eq!(decoded.candidates[0].annotation.as_deref(), Some("数字"));
    assert_eq!(
        session.preedit.text, "v1",
        "the panel shows the keystrokes as typed"
    );
}

#[test]
fn test_v_panel_commits_the_highlight_without_learning() {
    // A selection commits the symbol and nothing else: no frequency is recorded
    // for a candidate the user chose from a panel rather than spelled. The
    // selecting key is Space -- inside the panel the number keys key the code,
    // which is what `v1` is made of -- so the walkthrough highlights first and
    // commits second.
    let words: &'static [(&'static str, &'static str)] = &[("zhishi", "知识")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "v1".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let effects = session.handle_key(KeyAction::SelectIndex(1), &cfg, &env);
    // A digit that arrives while the commit is in flight is the application's,
    // as every key of the committing state is.
    assert!(
        committed(&effects).is_none(),
        "the committing state answers no key of its own"
    );
}

#[test]
fn test_v_panel_digit_keys_the_code_instead_of_selecting() {
    // Inside the panel the number keys build the code -- `v1` is made of them --
    // and the one candidate interaction they give up is selection: Space and the
    // highlight keys still select. So the digit that would have selected the
    // first candidate extends the input instead, and the category stays served.
    let words: &'static [(&'static str, &'static str)] = &[("zhishi", "知识")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "v1".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let effects = session.handle_key(KeyAction::SelectIndex(1), &cfg, &env);
    assert_eq!(session.buf.raw(), "v11", "the digit joined the code");
    assert!(
        committed(&effects).is_none(),
        "no candidate was selected by it"
    );
    assert_eq!(
        session.decoded().candidates.len(),
        6,
        "the category is still served"
    );
    // Space selects, unchanged.
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    assert_eq!(
        committed(&effects),
        Some("０"),
        "space commits the highlight"
    );
    assert!(
        session.take_pending().is_none(),
        "a panel candidate is chosen, not spelled"
    );
}

#[test]
fn test_v_prefix_before_pinyin_falls_through_to_the_decode() {
    // `vzhishi` names no category, so it goes where it always went: the decode,
    // whose normalizer turns the `v` into the umlaut it stands for and answers
    // the unsegmentable rest with the pass-through echo. The panel changes
    // nothing about an input it does not serve.
    let words: &'static [(&'static str, &'static str)] = &[("zhishi", "知识")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "vzhishi".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let decoded = session.decoded();
    assert_eq!(
        decoded.candidates[0].text, "vzhishi",
        "the input the panel does not serve is echoed, as before it existed"
    );
    assert_eq!(decoded.candidates[0].source, CandidateSource::Passthrough);
    assert!(
        decoded
            .candidates
            .iter()
            .all(|candidate| candidate.source != CandidateSource::Symbol),
        "no panel candidate exists for an input no category names"
    );
}

#[test]
fn test_digit_joins_the_input_only_behind_a_v() {
    // `ni1` composes as `ni`: the digit is not the panel's, so it is refused and
    // the key travels to the application, exactly as before the panel existed.
    // Behind a `v`, the same digit joins the input and opens the panel.
    let words: &'static [(&'static str, &'static str)] = &[("ni", "伱")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "ni".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let _ = session.handle_key(KeyAction::InputChar('1'), &cfg, &env);
    assert_eq!(
        session.buf.raw(),
        "ni",
        "the digit never joined a pinyin input"
    );
    assert_eq!(session.decoded().candidates[0].text, "伱");
    // The panel's own input takes the digit.
    let mut panel = Session::new();
    for ch in "v".chars() {
        let _ = panel.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let _ = panel.handle_key(KeyAction::InputChar('1'), &cfg, &env);
    assert_eq!(panel.buf.raw(), "v1", "the digit joins a panel input");
    assert_eq!(
        panel.decoded().candidates.len(),
        6,
        "the digit category is served"
    );
}

#[test]
fn test_bare_v_opens_an_empty_panel_and_a_letter_falls_through() {
    // The bare `v` is the panel's entry: the mode is on, the list is empty, and
    // the preedit shows the keystroke. A letter that names no category hands the
    // composition back to the ordinary decode.
    let words: &'static [(&'static str, &'static str)] = &[("a", "啊")];
    let fixture = Fixture::with_dictionary(words);
    let table = symbol_fixture();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::InputChar('v'), &cfg, &env);
    assert!(session.symbol_mode, "the bare v opens the panel");
    assert!(
        session.decoded().candidates.is_empty(),
        "nothing to serve yet"
    );
    assert_eq!(session.preedit.text, "v");
    let _ = session.handle_key(KeyAction::InputChar('a'), &cfg, &env);
    assert!(
        !session.symbol_mode,
        "a code no category names falls back to the decode"
    );
    assert_eq!(
        session.decoded().candidates[0].text,
        "va",
        "the input the panel gave up is echoed, as before it existed"
    );
}

#[test]
fn test_im_switch_clear_takes_the_composition_back_and_commits_nothing() {
    // The shipped policy is exactly a focus loss: the composing text goes, the
    // window hides, and no commit reaches the application.
    let words: &'static [(&'static str, &'static str)] = &[("nihao", "你好")];
    let fixture = Fixture::with_dictionary(words);
    let env = fixture.env();
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "nihao".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let effects = step(&mut session, SessionEvent::Reset, &cfg, &env);
    assert!(
        committed(&effects).is_none(),
        "the clear policy commits nothing"
    );
    assert!(
        kinds(&effects).contains(&"hide"),
        "the window goes away with the composition"
    );
    assert!(session.take_pending().is_none());
}

#[test]
fn test_im_switch_commit_raw_commits_the_typed_input_on_reset() {
    // The policy a user who switches away mid-word asks for: the raw input
    // reaches the application through the ordinary commit effect -- whose
    // host-side execution applies the conversion chain -- and the composition
    // is finished in the same step, because a switch-away reset brings no
    // commit confirmation back.
    let words: &'static [(&'static str, &'static str)] = &[("nihao", "你好")];
    let fixture = Fixture::with_dictionary(words);
    let env = fixture.env();
    let cfg = SessionConfig {
        on_im_switch: ImSwitchPolicy::CommitRaw,
        ..SessionConfig::default()
    };
    let mut session = Session::new();
    for ch in "nihao".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let effects = step(&mut session, SessionEvent::Reset, &cfg, &env);
    assert_eq!(committed(&effects), Some("nihao"), "the raw input commits");
    assert!(
        session.take_pending().is_none(),
        "nothing is learned from a switch-away commit"
    );
    assert!(
        kinds(&effects).contains(&"set-client-preedit"),
        "the client preedit area is emptied in the same step"
    );
}

#[test]
fn test_im_switch_commit_raw_commits_nothing_when_nothing_is_composing() {
    // A reset with an empty input has nothing to keep: the policy degrades to
    // the clear the shipped behaviour already is.
    let words: &'static [(&'static str, &'static str)] = &[("nihao", "你好")];
    let fixture = Fixture::with_dictionary(words);
    let env = fixture.env();
    let cfg = SessionConfig {
        on_im_switch: ImSwitchPolicy::CommitRaw,
        ..SessionConfig::default()
    };
    let mut session = Session::new();
    let effects = step(&mut session, SessionEvent::Reset, &cfg, &env);
    assert!(
        committed(&effects).is_none(),
        "an empty input commits nothing"
    );
}

#[test]
fn test_confusable_hint_attaches_to_the_candidate_that_spells_a_wrong_form() {
    // The table holds `按奈` for `按捺`; the candidate that spells the wrong form
    // says so in its annotation, and a candidate that spells nothing the table
    // holds stays quiet. The hints latch rides `ui.show_annotation`, whose
    // default is on, so a plain configuration is the on case.
    let words: &'static [(&'static str, &'static str)] = &[("an'nai", "按奈")];
    let fixture = Fixture::with_dictionary(words);
    let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are usable");
    let env = fixture.env_with(&table);
    let cfg = SessionConfig::default();
    let mut session = Session::new();
    for ch in "an'nai".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let decoded = session.decoded();
    let top = decoded
        .candidates
        .first()
        .expect("the dictionary holds the reading");
    assert_eq!(top.text, "按奈");
    assert_eq!(
        top.annotation.as_deref(),
        Some("字:→按捺"),
        "the wrong form names the correct one: {top:?}"
    );
}

#[test]
fn test_show_annotation_off_leaves_the_hints_out_of_the_candidates() {
    // With the switch off the whole channel short-circuits at the decode: the
    // table is attached to the environment, and the candidate still carries no
    // annotation, because the lookup is never made.
    let words: &'static [(&'static str, &'static str)] = &[("an'nai", "按奈")];
    let fixture = Fixture::with_dictionary(words);
    let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are usable");
    let env = fixture.env_with(&table);
    let cfg = SessionConfig {
        show_annotation: false,
        ..SessionConfig::default()
    };
    let mut session = Session::new();
    for ch in "an'nai".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    let decoded = session.decoded();
    let top = decoded
        .candidates
        .first()
        .expect("the dictionary holds the reading");
    assert_eq!(top.text, "按奈");
    assert_eq!(
        top.annotation, None,
        "the switch is off, so the channel never ran: {top:?}"
    );
}

#[test]
fn test_correction_switch_reads_a_one_step_slip() {
    // `zhegn` has no exact cut; the correction pass widens the misspelled span
    // onto `zheng`, and the word the dictionary stores there becomes a candidate.
    let words: &'static [(&'static str, &'static str)] = &[("zheng", "真诚")];
    let fixture = Fixture::with_dictionary(words);
    let on = SessionConfig {
        correction: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "zhegn".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &on, &env);
    }
    let decoded = session.decoded();
    let texts: Vec<&str> = decoded
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(
        texts.contains(&"真诚"),
        "the slip reaches the word: {texts:?}"
    );
    // The corrected candidate says why it is there: the annotation carries the
    // spelling the user typed, which the correction edge stands for. A candidate
    // the input spells exactly -- the pass-through echo here -- stays quiet.
    let corrected = decoded
        .candidates
        .iter()
        .find(|candidate| candidate.text == "真诚")
        .expect("the assertion above held");
    assert_eq!(
        corrected.annotation.as_deref(),
        Some("音:zheng｜纠错：zhegn"),
        "the annotation carries the standard reading and the original spelling: {corrected:?}"
    );

    // Off is the shipped switch, and off is the contract: the same keystrokes
    // answer the pass-through candidate alone, because nothing about them spells
    // a word.
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "zhegn".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &SessionConfig::default(), &env);
    }
    let decoded = session.decoded();
    assert!(decoded.degraded, "the default decodes nothing: {decoded:?}");
    assert_eq!(
        decoded
            .candidates
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["zhegn"],
        "the input is echoed, not corrected"
    );
}

#[test]
fn test_correction_annotates_a_word_that_hides_the_slip_inside() {
    // The slip sits inside a word: `awnba` reads as `wan'ba`, and the word that
    // covers the widened span goes on past it. The candidate still says why it is
    // there -- the penalty rides on the word's edge wherever the widened span sits,
    // and the annotation rides on the penalty.
    let words: &'static [(&'static str, &'static str)] = &[("wan'ba", "完把")];
    let fixture = Fixture::with_dictionary(words);
    let on = SessionConfig {
        correction: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    for ch in "awnba".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &on, &env);
    }
    let decoded = session.decoded();
    let texts: Vec<&str> = decoded
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts.first(),
        Some(&"完把"),
        "the corrected reading wins: {texts:?}"
    );
    let top = decoded
        .candidates
        .first()
        .expect("the assertion above held");
    assert_eq!(
        top.annotation.as_deref(),
        Some("音:wan'ba｜纠错：awnba"),
        "the word over the interior slip is annotated: {top:?}"
    );
}

#[test]
fn test_correction_switch_leaves_a_spelled_input_unmoved() {
    // The miss gate is the whole guarantee: an input that spells a word never
    // enters the correction pass, so with the bit on it decodes -- and ranks --
    // exactly as it does with the bit off.
    let words: &'static [(&'static str, &'static str)] =
        &[("zheng", "真诚"), ("ni'hao", "你好"), ("xian", "西安")];
    let fixture = Fixture::with_dictionary(words);
    let on = SessionConfig {
        correction: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    for raw in ["zheng", "nihao", "ni'hao", "xian"] {
        let mut off_session = Session::new();
        let mut on_session = Session::new();
        for ch in raw.chars() {
            let _ =
                off_session.handle_key(KeyAction::InputChar(ch), &SessionConfig::default(), &env);
            let _ = on_session.handle_key(KeyAction::InputChar(ch), &on, &env);
        }
        let off: Vec<&str> = off_session
            .decoded()
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        let on_texts: Vec<&str> = on_session
            .decoded()
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(off, on_texts, "{raw:?} changed under the correction bit");
        // The same holds for the annotation: an input the miss gate let through
        // uncorrected is annotated with nothing, with the bit on or off.
        assert!(
            on_session
                .decoded()
                .candidates
                .iter()
                .all(|candidate| candidate.annotation.is_none()),
            "{raw:?} grew an annotation under the correction bit"
        );
    }
}

#[test]
fn test_correction_rides_below_abbreviation_at_equal_weight() {
    // Both switches on, one input readable both ways: `nh` reads as the
    // abbreviation of `ni'hao` and, corrected, as the exact syllables `n` and
    // `n`. The penalty ladder must keep the narrower tolerance ahead.
    let words: &'static [(&'static str, &'static str)] =
        &[("n'h", "你好"), ("ni'hao", "你好"), ("n'n", "尼尼")];
    let fixture = Fixture::with_dictionary(words);
    let both = SessionConfig {
        abbrev: true,
        correction: true,
        ..SessionConfig::default()
    };
    let env = fixture.env();
    let mut session = Session::new();
    let _ = session.handle_key(KeyAction::InputChar('n'), &both, &env);
    let _ = session.handle_key(KeyAction::InputChar('h'), &both, &env);
    let texts: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(
        texts.contains(&"尼尼"),
        "the corrected reading is a candidate: {texts:?}"
    );
    assert_eq!(
        texts.first(),
        Some(&"你好"),
        "the abbreviation outranks the correction: {texts:?}"
    );
}
