//! Tests for the generated fixtures: what the generator produces is held to the expectation
//! table in `tests/fixtures/dict/expectations.toml`, determinism is held to `sha256`, and the
//! containers are opened with the real reader and loaded with the real lexicon.
//!
//! The table is read once per test from the repository's own file, so a fixture and its
//! expectations cannot drift apart silently: a change to a word table in [`super`] that is
//! not mirrored in the table fails here, and so does the reverse.

use std::collections::BTreeMap;

use serde::Deserialize;

use ime_dict::format::reader::Reader;
use ime_dict::format::{FLAG_BIGRAM_PRESENT, PROB_Q12_MAX, hash_word};
use ime_types::Lexicon;

use super::support::Scratch;
use super::{CORPUS_SNIPPET, DictFixture};

/// The expectation table, as the repository states it.
#[derive(Deserialize)]
struct Expectations {
    /// Table format version, read to refuse a table a later build wrote.
    version: u32,
    /// The synthetic build's expectations.
    synthetic: FixtureExpectation,
    /// The layered build's expectations.
    layered: LayeredExpectation,
    /// The bigram build's expectations.
    bigram_present: BigramExpectation,
    /// The staged variant's expectations.
    staged_variant: FixtureExpectation,
}

/// Counts, a within-key order table and a unigram probability table for one fixture.
#[derive(Deserialize)]
struct FixtureExpectation {
    /// The file the fixture lands as.
    file: String,
    /// How many words the container holds.
    words: u32,
    /// How many keys its index holds.
    keys: u32,
    /// Key -> candidates, heaviest first, for the keys the table pins.
    #[serde(default)]
    order: BTreeMap<String, Vec<String>>,
    /// Word -> unigram Q12, for the words the table pins.
    #[serde(default)]
    prob: BTreeMap<String, u16>,
}

/// The layered fixture's expectations, with the band membership the normalization cases
/// assert against.
#[derive(Deserialize)]
struct LayeredExpectation {
    /// The file the fixture lands as.
    file: String,
    /// How many words the container holds.
    words: u32,
    /// How many keys its index holds.
    keys: u32,
    /// Words that keep their character frequency.
    char_band: Vec<String>,
    /// Two-character words at or above the floor.
    core_band: Vec<String>,
    /// Words flattened to the tail constant.
    tail_band: Vec<String>,
    /// Word -> unigram Q12, for the words the table pins.
    prob: BTreeMap<String, u16>,
}

/// The bigram fixture's expectations: the pairs the corpus produces and the Q12s the
/// `count * 4096 / right-marginal` arithmetic pins.
#[derive(Deserialize)]
struct BigramExpectation {
    /// The file the fixture lands as.
    file: String,
    /// How many words the container holds.
    words: u32,
    /// How many keys its index holds.
    keys: u32,
    /// How many records the section carries.
    pairs: u32,
    /// Every pair, as `"previous next"`, that the section must hold.
    present: Vec<String>,
    /// Pair -> Q12, for the pairs the table pins.
    prob: BTreeMap<String, u16>,
}

/// Reads the expectation table.
///
/// # Panics
///
/// Never; the table is a repository file this build compiles against.
fn expectations() -> Expectations {
    toml::from_str(include_str!(
        "../../../../tests/fixtures/dict/expectations.toml"
    ))
    .expect("the expectation table parses")
}

/// The pair key the section stores, the same arithmetic the compiler counts with.
fn pair_key(pair: &str) -> u64 {
    let (previous, next) = pair.split_once(' ').expect("a two-word pair");
    u64::from(hash_word(previous)) << 32 | u64::from(hash_word(next))
}

#[test]
fn test_every_fixture_generates_deterministically_and_inside_its_budget() {
    let scratch = Scratch::new("generate-determinism");
    for fixture in DictFixture::all() {
        let first = scratch.path(&format!("{}-1.dict", fixture.name()));
        let second = scratch.path(&format!("{}-2.dict", fixture.name()));
        let began = std::time::Instant::now();
        let generated = fixture
            .generate(&first)
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.name()));
        let elapsed = began.elapsed();
        let again = fixture
            .generate(&second)
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.name()));
        assert_eq!(
            generated.digest,
            again.digest,
            "{}: two generations are the same bytes",
            fixture.name()
        );
        assert!(
            elapsed <= std::time::Duration::from_millis(200),
            "{}: generation took {elapsed:?}, over the 200 ms budget",
            fixture.name()
        );
    }
}

#[test]
fn test_every_generated_container_opens_from_disk() {
    // The physical shape of the fixture family: four files a `Reader::open` accepts, with
    // the counts the table states and the bigram flag on exactly the container that owes it.
    let expect = expectations();
    assert_eq!(expect.version, 1, "the table this build reads");
    let scratch = Scratch::new("generate-open");
    for fixture in DictFixture::all() {
        let generated = fixture
            .generate(&scratch.path(fixture.file_name()))
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.name()));
        let reader = Reader::open(&generated.path)
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.name()));
        let expected = match fixture {
            DictFixture::Synthetic => (
                expect.synthetic.file.as_str(),
                expect.synthetic.words,
                expect.synthetic.keys,
                0,
            ),
            DictFixture::Layered => (
                expect.layered.file.as_str(),
                expect.layered.words,
                expect.layered.keys,
                0,
            ),
            DictFixture::BigramPresent => (
                expect.bigram_present.file.as_str(),
                expect.bigram_present.words,
                expect.bigram_present.keys,
                expect.bigram_present.pairs,
            ),
            DictFixture::StagedVariant => (
                expect.staged_variant.file.as_str(),
                expect.staged_variant.words,
                expect.staged_variant.keys,
                0,
            ),
        };
        assert_eq!(
            fixture.file_name(),
            expected.0,
            "{}: the table's file",
            fixture.name()
        );
        assert_eq!(generated.words, expected.1, "{}: words", fixture.name());
        assert_eq!(generated.keys, expected.2, "{}: keys", fixture.name());
        assert_eq!(
            u32::try_from(reader.bigram_count()).unwrap_or(u32::MAX),
            expected.3,
            "{}: bigram records",
            fixture.name()
        );
        let has_bigram = reader.header().flags & FLAG_BIGRAM_PRESENT != 0;
        assert_eq!(
            has_bigram,
            fixture == DictFixture::BigramPresent,
            "{}: the bigram flag marks exactly the container that owes it",
            fixture.name()
        );
    }
}

#[test]
fn test_synthetic_encodes_the_declared_order_and_probs() {
    let expect = expectations();
    let scratch = Scratch::new("generate-synthetic");
    let loaded = DictFixture::Synthetic
        .generate_in(scratch.path("dir").as_path())
        .expect("generating the synthetic fixture");
    let reader = Reader::open(&loaded.generated.path).expect("opening the container");
    for (key, wanted) in &expect.synthetic.order {
        let words: Vec<&str> = reader
            .key_words(key)
            .unwrap_or_else(|error| panic!("{key}: {error}"))
            .into_iter()
            .collect();
        assert_eq!(&words, wanted, "{key}: within-key order is the weight's");
    }
    for (word, wanted) in &expect.synthetic.prob {
        let (_, prob) = reader
            .unigram_lookup(word)
            .expect("the unigram table reads")
            .unwrap_or_else(|| panic!("{word}: not in the unigram table"));
        assert_eq!(prob, *wanted, "{word}: the banded weight, quantized");
    }
}

#[test]
fn test_layered_lands_every_word_in_the_band_the_table_declares() {
    let expect = expectations();
    let scratch = Scratch::new("generate-layered");
    let loaded = DictFixture::Layered
        .generate_in(scratch.path("dir").as_path())
        .expect("generating the layered fixture");
    let reader = Reader::open(&loaded.generated.path).expect("opening the container");
    // The tail is the normalization's subject: every word the table names there carries the
    // same flat Q12, whatever its source frequency was -- 700 and 500 land where 10 and 5
    // do.
    let tail = expect
        .layered
        .prob
        .get(
            expect
                .layered
                .tail_band
                .first()
                .expect("the table names a tail"),
        )
        .copied()
        .expect("the table pins the tail's Q12");
    for word in &expect.layered.tail_band {
        let (_, prob) = reader
            .unigram_lookup(word)
            .expect("the unigram table reads")
            .unwrap_or_else(|| panic!("{word}: not in the unigram table"));
        assert_eq!(prob, tail, "{word}: the tail is flat");
    }
    for (word, wanted) in &expect.layered.prob {
        let (_, prob) = reader
            .unigram_lookup(word)
            .expect("the unigram table reads")
            .unwrap_or_else(|| panic!("{word}: not in the unigram table"));
        assert_eq!(prob, *wanted, "{word}: the banded weight, quantized");
    }
    // The bands partition the container: every word lands in exactly one.
    let mut expected_words = expect.layered.char_band.clone();
    expected_words.extend(expect.layered.core_band.iter().cloned());
    expected_words.extend(expect.layered.tail_band.iter().cloned());
    expected_words.sort();
    let mut actual_words = (0..reader.entry_count())
        .map(|index| {
            let entry = reader.entry(index).expect("the entry table reads");
            reader.word(&entry).expect("the pool serves it").to_owned()
        })
        .collect::<Vec<_>>();
    actual_words.sort();
    assert_eq!(
        actual_words, expected_words,
        "the bands cover every word once"
    );
}

#[test]
fn test_bigram_present_carries_every_pair_the_corpus_produces() {
    let expect = expectations();
    let scratch = Scratch::new("generate-bigram");
    let generated = DictFixture::BigramPresent
        .generate(&scratch.path(DictFixture::BigramPresent.file_name()))
        .expect("generating the bigram fixture");
    let reader = Reader::open(&generated.path).expect("opening the container");
    let records: BTreeMap<u64, u16> = (0..u32::try_from(reader.bigram_count()).unwrap_or(u32::MAX))
        .map(|index| {
            let record = reader.bigram(index).expect("the section reads");
            (record.pair_key, record.prob_q12)
        })
        .collect();
    for pair in &expect.bigram_present.present {
        let key = pair_key(pair);
        assert!(
            records.contains_key(&key),
            "{pair}: the section holds every declared pair"
        );
    }
    assert_eq!(
        records.len(),
        expect.bigram_present.pairs as usize,
        "the section holds exactly the corpus's distinct pairs"
    );
    for (pair, wanted) in &expect.bigram_present.prob {
        let prob = records
            .get(&pair_key(pair))
            .unwrap_or_else(|| panic!("{pair}: in the section"));
        assert_eq!(prob, wanted, "{pair}: count over right marginal, quantized");
    }
}

#[test]
fn test_staged_variant_shares_the_keys_and_inverts_the_order() {
    let expect = expectations();
    let scratch = Scratch::new("generate-staged");
    let synthetic = DictFixture::Synthetic
        .generate(&scratch.path(DictFixture::Synthetic.file_name()))
        .expect("generating the synthetic fixture");
    let staged = DictFixture::StagedVariant
        .generate(&scratch.path(DictFixture::StagedVariant.file_name()))
        .expect("generating the staged variant");
    assert_ne!(
        synthetic.digest, staged.digest,
        "the two containers are distinguishable bytes"
    );
    let staged_reader = Reader::open(&staged.path).expect("opening the staged container");
    for (key, wanted) in &expect.staged_variant.order {
        let words: Vec<&str> = staged_reader
            .key_words(key)
            .unwrap_or_else(|error| panic!("{key}: {error}"))
            .into_iter()
            .collect();
        assert_eq!(&words, wanted, "{key}: the staged order");
    }
    // Through the lexicon, the way a hot-swap case reads it: the same key serves a different
    // first candidate after the swap.
    let before = DictFixture::Synthetic
        .generate_in(scratch.path("before").as_path())
        .expect("loading the synthetic build");
    let after = DictFixture::StagedVariant
        .generate_in(scratch.path("after").as_path())
        .expect("loading the staged variant");
    for key in expect.staged_variant.order.keys() {
        let first = |loaded: &super::Loaded| {
            loaded
                .lexicon
                .lookup(key)
                .expect("the key reads")
                .next()
                .expect("the key serves a candidate")
                .text
                .to_owned()
        };
        assert_ne!(
            first(&before),
            first(&after),
            "{key}: the swap is observable in the first candidate"
        );
    }
}
