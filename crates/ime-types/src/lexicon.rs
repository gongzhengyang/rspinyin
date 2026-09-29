//! The three engine-injected data sources and the zero-copy word handle.
//!
//! The decoder never opens a file: it holds `&dyn Lexicon`, `&dyn UserFreqSource`
//! and `&dyn LanguageModel` and nothing else. That is what makes the decode
//! pipeline testable against in-memory doubles, and what lets the dictionary, the
//! user database and the language model change without touching the engine.
//!
//! [`WordRef`] borrows straight out of the dictionary's read-only mapping; nothing
//! on the lookup path copies a word into an owned `String`.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use crate::decode::SyllableId;
use crate::error::ImeError;

/// Dictionary lookup, injected into the decoder as `&dyn Lexicon`.
///
/// # Concurrency
///
/// Implementations must be `Send + Sync`: the handle is shared between the host
/// thread, the UI thread and the background dictionary-reload path, even though
/// decoding itself is serial per session. The methods run on the hot decode path
/// and must not block, must not take a lock a writer can hold, and must not
/// allocate per candidate.
pub trait Lexicon: Send + Sync {
    /// Exact lookup: `key` is a `'`-separated syllable string such as `ni'hao`,
    /// and the returned words are ordered by descending weight.
    ///
    /// The returned `WordRef` borrows the dictionary's memory, so its lifetime is
    /// the lifetime of the dictionary handle.
    ///
    /// # Errors
    ///
    /// Returns an `ImeError` when the dictionary cannot serve the query, for
    /// example because it is in a degraded state or because an entry failed its
    /// bounds check.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError>;

    /// Prefix enumeration, used by the Phase 2 abbreviation expansion.
    ///
    /// Phase 1 returns `Err(ImeError::Unsupported)`. The signature is frozen now
    /// so that callers do not have to change when the implementation lands.
    ///
    /// # Errors
    ///
    /// Returns `ImeError::Unsupported` until prefix enumeration is implemented,
    /// and an `ImeError` when the dictionary cannot serve the query.
    fn prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError>;

    /// Single-character fallback for one syllable.
    ///
    /// Every syllable must yield at least one candidate, so that any input with a
    /// legal segmentation still produces something the user can commit.
    ///
    /// # Errors
    ///
    /// Returns an `ImeError` when the dictionary cannot serve the query.
    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError>;
}

/// User frequency source: the learned half of the ranking signal.
///
/// # Concurrency
///
/// Implementations must be `Send + Sync`. `freq` runs on the decode hot path and
/// must be O(1) or O(log n) -- never a linear scan. `record` runs on the host
/// thread right after a commit and must not block: batching and disk writes are
/// the implementation's own business.
pub trait UserFreqSource: Send + Sync {
    /// Returns the user frequency of `key`; 0 means "never recorded".
    fn freq(&self, key: &str) -> u32;

    /// Records one commit. Implementations batch and flush asynchronously; this
    /// method must return within 5us.
    fn record(&self, key: &str, weight_hint: u16);

    /// Whether `key` is a word the user coined. Phase 1 is read-only; Phase 2
    /// adds the write path.
    fn is_user_word(&self, key: &str) -> bool;
}

/// Language model scoring.
///
/// # Concurrency
///
/// Implementations must be `Send + Sync`. Both methods are pure lookups over
/// read-only data and run on the decode hot path.
pub trait LanguageModel: Send + Sync {
    /// Returns the quantized log probability in Q8.8 fixed point, so that the
    /// candidate order stays reproducible instead of drifting with floating-point
    /// rounding.
    fn unigram(&self, word: &str) -> i32;

    /// Returns the quantized conditional log probability of `word` after `prev`.
    fn bigram(&self, prev: &str, word: &str) -> i32;
}

/// A zero-copy word reference borrowed from the dictionary's mapping.
///
/// `text` points straight into the mapped string pool. The borrow lives as long
/// as the dictionary handle, which is why the `Lexicon` methods return a
/// `WordIter<'_>` tied to `&self`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WordRef<'a> {
    /// The word text, borrowed from the mapped string pool.
    pub text: &'a str,
    /// Ranking weight within one key: a Q16.16 log probability, higher first.
    pub weight: u32,
    /// Number of syllables the word consumes, 1..=16.
    pub syl_count: u8,
    /// Flags carried by the dictionary entry.
    pub flags: WordFlags,
}

/// Iterator over the words of one lookup.
///
/// The backing store is deliberately unspecified by the contract. Phase 1
/// materializes the result -- a lookup is bounded by one key's word list -- into
/// a `Vec` and iterates that, which keeps every `WordRef` borrowed from the
/// dictionary mapping while making the type cheap to hand back through a
/// `Result`. A later phase may replace what is inside this struct without
/// touching a single signature.
#[derive(Debug)]
pub struct WordIter<'a> {
    inner: std::vec::IntoIter<WordRef<'a>>,
}

impl<'a> WordIter<'a> {
    /// Builds an iterator over an already materialized, already ordered word list.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_types::lexicon::{WordFlags, WordIter, WordRef};
    ///
    /// let words = vec![WordRef {
    ///     text: "ni'hao",
    ///     weight: 900,
    ///     syl_count: 2,
    ///     flags: WordFlags::empty(),
    /// }];
    /// let collected: Vec<_> = WordIter::from_vec(words).collect();
    /// assert_eq!(collected.len(), 1);
    /// assert_eq!(collected[0].text, "ni'hao");
    /// ```
    pub fn from_vec(words: Vec<WordRef<'a>>) -> Self {
        Self {
            inner: words.into_iter(),
        }
    }
}

impl<'a> Iterator for WordIter<'a> {
    type Item = WordRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> ExactSizeIterator for WordIter<'a> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> core::iter::FusedIterator for WordIter<'a> {}

bitflags::bitflags! {
    /// Per-entry flags from the dictionary container.
    ///
    /// Unknown bits are ignored on the way in, so a dictionary compiled by a
    /// newer build stays readable.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct WordFlags: u8 {
        /// Surname: the entry is a family name.
        const SURNAME = 0b0001;
        /// Place name.
        const PLACE = 0b0010;
        /// Technical term.
        const TERM = 0b0100;
        /// User word, merged in from the user database.
        const USER = 0b1000;
    }
}

// Serialized as the raw bit set. Unknown bits are dropped on the way in, which
// keeps a snapshot written by a newer build readable here.
#[cfg(feature = "serde")]
impl serde::Serialize for WordFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(self.bits())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for WordFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bits = <u8 as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Ok(Self::from_bits_truncate(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, weight: u32, syl_count: u8) -> WordRef<'_> {
        WordRef {
            text,
            weight,
            syl_count,
            flags: WordFlags::empty(),
        }
    }

    #[test]
    fn test_word_iter_yields_words_in_order() {
        let words = vec![word("ni'hao", 900, 2), word("ni", 100, 1)];
        let collected: Vec<_> = WordIter::from_vec(words).collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].text, "ni'hao");
        assert_eq!(collected[0].weight, 900);
        assert_eq!(collected[0].syl_count, 2);
        assert_eq!(collected[1].text, "ni");
        assert_eq!(collected[1].weight, 100);
    }

    #[test]
    fn test_word_iter_empty_reports_zero_len_and_no_items() {
        let iter = WordIter::from_vec(Vec::new());
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.count(), 0);
    }

    #[test]
    fn test_word_iter_len_matches_yielded_items() {
        let words = vec![word("a", 1, 1), word("b", 2, 1), word("c", 3, 1)];
        let iter = WordIter::from_vec(words);
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.count(), 3);
    }

    #[test]
    fn test_word_flags_bit_operations() {
        let both = WordFlags::SURNAME | WordFlags::PLACE;
        assert!(both.contains(WordFlags::SURNAME));
        assert!(both.contains(WordFlags::PLACE));
        assert!(!both.contains(WordFlags::TERM));
        assert!(!both.contains(WordFlags::USER));

        let without_surname = both & !WordFlags::SURNAME;
        assert!(!without_surname.contains(WordFlags::SURNAME));
        assert!(without_surname.contains(WordFlags::PLACE));

        assert!(WordFlags::empty().is_empty());
        assert_eq!(WordFlags::USER.bits(), 0b1000);
        assert_eq!(WordFlags::from_bits_truncate(u8::MAX).bits(), 0b1111);
    }

    #[test]
    fn test_word_ref_is_copy_and_compares_by_value() {
        let first = word("ni", 5, 1);
        let second = first;
        assert_eq!(first, second);

        let tagged = WordRef {
            text: "ni",
            weight: 5,
            syl_count: 1,
            flags: WordFlags::TERM,
        };
        assert_ne!(first, tagged);
        assert!(tagged.flags.contains(WordFlags::TERM));
    }
}
