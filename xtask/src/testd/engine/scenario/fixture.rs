//! The doubles a scenario's dictionary spec builds into.
//!
//! A fixture is the pairing of one scenario with the dictionary, the model and the user
//! frequency tables it runs against. Building it is where a spec that cannot describe a
//! real dictionary is refused -- an unknown failure code, a fallback for a string the
//! syllable table does not hold, a key spanning more syllables than a word can consume --
//! so that a scenario fails at load time with a message about the spec rather than
//! mid-run with a message about the engine.
//!
//! # The v2 extras are load-time, not drive-time
//!
//! A scenario's `[user_db]` section and its `bigram = "present"` mode replace the doubles
//! with the real things: a user store opened at a scratch path, and the compiler's own
//! container read as the model. Both are built *here*, while the fixture is assembled --
//! the same layer that reads the scenario file -- and never during the drive: a replay
//! step opens no file and reads no clock, which is the purity the `--strace` check
//! traces. The built-in set carries no v2 section, so the check's runs stay pure by
//! construction.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, ensure};
use ime_core::lm::InMemoryLm;
use ime_core::segment::lookup as syllable_lookup;
use ime_dict::fst_index::DictLm;

use crate::testd::dict_inject::{DictFixture, Loaded};
use crate::testd::engine::doubles::{LookupFailure, MockLexicon, MockUserFreq, OwnedWord, Sources};
use crate::testd::engine::scenario::store::{
    StoreFixture, build_store, remove_scratch, scratch_dir,
};
use crate::testd::engine::scenario::{BigramMode, DictionarySpec, Scenario, ScenarioV2};

/// One scenario together with the doubles its dictionary describes.
pub struct EngineFixture {
    /// The scenario to run.
    pub scenario: Scenario,
    /// The dictionary built from the scenario's spec.
    pub lexicon: MockLexicon,
    /// The user frequencies and coined words built from the scenario's spec.
    ///
    /// A scenario whose `[user_db]` section names a store does not run against this: the
    /// store is the source then, and this field holds what a v1 scenario's spec built.
    pub user_freq: MockUserFreq,
    /// The model built from the scenario's spec.
    ///
    /// A scenario whose `[decode]` section sets `bigram = "present"` does not run against
    /// this: the container's model is the source then.
    pub lm: InMemoryLm,
    /// The real user store, when the `[user_db]` section named one.
    store: Option<StoreFixture>,
    /// The container the `bigram = "present"` model reads, kept so its mapping outlives
    /// the model built over it.
    container: Option<Loaded>,
    /// The model built over the container.
    container_lm: Option<DictLm>,
}

impl std::fmt::Debug for EngineFixture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The doubles are named by what they were built from: a test failure names the
        // scenario, and the store and container behind it are files under scratch
        // directories nobody debugs by value.
        formatter
            .debug_struct("EngineFixture")
            .field("scenario", &self.scenario.name)
            .finish_non_exhaustive()
    }
}

impl EngineFixture {
    /// Builds the doubles a scenario's dictionary spec describes.
    ///
    /// # Returns
    ///
    /// The fixture, ready to run.
    ///
    /// # Errors
    ///
    /// Returns an error when a failure code is not one of the three the mock dictionary
    /// can answer with, when a spec declares more than one of them, when a fallback names
    /// a string the syllable table does not hold, or when a key spans more syllables than
    /// a word can consume. A scenario with v2 sections is refused here too, when its
    /// sections cannot be assembled together or when the store or container they name
    /// cannot be built.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(scenario: Scenario) -> anyhow::Result<Self> {
        let v2 = ScenarioV2::of(&scenario)?;
        let codes = scenario.dictionary.failure_codes();
        ensure!(
            codes.len() <= 1,
            "{}: a scenario's dictionary may refuse lookups with one code, not {codes:?}",
            scenario.name
        );
        let lexicon = build_lexicon(&scenario.dictionary)?;
        let user_freq = build_user_freq(&scenario.dictionary);
        let lm = build_lm(&scenario.dictionary);
        let store = match &v2.user_db {
            Some(fixture) => Some(
                build_store(fixture, v2.decay)
                    .with_context(|| format!("{}: building the [user_db] store", scenario.name))?,
            ),
            None => None,
        };
        let (container, container_lm) = match v2.bigram {
            BigramMode::Absent => (None, None),
            BigramMode::Present => {
                let loaded = DictFixture::BigramPresent
                    .generate_in(scratch_dir("container")?.as_path())
                    .with_context(|| format!("{}: building the bigram container", scenario.name))?;
                let model = DictLm::new(&loaded.lexicon);
                (Some(loaded), Some(model))
            }
        };
        Ok(Self {
            scenario,
            lexicon,
            user_freq,
            lm,
            store,
            container,
            container_lm,
        })
    }

    /// Returns the sources to inject for this fixture.
    ///
    /// A scenario with v2 sections gets the real store and the container's model here;
    /// every other scenario gets the spec-built doubles, exactly as v1 did.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn sources(&self) -> Sources<'_> {
        Sources {
            lexicon: &self.lexicon,
            user_freq: match &self.store {
                Some(store) => &store.db,
                None => &self.user_freq,
            },
            lm: match &self.container_lm {
                Some(model) => model,
                None => &self.lm,
            },
        }
    }
}

impl Drop for EngineFixture {
    /// Takes the scratch directories a v2 fixture built away.
    ///
    /// The store and the container own files under temporary directories, and a fixture
    /// that already failed must not be turned into an abort by a second panic: the
    /// cleanup results are dropped rather than unwrapped. The fields drop before the
    /// directories -- the store's flush thread must have settled the file before the
    /// directory under it is removed -- which is the order struct fields drop in.
    fn drop(&mut self) {
        let store_dir = self.store.take().map(|store| store.dir);
        let container_dir = self
            .container
            .take()
            .map(|container| container.generated.path.parent().map(PathBuf::from));
        // Dropping the values above ran their own teardowns; the directories themselves
        // go now, and a directory that would not go is a leaked empty scratch tree, which
        // is what a second fixture's serial number exists to tolerate.
        for dir in store_dir
            .into_iter()
            .chain(container_dir.into_iter().flatten())
        {
            remove_scratch(dir);
        }
    }
}

/// Builds the mock dictionary a spec describes.
///
/// # Errors
///
/// As [`EngineFixture::new`].
///
/// # Panics
///
/// Never panics.
fn build_lexicon(spec: &DictionarySpec) -> anyhow::Result<MockLexicon> {
    let mut words: BTreeMap<String, Vec<OwnedWord>> = BTreeMap::new();
    for row in &spec.words {
        let syllables = u8::try_from(row.key.split('\'').count())
            .context("a dictionary key spans more syllables than a word can consume")?;
        let stored = words.entry(row.key.clone()).or_default();
        for text in &row.texts {
            let position = stored.len();
            stored.push(OwnedWord::at(position, syllables, text.clone()));
        }
    }

    let mut singles: BTreeMap<String, Vec<OwnedWord>> = BTreeMap::new();
    for row in &spec.singles {
        ensure!(
            syllable_lookup(&row.syllable).is_some(),
            "{:?} is not a syllable of the table",
            row.syllable
        );
        let stored = singles.entry(row.syllable.clone()).or_default();
        for text in &row.texts {
            let position = stored.len();
            stored.push(OwnedWord::at(position, 1, text.clone()));
        }
    }

    let mut failures: BTreeMap<String, LookupFailure> = BTreeMap::new();
    for (key, code) in &spec.failures {
        let failure = LookupFailure::parse(code)
            .map_err(|reason| anyhow::anyhow!("dictionary key {key:?}: {reason}"))?;
        failures.insert(key.clone(), failure);
    }
    Ok(MockLexicon::new(words, singles, failures))
}

/// Builds the user-frequency double a spec describes.
///
/// # Panics
///
/// Never panics.
fn build_user_freq(spec: &DictionarySpec) -> MockUserFreq {
    let coined: BTreeSet<String> = spec.user_words.iter().map(|row| row.text.clone()).collect();
    MockUserFreq::new(spec.user_freq.clone(), coined)
}

/// Builds the model a spec describes.
///
/// # Panics
///
/// Never panics.
fn build_lm(spec: &DictionarySpec) -> InMemoryLm {
    let mut lm = InMemoryLm::new();
    for (word, score) in &spec.unigrams {
        lm.insert_unigram(word, *score);
    }
    for row in &spec.bigrams {
        lm.insert_bigram(&row.prev, &row.word, row.score);
    }
    lm
}
