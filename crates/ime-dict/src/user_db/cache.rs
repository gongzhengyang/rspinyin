//! The fallback path's frequency cache.
//!
//! Responsibility: remember the totals the on-demand reads produced, so that a store too
//! large to load does not open a read transaction for the same word twice in a session.
//!
//! Boundaries: this module owns the cache and nothing else. It holds no handle to the
//! store, knows nothing about flushing, and is a leaf -- the store never takes another lock
//! while this one is held. Only the fallback path consults it: a store whose counts were
//! loaded answers from memory and never reaches here.

use super::*;

/// A fixed-capacity second-chance cache of user frequencies.
///
/// The lookup path may not allocate and the footprint may not grow with the number of keys
/// ever seen, so a strict LRU -- which needs a list node or a fresh allocation per hit --
/// is out. This is the CLOCK approximation: one reference bit per slot plus a hand that
/// clears it, which keeps the hot keys resident at a constant cost per access and a
/// footprint bounded by the capacity.
///
/// Entries are totals, not committed counts: a word the store has never seen is remembered
/// as a zero, which is what keeps it to one read transaction per session, and `record`
/// bumps a cached entry rather than dropping it so that the total keeps including what has
/// been recorded but not yet flushed.
pub(super) struct LruCache {
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
    pub(super) fn new(capacity: usize) -> Self {
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
    pub(super) fn get(&mut self, key: &str) -> Option<u32> {
        let index = *self.index.get(key)?;
        let slot = self.slots.get_mut(index as usize)?;
        slot.referenced = true;
        Some(slot.value)
    }

    /// Stores `value` under `key`, replacing the least recently referenced slot once the
    /// cache is full.
    pub(super) fn insert(&mut self, key: &str, value: u32) {
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
    pub(super) fn bump(&mut self, key: &str) -> bool {
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
    pub(super) fn remove(&mut self, key: &str) {
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
