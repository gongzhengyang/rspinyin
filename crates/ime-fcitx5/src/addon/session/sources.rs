//! The four trait objects a session decodes against, and their degraded stand-ins.
//!
//! Responsibility: wrap the concrete sources this plugin loads — the mapped dictionary, the
//! language model read out of its unigram table, the user's frequency store — in the three
//! frozen traits the decoder reads, and answer the same traits when a source could not be
//! loaded at all.
//!
//! Boundaries: this module decides nothing about *when* a source is loaded or what a
//! failure means; it only makes a missing source a value the decoder can run on. The
//! loading is [`super::load_lexicon`]'s, and the reporting is the step's.
//!
//! # Why an absent source is a value and not an error
//!
//! The decoder answers a request it cannot decode with a candidate rather than with an
//! error, and the same has to hold one level up: a plugin with no dictionary must still
//! produce something for the user to commit. An empty dictionary and a model that knows no
//! word are exactly that — the decoder's own pass-through path turns them into "the text
//! you typed", which is the honest degradation and never a swallowed key.
//!
//! # Concurrency
//!
//! All three types are `Send + Sync` and reentrant. [`Dictionary::Mapped`] and
//! [`LanguageModelSource::Mapped`] read immutable mapped memory and take no lock;
//! [`UserFrequency::Store`] takes the store's own lock, which is the lock the shutdown
//! flush also takes and which no caller holds across another call.

use std::io::Write;
use std::sync::Arc;

use ime_core::lm::{BIGRAM_MISS_PENALTY, UNIGRAM_MISS};
use ime_dict::fst_index::{DictLm, FstLexicon};
use ime_types::{ImeError, LanguageModel, Lexicon, SyllableId, UserFreqSource, WordIter, WordRef};

use super::super::lock;
use super::super::user_store::UserStore;

/// The dictionary a session decodes against.
///
/// The mapped lexicon is boxed so that the two variants of this enum stay the same size:
/// a value that carried the whole lexicon inline would be one every `SessionSources` pays
/// for, and the indirection costs one dereference per lookup against an FST walk.
pub(super) enum Dictionary {
    /// The compiled dictionary, mapped read-only and verified at load.
    Mapped(Box<FstLexicon>),
    /// No dictionary at all: every lookup answers nothing.
    ///
    /// The decoder turns that into one candidate holding the input unchanged, which is what
    /// the user gets to commit.
    Missing,
}

impl Lexicon for Dictionary {
    /// Reads the words of `key`, or nothing at all when no dictionary is mapped.
    ///
    /// # Panics
    ///
    /// Never.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        match self {
            Self::Mapped(lexicon) => lexicon.lookup(key),
            Self::Missing => Ok(WordIter::from_slice(&[])),
        }
    }

    /// Prefix enumeration, which neither variant serves yet.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] with a dictionary, because the mapped lexicon answers the
    /// same code until the phase that implements it lands.
    ///
    /// # Panics
    ///
    /// Never.
    fn prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError> {
        match self {
            Self::Mapped(lexicon) => lexicon.prefix(prefix, limit),
            // Prefix enumeration is a later phase even with a dictionary, so an absent one
            // answers the same code the mapped one does.
            Self::Missing => Err(ImeError::Unsupported),
        }
    }

    /// Reads the single-character words of one syllable, or nothing at all.
    ///
    /// # Errors
    ///
    /// As the mapped lexicon's, which reports a syllable identifier outside its table.
    ///
    /// # Panics
    ///
    /// Never.
    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        match self {
            Self::Mapped(lexicon) => lexicon.fallback_single(syl, limit),
            Self::Missing => Ok(WordIter::from_slice(&[])),
        }
    }
}

/// The language model a session scores with.
///
/// Boxed for the same reason the dictionary is, and more urgently: the model carries a
/// sixteen-kilobyte score table, so an inline variant would make every value of this enum
/// that size and every construction copy it.
pub(super) enum LanguageModelSource {
    /// The unigram table of the mapped dictionary, read out of its mapping.
    Mapped(Box<DictLm>),
    /// No table at all: every word answers the miss value.
    Missing,
}

impl LanguageModel for LanguageModelSource {
    /// The word's unigram score in Q8.8, or the floor when no table is mapped.
    ///
    /// # Panics
    ///
    /// Never.
    fn unigram(&self, word: &str) -> i32 {
        match self {
            Self::Mapped(model) => model.unigram(word),
            Self::Missing => UNIGRAM_MISS,
        }
    }

    /// The word's conditional score, which is the miss path with no table.
    ///
    /// # Panics
    ///
    /// Never.
    fn bigram(&self, prev: &str, word: &str) -> i32 {
        match self {
            Self::Mapped(model) => model.bigram(prev, word),
            // The miss path of the mapped model, value for value: a word the table does not
            // hold and a table that holds nothing have to score identically, so that the
            // degraded start ranks the candidates the same way a dictionary full of
            // unknown words would.
            Self::Missing => UNIGRAM_MISS.saturating_add(BIGRAM_MISS_PENALTY),
        }
    }
}

/// The user's own frequencies, as the decoder's source.
pub(super) enum UserFrequency {
    /// The store the process adopted, behind the lock its shutdown flush also takes.
    Store(Arc<UserStore>),
    /// No store: nothing is read and nothing is learned.
    ///
    /// A store that could not be opened is the read-only degradation: the plugin keeps
    /// committing text and stops learning, which is what `ASM-15` asks for.
    Missing,
}

impl UserFreqSource for UserFrequency {
    /// The count the store holds for `key`, or zero with no store.
    ///
    /// The store's own lock is taken for the call and released before it returns, which is
    /// what the frozen contract asks of a lookup on the decode path.
    ///
    /// # Panics
    ///
    /// Never.
    fn freq(&self, key: &str) -> u32 {
        match self {
            Self::Store(store) => lock(&store.db).freq(key),
            Self::Missing => 0,
        }
    }

    /// Records one commit, or nothing at all with no store.
    ///
    /// The store batches and flushes on a thread of its own, so this stays a lock, a hash
    /// lookup and a wakeup.
    ///
    /// # Panics
    ///
    /// Never.
    fn record(&self, key: &str, weight_hint: u16) {
        if let Self::Store(store) = self {
            lock(&store.db).record(key, weight_hint);
        }
    }

    /// Whether `key` is a word the user coined, which no version of the store answers yet.
    ///
    /// # Panics
    ///
    /// Never.
    fn is_user_word(&self, key: &str) -> bool {
        match self {
            Self::Store(store) => lock(&store.db).is_user_word(key),
            Self::Missing => false,
        }
    }

    /// Drops `key` from the user's learned frequencies.
    ///
    /// # Panics
    ///
    /// Never.
    fn forget(&self, key: &str) -> bool {
        match self {
            Self::Store(store) => lock(&store.db).forget(key),
            Self::Missing => false,
        }
    }

    /// Lists the user's learned words, which this source cannot serve.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] always.
    ///
    /// # Panics
    ///
    /// Never.
    fn list(&self, offset: u64, limit: u16) -> Result<Vec<WordRef<'_>>, ImeError> {
        // The store's own `list` answers `dict/unsupported` and documents why: a `WordRef`
        // borrows for as long as `&self`, and the store's keys live behind a lock, so there
        // is no safe way to publish them. This wrapper cannot do better — the borrow it
        // would hand out lives inside the guard it holds — so the answer is the same code
        // the store answers with, without taking the lock to read a constant.
        let _ = (offset, limit);
        Err(ImeError::Unsupported)
    }

    /// Writes the user's learned words as TSV, which is the store's own export.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] with no store, and whatever the export raises with one.
    ///
    /// # Panics
    ///
    /// Never.
    fn export_tsv(&self, writer: &mut dyn Write) -> Result<u64, ImeError> {
        match self {
            Self::Store(store) => lock(&store.db).export_tsv(writer),
            Self::Missing => Err(ImeError::Unsupported),
        }
    }
}
