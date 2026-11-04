//! Tests for the negative-feedback penalty: the store half of the demotion.
//!
//! What is pinned here is the arithmetic and the durability of the trust level: one press
//! is one right shift of the answer a read produces, a normal commit of the word lowers
//! the level again, the level survives a close and a reopen, a forgotten or evicted word
//! takes its penalty with it, and a store written without the penalty table reads the
//! default of zero the additive schema promises. Everything runs on the module's own
//! fixture -- a per-test directory and a clock the test drives -- so no test here touches
//! a real XDG directory or the wall clock, and every number asserted below is exact
//! because the clock stands still between the steps that need it to.

use ime_types::UserFreqSource;

use super::tests::{TestClock, db_path, open_in, temp_dir};
use super::*;

/// The one key every test records, eight times: a baseline the shifts read against.
const KEY: &str = "ni'hao";

/// The commits `KEY` is seeded with, before anything penalizes it.
const SEED_COMMITS: u32 = 8;

/// Records `KEY` `SEED_COMMITS` times and flushes, leaving an un-penalized baseline.
fn seeded(dir: &std::path::Path) -> UserDb {
    let mut db = open_in(dir, TestClock::default());
    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }
    db.final_commit().expect("flushing the seed records");
    db
}

#[test]
fn test_reduce_frequency_lowers_the_answer_one_shift_per_press() {
    let dir = temp_dir("penalty-shift");
    let db = seeded(&dir);
    assert_eq!(
        db.freq(KEY),
        SEED_COMMITS,
        "the seeded baseline is un-penalized"
    );

    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.penalty_of(KEY), 1);
    assert_eq!(
        db.freq(KEY),
        SEED_COMMITS >> 1,
        "one level is one right shift"
    );

    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.freq(KEY), SEED_COMMITS >> 2, "presses compound");
}

#[test]
fn test_reduce_frequency_on_an_unknown_word_answers_false_and_changes_nothing() {
    let dir = temp_dir("penalty-unknown");
    let db = seeded(&dir);

    assert!(
        !db.reduce_frequency("never-learned"),
        "a penalty is a statement about a record, and there is none"
    );
    assert_eq!(db.freq("never-learned"), 0);
    assert_eq!(db.pending_penalties_len(), 0, "nothing was queued");
}

#[test]
fn test_reduce_frequency_caps_at_the_last_expressible_shift() {
    let dir = temp_dir("penalty-cap");
    let db = seeded(&dir);

    for _ in 0..(u16::from(MAX_PENALTY) + 4) {
        assert!(db.reduce_frequency(KEY));
    }
    assert_eq!(
        db.penalty_of(KEY),
        MAX_PENALTY,
        "the cap is the last shift a u32 answer can express"
    );
    assert_eq!(
        db.freq(KEY),
        SEED_COMMITS
            .checked_shr(u32::from(MAX_PENALTY))
            .unwrap_or(0),
        "the answer at the cap is the shift of the baseline, no further"
    );
}

#[test]
fn test_commit_decays_the_penalty_one_level_per_commit() {
    let dir = temp_dir("penalty-decay");
    let db = seeded(&dir);

    assert!(db.reduce_frequency(KEY));
    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.freq(KEY), SEED_COMMITS >> 2);

    // One normal commit: positive evidence about the spelling, worth one level of trust.
    // The answer folds the unflushed delta over the committed record, so the exact values
    // are the fold's own -- what is pinned is the level, and that the answer moves back up.
    db.record(KEY, 1);
    assert_eq!(db.penalty_of(KEY), 1);
    let after_one = db.freq(KEY);
    assert!(
        after_one > SEED_COMMITS >> 2,
        "the first commit lifted the answer: {after_one}"
    );

    db.record(KEY, 1);
    assert_eq!(
        db.penalty_of(KEY),
        0,
        "the second commit recovered the word fully"
    );
    assert!(
        db.freq(KEY) > after_one,
        "the answer keeps climbing as the levels go"
    );
}

#[test]
fn test_commit_of_an_unpenalized_word_does_not_queue_a_penalty_row() {
    let dir = temp_dir("penalty-clean-record");
    let mut db = open_in(&dir, TestClock::default());

    db.record(KEY, 1);
    db.final_commit().expect("flushing");
    db.record(KEY, 1);

    assert_eq!(db.penalty_of(KEY), 0);
    assert_eq!(
        db.pending_penalties_len(),
        0,
        "the record path costs nothing for a word the user never penalized"
    );
}

#[test]
fn test_penalty_survives_a_close_and_a_reopen() {
    let dir = temp_dir("penalty-reopen");
    let mut db = seeded(&dir);
    assert!(db.reduce_frequency(KEY));
    db.final_commit().expect("flushing the penalty row");
    drop(db);

    let reopened = open_in(&dir, TestClock::default());
    assert_eq!(reopened.penalty_of(KEY), 1, "the level is store state");
    assert_eq!(
        reopened.freq(KEY),
        SEED_COMMITS >> 1,
        "and the read paths apply it after the reopen"
    );
}

#[test]
fn test_penalty_write_rides_the_flush_the_typing_earns() {
    let dir = temp_dir("penalty-batched");
    let mut db = open_in(&dir, TestClock::default());
    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }

    // No flush has run: the batch trigger is far away and the clock is still, so the
    // records and the penalty wait in the same window. The read answers from memory
    // either way, which is what makes the press visible to the next decode at once.
    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.freq(KEY), SEED_COMMITS >> 1);
    assert_eq!(
        db.record_count().expect("counting the file"),
        0,
        "nothing reached the disk: the penalty asked for no flush of its own"
    );
    assert_eq!(
        db.pending_penalties_len(),
        1,
        "the row waits with the records"
    );

    // The owner's own flush lands both in one transaction.
    db.final_commit().expect("flushing the window");
    assert_eq!(db.record_count().expect("counting the file"), 1);
    drop(db);
    let reopened = open_in(&dir, TestClock::default());
    assert_eq!(reopened.freq(KEY), SEED_COMMITS >> 1, "the row is durable");
}

#[test]
fn test_forget_takes_the_penalty_with_the_record() {
    let dir = temp_dir("penalty-forget");
    let mut db = seeded(&dir);
    assert!(db.reduce_frequency(KEY));
    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.freq(KEY), SEED_COMMITS >> 2);

    assert!(db.forget(KEY), "the word was there to remove");
    assert_eq!(db.freq(KEY), 0, "the tombstone outranks everything");
    assert_eq!(db.penalty_of(KEY), 0, "the level left with the word");

    // The word the user re-learns starts clean: a stale level would shadow the first
    // record the new commits write, ranking it with a distrust aimed at its predecessor.
    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }
    db.final_commit().expect("flushing the re-learned word");
    assert_eq!(db.penalty_of(KEY), 0);
    assert_eq!(
        db.freq(KEY),
        2 * SEED_COMMITS,
        "un-penalized: the baseline of the re-learned word is its own"
    );
}

#[test]
fn test_eviction_takes_the_penalty_row_with_the_record() {
    let dir = temp_dir("penalty-evict");
    let mut db = open_in(&dir, TestClock::default());
    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }
    db.record("shi'jie", 1);
    db.final_commit().expect("flushing");
    for _ in 0..3 {
        assert!(db.reduce_frequency(KEY));
    }
    db.final_commit().expect("flushing the penalty row");
    assert_eq!(db.penalty_of(KEY), 3);

    // The older record is the one evicted: `KEY` was written first, so its tick is the
    // smaller one, and the sweep takes exactly the target it was given.
    let evicted = db.evict_oldest(50).expect("sweeping");
    assert_eq!(evicted, 1);
    assert_eq!(db.penalty_of(KEY), 0, "the row left with the record");

    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }
    db.final_commit().expect("flushing the re-learned word");
    assert_eq!(
        db.freq(KEY),
        SEED_COMMITS,
        "un-penalized: a re-learned word is not the one the user distrusted"
    );
}

#[test]
fn test_a_pinned_word_keeps_its_capped_answer_despite_a_penalty() {
    let dir = temp_dir("penalty-pinned");
    let db = seeded(&dir);
    db.set_pinned("ni", Some(KEY)).expect("pinning the word");
    assert_eq!(db.freq(KEY), u32::MAX, "the pin is what the ranking reads");

    assert!(db.reduce_frequency(KEY));
    assert_eq!(db.penalty_of(KEY), 1, "the press was taken");
    assert_eq!(
        db.freq(KEY),
        u32::MAX,
        "but the pin outranks it: the user asked for the word to stay"
    );
}

#[test]
fn test_a_store_without_the_penalty_table_reads_the_default_of_zero() {
    let dir = temp_dir("penalty-old-store");
    let mut db = open_in(&dir, TestClock::default());
    for _ in 0..SEED_COMMITS {
        db.record(KEY, 1);
    }
    db.final_commit().expect("flushing");
    drop(db);

    // Stand in for a file an older build wrote: the additive schema promises that a build
    // without the negative-feedback work never names the table, and `redb` keeps a table
    // an open does not name -- so dropping the row store here is exactly the file such a
    // build would leave behind. The handle is dropped before the reopen, because `redb`
    // refuses a second handle on one path.
    {
        let database = redb::Database::create(db_path(&dir)).expect("reopening the file");
        let txn = database.begin_write().expect("beginning the drop");
        txn.delete_table(USER_PENALTIES)
            .expect("dropping the penalty table");
        txn.commit().expect("committing the drop");
    }

    let reopened = open_in(&dir, TestClock::default());
    assert_eq!(
        reopened.penalty_of(KEY),
        0,
        "an old store has no levels to read, and zero is the default"
    );
    assert_eq!(
        reopened.freq(KEY),
        SEED_COMMITS,
        "the answer is the un-penalized one the old build wrote"
    );
}
