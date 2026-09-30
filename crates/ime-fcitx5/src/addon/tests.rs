//! Tests for the addon lifecycle and for the user store it adopts.
//!
//! The lifecycle's shape is pinned by name and by policy: which steps exist, in what
//! order, and which of them may decline the addon. The store is a file, so every test
//! that exercises the recovery, the flush or the backup writes one: each takes a
//! directory of its own under the system temporary directory, which is the shape the
//! user-database tests use for the same reason. No test here reads the process
//! environment, the wall clock or a real XDG directory, and the backup stamp is an
//! argument -- the one exception is the test that runs the real initialisation sequence,
//! and it asserts only what that sequence reports about itself.
//!
//! # Layout
//!
//! This file holds the sequence, the store and the fixtures the three of them share. What
//! a step does with a real file lives in `steps`, and what the packaging descriptors pin
//! lives in `packaging`; both are split out so that no one file carries every subject.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ime_config::{Config, ConfigStore};
use ime_dict::paths::READONLY_CODE;
use ime_dict::recover::USER_DB_RECOVERED_CODE;
use ime_dict::user_db::{
    BACKUP_FAILED_CODE, BACKUP_RESTORED_CODE, BackupConfig, BackupOutcome, RestoreOutcome, UserDb,
    backup_dir, list_backups,
};
use ime_types::{ImeError, SchemeId, UserFreqSource};

use crate::engine::{DigitZero, FlipSet, HighlightSet, RoutingConfig};

use super::config::{CONFIG, ROUTING, init_key_bindings, install_config_store, start_config_watch};
use super::user_store::{
    UserStore, backup_notice, install_user_store, recover_store, restore_notice,
    run_shutdown_backup, start_shutdown_backup, take_user_store,
};
use super::{
    DESTROY_BUDGET, INIT_BUDGET, INIT_STEPS, InitStep, lock, on_addon_destroy, on_addon_init,
    on_config_reload, routing_config, run_init_steps,
};

mod packaging;
mod steps;

/// The stamp the backup tests write at: 2023-11-14 22:13:20 UTC, so the generation names
/// the assertions expect are literals.
const STAMP: u64 = 1_700_000_000_000;

/// The keys the tests type, short and distinct.
const KEYS: [&str; 2] = ["ni'hao", "shi'jie"];

/// A directory of this test's own under the system temporary directory.
///
/// Emptied rather than merely created, so that a second run of the suite does not read
/// what the first one left behind. It is not removed afterwards: a test that fails leaves
/// its evidence in place.
fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-addon-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The path of the store inside a test directory.
fn db_path(dir: &Path) -> PathBuf {
    dir.join("user.redb")
}

/// A store holding [`KEYS`], flushed, so that a copy of it has rows to carry.
fn seeded(dir: &Path) -> UserDb {
    let mut db = UserDb::open(db_path(dir)).expect("opening the fixture store");
    for key in KEYS {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing the fixture");
    db
}

/// The store the shutdown path sees: [`seeded`], under a policy that is always due.
///
/// An interval of zero is "always due", which is what a test that must not wait a day
/// wants; the rotation and the interval policy themselves are `ime-dict`'s to test.
fn shutdown_store(dir: &Path) -> UserStore {
    UserStore {
        db: Mutex::new(seeded(dir)),
        backup: BackupConfig::new(backup_dir(dir)).with_interval_ms(0),
    }
}

/// The rows the store behind `store` holds.
fn rows_of(store: &UserStore) -> u64 {
    lock(&store.db).record_count().expect("counting the store")
}

/// Replaces the store with bytes that are not one, the way a crash does.
fn damage(dir: &Path) {
    let junk: Vec<u8> = (0..4096u32).map(|index| (index % 251) as u8).collect();
    fs::write(db_path(dir), &junk).expect("damaging the store");
}

/// Whether a quarantine file is present in `dir`.
fn quarantined(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt."))
}

// ── the initialisation sequence ────────────────────────────────────────────────────

/// A step that succeeds.
fn step_ok() -> Result<(), ImeError> {
    Ok(())
}

/// A step that fails. The variant is irrelevant — the policy only reads `is_fatal`.
fn step_fails() -> Result<(), ImeError> {
    Err(ImeError::UiChannelClosed)
}

#[test]
fn test_init_steps_pin_the_documented_lifecycle() {
    let names: Vec<&str> = INIT_STEPS.iter().map(|step| step.name).collect();
    assert_eq!(
        names,
        [
            "diagnostics",
            "data-dirs",
            "config",
            "key-bindings",
            "config-watch",
            "phrases",
            "store-recovery",
            "lexicon",
            "session-host",
        ]
    );
    let fatal: Vec<&str> = INIT_STEPS
        .iter()
        .filter(|step| step.is_fatal)
        .map(|step| step.name)
        .collect();
    assert_eq!(
        fatal,
        ["diagnostics"],
        "every other step must fail soft so input keeps working"
    );
}

#[test]
fn test_run_init_steps_keeps_the_addon_usable_when_a_non_fatal_step_fails() {
    let steps = [
        InitStep::new("diagnostics", true, step_ok),
        InitStep::new("lexicon", false, step_fails),
        InitStep::new("config", false, step_ok),
    ];
    assert!(
        run_init_steps(&steps),
        "a damaged subsystem must not decline the addon"
    );
}

#[test]
fn test_run_init_steps_declines_the_addon_after_a_fatal_failure() {
    static REACHED: AtomicUsize = AtomicUsize::new(0);
    fn later_step() -> Result<(), ImeError> {
        REACHED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    REACHED.store(0, Ordering::SeqCst);
    let steps = [
        InitStep::new("diagnostics", true, step_fails),
        InitStep::new("config", false, later_step),
    ];
    assert!(
        !run_init_steps(&steps),
        "a failed diagnostics step must decline the addon"
    );
    assert_eq!(
        REACHED.load(Ordering::SeqCst),
        0,
        "a declined addon must not run the steps after the failing one"
    );
}

#[test]
fn test_lifecycle_runs_and_shuts_down_cleanly() {
    assert!(
        on_addon_init(std::ptr::null_mut()),
        "no step but diagnostics is fatal, so initialisation must still succeed"
    );
    on_addon_destroy(std::ptr::null_mut());
}

#[test]
fn test_on_addon_init_stays_inside_the_load_budget() {
    // The synchronous sequence, measured as Fcitx5 runs it: the line the outcome is
    // reported with carries the same two numbers, and this is what fails when they cross.
    // The probe drives the real sequence, so it pays the real store open, the real phrase
    // read and the real dictionary load -- which is the point, because the dictionary's
    // checksum pass is the step this ceiling exists for.
    let started = Instant::now();
    assert!(on_addon_init(std::ptr::null_mut()));
    let elapsed = started.elapsed();
    on_addon_destroy(std::ptr::null_mut());

    // `BUDGET-LAT-05` is a release-build contract: it is measured by timing `fcitx5 -v`
    // against the shipped plugin, and the work below -- opening the user store, reading
    // the phrase document, creating the directories -- costs several times as much in an
    // unoptimised test binary as in the artifact the number is about. Comparing the two
    // would fail on a correct build, so the comparison is made only where the build is the
    // one the number describes; the sequence itself is exercised either way.
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        elapsed <= INIT_BUDGET,
        "the synchronous load took {elapsed:?}, past the {INIT_BUDGET:?} budget"
    );
}

#[test]
fn test_on_addon_destroy_stays_inside_the_shutdown_budget() {
    // The destructor with a store to release, which is the shape that costs anything: the
    // flush stays on this thread and the copy leaves it. The budget covers the flush, which
    // is why the worker is never joined — a shutdown that waited for a whole document's
    // write would be the regression this asserts against.
    let _ = take_user_store();
    let dir = scratch_dir("destroy-budget");
    install_user_store(shutdown_store(&dir));

    let started = Instant::now();
    on_addon_destroy(std::ptr::null_mut());
    let elapsed = started.elapsed();

    assert!(
        elapsed <= DESTROY_BUDGET,
        "the shutdown took {elapsed:?}, past the {DESTROY_BUDGET:?} budget"
    );
    assert!(
        take_user_store().is_none(),
        "the store the shutdown flushed is released"
    );
}

// ── the routing table the configuration projects to ────────────────────────────────
//
// The two routing steps and the reload handler behind them. A configuration store is a file,
// so every test here writes one into a directory of its own under the system temporary
// directory; nothing reads the process environment or a real XDG directory, and the stamp a
// corrupt file is quarantined under is an argument. The slots the steps fill are
// process-wide, so each test installs the store it wants rather than assuming one, and
// `nextest` gives a test a process of its own.

/// The document the routing tests load: every setting the projection reads, set away from
/// its default.
const ROUTING_DOCUMENT: &str = r#"
[keys]
digit_zero = "flip"
enter_commit_raw = true
flip_keys = ["minus", "page_up"]
highlight_keys = ["shift_tab"]

[ui]
client_preedit = true
max_per_row = 7

[scheme]
scheme = "xiaohe"
show_hint = true
"#;

/// The document [`ROUTING_DOCUMENT`] is edited into: a different binding table, the digit
/// handed back to the application, and the raw-input commit back off.
const EDITED_DOCUMENT: &str = r#"
[keys]
digit_zero = "passthrough"
flip_keys = ["equal"]

[ui]
client_preedit = true
max_per_row = 7

[scheme]
scheme = "xiaohe"
show_hint = true
"#;

/// A store loaded from `document`, written into `dir` first.
///
/// A document that raises a diagnostic fails the test rather than being tolerated: what
/// these tests are about is what a document the user could have written projects to.
fn config_store(dir: &Path, document: &str) -> ConfigStore {
    let path = dir.join("config.toml");
    fs::write(&path, document).expect("writing the fixture document");
    let (store, warnings) = ConfigStore::load_at(&path, STAMP);
    assert!(warnings.is_empty(), "{warnings:?}");
    store
}

#[test]
fn test_init_key_bindings_projects_the_configuration_in_force() {
    let dir = scratch_dir("key-bindings");
    install_config_store(config_store(&dir, ROUTING_DOCUMENT));

    init_key_bindings().expect("the step never fails");

    let routing = routing_config();
    assert!(routing.keys.enter_commit_raw);
    assert_eq!(routing.keys.digit_zero, DigitZero::Flip);
    assert_eq!(routing.keys.flip_keys, FlipSet::MINUS | FlipSet::PAGE_UP);
    assert_eq!(routing.keys.highlight_keys, HighlightSet::SHIFT_TAB);
    assert!(routing.client_preedit, "`[ui]` reached the executor");
    assert_eq!(routing.session.max_per_row, 7);
    assert_eq!(routing.session.scheme, SchemeId::XIAOHE);
    assert_eq!(
        routing.scheme_hint,
        Some("小鹤"),
        "the header names the layout the document declares"
    );
}

#[test]
fn test_init_key_bindings_falls_back_to_the_shipped_defaults_without_a_store() {
    // An environment with no configuration directory leaves the store uninstalled, and the
    // routing layer must still have a table: a fresh installation routes exactly as the
    // shipped document says, and a plugin that routed nothing would eat every key.
    let _ = lock(&CONFIG).take();
    *lock(&ROUTING) = None;

    init_key_bindings().expect("the step never fails");

    assert_eq!(routing_config(), RoutingConfig::default());
}

#[test]
fn test_start_config_watch_reports_success_with_and_without_a_store() {
    let _ = lock(&CONFIG).take();
    assert!(
        start_config_watch().is_ok(),
        "a subscription with nothing to re-read is not a failure"
    );

    let dir = scratch_dir("config-watch");
    install_config_store(config_store(&dir, ROUTING_DOCUMENT));
    assert!(
        start_config_watch().is_ok(),
        "a wired subscription is not a failure either"
    );
}

#[test]
fn test_on_config_reload_adopts_the_edited_document() {
    let dir = scratch_dir("config-reload");
    install_config_store(config_store(&dir, ROUTING_DOCUMENT));
    init_key_bindings().expect("the step never fails");
    assert_eq!(
        routing_config().keys.flip_keys,
        FlipSet::MINUS | FlipSet::PAGE_UP
    );

    // The user edits the document and the host asks for a reload.
    fs::write(dir.join("config.toml"), EDITED_DOCUMENT).expect("editing the document");
    let adopted = on_config_reload();

    assert_eq!(
        adopted.keys.flip_keys,
        FlipSet::EQUAL,
        "the new list is in force"
    );
    assert_eq!(adopted.keys.digit_zero, DigitZero::Passthrough);
    assert!(!adopted.keys.enter_commit_raw);
    assert_eq!(
        routing_config(),
        adopted,
        "the table the next router is built from is the adopted one"
    );
}

#[test]
fn test_on_config_reload_keeps_the_configuration_when_the_file_cannot_be_parsed() {
    // A reload may improve the configuration and nothing else: a file that is an edit in
    // progress must not be able to take a working configuration away.
    let dir = scratch_dir("config-reload-kept");
    install_config_store(config_store(&dir, ROUTING_DOCUMENT));
    init_key_bindings().expect("the step never fails");
    let before = routing_config();

    fs::write(dir.join("config.toml"), "this is not TOML at all\n").expect("breaking the file");
    let adopted = on_config_reload();

    assert_eq!(adopted, before, "the configuration in force is kept");
    assert_eq!(
        routing_config(),
        before,
        "and so is the table projected from it"
    );
}

#[test]
fn test_on_config_reload_without_a_store_keeps_the_table_in_force() {
    let _ = lock(&CONFIG).take();
    *lock(&ROUTING) = None;
    let before = routing_config();

    assert_eq!(on_config_reload(), before);
}

#[test]
fn test_the_addon_loads_when_the_configuration_cannot_be_parsed() {
    // The load path, not the reload path: a document that is not TOML at all is what a
    // half-written file or a hand edit gone wrong looks like at startup. The loader
    // answers it with the built-in defaults, the damaged file is moved aside rather than
    // deleted, and the addon still loads — a configuration problem must never cost the
    // user their input method.
    let dir = scratch_dir("config-corrupt");
    let path = dir.join("config.toml");
    fs::write(&path, "this is not TOML at all\n").expect("damaging the document");

    let (store, warnings) = ConfigStore::load_at(&path, STAMP);
    assert!(
        !warnings.is_empty(),
        "a document that cannot be parsed is reported, never silently replaced"
    );
    assert_eq!(
        **store.current(),
        Config::default(),
        "the built-in defaults are what a damaged document answers with"
    );
    assert!(
        path.with_file_name(format!("config.toml.bad.{STAMP}"))
            .exists(),
        "the damaged document is kept beside the new one, never deleted"
    );

    install_config_store(store);
    init_key_bindings().expect("the step never fails");
    assert_eq!(
        routing_config(),
        RoutingConfig::default(),
        "the shipped binding table is what the defaults project to"
    );
    assert!(
        on_addon_init(std::ptr::null_mut()),
        "a damaged configuration must not decline the addon"
    );
    on_addon_destroy(std::ptr::null_mut());
}

// ── the store the process runs on ──────────────────────────────────────────────────

#[test]
fn test_install_user_store_keeps_the_first_store_and_take_empties_the_slot() {
    // The slot is process-wide, so the test starts from a known state. `nextest` gives a
    // test a process of its own; the take is what makes a rerun of one binary agree too.
    let _ = take_user_store();
    let first = scratch_dir("install-first");
    let second = scratch_dir("install-second");

    install_user_store(shutdown_store(&first));
    install_user_store(shutdown_store(&second));

    let taken = take_user_store().expect("the first store was installed");
    assert_eq!(
        taken.backup.directory,
        backup_dir(&first),
        "a second install must not replace the store the engine may be reading through"
    );
    assert!(
        take_user_store().is_none(),
        "taking the store leaves the slot empty"
    );
}

#[test]
fn test_on_addon_destroy_takes_the_store_so_a_second_call_does_nothing() {
    let _ = take_user_store();
    let dir = scratch_dir("destroy");
    install_user_store(shutdown_store(&dir));

    on_addon_destroy(std::ptr::null_mut());

    assert!(
        take_user_store().is_none(),
        "the destructor releases the store it flushed and copied"
    );
    // The destructor is documented as safe to call when nothing was taken: the second
    // call must find no store to flush and none to copy.
    on_addon_destroy(std::ptr::null_mut());
    assert!(take_user_store().is_none());
}

// ── the shutdown backup ────────────────────────────────────────────────────────────

#[test]
fn test_run_shutdown_backup_writes_a_generation_of_the_adopted_store() {
    let dir = scratch_dir("backup-written");
    let store = shutdown_store(&dir);

    run_shutdown_backup(&store, STAMP);

    let listed = list_backups(&backup_dir(&dir)).expect("listing the generations");
    assert_eq!(listed.len(), 1, "the shutdown wrote one generation");
    assert_eq!(listed[0].taken_at_unix, STAMP);
    assert_eq!(
        listed[0].rows,
        u64::try_from(KEYS.len()).expect("a small count"),
        "one row per learned word"
    );
}

#[test]
fn test_start_shutdown_backup_writes_the_generation_on_a_worker_thread() {
    let dir = scratch_dir("backup-worker");
    let store = Arc::new(shutdown_store(&dir));

    // Joined rather than dropped, which is what makes the assertion below deterministic:
    // the destructor drops the handle instead, so that the host thread never waits.
    let worker = start_shutdown_backup(store, STAMP).expect("the worker starts");
    worker.join().expect("the worker finished");

    let listed = list_backups(&backup_dir(&dir)).expect("listing the generations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].taken_at_unix, STAMP);
}

#[test]
fn test_run_shutdown_backup_rotates_and_keeps_the_configured_count() {
    let dir = scratch_dir("backup-rotate");
    let store = UserStore {
        db: Mutex::new(seeded(&dir)),
        backup: BackupConfig::new(backup_dir(&dir))
            .with_keep(2)
            .with_interval_ms(0),
    };

    for step in 0..3u64 {
        run_shutdown_backup(&store, STAMP + step * 1_000);
    }

    let listed = list_backups(&backup_dir(&dir)).expect("listing the generations");
    assert_eq!(listed.len(), 2, "the surplus generation was removed");
    assert_eq!(listed[0].taken_at_unix, STAMP + 2_000, "newest first");
    assert_eq!(
        listed[1].taken_at_unix,
        STAMP + 1_000,
        "the oldest generation is the one that went"
    );
}

#[test]
fn test_run_shutdown_backup_writes_nothing_when_the_policy_is_disabled() {
    let dir = scratch_dir("backup-disabled");
    let store = UserStore {
        db: Mutex::new(seeded(&dir)),
        backup: BackupConfig::disabled(backup_dir(&dir)),
    };

    run_shutdown_backup(&store, STAMP);

    assert!(
        !backup_dir(&dir).exists(),
        "a disabled policy does not even create its directory"
    );
}

#[test]
fn test_run_shutdown_backup_leaves_the_store_intact_when_it_cannot_write() {
    let dir = scratch_dir("backup-blocked");
    let store = shutdown_store(&dir);
    // A file where the directory belongs: nothing can be written through it, and the
    // plugin degrades to read-only rather than failing the shutdown.
    fs::write(backup_dir(&dir), b"not a directory").expect("taking the directory's place");

    run_shutdown_backup(&store, STAMP);

    assert!(
        backup_dir(&dir).is_file(),
        "the obstruction is left exactly as it was"
    );
    assert_eq!(
        rows_of(&store),
        u64::try_from(KEYS.len()).expect("a small count"),
        "the store is untouched"
    );
    let db = lock(&store.db);
    assert_eq!(db.freq("ni'hao"), 1, "and still answers for its words");
}

// ── the rollback ───────────────────────────────────────────────────────────────────

#[test]
fn test_recover_store_leaves_a_healthy_store_alone() {
    let dir = scratch_dir("recover-healthy");
    drop(seeded(&dir));

    let outcome = recover_store(&db_path(&dir), &backup_dir(&dir)).expect("the pass reports");

    assert_eq!(outcome, RestoreOutcome::Healthy);
    assert!(!quarantined(&dir), "a usable store is not moved aside");
}

#[test]
fn test_recover_store_restores_a_damaged_store_from_the_newest_generation() {
    let dir = scratch_dir("recover-newest");
    let store = shutdown_store(&dir);
    let rows = rows_of(&store);
    run_shutdown_backup(&store, STAMP);
    // Released before the recovery pass opens the same path: `redb` refuses a second
    // handle on one database.
    drop(store);

    damage(&dir);
    let outcome = recover_store(&db_path(&dir), &backup_dir(&dir)).expect("restoring");

    assert_eq!(
        outcome,
        RestoreOutcome::RestoredFromBackup {
            from_unix: STAMP,
            rows,
        },
        "the newest generation is what the store is rebuilt from"
    );
    assert!(
        quarantined(&dir),
        "the damaged file was kept, never deleted"
    );
    let restored = UserDb::open(db_path(&dir)).expect("opening the restored store");
    assert_eq!(
        restored.freq("ni'hao"),
        1,
        "the counts came back with the rows"
    );
}

#[test]
fn test_recover_store_skips_a_generation_that_does_not_import() {
    let dir = scratch_dir("recover-skip");
    let store = shutdown_store(&dir);
    let rows = rows_of(&store);
    run_shutdown_backup(&store, STAMP);
    drop(store);
    // A newer generation -- the name carries the next day -- that holds the marker and a
    // row the format cannot read. It is listed like any other; the import is what refuses
    // it, and the older generation is what the store is rebuilt from.
    let newer = backup_dir(&dir).join("user-20231115-221320.tsv");
    fs::write(&newer, b"# rspinyin user dictionary export v1\nnot a row\n")
        .expect("planting a damaged generation");

    damage(&dir);
    let outcome = recover_store(&db_path(&dir), &backup_dir(&dir)).expect("restoring");

    assert_eq!(
        outcome,
        RestoreOutcome::RestoredFromBackup {
            from_unix: STAMP,
            rows,
        },
        "the older generation was tried after the newer one was refused"
    );
}

#[test]
fn test_recover_store_rebuilds_an_empty_store_when_there_is_no_generation() {
    let dir = scratch_dir("recover-empty");
    drop(seeded(&dir));
    damage(&dir);

    let outcome = recover_store(&db_path(&dir), &backup_dir(&dir)).expect("rebuilding");

    assert_eq!(outcome, RestoreOutcome::RebuiltEmpty);
    assert!(quarantined(&dir), "the damaged file is kept");
    let rebuilt = UserDb::open(db_path(&dir)).expect("the store is usable again");
    assert_eq!(rebuilt.record_count().expect("counting"), 0);
}

// ── what the user is told ──────────────────────────────────────────────────────────

#[test]
fn test_restore_notice_names_the_code_each_outcome_reports() {
    assert_eq!(
        restore_notice(RestoreOutcome::Healthy),
        None,
        "a store that opened needs no line"
    );
    assert_eq!(
        restore_notice(RestoreOutcome::RebuiltEmpty).as_deref(),
        Some(USER_DB_RECOVERED_CODE),
        "a rebuild reports the recovery pass's own code"
    );
    let restored = restore_notice(RestoreOutcome::RestoredFromBackup {
        from_unix: STAMP,
        rows: 3,
    })
    .expect("a restore is reported");
    assert_eq!(
        restored,
        format!("{BACKUP_RESTORED_CODE}: from={STAMP} rows=3"),
        "the code stays the first field and the generation is named"
    );
}

#[test]
fn test_backup_notice_reports_the_two_failures_and_stays_quiet_otherwise() {
    for quiet in [
        BackupOutcome::Written,
        BackupOutcome::Skipped,
        BackupOutcome::Disabled,
    ] {
        assert_eq!(backup_notice(&quiet), None, "{quiet:?} needs no line");
    }
    let degraded = backup_notice(&BackupOutcome::Readonly {
        reason: String::from("the user store is read-only"),
    })
    .expect("a degradation is reported");
    assert!(
        degraded.starts_with(READONLY_CODE),
        "the degradation is reported under the read-only code: {degraded}"
    );
    let failed = backup_notice(&BackupOutcome::Failed {
        reason: String::from("dict/export-too-large: bytes=9 limit=8"),
    })
    .expect("a failure is reported");
    assert!(
        failed.starts_with(BACKUP_FAILED_CODE),
        "the failure is reported under the backup code: {failed}"
    );
}

// ── what ships ─────────────────────────────────────────────────────────────────────
//
// The packaging descriptors Fcitx5 resolves are pinned in `tests::packaging`, which is
// where the assertions about them live now that this module has more than one subject.
