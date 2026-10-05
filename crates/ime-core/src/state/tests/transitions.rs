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

use ime_types::KeyAction;

use crate::state::SessionConfig;
use crate::state::machine::{Effect, Session, SessionEvent, SessionState, step};

use super::{Fixture, composing, frame_of, highlighted, kinds};

/// A live composition on `raw`, with the fixture that decoded it kept alive beside it.
fn composing_on(raw: &str) -> (Session, Fixture) {
    let fixture = Fixture::new();
    let cfg = SessionConfig::default();
    let mut session = composing(&cfg, &fixture.env(), raw);
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
