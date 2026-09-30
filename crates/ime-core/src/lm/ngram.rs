//! The in-memory language model: the double the engine is tested against, and
//! the shape the dictionary-backed model has to match.
//!
//! Responsibility: hold a table of quantized log probabilities and answer the
//! two questions the frozen `LanguageModel` trait asks. It exists because the
//! decoder has to be testable with no dictionary, no file and no clock: a test
//! builds one of these from a handful of literals and gets a model whose scores
//! it can predict by hand, and the offline weight tuner builds one from a word
//! list before the compiled dictionary exists.
//!
//! Boundaries: the production model is `ime_dict::fst_index::DictLm`, which reads
//! the compiled dictionary's `UNIGRAM` section straight out of its mapping and
//! allocates nothing. This one never touches a file and holds only what it was
//! given, and no production assembly path constructs it: it is the double the
//! engine is tested against and the model the offline tuner scores with. It
//! implements the same frozen trait, so every test written against it keeps its
//! meaning when the real model is assembled in its place -- and the two answer
//! identically for the same data, which is the property the dictionary model's
//! own tests assert.
//!
//! # The miss path
//!
//! A bigram the model has never seen falls back to the word's unigram minus a
//! fixed constant. The constant is deliberately not derived from anything at run
//! time: a fallback that varied with the data would make the candidate order
//! depend on how the counts happened to be loaded, and reproducibility is part
//! of the decode contract.

use std::collections::BTreeMap;

use ime_types::LanguageModel;

use crate::lm::score::LOG2_FLOOR;

/// Score of a word the model holds no count for, in Q8.8.
///
/// It is the floor of the quantized logarithm, that is `log2` of `2^-8`: a word
/// nobody has counted is ranked last without being removed from the lattice, so
/// a word the dictionary knows still produces a candidate.
pub const UNIGRAM_MISS: i32 = LOG2_FLOOR;

/// Penalty added to the unigram when a bigram is missing, in Q8.8.
///
/// `-64` is a quarter of a `log2` unit. That is enough to break a tie between
/// two words whose unigrams agree without overruling a clear unigram preference,
/// and it is a fixed constant rather than a value derived from the counts,
/// because a miss has to score the same on every run.
pub const BIGRAM_MISS_PENALTY: i32 = -64;

/// A language model backed by two in-memory tables.
///
/// # Concurrency
///
/// `Send + Sync` and reentrant: the tables are plain `BTreeMap`s read through
/// `&self`, and neither lookup mutates anything, allocates or blocks. Both run
/// on the decode hot path, where they cost one tree lookup and no more.
///
/// # Examples
///
/// ```
/// use ime_core::lm::{BIGRAM_MISS_PENALTY, InMemoryLm};
/// use ime_types::LanguageModel;
///
/// let mut lm = InMemoryLm::new();
/// lm.insert_unigram("hao", -256);
/// lm.insert_bigram("ni", "hao", -64);
///
/// assert_eq!(lm.unigram("hao"), -256);
/// assert_eq!(lm.bigram("ni", "hao"), -64);
/// // A pair nobody recorded falls back to the unigram plus the miss penalty.
/// assert_eq!(lm.bigram("wo", "hao"), -256 + BIGRAM_MISS_PENALTY);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InMemoryLm {
    /// Word to quantized log probability.
    unigrams: BTreeMap<String, i32>,
    /// Previous word to word to quantized conditional log probability.
    bigrams: BTreeMap<String, BTreeMap<String, i32>>,
}

impl InMemoryLm {
    /// Builds an empty model, where every lookup takes the miss path.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the unigram score of `word`, replacing an earlier one.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn insert_unigram(&mut self, word: &str, score: i32) {
        self.unigrams.insert(word.to_owned(), score);
    }

    /// Records the conditional score of `word` after `prev`, replacing an
    /// earlier one.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn insert_bigram(&mut self, prev: &str, word: &str, score: i32) {
        self.bigrams
            .entry(prev.to_owned())
            .or_default()
            .insert(word.to_owned(), score);
    }

    /// Returns how many words carry a unigram score.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn unigram_count(&self) -> usize {
        self.unigrams.len()
    }

    /// Returns how many bigram pairs carry a score.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn bigram_count(&self) -> usize {
        self.bigrams.values().map(BTreeMap::len).sum()
    }
}

impl LanguageModel for InMemoryLm {
    fn unigram(&self, word: &str) -> i32 {
        self.unigrams.get(word).copied().unwrap_or(UNIGRAM_MISS)
    }

    fn bigram(&self, prev: &str, word: &str) -> i32 {
        match self.bigrams.get(prev).and_then(|row| row.get(word)) {
            Some(score) => *score,
            None => self.unigram(word).saturating_add(BIGRAM_MISS_PENALTY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model holding one word with a bigram from another.
    fn populated() -> InMemoryLm {
        let mut lm = InMemoryLm::new();
        lm.insert_unigram("hao", -256);
        lm.insert_unigram("ni", -512);
        lm.insert_bigram("ni", "hao", -64);
        lm
    }

    #[test]
    fn test_new_model_is_empty_and_every_lookup_misses() {
        let lm = InMemoryLm::new();
        assert_eq!(lm.unigram_count(), 0);
        assert_eq!(lm.bigram_count(), 0);
        assert_eq!(lm.unigram("hao"), UNIGRAM_MISS);
        assert_eq!(lm.bigram("ni", "hao"), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
    }

    #[test]
    fn test_unigram_returns_the_stored_score_and_the_miss_floor() {
        let lm = populated();
        assert_eq!(lm.unigram("hao"), -256);
        assert_eq!(lm.unigram("ni"), -512);
        assert_eq!(lm.unigram("zhong"), UNIGRAM_MISS);
        assert_eq!(lm.unigram(""), UNIGRAM_MISS);
    }

    #[test]
    fn test_bigram_returns_the_stored_score_when_the_pair_is_known() {
        let lm = populated();
        assert_eq!(lm.bigram("ni", "hao"), -64);
        // The stored value is returned verbatim, not offset by anything.
        assert_ne!(lm.bigram("ni", "hao"), -256 + BIGRAM_MISS_PENALTY);
    }

    #[test]
    fn test_bigram_falls_back_to_the_unigram_plus_the_fixed_penalty() {
        let lm = populated();
        // Known predecessor, unknown successor.
        assert_eq!(lm.bigram("ni", "bu"), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
        // Known successor, unknown predecessor.
        assert_eq!(lm.bigram("wo", "hao"), -256 + BIGRAM_MISS_PENALTY);
        // Both sides unknown.
        assert_eq!(lm.bigram("wo", "bu"), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
        assert_eq!(BIGRAM_MISS_PENALTY, -64);
    }

    #[test]
    fn test_bigram_miss_penalty_stays_the_same_across_one_hundred_repeats() {
        // The miss path is a constant rather than something derived from the counts that
        // happened to be loaded, so the same pair scores the same on every run and in
        // every process. Every shape of pair is asked for a hundred times and compared
        // with the first answer: a hit, an unknown successor, an unknown predecessor,
        // both unknown, and the empty strings on either side.
        let lm = populated();
        let pairs = [
            ("ni", "hao"),
            ("ni", "bu"),
            ("wo", "hao"),
            ("wo", "bu"),
            ("", "hao"),
            ("ni", ""),
            ("", ""),
        ];
        let expected: Vec<i32> = pairs
            .iter()
            .map(|&(prev, word)| lm.bigram(prev, word))
            .collect();
        for run in 0..100 {
            for (pair, wanted) in pairs.iter().zip(&expected) {
                assert_eq!(
                    lm.bigram(pair.0, pair.1),
                    *wanted,
                    "the score of {pair:?} on run {run}"
                );
            }
        }
        // And the constant is exactly what separates a miss from the unigram it falls
        // back to, on every pair the model does not hold.
        assert_eq!(BIGRAM_MISS_PENALTY, -64);
        let misses = [("ni", "bu"), ("wo", "hao"), ("wo", "bu"), ("", "hao")];
        for (prev, word) in misses {
            assert_eq!(
                lm.bigram(prev, word),
                lm.unigram(word) + BIGRAM_MISS_PENALTY,
                "the miss of {word:?} after {prev:?}"
            );
        }
    }

    #[test]
    fn test_bigram_with_an_empty_predecessor_takes_the_miss_path() {
        // The boundary the fallback has to hold at: an empty predecessor is not a case of
        // its own, it is a pair nobody recorded, and it degrades like any other.
        let lm = populated();
        assert_eq!(lm.bigram("", "hao"), -256 + BIGRAM_MISS_PENALTY);
        assert_eq!(lm.bigram("", "bu"), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
        assert_eq!(lm.bigram("", ""), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
        // A model that recorded the empty predecessor answers with what it recorded,
        // which is what makes the branch above a fallback rather than a rule.
        let mut recorded = populated();
        recorded.insert_bigram("", "hao", -8);
        assert_eq!(recorded.bigram("", "hao"), -8);
        assert_eq!(
            recorded.bigram("", "bu"),
            UNIGRAM_MISS + BIGRAM_MISS_PENALTY
        );
    }

    #[test]
    fn test_insert_replaces_an_earlier_score_and_keeps_the_counts() {
        let mut lm = populated();
        lm.insert_unigram("hao", -128);
        assert_eq!(lm.unigram("hao"), -128);
        assert_eq!(lm.unigram_count(), 2, "a replacement is not a new word");
        lm.insert_bigram("ni", "hao", -32);
        assert_eq!(lm.bigram("ni", "hao"), -32);
        assert_eq!(lm.bigram_count(), 1, "a replacement is not a new pair");
        lm.insert_bigram("ni", "bu", -16);
        assert_eq!(lm.bigram_count(), 2);
    }

    #[test]
    fn test_scores_may_be_negative_at_the_extremes_without_overflow() {
        let mut lm = InMemoryLm::new();
        lm.insert_unigram("rare", i32::MIN);
        assert_eq!(lm.unigram("rare"), i32::MIN);
        // The fallback saturates rather than wrapping past the floor.
        assert_eq!(lm.bigram("x", "rare"), i32::MIN);
        lm.insert_unigram("loud", i32::MAX);
        assert_eq!(lm.bigram("x", "loud"), i32::MAX - 64);
    }
}
