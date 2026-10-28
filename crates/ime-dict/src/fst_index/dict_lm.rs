//! The language model a compiled dictionary carries, read out of its mapping.
//!
//! Responsibility: answer the two questions the frozen [`LanguageModel`] trait asks --
//! the unigram score of a word and the conditional score of a word after another -- from
//! the container's `UNIGRAM` section, without copying or allocating anything.
//!
//! Boundaries: this module reads the table the compiler wrote and scores with it. It
//! does not build a table, does not read a file, and does not know how a word was
//! compiled; the lexicon it is built over owns the mapping, and the model is a view of
//! it. It never looks a word's text up, because the section stores no text.
//!
//! # Why this is not a map of strings
//!
//! The `UNIGRAM` section is `(hash, prob_q12)` records in ascending hash order, which is
//! exactly the index a `unigram(word)` query needs: hash the word, binary-search the
//! section, read the record. Nothing is copied, nothing is allocated, and every
//! comparison is an integer rather than a `strcmp` through a pointer chase -- which is
//! the whole difference from an in-memory map keyed by the word's own text.
//!
//! # Why the probability is table-mapped
//!
//! `prob_q12` has [`PROB_Q12_MAX`] possible values, so the Q8.8 log-probability each one
//! maps to is computed once, when the model is built, into a table indexed by the value.
//! The hot path is then one array index instead of the division and the segment lookup
//! [`log2_q8`] performs, and the table costs 16 KiB per model -- a model is built once
//! per dictionary load, so that is a load-time cost rather than a per-keystroke one.
//!
//! # Bigrams
//!
//! A version 2 container may carry a sorted `BIGRAM` section; version 1 cannot. A hit is
//! mapped through the same Q12-to-Q8.8 table as a unigram so both channels rank on one
//! scale. Every miss -- a missing section or an unknown pair -- answers what the in-memory
//! model answers: the word's unigram plus [`BIGRAM_MISS_PENALTY`]. That identical fallback
//! is why the two models can be swapped without changing candidate order. Successor
//! queries use the same fixed-width rows: binary search locates one predecessor's
//! interval and reads only that interval before ranking it by score.
//!
//! # Hash collisions
//!
//! A lookup compares 32-bit hashes and nothing else, so two words sharing a hash would
//! take each other's score. That cannot happen in a dictionary this project ships: the
//! compiler refuses a word list whose hashes collide, which is the only place with the
//! whole list in hand. A hand-built container can still hold a collision, and the answer
//! it produces is a wrong score for one word rather than an out-of-bounds read.
//!
//! # Concurrency
//!
//! `Send + Sync` and reentrant: the view is a read-only slice of a mapping the owning
//! lexicon keeps alive, the score table is immutable, and both methods are pure reads of
//! that memory. No method blocks, allocates or takes a lock.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use ime_core::lm::{BIGRAM_MISS_PENALTY, UNIGRAM_MISS, log2_q8};
use ime_types::{LanguageModel, WordId};

use crate::format::{
    BIGRAM_ENTRY_SIZE, PROB_Q12_MAX, UNIGRAM_ENTRY_SIZE, hash_word, read_u16, read_u32,
};

use super::FstLexicon;

/// Entries in the score table: one per Q12 probability a record can carry, plus zero.
const SCALE_LEN: usize = PROB_Q12_MAX as usize + 1;

/// A verified `BIGRAM` section: sorted fixed-width rows plus the shared score mapping.
///
/// The rows were bounds-checked, order-checked and pad-checked by the container reader,
/// but lookup still reads through slices and checked arithmetic. A damaged row therefore
/// becomes a miss rather than a crash, and the caller applies its documented miss fallback.
pub(crate) struct BigramTable {
    /// `(previous hash, next hash, probability)` rows in ascending pair-key order.
    records: &'static [u8],
}

impl BigramTable {
    /// Builds the table, or returns `None` when the section is absent.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(crate) fn new(records: &'static [u8]) -> Option<Self> {
        (!records.is_empty()).then_some(Self { records })
    }

    /// Returns how many complete rows the section holds.
    pub(crate) fn count(&self) -> u32 {
        u32::try_from(self.records.len() / BIGRAM_ENTRY_SIZE).unwrap_or(u32::MAX)
    }

    /// Returns the conditional score for one pair, or `None` for a miss.
    ///
    /// A miss is also the safe answer if a row no longer looks complete: the container
    /// reader should have rejected that file, and a miss keeps the model usable without
    /// repeating validation work on the hot path.
    pub(crate) fn prob_q8(&self, prev: &str, word: &str, scale: &[i32]) -> Option<i32> {
        let target = u64::from(hash_word(prev)) << 32 | u64::from(hash_word(word));
        let (mut low, mut high) = (0u32, self.count());
        while low < high {
            let middle = low + (high - low) / 2;
            let (pair_key, prob_q12) = self.record(middle)?;
            if pair_key == target {
                return Some(
                    scale
                        .get(usize::from(prob_q12))
                        .copied()
                        .unwrap_or(UNIGRAM_MISS),
                );
            }
            if pair_key < target {
                low = middle.saturating_add(1);
            } else {
                high = middle;
            }
        }
        None
    }

    /// Returns the scored successors whose high pair-key half equals `prev`.
    ///
    /// The pair key stores the predecessor in its upper half and the successor in its
    /// lower half, so one predecessor's rows are contiguous. The lower-bound search finds
    /// that segment and the scan stops at its end rather than visiting later
    /// predecessors. A bounded heap retains only the best `limit` rows, ranked first by
    /// descending score and then by ascending successor id. A damaged row safely
    /// truncates the segment instead of reading outside the section.
    fn successors(&self, prev: WordId, limit: usize, scale: &[i32]) -> Vec<(WordId, i32)> {
        let mut best = BinaryHeap::with_capacity(limit);
        if limit == 0 {
            return Vec::new();
        }
        let prefix = u64::from(prev.value()) << 32;
        let mut index = self.lower_bound(prefix);
        while let Some((pair_key, prob_q12)) = self.record(index) {
            if pair_key >> 32 != u64::from(prev.value()) {
                break;
            }
            let score = scale
                .get(usize::from(prob_q12))
                .copied()
                .unwrap_or(UNIGRAM_MISS);
            let next = WordId::new((pair_key & u64::from(u32::MAX)) as u32);
            let candidate = Reverse((score, next.value()));
            if best.len() < limit {
                best.push(candidate);
            } else if best.peek().is_some_and(|worst| candidate > *worst) {
                best.pop();
                best.push(candidate);
            }
            index = index.saturating_add(1);
        }

        let mut successors = Vec::with_capacity(best.len());
        while let Some(Reverse((score, next))) = best.pop() {
            successors.push((WordId::new(next), score));
        }
        successors.reverse();
        successors
    }

    /// Returns the first row whose pair key is at least `target`.
    fn lower_bound(&self, target: u64) -> u32 {
        let (mut low, mut high) = (0u32, self.count());
        while low < high {
            let middle = low + (high - low) / 2;
            let Some((pair_key, _)) = self.record(middle) else {
                return low;
            };
            if pair_key < target {
                low = middle.saturating_add(1);
            } else {
                high = middle;
            }
        }
        low
    }

    /// Returns the record at `index`, or `None` when the row is unavailable.
    fn record(&self, index: u32) -> Option<(u64, u16)> {
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(BIGRAM_ENTRY_SIZE)?;
        let end = start.checked_add(BIGRAM_ENTRY_SIZE)?;
        let row = self.records.get(start..end)?;
        let mut pair_key = [0; size_of::<u64>()];
        pair_key.copy_from_slice(row.first_chunk::<8>()?);
        let mut probability = [0; size_of::<u16>()];
        probability.copy_from_slice(row.get(8..10)?.first_chunk::<2>()?);
        Some((
            u64::from_le_bytes(pair_key),
            u16::from_le_bytes(probability),
        ))
    }
}
/// The language model of a compiled dictionary, read straight out of its mapping.
///
/// The view is borrowed for `'static` for the same reason the lexicon's own views are:
/// the lexicon owns the mapping for its whole life, never borrows it mutably and never
/// remaps it, so a model built over it stays valid for as long as the lexicon does. A
/// `DictLm` must not outlive the [`FstLexicon`] it was built from.
///
/// # Examples
///
/// ```
/// use ime_dict::format::{
///     DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer,
/// };
/// use ime_dict::fst_index::{DictLm, FstLexicon};
/// use ime_types::LanguageModel;
///
/// // One key, one record, one word, and the unigram table that scores it. The table is
/// // sorted by hash, which a single entry trivially is.
/// let mut index = fst::MapBuilder::memory();
/// let packed = pack_fst_value(0, 1).expect("packing the word-list range");
/// index.insert("ni".as_bytes(), packed).expect("inserting the key");
/// let mut unigram = Vec::new();
/// unigram.extend_from_slice(&hash_word("\u{4f60}").to_le_bytes());
/// unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
/// unigram.extend_from_slice(&0u16.to_le_bytes());
///
/// let mut container = writer::DictWriter::new();
/// for (kind, payload) in [
///     (SectionKind::Fst, index.into_inner().expect("finishing the index")),
///     (
///         SectionKind::Entries,
///         DictEntry::new(0, 3, 1, 0, 900)
///             .with_characters(1)
///             .encode()
///             .to_vec(),
///     ),
///     (SectionKind::StrPool, "\u{4f60}".as_bytes().to_vec()),
///     (SectionKind::Unigram, unigram),
///     (SectionKind::WordList, 0u32.to_le_bytes().to_vec()),
/// ] {
///     container.add_section(kind, payload).expect("adding a section");
/// }
/// let image = container.encode().expect("encoding the container");
///
/// let dir = std::env::temp_dir().join(format!("rspinyin-dict-lm-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&dir).expect("creating the scratch directory");
/// let path = dir.join("base.dict");
/// std::fs::write(&path, image).expect("writing the container");
/// let lexicon = FstLexicon::load(&path).expect("loading the container");
///
/// let model = DictLm::new(&lexicon);
/// // The table's most frequent word scores exactly zero, which is `log2(1.0)`.
/// assert_eq!(model.unigram("\u{4f60}"), 0);
/// // A word the table does not hold answers the floor the in-memory model answers.
/// assert_eq!(model.unigram("\u{597d}"), ime_core::lm::UNIGRAM_MISS);
///
/// std::fs::remove_dir_all(&dir).expect("removing the scratch directory");
/// ```
pub struct DictLm {
    /// The `UNIGRAM` section: `(hash, prob_q12)` records in ascending hash order.
    unigram: &'static [u8],
    /// The `BIGRAM` section, or `None` for a container that carries no pairs.
    bigram: Option<BigramTable>,
    /// Number of records the search may read.
    count: u32,
    /// Q8.8 log-probability of each Q12 probability a record can carry.
    scale: [i32; SCALE_LEN],
}

impl DictLm {
    /// Builds the model over a lexicon's mapping.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(lexicon: &FstLexicon) -> Self {
        let mut scale = [UNIGRAM_MISS; SCALE_LEN];
        for (q12, slot) in scale.iter_mut().enumerate() {
            // A record's probability is `q12 / PROB_Q12_MAX`; the score the decoder ranks
            // with is its base-2 logarithm in Q8.8. Zero has no logarithm and answers the
            // floor, which is the value a record that scores nothing should produce.
            *slot = log2_q8(
                u32::try_from(q12).unwrap_or(u32::MAX),
                u32::from(PROB_Q12_MAX),
            );
        }
        let unigram = lexicon.unigram_section();
        // A container the loader accepted holds exactly `entry_count` records, so this
        // cannot narrow anything in practice; the clamp is here because the binary search
        // trusts the count, and a count past the section would walk past its bytes.
        let records = unigram.len() / UNIGRAM_ENTRY_SIZE;
        let count = usize::try_from(lexicon.entry_count())
            .unwrap_or(usize::MAX)
            .min(records);
        Self {
            unigram,
            bigram: BigramTable::new(lexicon.bigram_section()),
            count: u32::try_from(count).unwrap_or(u32::MAX),
            scale,
        }
    }

    /// Returns up to `limit` Bigram successors of `prev` with their Q8.8 scores.
    ///
    /// The result is empty for a limit of zero, an absent `BIGRAM` section, an absent
    /// predecessor segment or a predecessor whose segment ends. Scores are deterministic
    /// and descending; equal scores are ordered by ascending successor id. The method
    /// reads only the predecessor's interval after one lower-bound search, never the
    /// whole table.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn successors(&self, prev: WordId, limit: u8) -> Vec<(WordId, i32)> {
        match self.bigram.as_ref() {
            Some(table) => table.successors(prev, usize::from(limit), &self.scale),
            None => Vec::new(),
        }
    }

    /// Returns the record at `index`, or `None` when the index leaves the section.
    ///
    /// The section is untrusted input, so every read is bounds-checked and the offset is
    /// formed with checked arithmetic: a record whose index would overflow the offset is
    /// reported as a miss rather than read.
    fn record(&self, index: u32) -> Option<(u32, u16)> {
        let at = usize::try_from(index)
            .ok()?
            .checked_mul(UNIGRAM_ENTRY_SIZE)?;
        let hash = read_u32(self.unigram, at, "unigram_hash").ok()?;
        let prob = read_u16(self.unigram, at.checked_add(4)?, "unigram_prob").ok()?;
        Some((hash, prob))
    }
}

impl LanguageModel for DictLm {
    /// Returns the word's unigram score in Q8.8, or [`UNIGRAM_MISS`] for a word the
    /// table does not hold.
    ///
    /// The section is sorted by hash, so the search is a plain integer binary search
    /// over fixed-width records: about `log2(count)` comparisons, none of which decodes
    /// or compares the word's text.
    fn unigram(&self, word: &str) -> i32 {
        let target = hash_word(word);
        let (mut low, mut high) = (0u32, self.count);
        while low < high {
            let middle = low + (high - low) / 2;
            match self.record(middle) {
                Some((hash, prob)) if hash == target => {
                    return self
                        .scale
                        .get(usize::from(prob))
                        .copied()
                        .unwrap_or(UNIGRAM_MISS);
                }
                Some((hash, _)) if hash < target => low = middle.saturating_add(1),
                Some(_) => high = middle,
                None => return UNIGRAM_MISS,
            }
        }
        UNIGRAM_MISS
    }

    /// Returns the recorded conditional score, or the unigram miss fallback.
    ///
    /// The fallback is used both for an absent section and for a pair the section does
    /// not hold. It deliberately equals what the in-memory model answers for the same
    /// miss: the two models are interchangeable, and swapping them must not change the
    /// candidate order.
    fn bigram(&self, prev: &str, word: &str) -> i32 {
        match self
            .bigram
            .as_ref()
            .and_then(|table| table.prob_q8(prev, word, &self.scale))
        {
            Some(score) => score,
            None => self.unigram(word).saturating_add(BIGRAM_MISS_PENALTY),
        }
    }
}

impl core::fmt::Debug for DictLm {
    /// Reports how many records the table holds rather than the scores themselves: the
    /// table is 16 KiB of derived values, and the count is the part a log line can use.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DictLm")
            .field("entries", &self.count)
            .field(
                "bigram_entries",
                &self.bigram.as_ref().map_or(0, BigramTable::count),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests;
