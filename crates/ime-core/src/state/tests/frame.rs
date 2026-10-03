//! The frame one keystroke hands the window, and the buffers it is built from.
//!
//! Responsibility: pin what a keystroke puts on screen -- the preedit the frame
//! carries, the page of candidates it carries, and the preedit the application's
//! preedit area is told to show -- and pin the reuse the frame path rests on, so that
//! a later change cannot start sizing a buffer per frame, or start handing the frame a
//! buffer the session keeps writing into, without a test going red.
//!
//! Boundaries: driven entirely from the in-memory doubles next door. No dictionary
//! file, no clock, no environment, no display server.

use ime_types::{KeyAction, Preedit};

use super::{Effect, Fixture, Session, SessionConfig, composing, frame_of, index_of};

/// The keystrokes the consistency case walks through.
///
/// The first opens the window and the rest follow it, and none of them empties the
/// input, so every one of them carries a preedit and a frame.
const TYPED: &str = "nihao";

/// Moves the caret back and forth this many times, which is the steady state the
/// buffer-reuse case measures.
const CARET_STEPS: usize = 64;

/// Returns the preedit the step asked the application's preedit area to show.
fn update_preedit(effects: &[Effect]) -> Option<&Preedit> {
    effects.iter().find_map(|effect| match effect {
        Effect::UpdatePreedit(preedit) => Some(preedit),
        _ => None,
    })
}

#[test]
fn test_update_preedit_payload_matches_the_frame_byte_for_byte() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    // The first character opens the window and the rest follow it, so the walk covers
    // both paths that carry a preedit: the one that puts a composition on screen and
    // the one that follows a change to it.
    for (step, ch) in TYPED.chars().enumerate() {
        let effects = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);

        let payload = update_preedit(&effects).expect("a composing step shows its preedit");
        let frame = frame_of(&effects).expect("a composing step sends a frame");

        assert_eq!(payload.text.as_bytes(), frame.preedit.text.as_bytes());
        assert_eq!(payload.caret, frame.preedit.caret, "step {step}");
        assert_eq!(payload.spans, frame.preedit.spans, "step {step}");
        assert_eq!(payload, &frame.preedit, "step {step}");
    }
}

#[test]
fn test_repeat_frames_do_not_grow_the_session_buffers() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");

    // Moving the caret is the one keystroke that rebuilds the whole frame without
    // changing the input: it rebuilds the preedit and emits the pair a keystroke
    // emits. Repeating it is therefore a steady state in which nothing but the caret
    // moves, and the first move is the one that sizes the buffers -- every move after
    // it must reuse them exactly as they are.
    let mut frames = 0usize;
    let mut warmed: Option<(usize, usize)> = None;
    let mut text = String::new();
    for step in 0..CARET_STEPS {
        let delta = if step % 2 == 0 { -1 } else { 1 };
        let effects = session.handle_key(KeyAction::MoveCaret(delta), &cfg, &env);
        let frame = frame_of(&effects).expect("moving the caret repaints the window");
        frames += 1;

        let preedit = session.preedit();
        let sizes = (preedit.text.capacity(), preedit.spans.capacity());
        match warmed {
            None => {
                text = preedit.text.clone();
                warmed = Some(sizes);
            }
            Some(first) => {
                assert_eq!(sizes, first, "step {step}: the buffers did not grow");
                assert_eq!(preedit.text, text, "step {step}: the text is unchanged");
            }
        }
        assert_eq!(frame.preedit, *preedit);
    }

    assert_eq!(frames, CARET_STEPS, "every move rebuilt the frame");
    let sized = warmed.is_some_and(|(bytes, spans)| bytes > 0 && spans > 0);
    assert!(sized, "the first move sized the preedit buffers");
}

#[test]
fn test_frame_carries_the_page_of_the_list_it_was_built_from() {
    let cfg = SessionConfig {
        max_per_row: 5,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    let effects = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let frame = frame_of(&effects).expect("turning the page repaints the window");

    let total = u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX);
    let start = usize::from(session.paging.page_start()).min(session.decoded().candidates.len());
    let end = usize::from(session.paging.page_end(total)).min(session.decoded().candidates.len());
    let page = session.decoded().candidates[start..end].to_vec();

    assert_eq!(
        frame.candidates, page,
        "the frame carries the page, in order"
    );
    assert!(!frame.candidates.is_empty(), "the second page is not empty");
    assert_eq!(frame.page, session.paging.page_state(total));
    assert_eq!(frame.preedit.text, session.preedit().text);
    assert_eq!(frame.status, session.frame_context().status);
    assert_eq!(frame.revision, session.revision.value());

    // Field by field, so that a snapshot which dropped an annotation or a source label
    // would fail here rather than in the window.
    let carried = &frame.candidates[0];
    let held = &session.decoded().candidates[start];
    assert_eq!(carried.index, held.index);
    assert_eq!(carried.text, held.text);
    assert_eq!(carried.annotation, held.annotation);
    assert_eq!(carried.source, held.source);
    assert_eq!(carried.score, held.score);
    assert_eq!(carried.consumed_syllables, held.consumed_syllables);
    assert_eq!(
        frame.highlight,
        session.paging.highlight_position_in_page(total),
        "the highlight travels with the page, in the page's own numbering"
    );
}

#[test]
fn test_frame_preedit_is_a_snapshot_and_not_a_view_of_the_session() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let first = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);
    let frame = frame_of(&first).expect("the first keystroke shows the window");
    let snapshot = frame.preedit.clone();
    assert_eq!(snapshot.text, "n");

    // The next keystroke rebuilds the session's preedit in place. The frame that was
    // already handed over must not move with it: reusing a buffer across keystrokes is
    // not the same as sharing it with a frame that has left the session.
    let _ = session.handle_key(KeyAction::InputChar('i'), &cfg, &env);

    assert_eq!(frame.preedit, snapshot, "the frame keeps what it carried");
    assert_eq!(frame.preedit.text, "n");
    assert_ne!(session.preedit().text, frame.preedit.text);
}

#[test]
fn test_refresh_finds_the_highlighted_text_without_copying_it() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    assert!(session.decoded().candidates.len() > 3, "a page of readings");
    session.paging.highlight = 3;
    let chosen = session.decoded().candidates[3].text.clone();

    // Re-decoding the same input offers the same words, so the highlight has to stay
    // on the word it was on. The comparison that puts it back reads that word out of
    // the list the decode is about to replace, and it reads it as a borrow: a build
    // that dropped the read would land on the first candidate instead.
    session.refresh(&env);

    assert_eq!(
        index_of(&session, &chosen),
        Some(3),
        "the list is unchanged"
    );
    assert_eq!(session.paging.highlight, 3, "the highlight followed it");
    assert_eq!(session.paging.page, 0);
}

#[test]
fn test_refresh_with_a_highlight_past_the_list_starts_the_list_over() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    session.paging.highlight = u16::MAX;

    session.refresh(&env);

    assert_eq!(session.paging.highlight, 0, "the list starts over");
    assert_eq!(session.paging.page, 0);
    assert!(
        !session.decoded().candidates.is_empty(),
        "words are still offered"
    );
}

/// Asserts the frame's highlight against the session's paging state.
///
/// The one invariant every state of the matrix reduces to: the frame carries the
/// highlight the paging state holds, converted to the page on show's own numbering,
/// and the cell it names is the one `Space` would commit.
fn assert_frame_highlight_matches_paging(frame: &ime_types::UiFrame, session: &Session) {
    let total = u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX);
    assert_eq!(
        frame.highlight,
        session.paging.highlight_position_in_page(total),
        "the frame carries the paging state's highlight, in page coordinates"
    );
    let position = usize::from(frame.highlight.expect("a shown page holds its highlight"));
    let committed = session
        .highlighted_candidate()
        .expect("a composing session keeps a highlighted candidate");
    assert_eq!(
        frame.candidates[position].text, committed.text,
        "the ring sits on the candidate the commit keys would take"
    );
}

#[test]
fn test_frame_highlight_follows_the_paging_state_through_moves_and_pages() {
    let cfg = SessionConfig {
        max_per_row: 5,
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    // The lattice keeps WORDS_PER_KEY words per key, so the fixture's readings of one
    // syllable stop at eight candidates -- exactly two pages at five per row: a full
    // first page, a boundary to cross, and a partly filled last page. That is the
    // whole shape this walk needs, and a longer list would only push the end of it
    // past the five-page window.
    assert!(
        session.decoded().candidates.len() > usize::from(session.paging.page_size),
        "the fixture offers more than one page of readings (got {})",
        session.decoded().candidates.len()
    );

    // Every direction-key and paging state of the matrix, one row each. The empty page
    // and the single-candidate page have no composing session to drive them with the
    // shared fixture; their arithmetic is pinned by the `Paging` tests next door.
    let steps = [
        KeyAction::MoveHighlight(1),
        KeyAction::MoveHighlight(1),
        KeyAction::MoveHighlight(-1),
        KeyAction::MoveHighlight(1),
        KeyAction::MoveHighlight(1),
        KeyAction::MoveHighlight(1),
        // The seventh step crosses onto the second page; paging back lands on the first
        // page's last candidate, and paging forward again on the second page's first.
        KeyAction::MoveHighlight(1),
        KeyAction::PagePrev,
        KeyAction::PageNext,
    ];
    for action in steps {
        let effects = session.handle_key(action, &cfg, &env);
        let frame = frame_of(&effects).expect("a move that lands re-sends the frame");
        assert_frame_highlight_matches_paging(frame, &session);
    }

    // Holding the key to the end of the list walks onto the partly filled last page,
    // and the highlight stops at its last candidate rather than past it.
    let mut last = None;
    for _ in 0..64 {
        let effects = session.handle_key(KeyAction::MoveHighlight(1), &cfg, &env);
        let Some(frame) = frame_of(&effects) else {
            break;
        };
        assert_frame_highlight_matches_paging(frame, &session);
        last = Some(frame.highlight);
    }
    let total = session.decoded().candidates.len();
    let last_page_start = usize::from(session.paging.page_start());
    let frame_highlight = last.expect("walking the list re-sends a frame");
    assert_eq!(
        usize::from(session.paging.highlight),
        total - 1,
        "the walk ends on the last candidate the window can reach"
    );
    assert_eq!(
        frame_highlight,
        Some(u16::try_from(total - last_page_start - 1).unwrap_or(u16::MAX)),
        "the last frame highlights the last cell of the partly filled page"
    );
}

#[test]
fn test_frame_highlight_hides_when_the_page_holds_no_candidate_for_it() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");

    // A highlight on the next page's territory while the window still shows the first
    // one: the page on show holds no candidate for it, so the frame carries `None`.
    // The ring must not outlive the candidate it named, which is what `None` exists to
    // say; the mode keys are the one re-send that rebuilds the frame without touching
    // the paging state.
    let total = u16::try_from(session.decoded().candidates.len()).unwrap_or(u16::MAX);
    session.paging.highlight = session.paging.page_end(total);

    let effects = session.handle_key(KeyAction::ToggleLang, &cfg, &env);
    let frame = frame_of(&effects).expect("a mode flip re-sends the frame alone");
    assert_eq!(
        frame.candidates.len(),
        usize::from(session.paging.page_size),
        "the first page is still the one on show"
    );
    assert_eq!(frame.highlight, None, "and it holds nothing to highlight");
}
