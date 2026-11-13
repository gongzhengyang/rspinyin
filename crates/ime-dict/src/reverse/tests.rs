//! Tests for the tail REVERSE index: block construction, structural
//! validation, the reader's query path, the `ReverseSource` view, and
//! container-level integration (flag handling, stacking after the AUX
//! column, corruption rejection).
//!
//! Every fixture is built in memory from hand-written decomposition
//! entries, so the suite is deterministic and touches no dictionary file.

use ime_core::reverse::{ReverseHit, ReverseSource};
use ime_types::DictError;

use super::{
    IndexEntry, REVERSE_CHAR_ROW_SIZE, REVERSE_HEADER_SIZE, REVERSE_KEY_ROW_SIZE, REVERSE_MAGIC,
    REVERSE_MAX_CODE_LEN, ReverseIndex, build_index, validate_block,
};
use crate::format::aux::{AUX_HEADER_SIZE, AUX_ROW_SIZE};
use crate::format::reader::{Reader, Verify};
use crate::format::writer::DictWriter;
use crate::format::{
    DictEntry, FLAG_AUX_PRESENT, FLAG_REVERSE_PRESENT, FORMAT_VERSION_V1, SECTIONS_OFFSET,
    SectionKind, align_up, hash_word, parse_header,
};

/// The hand-written decomposition table the fixtures compile: five
/// characters in ascending codepoint order, whose codes cover one through
/// four letters (the [`REVERSE_MAX_CODE_LEN`] cap) and share fragments.
const FIXTURE: [(char, &str); 5] = [
    ('一', "y"),
    ('中', "zh"),
    ('房', "hf"),
    ('放', "fh"),
    ('输', "scjg"),
];

/// Every non-empty subsequence of the four-letter code `scjg`, hand-listed:
/// exactly the fragments the compiler must materialize as keys (4 + 6 + 4 + 1).
const SCJG_SUBSEQUENCES: [&str; 15] = [
    "s", "c", "j", "g", "sc", "sj", "sg", "cj", "cg", "jg", "scj", "scg", "sjg", "cjg", "scjg",
];

/// The fixture as build-time entries, in ascending codepoint order.
fn fixture_entries() -> Vec<IndexEntry<'static>> {
    FIXTURE
        .iter()
        .map(|&(character, code)| IndexEntry { character, code })
        .collect()
}

/// The compiled fixture index.
///
/// Five characters, 22 distinct fragments (`y h z zh f hf fh` plus the
/// fifteen `scjg` subsequences), 25 postings in total.
fn fixture_index() -> ReverseIndex {
    build_index(&fixture_entries()).expect("a valid fixture")
}

/// A one-character index (`房` = "hf"): the smallest block that still
/// carries several key rows, so structural patches land on hand-computable
/// offsets — characters at 20..28, keys at 28..64, postings at 64..76.
fn single_index() -> ReverseIndex {
    build_index(&[IndexEntry {
        character: '房',
        code: "hf",
    }])
    .expect("a valid entry")
}

/// A container whose tail carries `index` and nothing else.
fn container_with(index: &ReverseIndex) -> Vec<u8> {
    let mut writer = sample_writer();
    writer
        .add_reverse_index(index.bytes.clone())
        .expect("adding the reverse index");
    writer.encode().expect("encoding")
}

/// A container whose tail carries the compiled fixture index.
fn reverse_container() -> Vec<u8> {
    container_with(&fixture_index())
}

/// The two-entry sample writer of the writer tests: `中` and `中国`, no
/// tail payload yet.
fn sample_writer() -> DictWriter {
    let mut entries = Vec::new();
    // Entries follow the builder's hash-ascending order (`中国`'s hash sorts
    // before `中`'s), matching the unigram rows below: `word_id` binary-searches
    // the hash table and then reads the entry text back at the same index, so
    // entry order, strpool layout and unigram rows must all agree.
    entries.extend_from_slice(&DictEntry::new(0, 6, 2, 0, 300).encode());
    entries.extend_from_slice(&DictEntry::new(6, 3, 1, 0, 900).encode());

    let mut wordlist = Vec::new();
    wordlist.extend_from_slice(&0u32.to_le_bytes());
    wordlist.extend_from_slice(&1u32.to_le_bytes());

    let mut unigram = Vec::new();
    // Unigram rows must be hash-ascending — the invariant `word_id`'s binary
    // search relies on. The hashes of these two words order `中国` (0x0b91…)
    // before `中` (0xb846…), so `中国` is record 0 and `中` is record 1;
    // writing them in text order would silently hide `中` from lookups.
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

/// The file offset the reader resolves the reverse block at: the aligned
/// end of the table sections (no AUX column in these fixtures), the
/// placement rule `Reader::parse` follows mirrored here for patch tests.
fn tail_block_start(reader: &Reader<Vec<u8>>) -> usize {
    let body_end = reader
        .sections()
        .iter()
        .filter(|entry| !entry.is_absent())
        .map(|entry| entry.offset + entry.len)
        .max()
        .unwrap_or(SECTIONS_OFFSET);
    align_up(body_end).expect("aligned tail") as usize
}

fn bounds_field(err: &DictError) -> Option<&'static str> {
    match err {
        DictError::LengthOutOfRange { field, .. } => Some(*field),
        _ => None,
    }
}

#[test]
fn test_build_index_encodes_header_counts_and_rows() {
    let index = fixture_index();
    assert_eq!(index.char_count, 5);
    assert_eq!(index.key_count, 22);
    assert_eq!(index.posting_count, 25);
    assert_eq!(
        index.bytes.len(),
        REVERSE_HEADER_SIZE
            + index.char_count * REVERSE_CHAR_ROW_SIZE
            + index.key_count * REVERSE_KEY_ROW_SIZE
            + index.posting_count * 4
    );
    assert_eq!(&index.bytes[0..4], &REVERSE_MAGIC[..]);
    // Characters ascend by codepoint: the first row is 一/"y", the last
    // one is 输/"scjg".
    let chars_at = REVERSE_HEADER_SIZE;
    assert_eq!(
        &index.bytes[chars_at..chars_at + 4],
        &0x4E00u32.to_le_bytes()
    );
    assert_eq!(&index.bytes[chars_at + 4..chars_at + 8], b"y\0\0\0");
    let last_at = chars_at + 4 * REVERSE_CHAR_ROW_SIZE;
    assert_eq!(&index.bytes[last_at..last_at + 4], &0x8F93u32.to_le_bytes());
    assert_eq!(&index.bytes[last_at + 4..last_at + 8], b"scjg");
    // The key rows ascend (the reader's binary search depends on it) and
    // tile the postings array exactly, each range continuing the run.
    let keys_at = chars_at + 5 * REVERSE_CHAR_ROW_SIZE;
    let mut running = 0u32;
    let mut previous = [0u8; REVERSE_MAX_CODE_LEN];
    for row in 0..22 {
        let at = keys_at + row * REVERSE_KEY_ROW_SIZE;
        let fragment: [u8; REVERSE_MAX_CODE_LEN] = index.bytes[at..at + REVERSE_MAX_CODE_LEN]
            .try_into()
            .expect("fragment");
        if row > 0 {
            assert!(fragment > previous, "key rows must ascend");
        }
        previous = fragment;
        let start = u32::from_le_bytes(index.bytes[at + 4..at + 8].try_into().expect("start"));
        let count = u32::from_le_bytes(index.bytes[at + 8..at + 12].try_into().expect("count"));
        assert_eq!(start, running, "key row {row} continues the postings run");
        running += count;
    }
    assert_eq!(running, 25);
    // The shared letter h carries exactly the three zh/hf/fh characters.
    let h_at = (0..22)
        .map(|row| keys_at + row * REVERSE_KEY_ROW_SIZE)
        .find(|&at| index.bytes[at..at + 4] == [b'h', 0, 0, 0])
        .expect("an h key row");
    let start = u32::from_le_bytes(index.bytes[h_at + 4..h_at + 8].try_into().expect("start"));
    let count = u32::from_le_bytes(index.bytes[h_at + 8..h_at + 12].try_into().expect("count"));
    assert_eq!((start, count), (8, 3));
    // The build is a pure function of its entries: the same input encodes
    // to the same bytes, which is what makes a container reproducible.
    assert_eq!(fixture_index(), index);
}

#[test]
fn test_build_index_rejects_invalid_entries() {
    // Each entry breaks the one-to-four-lowercase-letter shape; the empty
    // entry list is its own error.
    let cases: [(IndexEntry<'static>, &str); 4] = [
        (
            IndexEntry {
                character: '房',
                code: "",
            },
            "reverse_code",
        ),
        (
            IndexEntry {
                character: '房',
                code: "hfhfx",
            },
            "reverse_code",
        ),
        (
            IndexEntry {
                character: '房',
                code: "Hf",
            },
            "reverse_code",
        ),
        (
            IndexEntry {
                character: '房',
                code: "h1",
            },
            "reverse_code",
        ),
    ];
    for (entry, field) in cases {
        let error = build_index(&[entry]).expect_err("an invalid code");
        assert_eq!(bounds_field(&error), Some(field), "code {:?}", entry.code);
    }
    let error = build_index(&[]).expect_err("no entries");
    assert_eq!(bounds_field(&error), Some("reverse_empty"));
}

#[test]
fn test_build_index_at_max_code_len_enumerates_every_subsequence() {
    assert_eq!(
        "scjg".len(),
        REVERSE_MAX_CODE_LEN,
        "the code sits exactly at the cap"
    );
    let index = build_index(&[IndexEntry {
        character: '输',
        code: "scjg",
    }])
    .expect("a valid code");
    assert_eq!(index.char_count, 1);
    assert_eq!(index.key_count, SCJG_SUBSEQUENCES.len());
    assert_eq!(
        index.posting_count,
        SCJG_SUBSEQUENCES.len(),
        "each key holds the one matching character"
    );
    // Reading it back, every subsequence of the code — prefix, middle and
    // suffix alike — finds the same character.
    let reader = Reader::parse(container_with(&index), Verify::Full).expect("parsing");
    for payload in SCJG_SUBSEQUENCES {
        let hits = reader.reverse_lookup(payload).expect("lookup");
        assert_eq!(hits.len(), 1, "payload {payload}");
        assert_eq!(hits[0].character, '输');
        assert_eq!(hits[0].code, "scjg");
    }
}

#[test]
fn test_build_index_dedupes_a_repeated_character_keeping_the_first_code() {
    // The polyphonic path: one character recorded with two codes keeps the
    // first code as its characters-table annotation while both codes
    // contribute fragment keys.
    let entries = [
        IndexEntry {
            character: '长',
            code: "cz",
        },
        IndexEntry {
            character: '长',
            code: "dz",
        },
    ];
    let index = build_index(&entries).expect("valid entries");
    assert_eq!(index.char_count, 1);
    // "cz" contributes c, z, cz and "dz" adds d and dz; the shared z is
    // deduplicated, so the character appears once per key.
    assert_eq!(index.key_count, 5);
    assert_eq!(index.posting_count, 5);
    let reader = Reader::parse(container_with(&index), Verify::Full).expect("parsing");
    for payload in ["c", "z", "cz", "d", "dz"] {
        let hits = reader.reverse_lookup(payload).expect("lookup");
        assert_eq!(hits.len(), 1, "payload {payload}");
        assert_eq!(hits[0].character, '长');
        // The annotation stays the primary code regardless of which code
        // the query travelled through.
        assert_eq!(hits[0].code, "cz");
    }
}

#[test]
fn test_validate_block_accepts_a_built_index() {
    let index = fixture_index();
    assert!(validate_block(&index.bytes, 0, Verify::Header).is_ok());
    assert!(validate_block(&index.bytes, 0, Verify::Full).is_ok());
    // A block embedded mid-file: the given offset is honored, not assumed
    // to be zero.
    let mut embedded = vec![0u8; 8];
    embedded.extend_from_slice(&index.bytes);
    assert!(validate_block(&embedded, 8, Verify::Full).is_ok());
}

#[test]
fn test_validate_block_rejects_an_out_of_bounds_frame() {
    let index = single_index();
    // Below the header size there is no block to name.
    let error =
        validate_block(&index.bytes[..10], 0, Verify::Full).expect_err("shorter than a header");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
    // An offset past the image names the offset, not the contents.
    let error = validate_block(&index.bytes, 1_000, Verify::Full).expect_err("past the end");
    assert_eq!(bounds_field(&error), Some("reverse_offset"));
}

#[test]
fn test_validate_block_rejects_a_wrong_magic() {
    let mut bytes = single_index().bytes;
    bytes[3] = b'2';
    let error = validate_block(&bytes, 0, Verify::Full).expect_err("wrong magic");
    assert_eq!(bounds_field(&error), Some("reverse_magic"));
}

#[test]
fn test_validate_block_rejects_a_truncated_tail() {
    let index = single_index();
    // Dropping the last posting byte leaves the declared counts pointing at
    // a payload longer than the block actually carries.
    let error = validate_block(&index.bytes[..index.bytes.len() - 1], 0, Verify::Full)
        .expect_err("truncated");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
}

#[test]
fn test_validate_block_rejects_counts_that_overrun_the_block() {
    // An inflated count makes the declared payload larger than the tail the
    // block carries, which the exact-length check must catch.
    let mut bytes = single_index().bytes;
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("inflated char_count");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
    let mut bytes = single_index().bytes;
    bytes[16..20].copy_from_slice(&4u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("inflated posting_count");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
}

#[test]
fn test_validate_block_rejects_a_corrupt_character_row() {
    // A codepoint that is not a scalar (a UTF-16 surrogate) names no character.
    let mut bytes = single_index().bytes;
    bytes[20..24].copy_from_slice(&0xD800u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("surrogate codepoint");
    assert_eq!(bounds_field(&error), Some("reverse_char"));
    // A code row holds only lowercase letters up to its zero fill.
    let mut bytes = single_index().bytes;
    bytes[24..28].copy_from_slice(b"HF\0\0");
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("uppercase code");
    assert_eq!(bounds_field(&error), Some("reverse_code"));
    // Nothing but zeros may follow the letters.
    let mut bytes = single_index().bytes;
    bytes[24..28].copy_from_slice(b"h\0f\0");
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("letter after the fill");
    assert_eq!(bounds_field(&error), Some("reverse_code"));
}

#[test]
fn test_validate_block_rejects_a_corrupt_key_row() {
    // An all-zero fragment names no subsequence at all.
    let mut bytes = single_index().bytes;
    bytes[40..44].copy_from_slice(&[0; 4]);
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("empty fragment");
    assert_eq!(bounds_field(&error), Some("reverse_key"));
    // Letters may not resume after the zero fill.
    let mut bytes = single_index().bytes;
    bytes[52..56].copy_from_slice(b"h\0f\0");
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("letter after the fill");
    assert_eq!(bounds_field(&error), Some("reverse_key"));
    // An uppercase byte in a fragment surfaces through the shared code-row
    // shape check, which names `reverse_code` before the fragment check runs.
    let mut bytes = single_index().bytes;
    bytes[28] = b'F';
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("uppercase fragment");
    assert_eq!(bounds_field(&error), Some("reverse_code"));
}

#[test]
fn test_validate_block_rejects_unsorted_keys() {
    // Descending: h patched to a sorts before the f row ahead of it.
    let mut bytes = single_index().bytes;
    bytes[40] = b'a';
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("descending key");
    assert_eq!(bounds_field(&error), Some("reverse_order"));
    // Equal rows cannot stand either: two rows naming f.
    let mut bytes = single_index().bytes;
    bytes[40] = b'f';
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("duplicate key");
    assert_eq!(bounds_field(&error), Some("reverse_order"));
}

#[test]
fn test_validate_block_rejects_a_broken_postings_range() {
    // A start that breaks the contiguity: the h row owns postings 1..2,
    // not 0..1.
    let mut bytes = single_index().bytes;
    bytes[44..48].copy_from_slice(&0u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("discontiguous run");
    assert_eq!(bounds_field(&error), Some("reverse_range"));
    // A zero count names an empty run, which no key may declare.
    let mut bytes = single_index().bytes;
    bytes[48..52].copy_from_slice(&0u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("empty run");
    assert_eq!(bounds_field(&error), Some("reverse_range"));
    // Counts that tile beyond the declared postings disagree with the
    // header: the last run would end past posting_count.
    let mut bytes = single_index().bytes;
    bytes[60..64].copy_from_slice(&2u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("runs overshoot the array");
    assert_eq!(bounds_field(&error), Some("reverse_range"));
}

#[test]
fn test_validate_block_rejects_an_out_of_range_posting() {
    // One character in the table, so only the index 0 names a row.
    let mut bytes = single_index().bytes;
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    let error = validate_block(&bytes, 0, Verify::Header).expect_err("posting past the table");
    assert_eq!(bounds_field(&error), Some("reverse_posting"));
}

#[test]
fn test_validate_block_rejects_a_crc_mismatch() {
    let index = single_index();
    let stored = u32::from_le_bytes(index.bytes[4..8].try_into().expect("crc field"));
    // Rewriting a code letter keeps the block structurally valid, so only
    // the checksum can catch it — and only under a full verify.
    let mut bytes = index.bytes.clone();
    bytes[25] = b'g';
    assert!(validate_block(&bytes, 0, Verify::Header).is_ok());
    let error = validate_block(&bytes, 0, Verify::Full).expect_err("stale checksum");
    assert!(matches!(error, DictError::Crc { expected, .. } if expected == stored));
}

#[test]
fn test_reverse_container_roundtrip_answers_queries_in_codepoint_order() {
    let reader = Reader::parse(reverse_container(), Verify::Full).expect("parsing");
    assert!(reader.reverse_index_present());
    // An exact code matches its own character.
    let hits = reader.reverse_lookup("hf").expect("lookup");
    assert_eq!(
        hits,
        vec![ReverseHit {
            character: '房',
            code: "hf"
        }]
    );
    // A shared single letter answers with every carrier in container
    // order, which is ascending codepoint order here.
    let hits = reader.reverse_lookup("h").expect("lookup");
    assert_eq!(
        hits,
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
    // Prefix, middle and suffix fragments of a four-letter code all answer.
    for payload in ["s", "cj", "jg", "scjg"] {
        let hits = reader.reverse_lookup(payload).expect("lookup");
        assert_eq!(hits.len(), 1, "payload {payload}");
        assert_eq!(hits[0].character, '输');
        assert_eq!(hits[0].code, "scjg");
    }
}

#[test]
fn test_reverse_lookup_returns_empty_for_unrecognized_payloads() {
    let reader = Reader::parse(reverse_container(), Verify::Full).expect("parsing");
    // Payloads the domain recognizer would never accept read as empty
    // lists, not errors: empty, uppercase, digits, past the letter cap,
    // non-letters.
    for payload in ["", "H", "h1", "hfxxx", ".."] {
        let hits = reader.reverse_lookup(payload).expect("not an error");
        assert!(hits.is_empty(), "payload {payload:?}");
    }
    // A well-shaped payload that names no key in this index is empty too.
    let hits = reader.reverse_lookup("q").expect("lookup");
    assert!(hits.is_empty());
}

#[test]
fn test_reader_accepts_a_container_without_a_reverse_index() {
    let image = sample_writer().encode().expect("encoding");
    let reader = Reader::parse(image, Verify::Full).expect("parsing");
    assert!(!reader.reverse_index_present());
    let error = reader.reverse_lookup("hf").expect_err("absent index");
    assert_eq!(bounds_field(&error), Some("reverse_absent"));
    // A container that carries another tail flag but not this one degrades
    // the same way: the pre-reverse layout reads identically.
    let mut writer = sample_writer();
    writer
        .add_aux_column([&[0u8; AUX_ROW_SIZE], b"kg\0\0".as_slice()].concat())
        .expect("adding the aux column");
    let reader = Reader::parse(writer.encode().expect("encoding"), Verify::Full).expect("parsing");
    assert!(!reader.reverse_index_present());
    let error = reader.reverse_lookup("hf").expect_err("absent index");
    assert_eq!(bounds_field(&error), Some("reverse_absent"));
}

#[test]
fn test_reverse_source_view_mirrors_the_reader() {
    let reader = Reader::parse(reverse_container(), Verify::Full).expect("parsing");
    let source: &dyn ReverseSource = &reader;
    assert!(source.has_reverse_index());
    let hits = source.reverse_lookup("zh").expect("lookup");
    assert_eq!(
        hits,
        vec![ReverseHit {
            character: '中',
            code: "zh"
        }]
    );
}

#[test]
fn test_add_reverse_index_sets_the_flag_and_the_reader_finds_the_block() {
    let mut writer = sample_writer();
    writer
        .add_reverse_index(fixture_index().bytes)
        .expect("adding the reverse index");
    assert_eq!(writer.flags(), FLAG_REVERSE_PRESENT);
    let image = writer.encode().expect("encoding");
    let header = parse_header(&image).expect("header");
    assert_eq!(header.flags & FLAG_REVERSE_PRESENT, FLAG_REVERSE_PRESENT);
    let reader = Reader::parse(image.clone(), Verify::Full).expect("parsing");
    assert!(reader.reverse_index_present());
    // The block sits exactly at the aligned end of the table sections, the
    // placement the reader resolves it by.
    let start = tail_block_start(&reader);
    assert_eq!(&image[start..start + 4], &REVERSE_MAGIC[..]);
}

#[test]
fn test_add_reverse_index_rejects_bad_payloads() {
    let valid = fixture_index().bytes;
    let mut writer = sample_writer();
    let error = writer
        .add_reverse_index(Vec::new())
        .expect_err("empty payload");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
    // The header must be present and carry the block magic.
    let error = writer
        .add_reverse_index(valid[..REVERSE_HEADER_SIZE - 1].to_vec())
        .expect_err("header cut short");
    assert_eq!(bounds_field(&error), Some("reverse_magic"));
    let mut wrong_magic = valid.clone();
    wrong_magic[3] = b'2';
    let error = writer
        .add_reverse_index(wrong_magic)
        .expect_err("wrong magic");
    assert_eq!(bounds_field(&error), Some("reverse_magic"));
    // A declared count the bytes do not back is a length disagreement.
    let mut inflated = valid.clone();
    inflated[8..12].copy_from_slice(&6u32.to_le_bytes());
    let error = writer
        .add_reverse_index(inflated)
        .expect_err("inflated char_count");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
    // The first add succeeds; a second one cannot follow it.
    writer.add_reverse_index(valid).expect("first add");
    let error = writer
        .add_reverse_index(fixture_index().bytes)
        .expect_err("duplicate index");
    assert_eq!(bounds_field(&error), Some("reverse_duplicate"));
}

#[test]
fn test_reverse_block_stacks_after_the_aux_column() {
    let mut writer = sample_writer();
    writer
        .add_aux_column([&[0u8; AUX_ROW_SIZE], b"kg\0\0".as_slice()].concat())
        .expect("adding the aux column");
    writer
        .add_reverse_index(fixture_index().bytes)
        .expect("adding the reverse index");
    assert_eq!(writer.flags(), FLAG_AUX_PRESENT | FLAG_REVERSE_PRESENT);
    let image = writer.encode().expect("encoding");
    let reader = Reader::parse(image.clone(), Verify::Full).expect("parsing");
    assert!(reader.reverse_index_present());
    // The reader resolves the reverse block past the AUX column's end —
    // the writer's stacking rule — and the raw image agrees.
    let aux_start = reader.aux_start.expect("aux block") as usize;
    let aux_len = AUX_HEADER_SIZE + reader.header().entry_count as usize * AUX_ROW_SIZE;
    let start = align_up((aux_start + aux_len) as u64).expect("aligned tail") as usize;
    assert_eq!(&image[start..start + 4], &REVERSE_MAGIC[..]);
    // Both tail domains answer on the same container.
    let hits = reader.reverse_lookup("hf").expect("reverse lookup");
    assert_eq!(hits[0].character, '房');
    assert_eq!(reader.aux_lookup("中").expect("aux lookup"), Some("kg"));
}

#[test]
fn test_reader_rejects_reverse_payload_corruption() {
    let start =
        tail_block_start(&Reader::parse(reverse_container(), Verify::Full).expect("parsing"));
    let code_at = start + REVERSE_HEADER_SIZE + 4;
    // A flipped code letter keeps the block structurally valid — 'y' becomes
    // 'x', still a well-formed lowercase code — so only the block checksum
    // can catch it: Header verify reads on, Full verify rejects the image.
    let mut bytes = reverse_container();
    bytes[code_at] ^= 0x01;
    assert!(Reader::parse(bytes.clone(), Verify::Header).is_ok());
    assert!(matches!(
        Reader::parse(bytes, Verify::Full),
        Err(DictError::Crc { .. })
    ));
    // A flipped posting byte breaks the structure instead: the last
    // posting's high byte now names a character row that does not exist.
    // The row checks run before the checksum inside the block validation,
    // so the structural error surfaces even under Full verify.
    let mut bytes = reverse_container();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let error = Reader::parse(bytes, Verify::Header).expect_err("bad posting");
    assert_eq!(bounds_field(&error), Some("reverse_posting"));
    // A character row byte outside a..z breaks the charset even under
    // header-only verify, where the checksums are skipped.
    let mut bytes = reverse_container();
    assert_eq!(bytes[code_at], b'y');
    bytes[code_at] = b'Y';
    let error = Reader::parse(bytes, Verify::Header).expect_err("bad charset");
    assert_eq!(bounds_field(&error), Some("reverse_code"));
}

#[test]
fn test_reader_rejects_a_reverse_flag_without_a_block() {
    // Cut the container to just before the block: the flag points at a tail
    // that no longer exists. The declared length is rewritten alongside so
    // the failure is the missing block, not the truncation itself.
    let bytes = reverse_container();
    let start = tail_block_start(&Reader::parse(bytes.clone(), Verify::Full).expect("parsing"));
    let mut truncated = bytes[..start].to_vec();
    let declared = truncated.len() as u64;
    truncated[16..24].copy_from_slice(&declared.to_le_bytes());
    let error = Reader::parse(truncated, Verify::Header).expect_err("flag without a block");
    assert_eq!(bounds_field(&error), Some("reverse_len"));
}

#[test]
fn test_reader_rejects_the_reverse_flag_on_a_version_one_container() {
    let mut bytes = reverse_container();
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_V1.to_le_bytes());
    let error = Reader::parse(bytes, Verify::Header).expect_err("v1 with reverse");
    assert_eq!(bounds_field(&error), Some("reverse_flags"));
}
