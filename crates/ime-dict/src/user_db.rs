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
//! user.redb
//!   user_words   &str -> (u32 commits, u64 last_used_ms)   one record per key
//!   user_meta    &str -> (u64 created_ms, u64 pinned)      one row per record that has one
//!   meta         &str -> u64                               schema_version
//! ```
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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ime_types::{DictError, ImeError, UserFreqSource, WordRef};
use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};

#[cfg(test)]
use std::cell::Cell;

mod backup;
mod cache;
mod evict;
mod export;
mod flush;
mod hydrate;
mod manage;

use self::cache::MissCache;
use self::evict::{Sweep, start_sweep};
use self::flush::{FlushThread, Pending};
use self::hydrate::load_committed;

pub use self::backup::{
    BACKUP_FAILED_CODE, BACKUP_INTERVAL_MS, BACKUP_KEEP_DEFAULT, BACKUP_RESTORED_CODE,
    BACKUP_SUBDIR, BackupConfig, BackupFile, BackupOutcome, MAX_BACKUP_KEEP, RestoreOutcome,
    backup_dir, list_backups, recover_user_db_with_backup, run_backup,
};
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
/// Schema version stamped into the `meta` table.
const SCHEMA_VERSION: u64 = 1;
/// Key the schema version is stored under.
const SCHEMA_KEY: &str = "schema_version";
/// One record per key: commits so far and the last time it was used.
const USER_WORDS: TableDefinition<&str, (u32, u64)> = TableDefinition::new("user_words");
/// One row per key that has one: when it was first learned, and whether it is pinned.
///
/// A key with no row here is a record an older build wrote -- see the module documentation.
/// The pin flag is a `u64` holding zero or one rather than a `bool`, because redb's `bool`
/// decoder panics on any other byte and this table is read from a user's own file.
const USER_META: TableDefinition<&str, (u64, u64)> = TableDefinition::new("user_meta");
/// Store-wide markers; version 1 holds only the schema version.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// The time source the store reads.
///
/// Injected rather than called directly because the commit interval, the `last_used_ms`
/// stamps and the idle sweep are all timing decisions, and no test in this workspace may
/// depend on the wall clock.
pub trait Clock: Send + Sync {
    /// Wall-clock milliseconds since the Unix epoch, stored per record.
    fn now_ms(&self) -> u64;
    /// Nanoseconds of a monotonic clock; only differences are meaningful.
    fn now_nanos(&self) -> u64;
}

/// The clock a running store uses: the system wall clock and a monotonic counter started
/// when the store was opened.
#[derive(Debug)]
pub struct SystemClock {
    epoch: Instant,
}

impl SystemClock {
    /// Starts a clock whose monotonic reading is zero now.
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn now_nanos(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

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
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|error| store_error(path, &error))?;
            set_mode(parent, DIR_MODE).map_err(|error| store_error(path, &error))?;
        }
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
        if let Some(version) = found {
            if version != SCHEMA_VERSION {
                return Err(ImeError::DictUnavailable {
                    path: path.to_path_buf(),
                    cause: DictError::FormatVersion {
                        found: u16::try_from(version).unwrap_or(u16::MAX),
                    },
                });
            }
        }
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION)
            .map_err(|error| store_error(path, &error))?;
        txn.open_table(USER_WORDS)
            .map_err(|error| store_error(path, &error))?;
        // Created here rather than on first use: a read transaction cannot create a table,
        // so a store whose metadata table was never written would fail the first
        // enumeration instead of answering an empty one.
        txn.open_table(USER_META)
            .map_err(|error| store_error(path, &error))?;
    }
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
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
    committed: Mutex<HashMap<Box<str>, u32>>,
    /// Whether [`Inner::committed`] holds the whole store.
    ///
    /// Decided once, at open, and never changed: it is what lets the hot paths choose
    /// their read strategy without taking the map's lock.
    is_hydrated: bool,
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
    sweep: Option<Sweep>,
    /// The flush thread, when one could be started.
    ///
    /// The record path publishes a request here and returns; the write transaction happens on
    /// that thread. A store whose thread could not be started flushes on the caller's thread
    /// instead -- see [`Inner::request_flush`].
    writer: Option<FlushThread>,
}

impl Inner {
    /// Number of records the store holds on disk.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the read transaction fails.
    fn stored_count(&self) -> Result<u64, ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
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
        stamp_schema(&path, &db)?;
        let loaded = load_committed(&db, hydrate_cap);
        let is_hydrated = loaded.is_some();
        let held = loaded.unwrap_or_default();
        let mut inner = Arc::new(Inner {
            db,
            is_hydrated,
            committed: Mutex::new(held.counts),
            pinned: Mutex::new(held.pinned),
            pending: Mutex::new(PendingState::default()),
            cache: if is_hydrated {
                None
            } else {
                Some(Mutex::new(MissCache::new(CACHE_CAPACITY)))
            },
            readonly: AtomicBool::new(false),
            batch: AtomicUsize::new(COMMIT_BATCH),
            interval_ms: AtomicU64::new(COMMIT_INTERVAL_MS),
            slow_streak: AtomicUsize::new(0),
            last_record_nanos: AtomicU64::new(clock.now_nanos()),
            clock,
            path,
            sweep: None,
            writer: None,
        });
        // Both threads reach the store through a weak handle, so neither can keep it alive,
        // and both are started before the last strong reference is published -- which is what
        // makes the `Arc::get_mut` below the only writer of these two fields.
        let sweep = start_sweep(&inner);
        let writer = FlushThread::start(&inner);
        if let Some(state) = Arc::get_mut(&mut inner) {
            state.sweep = sweep;
            state.writer = writer;
        }
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
        if let Some(writer) = &self.inner.writer {
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

impl UserFreqSource for UserDb {
    fn freq(&self, key: &str) -> u32 {
        // Both halves of the answer are in memory, so a decode never opens a read
        // transaction: the frozen `UserFreqSource` contract requires this call not to block
        // and not to take a lock a writer can hold, and a read transaction does both. The
        // two locks are taken in the order the module fixes, `committed` before `pending`,
        // and both are released before the call returns.
        if self.inner.is_hydrated {
            let committed = lock(&self.inner.committed);
            let base = committed.get(key).copied().unwrap_or(0);
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
