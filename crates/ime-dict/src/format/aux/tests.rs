//! Tests for the tail AUX column: block validation, the read path, the
//! `AuxSource` view, and old-container compatibility.

use ime_core::aux::AuxSource;
use ime_types::DictError;

use super::{AUX_HEADER_SIZE, AUX_MAGIC, AUX_ROW_SIZE, encode_block};
use crate::format::reader::{Reader, Verify};
use crate::format::writer::DictWriter;
use crate::format::{
    DictEntry, FLAG_AUX_PRESENT, FLAG_BIGRAM_PRESENT, FLAG_MASK, FORMAT_VERSION_V1, SectionKind,
    hash_word, parse_header,
};

/// A two-entry container (`中`, `中国`) with the AUX column added.
///
/// The aux rows follow the record order the sample writer produces: `中国`
/// (record 0) carries the empty row, `中` (record 1) carries `kg` — one
/// recorded and one unrecorded word, as the assertions below rely on.
fn aux_container() -> Vec<u8> {
    writer_with_aux(vec![vec![0; AUX_ROW_SIZE], b"kg\0\0".to_vec()])
        .encode()
        .expect("encoding")
}

/// The two-entry sample writer of the writer tests, plus an aux payload.
fn writer_with_aux(rows: Vec<Vec<u8>>) -> DictWriter {
    let mut writer = sample_writer();
    let rows: Vec<u8> = rows.into_iter().flatten().collect();
    writer.add_aux_column(rows).expect("adding the aux column");
    writer
}

/// The sample writer of the writer tests: `中` and `中国`, no aux column.
///
/// Entries, strpool and unigram rows all follow the builder's hash-ascending
/// order (`中国`'s hash sorts before `中`'s): `word_id` binary-searches the
/// hash table and reads the entry text back at the same index, so record 0 is
/// `中国` and record 1 is `中`.
fn sample_writer() -> DictWriter {
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
        .add_section(SectionKind::Fst, vec![0xAA; 16])
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

fn bounds_field(err: &DictError) -> Option<&'static str> {
    match err {
        DictError::LengthOutOfRange { field, .. } => Some(*field),
        _ => None,
    }
}

#[test]
fn test_encode_block_lays_out_magic_crc_count_and_rows() {
    let block = encode_block(2, b"kg\0\0\0\0\0\0").expect("encoding");
    assert_eq!(block.len(), AUX_HEADER_SIZE + 2 * AUX_ROW_SIZE);
    assert_eq!(&block[0..4], &AUX_MAGIC[..]);
    assert_eq!(&block[8..12], &2u32.to_le_bytes());
    assert!(block[12..AUX_HEADER_SIZE].iter().all(|byte| *byte == 0));
    assert_eq!(&block[AUX_HEADER_SIZE..AUX_HEADER_SIZE + 4], b"kg\0\0");
    assert!(block[AUX_HEADER_SIZE + 4..].iter().all(|byte| *byte == 0));

    assert!(matches!(
        bounds_field(&encode_block(2, b"kg\0\0\0").expect_err("partial row")),
        Some("aux_len")
    ));
    // Eight payload bytes are two whole rows, so a declared count of three is
    // what disagrees (a 5-byte payload would trip the row-width check first).
    assert!(matches!(
        bounds_field(&encode_block(3, b"kg\0\0\0\0\0\0").expect_err("count mismatch")),
        Some("aux_count")
    ));
}

#[test]
fn test_add_aux_column_rejects_bad_payloads() {
    let mut writer = sample_writer();
    assert!(matches!(
        bounds_field(&writer.add_aux_column(Vec::new()).expect_err("empty")),
        Some("aux_len")
    ));
    assert!(matches!(
        bounds_field(
            &writer
                .add_aux_column(b"kg\0\0\0".to_vec())
                .expect_err("partial")
        ),
        Some("aux_len")
    ));
    // Two rows for a two-entry container is right; three is a count error.
    assert!(matches!(
        bounds_field(
            &writer
                .add_aux_column(vec![0; 3 * AUX_ROW_SIZE])
                .expect_err("count mismatch")
        ),
        Some("aux_count")
    ));
    writer
        .add_aux_column(vec![0; 2 * AUX_ROW_SIZE])
        .expect("first add");
    assert!(matches!(
        bounds_field(
            &writer
                .add_aux_column(vec![0; 2 * AUX_ROW_SIZE])
                .expect_err("duplicate")
        ),
        Some("aux_duplicate")
    ));
}

#[test]
fn test_encode_sets_the_aux_flag_and_readers_see_the_column() {
    let writer = writer_with_aux(vec![b"kg\0\0".to_vec(), vec![0; AUX_ROW_SIZE]]);
    assert_eq!(writer.flags(), FLAG_AUX_PRESENT);
    let image = writer.encode().expect("encoding");
    let image_len = image.len();
    let header = parse_header(&image).expect("header");
    assert_eq!(header.flags & FLAG_AUX_PRESENT, FLAG_AUX_PRESENT);
    // The block sits past the section table, at the aligned end of the file.
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    assert!(reader.aux_column_present());
    let block_start = reader
        .sections()
        .iter()
        .filter(|entry| !entry.is_absent())
        .map(|entry| entry.offset + entry.len)
        .max()
        .map(crate::format::align_up)
        .and_then(Result::ok)
        .expect("aligned block start");
    assert!(
        block_start as usize + AUX_HEADER_SIZE < image_len,
        "the block must hold at least its header"
    );
}

#[test]
fn test_aux_roundtrip_reads_codes_by_word_text() {
    let reader = Reader::parse(aux_container(), Verify::Full).expect("parsing");
    assert_eq!(reader.aux_lookup("中").expect("lookup"), Some("kg"));
    assert_eq!(
        reader.aux_lookup("中国").expect("lookup"),
        None,
        "an all-zero row reads as unrecorded"
    );
    assert_eq!(reader.aux_lookup(" absent").expect("lookup"), None);
    assert!(matches!(
        bounds_field(&reader.aux_row(2).expect_err("past the column")),
        Some("aux_index")
    ));
}

#[test]
fn test_aux_source_view_mirrors_the_reader() {
    let reader = Reader::parse(aux_container(), Verify::Full).expect("parsing");
    let source: &dyn AuxSource = &reader;
    assert!(source.has_aux_column());
    assert_eq!(source.aux_code("中").expect("lookup"), Some("kg"));
    assert_eq!(source.aux_code("中国").expect("lookup"), None);
}

#[test]
fn test_reader_accepts_a_container_without_the_aux_column() {
    let image = sample_writer().encode().expect("encoding");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    assert!(!reader.aux_column_present());
    assert!(matches!(
        bounds_field(&reader.aux_row(0).expect_err("absent column")),
        Some("aux_absent")
    ));
    // A v2 container that only carries the bigram flag is untouched.
    let mut writer = sample_writer();
    writer
        .add_section(
            SectionKind::Bigram,
            crate::format::BigramRecord::new(1, 1).encode().to_vec(),
        )
        .expect("bigram");
    let reader = Reader::parse(writer.encode().expect("encoding"), Verify::Full).expect("parsing");
    assert_eq!(reader.header().flags, FLAG_BIGRAM_PRESENT);
    assert!(!reader.aux_column_present());
}

#[test]
fn test_reader_rejects_the_aux_flag_on_a_version_one_container() {
    let image = aux_container();
    let header = parse_header(&image).expect("header");
    assert!(header.flags & FLAG_AUX_PRESENT != 0);
    // A v1 header with the aux bit: the flag word passes FLAG_MASK, so the
    // version check is what must hold the line.
    let mut bytes = image;
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_V1.to_le_bytes());
    let error = Reader::parse(bytes, Verify::Full).expect_err("v1 with aux");
    assert_eq!(bounds_field(&error), Some("aux_flags"));
}

#[test]
fn test_reader_rejects_aux_payload_corruption() {
    // A flipped byte inside the recorded code trips the block checksum under
    // Full verify. The flip lands on `k` of the `kg` row (the last row of the
    // fixture), which stays inside a..z on purpose: a zero-fill flip would be
    // caught by the charset check before the checksum ever runs.
    let mut bytes = aux_container();
    let code_start = bytes.len() - 4;
    assert_eq!(bytes[code_start], b'k');
    bytes[code_start] ^= 0x01;
    assert!(matches!(
        Reader::parse(bytes, Verify::Full),
        Err(DictError::Crc { .. })
    ));

    // A row byte outside a..z (here an uppercase letter) breaks the charset
    // even under header-only verify, where checksums are skipped. The `kg`
    // row is the fixture's second row, one row past the block header.
    let mut bytes = aux_container();
    let block_start = Reader::parse(aux_container(), Verify::Full)
        .expect("parsing")
        .sections()
        .iter()
        .filter(|entry| !entry.is_absent())
        .map(|entry| entry.offset + entry.len)
        .max()
        .map(crate::format::align_up)
        .and_then(Result::ok)
        .expect("block start") as usize;
    let row_at = block_start + AUX_HEADER_SIZE + AUX_ROW_SIZE;
    assert_eq!(bytes[row_at], b'k');
    bytes[row_at] = b'K';
    let error = Reader::parse(bytes, Verify::Header).expect_err("bad charset");
    assert_eq!(bounds_field(&error), Some("aux_row"));
}

#[test]
fn test_reader_rejects_an_aux_count_disagreement() {
    let mut bytes = aux_container();
    let block_start = Reader::parse(aux_container(), Verify::Full)
        .expect("parsing")
        .sections()
        .iter()
        .filter(|entry| !entry.is_absent())
        .map(|entry| entry.offset + entry.len)
        .max()
        .map(crate::format::align_up)
        .and_then(Result::ok)
        .expect("block start") as usize;
    let count_at = block_start + 8;
    bytes[count_at..count_at + 4].copy_from_slice(&3u32.to_le_bytes());
    let error = Reader::parse(bytes, Verify::Header).expect_err("count mismatch");
    assert_eq!(bounds_field(&error), Some("aux_count"));
}

#[test]
fn test_reader_rejects_an_aux_flag_without_a_block() {
    // Truncate the container to just before the block: the flag still points
    // at a tail that no longer exists.
    let bytes = aux_container();
    let block_start = Reader::parse(bytes.clone(), Verify::Full)
        .expect("parsing")
        .sections()
        .iter()
        .filter(|entry| !entry.is_absent())
        .map(|entry| entry.offset + entry.len)
        .max()
        .map(crate::format::align_up)
        .and_then(Result::ok)
        .expect("block start") as usize;
    let mut truncated = bytes[..block_start].to_vec();
    // Keep total_len honest for the truncation itself, so the failure is the
    // missing block and not a length disagreement: rewrite the declared and
    // actual size together.
    let declared = truncated.len() as u64;
    truncated[16..24].copy_from_slice(&declared.to_le_bytes());
    let error = Reader::parse(truncated, Verify::Header).expect_err("flag without a block");
    assert_eq!(bounds_field(&error), Some("aux_len"));
}

#[test]
fn test_reader_rejects_an_unknown_flag_bit() {
    let mut bytes = aux_container();
    let unknown = FLAG_MASK + 1;
    bytes[6..8].copy_from_slice(&unknown.to_le_bytes());
    assert!(matches!(
        Reader::parse(bytes, Verify::Header),
        Err(DictError::LengthOutOfRange { field: "flags", .. })
    ));
}
