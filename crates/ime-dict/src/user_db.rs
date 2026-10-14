//! The user frequency store: the persisted half of the ranking signal.
//!
//! Responsibility: remember how often the user commits a word, keep the store under
//! its record cap, and degrade to read-only rather than fail when the disk cannot be
//! written. The store implements the frozen [`UserFreqSource`] trait, so the decoder
//! never learns that a file is involved.
//!
//! Boundaries: this module owns the user database and nothing else. It never decodes,
//! never ranks and never talks to the UI; the read-only flag is published through
//! [`UserDb::is_readonly`] and the slow-disk condition through [`CommitReport::relaxed`]
//! plus [`SLOW_DISK_CODE`], because this crate has no logger to report them with. It is
//! the only place in the crate that opens the user database, and it holds the only write
//! path to it.
//!
//! ```text
//! user.redb (schema 2)
//!   user_words_v2  &str -> (u32 commits, u64 last_used_ms, u64 tick, u32 dee f32 bits)
//!                  one record per key; `tick` is the record's logical stamp and `dee`
//!                  the decay accumulator (stored as f32 bits, scored in Q16.16)
//!   tick           &str -> u64   the global logical clock, one row
//!   user_meta      &str -> (u64 created_ms, u64 pinned)      one row per record that has one
//!   meta           &str -> u64                               schema_version
//! ```
//!
//! The logical clock measures usage, not time: it advances once per `record` call, and
//! `tick` on a record is the value the clock held when that record was last written. The
//! decay the clock feeds is `TASK-1.02.02`'s arithmetic; this schema only carries its two
//! inputs so the fold never needs another migration.
//!
//! # Migration from schema 1
//!
//! A schema-1 file holds `user_words  &str -> (u32 commits, u64 last_used_ms)`. The open
//! path migrates it in one write transaction -- copy every row into `user_words_v2` with
//! `tick = 0` and `dee = commits`, write the clock at zero, drop the old table and stamp
//! version 2 -- so a crash rolls the whole migration back and the next open retries it.
//! A v1 row has no tick trace, which is exactly what `tick = 0` records: the clock itself
//! starts at zero, so at the migration instant the rows are un-aged and decay begins as
//! the user's next commits advance the clock. A migration whose transaction fails leaves
//! the file a untouched v1 store and the store opens read-only over it, which is the
//! same readonly discipline a failed flush takes; the caller reports
//! [`SCHEMA_MIGRATED_CODE`] for the degradation.
//!
//! The second table holds what was added after the first release: when a word was first
//! learned, and whether the user pinned it. It is a table of its own rather than two more
//! fields on `user_words` because redb decodes a fixed-width tuple by slicing exactly the
//! bytes the tuple needs, so a row written by an older build -- twelve bytes -- read as a
//! longer tuple indexes past its end and panics. With a table of its own, a record that has
//! no metadata row is simply one an older build wrote, and it reads as what it was: unpinned
//! and unstamped. A `pinned` column is stored as a `u64` rather than a `bool` for the same
//! class of reason -- redb's `bool` decoder is `unreachable!()` on any byte but 0 and 1.
//!
//! Durability: records accumulate in memory and reach the disk on the first of three
//! triggers -- [`COMMIT_BATCH`] distinct keys, [`COMMIT_INTERVAL_MS`] of typing, or the
//! ceiling [`PENDING_CAPACITY`]. The write itself happens on the store's own flush thread and
//! never on the caller's, which is what the frozen `UserFreqSource::record` contract requires
//! in its own words ("Implementations batch and flush asynchronously; this method must return
//! within 5us") and what `AGENTS.md` requires of an fcitx5 callback. A crash costs at most the
//! current window (`ASM-20`); a graceful shutdown loses nothing, because the owner calls
//! [`UserDb::final_commit`] before the store is dropped. A removal travels the same path: it
//! is a tombstone in the same delta, so a word the user has forgotten is gone from the ranking
//! at once and from the file at the next flush, and a crash before that flush costs the
//! removal the way it costs the records of the same window.
//!
//! Reads: the counts the store has flushed live in memory. A store holding at most
//! [`HYDRATE_CAP`] records is loaded whole at open and kept in step with the file by the
//! flush and the sweep, so a decode never opens a read transaction -- the frozen
//! `UserFreqSource` contract requires `freq` not to block and not to take a lock a writer
//! can hold, and a read transaction does both. A store past the ceiling is not loaded: its
//! reads fall back to the on-demand path, which is bounded by the ceiling rather than by the
//! word count and backed by the miss cache in `cache` -- a cache a loaded store is opened
//! without, because nothing would consult it.
//!
//! Threading: `record` runs on the host thread right after a commit and does no IO -- it
//! takes one short mutex, touches a map and returns. Everything that touches the file runs
//! elsewhere: the flush on the store's flush thread, and the idle sweep in [`evict`]. Both
//! reach the store through a weak handle, so neither can keep it alive, and each is the
//! single consumer of its own trigger -- which is what preserves the order one session's
//! records have to reach the store in (`ASM-11`). The two explicit flushes,
//! [`UserDb::commit`] and [`UserDb::final_commit`], do run on the caller's thread: they are
//! the owner's own calls, and a graceful shutdown may not return before the delta is on disk.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use ime_types::{DictError, ImeError, UserFreqSource, WordRef};
use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};

#[cfg(test)]
use std::cell::Cell;

mod backup;
mod cache;
mod clock;
mod evict;
mod export;
mod flush;
mod hydrate;
mod manage;

use self::cache::MissCache;
use self::evict::{Sweep, start_sweep};
use self::flush::{FLUSH_SETTLE_TIMEOUT_MS, FlushThread, Pending};
use self::hydrate::load_committed;

pub use self::backup::{
    BACKUP_FAILED_CODE, BACKUP_INTERVAL_MS, BACKUP_KEEP_DEFAULT, BACKUP_RESTORED_CODE,
    BACKUP_SUBDIR, BackupConfig, BackupFile, BackupOutcome, MAX_BACKUP_KEEP, RestoreOutcome,
    backup_dir, list_backups, recover_user_db_with_backup, run_backup,
};
pub use self::clock::{Clock, SystemClock};
pub use self::export::{EXPORT_LIMIT_BYTES, ImportReport};
pub use self::flush::CommitReport;
pub use self::manage::{ForgetOutcome, MAX_LIST_LIMIT, UserRecord};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod manage_tests;

#[cfg(test)]
mod export_tests;

/// Distinct keys that accumulate before a flush is due.
pub const COMMIT_BATCH: usize = 32;
/// Milliseconds of typing after which a flush is due.
pub const COMMIT_INTERVAL_MS: u64 = 2000;
/// Keys the fallback path's cache keeps resident.
///
/// The cache belongs to the fallback path: a store whose counts were loaded answers from
/// memory, and the cache is what keeps a store past [`HYDRATE_CAP`] to one read transaction
/// per distinct word per session rather than one per lookup. A loaded store is opened without
/// one, so the capacity is what a store that fell back costs and nothing else.
pub const CACHE_CAPACITY: usize = 4096;
/// Records the store loads into memory at open.
///
/// A user's own vocabulary is a few thousand words; the ceiling exists for the pathological
/// store, and past it the store keeps the on-demand path rather than growing the plugin's
/// resident set without bound.
pub const HYDRATE_CAP: u64 = 50_000;
/// Hard ceiling on the unflushed delta map; the batch trigger normally fires first, and
/// the ceiling is what keeps the map bounded if a flush stops making progress.
pub const PENDING_CAPACITY: usize = 4096;
/// Flush duration, in milliseconds, past which the batching policy is relaxed.
pub const SLOW_COMMIT_MS: u64 = 3;
/// Consecutive slow flushes before the policy widens.
///
/// One slow flush is not evidence of a slow disk. The first flush of a new database pays
/// for creating the file and its first pages, and a busy machine can push any single flush
/// over the line. Widening on one sample trades a real durability loss -- the in-memory
/// window grows from [`COMMIT_BATCH`] records to [`RELAXED_COMMIT_BATCH`], and the interval
/// from [`COMMIT_INTERVAL_MS`] to [`RELAXED_COMMIT_INTERVAL_MS`] -- for noise, so the
/// slowness has to persist before the store believes it.
pub const SLOW_COMMIT_STREAK: usize = 3;
/// Batch size the policy moves to when the disk turns out to be slow.
pub const RELAXED_COMMIT_BATCH: usize = 128;
/// Flush interval the policy moves to when the disk turns out to be slow.
pub const RELAXED_COMMIT_INTERVAL_MS: u64 = 10_000;
/// Records the store keeps before the idle sweep starts evicting.
pub const USER_WORD_CAP: u64 = 500_000;
/// Milliseconds without a record after which the sweep may run.
pub const EVICT_IDLE_MS: u64 = 30_000;
/// Share of the store one sweep removes, in percent.
pub const EVICT_FRACTION: u8 = 10;
/// Records one sweep removes at most; the sweep's working set is its threshold heap, so
/// it is bounded here rather than by the store, and a store far above [`USER_WORD_CAP`]
/// converges over several idle windows.
pub const MAX_EVICT_KEYS: u64 = 32_768;
/// Keys one sweep transaction examines before it ends and the next one begins.
///
/// The sweep shares the database with the host thread, so a transaction long enough to
/// cover a full store would hold a flush off for as long as the sweep runs. This chunk is
/// small enough to stay well inside a flush's budget, and large enough that the sweep is
/// not dominated by transaction setup: at [`MAX_EVICT_KEYS`] it costs at most eight of
/// them per idle window.
pub const EVICT_BATCH: usize = 4_096;
/// Mode the user database file carries.
pub const FILE_MODE: u32 = 0o600;
/// Mode of the directory the store creates for itself.
pub const DIR_MODE: u32 = 0o700;
/// Diagnostic code reported when a slow flush relaxed the batching policy.
pub const SLOW_DISK_CODE: &str = "data/commit/slow-disk";
/// Diagnostic code reported when the store was too large to load into memory.
///
/// The store is still usable in that state -- [`UserDb::is_hydrated`] reports it, and reads
/// fall back to the on-demand path -- so the code names a degradation rather than a failure.
pub const LARGE_STORE_CODE: &str = "data/user-db/large";
/// Diagnostic code reported when this open migrated a schema-1 store to schema 2, or
/// when that migration failed and the store opened read-only over its v1 table.
///
/// Both are open-time facts a caller can only render once; [`UserDb::schema_migrated`]
/// reports whether the open migrated, and [`UserDb::is_readonly`] tells the two cases
/// apart (readonly set means the migration failed and the store serves its v1 table).
pub const SCHEMA_MIGRATED_CODE: &str = "data/user-db/schema-migrated";
/// Schema version stamped into the `meta` table.
const SCHEMA_VERSION: u64 = 2;
/// Schema version a store written by an older build carries, and the one this build
/// migrates from.
const SCHEMA_VERSION_V1: u64 = 1;
/// Key the schema version is stored under.
const SCHEMA_KEY: &str = "schema_version";
/// One record per key: commits so far, the last wall-clock use, the record's logical
/// tick, and the decay accumulator in its storage encoding (f32 bits; all arithmetic
/// on the value happens in Q16.16 after the store hydrates it).
///
/// The wall-clock column is a schema-1 carry-over the management surface still speaks
/// (the enumeration's "most recently used" order and the export document), while
/// eviction ranks by the logical `tick`.
const USER_WORDS_V2: TableDefinition<&str, (u32, u64, u64, u32)> =
    TableDefinition::new("user_words_v2");
/// The v1 record table, kept for exactly two purposes: the open-time migration reads
/// it as its source, and a store whose migration failed serves it read-only. A store
/// this build has migrated no longer carries it.
const USER_WORDS: TableDefinition<&str, (u32, u64)> = TableDefinition::new("user_words");
/// The global logical clock: one row, advanced once per `record` in memory and written
/// to the file by the flush that carries the records it stamps.
const META_TICK: TableDefinition<&str, u64> = TableDefinition::new("tick");
/// The row key the clock is stored under.
const TICK_KEY: &str = "clock";
/// One row per key that has one: when it was first learned, and whether it is pinned.
///
/// A key with no row here is a record an older build wrote -- see the module documentation.
/// The pin flag is a `u64` holding zero or one rather than a `bool`, because redb's `bool`
/// decoder panics on any other byte and this table is read from a user's own file.
const USER_META: TableDefinition<&str, (u64, u64)> = TableDefinition::new("user_meta");
/// Store-wide markers; version 2 holds the schema version.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// Everything the store holds in memory between two flushes.
#[derive(Debug, Default)]
struct PendingState {
    /// One entry per key touched since the last successful flush.
    entries: HashMap<Box<str>, Pending>,
    /// Keys the user asked to forget since the last successful flush.
    ///
    /// A removal travels the path a record does -- the next flush applies it -- so the
    /// keystroke that asks for one costs a map insert rather than a write transaction
    /// (`ASM-04`, `ASM-20`). The tombstone is also what makes the word leave the candidate
    /// list at once: `freq` answers zero for it whether or not the file still holds it. A
    /// failed flush drops the tombstones with the deltas of the same window, and the store
    /// is read-only from then on.
    removed: HashSet<Box<str>>,
    /// Monotonic nanoseconds of the last flush, so that the interval trigger measures
    /// typing time rather than wall-clock time.
    last_flush_nanos: u64,
}

/// Locks a mutex, recovering from poisoning.
///
/// A poisoned lock means another thread panicked while holding it. The data behind these
/// locks -- a map of deltas and a cache of counts -- stays usable, and refusing to serve
/// frequencies would break input, so the flag is discarded and the data is used as it
/// stands.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wraps a store failure as the frozen cross-boundary error.
///
/// The frozen error list has no user-database variant, so a `redb` failure is carried as
/// [`DictError::Io`] -- the closest variant -- and surfaced as [`ImeError::DictUnavailable`]
/// with the path that failed; the precise failure stays in the message.
fn store_error(path: &Path, error: &impl std::fmt::Display) -> ImeError {
    ImeError::DictUnavailable {
        path: path.to_path_buf(),
        cause: DictError::Io(std::io::Error::other(error.to_string())),
    }
}

/// Sets the mode of `path` when it differs from `mode`.
///
/// # Errors
/// Returns the underlying `std::io::Error` when the metadata or the mode change fails.
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if current == mode {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Creates the directory and the file the store needs, with the modes it must keep.
///
/// The file is created here rather than by `redb`, which would create it `0644` under the
/// user's umask, and the user database is private data. An existing file is tightened
/// rather than trusted, because a wider mode survives a copy or a restore from a backup.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the directory cannot be created, or the file
/// cannot be created or tightened.
fn prepare_path(path: &Path) -> Result<(), ImeError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|error| store_error(path, &error))?;
        set_mode(parent, DIR_MODE).map_err(|error| store_error(path, &error))?;
    }
    if path.exists() {
        return set_mode(path, FILE_MODE).map_err(|error| store_error(path, &error));
    }
    let created = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|error| store_error(path, &error))?;
    drop(created);
    Ok(())
}

/// Creates the tables and stamps the schema version this build writes.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the store cannot be written, or when it
/// already carries a schema version this build does not know.
fn stamp_schema(path: &Path, db: &Database) -> Result<(), ImeError> {
    let mut txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    {
        let mut meta = txn
            .open_table(META)
            .map_err(|error| store_error(path, &error))?;
        let found = meta
            .get(SCHEMA_KEY)
            .map_err(|error| store_error(path, &error))?
            .map(|guard| guard.value());
        if let Some(version) = found
            && version != SCHEMA_VERSION
        {
            return Err(ImeError::DictUnavailable {
                path: path.to_path_buf(),
                cause: DictError::FormatVersion {
                    found: u16::try_from(version).unwrap_or(u16::MAX),
                },
            });
        }
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION)
            .map_err(|error| store_error(path, &error))?;
        txn.open_table(USER_WORDS_V2)
            .map_err(|error| store_error(path, &error))?;
        // Created here rather than on first use: a read transaction cannot create a table,
        // so a store whose metadata table was never written would fail the first
        // enumeration instead of answering an empty one.
        txn.open_table(USER_META)
            .map_err(|error| store_error(path, &error))?;
        let mut tick = txn
            .open_table(META_TICK)
            .map_err(|error| store_error(path, &error))?;
        tick.insert(TICK_KEY, 0)
            .map_err(|error| store_error(path, &error))?;
    }
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// The schema state an open finds, and what this build did about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemaOutcome {
    /// A fresh file: the tables were created and the version stamped.
    Fresh,
    /// A store this build already wrote; nothing to do.
    Current,
    /// A schema-1 store this build migrated to schema 2.
    Migrated,
    /// A schema-1 store whose migration failed; the store opens read-only over v1
    /// and the caller reports [`SCHEMA_MIGRATED_CODE`].
    V1Degraded,
}

/// Brings the store's schema to the version this build writes.
///
/// A fresh file is stamped; a current one is left alone; a schema-1 store is migrated
/// in one write transaction (see the module documentation); and a migration whose
/// transaction fails degrades to [`SchemaOutcome::V1Degraded`] rather than refusing to
/// open -- a user database that cannot be upgraded must still serve the history it
/// holds, and the readonly discipline is how a store that cannot be written stays
/// useful.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when a store neither fresh nor readable names a
/// schema version this build does not know, or when a table or transaction outside the
/// migration itself fails.
fn prepare_schema(path: &Path, db: &Database) -> Result<SchemaOutcome, ImeError> {
    let version = {
        let txn = db
            .begin_read()
            .map_err(|error| store_error(path, &error))?;
        let meta = txn.open_table(META).map_err(|error| store_error(path, &error))?;
        meta.get(SCHEMA_KEY)
            .map_err(|error| store_error(path, &error))?
            .map(|guard| guard.value())
    };
    match version {
        None => {
            stamp_schema(path, db)?;
            Ok(SchemaOutcome::Fresh)
        }
        Some(found) if found == SCHEMA_VERSION => {
            ensure_tables(path, db)?;
            Ok(SchemaOutcome::Current)
        }
        Some(found) if found == SCHEMA_VERSION_V1 => {
            if migrate_v1(path, db).is_ok() {
                Ok(SchemaOutcome::Migrated)
            } else {
                Ok(SchemaOutcome::V1Degraded)
            }
        }
        Some(other) => Err(ImeError::DictUnavailable {
            path: path.to_path_buf(),
            cause: DictError::FormatVersion {
                found: u16::try_from(other).unwrap_or(u16::MAX),
            },
        }),
    }
}

/// Opens every table a current store carries, creating the ones a fresh schema would
/// have made.
///
/// A read transaction cannot create a table, so a store whose auxiliary table was never
/// written would fail its first enumeration instead of answering an empty one; the
/// creation belongs to the open path, as it did under schema 1.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when a table cannot be created.
fn ensure_tables(path: &Path, db: &Database) -> Result<(), ImeError> {
    let mut txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(USER_WORDS_V2)
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(USER_META)
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(META_TICK)
        .map_err(|error| store_error(path, &error))?;
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// Migrates a schema-1 store to schema 2 in one write transaction.
///
/// Every v1 row enters v2 with `tick = 0` and `dee = commits`: the migration folds no
/// history, because a v1 row has no tick trace to fold from. The clock itself starts at
/// zero, so at the migration instant the rows are un-aged and the fold reads them exactly
/// as v1 ranked them; the decay begins as the user's next commits advance the clock. The
/// v1 table is dropped once its rows are copied -- the upgrade is one-way (`ASM-A-09`),
/// and a crash rolls the whole transaction back, leaving the file a v1 store the next
/// open migrates again.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the transaction or any table fails; the
/// caller degrades to a read-only v1 store on any of them.
fn migrate_v1(path: &Path, db: &Database) -> Result<(), ImeError> {
    #[cfg(test)]
    if take_injected_migration_failure() {
        return Err(store_error(path, &"injected migration failure"));
    }
    let mut txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    {
        let v1 = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(path, &error))?;
        let mut v2 = txn
            .open_table(USER_WORDS_V2)
            .map_err(|error| store_error(path, &error))?;
        for entry in v1
            .iter()
            .map_err(|error| store_error(path, &error))?
        {
            let (key, value) = entry.map_err(|error| store_error(path, &error))?;
            let (commits, last_used) = value.value();
            let dee = i64::from(commits) << 16;
            v2.insert(key.value(), (commits, last_used, 0, dee_bits(dee)))
                .map_err(|error| store_error(path, &error))?;
        }
        let mut tick = txn
            .open_table(META_TICK)
            .map_err(|error| store_error(path, &error))?;
        tick.insert(TICK_KEY, 0)
            .map_err(|error| store_error(path, &error))?;
        let mut meta = txn
            .open_table(META)
            .map_err(|error| store_error(path, &error))?;
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION)
            .map_err(|error| store_error(path, &error))?;
    }
    // The borrow of `v1` ends with the block above, so the drop here is allowed. Reopening
    // a v1-shaped table afterwards would answer an empty one, which is why the drop and
    // the version stamp share the transaction: either both land or neither does.
    txn.delete_table(USER_WORDS)
        .map_err(|error| store_error(path, &error))?;
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// The decay accumulator's storage encoding: f32 bits. A storage encoding only -- the
/// scoring arithmetic on the value is integer fixed point, so no float order can leak
/// into a ranking.
fn dee_bits(dee_q16: i64) -> u32 {
    f32::to_bits(dee_q16 as f32 / 65_536.0)
}

/// The working form of a stored accumulator: Q16.16 fixed point.
fn dee_q16(bits: u32) -> i64 {
    let held = f32::from_bits(bits);
    // The `as` cast saturates instead of overflowing, and a value past the integer
    // domain was already unusable as a weight -- the saturation is the floor behaviour
    // the decay expects, not a silent error.
    (held * 65_536.0) as i64
}

/// Reads the global logical clock the file holds.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the read fails; the clock row is written
/// with the tables, so a store that opens without one is a store this build did not
/// prepare and cannot serve.
fn read_tick(path: &Path, db: &Database) -> Result<u64, ImeError> {
    let txn = db
        .begin_read()
        .map_err(|error| store_error(path, &error))?;
    let tick = txn
        .open_table(META_TICK)
        .map_err(|error| store_error(path, &error))?;
    tick.get(TICK_KEY)
        .map_err(|error| store_error(path, &error))?
        .map(|guard| guard.value())
        .ok_or_else(|| {
            store_error(
                path,
                &"the logical clock row is missing from the tick table",
            )
        })
}

/// One hydrated record: what the store flushed, in the form the read paths consume.
///
/// `dee_q16` is the decay accumulator in Q16.16 and `tick` the record's own logical
/// stamp; both are the inputs the decay fold (`1.02.02`) consumes. Until that fold
/// arrives the read paths answer `commits`, which is the v1 semantic the store keeps
/// for an un-decayed record.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stored {
    /// Commits so far, saturating at `u32::MAX`.
    pub(crate) commits: u32,
    /// The decay accumulator in Q16.16.
    pub(crate) dee_q16: i64,
    /// The record's logical tick.
    pub(crate) tick: u64,
}

impl Stored {
    /// The record a fresh write of `commits` commits carries, with no decay applied.
    pub(crate) fn identity(commits: u32) -> Self {
        Self {
            commits,
            dee_q16: i64::from(commits) << 16,
            tick: 0,
        }
    }

    /// The stored encoding of this record's accumulator.
    pub(crate) fn dee_bits(&self) -> u32 {
        dee_bits(self.dee_q16)
    }
}

/// One record as a file row holds it: a [`Stored`] plus the wall-clock column the
/// management surface still speaks.
#[derive(Clone, Copy, Debug)]
pub(super) struct StoredRow {
    /// Commits so far.
    pub(super) commits: u32,
    /// Last use, in unix milliseconds.
    pub(super) last_used_ms: u64,
    /// The record's logical tick.
    pub(super) tick: u64,
    /// The decay accumulator in Q16.16.
    pub(super) dee_q16: i64,
}

impl StoredRow {
    /// The in-memory form the hot read paths hold.
    pub(super) fn as_stored(&self) -> Stored {
        Stored {
            commits: self.commits,
            dee_q16: self.dee_q16,
            tick: self.tick,
        }
    }

    /// The row a v1 file's record reads as: no tick, an un-decayed accumulator.
    pub(super) fn from_v1(commits: u32, last_used_ms: u64) -> Self {
        Self {
            commits,
            last_used_ms,
            tick: 0,
            dee_q16: i64::from(commits) << 16,
        }
    }
}

/// The words table the store actually carries, in the schema it was opened with.
///
/// A store whose migration failed serves its v1 table read-only, so every read of the
/// words table goes through this view rather than naming a table. A v1 row reads as
/// its v2 equivalent with no tick and an un-decayed accumulator -- which is exactly
/// what it is.
pub(super) enum WordsView<'txn> {
    /// The v1 table of a store whose migration failed.
    V1(redb::ReadOnlyTable<&'txn str, (u32, u64)>),
    /// The table this build writes.
    V2(redb::ReadOnlyTable<&'txn str, (u32, u64, u64, u32)>),
}

impl WordsView<'_> {
    /// The stored record `key` maps to, or `None` when the store does not hold it.
    ///
    /// # Errors
    /// Returns the underlying `redb` failure; every caller renders it against the
    /// store's own path.
    pub(super) fn get(&self, key: &str) -> Result<Option<Stored>, redb::StorageError> {
        match self {
            Self::V1(table) => Ok(table
                .get(key)?
                .map(|guard| Stored::identity(guard.value().0))),
            Self::V2(table) => Ok(table.get(key)?.map(|guard| {
                let (commits, _, tick, dee) = guard.value();
                Stored {
                    commits,
                    dee_q16: dee_q16(dee),
                    tick,
                }
            })),
        }
    }

    /// Feeds every record, in key order, to `visit`.
    ///
    /// # Errors
    /// Returns the underlying `redb` failure for the iteration itself, and whatever
    /// `visit` reports for a row.
    pub(super) fn for_each<F>(&self, mut visit: F) -> Result<(), redb::StorageError>
    where
        F: FnMut(&str, StoredRow) -> Result<(), ImeError>,
    {
        match self {
            Self::V1(table) => {
                for entry in table.iter()? {
                    let (key, value) = entry?;
                    let (commits, last_used_ms) = value.value();
                    visit(key.value(), StoredRow::from_v1(commits, last_used_ms))?;
                }
            }
            Self::V2(table) => {
                for entry in table.iter()? {
                    let (key, value) = entry?;
                    let (commits, last_used_ms, tick, dee) = value.value();
                    visit(
                        key.value(),
                        StoredRow {
                            commits,
                            last_used_ms,
                            tick,
                            dee_q16: dee_q16(dee),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// The store's shared state: everything the clones of a [`UserDb`] point at.
struct Inner {
    /// The `redb` handle, and the only holder of the file lock.
    db: Database,
    /// The committed half of the frequency signal, held in memory.
    ///
    /// Authoritative for reads, and kept equal to the file by every path that changes
    /// either: [`load_committed`] fills it at open, `write_batch` adopts what it wrote,
    /// `degrade` leaves it alone because the failed transaction was rolled back, and the
    /// sweep forgets what it removed. Empty when the store was too large to load -- see
    /// [`Inner::is_hydrated`].
    committed: Mutex<HashMap<Box<str>, Stored>>,
    /// Whether [`Inner::committed`] holds the whole store.
    ///
    /// Decided once, at open, and never changed: it is what lets the hot paths choose
    /// their read strategy without taking the map's lock.
    is_hydrated: bool,
    /// The global logical clock, in memory.
    ///
    /// Advanced once per `record` (`fetch_add` -- no lock, no IO, the same budget the
    /// delta map's insert keeps) and written to the `tick` table by the flush that
    /// carries the records it stamped. Hydrated from the file at open; a store opened
    /// from a migrated v1 file starts it at zero, which is what makes the migrated
    /// rows un-aged at the migration instant.
    tick: AtomicU64,
    /// Whether this open migrated a schema-1 store, or degraded to serving one.
    ///
    /// Decided once at open and never changed; the caller turns it into the
    /// [`SCHEMA_MIGRATED_CODE`] diagnostic. A degraded store is also read-only, which
    /// is how its callers tell the two flavors apart.
    schema_migrated: bool,
    /// Whether the store serves its original v1 table read-only, because its migration
    /// failed.
    ///
    /// Decided once at open; every read of the words table consults it to pick the
    /// table the file actually carries, and every write path refuses a store that has
    /// it set (the store is read-only, which they check first).
    v1_readonly: bool,
    /// Deltas recorded but not yet flushed.
    pending: Mutex<PendingState>,
    /// The keys the user pinned, which eviction leaves alone.
    ///
    /// Only the pinned keys are held, so an ordinary store pays for an empty set. The flags
    /// are loaded at open for the same reason the counts are: a keystroke that asks to
    /// forget a word may not open a read transaction to find out whether it is pinned.
    pinned: Mutex<HashSet<Box<str>>>,
    /// The fallback path's cache, present only for a store that was not loaded.
    ///
    /// A loaded store answers from [`Inner::committed`] and never consults a cache, so it is
    /// opened without one. That is what keeps the record path down to the delta map's single
    /// lock: the previous form bumped a cache on every record, for a reader that does not
    /// exist on a loaded store.
    cache: Option<Mutex<MissCache>>,
    /// Set once the store can no longer be written.
    readonly: AtomicBool,
    /// Distinct keys that make a flush due.
    batch: AtomicUsize,
    /// Milliseconds of typing that make a flush due.
    interval_ms: AtomicU64,
    /// Consecutive flushes that ran past [`SLOW_COMMIT_MS`]; a fast flush resets it.
    slow_streak: AtomicUsize,
    /// Monotonic nanoseconds of the last record, for the idle sweep.
    last_record_nanos: AtomicU64,
    /// The time source, injected so that tests drive the clock themselves.
    clock: Box<dyn Clock>,
    /// The path, kept for error reporting.
    path: PathBuf,
    /// The idle sweep thread, when one could be started.
    ///
    /// Written once by [`UserDb::open_with_hydrate_cap`] -- the only stretch in which the
    /// store is not shared yet -- and read everywhere else through `get`.
    sweep: OnceLock<Option<Sweep>>,
    /// The flush thread, when one could be started.
    ///
    /// The record path publishes a request here and returns; the write transaction happens on
    /// that thread. A store whose thread could not be started flushes on the caller's thread
    /// instead -- see [`Inner::request_flush`]. Written once at open, for the same reason the
    /// sweep is: the two handles live behind locks rather than behind `Arc::get_mut`, because
    /// a weak handle is enough to keep `get_mut` from ever answering.
    writer: OnceLock<Option<FlushThread>>,
}

impl Inner {
    /// Opens the words table the file carries, in the schema this open found.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the table cannot be opened -- which
    /// for a store this build wrote means the file is not the one that was opened.
    pub(super) fn open_words<'txn>(
        &self,
        txn: &'txn redb::ReadTransaction,
    ) -> Result<WordsView<'txn>, ImeError> {
        let view = if self.v1_readonly {
            WordsView::V1(
                txn.open_table(USER_WORDS)
                    .map_err(|error| store_error(&self.path, &error))?,
            )
        } else {
            WordsView::V2(
                txn.open_table(USER_WORDS_V2)
                    .map_err(|error| store_error(&self.path, &error))?,
            )
        };
        Ok(view)
    }

    /// Number of records the store holds on disk.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the read transaction fails, or when
    /// neither words table the store could carry opens.
    fn stored_count(&self) -> Result<u64, ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        if !self.v1_readonly {
            let table = txn
                .open_table(USER_WORDS_V2)
                .map_err(|error| store_error(&self.path, &error))?;
            return table.len().map_err(|error| store_error(&self.path, &error));
        }
        let table = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(&self.path, &error))?;
        table.len().map_err(|error| store_error(&self.path, &error))
    }
}

/// The user frequency store: the persisted half of the ranking signal.
///
/// # Concurrency
///
/// The store is `Send + Sync` and cheap to clone: every clone shares one `redb` handle, one
/// delta map, and -- for a store that was not loaded -- one cache. `freq` and `record` are
/// safe to call from any thread, but the store relies on the decoder's serial-per-session
/// discipline (`ASM-11`) for the order of the records it is given, and on that discipline
/// alone to keep a flush from interleaving with the records of another session.
///
/// The file itself is touched by two threads of the store's own -- the flush thread and the
/// idle sweep -- and both are single consumers of their own trigger, so the order one
/// session's records arrive in is the order they reach the disk in.
#[derive(Clone)]
pub struct UserDb {
    inner: Arc<Inner>,
}

impl UserDb {
    /// Opens the store at `path`, creating it when it does not exist.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the directory cannot be prepared, when
    /// the file cannot be created with mode [`FILE_MODE`], when the store refuses the file
    /// (a second handle on the same path, a corrupt image), or when the file carries a
    /// schema version this build does not know.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ImeError> {
        Self::open_with(path, Box::new(SystemClock::new()))
    }

    /// Opens the store at `path` with an injected clock.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with(path: impl AsRef<Path>, clock: Box<dyn Clock>) -> Result<Self, ImeError> {
        Self::open_with_hydrate_cap(path, clock, HYDRATE_CAP)
    }

    /// Opens the store at `path` with an injected clock and hydration ceiling.
    ///
    /// The ceiling is a parameter for the same reason the clock is: the behaviour of a
    /// store too large to load is worth asserting, and no test may build a
    /// fifty-thousand-record store to reach it. A running plugin opens through
    /// [`UserDb::open`] and so always uses [`HYDRATE_CAP`]; a ceiling of zero loads no
    /// store that holds a record, which is how the fallback path is reached deliberately.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_hydrate_cap(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        hydrate_cap: u64,
    ) -> Result<Self, ImeError> {
        let path = path.as_ref().to_path_buf();
        prepare_path(&path)?;
        let db = Database::create(&path).map_err(|error| store_error(&path, &error))?;
        let schema = prepare_schema(&path, &db)?;
        let v1_readonly = schema == SchemaOutcome::V1Degraded;
        let loaded = load_committed(&db, hydrate_cap, v1_readonly);
        let is_hydrated = loaded.is_some();
        let held = loaded.unwrap_or_default();
        // A v1 store served read-only carries no clock at all -- its migration, the
        // only thing that would have written one, failed. Zero is the clock a fresh
        // store starts with, and a read-only store never advances it.
        let tick = if v1_readonly {
            0
        } else {
            read_tick(&path, &db)?
        };
        let inner = Arc::new(Inner {
            db,
            is_hydrated,
            committed: Mutex::new(held.records),
            pinned: Mutex::new(held.pinned),
            tick: AtomicU64::new(tick),
            schema_migrated: matches!(schema, SchemaOutcome::Migrated | SchemaOutcome::V1Degraded),
            v1_readonly,
            pending: Mutex::new(PendingState::default()),
            cache: if is_hydrated {
                None
            } else {
                Some(Mutex::new(MissCache::new(CACHE_CAPACITY)))
            },
            readonly: AtomicBool::new(v1_readonly),
            batch: AtomicUsize::new(COMMIT_BATCH),
            interval_ms: AtomicU64::new(COMMIT_INTERVAL_MS),
            slow_streak: AtomicUsize::new(0),
            last_record_nanos: AtomicU64::new(clock.now_nanos()),
            clock,
            path,
            sweep: OnceLock::new(),
            writer: OnceLock::new(),
        });
        // Both threads reach the store through a weak handle, so neither can keep it alive.
        // Their handles are installed through the locks rather than through `Arc::get_mut`:
        // the weak handles the two threads hold are themselves enough to keep `get_mut`
        // answering `None` forever, which is how an install here used to silently drop both
        // threads and turn every record path into a caller-thread flush. `set` cannot
        // refuse -- the locks are fresh, and only this function, which has not returned yet,
        // can reach them -- so the results are dropped rather than matched.
        let _ = inner.sweep.set(start_sweep(&inner));
        let _ = inner.writer.set(FlushThread::start(&inner));
        Ok(Self { inner })
    }

    /// Flushes every pending record with `durability`.
    ///
    /// The flush runs on the calling thread, with the store's flush thread held aside for the
    /// length of the call. That is deliberate: this is an explicit request from the owner, and
    /// what it reports is the delta this call itself made durable.
    ///
    /// # Errors
    /// As [`UserDb::final_commit`].
    pub fn commit(&mut self, durability: Durability) -> Result<CommitReport, ImeError> {
        self.commit_inner(durability)
    }

    /// Flushes every pending record with [`Durability::Immediate`].
    ///
    /// The owner calls this on shutdown: it is what makes a graceful exit lose nothing
    /// (`ASM-20`), because the records a batch trigger had not yet handed to the flush thread
    /// are written here and the call does not return until they are. It is also the flush a
    /// caller uses when the configuration asks for a synchronous commit after every candidate.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] -- the `data/readonly-mode` code -- when the write
    /// fails, after the store has degraded to read-only. [`CommitReport::relaxed`] is set
    /// when the flush was slow enough to relax the batching policy, and the caller reports
    /// [`SLOW_DISK_CODE`] then. A flush a batch trigger earned runs on the store's own thread
    /// and answers to nobody, so this is the only path that reports `relaxed` at all.
    pub fn final_commit(&mut self) -> Result<CommitReport, ImeError> {
        self.commit_inner(Durability::Immediate)
    }

    /// Whether the store has degraded to read-only.
    ///
    /// The status strip renders its lock from this: `record` is a no-op while it is set,
    /// and `freq` keeps answering.
    pub fn is_readonly(&self) -> bool {
        self.inner.readonly.load(Ordering::Acquire)
    }

    /// Whether the store's committed counts were loaded into memory when it opened.
    ///
    /// A store past [`HYDRATE_CAP`] records is not loaded: its reads fall back to the
    /// on-demand path, which is correct but opens a read transaction per distinct word.
    /// This crate has no logger, so the caller reports [`LARGE_STORE_CODE`] for that state
    /// -- the same division of labour as [`SLOW_DISK_CODE`] and [`CommitReport::relaxed`].
    pub fn is_hydrated(&self) -> bool {
        self.inner.is_hydrated
    }

    /// Whether this open migrated a schema-1 store, or degraded to serving one.
    ///
    /// The caller reports [`SCHEMA_MIGRATED_CODE`] once for it: as an info when the store
    /// is writable, and as a warn when [`UserDb::is_readonly`] is also set -- the latter
    /// means the migration's transaction failed and the store is serving its original v1
    /// table, read-only, until the condition that failed it goes away.
    pub fn schema_migrated(&self) -> bool {
        self.inner.schema_migrated
    }

    /// Number of records the store holds on disk.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the read transaction fails.
    pub fn record_count(&self) -> Result<u64, ImeError> {
        self.inner.stored_count()
    }

    /// Drops the oldest `percent` of the stored records, oldest by `last_used_ms`.
    ///
    /// The threshold is the exact percentile, found with a bounded max-heap over one
    /// read-only scan, so the sweep removes the records the policy names rather than an
    /// estimate of them.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when a transaction fails. A read-only store
    /// evicts nothing and reports zero.
    pub fn evict_oldest(&self, percent: u8) -> Result<u64, ImeError> {
        self.inner.evict_oldest(percent)
    }

    /// Flushes the pending records with `durability`.
    ///
    /// The store's flush thread is held aside for the length of the call, so what this
    /// reports is the delta this flush itself made durable. The wait is bounded; a writer
    /// that does not answer inside the bound is left running and the flush proceeds beside
    /// it, which is safe because both drain the same delta map.
    ///
    /// # Errors
    /// As [`UserDb::final_commit`].
    fn commit_inner(&self, durability: Durability) -> Result<CommitReport, ImeError> {
        self.inner.flush_now(durability)
    }

    /// How many records are waiting to be flushed.
    ///
    /// Test-only: the count is an implementation detail, and production code reads it
    /// through [`UserDb::record_count`] after a flush.
    #[cfg(test)]
    fn pending_len(&self) -> usize {
        lock(&self.inner.pending).entries.len()
    }

    /// How many removals are waiting to be flushed.
    ///
    /// Test-only, and the counterpart of [`UserDb::pending_len`]: a tombstone is the one
    /// other thing the pending state holds.
    #[cfg(test)]
    fn pending_removed_len(&self) -> usize {
        lock(&self.inner.pending).removed.len()
    }

    /// How many keys the in-memory counts hold, or `None` when the store was not loaded.
    ///
    /// Test-only: the count is an implementation detail, and production code reads the
    /// store's size through [`UserDb::record_count`].
    #[cfg(test)]
    fn committed_len(&self) -> Option<usize> {
        if !self.inner.is_hydrated {
            return None;
        }
        Some(lock(&self.inner.committed).len())
    }

    /// Waits until the flush thread has served everything it was asked to.
    ///
    /// Test-only: the production paths that need this are `commit` and `final_commit`, and
    /// both flush on the caller's own thread. A test that asserts what a trigger wrote cannot
    /// otherwise tell "not yet" from "never", and the bound is generous because a flush of a
    /// batch is bounded by one write transaction -- a thread that misses it is broken rather
    /// than slow, which is what the assertion says.
    #[cfg(test)]
    fn wait_for_flush(&self) {
        if let Some(writer) = self.inner.writer.get().and_then(|set| set.as_ref()) {
            assert!(
                writer.settle(std::time::Duration::from_secs(5)),
                "the flush thread did not settle"
            );
        }
    }

    /// Whether the store was opened with the fallback path's cache.
    ///
    /// Test-only: it is what turns "the cache exists only for a store that was not loaded"
    /// from a code-review item into an assertion.
    #[cfg(test)]
    fn has_miss_cache(&self) -> bool {
        self.inner.cache.is_some()
    }
}

impl Drop for UserDb {
    /// Gives the flush thread a moment to step out of the file.
    ///
    /// The thread holds the store only for the length of one flush, but that is long enough
    /// to keep the `redb` file lock standing after the last clone has dropped -- which is
    /// what an immediate reopen (the recovery pass, a test) would otherwise meet. The wait
    /// is bounded by [`FLUSH_SETTLE_TIMEOUT_MS`], the same bound an explicit flush waits
    /// with; past it the flush is still running, and the lock lifts when it ends rather than
    /// when this drop does.
    fn drop(&mut self) {
        // Best effort by design, and the guard is what keeps it so: only the last clone
        // settles, because only it can be the one whose release ends the store. A clone
        // that appears while this one drops keeps the store -- and its thread -- alive and
        // owns the next settle itself.
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        if let Some(writer) = self.inner.writer.get().and_then(|set| set.as_ref()) {
            writer.settle(Duration::from_millis(FLUSH_SETTLE_TIMEOUT_MS));
        }
    }
}

impl UserFreqSource for UserDb {
    fn freq(&self, key: &str) -> u32 {
        // Both halves of the answer are in memory, so a decode never opens a read
        // transaction: the frozen `UserFreqSource` contract requires this call not to block
        // and not to take a lock a writer can hold, and a read transaction does both. The
        // two locks are taken in the order the module fixes, `committed` before `pending`,
        // and both are released before the call returns.
        if self.inner.is_hydrated {
            let committed = lock(&self.inner.committed);
            // The decay fold (`1.02.02`) reads the accumulator and the tick out of the
            // same record; until it lands the answer is the commit count, which is the
            // v1 semantic and the un-decayed limit of the fold.
            let base = committed.get(key).map_or(0, |record| record.commits);
            // The delta is read under its own lock while `committed` is still held, in the
            // order the module fixes: `committed` first, then `pending`, never the reverse.
            let (forgotten, delta) = {
                let pending = lock(&self.inner.pending);
                (
                    pending.removed.contains(key),
                    pending.entries.get(key).map_or(0, |entry| entry.count),
                )
            };
            // A tombstone outranks the count the file still holds: the user asked for the
            // word to go, and the flush that makes the file agree has not run yet.
            return if forgotten {
                0
            } else {
                base.saturating_add(delta)
            };
        }
        // The store was too large to load, so the answer has to come from the file. The
        // fallback is bounded by the ceiling rather than by the word count: `on_demand`
        // opens one transaction per distinct word per session.
        self.inner.on_demand(key)
    }

    /// Records one commit of `key`, without blocking and without allocating when the key is
    /// already pending.
    ///
    /// The wall clock is not read, the cache is not touched, and the write transaction is not
    /// performed here: the delta map's own entry point owns the batching policy, and the flush
    /// thread owns the disk. What is left on this path is one monotonic reading, one lock of
    /// the delta map, one hash lookup, and -- only for a key the map has not seen -- one
    /// allocation.
    fn record(&self, key: &str, _weight_hint: u16) {
        if self.is_readonly() {
            return;
        }
        if self.inner.record_delta(key) {
            self.inner.request_flush();
        }
    }

    fn is_user_word(&self, _key: &str) -> bool {
        // Version 1 stores frequencies only: a key is a word the user typed, not a word
        // the user coined. The table that marks a coined word, and the write path that
        // fills it, arrive with the Phase 2 user-word support.
        false
    }

    fn forget(&self, key: &str) -> bool {
        // The frozen method answers whether a record went away; `forget_with_outcome` is
        // the same call with the reason kept, which is what the host reports a diagnostic
        // from. A pinned record and a read-only store both answer `false`, because neither
        // removed anything -- and the caller that needs to tell them apart asks for the
        // outcome instead.
        self.forget_with_outcome(key) == ForgetOutcome::Removed
    }

    fn list(&self, offset: u64, limit: u16) -> Result<Vec<WordRef<'_>>, ImeError> {
        // The store cannot serve this method, and the reason is a property of the signature
        // rather than a gap in the implementation. `WordRef<'a>`'s text borrows for as long
        // as `&self`, and the store's word set changes under it: a borrow can be taken out
        // of a mutex guard only for as long as the guard lives, and the store's keys live
        // behind one -- `committed` for a loaded store, the cache for a fallback one --
        // because `record` and `forget` take `&self` and must be able to write. There is no
        // safe way to publish them for `'self`, and the two ways to fake it are both
        // forbidden here: leaking the keys grows the resident set with the user's
        // vocabulary, and reaching past the guard needs `unsafe`, which this crate confines
        // to `mmap.rs`.
        //
        // `dict/unsupported` is the contract's code for a capability the build does not
        // provide, and it is what a caller falls back from: `UserDb::list_words` answers the
        // same page as owned rows, and `UserFreqSource::export_tsv` answers the whole store.
        let _ = (offset, limit);
        Err(ImeError::Unsupported)
    }

    fn export_tsv(&self, writer: &mut dyn std::io::Write) -> Result<u64, ImeError> {
        self.inner.write_export(writer)
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only: makes the next flush fail.
    ///
    /// The degradation path has to be exercised deterministically, and a real refusal
    /// needs a read-only filesystem or a full disk -- neither of which a test may assume,
    /// and neither of which a write through an already-open descriptor respects anyway.
    static INJECTED_FAILURE: Cell<bool> = const { Cell::new(false) };

    /// Test-only: makes the next schema migration fail.
    ///
    /// The v1-read-only degradation has the same shape as the flush one -- a write
    /// transaction that fails is what both degrade on -- and a test reaches it the same
    /// way: by arming the failure instead of building a disk that refuses writes.
    static INJECTED_MIGRATION_FAILURE: Cell<bool> = const { Cell::new(false) };

    /// Test-only: counts the store reads this thread has opened.
    ///
    /// The hot path's whole claim is that a lookup on a loaded store opens no read
    /// transaction, and a count of zero cannot be asserted without a counter -- the
    /// alternative is to time the call, which no test in this workspace may do.
    static STORE_READS: Cell<u64> = const { Cell::new(0) };
}

/// Counts one store read. Test-only.
#[cfg(test)]
fn note_store_read() {
    STORE_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
}

/// Store reads this thread has opened so far. Test-only.
#[cfg(test)]
fn store_reads() -> u64 {
    STORE_READS.with(|reads| reads.get())
}

/// Arms the next flush to fail. Test-only.
#[cfg(test)]
fn inject_failure() {
    INJECTED_FAILURE.with(|flag| flag.set(true));
}

/// Consumes an armed failure. Test-only.
#[cfg(test)]
fn take_injected_failure() -> bool {
    INJECTED_FAILURE.with(|flag| flag.replace(false))
}

/// Arms the next schema migration to fail. Test-only.
#[cfg(test)]
fn inject_migration_failure() {
    INJECTED_MIGRATION_FAILURE.with(|flag| flag.set(true));
}

/// Consumes an armed migration failure. Test-only.
#[cfg(test)]
fn take_injected_migration_failure() -> bool {
    INJECTED_MIGRATION_FAILURE.with(|flag| flag.replace(false))
}
