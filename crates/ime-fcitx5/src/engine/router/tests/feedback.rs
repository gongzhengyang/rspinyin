//! The negative-feedback routes at the engine level: the penalty receipt reaching the
//! store the router was built with, the re-decode that follows it, and the hide that
//! lives and dies with the context's session.
//!
//! What is pinned here is the wiring, not the arithmetic -- the shift and the decay are
//! the store's, pinned in `ime-dict`, and the transition guards are the session's,
//! pinned in `ime-core`. What a session-level test cannot see is that the store the
//! penalty lands in is the one store the handle owns, and that the flag the executor
//! sets after a landed penalty really feeds the follow-up event that repaints the
//! window; those two are what this file drives, over a real store in a scratch
//! directory and in-memory doubles everywhere else.

use std::path::Path;

use ime_core::state::SessionEvent;
use ime_dict::user_db::UserDb;
use ime_dict::user_lexicon::UserLexiconHandle;
use ime_types::{KeyAction, UserFreqSource};

use crate::engine::router::{KeyRouter, RoutingConfig};

use super::{Fixture, IC, RecordingHost, ordinary_report, shipped_privacy, type_ni};

/// The commits the fixture word is seeded with: the baseline the shifts read against.
const SEED_COMMITS: u32 = 8;

/// The word the tests penalize: the fixture dictionary's top candidate.
const WORD: &str = "你";

/// Opens a store in `dir`, learns [`WORD`] [`SEED_COMMITS`] times and flushes, leaving
/// an un-penalized baseline the assertions read against.
fn seeded_store(dir: &Path) -> UserDb {
    let mut db = UserDb::open(dir.join("user.redb")).expect("opening the test store");
    for _ in 0..SEED_COMMITS {
        db.record(WORD, 1);
    }
    db.final_commit().expect("flushing the seed records");
    db
}

/// A router over `fixture`'s doubles whose negative-feedback receipts reach `handle`.
///
/// The fixture's own helper builds a router without the user lexicon, which is the
/// degraded shape a test router has; the routes under test are the ones that need the
/// handle, so the router is assembled here with it installed.
fn router_with_store<'a>(fixture: &'a Fixture, handle: &'a UserLexiconHandle) -> KeyRouter<'a> {
    KeyRouter::new(fixture.env(), shipped_privacy(), RoutingConfig::default())
        .with_user_lexicon(handle)
}

#[test]
fn test_reduce_frequency_drops_the_store_answer_and_repaints() {
    let fixture = Fixture::new("penalty-drops");
    let mut db = seeded_store(&fixture.dir);
    assert_eq!(db.freq(WORD), SEED_COMMITS, "the seeded baseline is clean");
    let handle = UserLexiconHandle::hydrate(db.clone());
    let mut router = router_with_store(&fixture, &handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert_eq!(host.highlighted.as_deref(), Some(WORD));
    let frames_before = host.frames;

    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::ReduceFrequency), &mut host),
        "a penalty the store took is something the plugin did"
    );
    assert!(
        host.diagnostics.is_empty(),
        "a landed penalty is not a failure: {:?}",
        host.diagnostics
    );
    assert_eq!(
        db.freq(WORD),
        SEED_COMMITS >> 1,
        "the store the handle owns is the one the penalty landed in"
    );
    assert_eq!(
        host.frames,
        frames_before + 1,
        "the follow-up re-decode repainted the window with the new rank"
    );

    // The recovery the demotion promises: a normal commit of the word is positive
    // evidence, worth one level of trust, and the record path lowers the level again.
    db.record(WORD, 1);
    db.final_commit().expect("flushing the recovery commit");
    assert_eq!(
        db.freq(WORD),
        SEED_COMMITS + 1,
        "un-penalized: the commit took the level back off"
    );
}

#[test]
fn test_reduce_frequency_of_an_unknown_word_reports_not_found() {
    let fixture = Fixture::new("penalty-unknown");
    let db = seeded_store(&fixture.dir);
    let handle = UserLexiconHandle::hydrate(db.clone());
    let mut router = router_with_store(&fixture, &handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    // The highlight is on a word the store never learned: the fixture dictionary holds
    // it, the user store does not, and a penalty is a statement about a record.
    type_ni(&mut router, &mut host);
    router.step_session(IC, SessionEvent::Key(KeyAction::InputChar('n')), &mut host);
    let highlighted = host.highlighted.clone().expect("a frame with a candidate");
    let store_answer = db.freq(&highlighted);
    let frames_before = host.frames;

    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::ReduceFrequency), &mut host),
        "the key is claimed and answered with a diagnostic"
    );
    let reported = host
        .diagnostics
        .last()
        .cloned()
        .unwrap_or_else(|| String::from("<no diagnostic>"));
    assert!(
        reported.starts_with("dict/user-word-not-found"),
        "the store's answer is what the diagnostic reports: {reported}"
    );
    assert_eq!(
        db.freq(&highlighted),
        store_answer,
        "nothing changed in the store"
    );
    assert_eq!(
        host.frames, frames_before,
        "a penalty that changed nothing owes no re-decode"
    );
}

#[test]
fn test_reduce_frequency_without_a_store_reports_and_keeps_the_key() {
    let fixture = Fixture::new("penalty-no-store");
    let mut router = KeyRouter::new(fixture.env(), shipped_privacy(), RoutingConfig::default());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::ReduceFrequency), &mut host),
        "the route answered, whatever the store's absence made of it"
    );
    let reported = host
        .diagnostics
        .last()
        .cloned()
        .unwrap_or_else(|| String::from("<no diagnostic>"));
    assert!(
        reported.starts_with("dict/user-word-not-found"),
        "with no store to penalize, nothing was penalized: {reported}"
    );
}

#[test]
fn test_hide_survives_a_new_composition_and_dies_with_the_context() {
    let fixture = Fixture::new("penalty-hide");
    let db = seeded_store(&fixture.dir);
    let handle = UserLexiconHandle::hydrate(db);
    let mut router = router_with_store(&fixture, &handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        router.step_session(IC, SessionEvent::Key(KeyAction::HideCandidate), &mut host),
        "the hide acted on the list, so the key is the plugin's"
    );
    let hidden_frame = host.last_frame.clone().expect("the repaint");
    assert!(
        hidden_frame.candidates.iter().all(|held| held.text != WORD),
        "the candidate left the frame on the keystroke"
    );

    // A new composition in the same context's session: the hide is session memory, and
    // the session -- and the hide with it -- survived the composition that ended.
    let _ = router.step_session(IC, SessionEvent::Key(KeyAction::Escape), &mut host);
    let _ = router.step_session(IC, SessionEvent::Key(KeyAction::InputChar('n')), &mut host);
    let _ = router.step_session(IC, SessionEvent::Key(KeyAction::InputChar('i')), &mut host);
    let again = host
        .last_frame
        .clone()
        .expect("the new composition's frame");
    assert!(
        again.candidates.iter().all(|held| held.text != WORD),
        "the same session keeps the hide"
    );

    // Ending the context ends its session, and the hide with it: hiding across
    // sessions is the forget action's job, so a restarted context offers the word
    // again. `deactivate` is what a window closing takes; the session it parks is the
    // per-context tier's, which ends as a whole.
    router.deactivate(IC, &mut host);
    router.activate_reported(IC, ordinary_report());
    type_ni(&mut router, &mut host);
    let fresh = host
        .last_frame
        .clone()
        .expect("the restarted context's frame");
    assert!(
        fresh.candidates.iter().any(|held| held.text == WORD),
        "a restarted context offers the candidate again"
    );
}

#[test]
fn test_hide_is_inert_without_a_composition() {
    let fixture = Fixture::new("penalty-hide-idle");
    let db = seeded_store(&fixture.dir);
    let handle = UserLexiconHandle::hydrate(db);
    let mut router = router_with_store(&fixture, &handle);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(
        !router.step_session(IC, SessionEvent::Key(KeyAction::HideCandidate), &mut host),
        "an idle session has no highlight to hide, so the key reaches the application"
    );
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
}
