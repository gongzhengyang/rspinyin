//! The doubles a scenario's dictionary spec builds into.
//!
//! A fixture is the pairing of one scenario with the dictionary, the model and the user
//! frequency tables it runs against. Building it is where a spec that cannot describe a
//! real dictionary is refused -- an unknown failure code, a fallback for a string the
//! syllable table does not hold, a key spanning more syllables than a word can consume --
//! so that a scenario fails at load time with a message about the spec rather than
//! mid-run with a message about the engine.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, ensure};
use ime_core::lm::InMemoryLm;
use ime_core::segment::lookup as syllable_lookup;

use crate::testd::engine::doubles::{
    LookupFailure, MockLexicon, MockUserFreq, OwnedWord, Sources,
};
use crate::testd::engine::scenario::{DictionarySpec, Scenario};

/// One scenario together with the doubles its dictionary describes.
#[derive(Debug)]
pub struct EngineFixture {
    /// The scenario to run.
    pub scenario: Scenario,
    /// The dictionary built from the scenario's spec.
    pub lexicon: MockLexicon,
    /// The user frequencies and coined words built from the scenario's spec.
    pub user_freq: MockUserFreq,
    /// The model built from the scenario's spec.
    pub lm: InMemoryLm,
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
    /// a word can consume.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(scenario: Scenario) -> anyhow::Result<Self> {
        let codes = scenario.dictionary.failure_codes();
        ensure!(
            codes.len() <= 1,
            "{}: a scenario's dictionary may refuse lookups with one code, not {codes:?}",
            scenario.name
        );
        let lexicon = build_lexicon(&scenario.dictionary)?;
        let user_freq = build_user_freq(&scenario.dictionary);
        let lm = build_lm(&scenario.dictionary);
        Ok(Self {
            scenario,
            lexicon,
            user_freq,
            lm,
        })
    }

    /// Returns the sources to inject for this fixture.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn sources(&self) -> Sources<'_> {
        Sources {
            lexicon: &self.lexicon,
            user_freq: &self.user_freq,
            lm: &self.lm,
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
    let coined: BTreeSet<String> = spec
        .user_words
        .iter()
        .map(|row| row.text.clone())
        .collect();
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

