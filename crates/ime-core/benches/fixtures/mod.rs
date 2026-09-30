//! The dictionary the decode benchmarks run against.
//!
//! Responsibility: hand the benchmarks a dictionary large enough for a decode to
//! cost what it costs against the shipped one, without depending on a compiled
//! `base.dict`. The repository carries only a 5,441-entry development subset, which
//! is far too small for a latency budget: a decode against it finds almost nothing,
//! and the number it produces is the cost of falling back rather than the cost of
//! decoding.
//!
//! The corpus is generated rather than shipped, and every part of it is a pure
//! function of the constants in [`corpus`] and [`inputs`] -- no clock, no
//! filesystem, no environment -- so two runs measure the same work and a run on a
//! machine with no `$HOME` and no compiled dictionary measures exactly what a
//! developer's run measures.
//!
//! Boundaries: this module is benchmark scaffolding. It implements the frozen
//! [`Lexicon`] contract over owned strings rather than over a mapping, which is the
//! one thing a real dictionary does differently; the decode path above it cannot
//! tell the two apart.

mod corpus;
mod inputs;

pub use inputs::{CASES, holdout_keys};

use std::collections::BTreeMap;

use ime_core::segment::syllable_at;
use ime_types::{ImeError, Lexicon, SyllableId, WordFlags, WordIter, WordRef};

/// Words one lookup serves, which is the head of the key's list.
///
/// The lattice takes no more than this from a key, so serving the head is serving the
/// whole list the decode path reads, and it is what lets the handles live in an array
/// on the stack rather than in a `Vec`: the contract requires a lookup not to allocate
/// per candidate, and the real dictionary's lookup hands its words out of the mapping
/// without building a list at all. A lookup that materialized a `Vec` per call would
/// put its own allocation into every number this target reports.
const HEAD: usize = ime_types::lexicon::WORD_ITER_INLINE;

/// Builds the dictionary the decode cases decode against.
///
/// The corpus carries a word for every span of every benchmark input and for every
/// key of the held-out set, so a case measures decoding rather than falling back.
///
/// # Errors
/// Returns a description of the first benchmark input that cannot be cut into
/// syllables, which is a defect in the case table rather than a runtime condition.
pub fn build() -> Result<SyntheticDict, String> {
    let mut keys = inputs::seed_keys()?;
    keys.extend(holdout_keys());
    Ok(corpus::generate(&keys, corpus::CORPUS_WORDS))
}

/// A dictionary built from the generated corpus.
#[derive(Default)]
pub struct SyntheticDict {
    /// Words per key, strongest first, which is the order [`Lexicon::lookup`] promises.
    words: BTreeMap<String, Vec<Word>>,
    /// Entries across every key.
    entries: usize,
}

/// One entry of the generated corpus.
struct Word {
    /// The word text.
    text: String,
    /// Ranking weight within one key, in Q16.16 log probability; higher first.
    weight: u32,
    /// Syllables the word consumes.
    syl_count: u8,
}

impl SyntheticDict {
    /// Appends one reading of `key`.
    ///
    /// The weight falls off with the corpus's size, so appending to a key's list
    /// keeps it ordered by descending weight without a sort.
    fn push(&mut self, key: &str, syl_count: u8) {
        let reading = u32::try_from(self.words.get(key).map_or(0, Vec::len)).unwrap_or(u32::MAX);
        let word = Word {
            text: corpus::text_of(key, reading),
            weight: corpus::weight_at(self.entries),
            syl_count,
        };
        self.words.entry(key.to_owned()).or_default().push(word);
        self.entries += 1;
    }

    /// Entries across every key.
    fn entries(&self) -> usize {
        self.entries
    }

    /// Every word text of the corpus, in key order.
    ///
    /// The language model the benchmarks score with is built from this list, so the
    /// model answers for the words the dictionary offers and for nothing else.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.words.values().flatten().map(|word| word.text.as_str())
    }
}

impl Lexicon for SyntheticDict {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let entries = self.words.get(key).map_or(&[][..], Vec::as_slice);
        let mut handles = [EMPTY_ENTRY; HEAD];
        let taken = references(entries, &mut handles);
        Ok(WordIter::from_slice(&handles[..taken]))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        // The contract leaves prefix enumeration to a later phase; the decoder does
        // not call it, so the benchmark's dictionary does not implement it either.
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        // Every syllable of the table has a reading in the corpus, so the fallback
        // answers for all of them; the lattice reaches it only for a span no word
        // covers, which the generated corpus leaves almost nowhere. The `limit` the
        // caller asks for is smaller than the head served here, and the caller takes
        // its own share of what comes back.
        let entries = syllable_at(syl)
            .and_then(|spelling| self.words.get(spelling))
            .map_or(&[][..], Vec::as_slice);
        let mut handles = [EMPTY_ENTRY; HEAD];
        let taken = references(entries, &mut handles);
        Ok(WordIter::from_slice(&handles[..taken]))
    }
}

/// The entry a slot holds before anything is written into it.
///
/// Only the slots below the count [`references`] answered are handed to the iterator,
/// so the placeholder is never served as a word.
const EMPTY_ENTRY: WordRef<'static> = WordRef {
    text: "",
    weight: 0,
    syl_count: 0,
    flags: WordFlags::empty(),
};

/// Fills `handles` with the head of `entries` and answers how many it filled.
///
/// The handles borrow the entries rather than copying their text, which is the
/// zero-copy shape the contract promises and the real dictionary delivers.
fn references<'a>(entries: &'a [Word], handles: &mut [WordRef<'a>; HEAD]) -> usize {
    let mut filled = 0usize;
    for (slot, word) in handles.iter_mut().zip(entries) {
        *slot = reference(word);
        filled += 1;
    }
    filled
}

/// Borrows one entry as the contract's zero-copy word handle.
fn reference(word: &Word) -> WordRef<'_> {
    WordRef {
        text: &word.text,
        weight: word.weight,
        syl_count: word.syl_count,
        flags: WordFlags::empty(),
    }
}
