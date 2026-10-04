//! The session object itself: its shape before anything happens, the identity and
//! the configuration it is built with, and the helpers the other cases rely on.
//!
//! Driven entirely from the in-memory doubles next door. No dictionary file, no
//! clock, no environment, no display server.

use ime_types::{CandidateSource, KeyAction};

use crate::segment::{SYLLABLES, SyllableDag, normalize};
use crate::state::{Paging, Session, SessionConfig, SessionState, paging::DEFAULT_PAGE_SIZE};

use super::{Fixture, composing, index_of};

#[test]
fn test_session_is_idle_before_anything_happens() {
    let session = Session::default();

    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.buf.raw(), "");
    assert_eq!(session.decoded().candidates.len(), 0);
    assert_eq!(session.paging, Paging::new());
    assert_eq!(session.revision.value(), 0);
    assert!(!session.temp_english);
    assert_eq!(session.preedit().text, "");
    assert_eq!(session.highlighted_candidate(), None);
}

#[test]
fn test_session_id_is_allocated_from_the_counter_it_was_given() {
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new_with_counter(1000);

    let _ = session.handle_key(KeyAction::InputChar('n'), &cfg, &env);

    assert_eq!(session.id.value(), 1000);
}

#[test]
fn test_session_config_clamps_a_length_limit_the_schema_forbids() {
    assert_eq!(SessionConfig::new(0, 5, true, 720).max_raw_len, 1);
    assert_eq!(SessionConfig::new(200, 5, true, 720).max_raw_len, 64);
    assert_eq!(
        SessionConfig::default(),
        SessionConfig::new(64, 5, true, 720)
    );
    assert_eq!(SessionConfig::default().max_per_row, DEFAULT_PAGE_SIZE);
}

#[test]
fn test_candidate_index_helper_finds_a_word() {
    // The helper the assertions above rely on, checked on its own so that a silent
    // change in the candidate list cannot make them vacuous.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "ni");

    let found = index_of(&session, "你");
    assert!(found.is_some(), "the reading is offered");
    assert_eq!(
        found
            .and_then(|at| session.decoded().candidates.get(usize::from(at)))
            .map(|held| held.text.as_str()),
        Some("你")
    );
    assert_eq!(index_of(&session, "nothing like this"), None);
    assert_eq!(
        session.decoded().candidates[0].source,
        CandidateSource::Dict,
        "the readings come from the dictionary"
    );
}

#[test]
fn test_typing_every_table_syllable_with_bare_keys_composes() {
    // Ergonomics: the whole syllable table is reachable with the keyboard alone. Each
    // entry is typed as its own spelling, with `v` standing in for the umlaut the
    // normalizer folds, one `InputChar` per character -- the action a bare letter key
    // produces -- and no modifier and no mouse event takes part in it.
    //
    // `ê` is the one entry no bare key types: the input alphabet is the ASCII letters and
    // `'`, and the table keeps `ê` as the syllable the normalizer passes through
    // unchanged. The list is pinned so that a second such entry cannot appear unnoticed.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut typed = String::new();
    let mut not_typeable = Vec::new();
    for entry in SYLLABLES {
        typed.clear();
        for ch in entry.chars() {
            typed.push(if ch == 'ü' { 'v' } else { ch });
        }
        if !typed
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '\'')
        {
            not_typeable.push(*entry);
            continue;
        }
        // The typed form reaches the entry it is spelled after...
        assert_eq!(normalize(&typed).text, *entry, "typing {typed:?}");
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build(&typed), Ok(()), "building {typed:?}");
        assert!(dag.has_path(), "{typed:?} must cut into syllables");
        // ...and the session takes it one key press at a time, with nothing typed lost.
        let mut session = Session::new();
        for ch in typed.chars() {
            let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
        }
        assert_eq!(session.state, SessionState::Composing, "typing {typed:?}");
        assert_eq!(session.buf.raw(), typed.as_str(), "typing {typed:?}");
    }
    assert_eq!(not_typeable, ["ê"]);
}
