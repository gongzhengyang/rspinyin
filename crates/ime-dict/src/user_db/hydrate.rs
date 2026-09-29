//! The in-memory half of the frequency signal.
//!
//! Responsibility: hold the counts the store has flushed, load them at open when the store
//! is small enough to fit, and answer the reads that fall outside the loaded set from the
//! store itself. It owns the rules that keep memory and file in step -- what a successful
//! flush adopts, what a failed one leaves alone, and what the sweep forgets.
//!
//! Boundaries: this module never writes to the store. The write path calls [`Inner::adopt`]
//! and [`Inner::forget_committed`] to report what changed, and the open path calls
//! [`load_committed`]. It never decodes, never ranks and never evicts.
//!
//! Lock order: the store takes `committed` before `pending` everywhere, and `cache` is a
//! leaf that is never held while another lock is taken. The store's own read runs with no
//! lock held at all, which is what the cache is for: a read transaction is the one
//! operation in the store that can block for as long as the disk takes.

use super::*;

/// What the store loaded into memory at open.
///
/// The counts are what the decoder reads on the hot path; the pinned set is what the
/// keystroke that forgets a word reads, and it is kept separate because it is empty for
/// every store whose owner never pinned anything -- which is every store today.
#[derive(Debug, Default)]
pub(super) struct Loaded {
    /// The flushed count of every record.
    pub(super) counts: HashMap<Box<str>, u32>,
    /// The keys the user pinned, which eviction leaves alone.
    pub(super) pinned: HashSet<Box<str>>,
}

/// Reads the whole store into memory, or reports that it is too large to load.
///
/// The ceiling is a record count rather than a byte size because a record count is what a
/// policy can state; [`HYDRATE_CAP`] is the number the plugin opens with.
///
/// A store that cannot be read is treated exactly like one that is too large: the store
/// still opens, its reads fall back to the on-demand path, and the caller reports
/// [`LARGE_STORE_CODE`] for it. Refusing to open would cost the user the history the store
/// already holds over a condition the decode path survives -- the same trade
/// [`Inner::stored_freq`] makes when a single read fails.
pub(super) fn load_committed(db: &Database, cap: u64) -> Option<Loaded> {
    let txn = db.begin_read().ok()?;
    let table = txn.open_table(USER_WORDS).ok()?;
    let stored = table.len().ok()?;
    if stored > cap {
        return None;
    }
    // Sized up front: the count is known, and growing the table mid-load would copy it.
    let mut counts = HashMap::with_capacity(usize::try_from(stored).ok()?);
    for entry in table.iter().ok()? {
        let (key, value) = entry.ok()?;
        counts.insert(Box::from(key.value()), value.value().0);
    }
    // The pinned keys come along for the same reason the counts do: the keystroke that asks
    // to forget a word may not open a read transaction to find out whether it is protected.
    // Only the pinned ones are kept, so an ordinary store holds an empty set.
    let mut pinned = HashSet::new();
    let meta = txn.open_table(USER_META).ok()?;
    for entry in meta.iter().ok()? {
        let (key, value) = entry.ok()?;
        if value.value().1 != 0 {
            pinned.insert(Box::from(key.value()));
        }
    }
    Some(Loaded { counts, pinned })
}

impl Inner {
    /// Reads the flushed frequency of `key`; zero when it is absent or unreadable.
    ///
    /// A read failure is deliberately not an error: `freq` returns a count, and a store
    /// that cannot be read must still let the user type. Zero means "no boost", the same
    /// answer as a word that was never recorded.
    ///
    /// This is the fallback path's reader. A store whose counts were loaded answers from
    /// [`Inner::committed`] and never reaches it.
    fn stored_freq(&self, key: &str) -> u32 {
        #[cfg(test)]
        note_store_read();
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

    /// Reads one frequency from the store, remembering the miss.
    ///
    /// Without the cache a word the user has never typed would open a read transaction on
    /// every one of the hundreds of edges that name it in a single decode. The cache turns
    /// that into one transaction per distinct word per session.
    pub(super) fn on_demand(&self, key: &str) -> u32 {
        // A key the user has just forgotten is answered before the cache: the cached total
        // is what the file held when it was read, and the tombstone is the newer truth.
        if lock(&self.pending).removed.contains(key) {
            return 0;
        }
        {
            let mut cache = lock(&self.cache);
            if let Some(hit) = cache.get(key) {
                return hit;
            }
        }
        // The read runs with no lock held: `cache` is a leaf, and a transaction is the one
        // operation here that can block for as long as the disk takes. The delta is read
        // afterwards, so the answer includes what has been recorded but not yet flushed.
        let committed = self.stored_freq(key);
        let delta = {
            let pending = lock(&self.pending);
            pending.entries.get(key).map_or(0, |entry| entry.count)
        };
        let total = committed.saturating_add(delta);
        lock(&self.cache).insert(key, total);
        total
    }

    /// Adopts what a successful flush wrote.
    ///
    /// The store's value for a key becomes what it held plus the delta, which is exactly
    /// what the write merged, so the map stays equal to the file. This is called after the
    /// commit returns: a failed flush rolled its transaction back and must leave the map
    /// alone, which is why the caller does not call it on that path.
    pub(super) fn adopt(&self, drained: &[(Box<str>, Pending)]) {
        if !self.is_hydrated {
            return;
        }
        let mut counts = lock(&self.committed);
        for (key, delta) in drained {
            let entry = counts.entry(key.clone()).or_insert(0);
            *entry = entry.saturating_add(delta.count);
        }
    }

    /// Drops `key` from the in-memory counts after the store lost it.
    ///
    /// The map is authoritative for reads, so a key the sweep removed from the file has to
    /// leave it too: a later `freq` of an evicted key must answer zero rather than a count
    /// the store no longer holds. The lock is taken per key rather than once per batch, for
    /// the same reason the sweep drops cache entries one at a time -- a record arriving
    /// while the sweep runs must never wait behind the whole batch.
    pub(super) fn forget_committed(&self, key: &str) {
        if self.is_hydrated {
            lock(&self.committed).remove(key);
        }
    }

    /// Adopts a record an import wrote.
    ///
    /// The import's transaction has committed, so the file holds the merged value; the
    /// in-memory map has to hold the same one, or a lookup would answer a count the store no
    /// longer has. The cache entry goes for the same reason it does after an eviction -- it
    /// is the fallback path's copy of the file, and the file changed under it -- and a
    /// tombstone for the key goes too: the import has just put the word back, and the
    /// removal it was waiting to flush is the older of the two statements.
    pub(super) fn adopt_imported(&self, key: &str, weight: u32, pinned: bool) {
        if self.is_hydrated {
            lock(&self.committed).insert(Box::from(key), weight);
        }
        lock(&self.cache).remove(key);
        lock(&self.pending).removed.remove(key);
        // The pin is what the next `forget` reads, so it has to follow the file rather than
        // wait for the next open.
        let mut pins = lock(&self.pinned);
        if pinned {
            pins.insert(Box::from(key));
        } else {
            pins.remove(key);
        }
    }
}
