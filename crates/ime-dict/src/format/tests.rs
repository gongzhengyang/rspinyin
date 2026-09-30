//! Unit tests for the container layout: the record, the header, the section table and
//! the packing helpers.
//!
//! They live in a file of their own because the layout's own file is at its line budget,
//! and they read the layout through its public entry points only.

use super::*;

/// A minimal but structurally valid container, used by several tests.
fn sample_file() -> Vec<u8> {
    let mut writer = writer::DictWriter::new();
    writer
        .add_section(SectionKind::Fst, vec![1, 2, 3, 4, 5, 6, 7, 8])
        .expect("adding the FST section");
    writer
        .add_section(SectionKind::Entries, Vec::new())
        .expect("adding the ENTRIES section");
    writer.encode().expect("encoding the container")
}

/// Returns the field name of a bounds failure, or `None` for any other error.
fn bounds_field(err: &DictError) -> Option<&'static str> {
    match err {
        DictError::LengthOutOfRange { field, .. } => Some(*field),
        _ => None,
    }
}

#[test]
fn test_dict_entry_round_trips_through_its_bytes() {
    let entry = DictEntry::new(0x1234, 6, 2, FLAG_PLACE, 30_000);
    let decoded = DictEntry::decode(&entry.encode()).expect("decoding");
    assert_eq!(decoded, entry);
    assert_eq!(decoded.word_off, 0x1234);
    assert_eq!(decoded.weight, 30_000);
    assert_eq!(decoded.flags, FLAG_PLACE);
}

#[test]
fn test_dict_entry_decode_rejects_a_short_buffer() {
    let entry = DictEntry::new(0, 6, 2, 0, 1);
    let bytes = entry.encode();
    for len in 0..ENTRY_SIZE {
        let result = DictEntry::decode(&bytes[..len]);
        assert!(
            matches!(result, Err(DictError::LengthOutOfRange { .. })),
            "a {len}-byte buffer must not decode"
        );
    }
}

#[test]
fn test_dict_entry_validate_rejects_out_of_contract_fields() {
    let padded = DictEntry {
        _pad: [1, 0, 0],
        ..DictEntry::new(0, 6, 1, 0, 0)
    };
    let cases = [
        DictEntry::new(0, 0, 1, 0, 0),
        DictEntry::new(0, MAX_WORD_LEN + 1, 1, 0, 0),
        DictEntry::new(0, 6, 0, 0, 0),
        DictEntry::new(0, 6, MAX_SYL_COUNT + 1, 0, 0),
        padded,
    ];
    for entry in cases {
        assert!(
            matches!(entry.validate(), Err(DictError::LengthOutOfRange { .. })),
            "{entry:?} must not validate"
        );
    }
    assert!(DictEntry::new(0, 3, 1, FLAG_SURNAME, 7).validate().is_ok());
}

#[test]
fn test_dict_entry_keeps_flag_bits_it_does_not_know() {
    // Forward compatibility: a later version that defines a new flag must not make its
    // dictionary unreadable here. The bit survives the round trip, and the bits this
    // version knows are still extractable from it.
    let future = DictEntry::new(0, 6, 1, FLAG_SURNAME | 0b1000_0000, 7);
    assert!(future.validate().is_ok(), "an undefined bit is not a fault");
    let decoded = DictEntry::decode(&future.encode()).expect("decoding");
    assert_eq!(decoded.flags, FLAG_SURNAME | 0b1000_0000);
    assert_eq!(decoded.flags & ENTRY_FLAG_MASK, FLAG_SURNAME);
}

#[test]
fn test_dict_entry_carries_the_character_count_through_its_bytes() {
    // Three bytes of Han text are one character, which is the case the count exists for:
    // the byte length and the character count are not the same number.
    let entry = DictEntry::new(0, 6, 2, 0, 300).with_characters(2);
    let decoded = DictEntry::decode(&entry.encode()).expect("decoding");
    assert_eq!(decoded.char_count, 2);
    assert_eq!(decoded.word_len, 6);
    assert_eq!(decoded, entry);
}

#[test]
fn test_dict_entry_without_a_character_count_is_still_in_contract() {
    // A record written before the field existed carries zero in those bytes, and the
    // read path has to keep accepting it: refusing it would turn every dictionary built
    // by an earlier compiler into an unreadable one.
    let entry = DictEntry::new(0, 3, 1, 0, 7);
    assert_eq!(entry.char_count, 0);
    assert!(entry.validate().is_ok());
    let decoded = DictEntry::decode(&entry.encode()).expect("decoding");
    assert_eq!(decoded.char_count, 0);
}

#[test]
fn test_dict_entry_validate_rejects_a_character_count_above_the_byte_length() {
    // A character is at least one byte, so a count above `word_len` describes a text no
    // compiler could have written.
    let too_many = DictEntry::new(0, 3, 1, 0, 7).with_characters(4);
    let err = too_many
        .validate()
        .expect_err("four characters need four bytes");
    assert_eq!(bounds_field(&err), Some("char_count"), "{err}");

    // The boundary itself is in contract: a three-byte word of three characters.
    let exact = DictEntry::new(0, 3, 1, 0, 7).with_characters(3);
    assert!(exact.validate().is_ok());
    let decoded = DictEntry::decode(&exact.encode()).expect("decoding");
    assert_eq!(decoded.char_count, 3);
}

#[test]
fn test_dict_entry_decode_rejects_a_non_zero_reserved_byte() {
    let entry = DictEntry::new(0, 3, 1, 0, 7).with_characters(1);
    for at in 13..ENTRY_SIZE {
        let mut bytes = entry.encode();
        bytes[at] = 1;
        let err = DictEntry::decode(&bytes).expect_err("the reserved bytes are zero");
        assert_eq!(bounds_field(&err), Some("_pad"), "byte {at}");
    }
}

#[test]
fn test_parse_header_accepts_a_written_file() {
    let bytes = sample_file();
    let header = parse_header(&bytes).expect("header");
    assert_eq!(header.format_version, FORMAT_VERSION);
    assert_eq!(header.total_len, bytes.len() as u64);
    assert_eq!(header.entry_count, 0);
    assert_eq!(header.flags, 0);
}

#[test]
fn test_parse_section_table_reads_the_written_layout() {
    let bytes = sample_file();
    let sections = parse_section_table(&bytes).expect("section table");
    assert_eq!(sections[0].kind, SectionKind::Fst);
    assert_eq!(sections[0].len, 8);
    assert_eq!(sections[0].offset, SECTIONS_OFFSET);
    assert_eq!(sections[0].crc32, crc32(&bytes[208..216]));
    assert!(sections[1].is_absent());
    assert_eq!(sections[5].kind, SectionKind::WordList);
}

#[test]
fn test_parse_section_table_rejects_a_truncated_table() {
    let bytes = vec![0u8; HEADER_SIZE];
    assert!(matches!(
        parse_section_table(&bytes),
        Err(DictError::LengthOutOfRange {
            field: "section_table",
            ..
        })
    ));
}

#[test]
fn test_section_kind_round_trips_and_names_itself() {
    for kind in SectionKind::ALL {
        assert_eq!(SectionKind::from_raw(kind.as_raw()), Some(kind));
        assert!(!kind.name().is_empty());
    }
    assert_eq!(SectionKind::ALL.len(), SECTION_COUNT);
}

#[test]
fn test_pack_and_unpack_fst_value_round_trip() {
    let cases = [(0u64, 0u32), (1, 1), (123_456, 32), (MAX_FST_START, 0)];
    for (start, count) in cases {
        let packed = pack_fst_value(start, count).expect("packing");
        assert_eq!(unpack_fst_value(packed), (start, count));
    }
    assert!(matches!(
        pack_fst_value(0, 1 << FST_COUNT_BITS),
        Err(DictError::LengthOutOfRange {
            field: "fst_count",
            ..
        })
    ));
    assert!(matches!(
        pack_fst_value(MAX_FST_START + 1, 1),
        Err(DictError::LengthOutOfRange {
            field: "fst_start",
            ..
        })
    ));
}

#[test]
fn test_hash_word_is_stable_and_documented() {
    assert_eq!(hash_word(""), 0x811c_9dc5);
    assert_eq!(hash_word("中国"), hash_word("中国"));
    assert_ne!(hash_word("中国"), hash_word("中國"));
}

#[test]
fn test_read_helpers_reject_short_slices() {
    let bytes = [1u8, 2, 3];
    assert_eq!(read_u16(&bytes, 0, "x").expect("two bytes"), 0x0201);
    assert!(matches!(
        read_u16(&bytes, 2, "x"),
        Err(DictError::LengthOutOfRange { field: "x", .. })
    ));
    assert!(matches!(
        read_u32(&bytes, 0, "y"),
        Err(DictError::LengthOutOfRange { field: "y", .. })
    ));
    assert!(matches!(
        read_u64(&bytes, 0, "z"),
        Err(DictError::LengthOutOfRange { field: "z", .. })
    ));
}

#[test]
fn test_crc32_matches_a_known_vector() {
    // The CRC32 of "123456789" is the standard check value.
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}
