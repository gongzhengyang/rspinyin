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

use ime_types::{DecodeFlags, KeyAction};

use crate::fuzzy;
use crate::state::SessionConfig;
use crate::state::machine::{CONTEXT_KEY_BUDGET, Effect, Session, SessionEvent, SessionState, step};

use super::{Fixture, composing, committed, frame_of, highlighted, kinds, pin_toggle};

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
    assert!(texts.contains(&"真诚"), "the slip reaches the word: {texts:?}");
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
        Some("纠错：zhegn"),
        "the annotation carries the original spelling: {corrected:?}"
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
        decoded.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
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
    let top = decoded.candidates.first().expect("the assertion above held");
    assert_eq!(
        top.annotation.as_deref(),
        Some("纠错：awnba"),
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
            let _ = off_session.handle_key(KeyAction::InputChar(ch), &SessionConfig::default(), &env);
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
