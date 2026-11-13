//! Tests for the lexicon's aux view: the column a container carries is the column
//! the lexicon answers through, and the answer is byte for byte the reader's own.

use ime_core::aux::AuxSource;
use ime_types::DictError;

use crate::format::reader::{Reader, Verify};
use crate::format::writer::DictWriter;
use crate::format::{AUX_ROW_SIZE, DictEntry, SectionKind, hash_word, pack_fst_value};

use super::FstLexicon;

/// A two-entry container (`中`, `中国`) with the AUX column added.
///
/// The fixture is the reader tests' own two-entry sample, with its FST stub replaced
/// by a real index: the lexicon builds its FST eagerly at load, so bytes the reader
/// never interprets are not valid here. The aux rows follow the record order the
/// writer produces (`中国` is record 0, `中` is record 1), so `中国` carries the
/// empty row and `中` carries `kg` -- one recorded and one unrecorded word, as the
/// assertions below rely on.
fn aux_container() -> Vec<u8> {
    writer_with_aux(vec![vec![0; AUX_ROW_SIZE], b"kg\0\0".to_vec()])
        .encode()
        .expect("encoding")
}

/// The two-entry sample writer, plus an aux payload.
fn writer_with_aux(rows: Vec<Vec<u8>>) -> DictWriter {
    let mut writer = sample_writer();
    let rows: Vec<u8> = rows.into_iter().flatten().collect();
    writer.add_aux_column(rows).expect("adding the aux column");
    writer
}

/// The two-entry sample writer: `中` and `中国`, no aux column.
///
/// Entries, strpool and unigram rows all follow the builder's hash-ascending order
/// (`中国`'s hash sorts before `中`'s): a `word_id` binary-searches the hash table
/// and reads the entry text back at the same index, so record 0 is `中国` and
/// record 1 is `中`.
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
fn test_from_image_answers_the_column_a_container_carries() {
    let image: &'static [u8] = Box::leak(aux_container().into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let source: &dyn AuxSource = &lexicon;
    assert!(source.has_aux_column());
    assert_eq!(source.aux_code("中").expect("lookup"), Some("kg"));
    assert_eq!(
        source.aux_code("中国").expect("lookup"),
        None,
        "an all-zero row reads as unrecorded"
    );
    assert_eq!(source.aux_code(" absent").expect("lookup"), None);
}

#[test]
fn test_from_image_matches_the_reader_word_for_word() {
    // The image the lexicon loaded is the image the reader parses, so the one
    // equivalence that matters is direct: per word, the trait object's answer and
    // the reader's are the same value. Delegation makes this hold by construction;
    // the assert is what notices if the delegation ever stops being one.
    let image: &'static [u8] = Box::leak(aux_container().into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    for word in ["中", "中国", " absent"] {
        assert_eq!(
            lexicon.aux_code(word).expect("lexicon lookup"),
            reader.aux_lookup(word).expect("reader lookup"),
            "the lexicon answers what the reader answers"
        );
    }
}

#[test]
fn test_from_image_reports_the_absent_column() {
    let encoded = sample_writer().encode().expect("encoding");
    let image: &'static [u8] = Box::leak(encoded.into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the image");
    let source: &dyn AuxSource = &lexicon;
    assert!(!source.has_aux_column());
    let error = source.aux_code("中").expect_err("absent column");
    assert_eq!(bounds_field(&error), Some("aux_absent"));
}
