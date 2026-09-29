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
//!   meta         &str -> u64                               schema_version
//! ```
//!
//! Durability: records accumulate in memory and reach the disk on the first of three
//! triggers -- [`COMMIT_BATCH`] distinct keys, [`COMMIT_INTERVAL_MS`] of typing, or the
//! ceiling [`PENDING_CAPACITY`]. A crash costs at most the current window (`ASM-20`); a
//! graceful shutdown loses nothing, because the owner calls [`UserDb::final_commit`]
//! before the store is dropped.
//!
//! Threading: `record` runs on the host thread right after a commit and does no IO -- it
//! takes one short mutex, touches a map and returns. The flush it may start runs on that
//! same thread on purpose: one session's records have to reach the store in the order
//! they were committed, and a writer thread would need a queue and a wakeup for a batch
//! of thirty-two small writes. The idle sweep in [`evict`] is the only other writer.

use std::collections::HashMap;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ime_types::{DictError, ImeError, UserFreqSource};
use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};

#[cfg(test)]
use std::cell::Cell;

mod evict;

use self::evict::{Sweep, start_sweep};

#[cfg(test)]
mod tests;

/// Distinct keys that accumulate before a flush is due.
pub const COMMIT_BATCH: usize = 32;
/// Milliseconds of typing after which a flush is due.
pub const COMMIT_INTERVAL_MS: u64 = 2000;
/// Keys the frequency cache keeps resident.
pub const CACHE_CAPACITY: usize = 4096;
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
/// Schema version stamped into the `meta` table.
const SCHEMA_VERSION: u64 = 1;
/// Key the schema version is stored under.
const SCHEMA_KEY: &str = "schema_version";
/// One record per key: commits so far and the last time it was used.
const USER_WORDS: TableDefinition<&str, (u32, u64)> = TableDefinition::new("user_words");
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
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
    }

    fn now_nanos(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// What one flush did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitReport {
    /// Number of keys written; zero when nothing was pending.
    pub written: usize,
    /// Duration of the write transaction in microseconds.
    pub elapsed_us: u64,
    /// Whether this flush relaxed the batching policy because the disk was slow.
    pub relaxed: bool,
}

/// The in-memory delta of one key: what has been recorded but not yet flushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    /// Commits recorded since the last successful flush.
    count: u32,
    /// Wall-clock milliseconds of the most recent record.
    last_used_ms: u64,
}

/// Everything the store holds in memory between two flushes.
#[derive(Debug, Default)]
struct PendingState {
    /// One entry per key touched since the last successful flush.
    entries: HashMap<Box<str>, Pending>,
    /// Monotonic nanoseconds of the last flush, so that the interval trigger measures
    /// typing time rather than wall-clock time.
    last_flush_nanos: u64,
}

/// A fixed-capacity second-chance cache of user frequencies.
///
/// The lookup path may not allocate and the footprint may not grow with the number of
/// keys ever seen, so a strict LRU -- which needs a list node or a fresh allocation per
/// hit -- is out. This is the CLOCK approximation: one reference bit per slot plus a hand
/// that clears it, which keeps the hot keys resident at a constant cost per access and a
/// footprint bounded by the capacity.
struct LruCache {
    slots: Vec<Slot>,
    index: HashMap<Arc<str>, u32>,
    hand: usize,
}

/// One cache slot; `key` is `None` while the slot holds nothing.
struct Slot {
    key: Option<Arc<str>>,
    value: u32,
    referenced: bool,
}

impl LruCache {
    /// Builds a cache of `capacity` slots.
    ///
    /// The capacity is clamped to at least one slot: a cache that can hold nothing would
    /// be a configuration mistake rather than a mode, and the clamp is what lets the
    /// eviction hand take a remainder without testing for an empty table.
    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1))
                .map(|_| Slot {
                    key: None,
                    value: 0,
                    referenced: false,
                })
                .collect(),
            index: HashMap::new(),
            hand: 0,
        }
    }

    /// Returns the cached value of `key`, marking the slot as referenced.
    fn get(&mut self, key: &str) -> Option<u32> {
        let index = *self.index.get(key)?;
        let slot = self.slots.get_mut(index as usize)?;
        slot.referenced = true;
        Some(slot.value)
    }

    /// Stores `value` under `key`, replacing the least recently referenced slot once the
    /// cache is full.
    fn insert(&mut self, key: &str, value: u32) {
        if let Some(index) = self.index.get(key).copied() {
            if let Some(slot) = self.slots.get_mut(index as usize) {
                slot.value = value;
                slot.referenced = true;
                return;
            }
        }
        let index = self.victim();
        let shared: Arc<str> = Arc::from(key);
        if let Some(slot) = self.slots.get_mut(index) {
            if let Some(previous) = slot.key.replace(Arc::clone(&shared)) {
                self.index.remove(previous.as_ref());
            }
            slot.value = value;
            slot.referenced = true;
            self.index.insert(shared, index as u32);
        }
    }

    /// Adds one to the cached value of `key` when it is present; reports whether it was.
    fn bump(&mut self, key: &str) -> bool {
        let Some(index) = self.index.get(key).copied() else {
            return false;
        };
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return false;
        };
        slot.value = slot.value.saturating_add(1);
        slot.referenced = true;
        true
    }

    /// Drops `key` from the cache.
    fn remove(&mut self, key: &str) {
        let Some(index) = self.index.remove(key) else {
            return;
        };
        if let Some(slot) = self.slots.get_mut(index as usize) {
            slot.key = None;
            slot.value = 0;
            slot.referenced = false;
        }
    }

    /// Returns the index of the slot the next insert replaces, clearing the reference bit
    /// of every slot the hand passes.
    ///
    /// The search always terminates: after one turn no bit is set, so the slot the hand
    /// started from is the victim.
    fn victim(&mut self) -> usize {
        loop {
            let index = self.hand % self.slots.len();
            self.hand = self.hand.wrapping_add(1);
            match self.slots.get_mut(index) {
                Some(slot) if slot.referenced => slot.referenced = false,
                _ => return index,
            }
        }
    }
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
    let mut txn = db.begin_write().map_err(|error| store_error(path, &error))?;
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
    }
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// The store's shared state: everything the clones of a [`UserDb`] point at.
struct Inner {
    /// The `redb` handle, and the only holder of the file lock.
    db: Database,
    /// Deltas recorded but not yet flushed.
    pending: Mutex<PendingState>,
    /// The frequency cache, consulted before the store itself.
    cache: Mutex<LruCache>,
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
}

impl Inner {
    /// Reads the flushed frequency of `key`; zero when it is absent or unreadable.
    ///
    /// A read failure is deliberately not an error: `freq` returns a count, and a store
    /// that cannot be read must still let the user type. Zero means "no boost", the same
    /// answer as a word that was never recorded.
    fn committed(&self, key: &str) -> u32 {
        let Ok(txn) = self.db.begin_read() else {
            return 0;
        };
        let Ok(table) = txn.open_table(USER_WORDS) else {
            return 0;
        };
        match table.get(key) {
            Ok(Some(guard)) => guard.value().0,
            _ => 0,
        }
    }

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

    /// Writes one batch of deltas, merging each with what the store already holds.
    ///
    /// The error is the rendered `redb` failure rather than a typed one: it is carried to
    /// the caller as the reason of the read-only diagnostic, which the frozen error model
    /// spells as a string.
    fn write_batch(
        &self,
        drained: &[(Box<str>, Pending)],
        durability: Durability,
    ) -> Result<(), String> {
        #[cfg(test)]
        if take_injected_failure() {
            return Err("injected flush failure".to_string());
        }
        let mut txn = self.db.begin_write().map_err(|error| error.to_string())?;
        {
            let mut table = txn
                .open_table(USER_WORDS)
                .map_err(|error| error.to_string())?;
            for (key, delta) in drained {
                let previous = table
                    .get(key.as_ref())
                    .map_err(|error| error.to_string())?
                    .map_or((0, 0), |guard| guard.value());
                let merged = (
                    previous.0.saturating_add(delta.count),
                    previous.1.max(delta.last_used_ms),
                );
                table
                    .insert(key.as_ref(), merged)
                    .map_err(|error| error.to_string())?;
            }
        }
        txn.set_durability(durability);
        txn.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Enters read-only mode after a failed flush.
    ///
    /// The drained deltas never reached the disk, so the cache -- which counts committed
    /// plus pending -- is rolled back to the last committed values. Reads keep working: a
    /// store that cannot learn must still answer, or the decoder would lose the history it
    /// already has.
    fn degrade(&self, drained: &[(Box<str>, Pending)]) {
        self.readonly.store(true, Ordering::Release);
        let mut cache = lock(&self.cache);
        for (key, delta) in drained {
            if let Some(value) = cache.get(key) {
                cache.insert(key, value.saturating_sub(delta.count));
            }
        }
    }

    /// Reports a flush and relaxes the batching policy when the disk is slow.
    ///
    /// A run of flushes past [`SLOW_COMMIT_MS`] costs more than the budget allows, so the
    /// store batches harder instead of blocking the host thread more often. One slow flush
    /// is not enough to act on -- see [`SLOW_COMMIT_STREAK`] -- and a fast flush resets the
    /// run, so a single hiccup cannot move the policy. The relaxation itself is one-way: a
    /// store that has been slow keeps the wider window rather than oscillating between the
    /// two policies.
    fn report(&self, written: usize, elapsed_us: u64) -> CommitReport {
        let streak = if elapsed_us >= SLOW_COMMIT_MS * 1_000 {
            self.slow_streak.fetch_add(1, Ordering::Relaxed).saturating_add(1)
        } else {
            self.slow_streak.store(0, Ordering::Relaxed);
            0
        };
        let mut relaxed = false;
        if streak >= SLOW_COMMIT_STREAK
            && self.batch.load(Ordering::Relaxed) < RELAXED_COMMIT_BATCH
        {
            self.batch.store(RELAXED_COMMIT_BATCH, Ordering::Relaxed);
            self.interval_ms
                .store(RELAXED_COMMIT_INTERVAL_MS, Ordering::Relaxed);
            relaxed = true;
        }
        CommitReport {
            written,
            elapsed_us,
            relaxed,
        }
    }
}

/// The user frequency store: the persisted half of the ranking signal.
///
/// # Concurrency
///
/// The store is `Send + Sync` and cheap to clone: every clone shares one `redb` handle,
/// one delta map and one cache. `freq` and `record` are safe to call from any thread, but
/// the store relies on the decoder's serial-per-session discipline (`ASM-11`) for the
/// order of the records it is given, and on that discipline alone to keep a flush from
/// interleaving with the records of another session.
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
        let path = path.as_ref().to_path_buf();
        prepare_path(&path)?;
        let db = Database::create(&path).map_err(|error| store_error(&path, &error))?;
        stamp_schema(&path, &db)?;
        let mut inner = Arc::new(Inner {
            db,
            pending: Mutex::new(PendingState::default()),
            cache: Mutex::new(LruCache::new(CACHE_CAPACITY)),
            readonly: AtomicBool::new(false),
            batch: AtomicUsize::new(COMMIT_BATCH),
            interval_ms: AtomicU64::new(COMMIT_INTERVAL_MS),
            slow_streak: AtomicUsize::new(0),
            last_record_nanos: AtomicU64::new(clock.now_nanos()),
            clock,
            path,
            sweep: None,
        });
        let sweep = start_sweep(&inner);
        if let Some(state) = Arc::get_mut(&mut inner) {
            state.sweep = sweep;
        }
        Ok(Self { inner })
    }

    /// Flushes every pending record with `durability`.
    ///
    /// # Errors
    /// As [`UserDb::final_commit`].
    pub fn commit(&mut self, durability: Durability) -> Result<CommitReport, ImeError> {
        self.commit_inner(durability)
    }

    /// Flushes every pending record with [`Durability::Immediate`].
    ///
    /// The owner calls this on shutdown: it is what makes a graceful exit lose nothing
    /// (`ASM-20`). It is also the flush a caller uses when the configuration asks for a
    /// synchronous commit after every candidate.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] -- the `data/readonly-mode` code -- when the write
    /// fails, after the store has degraded to read-only. [`CommitReport::relaxed`] is set
    /// when the flush was slow enough to relax the batching policy, and the caller reports
    /// [`SLOW_DISK_CODE`] then.
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
    /// # Errors
    /// As [`UserDb::final_commit`].
    fn commit_inner(&self, durability: Durability) -> Result<CommitReport, ImeError> {
        if self.is_readonly() {
            return Ok(CommitReport::default());
        }
        let drained: Vec<(Box<str>, Pending)> = {
            let mut pending = lock(&self.inner.pending);
            if pending.entries.is_empty() {
                return Ok(CommitReport::default());
            }
            pending.last_flush_nanos = self.inner.clock.now_nanos();
            pending.entries.drain().collect()
        };
        let started = self.inner.clock.now_nanos();
        match self.inner.write_batch(&drained, durability) {
            Ok(()) => {
                let elapsed_us = self.inner.clock.now_nanos().saturating_sub(started) / 1_000;
                Ok(self.inner.report(drained.len(), elapsed_us))
            }
            Err(reason) => {
                self.inner.degrade(&drained);
                Err(ImeError::DataReadonly { reason })
            }
        }
    }

    /// How many records are waiting to be flushed.
    ///
    /// Test-only: the count is an implementation detail, and production code reads it
    /// through [`UserDb::record_count`] after a flush.
    #[cfg(test)]
    fn pending_len(&self) -> usize {
        lock(&self.inner.pending).entries.len()
    }
}

impl UserFreqSource for UserDb {
    fn freq(&self, key: &str) -> u32 {
        {
            let mut cache = lock(&self.inner.cache);
            if let Some(hit) = cache.get(key) {
                return hit;
            }
        }
        // The cache holds committed plus pending counts, so a miss has to add the delta
        // the next flush will write; otherwise a word recorded a keystroke ago would score
        // as if it had never been typed.
        let committed = self.inner.committed(key);
        let delta = {
            let pending = lock(&self.inner.pending);
            pending.entries.get(key).map_or(0, |entry| entry.count)
        };
        let total = committed.saturating_add(delta);
        if total > 0 {
            lock(&self.inner.cache).insert(key, total);
        }
        total
    }

    fn record(&self, key: &str, _weight_hint: u16) {
        if self.is_readonly() {
            return;
        }
        let now_nanos = self.inner.clock.now_nanos();
        let now_ms = self.inner.clock.now_ms();
        self.inner
            .last_record_nanos
            .store(now_nanos, Ordering::Relaxed);
        let due = {
            let mut pending = lock(&self.inner.pending);
            let entry = pending.entries.entry(Box::from(key)).or_insert(Pending {
                count: 0,
                last_used_ms: now_ms,
            });
            entry.count = entry.count.saturating_add(1);
            entry.last_used_ms = now_ms;
            let batch = self.inner.batch.load(Ordering::Relaxed);
            let interval_ms = self.inner.interval_ms.load(Ordering::Relaxed);
            let held = pending.entries.len();
            let elapsed = now_nanos.saturating_sub(pending.last_flush_nanos);
            held >= batch
                || held >= PENDING_CAPACITY
                || elapsed >= interval_ms.saturating_mul(1_000_000)
        };
        {
            let mut cache = lock(&self.inner.cache);
            cache.bump(key);
        }
        if due {
            // The flush runs on the caller's thread on purpose: one session's records have
            // to reach the store in the order they were committed, and the alternative --
            // a writer thread -- would need a queue and a wakeup for a batch of thirty-two
            // small writes. The failure is not swallowed: it flips the store to read-only,
            // which is what the caller polls.
            let _ = self.commit_inner(Durability::Eventual);
        }
    }

    fn is_user_word(&self, _key: &str) -> bool {
        // Version 1 stores frequencies only: a key is a word the user typed, not a word
        // the user coined. The table that marks a coined word, and the write path that
        // fills it, arrive with the Phase 2 user-word support.
        false
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
