//! The degraded stand-ins a session decodes against when no container is mapped, and the
//! user's frequency source.
//!
//! Responsibility: answer the two frozen traits the decoder reads when the process started
//! without a dictionary, and wrap the user's frequency store in the third. The mapped
//! container itself is not a value of this module: the live one is owned by the hot-swap
//! slot in [`super`], which the router re-reads at every step, and a start that mapped
//! nothing holds the stand-ins below.
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
//! All three types are `Send + Sync` and reentrant. The stand-ins hold no state at all and
//! read nothing; [`UserFrequency::Store`] takes the store's own lock, which is the lock the
//! shutdown flush also takes and which no caller holds across another call.

use std::io::Write;
use std::sync::Arc;

use ime_core::lm::{BIGRAM_MISS_PENALTY, UNIGRAM_MISS};
use ime_types::{ImeError, LanguageModel, Lexicon, SyllableId, UserFreqSource, WordIter, WordRef};

use super::super::lock;
use super::super::user_store::UserStore;

/// The empty dictionary of a start with no container: every lookup answers nothing.
///
/// The decoder turns that into one candidate holding the input unchanged, which is what
/// the user gets to commit.
pub(in crate::addon) struct MissingDictionary;

impl Lexicon for MissingDictionary {
    /// Answers nothing: there are no words to read.
    ///
    /// # Panics
    ///
    /// Never.
    fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(&[]))
    }

    /// Prefix enumeration, which no start without a container serves.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`], the same code a mapped lexicon answers: prefix
    /// enumeration is a later phase either way.
    ///
    /// # Panics
    ///
    /// Never.
    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    /// Answers nothing: there are no single-character words to read.
    ///
    /// # Panics
    ///
    /// Never.
    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(&[]))
    }
}

/// The model of a start with no container: every word answers the miss path.
pub(in crate::addon) struct MissingLanguageModel;

impl LanguageModel for MissingLanguageModel {
    /// The floor: no word is known.
    ///
    /// # Panics
    ///
    /// Never.
    fn unigram(&self, _word: &str) -> i32 {
        UNIGRAM_MISS
    }

    /// The miss path of the mapped model, value for value.
    ///
    /// # Panics
    ///
    /// Never.
    fn bigram(&self, _prev: &str, _word: &str) -> i32 {
        // A word the table does not hold and a table that holds nothing have to score
        // identically, so that the degraded start ranks the candidates the same way a
        // dictionary full of unknown words would.
        UNIGRAM_MISS.saturating_add(BIGRAM_MISS_PENALTY)
    }
}

/// The empty dictionary of the process, for the environment of a degraded start.
pub(in crate::addon) static MISSING_DICTIONARY: MissingDictionary = MissingDictionary;

/// The model of the process, for the environment of a degraded start.
pub(in crate::addon) static MISSING_LANGUAGE_MODEL: MissingLanguageModel = MissingLanguageModel;

/// The user's own frequencies, as the decoder's source.
pub(in crate::addon) enum UserFrequency {
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

    /// Toggles, moves or clears a pin, or reports that there is no store to hold one.
    ///
    /// # Errors
    ///
    /// [`ImeError::Unsupported`] with no store, and whatever the toggle raises with one --
    /// `data/readonly-mode` when the store has degraded.
    ///
    /// # Panics
    ///
    /// Never.
    fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
        match self {
            Self::Store(store) => lock(&store.db).set_pinned(key, word),
            Self::Missing => Err(ImeError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The frequency wrapper's own arms: what an absent store answers, and that a real
    //! one receives what the session hands it.

    use ime_dict::user_db::{BackupConfig, UserDb};
    use std::sync::Mutex;

    use super::*;

    /// Opens a store in a directory of the test's own, under the system temp directory.
    fn store_in(label: &str) -> (std::path::PathBuf, UserDb) {
        let dir =
            std::env::temp_dir().join(format!("rspinyin-sources-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating the test directory");
        let db = UserDb::open(dir.join("user.redb")).expect("opening the test store");
        (dir, db)
    }

    #[test]
    fn test_missing_store_refuses_the_pin_and_answers_nothing_else() {
        let missing = UserFrequency::Missing;
        let error = missing
            .set_pinned("ni", Some("你"))
            .expect_err("there is no store to hold a pin");
        assert!(matches!(error, ImeError::Unsupported), "{error}");
        assert_eq!(missing.freq("你"), 0);
        assert!(!missing.forget("你"));
    }

    #[test]
    fn test_store_source_receives_the_pin_toggle() {
        let (_dir, mut db) = store_in("pin");
        db.record("你", 0);
        // The record is flushed before the toggling starts -- `final_commit` runs on the
        // caller's thread -- so the reads below answer from the flushed count and never
        // land in the window a flush's drain leaves between the delta map and the map
        // that adopted it.
        db.final_commit().expect("flushing the record");
        let store = UserStore {
            db: Mutex::new(db),
            backup: BackupConfig::disabled(_dir.clone()),
        };
        let source = UserFrequency::Store(Arc::new(store));
        source
            .set_pinned("ni", Some("你"))
            .expect("the store takes the pin");
        assert_eq!(
            source.freq("你"),
            u32::MAX,
            "the pinned word answers the cap the ranking lifts it with"
        );
        // The second press is the toggle's other half, and the store answers its own
        // count again.
        source
            .set_pinned("ni", Some("你"))
            .expect("the store takes the release");
        assert_eq!(source.freq("你"), 1);
    }
}
