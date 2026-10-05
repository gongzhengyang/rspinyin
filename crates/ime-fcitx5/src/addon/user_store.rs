//! The user's own words: the store the process runs on, and the two writes the shutdown
//! makes.
//!
//! Responsibility: run the `store-recovery` step — repair the user store, roll it back from
//! a backup generation when one is usable, and adopt it — and own everything the shutdown
//! does with it: the flush, which stays on the host thread, and the backup, which does not.
//!
//! Boundaries: `ime-dict` owns the database, its batching, its export format and the
//! backup policy; this module owns the process-wide slot, the ordering at unload and the
//! reporting.
//!
//! # The one piece of user data
//!
//! The frequency store is the one thing this addon owns that cannot be rebuilt from
//! anywhere else, and the one thing that cannot be rebuilt from anywhere else is what the
//! user typed. The recovery step does two things with it at load: it repairs a damaged file
//! — isolating it under a `.corrupt.` name, never deleting it — and it rebuilds the store
//! from the newest backup generation that imports cleanly, which is what turns "your store
//! is damaged" into "your words are back". The store is then adopted, so that the
//! destructor has one handle to flush and to copy; `redb` refuses a second handle on one
//! path, so the process wants exactly one.
//!
//! At unload the store is flushed on the host thread and copied on a worker thread of its
//! own. The split is the difference between the two costs: the flush writes the unflushed
//! delta, which the store's own batching caps, while a backup is the whole document — up to
//! the export ceiling — and a write of that size has no place inside an Fcitx5 callback.
//! The worker is not joined, so the host thread never waits on it: a backup cut short by the
//! process exiting leaves the previous generation where it was, because a generation is
//! written to a temporary and renamed onto its name, and the next start writes one once the
//! interval has passed.
//!
//! # Where the store lives
//!
//! The directory is the `data-dirs` step's layout. The `[data]` keys of the configuration
//! document are in force by the time this step runs — the `config` step precedes it in the
//! sequence — so the store is adopted under the policy they name: the backup policy is
//! assembled from `backup_enabled` and `backup_keep`, and `durability` is applied to the
//! store's batching window through `UserDb::set_flush_window`, the parameter channel the
//! reload path re-aims as well. An environment with no configuration directory leaves the
//! built-in defaults in force, which is what a fresh installation runs with.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ime_config::{DataConfig, Durability};
use ime_dict::paths::READONLY_CODE;
use ime_dict::recover::USER_DB_RECOVERED_CODE;
use ime_dict::user_db::{
    BACKUP_FAILED_CODE, BACKUP_RESTORED_CODE, BackupConfig, BackupOutcome, COMMIT_BATCH,
    COMMIT_INTERVAL_MS, Clock, LARGE_STORE_CODE, RestoreOutcome, SLOW_DISK_CODE, SystemClock,
    UserDb, backup_dir, list_backups, recover_user_db_with_backup, run_backup,
};
use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

use super::config::with_config_store;
use super::layout;
use super::lock;
use super::report_step_failure;

/// The name the shutdown backup's worker thread carries.
///
/// Linux caps a thread name at fifteen bytes, and the name is worth having because the
/// thread shows up in a stack dump of a process that is shutting down.
const BACKUP_THREAD_NAME: &str = "userdb-backup";

/// What the shutdown path needs: the store to flush and to copy, and where the copies go.
///
/// One handle for the whole process, because `redb` refuses a second one on the same path:
/// the engine's decode path reaches its frequencies through this store rather than opening
/// the database again.
pub(super) struct UserStore {
    /// The user frequency store, behind a lock because the final flush takes `&mut`.
    pub(super) db: Mutex<UserDb>,
    /// The backup policy: how many generations are kept, and how far apart they are.
    ///
    /// Assembled from the `[data]` section the `config` step has already installed —
    /// see [`backup_policy`]. The policy is read when the store is adopted, because the
    /// shutdown path reads it through the store handle the sessions share.
    pub(super) backup: BackupConfig,
}

/// The process's user store: installed by the recovery step, taken by the destructor.
///
/// A `Mutex` over a slot rather than a `OnceLock` because the slot is emptied as well as
/// filled: the destructor takes the store out, which is what makes a second call to it a
/// no-op instead of a second flush and a second backup. The lock is a leaf — nothing else
/// is taken while it is held — and it is uncontended, because its only two holders are the
/// host thread at shutdown and the worker that thread starts.
static USER_STORE: Mutex<Option<Arc<UserStore>>> = Mutex::new(None);

/// Installs `store` as the process's user store; the first install wins.
///
/// A second store is refused rather than replacing the first: the handle already there is
/// the one a decode may be reading through, and swapping it would leave that reader with a
/// store nothing flushes and nothing copies.
///
/// # Panics
///
/// Never.
pub(super) fn install_user_store(store: UserStore) {
    let mut slot = lock(&USER_STORE);
    if slot.is_none() {
        *slot = Some(Arc::new(store));
    }
}

/// Takes the installed store, leaving the slot empty.
///
/// The handle it answers with is shared with whoever else holds one, so dropping it does
/// not close the database: the last holder is what ends the store and its idle sweep.
///
/// # Panics
///
/// Never.
pub(super) fn take_user_store() -> Option<Arc<UserStore>> {
    lock(&USER_STORE).take()
}

/// The installed store without taking it, for the sources a session decodes against.
///
/// Clones the handle rather than the store: the decode path and the destructor reach one
/// database, which is the whole reason the slot holds an [`Arc`].
///
/// # Panics
///
/// Never.
pub(super) fn user_store() -> Option<Arc<UserStore>> {
    lock(&USER_STORE).clone()
}

/// The `store-recovery` step: repairs the user store, rolls it back and adopts it.
///
/// The layout the store lives in is the one the `data-dirs` step installed; a run with no
/// layout has no store to recover, and the step reports that and succeeds.
///
/// # Errors
///
/// None. A store that cannot be recovered or opened leaves the plugin without learning,
/// which is the read-only degradation and never a lost input method.
///
/// # Panics
///
/// Never.
pub(super) fn recover_stores() -> Result<(), ImeError> {
    let Some(paths) = layout::layout() else {
        report_step_failure("store-recovery", &"no data directory");
        return Ok(());
    };
    // The `[data]` section is in force by now — the `config` step precedes this one in the
    // sequence — so the store is adopted under the policy it names. An environment with no
    // configuration directory leaves the built-in defaults in force, which is what a fresh
    // installation runs with.
    let data = with_config_store(|store| store.current().data).unwrap_or_default();
    let directory = backup_dir(&paths.data_dir);
    match recover_store(&paths.user_db, &directory) {
        Ok(outcome) => emit_notice(restore_notice(outcome)),
        Err(error) => report_step_failure("store-recovery", &error),
    }
    match UserDb::open(&paths.user_db) {
        Ok(db) => {
            if !db.is_hydrated() {
                // The store is past its hydration ceiling and reads fall back to the
                // on-demand path. It has no logger, so the caller reports the code.
                emit_diagnostic(LARGE_STORE_CODE);
            }
            install_user_store(UserStore {
                db: Mutex::new(db),
                backup: backup_policy(&data, &directory),
            });
            apply_data_config(&data);
        }
        Err(error) => report_step_failure("store-recovery", &error),
    }
    Ok(())
}

/// The backup policy the `[data]` section names, over `directory`.
///
/// `backup_enabled = false` is the backup module's disabled policy, which writes nothing
/// and does not even create the directory; `backup_keep` is carried through
/// `with_keep`, whose clamp is the second line of defence behind the configuration
/// layer's `1..=MAX_BACKUP_KEEP` repair. The interval between generations is the backup
/// module's own — the schema names no key for it.
///
/// # Panics
///
/// Never.
fn backup_policy(data: &DataConfig, directory: &Path) -> BackupConfig {
    let policy = if data.backup_enabled {
        BackupConfig::new(directory)
    } else {
        BackupConfig::disabled(directory)
    };
    policy.with_keep(data.backup_keep)
}

/// The batching window a `data.durability` value names, as [`UserDb::set_flush_window`]
/// takes it.
///
/// `eventual` is the shipped window: the batch trigger at [`COMMIT_BATCH`] distinct keys
/// or [`COMMIT_INTERVAL_MS`] of typing, whichever first. `immediate` is the zero window:
/// one record and no delay, so every commit is handed to the flush thread as it happens
/// and a graceful shutdown's flush has nothing left to write. The record path itself never
/// changes — the window is the store's own trigger parameters, not a new path, and the
/// host thread stays as unblocked as it was.
///
/// # Panics
///
/// Never.
fn flush_window(durability: Durability) -> (usize, u64) {
    /// The record count the immediate window batches: one, so nothing accumulates.
    const IMMEDIATE_BATCH: usize = 1;
    /// The delay the immediate window allows: none.
    const IMMEDIATE_INTERVAL_MS: u64 = 0;

    match durability {
        Durability::Eventual => (COMMIT_BATCH, COMMIT_INTERVAL_MS),
        Durability::Immediate => (IMMEDIATE_BATCH, IMMEDIATE_INTERVAL_MS),
    }
}

/// Applies the `[data]` keys the engine can re-aim while it runs.
///
/// `data.durability` is live: the batching window lives in atomics the record path reads
/// per record, so a load and a reload both land without disturbing a session. The two
/// backup keys are deliberately absent here — the backup policy is a value the shutdown
/// path reads through the store handle the sessions share, so it is assembled when the
/// recovery step adopts the store ([`recover_stores`]) and a reload that edits it takes
/// effect the next time the store is adopted.
///
/// With no store installed — the recovery step has not adopted one, or the environment
/// has no data directory — there is nothing to apply and this is a no-op.
///
/// # Panics
///
/// Never.
pub(super) fn apply_data_config(data: &DataConfig) {
    let Some(store) = user_store() else {
        return;
    };
    let (batch, interval_ms) = flush_window(data.durability);
    lock(&store.db).set_flush_window(batch, interval_ms);
}

/// Wall-clock milliseconds since the Unix epoch: the stamp a generation is named after.
///
/// Read through the store's own clock rather than a second `SystemTime` call, so the
/// saturating conversion from a clock set before the epoch lives in one place.
///
/// # Panics
///
/// Never.
pub(super) fn now_ms() -> u64 {
    SystemClock::new().now_ms()
}

/// Repairs the user store at `path`, preferring the newest generation it can roll back to.
///
/// # Parameters
/// - `path`: the store, normally `user.redb` inside the data directory.
/// - `directory`: the backup directory, normally [`backup_dir`] of that data directory.
///
/// # Return value
/// What the recovery pass did with the store: [`RestoreOutcome::Healthy`] when it opened,
/// [`RestoreOutcome::RebuiltEmpty`] when a damaged one was isolated and replaced by an
/// empty store, and [`RestoreOutcome::RestoredFromBackup`] when the words came back from a
/// generation.
///
/// # Errors
/// As [`recover_user_db_with_backup`]: `data/backup-failed` when a generation existed but
/// none could be imported, and `data/readonly-mode` when a damaged store could not be
/// replaced at all.
///
/// # Panics
/// Never: a recovery pass runs during start-up and may not cost the user their input
/// method.
pub(super) fn recover_store(path: &Path, directory: &Path) -> Result<RestoreOutcome, ImeError> {
    let backups = match list_backups(directory) {
        Ok(found) => found,
        Err(error) => {
            // Reported and then dropped rather than propagated: a damaged store must still
            // be isolated and rebuilt, and what the failure costs is the rollback, which is
            // what the listing was for. `list_backups` answers `dict/unavailable` only for
            // a directory that exists and cannot be read.
            report_step_failure("store-recovery", &error);
            Vec::new()
        }
    };
    recover_user_db_with_backup(path, &backups)
}

/// The line a recovery outcome is reported with, or `None` when it needs none.
///
/// Returned rather than emitted so that the mapping is a value: which outcome reports
/// which code is the contract, and the diagnostic channel has no seam this crate's tests
/// could read a line back from.
///
/// `data/db/recovered` is the code the plain recovery pass reports for the same work, so a
/// quarantine is greppable under one spelling whether or not a generation was involved.
/// `data/backup-restored` carries the stamp and the row count, because which generation the
/// words came back from is what the user has to be told. A row count is a count and never
/// the words themselves.
///
/// # Panics
///
/// Never.
pub(super) fn restore_notice(outcome: RestoreOutcome) -> Option<String> {
    match outcome {
        RestoreOutcome::Healthy => None,
        RestoreOutcome::RebuiltEmpty => Some(String::from(USER_DB_RECOVERED_CODE)),
        RestoreOutcome::RestoredFromBackup { from_unix, rows } => Some(format!(
            "{BACKUP_RESTORED_CODE}: from={from_unix} rows={rows}"
        )),
    }
}

/// The line a backup attempt is reported with, or `None` when it needs none.
///
/// `Written`, `Skipped` and `Disabled` need no line: a generation that landed is the
/// ordinary answer at shutdown, and the two others are the policy working as asked. The
/// two failures are reported, because a user whose copies stopped needs to be able to find
/// out why.
///
/// # Panics
///
/// Never.
pub(super) fn backup_notice(outcome: &BackupOutcome) -> Option<String> {
    match outcome {
        BackupOutcome::Written | BackupOutcome::Skipped | BackupOutcome::Disabled => None,
        BackupOutcome::Readonly { reason } => Some(format!("{READONLY_CODE}: {reason}")),
        BackupOutcome::Failed { reason } => Some(format!("{BACKUP_FAILED_CODE}: {reason}")),
    }
}

/// Writes one notice to the diagnostic channel, when there is one.
///
/// The code stays the line's first field, so a grep for it still matches every line it
/// produced.
///
/// # Panics
///
/// Never.
pub(super) fn emit_notice(notice: Option<String>) {
    if let Some(line) = notice {
        emit_diagnostic(&line);
    }
}

/// Flushes the store's pending records: the last write the process can make.
///
/// Runs on the host thread, unlike the backup. What it writes is the unflushed delta the
/// store has accumulated, which its own batching caps, and the alternative — losing the
/// last window of learning on every restart — is the durability loss `ASM-20` bounds.
///
/// # Panics
///
/// Never.
pub(super) fn flush_store(store: &UserStore) {
    let mut db = lock(&store.db);
    match db.final_commit() {
        Ok(report) => {
            if report.relaxed {
                // The store relaxed its batching policy because the disk is slow. It has no
                // logger, so the caller reports the code.
                emit_diagnostic(SLOW_DISK_CODE);
            }
        }
        Err(error) => report_step_failure("user-data-flush", &error),
    }
}

/// Writes the shutdown generation if one is due, and reports what happened.
///
/// The body of the worker thread, and the entry point a test drives when it wants the
/// backup without the thread.
///
/// # Panics
///
/// Never.
pub(super) fn run_shutdown_backup(store: &UserStore, now_unix_ms: u64) {
    let db = lock(&store.db);
    match run_backup(&db, &store.backup, now_unix_ms) {
        Ok(outcome) => emit_notice(backup_notice(&outcome)),
        // A backup that was due and could not be written. The error's own rendering is the
        // contract's `data/backup-failed` code, which has to stay the line's first field,
        // so the failure is not wrapped in a lifecycle step's line.
        Err(error) => emit_diagnostic(&error.to_string()),
    }
}

/// Starts the shutdown backup on a thread of its own.
///
/// The backup writes the whole store's document — up to the export ceiling — and a write
/// of that size has no place inside an Fcitx5 callback, so it leaves the host thread. The
/// thread is this module's own rather than the store's idle sweep, which is private to
/// `ime-dict` and wakes on a schedule of its own; it lives for one backup and ends.
///
/// # Return value
/// The worker's handle, or `None` when no thread could be started. The caller drops the
/// handle: joining would put the write back inside the callback, and a backup cut short by
/// the process exiting leaves the previous generation where it was, because a generation
/// is written to a temporary and renamed onto its name.
///
/// # Panics
///
/// Never.
pub(super) fn start_shutdown_backup(
    store: Arc<UserStore>,
    now_unix_ms: u64,
) -> Option<JoinHandle<()>> {
    let worker = std::thread::Builder::new()
        .name(String::from(BACKUP_THREAD_NAME))
        .spawn(move || run_shutdown_backup(&store, now_unix_ms));
    match worker {
        Ok(handle) => Some(handle),
        Err(error) => {
            // A thread that cannot be started is a process out of room, which the user can
            // act on; the flush above already landed, so nothing else is lost.
            report_step_failure("user-data-backup", &error);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the `[data]` policy the recovery step assembles and a reload re-aims.
    //!
    //! What the store does with the values — a disabled policy writing nothing, a
    //! rotation keeping the configured count, a zero window flushing per record — is
    //! pinned where the mechanism lives: the backup module's and the flush path's own
    //! tests. What is pinned here is that the configuration's vocabulary reaches it
    //! unchanged.
    use ime_config::DEFAULT_BACKUP_KEEP;
    use ime_dict::user_db::BACKUP_INTERVAL_MS;

    use super::*;

    #[test]
    fn test_backup_policy_follows_the_data_section() {
        let directory = Path::new("/data/rspinyin/backups");
        let turned_off = backup_policy(
            &DataConfig {
                backup_enabled: false,
                backup_keep: 7,
                ..DataConfig::default()
            },
            directory,
        );
        assert!(
            !turned_off.enabled,
            "backup_enabled = false is the disabled policy, which writes nothing"
        );
        assert_eq!(turned_off.keep, 7, "backup_keep is carried through");
        assert_eq!(turned_off.directory, directory);

        let shipped = backup_policy(&DataConfig::default(), directory);
        assert!(shipped.enabled, "the shipped default keeps the copies on");
        assert_eq!(
            shipped.keep, DEFAULT_BACKUP_KEEP,
            "and keeps the shipped generation count"
        );
        assert_eq!(
            shipped.interval_ms, BACKUP_INTERVAL_MS,
            "the schema names no interval key, so the module's own interval stands"
        );
    }

    #[test]
    fn test_flush_window_follows_the_durability() {
        assert_eq!(
            flush_window(Durability::Eventual),
            (COMMIT_BATCH, COMMIT_INTERVAL_MS),
            "eventual is the shipped batching window"
        );
        assert_eq!(
            flush_window(Durability::Immediate),
            (1, 0),
            "immediate is the zero window: one record, no delay"
        );
    }

    #[test]
    fn test_apply_data_config_without_a_store_is_a_noop() {
        // The recovery step has not adopted a store in this process, so there is nothing
        // to re-aim and the call must not reach for one: a configuration applied to a
        // store nobody installed would be a wiring defect, not a value. The slot the steps
        // fill is process-wide, so the test starts from a known empty state.
        let _ = take_user_store();
        apply_data_config(&DataConfig::default());
        assert!(user_store().is_none(), "the slot stays empty");
    }
}
