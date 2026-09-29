//! The decode workspace a session carries, and what it holds between keystrokes.
//!
//! Responsibility: pin the state the session's decode workspace exposes -- the answer
//! the last keystroke produced and the graph it was segmented from -- and pin that a
//! keystroke leaves nothing of the keystroke before it behind, so that the buffers the
//! session reuses across keystrokes cannot turn into a stale candidate list.
//!
//! Boundaries: driven entirely from the in-memory doubles next door. No dictionary
//! file, no clock, no environment, no display server.

use ime_types::{DecodeRequest, DecodeResult, KeyAction};

use super::{Fixture, SessionConfig, composing};
use crate::state::{SessionEvent, step};

/// Decodes `raw` through the fixture's own sources, which is the answer the session's
/// workspace has to agree with.
///
/// The session decodes through the same entry point, against the same dictionary, so a
/// keystroke's answer and this one are the same value; a session that kept a candidate
/// from an earlier input, or decoded something other than what the user has typed,
/// shows up as a difference here.
fn decoded(fixture: &Fixture, raw: &str) -> DecodeResult {
    let env = fixture.env();
    env.decoder
        .decode(&DecodeRequest::new(raw), env.lexicon, env.user_freq, env.lm)
}

#[test]
fn test_session_decoded_is_the_answer_of_the_last_decode() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "ni");

    assert_eq!(session.decoded(), &decoded(&fixture, "ni"));
}

#[test]
fn test_session_refills_one_request_across_keystrokes() {
    // The session keeps one request and refills its input, so every keystroke decodes what
    // the user has typed at that moment -- never what an earlier keystroke left in it.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");

    let _ = session.handle_key(KeyAction::Backspace, &cfg, &env);
    assert_eq!(session.buf.raw(), "ni");
    assert_eq!(session.decoded(), &decoded(&fixture, "ni"));

    for ch in "hao".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    assert_eq!(session.buf.raw(), "nihao");
    assert_eq!(session.decoded(), &decoded(&fixture, "nihao"));
}

#[test]
fn test_session_graph_describes_the_input_of_the_last_keystroke() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihao");
    assert_eq!(session.dag().normalized(), "nihao");
    assert!(session.dag().has_path());

    let _ = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(session.buf.raw(), "ni");
    assert_eq!(
        session.dag().normalized(),
        "ni",
        "the graph follows the input"
    );
}

#[test]
fn test_session_holds_no_candidates_between_compositions() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    assert!(
        !session.decoded().candidates.is_empty(),
        "readings are offered"
    );

    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);

    assert!(
        session.decoded().candidates.is_empty(),
        "the list is emptied"
    );
    assert!(session.decoded().segments.is_empty());
    assert!(
        !session.decoded().degraded,
        "nothing is left to be degraded"
    );
    assert!(session.dag().is_empty(), "the graph goes with the input");
}

#[test]
fn test_composition_started_after_a_finished_one_decodes_its_own_input() {
    // The buffers a session keeps across keystrokes outlive a composition; what must not
    // outlive it is the answer they held. The first keystroke of the next composition
    // answers with a decode of that keystroke alone, on the first candidate of its own
    // list, and not with anything the finished composition left parked.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "ni");
    session.paging.highlight = 3;
    let _ = session.handle_key(KeyAction::Escape, &cfg, &env);
    let _ = step(&mut session, SessionEvent::PreeditCleared, &cfg, &env);

    let _ = session.handle_key(KeyAction::InputChar('h'), &cfg, &env);

    assert_eq!(session.buf.raw(), "h");
    assert_eq!(session.decoded(), &decoded(&fixture, "h"), "its own input");
    assert_eq!(session.paging.highlight, 0);
    assert_eq!(session.paging.page, 0);
}
