//! Tests for [`super`].
//!
//! Split out of `reader.rs` to keep that file inside the line limit. They hold the
//! untrusted-input rule the reader exists to enforce: magic, version, lengths and CRC
//! are checked before any offset is trusted, and every malformed case arrives as a
//! typed refusal rather than a read out of bounds.

use super::*;
use crate::format::writer::DictWriter;

/// Builds a small but complete container: three words, three keys.
///
/// Word ids follow hash order, exactly as the compiler assigns them, so the
/// unigram table is sorted by hash and `ENTRIES` and `UNIGRAM` share an index.
fn sample_image() -> Vec<u8> {
    let mut words: Vec<&str> = vec!["中", "中国", "中心"];
    words.sort_by_key(|word| (hash_word(word), *word));
    let id_of = |needle: &str| {
        words
            .iter()
            .position(|word| *word == needle)
            .expect("the fixture holds this word") as u32
    };

    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let offset = strpool.len() as u32;
        strpool.extend_from_slice(word.as_bytes());
        let syllables = if word.chars().count() == 1 { 1 } else { 2 };
        entries.extend_from_slice(
            &DictEntry::new(offset, word.len() as u16, syllables, 0, 100 - index as u32).encode(),
        );
        unigram.extend_from_slice(&hash_word(word).to_le_bytes());
        unigram.extend_from_slice(&format::PROB_Q12_MAX.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }

    // Keys in byte order, each range pointing at its words in the word list.
    let all: Vec<u32> = (0..words.len() as u32).collect();
    let keys: [(&str, &[u32]); 3] = [
        ("zhong", &all),
        ("zhong'guo", &[id_of("中国")]),
        ("zhong'xin", &[id_of("中心")]),
    ];
    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, ids) in keys {
        let start = (wordlist.len() / 4) as u64;
        for id in ids {
            wordlist.extend_from_slice(&id.to_le_bytes());
        }
        fst_builder
            .insert(
                key,
                format::pack_fst_value(start, ids.len() as u32).expect("packing"),
            )
            .expect("inserting");
    }
    let fst_bytes = fst_builder.into_inner().expect("finishing the FST");

    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst_bytes)
        .expect("fst");
    writer
        .add_section(SectionKind::Entries, entries)
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, strpool)
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, unigram)
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, wordlist)
        .expect("wordlist");
    writer.encode().expect("encoding")
}

/// Returns the word id the reader stores under `word`, by scanning the table.
fn id_of(reader: &Reader<Vec<u8>>, word: &str) -> u32 {
    for index in 0..reader.entry_count() {
        if reader.word_at(index).expect("word") == word {
            return index;
        }
    }
    panic!("{word} is not in the container");
}

#[test]
fn test_round_trip_reads_back_every_section_byte_for_byte() {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, vec![7u8; 24])
        .expect("fst");
    writer
        .add_section(SectionKind::StrPool, b"abcdefghij".to_vec())
        .expect("strpool");
    writer
        .add_section(SectionKind::WordList, vec![0u8; 8])
        .expect("wordlist");
    let image = writer.encode().expect("encoding");

    let reader = Reader::parse(image.clone(), Verify::Full).expect("parsing");
    for kind in [
        SectionKind::Fst,
        SectionKind::StrPool,
        SectionKind::WordList,
        SectionKind::Entries,
        SectionKind::Unigram,
        SectionKind::Bigram,
    ] {
        let expected = writer.section(kind).unwrap_or_default();
        assert_eq!(
            reader.section(kind).expect("section"),
            expected,
            "{kind:?} must round-trip byte for byte"
        );
    }
}

#[test]
fn test_parse_reads_entries_words_and_keys() {
    let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
    assert_eq!(reader.entry_count(), 3);

    let single = reader.entry(id_of(&reader, "中")).expect("entry");
    assert_eq!(reader.word(&single).expect("word"), "中");
    assert_eq!(single.syl_count, 1);
    let compound = reader.entry(id_of(&reader, "中国")).expect("entry");
    assert_eq!(reader.word(&compound).expect("word"), "中国");
    assert_eq!(compound.syl_count, 2);
    assert_eq!(compound.word_len as usize, "中国".len());

    let words = reader.key_words("zhong'guo").expect("lookup");
    assert_eq!(words, vec!["中国"]);
    let all = reader.key_words("zhong").expect("lookup");
    assert_eq!(all.len(), 3);
    assert!(reader.key_words("meiyou").expect("missing key").is_empty());
}

#[test]
fn test_unigram_lookup_finds_words_by_hash() {
    let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
    let found = reader.unigram_lookup("中心").expect("lookup");
    assert_eq!(found, Some((hash_word("中心"), format::PROB_Q12_MAX)));
    assert_eq!(reader.unigram_lookup("不存在").expect("miss"), None);
}

#[test]
fn test_parse_rejects_a_corrupted_magic() {
    let mut image = sample_image();
    image[0] = b'X';
    assert!(matches!(
        Reader::parse(image, Verify::Full),
        Err(DictError::MagicMismatch)
    ));
}

#[test]
fn test_parse_rejects_an_unsupported_format_version() {
    let mut image = sample_image();
    image[4..6].copy_from_slice(&3u16.to_le_bytes());
    assert!(matches!(
        Reader::parse(image, Verify::Full),
        Err(DictError::FormatVersion { found: 3 })
    ));
}

#[test]
fn test_parse_rejects_a_bigram_section_in_a_version_one_container() {
    let mut image = sample_image();
    image[4..6].copy_from_slice(&1u16.to_le_bytes());
    image[6..8].copy_from_slice(&FLAG_BIGRAM_PRESENT.to_le_bytes());
    assert!(matches!(
        Reader::parse(image, Verify::Full),
        Err(DictError::LengthOutOfRange {
            field: "bigram_flags",
            value: 1,
        })
    ));
}

#[test]
fn test_parse_rejects_a_corrupted_section_byte() {
    for kind in [
        SectionKind::Fst,
        SectionKind::Entries,
        SectionKind::StrPool,
        SectionKind::Unigram,
        SectionKind::WordList,
    ] {
        let mut image = sample_image();
        let entry = parse_section_table(&image)
            .expect("table")
            .iter()
            .copied()
            .find(|entry| entry.kind == kind)
            .expect("section present");
        assert!(
            !entry.is_absent(),
            "{kind:?}: an absent section has no byte to corrupt"
        );
        let at = entry.offset as usize + entry.len as usize - 1;
        image[at] ^= 0xFF;
        assert!(
            matches!(
                Reader::parse(image, Verify::Full),
                Err(DictError::Crc { .. })
            ),
            "{kind:?}: a flipped byte must fail the checksum"
        );
    }
}

#[test]
fn test_parse_rejects_a_corrupted_section_table() {
    let mut image = sample_image();
    // Point the FST section at the header: inside the file, but not a section.
    image[HEADER_SIZE + 8..HEADER_SIZE + 16].copy_from_slice(&8u64.to_le_bytes());
    assert!(matches!(
        Reader::parse(image, Verify::Full),
        Err(DictError::LengthOutOfRange {
            field: "section_offset",
            ..
        })
    ));
}

#[test]
fn test_parse_rejects_an_entry_count_that_disagrees_with_the_table() {
    let mut image = sample_image();
    image[28..32].copy_from_slice(&7u32.to_le_bytes());
    assert!(matches!(
        Reader::parse(image, Verify::Full),
        Err(DictError::LengthOutOfRange {
            field: "entries_len",
            ..
        })
    ));
}

#[test]
fn test_parse_rejects_a_truncated_file() {
    let image = sample_image();
    let short = image[..image.len() - 8].to_vec();
    assert!(matches!(
        Reader::parse(short, Verify::Full),
        Err(DictError::LengthOutOfRange { .. })
    ));
    let empty: Vec<u8> = Vec::new();
    assert!(matches!(
        Reader::parse(empty, Verify::Full),
        Err(DictError::LengthOutOfRange {
            field: "file_len",
            ..
        })
    ));
}

#[test]
fn test_header_only_verification_skips_checksums_but_keeps_structure() {
    let mut image = sample_image();
    image[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    assert!(
        Reader::parse(image.clone(), Verify::Full).is_err(),
        "full verification must reject the wrong section checksum"
    );
    let reader = Reader::parse(image, Verify::Header).expect("header-only parse");
    assert_eq!(reader.entry_count(), 3);
}

#[test]
fn test_accessors_reject_out_of_range_requests() {
    let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
    assert!(matches!(
        reader.entry(3),
        Err(DictError::LengthOutOfRange {
            field: "entry_index",
            ..
        })
    ));
    assert!(matches!(
        reader.unigram(99),
        Err(DictError::LengthOutOfRange {
            field: "unigram_index",
            ..
        })
    ));
    assert!(matches!(
        reader.word_list_id(9),
        Err(DictError::LengthOutOfRange {
            field: "wordlist_entry",
            ..
        })
    ));
    // The word list holds one entry per (key, word) pair, not one per word: a word
    // reachable under several keys appears several times, which is the whole reason
    // the section exists separately from the entry table.
    assert_eq!(reader.word_list_len().expect("word list length"), 5);

    // A record whose text range leaves the pool is reported, not read.
    let escaping = DictEntry::new(1_000, 3, 1, 0, 0);
    assert!(matches!(
        reader.word(&escaping),
        Err(DictError::LengthOutOfRange {
            field: "word_off",
            ..
        })
    ));
}

#[test]
fn test_word_rejects_a_range_that_is_not_utf8() {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::StrPool, vec![0xFF, 0xFE, 0xFD])
        .expect("strpool");
    writer
        .add_section(
            SectionKind::Entries,
            DictEntry::new(0, 3, 1, 0, 1).encode().to_vec(),
        )
        .expect("entries");
    writer
        .add_section(SectionKind::Unigram, vec![0u8; UNIGRAM_ENTRY_SIZE])
        .expect("unigram");
    let image = writer.encode().expect("encoding");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    assert!(matches!(
        reader.word_at(0),
        Err(DictError::LengthOutOfRange {
            field: "word_utf8",
            ..
        })
    ));
}

#[test]
fn test_fst_map_reports_an_absent_section() {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::StrPool, b"ab".to_vec())
        .expect("strpool");
    let image = writer.encode().expect("encoding");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    assert!(matches!(
        reader.fst_map(),
        Err(DictError::LengthOutOfRange {
            field: "fst_len",
            ..
        })
    ));
}

#[test]
fn test_open_reports_a_missing_file_as_an_io_error() {
    let path = std::env::temp_dir().join("rspinyin-dict-reader-does-not-exist.dict");
    let result = Reader::open(&path);
    assert!(
        matches!(result, Err(DictError::Io(_))),
        "a missing file must surface as an IO error"
    );
}
