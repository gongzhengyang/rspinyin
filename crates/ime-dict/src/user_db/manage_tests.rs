//! Tests for the management primitives: forgetting one word, and paging through them.
//!
//! Every test owns its database in a directory of its own under the system temp directory
//! and drives the clock through [`TestClock`], so nothing here depends on a real XDG
//! directory, on the wall clock, or on the state left by another test.

use super::tests::{KEYS, TestClock, db_path, open_in, temp_dir};
use super::*;

/// Records `keys` in order, moving the clock a second on before each one.
///
/// The stamp is what the enumeration orders by, so a test that wants a known order has to
/// give the records distinct ones.
fn record_all(db: &UserDb, clock: &TestClock, keys: &[&str]) {
    for key in keys {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
}

/// The keys of a page, in the order the page holds them.
fn keys_of(rows: &[UserRecord]) -> Vec<&str> {
    rows.iter().map(|row| row.key.as_str()).collect()
}

#[test]
fn test_forget_removes_the_record_and_reports_it() {
    let dir = temp_dir("forget-removed");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    record_all(&db, &clock, &KEYS);
    db.final_commit().expect("flushing");
    assert_eq!(db.record_count().expect("counting"), KEYS.len() as u64);

    assert_eq!(db.forget_with_outcome(KEYS[0]), ForgetOutcome::Removed);
    assert_eq!(db.freq(KEYS[0]), 0, "the word leaves the ranking at once");
    assert_eq!(
        db.record_count().expect("counting"),
        KEYS.len() as u64,
        "and the file is untouched until the flush that carries the removal"
    );
    db.final_commit().expect("flushing the removal");
    assert_eq!(db.record_count().expect("counting"), KEYS.len() as u64 - 1);
    assert_eq!(db.pending_removed_len(), 0, "the tombstone was drained");
    drop(db);

    let reopened = open_in(&dir, TestClock::default());
    assert_eq!(
        reopened.freq(KEYS[0]),
        0,
        "the removal survived the restart"
    );
    assert_eq!(
        reopened.record_count().expect("counting"),
        KEYS.len() as u64 - 1
    );
    for key in &KEYS[1..] {
        assert_eq!(reopened.freq(key), 1, "{key} was not touched");
    }
}

#[test]
fn test_forget_trait_method_reports_whether_a_record_went_away() {
    let dir = temp_dir("forget-bool");
    let mut db = open_in(&dir, TestClock::default());
    db.record(KEYS[0], 0);
    db.final_commit().expect("flushing");

    assert!(db.forget(KEYS[0]), "a record was there and it went");
    assert!(
        !db.forget("never-typed"),
        "there was nothing under this key"
    );
    assert_eq!(db.freq(KEYS[0]), 0, "and the first one is gone");
}

#[test]
fn test_forget_of_an_unknown_key_reports_not_found_and_changes_nothing() {
    let dir = temp_dir("forget-missing");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    record_all(&db, &clock, &KEYS);
    db.final_commit().expect("flushing");

    assert_eq!(
        db.forget_with_outcome("never-typed"),
        ForgetOutcome::NotFound
    );
    db.final_commit().expect("flushing");
    assert_eq!(
        db.record_count().expect("counting"),
        KEYS.len() as u64,
        "a key that was never learned removes nothing"
    );
    for key in KEYS {
        assert_eq!(db.freq(key), 1, "{key} is untouched");
    }
}

#[test]
fn test_forget_of_a_pinned_word_is_refused() {
    let dir = temp_dir("forget-pinned");
    let mut db = open_in(&dir, TestClock::default());
    let document = concat!(
        "# rspinyin user dictionary export v1\n",
        "# Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned\n",
        "yinhang\t7\t1700000000000\t1600000000000\t1\n"
    );
    let report = db
        .import_tsv(&mut document.as_bytes())
        .expect("importing a pinned word");
    assert_eq!(report.applied, 1);

    assert_eq!(db.forget_with_outcome("yinhang"), ForgetOutcome::Pinned);
    assert_eq!(db.freq("yinhang"), 7, "the record is still there");
    assert!(
        !db.forget("yinhang"),
        "the frozen method reports the refusal too"
    );
    db.final_commit().expect("flushing");
    assert_eq!(db.record_count().expect("counting"), 1);
}

#[test]
fn test_forget_on_a_readonly_store_reports_readonly() {
    let dir = temp_dir("forget-readonly");
    let mut db = open_in(&dir, TestClock::default());
    db.record(KEYS[0], 0);
    db.final_commit().expect("flushing");
    // Something has to be waiting. A flush with nothing to write is a no-op that returns
    // before the write path, which is where the failure is injected -- so arming the
    // failure against an empty batch would leave the store writable and prove nothing.
    db.record("second", 0);
    inject_failure();
    let _ = db.final_commit().expect_err("the injected flush fails");
    assert!(db.is_readonly(), "the store degraded");

    db.record("second", 0);
    assert_eq!(db.forget_with_outcome(KEYS[0]), ForgetOutcome::Readonly);
    assert_eq!(
        db.forget_with_outcome("second"),
        ForgetOutcome::Readonly,
        "even a key the store never flushed is not removed"
    );
    assert_eq!(db.freq(KEYS[0]), 1, "nothing was removed");
    assert_eq!(db.pending_removed_len(), 0, "and no tombstone was left");
}

#[test]
fn test_forget_batches_into_the_pending_state() {
    let dir = temp_dir("forget-batch");
    let mut db = open_in(&dir, TestClock::default());
    db.record(KEYS[0], 0);
    db.final_commit().expect("flushing");

    db.forget(KEYS[0]);
    db.forget(KEYS[0]);
    assert_eq!(
        db.pending_removed_len(),
        1,
        "a key forgotten twice is one tombstone"
    );
    assert_eq!(
        db.record_count().expect("counting"),
        1,
        "no write transaction ran on the key path"
    );
}

#[test]
fn test_record_after_forget_takes_the_tombstone_back() {
    let dir = temp_dir("forget-undone");
    let mut db = open_in(&dir, TestClock::default());
    db.record(KEYS[0], 0);
    db.final_commit().expect("flushing");

    db.forget(KEYS[0]);
    assert_eq!(db.freq(KEYS[0]), 0);
    db.record(KEYS[0], 0);
    assert_eq!(db.pending_removed_len(), 0, "the tombstone is gone");
    assert_eq!(db.freq(KEYS[0]), 2, "and the word is learned again");

    db.final_commit().expect("flushing");
    assert_eq!(db.freq(KEYS[0]), 2);
    assert_eq!(
        db.record_count().expect("counting"),
        1,
        "the record survived its own removal"
    );
}

#[test]
fn test_list_words_pages_by_offset_and_limit() {
    let dir = temp_dir("list-pages");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    let keys = ["w0", "w1", "w2", "w3", "w4"];
    record_all(&db, &clock, &keys);
    db.final_commit().expect("flushing");

    let first = db.list_words(0, 2).expect("listing");
    assert_eq!(keys_of(&first), ["w4", "w3"], "most recently used first");
    let second = db.list_words(2, 2).expect("listing");
    assert_eq!(keys_of(&second), ["w2", "w1"]);
    let last = db.list_words(4, 2).expect("listing");
    assert_eq!(keys_of(&last), ["w0"], "the last page is short");
    assert!(
        db.list_words(5, 2).expect("listing").is_empty(),
        "an offset past the end is an empty page"
    );
    assert!(
        db.list_words(0, 0).expect("listing").is_empty(),
        "a limit of zero asks for nothing"
    );
    assert!(
        db.list_words(u64::MAX, 2).expect("listing").is_empty(),
        "an offset that cannot be an index is still an empty page"
    );
}

#[test]
fn test_list_words_clamps_the_limit_to_the_page_ceiling() {
    let dir = temp_dir("list-clamp");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    let keys: Vec<String> = (0..u32::from(MAX_LIST_LIMIT) + 50)
        .map(|index| format!("w{index}"))
        .collect();
    for key in &keys {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");

    let page = db.list_words(0, u16::MAX).expect("listing");
    assert_eq!(
        page.len(),
        usize::from(MAX_LIST_LIMIT),
        "the ceiling bounds the page, not the store"
    );
    assert_eq!(
        page.first().map(|row| row.key.as_str()),
        keys.last().map(String::as_str),
        "and it is the head of the order"
    );
}

#[test]
fn test_list_words_orders_by_use_and_breaks_a_tie_by_key() {
    let dir = temp_dir("list-tie");
    let mut db = open_in(&dir, TestClock::default());
    // The clock never moves, so every record carries the same stamp and only the key can
    // decide the order.
    for key in ["c", "a", "b"] {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");

    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(keys_of(&rows), ["a", "b", "c"]);
    assert_eq!(
        rows.iter()
            .map(|row| row.last_used_unix)
            .collect::<Vec<_>>(),
        [0, 0, 0],
        "the fixture really does share one stamp"
    );
}

#[test]
fn test_list_words_shows_a_record_the_flush_has_not_written() {
    let dir = temp_dir("list-pending");
    let mut db = open_in(&dir, TestClock::default());
    db.record("fresh", 0);
    assert_eq!(db.pending_len(), 1, "the record is still in memory");

    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(keys_of(&rows), ["fresh"], "an unflushed word is a word");
    assert_eq!(rows[0].weight, 1);
    assert_eq!(rows[0].created_unix, 0, "it has no stored stamp yet");

    db.final_commit().expect("flushing");
    let stored = db.list_words(0, 10).expect("listing");
    assert_eq!(keys_of(&stored), ["fresh"]);
    assert_eq!(
        stored[0].weight, 1,
        "the flush adopts the delta instead of counting it twice"
    );
}

#[test]
fn test_list_words_drops_a_forgotten_word_before_the_flush() {
    let dir = temp_dir("list-forgotten");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    record_all(&db, &clock, &["kept", "gone"]);
    db.final_commit().expect("flushing");

    db.forget("gone");
    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(
        keys_of(&rows),
        ["kept"],
        "the tombstone hides the word the file still holds"
    );
    assert_eq!(db.record_count().expect("counting"), 2);
    db.final_commit().expect("flushing the removal");
    assert_eq!(keys_of(&db.list_words(0, 10).expect("listing")), ["kept"]);
    assert_eq!(db.record_count().expect("counting"), 1);
}

#[test]
fn test_list_words_reads_an_old_record_as_unpinned_and_unstamped() {
    let dir = temp_dir("list-legacy");
    let db = open_in(&dir, TestClock::default());
    // A row written the way the first release wrote one: an entry in `user_words` and no
    // metadata row at all.
    {
        let txn = db.inner.db.begin_write().expect("writing");
        {
            let mut words = txn.open_table(USER_WORDS).expect("opening the table");
            words
                .insert("legacy", (3u32, 1_700_000_000_000u64))
                .expect("inserting the old row");
        }
        txn.commit().expect("committing");
    }

    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(keys_of(&rows), ["legacy"]);
    assert_eq!(rows[0].weight, 3);
    assert_eq!(rows[0].last_used_unix, 1_700_000_000_000);
    assert_eq!(
        rows[0].created_unix, 0,
        "a row an older build wrote carries no creation time"
    );
    assert!(!rows[0].pinned, "and no pin");
    assert!(!db.is_readonly(), "the fixture store is a healthy one");
}

#[test]
fn test_list_words_answers_for_a_store_past_the_ceiling() {
    let dir = temp_dir("list-large");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    record_all(&db, &clock, &KEYS);
    db.final_commit().expect("flushing");
    drop(db);

    let opened = UserDb::open_with_hydrate_cap(db_path(&dir), Box::new(TestClock::default()), 1);
    let db = opened.expect("opening the user store");
    assert!(
        !db.is_hydrated(),
        "four records do not fit a ceiling of one"
    );
    let rows = db
        .list_words(0, 10)
        .expect("listing a store that was not loaded");
    assert_eq!(
        keys_of(&rows),
        ["pin'yin", "zhong'guo", "shi'jie", "ni'hao"],
        "the page comes from the file, in the order the stamps put it"
    );
}

#[test]
fn test_list_reports_unsupported_while_list_words_answers() {
    let dir = temp_dir("list-trait");
    let mut db = open_in(&dir, TestClock::default());
    db.record(KEYS[0], 0);
    db.final_commit().expect("flushing");

    let error = db
        .list(0, 10)
        .expect_err("the borrowed form is not served by a store whose keys change");
    assert!(matches!(error, ImeError::Unsupported));
    assert!(
        error.to_string().starts_with("dict/unsupported"),
        "the diagnostic carries the frozen code: {error}"
    );
    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(
        keys_of(&rows),
        [KEYS[0]],
        "the store can enumerate after all"
    );
}
