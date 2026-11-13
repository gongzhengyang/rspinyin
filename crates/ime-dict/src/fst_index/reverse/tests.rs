//! Tests for the lexicon's reverse view: the index a container carries is the
//! index the lexicon answers through, and the answer is byte for byte the
//! reader's own.

use ime_core::reverse::{ReverseHit, ReverseSource};
use ime_types::DictError;

use crate::format::reader::{Reader, Verify};
use crate::format::writer::DictWriter;
use crate::format::{DictEntry, SectionKind, hash_word, pack_fst_value};
use crate::reverse::{IndexEntry, ReverseIndex, build_index};

use super::FstLexicon;

/// The hand-written decompositions the fixture compiles: three characters whose
/// codes share the letter `h`, so one query answers all of them and the answer's
/// order is the container's own codepoint order (`中` < `房` < `放`).
const DECOMPOSITIONS: [(char, &str); 3] = [('中', "zh"), ('房', "hf"), ('放', "fh")];

/// The compiled fixture index.
fn decomposition_index() -> ReverseIndex {
    let entries: Vec<IndexEntry<'_>> = DECOMPOSITIONS
        .iter()
        .map(|&(character, code)| IndexEntry { character, code })
        .collect();
    build_index(&entries).expect("building the fixture index")
}

/// A two-entry container (`中`, `中国`) with the reverse index added.
fn reverse_container() -> Vec<u8> {
    let mut writer = sample_writer();
    writer
        .add_reverse_index(decomposition_index().bytes)
        .expect("adding the reverse index");
    writer.encode().expect("encoding")
}

/// The two-entry sample writer: `中` and `中国`, no tail payload.
///
/// Entries, strpool and unigram rows all follow the builder's hash-ascending order
/// (`中国`'s hash sorts before `中`'s): a `word_id` binary-searches the hash table
/// and reads the entry text back at the same index, so record 0 is `中国` and
/// record 1 is `中`. The FST section is a real index -- the lexicon builds its FST
/// eagerly at load, so a stub the reader never interprets is not valid here.
fn sample_writer() -> DictWriter {
    let mut index = fst::MapBuilder::memory();
    let packed = pack_fst_value(0, 1).expect("packing the word-list range");
    index
        .insert("zhong".as_bytes(), packed)
        .expect("inserting the key");

    let mut entries = Vec::new();
    entries.extend_from_slice(&DictEntry::new(0, 6, 2, 0, 300).encode());
    entries.extend_from_slice(&DictEntry::new(6, 3, 1, 0, 900).encode());

    let mut wordlist = Vec::new();
    wordlist.extend_from_slice(&0u32.to_le_bytes());
    wordlist.extend_from_slice(&1u32.to_le_bytes());

    let mut unigram = Vec::new();
    unigram.extend_from_slice(&hash_word("中国").to_le_bytes());
    unigram.extend_from_slice(&1u16.to_le_bytes());
    unigram.extend_from_slice(&0u16.to_le_bytes());
    unigram.extend_from_slice(&hash_word("中").to_le_bytes());
    unigram.extend_from_slice(&4096u16.to_le_bytes());
    unigram.extend_from_slice(&0u16.to_le_bytes());

    let mut writer = DictWriter::new();
    writer
        .add_section(
            SectionKind::Fst,
            index.into_inner().expect("finishing the index"),
        )
        .expect("fst");
    writer
        .add_section(SectionKind::Entries, entries)
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, "中国中".as_bytes().to_vec())
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, unigram)
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, wordlist)
        .expect("wordlist");
    writer
}

/// Returns the field name of a bounds failure, or `None` for any other error.
fn bounds_field(err: &DictError) -> Option<&'static str> {
    match err {
        DictError::LengthOutOfRange { field, .. } => Some(*field),
        _ => None,
    }
}

#[test]
fn test_from_image_answers_the_index_a_container_carries() {
    let image: &'static [u8] = Box::leak(reverse_container().into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let source: &dyn ReverseSource = &lexicon;
    assert!(source.has_reverse_index());
    // An exact code matches its own character, with the full code as the
    // annotation a candidate would show.
    assert_eq!(
        source.reverse_lookup("hf").expect("lookup"),
        vec![ReverseHit {
            character: '房',
            code: "hf"
        }]
    );
    // A shared letter answers with every carrier in container order, which is
    // ascending codepoint order here.
    assert_eq!(
        source.reverse_lookup("h").expect("lookup"),
        vec![
            ReverseHit {
                character: '中',
                code: "zh"
            },
            ReverseHit {
                character: '房',
                code: "hf"
            },
            ReverseHit {
                character: '放',
                code: "fh"
            },
        ]
    );
    // A well-shaped payload no key names reads as an empty list, and so does one
    // the domain would not accept: neither is an error.
    assert!(source.reverse_lookup("q").expect("lookup").is_empty());
    assert!(source.reverse_lookup(" absent").expect("lookup").is_empty());
}

#[test]
fn test_from_image_matches_the_reader_query_for_query() {
    // The image the lexicon loaded is the image the reader parses, so the one
    // equivalence that matters is direct: per payload, the trait object's answer
    // and the reader's are the same value. Delegation makes this hold by
    // construction; the assert is what notices if the delegation ever stops
    // being one.
    let image: &'static [u8] = Box::leak(reverse_container().into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    for payload in ["h", "hf", "q"] {
        assert_eq!(
            lexicon.reverse_lookup(payload).expect("lexicon lookup"),
            reader.reverse_lookup(payload).expect("reader lookup"),
            "the lexicon answers what the reader answers"
        );
    }
}

#[test]
fn test_from_image_reports_the_absent_index() {
    let encoded = sample_writer().encode().expect("encoding");
    let image: &'static [u8] = Box::leak(encoded.into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let source: &dyn ReverseSource = &lexicon;
    assert!(!source.has_reverse_index());
    let error = source.reverse_lookup("hf").expect_err("absent index");
    assert_eq!(bounds_field(&error), Some("reverse_absent"));
}
