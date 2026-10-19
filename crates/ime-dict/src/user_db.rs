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
//!   user_key_pins  &str -> &str   the word pinned under an input key, one row per pin
//!   meta           &str -> u64                               schema_version
//! ```
//!
//! The logical clock measures usage, not time: it advances once per `record` call, and
//! `tick` on a record is the value the clock held when that record was last written. The
//! decay the clock feeds is the user-decay arithmetic; this schema only carries its two
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use ime_types::{DictError, ImeError, UserFreqSource};
use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};

#[cfg(test)]
use std::cell::Cell;

mod backup;
mod cache;
mod clock;
mod decay;
mod evict;
mod export;
mod flush;
mod hydrate;
mod manage;
mod records;
mod schema;
mod source;

use self::cache::MissCache;
use self::evict::{Sweep, start_sweep};
use self::flush::{Delta, FLUSH_SETTLE_TIMEOUT_MS, FlushThread, Pending};
use self::hydrate::{load_committed, load_pins};
use self::records::{Stored, StoredRow, WordsView, dee_q16};
use self::schema::{SchemaOutcome, dee_bits, prepare_path, prepare_schema, read_tick};

pub use self::backup::{
    BACKUP_FAILED_CODE, BACKUP_INTERVAL_MS, BACKUP_KEEP_DEFAULT, BACKUP_RESTORED_CODE,
    BACKUP_SUBDIR, BackupConfig, BackupFile, BackupOutcome, MAX_BACKUP_KEEP, RestoreOutcome,
    backup_dir, list_backups, recover_user_db_with_backup, run_backup,
};
pub use self::clock::{Clock, SystemClock};
pub use self::decay::DecayParams;
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
/// The frequency a pinned word reports.
///
/// The value saturates the scoring formula's logarithmic user term at its ceiling, so a
/// pinned word wins its key without exposing an infinite score to the decoder.
pub(crate) const PINNED_FREQ: u32 = u32::MAX;
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
/// One row per pin: the input key, and the word pinned under it.
///
/// The word-level flag in [`USER_META`] is what the ranking reads -- `freq` is asked about
/// a word, and a pinned word answers [`PINNED_FREQ`] -- but the pin itself belongs to an
/// input key: pinning a second word under the same key moves the pin (ADR-0013), and
/// knowing which word to move requires the association. Both tables are written by the
/// same flush, so they cannot disagree.
const USER_KEY_PINS: TableDefinition<&str, &str> = TableDefinition::new("user_key_pins");
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
    /// Pin changes waiting for the next flush, with the target state after coalescing.
    pins: HashMap<Box<str>, bool>,
    /// Key-pin rows waiting for the next flush: the input key, and the word its row
    /// should name -- or `None` for a row to delete.
    key_rows: HashMap<Box<str>, Option<Box<str>>>,
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
    /// The decay policy this build uses until configuration projection supplies one.
    decay: DecayParams,
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
    /// The input key each pinned word is pinned under.
    ///
    /// The ranking reads [`Inner::pinned`] alone -- `freq` is asked about a word -- but
    /// the pin's owner is the input key it was pressed on: pinning a second word under
    /// the same key moves the pin rather than adding one (ADR-0013), and the move needs
    /// to know which word to release. Loaded at open from `user_key_pins`; kept in step
    /// with the set by [`UserDb::set_pinned`], which mutates both before queueing the
    /// durable write, so a read never sees the set without the map.
    key_pins: Mutex<HashMap<Box<str>, Box<str>>>,
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
    /// Merges one flushed delta into a stored record, advancing its decay state.
    ///
    /// Existing usage is folded to the newer of its own stamp and the delta's stamp,
    /// then the new commits enter at that stamp. The flush preserves that anchor:
    /// aging to the current clock is a read-path fold, while the next task's idle
    /// sweep will fold batches forward without rewriting every record's usage stamp.
    pub(super) fn merged_record(&self, previous: Stored, delta: Delta) -> Stored {
        let anchor = previous.tick.max(delta.tick);
        let aged = self.decay.fold(previous.dee_q16, previous.tick, anchor);
        let entered = aged.saturating_add(i64::from(delta.count) << 16);
        Stored {
            commits: previous.commits.saturating_add(delta.count),
            dee_q16: entered,
            tick: anchor,
        }
    }

    /// Reads the current effective frequency of one stored record.
    pub(super) fn effective_freq(&self, record: &Stored) -> u32 {
        let now = self.tick.load(Ordering::Relaxed);
        let aged = self.decay.fold(record.dee_q16, record.tick, now);
        self.decay.effective_freq(record.commits, aged)
    }

    /// Opens the words table the file carries, in the schema this open found.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the table cannot be opened -- which
    /// for a store this build wrote means the file is not the one that was opened.
    pub(super) fn open_words(&self, txn: &redb::ReadTransaction) -> Result<WordsView, ImeError> {
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

    /// Opens the store at `path` with a clock and decay policy supplied by the caller.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_decay(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        decay: DecayParams,
    ) -> Result<Self, ImeError> {
        Self::open_with_hydrate_cap_and_decay(path, clock, HYDRATE_CAP, decay)
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
        Self::open_with_hydrate_cap_and_decay(path, clock, hydrate_cap, DecayParams::default())
    }

    /// Opens the store with a hydration ceiling and an explicit decay policy.
    ///
    /// The policy is projected from configuration by the caller; it is captured before
    /// the store's threads start, so every later fold uses one coherent policy.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_hydrate_cap_and_decay(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        hydrate_cap: u64,
        decay: DecayParams,
    ) -> Result<Self, ImeError> {
        let path = path.as_ref().to_path_buf();
        prepare_path(&path)?;
        let db = Database::create(&path).map_err(|error| store_error(&path, &error))?;
        let schema = prepare_schema(&path, &db)?;
        let v1_readonly = schema == SchemaOutcome::V1Degraded;
        let loaded = load_committed(&db, &path, hydrate_cap, v1_readonly);
        let is_hydrated = loaded.is_some();
        // The pins ride with the load when the store fits, and are read on their own when
        // it does not: the pinned set is what both the ranking's cap and eviction's
        // protection consult, and neither may go blind because the record count passed
        // the ceiling -- a pin set is a handful of rows, never the reason a store is too
        // large to load.
        let (records, pins) = match loaded {
            Some(loaded) => (loaded.records, (loaded.pinned, loaded.key_pins)),
            None => (HashMap::new(), load_pins(&db, &path, v1_readonly)),
        };
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
            decay,
            committed: Mutex::new(records),
            pinned: Mutex::new(pins.0),
            key_pins: Mutex::new(pins.1),
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

    /// Drops the oldest `percent` of the stored records, oldest by logical `tick`.
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

    /// Folds every committed record forward to the current logical clock.
    ///
    /// The idle sweep calls this automatically; the public form exists for a maintainer
    /// tool that wants the same bounded write path before reading or benchmarking a
    /// store.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when a fold transaction fails.
    pub fn fold_forward(&self) -> Result<u64, ImeError> {
        self.inner.fold_forward()
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
