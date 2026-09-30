//! The fallback path's miss cache.
//!
//! Responsibility: remember the value the store answered for a word, so that a store too
//! large to load does not open a read transaction for the same word twice in a session.
//!
//! Boundaries: this module owns the cache and nothing else. It holds no handle to the store,
//! knows nothing about flushing, and is a leaf -- the store never takes another lock while
//! this one is held. Only the fallback path has one: a store whose counts were loaded is
//! opened without a cache, because its reads are answered from memory and an entry nothing
//! would consult is a lock the record path must not take.
//!
//! The value a slot holds is what the *file* holds and not the total a reader sees: the
//! pending delta is added on every read, exactly as the loaded path does. That is what makes
//! the cache correct without the record path touching it -- a total cached before a record
//! would otherwise under-report the word until the next flush refreshed it.

use super::*;

/// A fixed-capacity second-chance cache of the store's own values.
///
/// The lookup path may not allocate and the footprint may not grow with the number of keys
/// ever seen, so a strict LRU -- which needs a list node or a fresh allocation per hit --
/// is out. This is the CLOCK approximation: one reference bit per slot plus a hand that
/// clears it, which keeps the hot keys resident at a constant cost per access and a
/// footprint bounded by the capacity.
///
/// Entries are the values the store holds: a word the store has never seen is remembered as
/// a zero, which is what keeps it to one read transaction per session. The delta that has
/// been recorded but not yet flushed is *not* part of an entry -- the reader adds it -- so
/// the flush only has to bring a resident entry up to date with what it wrote, and a record
/// does not have to touch the cache at all.
pub(super) struct MissCache {
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

/// Locks the fallback path's cache, when the store has one.
///
/// A store whose counts were loaded is opened without a cache -- see the module
/// documentation -- so every caller of the fallback path has to ask for it rather than
/// assume it. The `Option` is what makes that structural instead of a flag tested on each
/// access, and it is why a loaded store's read path takes no cache lock at all.
pub(super) fn lock_cache(cache: &Option<Mutex<MissCache>>) -> Option<MutexGuard<'_, MissCache>> {
    cache.as_ref().map(|held| lock(held))
}

impl MissCache {
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

    /// Returns the stored value of `key`, marking the slot as referenced.
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

    /// Brings a resident entry up to date, reporting whether the key was resident.
    ///
    /// The flush calls this: a key's stored value changes from `held` to `held + delta` when
    /// its delta lands, and an entry a reader will consult has to follow, or the next read
    /// would answer a value the file no longer holds. A key the cache has never held is left
    /// out -- the cache records what was asked for, and an entry nobody reads would only
    /// displace one that somebody does. The reference bit is deliberately left alone: a
    /// refresh is not a read, and the hand is what decides what a *reader* keeps.
    pub(super) fn refresh(&mut self, key: &str, value: u32) -> bool {
        let Some(index) = self.index.get(key).copied() else {
            return false;
        };
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return false;
        };
        slot.value = value;
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

#[cfg(test)]
mod tests {
    use super::MissCache;

    #[test]
    fn test_miss_cache_reuses_a_freed_slot() {
        let mut cache = MissCache::new(3);
        cache.insert("a", 1);
        cache.insert("b", 2);
        cache.insert("c", 3);
        cache.remove("b");
        cache.insert("d", 4);
        assert_eq!(cache.get("a"), Some(1), "a resident key is not evicted");
        assert_eq!(cache.get("c"), Some(3));
        assert_eq!(cache.get("d"), Some(4));
        assert_eq!(cache.get("b"), None, "the removed key is gone");
    }

    #[test]
    fn test_miss_cache_is_bounded_by_its_capacity() {
        let mut cache = MissCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        cache.insert("c", 3);
        assert_eq!(
            cache.get("a"),
            None,
            "the third insert displaces one of two"
        );
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("c"), Some(3));
    }

    #[test]
    fn test_miss_cache_refreshes_a_resident_entry_and_adds_nothing() {
        let mut cache = MissCache::new(4);
        cache.insert("a", 1);
        assert!(cache.refresh("a", 9), "a resident key is brought up to date");
        assert_eq!(cache.get("a"), Some(9));
        assert!(
            !cache.refresh("b", 9),
            "a key no reader has asked for is not added"
        );
        assert_eq!(
            cache.get("b"),
            None,
            "so the entry a reader would need is still free"
        );
    }
}
