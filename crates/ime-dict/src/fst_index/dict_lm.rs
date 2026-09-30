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
//! Version 1 of the container has no bigram section (`SectionKind::Bigram` is reserved
//! and empty), so every pair is a miss. The miss path answers what the in-memory model
//! answers for a pair it was never given -- the word's unigram minus
//! [`BIGRAM_MISS_PENALTY`] -- rather than the bare penalty, because the two models have
//! to rank identically for a dictionary to be swappable for the model a test drives the
//! decoder with. When the Phase 3 section lands, this type gains a second view over it
//! and nothing above changes.
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

use ime_core::lm::{BIGRAM_MISS_PENALTY, UNIGRAM_MISS, log2_q8};
use ime_types::LanguageModel;

use crate::format::{PROB_Q12_MAX, UNIGRAM_ENTRY_SIZE, hash_word, read_u16, read_u32};

use super::FstLexicon;

/// Entries in the score table: one per Q12 probability a record can carry, plus zero.
const SCALE_LEN: usize = PROB_Q12_MAX as usize + 1;

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
            count: u32::try_from(count).unwrap_or(u32::MAX),
            scale,
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

    /// Returns the word's unigram minus [`BIGRAM_MISS_PENALTY`], which is what the miss
    /// path answers.
    ///
    /// The container carries no bigram section in version 1, so every pair is a miss and
    /// the predecessor is never read. The answer is the documented miss fallback rather
    /// than the bare penalty because it has to equal what the in-memory model answers
    /// for the same pair: the two models are interchangeable, and a caller that swaps
    /// one for the other must get the same candidate order.
    fn bigram(&self, _prev: &str, word: &str) -> i32 {
        self.unigram(word).saturating_add(BIGRAM_MISS_PENALTY)
    }
}

impl core::fmt::Debug for DictLm {
    /// Reports how many records the table holds rather than the scores themselves: the
    /// table is 16 KiB of derived values, and the count is the part a log line can use.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DictLm")
            .field("entries", &self.count)
            .finish()
    }
}
