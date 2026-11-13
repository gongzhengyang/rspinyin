//! Unit tests for the container writer: the section table, the flags word,
//! the atomic replacement, and the header they produce together.
//!
//! They live in a file of their own on the pattern of `reader/tests.rs`, and
//! they exercise the writer through its public entry points only.

use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::format::BigramRecord;
use crate::format::SECTION_ALIGN;
use crate::format::reader::{Reader, Verify};

/// Serial number for temporary directories; tests in one binary run in
/// parallel threads, and nextest runs several binaries at once.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A directory of this test process, unique per call and per test name.
fn scratch_dir(tag: &str) -> PathBuf {
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rspinyin-dict-writer-{}-{tag}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Two entries and a small string pool, enough to exercise every section.
fn sample_writer() -> DictWriter {
    let mut entries = Vec::new();
    entries.extend_from_slice(&format::DictEntry::new(0, 3, 1, 0, 900).encode());
    entries.extend_from_slice(&format::DictEntry::new(3, 6, 2, format::FLAG_PLACE, 300).encode());

    let mut wordlist = Vec::new();
    wordlist.extend_from_slice(&0u32.to_le_bytes());
    wordlist.extend_from_slice(&1u32.to_le_bytes());

    let mut unigram = Vec::new();
    unigram.extend_from_slice(&format::hash_word("中").to_le_bytes());
    unigram.extend_from_slice(&format::PROB_Q12_MAX.to_le_bytes());
    unigram.extend_from_slice(&0u16.to_le_bytes());
    unigram.extend_from_slice(&format::hash_word("中国").to_le_bytes());
    unigram.extend_from_slice(&1u16.to_le_bytes());
    unigram.extend_from_slice(&0u16.to_le_bytes());

    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, vec![0xAA; 16])
        .expect("fst");
    writer
        .add_section(SectionKind::Entries, entries)
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, "中中国".as_bytes().to_vec())
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, unigram)
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, wordlist)
        .expect("wordlist");
    writer
}

#[test]
fn test_encode_places_sections_on_alignment_boundaries() {
    let writer = sample_writer();
    let image = writer.encode().expect("encoding");
    let sections = format::parse_section_table(&image).expect("table");
    let mut previous_end = SECTIONS_OFFSET;
    for entry in sections.iter().filter(|entry| !entry.is_absent()) {
        assert_eq!(entry.offset % SECTION_ALIGN, 0, "{entry:?} is unaligned");
        assert!(
            entry.offset >= previous_end,
            "{entry:?} overlaps the previous section"
        );
        assert_eq!(
            entry.crc32,
            format::crc32(&image[entry.offset as usize..(entry.offset + entry.len) as usize])
        );
        previous_end = entry.offset + entry.len;
    }
    assert_eq!(previous_end, image.len() as u64);
}

#[test]
fn test_encode_writes_a_header_that_parses_back() {
    let writer = sample_writer();
    let image = writer.encode().expect("encoding");
    let header = format::parse_header(&image).expect("header");
    assert_eq!(header.format_version, FORMAT_VERSION);
    assert_eq!(header.entry_count, 2);
    assert_eq!(header.total_len, image.len() as u64);
    assert_eq!(header.flags, 0);
    assert_eq!(header.file_crc32, format::crc32(&image[HEADER_SIZE..]));
    assert!(image[32..HEADER_SIZE].iter().all(|byte| *byte == 0));
}

#[test]
fn test_encode_omits_empty_sections() {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::StrPool, Vec::new())
        .expect("empty section");
    writer
        .add_section(SectionKind::Fst, vec![1, 2, 3, 4, 5, 6, 7, 8])
        .expect("fst");
    let image = writer.encode().expect("encoding");
    let sections = format::parse_section_table(&image).expect("table");
    assert!(sections[2].is_absent(), "an empty payload is absent");
    assert_eq!(sections[2].offset, 0);
    assert!(!sections[0].is_absent());
    assert_eq!(writer.entry_count().expect("entry count"), 0);
}

#[test]
fn test_add_section_rejects_a_duplicate_kind() {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::StrPool, b"abc".to_vec())
        .expect("first");
    let result = writer.add_section(SectionKind::StrPool, b"def".to_vec());
    assert!(matches!(
        result,
        Err(DictError::LengthOutOfRange {
            field: "section_duplicate",
            ..
        })
    ));
}

#[test]
fn test_add_section_rejects_misaligned_record_payloads() {
    let cases = [
        (SectionKind::Entries, vec![0u8; ENTRY_SIZE + 1]),
        (
            SectionKind::Unigram,
            vec![0u8; format::UNIGRAM_ENTRY_SIZE + 1],
        ),
        (SectionKind::WordList, vec![0u8; 5]),
    ];
    for (kind, payload) in cases {
        let mut writer = DictWriter::new();
        let result = writer.add_section(kind, payload);
        assert!(
            matches!(
                result,
                Err(DictError::LengthOutOfRange {
                    field: "section_len",
                    ..
                })
            ),
            "{kind:?} with a partial record must be rejected"
        );
    }
}

#[test]
fn test_add_section_accepts_a_bigram_payload_and_rejects_a_partial_record() {
    let mut writer = DictWriter::new();
    writer
        .add_section(
            SectionKind::Bigram,
            BigramRecord::new(7, 9).encode().to_vec(),
        )
        .expect("a v2 container may carry a bigram section");
    assert_eq!(writer.flags(), FLAG_BIGRAM_PRESENT);
    let image = writer.encode().expect("encoding");
    let reader = crate::format::reader::Reader::parse(image, crate::format::reader::Verify::Full)
        .expect("the image is valid");
    assert_eq!(reader.header().format_version, FORMAT_VERSION);
    assert_eq!(reader.bigram_count(), 1);
    assert_eq!(reader.bigram(0).expect("record"), BigramRecord::new(7, 9));

    let mut writer = DictWriter::new();
    assert!(matches!(
        writer.add_section(SectionKind::Bigram, vec![1, 2, 3, 4]),
        Err(DictError::LengthOutOfRange {
            field: "section_len",
            ..
        })
    ));
    assert_eq!(writer.flags(), 0);
}

#[test]
fn test_finish_writes_a_readable_file_and_leaves_no_temporary() {
    let dir = scratch_dir("atomic");
    let path = dir.join("base.dict");
    let writer = sample_writer();
    writer.finish(&path).expect("finish");

    assert!(path.exists(), "the destination must exist");
    assert!(
        !temp_path(&path).exists(),
        "the temporary file must be gone after the rename"
    );
    let reader = Reader::open(&path).expect("opening the written file");
    assert_eq!(reader.entry_count(), 2);
    assert_eq!(reader.word_at(1).expect("second word"), "中国");
    assert_eq!(reader.section(SectionKind::Fst).expect("fst"), &[0xAA; 16]);
    assert_eq!(
        reader.unigram(0).expect("unigram"),
        (format::hash_word("中"), format::PROB_Q12_MAX)
    );

    fs::remove_dir_all(&dir).expect("cleaning up");
}

// A destination that exists as a non-empty directory is the deterministic way to
// fail the final rename after the temporary file has been written in full; a
// failing write (ENOSPC) leaves the same residue, and this test pins the cleanup
// that must run on every error path once the temporary exists.
#[test]
fn test_finish_removes_the_temporary_file_when_the_write_fails() {
    let dir = scratch_dir("failed-write");
    let path = dir.join("base.dict");
    fs::create_dir(&path).expect("the destination as a directory");
    fs::write(path.join("occupied"), b"rename onto a directory must fail")
        .expect("occupying the destination");

    let writer = sample_writer();
    let result = writer.finish(&path);
    assert!(result.is_err(), "rename onto a non-empty directory fails");
    assert!(
        !temp_path(&path).exists(),
        "a failed finish must not leave the temporary file behind"
    );
}

#[test]
fn test_finish_replaces_an_existing_file_atomically() {
    let dir = scratch_dir("replace");
    let path = dir.join("base.dict");

    let mut first = DictWriter::new();
    first
        .add_section(SectionKind::StrPool, b"first".to_vec())
        .expect("first section");
    first.finish(&path).expect("first finish");

    let mut second = DictWriter::new();
    second
        .add_section(SectionKind::StrPool, b"second payload".to_vec())
        .expect("second section");
    second.finish(&path).expect("second finish");

    let reader = Reader::open_with(&path, Verify::Full).expect("opening the replaced file");
    assert_eq!(
        reader.section(SectionKind::StrPool).expect("strpool"),
        b"second payload"
    );
    assert!(!temp_path(&path).exists());

    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_finish_creates_a_missing_parent_directory() {
    let dir = scratch_dir("parents");
    let path = dir.join("nested/deeper/base.dict");
    let writer = sample_writer();
    writer.finish(&path).expect("finish into a new directory");
    assert!(path.exists());
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_temp_path_keeps_the_file_in_its_directory() {
    let temp = temp_path(Path::new("/data/compiled/base.dict"));
    assert_eq!(temp, PathBuf::from("/data/compiled/base.dict.tmp"));
    assert_eq!(temp.parent(), Some(Path::new("/data/compiled")));
}

#[test]
fn test_align_up_rounds_to_the_section_boundary() {
    assert_eq!(align_up(208).expect("aligned"), 208);
    assert_eq!(align_up(209).expect("rounding"), 216);
    assert_eq!(align_up(215).expect("rounding"), 216);
    assert_eq!(align_up(216).expect("aligned"), 216);
}

/// A container image built by the writer, for the parser tests below: the
/// header and the section table are the writer's other half of the contract.
fn image() -> Vec<u8> {
    sample_writer().encode().expect("encoding")
}

#[test]
fn test_parse_header_rejects_magic_and_version() {
    let mut bytes = image();
    bytes[0] = b'X';
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::MagicMismatch)
    ));

    let mut bytes = image();
    bytes[4..6].copy_from_slice(&3u16.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::FormatVersion { found: 3 })
    ));

    let mut bytes = image();
    bytes[4..6].copy_from_slice(&0u16.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::FormatVersion { found: 0 })
    ));
}

#[test]
fn test_parse_header_rejects_layout_disagreements() {
    let mut bytes = image();
    bytes[8..12].copy_from_slice(&32u32.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "header_size",
            ..
        })
    ));

    let mut bytes = image();
    bytes[12..16].copy_from_slice(&5u32.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_count",
            ..
        })
    ));

    let mut bytes = image();
    let wrong = bytes.len() as u64 + 1;
    bytes[16..24].copy_from_slice(&wrong.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "total_len",
            ..
        })
    ));

    let mut bytes = image();
    // Every low bit is a legitimate flag now (bigram, aux, reverse), so the
    // probe flips a bit outside FLAG_MASK that no container version defined.
    bytes[6..8].copy_from_slice(&0x8000u16.to_le_bytes());
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::LengthOutOfRange { field: "flags", .. })
    ));

    let mut bytes = image();
    bytes[40] = 1;
    assert!(matches!(
        format::parse_header(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "reserved",
            ..
        })
    ));

    let short = vec![0u8; HEADER_SIZE - 1];
    assert!(matches!(
        format::parse_header(&short),
        Err(DictError::LengthOutOfRange {
            field: "file_len",
            ..
        })
    ));
}

#[test]
fn test_parse_section_table_rejects_an_unknown_kind() {
    let mut bytes = image();
    bytes[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&9u32.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_kind",
            ..
        })
    ));
}

#[test]
fn test_parse_section_table_rejects_a_misordered_table() {
    let mut bytes = image();
    // Swap the kinds of the first two entries; both are known values, so the
    // failure has to be caught by the ordering check.
    bytes[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&2u32.to_le_bytes());
    bytes[HEADER_SIZE + SECTION_ENTRY_SIZE..HEADER_SIZE + SECTION_ENTRY_SIZE + 4]
        .copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_order",
            ..
        })
    ));
}

#[test]
fn test_parse_section_table_rejects_unaligned_and_out_of_range_offsets() {
    let mut bytes = image();
    bytes[HEADER_SIZE + 8..HEADER_SIZE + 16].copy_from_slice(&209u64.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_offset",
            ..
        })
    ));

    let mut bytes = image();
    bytes[HEADER_SIZE + 8..HEADER_SIZE + 16].copy_from_slice(&64u64.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_offset",
            ..
        })
    ));

    let mut bytes = image();
    let total_len = bytes.len() as u64;
    bytes[HEADER_SIZE + 16..HEADER_SIZE + 24].copy_from_slice(&total_len.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_len",
            ..
        })
    ));

    let mut bytes = image();
    // An absent section must not point anywhere. Absence is expressed by a zero
    // *length*, so the length has to be cleared too — an offset alone describes a
    // present section, which is legitimately allowed to point at its data.
    bytes[HEADER_SIZE + SECTION_ENTRY_SIZE + 16..HEADER_SIZE + SECTION_ENTRY_SIZE + 24]
        .copy_from_slice(&0u64.to_le_bytes());
    bytes[HEADER_SIZE + SECTION_ENTRY_SIZE + 8..HEADER_SIZE + SECTION_ENTRY_SIZE + 16]
        .copy_from_slice(&208u64.to_le_bytes());
    assert!(matches!(
        format::parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_offset",
            ..
        })
    ));
}
