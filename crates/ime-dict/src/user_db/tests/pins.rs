//! The pin: the toggle's semantics in the store, and what the ranking reads.
//!
//! The pin lives in three places -- the word set the ranking's cap and eviction's
//! protection read, the key map the toggle reads, and the two tables the flush
//! writes -- so the tests here follow one rule through all of them: a pin is made
//! through the public toggle, and everything else is asserted through the public
//! read paths (`freq`, the record count, a reopen).

use super::*;

/// The word the tests pin, and the two input keys it is pinned under: short literals,
/// like every key in this suite.
const WORD: &str = "hao";
const KEY: &str = "ni";
const OTHER_KEY: &str = "nihao";

#[test]
fn test_set_pinned_lifts_the_word_to_the_cap_and_the_second_press_lowers_it() {
    let dir = temp_dir("pin-toggle");
    let mut db = open_in(&dir, TestClock::default());
    db.record(WORD, 0);
    db.final_commit().expect("flushing");
    db.wait_for_flush();
    assert_eq!(db.freq(WORD), 1, "an unpinned record answers its count");

    db.set_pinned(KEY, Some(WORD))
        .expect("pinning the highlighted word");
    assert_eq!(
        db.freq(WORD),
        PINNED_FREQ,
        "the pinned word answers the cap the ranking lifts it with"
    );

    // The second press of the same word is the toggle's other half: the pin goes, and
    // the word answers its own count again. This is the idempotency the double-press
    // the state machine forwards relies on.
    db.set_pinned(KEY, Some(WORD))
        .expect("pinning the same word again");
    assert_eq!(
        db.freq(WORD),
        1,
        "the second press released the pin the first one made"
    );
}

#[test]
fn test_set_pinned_moves_the_pin_when_a_second_word_takes_the_key() {
    let dir = temp_dir("pin-move");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    clock.advance_ms(1_000);
    db.record(WORD, 0);
    clock.advance_ms(1_000);
    db.record("jie", 0);
    db.final_commit().expect("flushing");
    db.wait_for_flush();

    db.set_pinned(KEY, Some(WORD)).expect("pinning the first");
    db.set_pinned(KEY, Some("jie"))
        .expect("pinning a different word under the same key");
    assert_eq!(
        db.freq(WORD),
        1,
        "the word that held the pin gave it up"
    );
    assert_eq!(
        db.freq("jie"),
        PINNED_FREQ,
        "and the new word holds it: one pinned word per key"
    );
}

#[test]
fn test_set_pinned_none_clears_the_key_pin_explicitly() {
    let dir = temp_dir("pin-clear");
    let mut db = open_in(&dir, TestClock::default());
    db.record(WORD, 0);
    db.final_commit().expect("flushing");
    db.wait_for_flush();
    db.set_pinned(KEY, Some(WORD)).expect("pinning");
    assert_eq!(db.freq(WORD), PINNED_FREQ);

    db.set_pinned(KEY, None).expect("clearing the key's pin");
    assert_eq!(db.freq(WORD), 1, "the explicit clear released the word");

    // A key that holds no pin has nothing to clear, and says so without an error or a
    // write: the call is a no-op, not a refusal.
    db.set_pinned(OTHER_KEY, None).expect("clearing an empty key");
    assert_eq!(db.freq(WORD), 1);
}

#[test]
fn test_a_word_pinned_under_two_keys_stays_until_both_release() {
    let dir = temp_dir("pin-two-keys");
    let mut db = open_in(&dir, TestClock::default());
    db.record(WORD, 0);
    db.final_commit().expect("flushing");
    db.wait_for_flush();

    db.set_pinned(KEY, Some(WORD)).expect("pinning typed short");
    db.set_pinned(OTHER_KEY, Some(WORD))
        .expect("pinning the same word typed long");
    db.set_pinned(KEY, None).expect("releasing the short key");
    assert_eq!(
        db.freq(WORD),
        PINNED_FREQ,
        "the word is still pinned under the long key"
    );
    db.set_pinned(OTHER_KEY, None)
        .expect("releasing the long key");
    assert_eq!(
        db.freq(WORD),
        1,
        "and only the last release takes it out of the cap"
    );
}

#[test]
fn test_set_pinned_survives_a_flush_and_a_reopen_with_its_key() {
    let dir = temp_dir("pin-reopen");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    db.record(WORD, 0);
    db.set_pinned(KEY, Some(WORD)).expect("pinning");
    db.final_commit().expect("flushing the pin");
    db.wait_for_flush();
    drop(db);

    let db = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("reopening the store");
    assert_eq!(
        db.freq(WORD),
        PINNED_FREQ,
        "the flushed pin is live after a reopen"
    );
    // The key association came back with the flag: toggling the same word under the
    // same key releases it, which only works if the reopen restored the key map.
    db.set_pinned(KEY, Some(WORD))
        .expect("toggling the restored pin");
    assert_eq!(db.freq(WORD), 1, "the restored pin was the same pin");
    drop(db);

    // And the release was durable too.
    let db = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("reopening after the release");
    assert_eq!(db.freq(WORD), 1, "the pin stayed released");
}

#[test]
fn test_set_pinned_on_a_readonly_store_is_refused_without_change() {
    let dir = temp_dir("pin-readonly");
    let mut db = open_in(&dir, TestClock::default());
    db.record(WORD, 0);
    inject_failure();
    let _ = db.final_commit().expect_err("the injected flush fails");
    assert!(db.is_readonly(), "the store degraded");

    let error = db
        .set_pinned(KEY, Some(WORD))
        .expect_err("a read-only store takes no pin");
    assert!(matches!(error, ImeError::DataReadonly { .. }));
    assert!(
        error.to_string().starts_with("data/readonly-mode"),
        "the diagnostic carries the frozen code: {error}"
    );
    assert_eq!(
        db.freq(WORD),
        0,
        "nothing was pinned -- the failed flush took the unflushed record with it, and no pin was made"
    );
}

#[test]
fn test_a_pinned_word_is_exempt_from_eviction() {
    let dir = temp_dir("pin-evict");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    for key in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    db.wait_for_flush();
    db.set_pinned(KEY, Some("a")).expect("pinning the oldest");
    let removed = db.evict_oldest(EVICT_FRACTION).expect("evicting");
    assert_eq!(removed, 1, "the sweep still removes its share");
    assert_eq!(db.freq("a"), PINNED_FREQ, "the pinned word stayed");
    assert_eq!(db.freq("b"), 0, "and its neighbour went instead");
}

#[test]
fn test_a_pin_is_known_to_a_store_too_large_to_load() {
    // The ceiling is one record, so the store opens on the fallback path with nothing
    // hydrated -- and the pin has to survive that, because a store does not stop
    // ranking its pins when its record count passes the ceiling.
    let dir = temp_dir("pin-fallback");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    db.record(WORD, 0);
    db.set_pinned(KEY, Some(WORD)).expect("pinning");
    db.final_commit().expect("flushing");
    db.wait_for_flush();
    drop(db);

    let db = UserDb::open_with_hydrate_cap(
        db_path(&dir),
        Box::new(TestClock::default()),
        0,
    );
    let db = db.expect("opening the store past its ceiling");
    assert!(!db.is_hydrated(), "the fixture is on the fallback path");
    assert_eq!(
        db.freq(WORD),
        PINNED_FREQ,
        "the fallback read answers the cap for the pinned word"
    );
}
