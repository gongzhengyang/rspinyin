//! Tests for the FST lexicon.
//!
//! Every test builds its container image with the public writer and writes it into a
//! scratch directory of its own, so nothing here needs a committed `base.dict`, a display
//! server or the clock.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use fst::Streamer;

use ime_core::lm::{BIGRAM_MISS_PENALTY, InMemoryLm, UNIGRAM_MISS, log2_q8};
use ime_core::viterbi::Decoder;
use ime_types::{DecodeRequest, LanguageModel, WordFlags};

use crate::format::{
    BIGRAM_ENTRY_SIZE, BigramRecord, DictEntry, FLAG_TERM, PROB_Q12_MAX, hash_word, pack_fst_value,
    reader::{Reader, Verify},
    unpack_fst_value, writer,
};

use super::*;

/// One fixture word: text, weight, syllable count and flags.
type FixtureWord = (&'static str, u32, u8, u8);

/// The fixture's words, in the order the container stores them: hash order, which the
/// builder applies. It holds the cases the read path has to get right -- two words under
/// one key in descending weight, a polyphone word reachable under two keys, and
/// single-character words under their bare syllable keys, which `fallback_single` reads.
const FIXTURE_WORDS: [FixtureWord; 7] = [
    ("你", 250_000, 1, 0),
    ("泥", 900, 1, 0),
    ("你好", 40_000, 2, 0),
    ("中", 200_000, 1, 0),
    ("中国", 45_000, 2, 0),
    ("中心", 30_000, 2, 0),
    ("银行", 5_500, 2, FLAG_TERM),
];

/// The fixture's keys and the words they hold, in ascending byte order.
const FIXTURE_KEYS: [(&str, &[&str]); 7] = [
    ("ni", &["你", "泥"]),
    ("ni'hao", &["你好"]),
    ("yin'hang", &["银行"]),
    ("yin'xing", &["银行"]),
    ("zhong", &["中"]),
    ("zhong'guo", &["中国"]),
    ("zhong'xin", &["中心"]),
];

/// What a test deliberately writes out of contract while building the image.
///
/// The variants are named after the field that is wrong rather than after the word the
/// fixture uses, because the same corruption is asserted through several keys.
/// `pub(crate)` because the hot-swap suite stages its damaged candidates through the
/// same corruption vocabulary.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Broken {
    /// `ni`'s packed value points past the end of the word list.
    ListRange,
    /// `ni`'s packed range starts inside the word list and its count runs past the end,
    /// which is the case a start-only check would let through.
    ListOverrun,
    /// `ni`'s word-list entry names a word id that does not exist.
    WordId,
    /// The record reached by `zhong` points past the end of the string pool.
    Text,
}

/// Builds the fixture's container image.
///
/// The image is assembled with the public writer, so the bytes are what the compiler
/// produces and the test needs no data directory. `broken` names a field a test wants out
/// of contract; the checksums stay the writer's, so the loader accepts the container and a
/// failure has to come from the loader's own bounds checks, not from the checksum gate.
fn fixture_image(broken: Option<Broken>) -> Vec<u8> {
    fixture_image_with_bigrams(broken, &[])
}

/// Builds the fixture's container image with an optional `BIGRAM` section.
///
/// `pub(crate)` because the hot-swap suite builds its staged candidates through the
/// same fixture: a swap test that encoded its own container would prove only that two
/// encoders agree with each other.
pub(crate) fn fixture_image_with_bigrams(
    broken: Option<Broken>,
    bigrams: &[(&str, &str, u16)],
) -> Vec<u8> {
    let mut words: Vec<FixtureWord> = FIXTURE_WORDS.to_vec();
    let order = |(text, ..): &FixtureWord| (hash_word(text), *text);
    words.sort_by_key(order);
    let [entries, strpool, unigram] = word_sections(&words, broken);
    let [fst, wordlist] = index_sections(&words, broken);
    let mut bigram = bigrams
        .iter()
        .map(|(prev, word, score)| {
            BigramRecord::new(
                u64::from(hash_word(prev)) << 32 | u64::from(hash_word(word)),
                *score,
            )
        })
        .collect::<Vec<_>>();
    bigram.sort_by_key(|record| record.pair_key);
    let bigram = bigram
        .into_iter()
        .flat_map(BigramRecord::encode)
        .collect::<Vec<_>>();

    let mut writer = writer::DictWriter::new();
    for (kind, payload) in [
        (SectionKind::Fst, fst),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
        (SectionKind::Bigram, bigram),
    ] {
        writer.add_section(kind, payload).expect("section added");
    }
    writer.encode().expect("encoding the container")
}

/// Encodes the `ENTRIES`, `STRPOOL` and `UNIGRAM` sections of `words`, in hash order;
/// `broken` writes one record out of contract and leaves the rest inside it.
fn word_sections(words: &[FixtureWord], broken: Option<Broken>) -> [Vec<u8>; 3] {
    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for (text, weight, syllables, flags) in words {
        let offset = u32::try_from(strpool.len()).expect("small fixture");
        let len = u16::try_from(text.len()).expect("small fixture");
        strpool.extend_from_slice(text.as_bytes());
        let entry = match (broken, *text) {
            (Some(Broken::Text), "中") => DictEntry::new(9_999, 3, 1, 0, *weight),
            _ => DictEntry::new(offset, len, *syllables, *flags, *weight),
        };
        entries.extend_from_slice(&entry.encode());
        unigram.extend_from_slice(&hash_word(text).to_le_bytes());
        unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }
    [entries, strpool, unigram]
}

/// Encodes the `FST` and `WORDLIST` sections: one key per fixture key, holding the ids of
/// its words in descending weight, with `broken` pointing one packed value or one
/// word-list entry out of range.
fn index_sections(words: &[FixtureWord], broken: Option<Broken>) -> [Vec<u8>; 2] {
    // Word ids are positions in hash order, so this table is all the builder needs to
    // turn a word into the id the word list stores.
    let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
    for (index, (text, ..)) in words.iter().enumerate() {
        ids.insert(*text, u32::try_from(index).expect("small fixture"));
    }

    // One bucket per key: a `BTreeMap` yields the byte order the FST builder requires.
    let mut keys: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
    for (key, texts) in FIXTURE_KEYS {
        let bucket = keys.entry(key).or_default();
        for text in texts {
            let id = *ids.get(text).expect("the fixture holds this word");
            bucket.push((words[id as usize].1, id));
        }
    }

    let entry_count = u32::try_from(words.len()).expect("small fixture");
    let total_ids: usize = keys.values().map(Vec::len).sum();
    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, mut candidates) in keys {
        candidates.sort_by_key(|(weight, _)| Reverse(*weight));
        let mut word_ids: Vec<u32> = candidates.iter().map(|(_, id)| *id).collect();
        let written = wordlist.len() / WORD_ID_SIZE;
        let start = u64::try_from(written).expect("small fixture");
        let count = u32::try_from(word_ids.len()).expect("small fixture");
        let packed = match broken {
            Some(Broken::ListRange) if key == "ni" => {
                let beyond = total_ids + 3;
                let beyond = u64::try_from(beyond).expect("small fixture");
                pack_fst_value(beyond, count).expect("packing")
            }
            Some(Broken::ListOverrun) if key == "ni" => {
                // The last id the word list holds, with a count one past what is left
                // after it: every id the range names but the first is outside the
                // section.
                let inside = total_ids.saturating_sub(1);
                let inside = u64::try_from(inside).expect("small fixture");
                pack_fst_value(inside, count + 1).expect("packing")
            }
            Some(Broken::WordId) if key == "ni" => {
                word_ids = vec![entry_count + 7];
                pack_fst_value(start, 1).expect("packing")
            }
            _ => pack_fst_value(start, count).expect("packing"),
        };
        for id in &word_ids {
            wordlist.extend_from_slice(&id.to_le_bytes());
        }
        fst_builder
            .insert(key.as_bytes(), packed)
            .expect("inserting the key");
    }
    [
        fst_builder.into_inner().expect("finishing the FST"),
        wordlist,
    ]
}

/// A fixture dictionary written into a scratch directory of its own, removed on drop.
struct ScratchDict {
    dir: PathBuf,
    path: PathBuf,
}

impl ScratchDict {
    /// Writes `image` into a fresh directory named after `tag` and this process.
    fn new(image: &[u8], tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rspinyin-fst-index-{}-{tag}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        let path = dir.join("base.dict");
        fs::write(&path, image).expect("writing the fixture");
        Self { dir, path }
    }

    /// Returns the path of the written dictionary.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDict {
    fn drop(&mut self) {
        // A test that already failed must not be turned into an abort by a second
        // panic, so the cleanup result is dropped rather than unwrapped.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Loads the fixture, failing the test if it cannot be read back.
fn fixture(tag: &str) -> FstLexicon {
    let scratch = ScratchDict::new(&fixture_image(None), tag);
    let loaded = FstLexicon::load(scratch.path());
    loaded.expect("loading the fixture")
}

/// Returns the field name of a bounds failure, or `None` for any other error.
fn bounds_field(err: &DictError) -> Option<&'static str> {
    match err {
        DictError::LengthOutOfRange { field, .. } => Some(*field),
        _ => None,
    }
}

#[test]
fn test_load_reads_the_container_and_reports_its_entry_count() {
    let scratch = ScratchDict::new(&fixture_image(None), "load");
    let loaded = FstLexicon::load(scratch.path());
    let lexicon = loaded.expect("loading the fixture");
    assert_eq!(lexicon.entry_count(), FIXTURE_WORDS.len() as u32);
    // Header-only verification is the loader's other mode: structure checked, no CRCs.
    let header_only = FstLexicon::load_with(scratch.path(), Verify::Header);
    let header_only = header_only.expect("loading without checksums");
    assert_eq!(header_only.entry_count(), 7);
}

#[test]
fn test_load_rejects_a_file_that_is_not_a_container() {
    // Long enough to hold a header, so that the magic is what rejects it rather than
    // the length check, which runs first and would report a different variant.
    let scratch = ScratchDict::new(&[0xAB; 128], "junk-magic");
    assert!(matches!(
        FstLexicon::load(scratch.path()),
        Err(DictError::MagicMismatch)
    ));

    // A file too short to hold a header is rejected before the magic is ever read.
    let stub = ScratchDict::new(b"not a dictionary at all", "junk-short");
    assert!(FstLexicon::load(stub.path()).is_err());
}

#[test]
fn test_from_image_serves_lookups_without_touching_the_filesystem() {
    // The image path skips the mapping entirely: the same fixture the loader accepts
    // from a file is validated from bytes, and the words come back borrowed from the
    // image the caller owns.
    let image: &'static [u8] = Box::leak(fixture_image(None).into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    assert_eq!(lexicon.entry_count(), FIXTURE_WORDS.len() as u32);
    let words: Vec<WordRef<'_>> = lexicon.lookup("ni").expect("looking up ni").collect();
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].text, "你");
    assert_eq!(words[0].weight, 250_000);
    assert_eq!(words[1].text, "泥");
}

#[test]
fn test_from_image_rejects_bytes_that_are_not_a_container() {
    // Long enough to hold a header, so the magic rejects it rather than the length
    // check that runs first; the same container, never written to a file, still
    // follows the same validation path.
    let junk: &'static [u8] = Box::leak(vec![0xAB; 128].into_boxed_slice());
    assert!(matches!(
        FstLexicon::from_image(junk),
        Err(DictError::MagicMismatch)
    ));
}

#[test]
fn test_load_rejects_a_container_whose_checksum_does_not_match() {
    let mut image = fixture_image(None);
    let last = image.len() - 1;
    image[last] ^= 0xFF;
    let scratch = ScratchDict::new(&image, "corrupt");
    assert!(matches!(
        FstLexicon::load(scratch.path()),
        Err(DictError::Crc { .. })
    ));
}

#[test]
fn test_load_rejects_a_container_without_an_fst_section() {
    // A container whose only section is the string pool: valid, but with no index.
    let mut writer = writer::DictWriter::new();
    writer
        .add_section(SectionKind::StrPool, b"ab".to_vec())
        .expect("adding the string pool");
    let image = writer.encode().expect("encoding");
    let scratch = ScratchDict::new(&image, "no-fst");
    let Err(err) = FstLexicon::load(scratch.path()) else {
        panic!("a container without an index must not load");
    };
    assert_eq!(bounds_field(&err), Some("fst_len"), "{err}");
}

#[test]
fn test_lookup_returns_the_words_of_a_key_in_ranking_order() {
    let lexicon = fixture("ranking");
    let words: Vec<WordRef<'_>> = lexicon.lookup("ni").expect("looking up ni").collect();
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].text, "你");
    assert_eq!(words[0].weight, 250_000);
    assert_eq!(words[0].syl_count, 1);
    assert!(words[0].flags.is_empty());
    assert_eq!(words[1].text, "泥");
    assert_eq!(words[1].weight, 900);
    assert!(words[1].weight <= words[0].weight, "heavier first");

    let looked = lexicon.lookup("zhong'guo");
    let single: Vec<WordRef<'_>> = looked.expect("looking up zhong'guo").collect();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].text, "中国");
    assert_eq!(single[0].weight, 45_000);
    assert_eq!(single[0].syl_count, 2);
}

#[test]
fn test_lookup_reads_the_flags_of_an_entry() {
    let lexicon = fixture("flags");
    let words: Vec<WordRef<'_>> = lexicon.lookup("yin'hang").expect("lookup").collect();
    assert_eq!(words.len(), 1);
    assert!(words[0].flags.contains(WordFlags::TERM));
    assert!(!words[0].flags.contains(WordFlags::SURNAME));
}

#[test]
fn test_lookup_returns_nothing_for_a_key_that_is_not_in_the_index() {
    let lexicon = fixture("miss");
    let miss = |key: &str| lexicon.lookup(key).expect("a miss is not an error");
    assert_eq!(miss("meiyou").count(), 0);
    assert_eq!(miss("").count(), 0);
    // The index holds whole keys: a prefix of a stored key is still a miss.
    assert_eq!(miss("ni'h").count(), 0);
}

#[test]
fn test_lookup_finds_one_word_under_every_key_of_a_polyphone_word() {
    let scratch = ScratchDict::new(&fixture_image(None), "polyphone");
    let loaded = FstLexicon::load(scratch.path());
    let lexicon = loaded.expect("loading the fixture");
    let hang: Vec<WordRef<'_>> = lexicon.lookup("yin'hang").expect("hang").collect();
    let xing: Vec<WordRef<'_>> = lexicon.lookup("yin'xing").expect("xing").collect();
    assert_eq!(hang.len(), 1);
    assert_eq!(xing.len(), 1);
    assert_eq!(hang[0].text, xing[0].text);
    assert_eq!(hang[0].weight, xing[0].weight);
    assert_eq!(hang[0].syl_count, xing[0].syl_count);
    assert_eq!(hang[0].weight, 5_500);

    // Both keys must reach the *same* record, not two records that happen to agree: the
    // word list is the indirection that lets one entry answer to several keys, and a
    // compiler that emitted a second entry per key would double the entry table. The
    // identity is read back from the container rather than inferred from the words,
    // because two words agreeing on every visible field is exactly what a duplicate
    // record would look like.
    let image = fs::read(scratch.path()).expect("reading the fixture");
    let reader = Reader::parse(image, Verify::Full).expect("parsing the fixture");
    let map = reader.fst_map().expect("the fixture has an index");
    let id_of = |key: &str| {
        let packed = map.get(key.as_bytes()).expect("the fixture holds this key");
        let (start, _) = unpack_fst_value(packed);
        reader
            .word_list_id(usize::try_from(start).expect("a small fixture"))
            .expect("the word list holds this id")
    };
    assert_eq!(id_of("yin'hang"), id_of("yin'xing"));
    let record = reader.entry(id_of("yin'hang")).expect("the record");
    assert_eq!(reader.word(&record).expect("the text"), "银行");
    assert_eq!(record.weight, 5_500);
}

#[test]
fn test_fallback_single_reads_the_bare_syllable_key() {
    let lexicon = fixture("fallback");
    let ni = ime_core::segment::lookup("ni");
    let ni = ni.expect("ni is in the syllable table");

    let served = lexicon.fallback_single(ni, 8);
    let words: Vec<WordRef<'_>> = served.expect("falling back").collect();
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].text, "你");
    assert_eq!(words[1].text, "泥");

    let served = lexicon.fallback_single(ni, 1);
    let limited: Vec<WordRef<'_>> = served.expect("limit").collect();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].text, "你");

    let served = lexicon.fallback_single(ni, 0);
    let empty: Vec<WordRef<'_>> = served.expect("zero limit").collect();
    assert!(empty.is_empty(), "a zero limit yields no candidate");
}

#[test]
fn test_fallback_single_rejects_an_identifier_outside_the_syllable_table() {
    let lexicon = fixture("bad-syllable");
    let outside = SyllableId::new(4_000);
    assert!(matches!(
        lexicon.fallback_single(outside, 4),
        Err(ImeError::Unsupported)
    ));
}

#[test]
fn test_lookup_reports_a_value_that_leaves_its_section() {
    // Three levels, three ways to leave the section: the packed word-list range, the word
    // id inside it, and the record's text range. The range case comes in both shapes --
    // a start past the section, and a start inside it whose count runs past the end --
    // because a check that only compared the start against the length would accept the
    // second one.
    let cases = [
        (Broken::ListRange, "ni", "wordlist_start"),
        (Broken::ListOverrun, "ni", "wordlist_start"),
        (Broken::WordId, "ni", "entry_index"),
        (Broken::Text, "zhong", "word_off"),
    ];
    for (broken, key, field) in cases {
        let scratch = ScratchDict::new(&fixture_image(Some(broken)), "broken");
        let loaded = FstLexicon::load(scratch.path());
        let lexicon = loaded.expect("the crafted container loads");
        let Err(err) = lexicon.lookup(key) else {
            panic!("{broken:?}: a value out of range must not be served");
        };
        match &err {
            ImeError::DictUnavailable { cause, .. } => {
                assert_eq!(bounds_field(cause), Some(field), "{broken:?}");
            }
            other => panic!("{broken:?}: expected a dictionary failure, got {other:?}"),
        }
    }
}

#[test]
fn test_lookup_records_a_malformed_record_once() {
    // The record reached by `zhong` points past the end of the string pool. Every lookup
    // fails, and the diagnostics the lexicon holds stay at one entry for it: a damaged
    // dictionary must not turn into one log record per keystroke.
    let scratch = ScratchDict::new(&fixture_image(Some(Broken::Text)), "malformed");
    let lexicon = FstLexicon::load(scratch.path()).expect("the crafted container loads");
    for _ in 0..100 {
        assert!(
            lexicon.lookup("zhong").is_err(),
            "the record stays broken on every access"
        );
    }
    let recorded = lexicon.malformed_entries();
    assert_eq!(recorded.len(), 1, "one broken range, one diagnostic");
    assert_eq!(recorded[0].word_off, 9_999);
    assert_eq!(recorded[0].word_len, 3);
}

#[test]
fn test_unigram_at_returns_the_record_and_checks_its_bounds() {
    let lexicon = fixture("unigram");
    let count = lexicon.entry_count();
    let mut found = None;
    for index in 0..count {
        let record = lexicon.unigram_at(index);
        let (hash, prob) = record.expect("reading the table");
        if hash == hash_word("中国") {
            found = Some(prob);
        }
    }
    assert_eq!(found, Some(PROB_Q12_MAX), "中国 is in the unigram table");
    let past = lexicon.unigram_at(count);
    let Err(err) = past else {
        panic!("an index past the table must be reported");
    };
    assert_eq!(bounds_field(&err), Some("unigram_index"), "{err}");
}

#[test]
fn test_lookup_survives_deleting_the_dictionary_file() {
    let scratch = ScratchDict::new(&fixture_image(None), "deleted");
    let loaded = FstLexicon::load(scratch.path());
    let lexicon = loaded.expect("loading the fixture");
    fs::remove_file(scratch.path()).expect("deleting the dictionary");
    // The mapping holds the inode the unlink removed, so the data stays readable: a
    // dictionary replaced on disk does not disturb a running session.
    let served = lexicon.lookup("zhong'guo");
    let words: Vec<WordRef<'_>> = served.expect("looking up").collect();
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "中国");
}

#[test]
fn test_dict_lm_scores_every_word_the_unigram_table_holds() {
    let lexicon = fixture("dict-lm-hit");
    assert_eq!(lexicon.entry_count() as usize, FIXTURE_WORDS.len());
    let model = DictLm::new(&lexicon);
    // The fixture writes `PROB_Q12_MAX` for every record, so every word scores
    // `log2(1.0)`, which is the one score a test can state without recomputing the table.
    for (text, ..) in FIXTURE_WORDS {
        assert_eq!(model.unigram(text), 0, "{text}");
    }
}

#[test]
fn test_dict_lm_answers_the_miss_floor_for_a_word_the_table_lacks() {
    let lexicon = fixture("dict-lm-miss");
    let model = DictLm::new(&lexicon);
    assert_eq!(model.unigram("没有"), UNIGRAM_MISS);
    assert_eq!(model.unigram(""), UNIGRAM_MISS);
    // A prefix of a word the table holds is a word of its own, and is a miss.
    assert_eq!(model.unigram("银"), UNIGRAM_MISS);
}

#[test]
fn test_dict_lm_bigram_falls_back_to_the_unigram_minus_the_miss_penalty() {
    let lexicon = fixture("dict-lm-bigram");
    let model = DictLm::new(&lexicon);
    // Version 1 carries no bigram section, so every pair takes the miss path -- and the
    // path is the documented one, not the bare penalty: the word's own unigram minus the
    // constant, which is what the in-memory model answers for a pair it was never given.
    assert_eq!(model.bigram("你", "中国"), BIGRAM_MISS_PENALTY);
    assert_eq!(
        model.bigram("你", "没有"),
        UNIGRAM_MISS + BIGRAM_MISS_PENALTY
    );
}

#[test]
fn test_dict_lm_bigram_hit_matches_the_in_memory_model_and_ranking() {
    let hit_score = log2_q8(u32::from(3_u16 * 1024), u32::from(PROB_Q12_MAX));
    let image = fixture_image_with_bigrams(None, &[("你", "中国", 3 * 1024)]);
    let scratch = ScratchDict::new(&image, "dict-lm-bigram-hit");
    let loaded = FstLexicon::load(scratch.path());
    let lexicon = loaded.expect("loading the v2 fixture");
    let dictionary = DictLm::new(&lexicon);
    let mut memory = InMemoryLm::new();
    for (text, ..) in FIXTURE_WORDS {
        memory.insert_unigram(text, 0);
    }
    memory.insert_bigram("你", "中国", hit_score);

    assert_eq!(lexicon.bigram_section().len(), BIGRAM_ENTRY_SIZE);
    assert_eq!(dictionary.bigram("你", "中国"), hit_score);
    for prev in ["你", "中", "没有"] {
        for word in FIXTURE_WORDS.iter().map(|(text, ..)| *text).chain(["没有"]) {
            assert_eq!(
                dictionary.bigram(prev, word),
                memory.bigram(prev, word),
                "bigram({prev:?}, {word:?})"
            );
        }
    }

    let decoder = Decoder::default();
    for (key, _) in FIXTURE_KEYS {
        let raw = key.replace('\'', "");
        let request = DecodeRequest::new(raw.as_str());
        let served = decoder.decode(&request, &lexicon, &NoUserFreq, &dictionary);
        let expected = decoder.decode(&request, &lexicon, &NoUserFreq, &memory);
        assert_eq!(served, expected, "{key}");
    }
}

#[test]
fn test_dict_lm_debug_reports_bigrams_without_word_text() {
    let image = fixture_image_with_bigrams(
        None,
        &[("你", "中国", 3 * 1024), ("你", "银行", PROB_Q12_MAX)],
    );
    let scratch = ScratchDict::new(&image, "dict-lm-bigram-debug");
    let loaded = FstLexicon::load(scratch.path());
    let model = DictLm::new(&loaded.expect("loading the v2 fixture"));
    let rendered = format!("{model:?}");

    assert!(rendered.contains("bigram_entries: 2"), "{rendered}");
    assert!(!rendered.contains("中国"), "{rendered}");
}

#[test]
fn test_dict_lm_v1_has_no_bigram_section_and_preserves_the_miss_path() {
    let lexicon = fixture("dict-lm-v1-bigram-absent");
    let model = DictLm::new(&lexicon);

    assert!(lexicon.bigram_section().is_empty());
    assert!(format!("{model:?}").contains("bigram_entries: 0"));
    assert_eq!(model.bigram("你", "中国"), BIGRAM_MISS_PENALTY);
}

#[test]
fn test_dict_lm_agrees_with_the_in_memory_model_over_the_same_words() {
    let lexicon = fixture("dict-lm-equivalence");
    let dictionary = DictLm::new(&lexicon);
    // The same data in the shape the engine is tested with. The dictionary model is the
    // production one and the in-memory model is the double, so the two answering alike
    // for every word is what lets a test written against the double keep its meaning
    // when the dictionary model is assembled in its place.
    let mut memory = InMemoryLm::new();
    for (text, ..) in FIXTURE_WORDS {
        memory.insert_unigram(text, 0);
    }
    let mut words: Vec<&str> = FIXTURE_WORDS.iter().map(|(text, ..)| *text).collect();
    words.extend(["没有", "", "银", "你泥", "中国银行"]);
    for word in &words {
        assert_eq!(
            dictionary.unigram(word),
            memory.unigram(word),
            "unigram({word:?})"
        );
    }
    for prev in ["你", "中", "没有"] {
        for word in &words {
            assert_eq!(
                dictionary.bigram(prev, word),
                memory.bigram(prev, word),
                "bigram({prev:?}, {word:?})"
            );
        }
    }
}

#[test]
fn test_dict_lm_over_a_container_with_no_entries_misses_every_word() {
    // A container whose index holds a key with no words: the unigram table has no
    // records, and a lookup answers the floor rather than walking an empty section.
    let mut index = fst::MapBuilder::memory();
    let packed = pack_fst_value(0, 0).expect("an empty range packs");
    index
        .insert("ni".as_bytes(), packed)
        .expect("inserting the key");
    let mut container = writer::DictWriter::new();
    container
        .add_section(
            SectionKind::Fst,
            index.into_inner().expect("finishing the index"),
        )
        .expect("adding the index");
    let image = container.encode().expect("encoding the container");
    let scratch = ScratchDict::new(&image, "dict-lm-empty");
    let lexicon = FstLexicon::load(scratch.path()).expect("loading the container");
    let model = DictLm::new(&lexicon);
    assert_eq!(model.unigram("你"), UNIGRAM_MISS);
    assert_eq!(model.bigram("你", "你"), UNIGRAM_MISS + BIGRAM_MISS_PENALTY);
}

#[test]
fn test_dict_lm_ranks_the_fixture_exactly_like_the_in_memory_model() {
    // The property the two models are interchangeable for, asserted where a user would
    // feel it: a whole decode through each, over every key the fixture holds, has to
    // produce the same candidates in the same order. The in-memory model is given the
    // same scores the container carries, so a difference can only come from the model.
    let lexicon = fixture("dict-lm-ranking");
    let dictionary = DictLm::new(&lexicon);
    let mut memory = InMemoryLm::new();
    for (text, ..) in FIXTURE_WORDS {
        memory.insert_unigram(text, 0);
    }
    let decoder = Decoder::default();
    for (key, _) in FIXTURE_KEYS {
        let raw = key.replace('\'', "");
        let request = DecodeRequest::new(raw.as_str());
        let served = decoder.decode(&request, &lexicon, &NoUserFreq, &dictionary);
        let expected = decoder.decode(&request, &lexicon, &NoUserFreq, &memory);
        assert_eq!(served, expected, "{key}");
    }
}

/// A user-frequency source that has recorded nothing.
///
/// The engine's own double lives in `ime-core`'s test tree, which is not compiled into
/// this crate; the decode path needs one either way, and a source that answers zero is
/// what a decode sees before the first commit.
struct NoUserFreq;

impl ime_types::UserFreqSource for NoUserFreq {
    fn freq(&self, _key: &str) -> u32 {
        0
    }
    fn record(&self, _key: &str, _weight_hint: u16) {}
    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// Cross-checks the lexicon against the reader over the development dictionary.
///
/// `data/compiled/base.dict` is generated by `xtask dictc` and is not committed, so this
/// test is ignored by default; run it with `--run-ignored` after building the dictionary.
/// It walks two independent paths over the same file -- the lexicon's FST to word-list to
/// entry indirection, and the reader's own `key_words` -- for every key the FST holds, and
/// checks what the word list does not carry: the syllable count against the key and the
/// weights against the ranking order.
#[test]
#[ignore = "needs data/compiled/base.dict, which xtask dictc generates"]
fn test_lookup_matches_the_reader_over_the_development_dictionary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("../../data/compiled/base.dict");
    if !path.exists() {
        // The dictionary is generated, not committed, so a checkout without it
        // skips this cross-check instead of failing it.
        return;
    }
    let loaded = FstLexicon::load(&path);
    let lexicon = loaded.expect("loading the development dictionary");
    let image = fs::read(&path).expect("reading the development dictionary");
    let parsed = Reader::parse(image, Verify::Full);
    let reader = parsed.expect("parsing the dictionary");
    let map = reader.fst_map().expect("the dictionary has an index");

    let mut keys = 0usize;
    let mut stream = map.stream();
    while let Some((key, _)) = stream.next() {
        let key = std::str::from_utf8(key);
        let key = key.expect("the index holds UTF-8 keys");
        let served = lexicon.lookup(key);
        let served = served.expect("the lexicon serves the key");
        let words: Vec<WordRef<'_>> = served.collect();
        let found: Vec<&str> = words.iter().map(|word| word.text).collect();
        let expected = reader.key_words(key);
        let expected = expected.expect("the reader serves the key");
        assert_eq!(found, expected, "key {key:?}");

        let syllables = key.matches('\'').count() as u8 + 1;
        for word in &words {
            assert_eq!(word.syl_count, syllables, "key {key:?}");
        }
        for pair in words.windows(2) {
            assert!(pair[0].weight >= pair[1].weight, "out of order");
        }
        keys += 1;
    }
    assert!(keys > 1_000, "found only {keys} keys");
}

#[test]
fn test_prefix_streams_the_words_of_every_key_in_the_range() {
    let lexicon = fixture("prefix-range");
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("zhong", 10)
        .expect("streaming the zhong range")
        .collect();
    assert_eq!(
        texts(&words),
        ["中", "中国", "中心"],
        "the range holds every key that starts with the prefix, in key order"
    );
    assert_eq!(words[0].weight, 200_000, "a key's own order is preserved");
}

#[test]
fn test_prefix_of_an_exact_key_answers_that_key_alone() {
    let lexicon = fixture("prefix-exact");
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("ni'hao", 10)
        .expect("streaming the ni'hao range")
        .collect();
    assert_eq!(texts(&words), ["你好"]);
}

#[test]
fn test_prefix_stops_at_the_first_key_outside_the_range() {
    let lexicon = fixture("prefix-stop");
    // `zhong'gu` names `zhong'guo` and nothing longer: the next key in order,
    // `zhong'xin`, does not start with the prefix, and lexicographic order says
    // nothing after it can.
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("zhong'gu", 10)
        .expect("streaming a range of one")
        .collect();
    assert_eq!(texts(&words), ["中国"]);

    // A prefix below every key of its block still finds the block.
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("yin", 10)
        .expect("streaming the yin range")
        .collect();
    assert_eq!(texts(&words), ["银行", "银行"], "both readings of the word");
}

#[test]
fn test_prefix_pushes_the_limit_down_into_the_walk() {
    let lexicon = fixture("prefix-limit");
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("zhong", 2)
        .expect("streaming past the limit")
        .collect();
    assert_eq!(
        texts(&words),
        ["中", "中国"],
        "the walk stops when the limit is spent, not when the range is"
    );

    // A limit of zero is an empty answer that never touches the index.
    let none: Vec<WordRef<'_>> = lexicon.prefix("zhong", 0).expect("a zero limit").collect();
    assert!(none.is_empty());
}

#[test]
fn test_prefix_of_the_empty_string_is_the_whole_map_under_the_limit() {
    let lexicon = fixture("prefix-empty");
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("", 3)
        .expect("streaming from the top")
        .collect();
    assert_eq!(
        texts(&words),
        ["你", "泥", "你好"],
        "the first keys in order, cut at the limit"
    );
}

#[test]
fn test_prefix_of_a_prefix_no_key_starts_with_answers_nothing() {
    let lexicon = fixture("prefix-miss");
    let words: Vec<WordRef<'_>> = lexicon
        .prefix("abc", 10)
        .expect("a miss is an empty answer, not a failure")
        .collect();
    assert!(words.is_empty());
}

/// The texts of the words a read produced, in the order they arrived.
fn texts<'a>(words: &[WordRef<'a>]) -> Vec<&'a str> {
    words.iter().map(|word| word.text).collect()
}
