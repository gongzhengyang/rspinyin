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

use ime_types::WordFlags;

use crate::format::{
    DictEntry, FLAG_TERM, PROB_Q12_MAX, hash_word, pack_fst_value,
    reader::{Reader, Verify},
    writer,
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
#[derive(Clone, Copy, Debug)]
enum Broken {
    /// `ni`'s packed value points past the end of the word list.
    WordListRange,
    /// `ni`'s word-list entry names a word id that does not exist.
    WordId,
    /// The record reached by `zhong` points past the end of the string pool.
    WordText,
}

/// Builds the fixture's container image.
///
/// The image is assembled with the public writer, so the bytes are what the compiler
/// produces and the test needs no data directory. `broken` names a field a test wants out
/// of contract; the checksums stay the writer's, so the loader accepts the container and a
/// failure has to come from the loader's own bounds checks, not from the checksum gate.
fn fixture_image(broken: Option<Broken>) -> Vec<u8> {
    let mut words: Vec<FixtureWord> = FIXTURE_WORDS.to_vec();
    let order = |(text, ..): &FixtureWord| (hash_word(text), *text);
    words.sort_by_key(order);
    let [entries, strpool, unigram] = word_sections(&words, broken);
    let [fst, wordlist] = index_sections(&words, broken);

    let mut writer = writer::DictWriter::new();
    for (kind, payload) in [
        (SectionKind::Fst, fst),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
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
            (Some(Broken::WordText), "中") => DictEntry::new(9_999, 3, 1, 0, *weight),
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
            Some(Broken::WordListRange) if key == "ni" => {
                let beyond = total_ids + 3;
                let beyond = u64::try_from(beyond).expect("small fixture");
                pack_fst_value(beyond, count).expect("packing")
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
    let lexicon = fixture("polyphone");
    let hang: Vec<WordRef<'_>> = lexicon.lookup("yin'hang").expect("hang").collect();
    let xing: Vec<WordRef<'_>> = lexicon.lookup("yin'xing").expect("xing").collect();
    assert_eq!(hang.len(), 1);
    assert_eq!(xing.len(), 1);
    assert_eq!(hang[0].text, xing[0].text);
    assert_eq!(hang[0].weight, xing[0].weight);
    assert_eq!(hang[0].syl_count, xing[0].syl_count);
    assert_eq!(hang[0].weight, 5_500);
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
fn test_prefix_reports_the_capability_as_not_yet_available() {
    let lexicon = fixture("prefix");
    assert!(matches!(
        lexicon.prefix("ni", 4),
        Err(ImeError::Unsupported)
    ));
}

#[test]
fn test_lookup_reports_a_value_that_leaves_its_section() {
    let cases = [
        (Broken::WordListRange, "ni", "wordlist_start"),
        (Broken::WordId, "ni", "entry_index"),
        (Broken::WordText, "zhong", "word_off"),
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
