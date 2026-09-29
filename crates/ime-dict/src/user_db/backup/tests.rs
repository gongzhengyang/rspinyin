//! Tests for the backup generations and the rollback they feed.
//!
//! Every test owns its database and its backup directory in a directory of its own under
//! the system temp directory, drives the store's clock through [`TestClock`], and passes
//! the backup stamp in as an argument, so nothing here depends on a real XDG directory, on
//! the wall clock, or on the state left by another test. The only clock a test cannot
//! inject is the one the recovery pass stamps a quarantine name with, and no assertion
//! here reads that name -- only whether a quarantine happened at all.

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use ime_types::UserFreqSource;

use super::super::export::set_export_limit;
use super::super::tests::{TestClock, db_path, open_in, temp_dir};
use super::super::{UserDb, inject_failure};
use super::*;

/// The stamp every test backs up at: 2023-11-14 22:13:20 UTC, so the generation names the
/// assertions expect are literals.
const STAMP: u64 = 1_700_000_000_000;

/// The keys the tests type, short and distinct.
const KEYS: [&str; 3] = ["ni'hao", "shi'jie", "zhong'guo"];

/// The policy a test drives: a backup directory beside the store, nothing else changed.
fn config(dir: &Path) -> BackupConfig {
    BackupConfig::new(backup_dir(dir))
}

/// A store holding [`KEYS`], flushed, with the backup directory not yet created.
fn seeded(dir: &Path) -> UserDb {
    let mut db = open_in(dir, TestClock::default());
    for key in KEYS {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing the fixture");
    db
}

/// The permission bits of a path.
fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("reading the metadata")
        .permissions()
        .mode()
        & 0o777
}

/// Replaces the store with bytes that are not a store, the way a crash does.
fn damage(dir: &Path) {
    let junk: Vec<u8> = (0..4096u32).map(|index| (index % 251) as u8).collect();
    std::fs::write(db_path(dir), &junk).expect("damaging the store");
}

/// Whether a quarantine file is present in `dir`.
fn quarantined(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt."))
}

/// The names of the files directly under `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

// ── the stamp codec ────────────────────────────────────────────────────────────────

#[test]
fn test_generation_name_renders_a_stamp_as_a_sortable_utc_name() {
    assert_eq!(generation_name(STAMP), "user-20231114-221320.tsv");
    assert_eq!(generation_name(0), "user-19700101-000000.tsv");
}

#[test]
fn test_backup_name_round_trips_through_the_stamp() {
    for stamp in [0u64, 1_000, 1_700_000_000_000, 2_000_000_000_000] {
        let name = generation_name(stamp);
        assert_eq!(stamp_of_name(&name), Some(stamp), "{name} lost its stamp");
    }
}

#[test]
fn test_backup_name_round_trips_across_a_leap_day() {
    // 2024-02-29 is a leap day, and it is the one day a hand-written calendar gets wrong.
    let name = generation_name(1_709_208_000_000);
    assert_eq!(name, "user-20240229-120000.tsv");
    assert_eq!(stamp_of_name(&name), Some(1_709_208_000_000));
}

#[test]
fn test_parse_backup_name_rejects_a_name_this_build_did_not_write() {
    for name in [
        "",
        "user-.tsv",
        "user-20231114-221320",
        "user-20231114-221320.txt",
        "notes-20231114-221320.tsv",
        "user-2023111-221320.tsv",
        "user-20231114-22132.tsv",
        "user-20231314-221320.tsv",
        "user-20231132-221320.tsv",
        "user-20231114-241320.tsv",
        "user-20231114-221360.tsv",
        "user-20231114-2213ab.tsv",
        "user-用户用户用-221320.tsv",
    ] {
        assert_eq!(stamp_of_name(name), None, "{name} is not a generation");
    }
    // A second generation claimed in the same second carries a suffix, and the stamp is
    // what both spellings share.
    assert_eq!(
        stamp_of_name("user-20231114-221320.1.tsv"),
        Some(STAMP),
        "a suffixed name is the same generation"
    );
}

// ── writing a generation ───────────────────────────────────────────────────────────

#[test]
fn test_run_backup_writes_a_generation_with_the_owner_only_modes() {
    let dir = temp_dir("backup-modes");
    let db = seeded(&dir);
    let cfg = config(&dir);

    let outcome = run_backup(&db, &cfg, STAMP).expect("backing up");
    assert_eq!(outcome, BackupOutcome::Written);
    assert_eq!(mode_of(&cfg.directory), 0o700, "the directory is private");

    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].taken_at_unix, STAMP);
    assert_eq!(listed[0].rows, 3, "one row per learned word");
    assert_eq!(mode_of(&listed[0].path), 0o600, "the generation is private");
    assert_eq!(
        listed[0].bytes,
        std::fs::metadata(&listed[0].path).expect("stat").len()
    );
    assert!(
        listed[0]
            .path
            .to_string_lossy()
            .ends_with("user-20231114-221320.tsv"),
        "the name carries the stamp: {}",
        listed[0].path.display()
    );
}

#[test]
fn test_run_backup_narrows_a_backup_directory_another_account_can_reach() {
    let dir = temp_dir("backup-tighten");
    let db = seeded(&dir);
    let cfg = config(&dir);
    std::fs::create_dir_all(&cfg.directory).expect("creating the directory");
    std::fs::set_permissions(&cfg.directory, std::fs::Permissions::from_mode(0o755))
        .expect("widening the mode");

    let outcome = run_backup(&db, &cfg, STAMP).expect("backing up");
    assert_eq!(outcome, BackupOutcome::Written);
    assert_eq!(mode_of(&cfg.directory), 0o700, "a wide mode is narrowed");
}

#[test]
fn test_run_backup_carries_a_record_the_flush_has_not_written() {
    let dir = temp_dir("backup-pending");
    let db = open_in(&dir, TestClock::default());
    db.record("fresh", 0);
    let cfg = config(&dir);

    run_backup(&db, &cfg, STAMP).expect("backing up");
    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed[0].rows, 1, "an unflushed word is part of the copy");
}

#[test]
fn test_run_backup_leaves_no_temporary_file_behind() {
    let dir = temp_dir("backup-atomic");
    let db = seeded(&dir);
    let cfg = config(&dir);

    run_backup(&db, &cfg, STAMP).expect("backing up");
    assert_eq!(
        names(&cfg.directory),
        vec![String::from("user-20231114-221320.tsv")],
        "the temporary was renamed onto the generation and nothing else is left"
    );
}

// ── the interval ───────────────────────────────────────────────────────────────────

#[test]
fn test_run_backup_skips_a_generation_that_is_not_due_yet() {
    let dir = temp_dir("backup-skip");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");

    let outcome = run_backup(&db, &cfg, STAMP + BACKUP_INTERVAL_MS - 1).expect("asking again");
    assert_eq!(outcome, BackupOutcome::Skipped);
    assert_eq!(
        list_backups(&cfg.directory).expect("listing").len(),
        1,
        "a skipped attempt writes nothing"
    );
}

#[test]
fn test_run_backup_writes_again_once_the_interval_has_passed() {
    let dir = temp_dir("backup-due");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");

    let outcome = run_backup(&db, &cfg, STAMP + BACKUP_INTERVAL_MS).expect("backing up again");
    assert_eq!(outcome, BackupOutcome::Written);
    assert_eq!(list_backups(&cfg.directory).expect("listing").len(), 2);
}

#[test]
fn test_run_backup_skips_when_the_newest_generation_is_stamped_ahead_of_the_clock() {
    let dir = temp_dir("backup-clock-back");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");

    // A clock that went backwards must not turn every idle window into a backup window.
    let outcome = run_backup(&db, &cfg, STAMP - BACKUP_INTERVAL_MS).expect("asking again");
    assert_eq!(outcome, BackupOutcome::Skipped);
}

// ── rotation ───────────────────────────────────────────────────────────────────────

#[test]
fn test_run_backup_rotates_oldest_first_and_keeps_the_configured_count() {
    let dir = temp_dir("backup-rotate");
    let db = seeded(&dir);
    let cfg = config(&dir).with_keep(3);
    for step in 0..4u64 {
        let stamp = STAMP + step * BACKUP_INTERVAL_MS;
        assert_eq!(
            run_backup(&db, &cfg, stamp).expect("backing up"),
            BackupOutcome::Written
        );
    }

    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed.len(), 3, "the surplus generation was removed");
    assert_eq!(listed[0].taken_at_unix, STAMP + 3 * BACKUP_INTERVAL_MS);
    assert_eq!(listed[1].taken_at_unix, STAMP + 2 * BACKUP_INTERVAL_MS);
    assert_eq!(
        listed[2].taken_at_unix,
        STAMP + BACKUP_INTERVAL_MS,
        "the oldest generation is the one that went"
    );
}

#[test]
fn test_run_backup_keeps_the_generation_it_just_wrote_when_only_one_is_kept() {
    let dir = temp_dir("backup-keep-one");
    let mut db = seeded(&dir);
    let cfg = config(&dir).with_keep(1);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    db.record("extra", 0);
    db.final_commit().expect("flushing");

    run_backup(&db, &cfg, STAMP + BACKUP_INTERVAL_MS).expect("backing up again");
    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].taken_at_unix, STAMP + BACKUP_INTERVAL_MS);
    assert_eq!(
        listed[0].rows, 4,
        "the survivor holds what was just written"
    );
}

#[test]
fn test_run_backup_keeps_the_older_generation_when_the_write_fails() {
    let dir = temp_dir("backup-write-order");
    let db = seeded(&dir);
    let cfg = config(&dir).with_keep(1);
    run_backup(&db, &cfg, STAMP).expect("backing up");

    // Block the next write by taking the temporary path it would use. The rotation runs
    // after the write, so a failed write must leave the previous generation exactly where
    // it was -- the other order would have deleted it first and left no copy at all.
    let next = STAMP + BACKUP_INTERVAL_MS;
    let blocked = crate::format::writer::temp_path(&cfg.directory.join(generation_name(next)));
    std::fs::create_dir(&blocked).expect("blocking the temporary path");

    let error = run_backup(&db, &cfg, next).expect_err("the write cannot land");
    assert!(
        error.to_string().starts_with("data/backup-failed"),
        "the diagnostic code names the failure: {error}"
    );
    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(
        listed.len(),
        1,
        "nothing was written and nothing was deleted"
    );
    assert_eq!(listed[0].taken_at_unix, STAMP);
}

#[test]
fn test_run_backup_clamps_a_keep_of_zero_to_one() {
    let dir = temp_dir("backup-keep-zero");
    let cfg = config(&dir).with_keep(0);
    assert_eq!(cfg.keep, 1, "a rotation that keeps nothing is not a policy");

    let db = seeded(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    assert_eq!(
        list_backups(&cfg.directory).expect("listing").len(),
        1,
        "the generation just written is not the one removed"
    );
}

// ── degradation ────────────────────────────────────────────────────────────────────

#[test]
fn test_run_backup_reports_disabled_when_the_user_turned_backups_off() {
    let dir = temp_dir("backup-disabled");
    let db = seeded(&dir);
    let cfg = BackupConfig::disabled(backup_dir(&dir));

    let outcome = run_backup(&db, &cfg, STAMP).expect("a disabled backup is not a failure");
    assert_eq!(outcome, BackupOutcome::Disabled);
    assert!(
        !cfg.directory.exists(),
        "a disabled backup does not even create its directory"
    );
}

#[test]
fn test_run_backup_reports_readonly_when_the_store_is_read_only() {
    let dir = temp_dir("backup-store-readonly");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    inject_failure();
    let _ = db.final_commit().expect_err("the injected flush fails");
    assert!(db.is_readonly(), "the store degraded");
    let cfg = config(&dir);

    let outcome = run_backup(&db, &cfg, STAMP).expect("the degradation is not an error");
    assert!(
        matches!(outcome, BackupOutcome::Readonly { .. }),
        "{outcome:?}"
    );
    assert!(!cfg.directory.exists(), "nothing is written");
}

#[test]
fn test_run_backup_reports_readonly_when_the_backup_directory_is_a_file() {
    let dir = temp_dir("backup-dir-is-file");
    let db = seeded(&dir);
    let cfg = config(&dir);
    std::fs::write(&cfg.directory, b"not a directory").expect("taking the directory's place");

    let outcome = run_backup(&db, &cfg, STAMP).expect("the degradation is not an error");
    match outcome {
        BackupOutcome::Readonly { reason } => assert!(!reason.is_empty()),
        other => panic!("expected a read-only degradation, got {other:?}"),
    }
    assert!(cfg.directory.is_file(), "the obstruction is left alone");
}

#[test]
fn test_run_backup_refuses_a_symlinked_backup_directory() {
    let dir = temp_dir("backup-dir-is-link");
    let db = seeded(&dir);
    let cfg = config(&dir);
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("creating the target");
    std::os::unix::fs::symlink(&elsewhere, &cfg.directory).expect("placing the link");

    let outcome = run_backup(&db, &cfg, STAMP).expect("the refusal is not an error");
    assert!(
        matches!(outcome, BackupOutcome::Readonly { .. }),
        "{outcome:?}"
    );
    assert!(
        names(&elsewhere).is_empty(),
        "nothing is written through the link"
    );
}

#[test]
fn test_run_backup_reports_failed_when_the_store_cannot_be_exported() {
    let dir = temp_dir("backup-export-refused");
    let db = seeded(&dir);
    let cfg = config(&dir);
    // The store's own ceiling, moved down to less than the document's header, so the
    // refusal is the real one and not a mock.
    set_export_limit(16);

    let outcome = run_backup(&db, &cfg, STAMP).expect("the refusal is not an error");
    match outcome {
        BackupOutcome::Failed { reason } => {
            assert!(
                reason.starts_with("dict/export-too-large"),
                "the reason carries the store's own code: {reason}"
            );
        }
        other => panic!("expected a failed backup, got {other:?}"),
    }
    assert!(!cfg.directory.exists(), "nothing is written");
}

// ── listing ────────────────────────────────────────────────────────────────────────

#[test]
fn test_list_backups_orders_newest_first_and_ignores_foreign_files() {
    let dir = temp_dir("backup-listing");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    run_backup(&db, &cfg, STAMP + BACKUP_INTERVAL_MS).expect("backing up again");
    std::fs::write(cfg.directory.join("notes.txt"), b"the user's own note").expect("a note");
    std::fs::write(cfg.directory.join("user-notastamp.tsv"), b"# ours?\n").expect("a stray");

    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed.len(), 2, "only generations are listed");
    assert_eq!(listed[0].taken_at_unix, STAMP + BACKUP_INTERVAL_MS);
    assert_eq!(listed[1].taken_at_unix, STAMP);
}

#[test]
fn test_list_backups_ignores_a_symlinked_generation() {
    let dir = temp_dir("backup-listing-link");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    let target = cfg.directory.join("user-20231114-221320.tsv");
    let link = cfg.directory.join("user-20231115-221320.tsv");
    std::os::unix::fs::symlink(&target, &link).expect("placing the link");

    let listed = list_backups(&cfg.directory).expect("listing");
    assert_eq!(listed.len(), 1, "a link is not a generation");
    assert_eq!(listed[0].path, target);
}

#[test]
fn test_list_backups_of_a_missing_directory_is_empty() {
    let dir = temp_dir("backup-listing-missing");

    let listed = list_backups(&backup_dir(&dir)).expect("a missing directory is not an error");
    assert!(listed.is_empty());
}

// ── rollback ───────────────────────────────────────────────────────────────────────

#[test]
fn test_backup_round_trips_into_an_empty_store() {
    let source_dir = temp_dir("backup-round-trip-source");
    let mut source = seeded(&source_dir);
    let before = source.record_count().expect("counting the source");
    source.final_commit().expect("flushing");
    let cfg = config(&source_dir);
    run_backup(&source, &cfg, STAMP).expect("backing up");
    let generation = list_backups(&cfg.directory).expect("listing").remove(0);

    let target_dir = temp_dir("backup-round-trip-target");
    let target = open_in(&target_dir, TestClock::default());
    let mut document = File::open(&generation.path).expect("opening the generation");
    let report = target.import_tsv(&mut document).expect("importing");

    assert!(report.recognised, "the document is this format");
    assert_eq!(report.rejected, 0, "every row was accepted");
    assert_eq!(
        target.record_count().expect("counting the target"),
        before,
        "the copy holds the whole store"
    );
    assert_eq!(target.freq("ni'hao"), 1, "and the counts survived");
}

#[test]
fn test_recover_user_db_with_backup_leaves_a_healthy_store_alone() {
    let dir = temp_dir("restore-healthy");
    let db = seeded(&dir);
    drop(db);

    let outcome =
        recover_user_db_with_backup(&db_path(&dir), &[]).expect("the pass reports the truth");
    assert_eq!(outcome, RestoreOutcome::Healthy);
    assert!(!quarantined(&dir), "a usable store is not moved aside");
    assert_eq!(
        names(&dir),
        vec![String::from("user.redb")],
        "nothing else was created"
    );
}

#[test]
fn test_recover_user_db_with_backup_restores_the_newest_generation() {
    let dir = temp_dir("restore-newest");
    let mut db = seeded(&dir);
    let before = db.record_count().expect("counting the store");
    db.final_commit().expect("flushing");
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    drop(db);

    damage(&dir);
    let backups = list_backups(&cfg.directory).expect("listing");
    let outcome = recover_user_db_with_backup(&db_path(&dir), &backups).expect("restoring");
    assert_eq!(
        outcome,
        RestoreOutcome::RestoredFromBackup {
            from_unix: STAMP,
            rows: before,
        }
    );
    assert!(
        quarantined(&dir),
        "the damaged file was kept, never deleted"
    );

    let restored = UserDb::open(db_path(&dir)).expect("opening the restored store");
    assert_eq!(restored.record_count().expect("counting"), before);
    assert_eq!(
        restored.freq("ni'hao"),
        1,
        "the counts came back with the rows"
    );
}

#[test]
fn test_recover_user_db_with_backup_skips_a_generation_that_does_not_import() {
    let dir = temp_dir("restore-skip-bad");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    // A newer generation that carries the marker and a row the format cannot read: it is
    // listed like any other, and the import is what refuses it.
    let damaged = cfg
        .directory
        .join(generation_name(STAMP + BACKUP_INTERVAL_MS));
    std::fs::write(
        &damaged,
        b"# rspinyin user dictionary export v1\nnot a row\n",
    )
    .expect("planting a damaged generation");
    drop(db);

    damage(&dir);
    let backups = list_backups(&cfg.directory).expect("listing");
    assert_eq!(backups.len(), 2, "both generations are listed");
    let outcome = recover_user_db_with_backup(&db_path(&dir), &backups).expect("restoring");
    assert_eq!(
        outcome,
        RestoreOutcome::RestoredFromBackup {
            from_unix: STAMP,
            rows: 3,
        },
        "the older generation was tried after the newer one was refused"
    );
    let restored = UserDb::open(db_path(&dir)).expect("opening the restored store");
    assert_eq!(restored.record_count().expect("counting"), 3);
}

#[test]
fn test_recover_user_db_with_backup_orders_the_generations_it_is_given() {
    let dir = temp_dir("restore-order");
    let mut db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    db.record("extra", 0);
    db.final_commit().expect("flushing");
    run_backup(&db, &cfg, STAMP + BACKUP_INTERVAL_MS).expect("backing up again");
    drop(db);

    damage(&dir);
    let newest_first = list_backups(&cfg.directory).expect("listing");
    // Handed over oldest-first on purpose: the function sorts, so the order it is called
    // with cannot decide which generation the user gets back.
    let oldest_first: Vec<BackupFile> = newest_first.into_iter().rev().collect();
    let outcome = recover_user_db_with_backup(&db_path(&dir), &oldest_first).expect("restoring");
    assert_eq!(
        outcome,
        RestoreOutcome::RestoredFromBackup {
            from_unix: STAMP + BACKUP_INTERVAL_MS,
            rows: 4,
        }
    );
}

#[test]
fn test_recover_user_db_with_backup_rebuilds_empty_when_there_is_no_backup() {
    let dir = temp_dir("restore-no-backup");
    let db = seeded(&dir);
    drop(db);
    damage(&dir);

    let outcome = recover_user_db_with_backup(&db_path(&dir), &[]).expect("rebuilding");
    assert_eq!(outcome, RestoreOutcome::RebuiltEmpty);
    assert!(quarantined(&dir), "the damaged file is kept");
    let rebuilt = UserDb::open(db_path(&dir)).expect("the store is usable again");
    assert_eq!(rebuilt.record_count().expect("counting"), 0);
}

#[test]
fn test_recover_user_db_with_backup_reports_failure_when_no_generation_imports() {
    let dir = temp_dir("restore-all-bad");
    let db = seeded(&dir);
    let cfg = config(&dir);
    run_backup(&db, &cfg, STAMP).expect("backing up");
    let generation = cfg.directory.join("user-20231114-221320.tsv");
    std::fs::write(
        &generation,
        b"# rspinyin user dictionary export v1\nnot a row\n",
    )
    .expect("ruining the generation");
    drop(db);
    damage(&dir);

    let backups = list_backups(&cfg.directory).expect("listing");
    let error =
        recover_user_db_with_backup(&db_path(&dir), &backups).expect_err("nothing could be read");
    assert!(
        error.to_string().starts_with("data/backup-failed"),
        "the diagnostic code names the failure: {error}"
    );
    let rebuilt = UserDb::open(db_path(&dir)).expect("the store is left usable");
    assert_eq!(
        rebuilt.record_count().expect("counting"),
        0,
        "the user keeps a working input method with no words"
    );
}
