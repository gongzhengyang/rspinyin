//! The in-memory doubles the direct-drive harness injects into the engine.
//!
//! The decoder reaches every one of its data sources through the three frozen traits of
//! `ime-types`, so a scenario drives a whole decode with no dictionary file, no user
//! database and no display server. These doubles are what a scenario file describes:
//! the words a key holds, the single characters a syllable falls back to, the keys whose
//! lookup is refused, and the model and frequency tables the ranking reads.
//!
//! They are deterministic by construction. Nothing here opens a file, reads a clock or
//! consults the environment; the two `BTreeMap`s fix the iteration order, and
//! [`MockUserFreq::record`] deliberately does nothing, so replaying a scenario cannot
//! change the data the next replay reads.
//!
//! # Concurrency
//!
//! All three doubles satisfy the `Send + Sync` bound the frozen traits impose, and none
//! of them needs interior mutability: a refused lookup answers an `ImeError` and records
//! nothing, because the code it refused with is read from the scenario's own dictionary
//! spec rather than from the double.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ime_core::segment::syllable_at;
use ime_types::{
    DictError, ImeError, LanguageModel, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter,
    WordRef,
};

/// The path a mock dictionary failure names.
///
/// A placeholder, never a real path: the frozen error model carries a path for a real
/// dictionary, and a harness whose own output named a directory of the machine it ran
/// on would report something different on every machine.
pub const MOCK_DICT_PATH: &str = "<mock>/base.dict";

/// Weight the first word of a key carries.
///
/// The words of one key come back in the order the fixture lists them, carrying a
/// descending weight, which is the order the frozen contract asks a real lookup for.
const HEAD_WEIGHT: u32 = 1 << 30;

/// One word of a mock dictionary entry, owned rather than borrowed.
///
/// A scenario file supplies its words as data, so the double cannot borrow them from
/// `'static` literals the way the engine's own test doubles do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedWord {
    /// The text the word commits.
    pub text: String,
    /// Ranking weight within its key; higher first.
    pub weight: u32,
    /// Syllables the word consumes.
    pub syl_count: u8,
}

impl OwnedWord {
    /// Builds the word at `position` of a key that spans `syllables` syllables.
    ///
    /// # Returns
    ///
    /// The word, weighted one unit below the word listed before it.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn at(position: usize, syllables: u8, text: impl Into<String>) -> Self {
        let rank = u32::try_from(position).unwrap_or(u32::MAX);
        Self {
            text: text.into(),
            weight: HEAD_WEIGHT.saturating_sub(rank),
            syl_count: syllables,
        }
    }

    /// Borrows the word as the frozen [`WordRef`] a lookup answers with.
    ///
    /// # Returns
    ///
    /// A reference into the word's own text; the borrow lives as long as the word.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn borrow(&self) -> WordRef<'_> {
        WordRef {
            text: self.text.as_str(),
            weight: self.weight,
            syl_count: self.syl_count,
            flags: WordFlags::empty(),
        }
    }
}

/// Why a mock dictionary refuses a lookup, named by the frozen code it renders as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupFailure {
    /// The dictionary cannot serve the query at all.
    Unavailable,
    /// The container failed its own validation.
    Corrupt,
    /// The capability is deliberately absent in this build phase.
    Unsupported,
}

impl LookupFailure {
    /// Returns the stable `domain/action/reason` code the failure renders as.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "dict/unavailable",
            Self::Corrupt => "dict/corrupt",
            Self::Unsupported => "dict/unsupported",
        }
    }

    /// Builds the error a lookup of a failing key answers with.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn error(self) -> ImeError {
        let path = PathBuf::from(MOCK_DICT_PATH);
        match self {
            Self::Unavailable => ImeError::DictUnavailable {
                path,
                cause: DictError::MagicMismatch,
            },
            Self::Corrupt => ImeError::DictCorrupt { path },
            Self::Unsupported => ImeError::Unsupported,
        }
    }

    /// Reads the frozen code a scenario file names.
    ///
    /// # Errors
    ///
    /// A message naming the offending code when it is not one of the three the mock
    /// dictionary can answer with.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn parse(code: &str) -> Result<Self, String> {
        match code {
            "dict/unavailable" => Ok(Self::Unavailable),
            "dict/corrupt" => Ok(Self::Corrupt),
            "dict/unsupported" => Ok(Self::Unsupported),
            other => Err(format!(
                "unknown dictionary failure code {other:?}; expected one of \
                 dict/unavailable, dict/corrupt, dict/unsupported"
            )),
        }
    }
}

/// A dictionary with fixed contents, built from a scenario's dictionary spec.
///
/// Words come back in the order they were listed, which is the order the frozen contract
/// describes for a real lookup: strongest first. A key the dictionary does not hold
/// answers an empty list rather than an error, so a scenario author never has to
/// enumerate every key the segmentation will spell.
#[derive(Debug, Default)]
pub struct MockLexicon {
    /// Key to the words stored under it, strongest first.
    words: BTreeMap<String, Vec<OwnedWord>>,
    /// Spelled syllable to its single-character fallbacks.
    singles: BTreeMap<String, Vec<OwnedWord>>,
    /// Keys whose lookup is refused, and why.
    failures: BTreeMap<String, LookupFailure>,
}

impl MockLexicon {
    /// Builds a dictionary from its three tables.
    ///
    /// # Parameters
    ///
    /// - `words`: key to the words stored under it, strongest first.
    /// - `singles`: spelled syllable to its single-character fallbacks.
    /// - `failures`: keys whose lookup is refused, and why.
    ///
    /// # Returns
    ///
    /// The dictionary.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(
        words: BTreeMap<String, Vec<OwnedWord>>,
        singles: BTreeMap<String, Vec<OwnedWord>>,
        failures: BTreeMap<String, LookupFailure>,
    ) -> Self {
        Self {
            words,
            singles,
            failures,
        }
    }
}

impl Lexicon for MockLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        if let Some(failure) = self.failures.get(key) {
            return Err(failure.error());
        }
        let words = self
            .words
            .get(key)
            .map_or(&[][..], |stored| stored.as_slice());
        Ok(WordIter::from_vec(
            words.iter().map(OwnedWord::borrow).collect(),
        ))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        // Prefix enumeration is a Phase 2 capability; the frozen contract says the
        // Phase 1 answer is this code rather than an invented one.
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let words = syllable_at(syl)
            .and_then(|spelling| self.singles.get(spelling))
            .map_or(&[][..], |stored| stored.as_slice());
        Ok(WordIter::from_vec(
            words.iter().take(limit).map(OwnedWord::borrow).collect(),
        ))
    }
}

/// A user-frequency source with fixed contents.
///
/// Both `freq` and `is_user_word` are asked about a *word's text*, not about a syllable
/// key: the decoder calls them with the text it is about to rank.
#[derive(Debug, Default)]
pub struct MockUserFreq {
    /// Word text to the count the user has committed it.
    counts: BTreeMap<String, u32>,
    /// Words the user is credited with coining.
    coined: BTreeSet<String>,
}

impl MockUserFreq {
    /// Builds a source from its two tables.
    ///
    /// # Parameters
    ///
    /// - `counts`: word text to the number of times the user has committed it.
    /// - `coined`: the words the user is credited with coining.
    ///
    /// # Returns
    ///
    /// The source.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(counts: BTreeMap<String, u32>, coined: BTreeSet<String>) -> Self {
        Self { counts, coined }
    }
}

impl UserFreqSource for MockUserFreq {
    fn freq(&self, key: &str) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    fn record(&self, _key: &str, _weight_hint: u16) {
        // Deliberately a no-op. A double that accumulated commits would make the second
        // replay of a scenario rank differently from the first, and the whole point of
        // this harness is that the same input always produces the same candidate order.
    }

    fn is_user_word(&self, key: &str) -> bool {
        self.coined.contains(key)
    }
}

/// The three injected data sources one scenario runs against.
///
/// A plain bundle of borrowed trait objects: copying it copies the borrows and nothing
/// else, which is what lets a session hold the bundle by value while the caller keeps
/// owning the doubles.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    /// The dictionary the lattice is built from.
    pub lexicon: &'a dyn Lexicon,
    /// The user's learned frequencies and coined words.
    pub user_freq: &'a dyn UserFreqSource,
    /// The model the ranking scores with.
    pub lm: &'a dyn LanguageModel,
}
