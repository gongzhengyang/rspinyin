//! The word list the tuner ranks against, and the model built from its weights.
//!
//! Responsibility: hold the words in memory under the keys the compiler would compose for
//! them, serve them through the frozen [`Lexicon`] contract, and turn their weights into
//! the unigram model the scorer reads.
//!
//! Boundaries: this layer never reads a file's text (that is [`super::io`]) and never
//! ranks anything (that is [`super::eval`]). It stands in for the compiled dictionary
//! before the dictionary and its FST land: it is a build-time tool's approximation, it
//! never ships, and it holds every word of the word list in memory.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use ime_core::lm::{InMemoryLm, log2_q8};
use ime_core::segment::syllable_at;
use ime_types::{ImeError, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef};

use super::io::{baseline_key, data_lines, read_text};

struct LexWord {
    text: String,
    syl_count: u8,
    weight: u32,
}

/// Borrows one lexicon word as the contract's zero-copy handle.
fn word_ref(word: &LexWord) -> WordRef<'_> {
    WordRef {
        text: word.text.as_str(),
        weight: word.weight,
        syl_count: word.syl_count,
        flags: WordFlags::empty(),
    }
}

/// A word list held in memory and looked up by key, so the tuner can rank
/// candidates before the compiled dictionary and its FST land. It never ships.
pub(super) struct TsvLexicon {
    by_key: BTreeMap<String, Vec<LexWord>>,
}

impl TsvLexicon {
    /// Number of distinct keys the word list covers.
    pub(super) fn key_count(&self) -> usize {
        self.by_key.len()
    }

    /// Adds one word under `key`.
    fn insert(&mut self, key: String, text: &str, weight: u32) {
        let syl_count = u8::try_from(key.split('\'').count()).unwrap_or(u8::MAX);
        let word = LexWord {
            text: text.to_owned(),
            syl_count,
            weight,
        };
        self.by_key.entry(key).or_default().push(word);
    }

    /// Borrows every word stored under `key`.
    fn words_under(&self, key: &str) -> Vec<WordRef<'_>> {
        match self.by_key.get(key) {
            Some(words) => words.iter().map(word_ref).collect(),
            None => Vec::new(),
        }
    }
}

impl Lexicon for TsvLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(self.words_under(key)))
    }

    /// Prefix enumeration is a Phase 2 decode feature and no tuner path needs it,
    /// so this answers the way the contract says an implementation without it does.
    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let mut words = syllable_at(syl).map_or_else(Vec::new, |text| self.words_under(text));
        words.truncate(limit);
        Ok(WordIter::from_vec(words))
    }
}

/// Builds the in-memory lexicon. Keys are composed the way the compiler composes
/// them, so a word the tuner ranks is one the compiled dictionary can serve.
///
/// # Errors
///
/// Returns an error when the word list cannot be read.
pub(super) fn build_lexicon(words: &Path, l1: &BTreeMap<char, String>) -> Result<TsvLexicon> {
    let text = read_text(words)?;
    let mut lexicon = TsvLexicon {
        by_key: BTreeMap::new(),
    };
    let mut skipped = 0u64;
    for (_, line) in data_lines(&text) {
        let mut columns = line.split('\t').map(str::trim);
        let word = columns.next().unwrap_or_default();
        let Ok(weight) = columns.next().unwrap_or_default().parse::<u32>() else {
            skipped += 1;
            continue;
        };
        match baseline_key(word, l1) {
            Some(key) => lexicon.insert(key, word, weight),
            None => skipped += 1,
        }
    }
    for bucket in lexicon.by_key.values_mut() {
        // Heaviest first, then by text: equal weights need a fixed order or the
        // tuner stops being reproducible.
        bucket.sort_by(|left, right| {
            right
                .weight
                .cmp(&left.weight)
                .then_with(|| left.text.cmp(&right.text))
        });
    }
    println!(
        "tune: {} keys from {}, {skipped} rows unusable",
        lexicon.key_count(),
        words.display()
    );
    Ok(lexicon)
}

/// Builds a unigram language model from the lexicon's weights.
///
/// The word list carries counts, not a corpus, so the model holds no bigram counts
/// at all: every `bigram` call takes the miss path and answers with the word's own
/// unigram minus a constant, whatever the predecessor was -- which is why the
/// ranking walk can score an edge without knowing what came before it.
pub(super) fn build_lm(lexicon: &TsvLexicon) -> InMemoryLm {
    let total: u64 = lexicon
        .by_key
        .values()
        .flatten()
        .map(|word| u64::from(word.weight))
        .sum();
    let mut lm = InMemoryLm::new();
    let Ok(denominator) = u32::try_from(total) else {
        return lm;
    };
    if denominator == 0 {
        return lm;
    }
    for word in lexicon.by_key.values().flatten() {
        lm.insert_unigram(&word.text, log2_q8(word.weight, denominator));
    }
    lm
}

/// The user-frequency source the tuner ranks with: it has never recorded a
/// commit, so every lookup answers zero and the user term drops out of the score.
pub(super) struct NoUserFreq;

impl UserFreqSource for NoUserFreq {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}
