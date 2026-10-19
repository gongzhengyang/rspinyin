//! Schema-1 migration tests.

use super::*;

fn write_v1_store(path: &Path, rows: &[(&str, u32, u64)]) {
    let database = Database::create(path).expect("creating the v1 fixture");
    let mut txn = database.begin_write().expect("writing the v1 fixture");
    {
        let mut meta = txn.open_table(META).expect("opening the fixture metadata");
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION_V1)
            .expect("stamping schema 1");
        let mut words = txn
            .open_table(USER_WORDS)
            .expect("opening the fixture words");
        for (key, commits, last_used_ms) in rows {
            words
                .insert(*key, (*commits, *last_used_ms))
                .expect("inserting the fixture row");
        }
    }
    txn.set_durability(Durability::Immediate);
    txn.commit().expect("committing the v1 fixture");
}

#[test]
fn test_user_db_migrates_v1_rows_once_and_preserves_their_values() {
    let dir = temp_dir("migrate-v1");
    write_v1_store(&db_path(&dir), &[("old", 7, 1_000), ("newer", 3, 2_000)]);

    let db = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("opening the v1 fixture");
    assert!(db.schema_migrated(), "an upgrade open reports migration");
    assert!(!db.is_readonly(), "a successful migration stays writable");
    assert_eq!(db.record_count().expect("counting"), 2);
    assert_eq!(db.freq("old"), 7);
    assert_eq!(db.freq("newer"), 3);
    drop(db);

    let reopened = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("reopening the migrated store");
    assert!(
        !reopened.schema_migrated(),
        "the second open sees the current schema"
    );
    assert!(!reopened.is_readonly());
    assert_eq!(reopened.record_count().expect("counting"), 2);

    let txn = reopened
        .inner
        .db
        .begin_read()
        .expect("reading the migrated rows");
    assert!(
        txn.open_table(USER_WORDS).is_err(),
        "a completed upgrade drops the v1 table"
    );
    let words = txn.open_table(USER_WORDS_V2).expect("opening the v2 rows");
    let (commits, last_used_ms, tick, dee) = words
        .get("old")
        .expect("reading old")
        .expect("the row survived migration")
        .value();
    assert_eq!((commits, last_used_ms, tick), (7, 1_000, 0));
    assert_eq!(dee_q16(dee), 7 << 16);
}

#[test]
fn test_user_db_migrates_an_empty_v1_store_without_writes() {
    let dir = temp_dir("migrate-v1-empty");
    write_v1_store(&db_path(&dir), &[]);
    let db = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("opening the empty fixture");
    assert!(db.schema_migrated());
    assert!(!db.is_readonly());
    assert_eq!(db.record_count().expect("counting"), 0);
    assert_eq!(db.freq("never-typed"), 0);
}

#[test]
fn test_a_migrated_store_hydrates_and_lists_its_rows() {
    // The migration leaves behind every v2 table, the metadata one included: a store
    // missing it cannot load at open (a read transaction cannot create a table) and
    // cannot name a page of its rows, which is a silence no migration may leave.
    let dir = temp_dir("migrate-v1-hydrate");
    write_v1_store(&db_path(&dir), &[("old", 7, 1_000), ("newer", 3, 2_000)]);

    let db = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("opening the v1 fixture");
    assert!(
        db.is_hydrated(),
        "the migrated store loaded whole, as a fresh one would"
    );
    let rows = db
        .list_words(0, 10)
        .expect("listing a store the migration just made");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, "newer", "the most recently used first");
    assert_eq!(rows[1].weight, 7);
}

#[test]
fn test_user_db_failed_v1_migration_degrades_readonly_and_can_retry() {
    let dir = temp_dir("migrate-v1-failed");
    let backup = dir.join("user.redb.v1");
    write_v1_store(&db_path(&dir), &[("kept", 9, 3_000)]);
    std::fs::copy(db_path(&dir), &backup).expect("copying the v1 backup");

    inject_migration_failure();
    let degraded = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()))
        .expect("a failed migration still opens read-only");
    assert!(degraded.schema_migrated());
    assert!(degraded.is_readonly());
    assert_eq!(degraded.freq("kept"), 9);
    degraded.record("kept", 0);
    assert_eq!(
        degraded.pending_len(),
        0,
        "a degraded store records nothing"
    );
    drop(degraded);

    let mut restored = open_in(&dir, TestClock::default());
    assert!(
        restored.schema_migrated(),
        "the untouched file migrates on retry"
    );
    assert!(!restored.is_readonly());
    assert_eq!(restored.freq("kept"), 9);
    restored.record("kept", 0);
    restored.final_commit().expect("writing after recovery");
    drop(restored);

    let from_backup = dir.join("from-v1-backup.redb");
    std::fs::copy(&backup, &from_backup).expect("restoring the raw v1 backup");
    let db = UserDb::open_with(&from_backup, Box::new(TestClock::default()))
        .expect("a raw v1 backup is a valid migration source");
    assert!(db.schema_migrated());
    assert!(!db.is_readonly());
    assert_eq!(db.freq("kept"), 9);
}
