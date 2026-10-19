//! Fallback-cache read and flush tests.

use super::*;

#[test]
fn test_user_db_miss_cache_exists_only_for_a_store_that_was_not_loaded() {
    let dir = temp_dir("cache-presence");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");
    drop(db);

    let loaded = open_in(&dir, TestClock::default());
    assert!(loaded.is_hydrated(), "the fixture store has to be loaded");
    assert!(
        !loaded.has_miss_cache(),
        "a loaded store answers from memory and needs no cache"
    );
    // The store holds an exclusive lock on its file, so the handle has to go before the
    // fallback one opens.
    drop(loaded);

    let fallback_clock = TestClock::default();
    let opened = UserDb::open_with_hydrate_cap(db_path(&dir), Box::new(fallback_clock), 0);
    let fallback = opened.expect("opening the user store");
    assert!(!fallback.is_hydrated(), "a ceiling of zero loads nothing");
    assert!(
        fallback.has_miss_cache(),
        "the fallback path is the one that needs the cache"
    );
}

#[test]
fn test_user_db_fallback_read_adds_the_delta_to_a_cached_value() {
    // An entry holds what the *file* holds, and the delta is added on every read. Caching the
    // total instead would freeze the answer at the first read, and the record path no longer
    // bumps the cache -- it may not take a second lock -- so nothing would correct it.
    let dir = temp_dir("fallback-delta");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");
    drop(db);

    let fallback_clock = TestClock::default();
    let opened = UserDb::open_with_hydrate_cap(db_path(&dir), Box::new(fallback_clock), 0);
    let db = opened.expect("opening the user store");
    assert!(!db.is_hydrated(), "the fixture has to take the fallback");
    assert_eq!(db.freq("ni'hao"), 1, "the first read comes from the store");
    db.record("ni'hao", 0);
    assert_eq!(
        db.freq("ni'hao"),
        2,
        "the record is added to the value the cache remembers"
    );
}

#[test]
fn test_user_db_fallback_read_after_a_flush_sees_the_written_value() {
    let dir = temp_dir("fallback-flush");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");
    drop(db);

    let fallback_clock = TestClock::default();
    let opened = UserDb::open_with_hydrate_cap(db_path(&dir), Box::new(fallback_clock), 0);
    let mut db = opened.expect("opening the user store");
    assert!(!db.is_hydrated(), "the fixture has to take the fallback");
    assert_eq!(db.freq("ni'hao"), 1, "the first read comes from the store");
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing the record");
    assert_eq!(
        db.freq("ni'hao"),
        2,
        "the flush moved the delta into the file and brought the entry with it"
    );
}

#[test]
#[ignore = "soak measurement: only meaningful on an idle machine"]
fn test_user_db_loading_stays_within_its_memory_ceiling() {
    let dir = temp_dir("load-rss");
    let mut db = open_in(&dir, TestClock::default());
    let mut keys: Vec<String> = Vec::with_capacity(HYDRATE_CAP as usize);
    for index in 0..HYDRATE_CAP {
        keys.push(format!("soak{index}"));
    }
    for key in &keys {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing the soak run");
    drop(db);

    let before = resident_bytes();
    assert!(
        before > 0,
        "the resident set cannot be read on this platform"
    );
    let db = open_in(&dir, TestClock::default());
    let drift = resident_bytes().saturating_sub(before);
    assert!(db.is_hydrated(), "a store at the ceiling is loaded");
    assert_eq!(db.committed_len(), Some(HYDRATE_CAP as usize));
    assert_eq!(db.freq("soak0"), 1, "the loaded counts answer");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        drift <= HYDRATE_RSS_CEILING,
        "loading {HYDRATE_CAP} records added {drift} bytes"
    );
}
