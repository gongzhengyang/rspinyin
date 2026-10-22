//! The user's own words: the entries the commit path enrolled, and the chain that puts
//! them ahead of the system dictionary.
//!
//! Responsibility: hold the words a user coined by choosing characters one at a time --
//! the two-character combinations a commit confirms -- and answer the frozen [`Lexicon`]
//! trait for them, so the host can compose them ahead of the system dictionary and the
//! decoder never learns that a second table exists. The chain is the composition: the
//! front lexicon's words are served under the same keys the system dictionary spells, with
//! a weight that puts each coined word above every system word of its key, and a system
//! entry that spells the same text as a coined one is merged into it rather than offered
//! twice.
//!
//! Boundaries: this module owns the in-memory table and nothing else. It decides nothing
//! about *when* a word is coined -- that is the commit path's -- and nothing about *where*
//! the words persist -- that is the user database's, whose flush writes the same entries
//! the coin path inserts here. A word the store could not persist is still served for the
//! session; a store in read-only mode enrols nothing at all, which is the caller's to
//! check through the handle.
//!
//! # Weights
//!
//! A coined word's own weight grows with its commits, but what puts it first is the chain:
//! the effective weight of a coined entry is the heaviest system weight under its key,
//! raised by a quarter, so the entry wins the key it was coined under without pretending
//! to be evidence about any other key. The language model still decides between
//! segmentations; coining changes which words exist, not how a reading is scored.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::user_db::UserDb;

use ime_types::{ImeError, Lexicon, SyllableId, WordFlags, WordIter, WordRef};

use crate::user_db::USER_WORD_CAP;

/// The weight a coined entry carries before the chain raises it: one quarter of the
/// typical top weight a compiled dictionary stores under a key.
///
/// The value exists so that a key with no system words at all still serves the coined
/// entry at a competitive weight; the chain's raise above the key's system maximum is
/// what does the ranking work wherever system words exist.
pub const COINED_BASE_WEIGHT: u32 = 1 << 18;

/// How much one recorded commit adds to a coined entry's own weight.
pub const COINED_COMMIT_STEP: u32 = 4_096;

/// The raise a coined entry is given over the heaviest system weight of its key.
///
/// A quarter is the librarian's margin: enough that the entry wins the key at equal
/// language-model evidence by a step the ordering cannot miss, not enough to claim a
/// different key's competition.
pub const COINED_RAISE_NUMERATOR: u32 = 5;
pub const COINED_RAISE_DENOMINATOR: u32 = 4;

/// One coined entry: the word, how often the user has confirmed it, and whether the user
/// pinned it against eviction.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UserWord {
    /// The word's text.
    text: Box<str>,
    /// How many commits enrolled or confirmed it.
    commits: u32,
    /// Whether the user pinned it; a pinned entry is never evicted.
    pinned: bool,
    /// When the entry was last coined or confirmed, in the lexicon's own usage clock.
    last_used: u64,
}

/// The user's own words, keyed by the syllable string they were coined under.
///
/// The table is small by construction -- entries are only ever added by a commit the user
/// made -- and the cap it shares with the frequency store is the bound that keeps it from
/// growing without one.
#[derive(Default)]
pub struct UserLexicon {
    /// The entries, keyed by the syllable string of the composition they were coined
    /// from, separators included -- the same spelling a decode looks them up under. A
    /// key holds a list because homophones are coined independently: the user who
    /// enrolls both word of one reading does not want the second to replace the first.
    words: HashMap<Box<str>, Vec<UserWord>>,
    /// The usage clock the entries' recency is read against.
    clock: u64,
}

impl UserLexicon {
    /// The empty table.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the table over rows read back from the store: `(key, text, commits)`.
    ///
    /// The entries land in the order the store handed them over, which is the order the
    /// coin path wrote them in; the usage clock starts past all of them, so the first
    /// coin of the session is the most recent entry.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String, u32)>) -> Self {
        let mut lexicon = Self::new();
        for (key, text, commits) in rows {
            lexicon.clock += 1;
            let clock = lexicon.clock;
            lexicon
                .words
                .entry(Box::from(key.as_str()))
                .or_default()
                .push(UserWord {
                    text: Box::from(text.as_str()),
                    commits,
                    pinned: false,
                    last_used: clock,
                });
        }
        lexicon
    }

    /// Enrols `text` under `key`, or confirms it with one more commit.
    ///
    /// # Returns
    ///
    /// Whether the entry is new: the caller persists an insert either way, but a new
    /// entry is the one a diagnostic would be worth for.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn coin(&mut self, key: &str, text: &str) -> bool {
        self.clock += 1;
        let clock = self.clock;
        if let Some(entries) = self.words.get_mut(key) {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.text.as_ref() == text) {
                entry.commits = entry.commits.saturating_add(1);
                entry.last_used = clock;
                return false;
            }
        }
        self.evict_if_full();
        self.words
            .entry(Box::from(key))
            .or_default()
            .push(UserWord {
                text: Box::from(text),
                commits: 1,
                pinned: false,
                last_used: clock,
            });
        true
    }

    /// Makes room for one more entry when the table is at the cap it shares with the
    /// frequency store.
    ///
    /// The least recently used entry goes, unless the user pinned it; a table of pinned
    /// entries at the cap simply stops growing, which is the same refusal the frequency
    /// store's own eviction ends in.
    fn evict_if_full(&mut self) {
        let entries = usize::try_from(USER_WORD_CAP).unwrap_or(usize::MAX);
        if self.words.values().map(Vec::len).sum::<usize>() < entries {
            return;
        }
        // The victim is the least recently used entry the user has not pinned, wherever
        // under whatever key it sits; a whole key never goes for one entry's sins.
        let victim = self
            .words
            .iter()
            .flat_map(|(key, entries)| {
                entries
                    .iter()
                    .filter(|entry| !entry.pinned)
                    .map(move |entry| (key.clone(), entry.last_used))
            })
            .min_by_key(|(_, last_used)| *last_used)
            .map(|(key, _)| key);
        let Some(key) = victim else {
            return;
        };
        if let Some(entries) = self.words.get_mut(&key) {
            let oldest = entries
                .iter()
                .filter(|entry| !entry.pinned)
                .min_by_key(|entry| entry.last_used)
                .map(|entry| entry.last_used);
            if let Some(oldest) = oldest {
                entries.retain(|entry| entry.last_used != oldest || entry.pinned);
            }
            if entries.is_empty() {
                self.words.remove(&key);
            }
        }
    }

    /// Drops every entry whose word is `text`, answering whether there was one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn forget_word(&mut self, text: &str) -> bool {
        let before: usize = self.words.values().map(Vec::len).sum();
        self.words.retain(|_, entries| {
            entries.retain(|entry| entry.text.as_ref() != text);
            !entries.is_empty()
        });
        let after: usize = self.words.values().map(Vec::len).sum();
        after != before
    }

    /// Flips the user's pin on every entry whose word is `text`, answering whether any
    /// entry moved.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn toggle_pinned(&mut self, text: &str) -> bool {
        let mut moved = false;
        for entries in self.words.values_mut() {
            for entry in entries {
                if entry.text.as_ref() == text {
                    entry.pinned = !entry.pinned;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Records the user's pin on every entry whose word is `text`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_pinned(&mut self, text: &str, pinned: bool) {
        for entries in self.words.values_mut() {
            for entry in entries {
                if entry.text.as_ref() == text {
                    entry.pinned = pinned;
                }
            }
        }
    }

    /// Whether any entry spells `text`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn knows_word(&self, text: &str) -> bool {
        self.words
            .values()
            .flatten()
            .any(|entry| entry.text.as_ref() == text)
    }

    /// The words one key holds, as the frozen contract's references.
    ///
    /// # Panics
    ///
    /// Never.
    fn words_for(&self, key: &str) -> Vec<WordRef<'_>> {
        match self.words.get(key) {
            Some(entries) => entries
                .iter()
                .map(|entry| WordRef {
                    text: &entry.text,
                    weight: Self::weight_of(entry),
                    syl_count: Self::syllables_of(key),
                    flags: WordFlags::USER,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// The weight one entry is served with.
    fn weight_of(entry: &UserWord) -> u32 {
        COINED_BASE_WEIGHT.saturating_add(entry.commits.saturating_mul(COINED_COMMIT_STEP))
    }

    /// How many syllables a key spells: the separator count plus one, capped at the
    /// contract's own ceiling.
    fn syllables_of(key: &str) -> u8 {
        u8::try_from(key.split('\'').count().max(1)).unwrap_or(u8::MAX)
    }
}

impl Lexicon for UserLexicon {
    /// The words coined under `key`.
    ///
    /// # Errors
    ///
    /// None: an in-memory table cannot refuse a lookup.
    ///
    /// # Panics
    ///
    /// Never.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(self.words_for(key)))
    }

    /// The coined entries whose key starts with `prefix`, the abbreviation query's shape.
    ///
    /// The scan is linear over the table, which is the price of an in-memory map with no
    /// index; the table is bounded by the commit path, not by the dictionary's size, so
    /// the scan is bounded by what the user actually coined.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    fn prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let mut words: Vec<WordRef<'_>> = self
            .words
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .flat_map(|(key, entries)| {
                entries.iter().map(move |entry| WordRef {
                    text: &entry.text,
                    weight: Self::weight_of(entry),
                    syl_count: Self::syllables_of(key),
                    flags: WordFlags::USER,
                })
            })
            .collect();
        words.sort_by_key(|word| std::cmp::Reverse(word.weight));
        words.truncate(limit);
        Ok(WordIter::from_vec(words))
    }

    /// The single-character words of one syllable, which an in-memory coin table never
    /// holds: entries are coined over two keys, so there is nothing to answer.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }
}

/// The system dictionary with the user's own words ahead of it.
///
/// Assembled per decode over whatever container is live, which is what makes a hot swap
/// reach the user's words too: the front is the user's, which no swap replaces, and the
/// back is the container of the moment.
#[derive(Clone, Copy)]
pub struct ChainedLexicon<'a> {
    /// The user's own words.
    front: &'a UserLexicon,
    /// The system dictionary of the moment.
    back: &'a dyn Lexicon,
}

impl<'a> ChainedLexicon<'a> {
    /// Chains the user's words ahead of `back`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(front: &'a UserLexicon, back: &'a dyn Lexicon) -> Self {
        Self { front, back }
    }
}

#[cfg(test)]
mod tests;

impl Lexicon for ChainedLexicon<'_> {
    /// The words of `key`: the user's first, then the system's, merged.
    ///
    /// A system entry that spells the same text as a coined one is dropped -- the merged
    /// pair is one candidate, not two -- and the coined entry is served at the chain's
    /// weight: the heaviest system weight of the key, raised by a quarter, or its own
    /// commit-grown weight when that is already higher. This is the whole of what "the
    /// user's words outrank the system's" means, and it holds by construction of the list
    /// this returns.
    ///
    /// With nothing coined under the key the answer is the system dictionary's own, with
    /// no allocation: the chain costs nothing where the user coined nothing.
    ///
    /// # Errors
    ///
    /// The back lexicon's, which the pass-through hands on.
    ///
    /// # Panics
    ///
    /// Never.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let front = self.front.words_for(key);
        if front.is_empty() {
            return self.back.lookup(key);
        }
        let back: Vec<WordRef<'_>> = self.back.lookup(key)?.collect();
        // The raise over the key's own system maximum, applied to every coined entry:
        // max(system_max, own) raised by the quarter that puts the entry ahead of the
        // key without pretending to rank another key.
        let system_max = back.iter().map(|word| word.weight).max().unwrap_or(0);
        let raised = system_max
            .saturating_mul(COINED_RAISE_NUMERATOR)
            .saturating_div(COINED_RAISE_DENOMINATOR);
        let mut merged: Vec<WordRef<'_>> = Vec::with_capacity(front.len() + back.len());
        for mut word in front {
            word.weight = word.weight.max(raised);
            merged.push(word);
        }
        for word in back {
            if merged.iter().any(|front| front.text == word.text) {
                continue;
            }
            merged.push(word);
        }
        Ok(WordIter::from_vec(merged))
    }

    /// Prefix enumeration, the abbreviation query's shape: the user's entries scanned,
    /// then the system's, merged the same way [`ChainedLexicon::lookup`] merges.
    ///
    /// # Errors
    ///
    /// The back lexicon's.
    ///
    /// # Panics
    ///
    /// Never.
    fn prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let front: Vec<WordRef<'_>> = self.front.prefix(prefix, limit)?.collect();
        let back = self.back.prefix(prefix, limit)?;
        if front.is_empty() {
            return Ok(back);
        }
        let mut merged: Vec<WordRef<'_>> = front;
        for word in back {
            if merged.iter().any(|front| front.text == word.text) {
                continue;
            }
            merged.push(word);
        }
        merged.sort_by_key(|word| std::cmp::Reverse(word.weight));
        merged.truncate(limit);
        Ok(WordIter::from_vec(merged))
    }

    /// Single-character fallbacks, which only the system dictionary has: entries are
    /// coined over two keys, so the front has nothing to add.
    ///
    /// # Errors
    ///
    /// The back lexicon's.
    ///
    /// # Panics
    ///
    /// Never.
    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        self.back.fallback_single(syl, limit)
    }
}

/// The user's lexicon and the store its entries persist through, as one handle.
///
/// The split the two halves keep is the one the commit path needs: the lexicon is the
/// decode-side table the router's step locks and reads, and the store is where the same
/// entries reach the disk through the flush thread's own window. A coin lands in both --
/// the store first, because a store that refuses (read-only mode) means the entry is not
/// enrolled at all -- and a forget takes the word out of both, the store through the
/// frozen trait's own call and the lexicon through [`UserLexiconHandle::forget_word`].
///
/// # Locking
///
/// The lexicon's lock is taken once per decode by the router and held for the step's
/// whole run; every mutation this handle makes happens outside that guard, which is why
/// the methods take the lock themselves and never re-enter. The store's own lock is the
/// leaf the user database documents; the lexicon lock is never held while it is taken.
pub struct UserLexiconHandle {
    /// The decode-side table.
    lexicon: Mutex<UserLexicon>,
    /// The store the entries persist through.
    store: UserDb,
}

impl UserLexiconHandle {
    /// Builds the handle over `store`, loading the entries the store already holds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn hydrate(store: UserDb) -> Self {
        let rows = store.coined_rows().unwrap_or_default();
        Self {
            lexicon: Mutex::new(UserLexicon::from_rows(rows)),
            store,
        }
    }

    /// The decode-side table, locked for the caller's scope.
    ///
    /// The router holds this guard across one step, which is what lets a decode read the
    /// user's words without a lock per lookup; everything that mutates the table waits
    /// for the guard to go.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lock(&self) -> MutexGuard<'_, UserLexicon> {
        match self.lexicon.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Enrols the pairs a commit reported, and answers whether any landed.
    ///
    /// The store takes each pair first: a read-only store enrols nothing, and the
    /// in-memory table follows the store so the two never disagree about what exists.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn coin(&self, pairs: &[(String, String)]) -> bool {
        if pairs.is_empty() {
            return false;
        }
        let mut lexicon = self.lock();
        let mut any = false;
        for (key, text) in pairs {
            if self.store.record_coin(key, text) {
                lexicon.coin(key, text);
                any = true;
            }
        }
        any
    }

    /// Drops every entry whose word is `text`, from the in-memory table.
    ///
    /// The store's half of the removal travels the frozen trait's own `forget` call,
    /// which the executor runs; this is the memory side, taken after the guard that made
    /// the caller defer it has gone.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn forget_word(&self, word: &str) -> bool {
        self.lock().forget_word(word)
    }

    /// Records the user's pin on every entry whose word is `text`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_pinned(&self, word: &str, pinned: bool) {
        self.lock().set_pinned(word, pinned);
    }

    /// Flips the user's pin on every entry whose word is `text`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn toggle_pin(&self, word: &str) {
        self.lock().toggle_pinned(word);
    }
}
