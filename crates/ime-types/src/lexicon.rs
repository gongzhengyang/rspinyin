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

    /// Drops `key` from the user's learned frequencies, reporting whether anything
    /// was there to drop.
    ///
    /// The three methods below were appended by ADR-0005 and every one of them has a
    /// default implementation. That is not politeness: a trait method without a
    /// default breaks every implementor, including the in-memory mocks `ime-core`
    /// drives its determinism tests through, so the extension would have been far
    /// more invasive than the struct field it sits beside.
    ///
    /// The default reports "nothing was forgotten", which is the truthful answer for
    /// a source that keeps no removable record.
    fn forget(&self, key: &str) -> bool {
        let _ = key;
        false
    }

    /// Lists the user's learned words, `limit` of them starting at `offset`.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] by default. The card proposed
    /// `ImeError::DictUnavailable` here, but that variant carries a `PathBuf` and a
    /// `DictError`, and a source that simply has no enumeration to offer has
    /// neither -- inventing a path would put a filesystem location into a
    /// user-visible diagnostic that names no real file. `dict/unsupported` is the
    /// code the contract already reserves for "this build phase does not implement
    /// that capability", which is exactly the situation.
    fn list(&self, offset: u64, limit: u16) -> Result<Vec<WordRef<'_>>, ImeError> {
        let _ = (offset, limit);
        Err(ImeError::Unsupported)
    }

    /// Writes the user's learned words as TSV, returning how many rows were written.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] by default, for the same reason as [`Self::list`];
    /// a real implementation also propagates whatever `writer` raises.
    fn export_tsv(&self, writer: &mut dyn std::io::Write) -> Result<u64, ImeError> {
        let _ = writer;
        Err(ImeError::Unsupported)
    }
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

/// Words one [`WordIter`] holds without touching the allocator.
///
/// Eight matches the head of a key's word list -- the part a lookup is asked for, and
/// the part the lattice reads -- and it is a compile-time constant of the contract
/// rather than a dependency: this crate may only depend on `thiserror`, `bitflags` and
/// `serde`, so the inline storage is a plain array.
pub const WORD_ITER_INLINE: usize = 8;

/// The word a slot holds before anything is written into it.
///
/// [`WordRef`] has no `Default` -- an empty word is not a dictionary entry -- but the
/// inline storage needs a value to start from. Only the slots below the live length are
/// ever read, so the placeholder is never handed out.
const EMPTY_WORD: WordRef<'static> = WordRef {
    text: "",
    weight: 0,
    syl_count: 0,
    flags: WordFlags::empty(),
};

/// Iterator over the words of one lookup.
///
/// The backing store is deliberately unspecified by the contract. A lookup whose word
/// list fits [`WORD_ITER_INLINE`] -- which is every lookup the decode path makes in the
/// common case -- holds its words inline and allocates nothing; a longer list is moved
/// into a vector, so it is served whole rather than cut short. Either way every
/// [`WordRef`] borrows the dictionary's own mapping and nothing is copied out of it.
#[derive(Debug)]
pub struct WordIter<'a> {
    inner: Inner<'a>,
}

/// Where a [`WordIter`] keeps its words.
#[derive(Debug)]
enum Inner<'a> {
    /// The words, how many of the slots are live, and how many have been read.
    Inline {
        words: [WordRef<'a>; WORD_ITER_INLINE],
        len: u8,
        next: u8,
    },
    /// A list longer than the inline capacity.
    Heap(std::vec::IntoIter<WordRef<'a>>),
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
            inner: Inner::Heap(words.into_iter()),
        }
    }

    /// Builds an iterator that holds its words inline, copying them into its own slots.
    ///
    /// This is the constructor a lookup whose list fits [`WORD_ITER_INLINE`] uses: the
    /// words are copied out of the reader's own buffer, which leaves that buffer free for
    /// the next lookup and keeps the iterator free of the allocator.
    ///
    /// A longer list is truncated to [`WORD_ITER_INLINE`]; a caller that has to serve a
    /// whole long list uses [`WordIter::from_vec`] instead. Truncating here rather than
    /// growing is what keeps this constructor infallible, and the two constructors
    /// together are what let a caller decide per lookup.
    ///
    /// # Panics
    ///
    /// Never: a list longer than [`WORD_ITER_INLINE`] is cut to what fits.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_types::lexicon::{WordFlags, WORD_ITER_INLINE, WordIter, WordRef};
    ///
    /// let words = [WordRef {
    ///     text: "ni",
    ///     weight: 5,
    ///     syl_count: 1,
    ///     flags: WordFlags::empty(),
    /// }];
    /// let mut iter = WordIter::from_slice(&words);
    /// assert_eq!(iter.len(), 1);
    /// assert_eq!(iter.next().map(|word| word.text), Some("ni"));
    /// assert_eq!(iter.next(), None);
    /// // The inline capacity is the ceiling this constructor serves.
    /// let many = vec![words[0]; WORD_ITER_INLINE + 4];
    /// assert_eq!(WordIter::from_slice(&many).count(), WORD_ITER_INLINE);
    /// ```
    pub fn from_slice(words: &[WordRef<'a>]) -> Self {
        let mut inline: [WordRef<'a>; WORD_ITER_INLINE] = [EMPTY_WORD; WORD_ITER_INLINE];
        let taken = words.len().min(WORD_ITER_INLINE);
        // `taken` is at most the array's own length, so both slices exist; the lookup is
        // taken rather than indexed so that this constructor has no panic path at all.
        if let (Some(slots), Some(source)) = (inline.get_mut(..taken), words.get(..taken)) {
            slots.copy_from_slice(source);
        }
        Self {
            inner: Inner::Inline {
                words: inline,
                len: u8::try_from(taken).unwrap_or(u8::MAX),
                next: 0,
            },
        }
    }

    /// Returns how many words have not been yielded yet.
    fn remaining(&self) -> usize {
        match &self.inner {
            Inner::Inline { len, next, .. } => usize::from(len.saturating_sub(*next)),
            Inner::Heap(words) => words.len(),
        }
    }
}

impl<'a> Iterator for WordIter<'a> {
    type Item = WordRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            Inner::Inline { words, len, next } => {
                if *next >= *len {
                    return None;
                }
                let word = words.get(usize::from(*next)).copied();
                *next = next.saturating_add(1);
                word
            }
            Inner::Heap(words) => words.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Both stores know their exact length, so the hint is exact for either.
        let remaining = self.remaining();
        (remaining, Some(remaining))
    }
}

impl<'a> ExactSizeIterator for WordIter<'a> {
    fn len(&self) -> usize {
        self.remaining()
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
    fn test_word_iter_from_slice_holds_its_words_inline() {
        let words = [word("ni", 5, 1), word("hao", 7, 1)];
        let iter = WordIter::from_slice(&words);
        assert!(
            matches!(
                iter.inner,
                Inner::Inline {
                    len: 2,
                    next: 0,
                    ..
                }
            ),
            "a short list is held in the inline slots"
        );
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        let collected: Vec<_> = iter.collect();
        assert_eq!(collected[0].text, "ni");
        assert_eq!(collected[0].weight, 5);
        assert_eq!(collected[1].text, "hao");
    }

    #[test]
    fn test_word_iter_from_slice_truncates_a_list_past_the_inline_capacity() {
        let words: Vec<WordRef<'_>> = (0..WORD_ITER_INLINE + 4)
            .map(|index| word("shi", u32::try_from(index).unwrap_or(0), 1))
            .collect();
        let iter = WordIter::from_slice(&words);
        assert_eq!(iter.len(), WORD_ITER_INLINE);
        assert_eq!(iter.size_hint(), (WORD_ITER_INLINE, Some(WORD_ITER_INLINE)));
        let collected: Vec<_> = iter.collect();
        assert_eq!(collected.len(), WORD_ITER_INLINE);
        // The head of the list is what survives: the words are copied in order.
        for (index, kept) in collected.iter().enumerate() {
            assert_eq!(kept.weight, u32::try_from(index).unwrap_or(0));
        }
    }

    #[test]
    fn test_word_iter_from_slice_of_an_empty_list_yields_nothing() {
        let iter = WordIter::from_slice(&[]);
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.count(), 0);
    }

    #[test]
    fn test_word_iter_inline_reports_the_length_left_as_it_advances() {
        let words = [word("a", 1, 1), word("b", 2, 1), word("c", 3, 1)];
        let mut iter = WordIter::from_slice(&words);
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.next().map(|kept| kept.text), Some("a"));
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.next().map(|kept| kept.text), Some("b"));
        assert_eq!(iter.next().map(|kept| kept.text), Some("c"));
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn test_word_iter_inline_stays_exhausted_at_the_end_of_its_slots() {
        let words = [word("a", 1, 1)];
        let mut iter = WordIter::from_slice(&words);
        assert_eq!(iter.next().map(|kept| kept.text), Some("a"));
        // A fused iterator: once it answered `None` it keeps answering `None`, so a
        // caller cannot be handed a word out of the slots past the live length.
        for _ in 0..3 {
            assert_eq!(iter.next(), None);
        }
        assert_eq!(iter.len(), 0);
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
