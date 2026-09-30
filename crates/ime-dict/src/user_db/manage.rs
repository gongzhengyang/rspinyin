//! The management primitives: dropping one learned word, and reading the store back.
//!
//! Responsibility: own the two operations a user performs *on* their word list rather than
//! with it -- forget one word, and page through what has been learned -- and the outcome
//! vocabulary the diagnostics surface reports them with.
//!
//! Boundaries: this module never decodes, never ranks and never writes a file. A removal is
//! recorded as a tombstone in the store's pending delta and reaches the disk with the next
//! flush, exactly like a record does, so the key that asks for it costs the input path one
//! map insert rather than a write transaction (`ASM-04`, `ASM-20`). The enumeration is the
//! management path: it is allowed to read the store, because nothing calls it while the
//! user is typing.

use std::collections::BinaryHeap;

use super::*;

use super::cache::lock_cache;
use super::flush::Delta;

// The parent module imports `Ordering` for its atomics; this module orders records. An
// explicit import wins over a glob, so the name below means the comparison here and the
// one atomic load that needs the other one spells it out in full.
use std::cmp::Ordering;

/// Most records one [`UserDb::list_words`] call returns.
///
/// The management surface draws a page of them; the ceiling keeps the call's footprint a
/// function of the page rather than of the store, which is what the enumeration's memory
/// budget is stated against.
pub const MAX_LIST_LIMIT: u16 = 200;

/// One learned word, as the management surface sees it.
///
/// The key is the word the user committed, which is what the frequency was recorded
/// against; `weight` is what [`UserFreqSource::freq`] answers, so a word recorded but not
/// yet flushed reports the same number here as it does to the decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRecord {
    /// The word the user committed, and the key every other call uses.
    pub key: String,
    /// Accumulated frequency, saturating at `u32::MAX`.
    pub weight: u32,
    /// Last use, in unix milliseconds; drives eviction and the enumeration's order.
    pub last_used_unix: u64,
    /// First learning time, in unix milliseconds; zero for a record written before this
    /// build started stamping it.
    pub created_unix: u64,
    /// Whether the user pinned the word, which exempts it from eviction.
    pub pinned: bool,
}

/// The reason a removal did or did not happen, for the diagnostics surface.
///
/// [`UserFreqSource::forget`] answers `true` for [`ForgetOutcome::Removed`] and `false` for
/// the other three, because the frozen trait asks only whether a record went away. The
/// distinction between "there was nothing there" and "the store is read-only" is what the
/// host layer needs in order to report `dict/user-word-not-found` or `data/readonly-mode`,
/// which is why the store also offers [`UserDb::forget_with_outcome`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgetOutcome {
    /// A record was removed.
    Removed,
    /// The key was never learned.
    NotFound,
    /// The key is pinned; the user must unpin it first.
    Pinned,
    /// The store is in read-only mode.
    Readonly,
}

impl UserDb {
    /// Drops `key` and reports why the removal did or did not happen.
    ///
    /// This is the form the host layer reports a diagnostic from; the frozen
    /// [`UserFreqSource::forget`] is the same call with the outcome reduced to a `bool`.
    ///
    /// The removal is batched: the key is tombstoned in memory and deleted from the store
    /// by the next flush, so the call opens no write transaction and does not block the
    /// thread that handles the keystroke. Until that flush the tombstone is what keeps the
    /// word out of the candidate list -- [`UserFreqSource::freq`] answers zero for it -- and
    /// a crash before the flush costs the removal, exactly as it costs the records of the
    /// same window (`ASM-20`).
    ///
    /// The presence check runs against what the store holds in memory. For a store loaded
    /// at open -- every store under [`HYDRATE_CAP`] records, and so every ordinary one --
    /// that is the whole store and the answer is exact. A store past the ceiling keeps only
    /// the keys this session has touched, so a key it has not seen reports
    /// [`ForgetOutcome::NotFound`] although the tombstone still removes its row; the
    /// alternative is a read transaction on the input path, which `ASM-04` forbids. The pin
    /// check degrades the same way, and for the same reason.
    pub fn forget_with_outcome(&self, key: &str) -> ForgetOutcome {
        self.inner.forget(key)
    }

    /// Lists learned words, `limit` of them starting at `offset`, most recently used first.
    ///
    /// The order is `last_used_unix` descending with the key ascending as the tie-break, so
    /// a page is reproducible: two records sharing a stamp always come back in the same
    /// order. `limit` is clamped to [`MAX_LIST_LIMIT`], and an `offset` past the end answers
    /// an empty page rather than an error.
    ///
    /// The working set is the page asked for -- `offset + limit` records, never the whole
    /// store -- so a deep page costs more than a shallow one and a page of a
    /// half-million-record store costs what a page of a ten-record one costs.
    ///
    /// This is the store's own enumeration. [`UserFreqSource::list`] is the frozen borrowed
    /// form of it, which a store whose key set changes cannot serve; that implementation
    /// carries the note explaining why.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the read transaction fails.
    pub fn list_words(&self, offset: u64, limit: u16) -> Result<Vec<UserRecord>, ImeError> {
        self.inner.list_records(offset, limit)
    }
}

impl Inner {
    /// Records a tombstone for `key` and reports what the store knew about it.
    ///
    /// The checks run in the order the outcome names them: a read-only store changes nothing
    /// at all, a pinned record is protected, and everything else is tombstoned -- including
    /// a key the store has never seen, because the delete the flush performs is idempotent
    /// and a caller that forgets a word twice must not be told the second call did something
    /// different from the first.
    ///
    /// The tombstone set is bounded by [`PENDING_CAPACITY`], the same ceiling the delta map
    /// has, and a flush is started when it is reached. The batch size and the interval are
    /// deliberately *not* consulted here: they are the record path's policy, and a removal
    /// is not urgent -- the tombstone already hides the word, and the next flush the user's
    /// typing earns writes it, as does the shutdown flush.
    pub(super) fn forget(&self, key: &str) -> ForgetOutcome {
        if self.readonly.load(std::sync::atomic::Ordering::Acquire) {
            return ForgetOutcome::Readonly;
        }
        // The lock order the module fixes: `committed` before `pending`; `cache` and
        // `pinned` are leaves, never held while another lock is taken.
        let present = if self.is_hydrated {
            let committed = lock(&self.committed);
            let known = committed.contains_key(key);
            let pending = lock(&self.pending);
            known || pending.entries.contains_key(key)
        } else {
            // The cache is consulted before the delta map rather than inside it, so that the
            // two are never held at once: the module's lock order keeps `cache` a leaf. What
            // an entry holds is the file's value, so a positive one is a word the file has.
            let cached = match lock_cache(&self.cache) {
                Some(mut cache) => cache.get(key).is_some_and(|held| held > 0),
                None => false,
            };
            let pending = lock(&self.pending);
            pending.entries.contains_key(key) || cached
        };
        if lock(&self.pinned).contains(key) {
            return ForgetOutcome::Pinned;
        }
        let due = {
            let mut pending = lock(&self.pending);
            // A key with an unflushed delta loses it: the user is saying the word is not
            // theirs, and a count recorded before that is not evidence to the contrary.
            pending.entries.remove(key);
            pending.removed.insert(Box::from(key));
            pending.removed.len() >= PENDING_CAPACITY
        };
        if due {
            // The write is the flush thread's, for the reason `record`'s is: a tombstone
            // reaches the disk in a write transaction, and this runs inside a key callback.
            // The failure is not swallowed either -- it flips the store to read-only, which
            // the caller polls.
            self.request_flush();
        }
        if present {
            ForgetOutcome::Removed
        } else {
            ForgetOutcome::NotFound
        }
    }

    /// Reads the store's records and returns one page of them, most recently used first.
    ///
    /// The page is assembled from the file plus the deltas that have not been flushed, and
    /// the keys tombstoned in the meantime are dropped from it, so what a caller sees here
    /// is what [`UserFreqSource::freq`] would answer for each key.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when a read transaction fails.
    pub(super) fn list_records(
        &self,
        offset: u64,
        limit: u16,
    ) -> Result<Vec<UserRecord>, ImeError> {
        let limit = usize::from(limit.min(MAX_LIST_LIMIT));
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (removed, unclaimed) = self.pending_snapshot();
        let mut unclaimed = self.stamp_snapshot(unclaimed);
        let skip = usize::try_from(offset).unwrap_or(usize::MAX);
        let asked = offset.saturating_add(u64::try_from(limit).unwrap_or(u64::MAX));
        let mut kept = TopRows::new(usize::try_from(asked).unwrap_or(usize::MAX));
        self.scan_records(&removed, &mut unclaimed, &mut kept)?;
        // What the delta map still holds was not in the file: a word learned a keystroke ago
        // is a word the user can already see and drop.
        for (key, delta) in unclaimed {
            kept.push(UserRecord {
                key: String::from(key.as_ref()),
                weight: delta.count,
                last_used_unix: delta.last_used_ms,
                created_unix: 0,
                pinned: false,
            });
        }
        Ok(kept
            .into_sorted()
            .into_iter()
            .skip(skip)
            .take(limit)
            .collect())
    }

    /// Copies the pending delta out of its lock.
    ///
    /// The enumeration scans the store, and holding the delta lock across a scan would put
    /// a whole read transaction between a keystroke and its record. The copy is bounded by
    /// [`PENDING_CAPACITY`] entries, so it is the delta's own ceiling that bounds this.
    pub(super) fn pending_snapshot(&self) -> (HashSet<Box<str>>, HashMap<Box<str>, Pending>) {
        let pending = lock(&self.pending);
        (pending.removed.clone(), pending.entries.clone())
    }

    /// Feeds every record the store holds into `kept`, folded with the unflushed delta.
    ///
    /// One read transaction covers both tables, so the page cannot mix two generations of
    /// the store. A record with no metadata row is one an older build wrote: it becomes an
    /// unpinned record with no creation time, which is exactly what it was.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the transaction or a table fails.
    fn scan_records(
        &self,
        removed: &HashSet<Box<str>>,
        unclaimed: &mut HashMap<Box<str>, Delta>,
        kept: &mut TopRows,
    ) -> Result<(), ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let words = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(&self.path, &error))?;
        let meta = txn
            .open_table(USER_META)
            .map_err(|error| store_error(&self.path, &error))?;
        for entry in words
            .iter()
            .map_err(|error| store_error(&self.path, &error))?
        {
            let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
            let key = key.value();
            if removed.contains(key) {
                continue;
            }
            let (weight, last_used_unix) = value.value();
            let held = meta
                .get(key)
                .map_err(|error| store_error(&self.path, &error))?;
            let (created_unix, pinned) = held.map_or((0, 0), |guard| guard.value());
            let mut row = UserRecord {
                key: String::from(key),
                weight,
                last_used_unix,
                created_unix,
                pinned: pinned != 0,
            };
            if let Some(delta) = unclaimed.remove(key) {
                row.weight = row.weight.saturating_add(delta.count);
                row.last_used_unix = row.last_used_unix.max(delta.last_used_ms);
            }
            kept.push(row);
        }
        Ok(())
    }
}

/// Orders two records the way the enumeration returns them: most recently used first, and
/// the key ascending to break a tie.
fn rank(left: &UserRecord, right: &UserRecord) -> Ordering {
    right
        .last_used_unix
        .cmp(&left.last_used_unix)
        .then_with(|| left.key.cmp(&right.key))
}

/// One record in the enumeration's ordering.
///
/// A newtype rather than an `Ord` on [`UserRecord`]: the order belongs to the enumeration
/// and not to the value the caller reads.
#[derive(Debug)]
struct Ranked(UserRecord);

impl Ord for Ranked {
    /// [`rank`] itself, because `BinaryHeap` keeps its greatest at the top and the row the
    /// enumeration returns last is the one a better candidate replaces. Reversing it here
    /// would put the *best* row on top, so every push would evict the best row it had and
    /// the heap would end up holding the worst `rank` rows of the store.
    fn cmp(&self, other: &Self) -> Ordering {
        rank(&self.0, &other.0)
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

/// The best `rank` records a scan has seen, and no more.
///
/// The enumeration may not materialize the store, so every candidate the scan produces goes
/// through this set: it keeps the rows the caller's page is cut from and drops the rest.
/// The heap's top is the worst row it holds, which is the one a better candidate replaces,
/// so the set costs `O(log rank)` per record and `O(rank)` in memory whatever the store
/// holds.
#[derive(Debug)]
struct TopRows {
    /// How many rows the set keeps; the caller's `offset + limit`.
    rank: usize,
    /// The rows kept so far, worst first.
    rows: BinaryHeap<Ranked>,
}

impl TopRows {
    /// Creates a set that keeps `rank` rows.
    fn new(rank: usize) -> Self {
        Self {
            rank,
            rows: BinaryHeap::new(),
        }
    }

    /// Offers one row to the set, keeping it when it ranks inside the page.
    fn push(&mut self, row: UserRecord) {
        if self.rank == 0 {
            return;
        }
        let candidate = Ranked(row);
        if self.rows.len() < self.rank {
            self.rows.push(candidate);
            return;
        }
        let better = self
            .rows
            .peek()
            .is_some_and(|worst| rank(&candidate.0, &worst.0) == Ordering::Less);
        if better {
            self.rows.pop();
            self.rows.push(candidate);
        }
    }

    /// Returns the rows kept, in the order the enumeration hands them out.
    fn into_sorted(self) -> Vec<UserRecord> {
        let mut rows: Vec<UserRecord> = self.rows.into_iter().map(|ranked| ranked.0).collect();
        rows.sort_by(rank);
        rows
    }
}
