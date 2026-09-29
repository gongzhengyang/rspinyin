//! Tests for the injection channel's fixture: the scratch copy, the mutations it refuses to
//! apply, and the two shapes of damage a user store arrives in.
//!
//! Every case builds its own container with the real writer and works inside its own scratch
//! directory, so nothing here reads the clock, the environment, a real dictionary, a real
//! fcitx5 or a display server. The two cases that put a file back on disk under the fixture
//! do it deliberately, and say so where they do it.
//!
//! The mutation matrix -- one case per way of breaking a container, with the kind and the
//! field each one has to be refused on -- lives in the sibling module, which is where a
//! reader asking "what is covered" should start.

use std::fs;

use ime_dict::format::{
    FORMAT_VERSION, HEADER_SIZE, MAX_WORD_LEN, SectionKind,
    reader::{Reader, Verify},
};
use ime_dict::fst_index::FstLexicon;
use ime_types::{DictError, Lexicon};

use super::mutate::{GARBAGE_BYTES, garbage};
use super::support::{Scratch, fixture};
use super::{
    DictErrorKind, DictFixture, DictMutation, FixtureError, RefusalPoint, USER_STORE_NAME,
    UserStoreFixture, UserStoreMutation, sha256,
};

#[test]
fn test_from_compiled_copies_the_source_and_leaves_it_alone() {
    let scratch = Scratch::new("copy");
    let (mut fixture, source) = fixture(&scratch);
    let origin = scratch.path("base.dict");
    let copy = scratch.path("copy.dict");
    let digest = sha256(&origin).expect("hashing the source");

    assert_eq!(
        fs::read(&copy).expect("reading the copy"),
        source.image,
        "the copy is the source, byte for byte"
    );
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    assert_ne!(
        fs::read(&copy).expect("reading the copy"),
        source.image,
        "the copy carries the mutation"
    );
    assert_eq!(
        sha256(&origin).expect("hashing the source"),
        digest,
        "the source is read and never written"
    );

    drop(fixture);
    assert_eq!(
        scratch.names(),
        vec!["base.dict"],
        "the copy is taken away and the source is not"
    );
    assert_eq!(fs::read(&origin).expect("reading the source"), source.image);
}

#[test]
fn test_from_compiled_refuses_a_source_that_is_not_a_container() {
    let scratch = Scratch::new("bad-source");
    let origin = scratch.path("base.dict");
    fs::write(&origin, b"not a container").expect("writing the source");
    let refused = DictFixture::from_compiled(&origin, &scratch.path("copy.dict")).err();
    assert!(
        matches!(&refused, Some(FixtureError::Unreadable { .. })),
        "{refused:?}"
    );

    let absent = scratch.path("nothing.dict");
    let copy = scratch.path("copy.dict");
    let missing = DictFixture::from_compiled(&absent, &copy).err();
    assert!(
        matches!(&missing, Some(FixtureError::Io { .. })),
        "{missing:?}"
    );
    assert_eq!(
        scratch.names(),
        vec!["base.dict"],
        "a refused source leaves no copy behind"
    );
}

#[test]
fn test_mutate_refuses_a_change_that_would_leave_the_container_valid() {
    let scratch = Scratch::new("inert");
    let (mut fixture, source) = fixture(&scratch);
    let cases = [
        DictMutation::FormatVersion {
            version: FORMAT_VERSION,
        },
        DictMutation::EntryLength { index: 0, len: 3 },
        DictMutation::EntryOffset { index: 0, off: 0 },
        DictMutation::Truncate {
            at: source.image.len(),
        },
    ];
    for mutation in cases {
        let refused = fixture.mutate(mutation.clone()).err();
        assert!(
            matches!(&refused, Some(FixtureError::InertMutation { .. })),
            "{mutation:?}: {refused:?}"
        );
    }
    assert_eq!(
        fs::read(fixture.path()).expect("reading the copy"),
        source.image,
        "a refused mutation leaves the copy pristine"
    );
    assert!(
        fixture.applied().is_none(),
        "a refused mutation is not recorded as applied"
    );
}

#[test]
fn test_mutate_refuses_a_mutation_the_container_cannot_carry() {
    let scratch = Scratch::new("inapplicable");
    let (mut fixture, _) = fixture(&scratch);
    let cases = [
        DictMutation::EntryLength { index: 99, len: 0 },
        DictMutation::SectionCrc {
            kind: SectionKind::Bigram,
        },
        DictMutation::WordlistRange {
            key: String::from("meiyou"),
        },
    ];
    for mutation in cases {
        let refused = fixture.mutate(mutation.clone()).err();
        assert!(
            matches!(&refused, Some(FixtureError::Inapplicable { .. })),
            "{mutation:?}: {refused:?}"
        );
    }
}

#[test]
fn test_magic_mutation_is_refused_when_the_container_is_opened() {
    let scratch = Scratch::new("magic");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    let refusal = fixture.refusal().expect("reading the refusal");
    assert_eq!(refusal.point, RefusalPoint::Load);
    assert!(
        matches!(&refusal.error, DictError::MagicMismatch),
        "{:?}",
        refusal.error
    );
    fixture
        .assert_rejected(DictErrorKind::MagicMismatch)
        .expect("the magic must be caught");
}

#[test]
fn test_format_version_mutation_is_refused_when_the_container_is_opened() {
    let scratch = Scratch::new("version");
    let (mut fixture, _) = fixture(&scratch);
    for version in [FORMAT_VERSION + 1, 0, u16::MAX] {
        fixture
            .mutate(DictMutation::FormatVersion { version })
            .expect("applying the mutation");
        fixture
            .assert_rejected(DictErrorKind::FormatVersion)
            .expect("the version must be caught");
        match fixture.refusal().expect("reading the refusal").error {
            DictError::FormatVersion { found } => {
                assert_eq!(found, version, "the reported version is the one written");
            }
            other => panic!("a version this build does not read: {other:?}"),
        }
    }
}

#[test]
fn test_truncate_mutation_is_refused_when_the_container_is_opened() {
    let scratch = Scratch::new("truncate");
    let (mut fixture, source) = fixture(&scratch);
    let at = source.image.len() - 8;
    fixture
        .mutate(DictMutation::Truncate { at })
        .expect("applying the mutation");
    assert_eq!(
        fs::metadata(fixture.path()).expect("stat").len(),
        at as u64,
        "the file is cut before it is mapped, so the load-time length check is what catches it"
    );
    let refusal = fixture.refusal().expect("reading the refusal");
    assert_eq!(refusal.point, RefusalPoint::Load);
    assert!(
        matches!(
            &refusal.error,
            DictError::LengthOutOfRange {
                field: "total_len",
                ..
            }
        ),
        "{:?}",
        refusal.error
    );
    fixture
        .assert_rejected(DictErrorKind::LengthOutOfRange)
        .expect("the declared length must be caught");

    // A file too short to hold a header takes the other branch of the same check.
    fixture
        .mutate(DictMutation::Truncate {
            at: HEADER_SIZE - 1,
        })
        .expect("applying the mutation");
    match fixture.refusal().expect("reading the refusal").error {
        DictError::LengthOutOfRange { field, .. } => assert_eq!(field, "file_len"),
        other => panic!("a stub is not a container: {other:?}"),
    }
}

#[test]
fn test_empty_file_mutation_is_refused_when_the_container_is_opened() {
    let scratch = Scratch::new("empty");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::EmptyFile)
        .expect("applying the mutation");
    assert_eq!(
        fs::metadata(fixture.path()).expect("stat").len(),
        0,
        "the file holds nothing"
    );
    let refusal = fixture.refusal().expect("reading the refusal");
    assert_eq!(refusal.point, RefusalPoint::Load);
    assert!(
        matches!(
            &refusal.error,
            DictError::LengthOutOfRange {
                field: "file_len",
                ..
            }
        ),
        "{:?}",
        refusal.error
    );
    fixture
        .assert_rejected(DictErrorKind::LengthOutOfRange)
        .expect("an empty file must be caught");
}

#[test]
fn test_section_crc_mutation_is_refused_by_the_checksum() {
    let scratch = Scratch::new("crc");
    let (mut fixture, _) = fixture(&scratch);
    let present = [
        SectionKind::Fst,
        SectionKind::Entries,
        SectionKind::StrPool,
        SectionKind::Unigram,
        SectionKind::WordList,
    ];
    for kind in present {
        fixture
            .mutate(DictMutation::SectionCrc { kind })
            .expect("applying the mutation");
        fixture
            .assert_rejected(DictErrorKind::Crc)
            .expect("a flipped byte must fail the checksum");
        // The flip is the only fault: the structure still parses, which is what makes the
        // refusal a statement about the checksum rather than about the layout.
        let reader = Reader::open_with(fixture.path(), Verify::Header).expect("header-only parse");
        assert_eq!(reader.entry_count(), 2, "{kind:?}");
    }
}

#[test]
fn test_entry_length_mutation_is_refused_when_the_record_is_decoded() {
    let scratch = Scratch::new("entry-length");
    let (mut fixture, _) = fixture(&scratch);
    for len in [0u16, MAX_WORD_LEN + 1, u16::MAX] {
        fixture
            .mutate(DictMutation::EntryLength { index: 0, len })
            .expect("applying the mutation");
        // The container itself is accepted: the loader never walks the entry table.
        FstLexicon::load(fixture.path()).expect("the container is otherwise valid");
        let refusal = fixture.refusal().expect("reading the refusal");
        assert_eq!(refusal.point, RefusalPoint::RecordDecode, "len={len}");
        assert!(
            matches!(
                &refusal.error,
                DictError::LengthOutOfRange {
                    field: "word_len",
                    ..
                }
            ),
            "len={len}: {:?}",
            refusal.error
        );
        fixture
            .assert_rejected(DictErrorKind::LengthOutOfRange)
            .expect("the length must be caught");
    }
}

#[test]
fn test_entry_offset_mutation_is_refused_when_the_record_becomes_a_word() {
    let scratch = Scratch::new("entry-offset");
    let (mut fixture, source) = fixture(&scratch);
    for off in [source.pool_len + 1, u32::MAX] {
        fixture
            .mutate(DictMutation::EntryOffset { index: 0, off })
            .expect("applying the mutation");
        FstLexicon::load(fixture.path()).expect("the container is otherwise valid");
        // The record decodes: `word_off` is not one of the fields the container's own
        // validation checks, which is why the refusal arrives one step later.
        let reader = Reader::open(fixture.path()).expect("reading the container");
        assert!(reader.entry(0).is_ok(), "off={off}: the record decodes");
        let refusal = fixture.refusal().expect("reading the refusal");
        assert_eq!(refusal.point, RefusalPoint::EntryToRef, "off={off}");
        assert!(
            matches!(
                &refusal.error,
                DictError::LengthOutOfRange {
                    field: "word_off",
                    ..
                }
            ),
            "off={off}: {:?}",
            refusal.error
        );
        fixture
            .assert_rejected(DictErrorKind::LengthOutOfRange)
            .expect("the offset must be caught");
    }
}

#[test]
fn test_wordlist_range_mutation_is_refused_by_the_lookup() {
    let scratch = Scratch::new("wordlist-range");
    let (mut fixture, source) = fixture(&scratch);
    fixture
        .mutate(DictMutation::WordlistRange {
            key: String::from(source.key),
        })
        .expect("applying the mutation");
    let lexicon = FstLexicon::load(fixture.path()).expect("the container is otherwise valid");
    let reader = Reader::open(fixture.path()).expect("reading the container");
    let map = reader.fst_map().expect("the rebuilt index parses");
    assert!(
        map.get(source.key.as_bytes()).is_some(),
        "the key survived the rebuild; only its range moved"
    );
    assert_eq!(
        lexicon
            .lookup(source.other_key)
            .expect("the other key still reads")
            .count(),
        1,
        "the rebuild kept the rest of the index"
    );

    let refusal = fixture.refusal().expect("reading the refusal");
    assert_eq!(refusal.point, RefusalPoint::Lookup);
    assert!(
        matches!(
            &refusal.error,
            DictError::LengthOutOfRange {
                field: "wordlist_start",
                ..
            }
        ),
        "{:?}",
        refusal.error
    );
    fixture
        .assert_rejected(DictErrorKind::LengthOutOfRange)
        .expect("the range must be caught rather than read");
}

#[test]
fn test_assert_rejected_reports_a_container_that_was_accepted() {
    let scratch = Scratch::new("accepted");
    let (mut fixture, source) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    // The pristine image is put back deliberately: it stands in for a fixture that produced
    // a usable container, which is the failure the assertion exists to catch.
    fs::write(fixture.path(), &source.image).expect("putting the pristine image back");
    let refused = fixture.assert_rejected(DictErrorKind::MagicMismatch).err();
    assert!(
        matches!(&refused, Some(FixtureError::NotRejected { .. })),
        "{refused:?}"
    );
}

#[test]
fn test_assert_rejected_reports_a_container_refused_at_another_step() {
    let scratch = Scratch::new("wrong-step");
    let (mut fixture, source) = fixture(&scratch);
    let off = source.pool_len + 1;
    fixture
        .mutate(DictMutation::EntryOffset { index: 0, off })
        .expect("applying the mutation");
    // A container that does not open at all stands in for a fixture whose machinery damaged
    // the file: the refusal arrives at the load, one step before the record is converted.
    let mut broken = source.image.clone();
    broken[0] = b'X';
    fs::write(fixture.path(), &broken).expect("writing a container that does not open");
    let refused = fixture
        .assert_rejected(DictErrorKind::LengthOutOfRange)
        .err();
    match refused {
        Some(FixtureError::WrongStep { actual, .. }) => {
            assert_eq!(
                actual,
                RefusalPoint::Load,
                "the refusal arrived at the load"
            );
        }
        other => panic!("the assertion must report the wrong step: {other:?}"),
    }
}

#[test]
fn test_assert_rejected_reports_the_wrong_kind() {
    let scratch = Scratch::new("wrong-kind");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    let refused = fixture.assert_rejected(DictErrorKind::Crc).err();
    match refused {
        Some(FixtureError::WrongKind {
            expected, actual, ..
        }) => {
            assert_eq!(expected, DictErrorKind::Crc);
            assert_eq!(actual, DictErrorKind::MagicMismatch);
        }
        other => panic!("a wrong expectation must be reported: {other:?}"),
    }
}

#[test]
fn test_assert_rejected_before_a_mutation_reports_no_mutation() {
    let scratch = Scratch::new("no-mutation");
    let (fixture, _) = fixture(&scratch);
    let asserted = fixture.assert_rejected(DictErrorKind::MagicMismatch).err();
    assert!(
        matches!(&asserted, Some(FixtureError::NoMutation { .. })),
        "{asserted:?}"
    );
    let read = fixture.refusal().err();
    assert!(
        matches!(&read, Some(FixtureError::NoMutation { .. })),
        "{read:?}"
    );
    assert!(fixture.applied().is_none(), "nothing was applied");
}

#[test]
fn test_assert_rejected_field_accepts_the_field_the_mutation_broke() {
    let scratch = Scratch::new("field-ok");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::EntryLength { index: 0, len: 0 })
        .expect("applying the mutation");
    fixture
        .assert_rejected_field(DictErrorKind::LengthOutOfRange, "word_len")
        .expect("the field the mutation broke is the one the refusal names");
}

#[test]
fn test_assert_rejected_field_reports_a_refusal_on_another_field() {
    let scratch = Scratch::new("field-wrong");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::EntryLength { index: 0, len: 0 })
        .expect("applying the mutation");
    let refused = fixture
        .assert_rejected_field(DictErrorKind::LengthOutOfRange, "word_off")
        .err();
    match refused {
        Some(FixtureError::WrongField {
            expected, actual, ..
        }) => {
            assert_eq!(expected, "word_off");
            assert!(
                actual.contains("word_len"),
                "the refusal must name the field it did break: {actual}"
            );
        }
        other => panic!("a wrong field must be reported: {other:?}"),
    }
}

#[test]
fn test_assert_rejected_field_reports_a_refusal_that_names_no_field() {
    let scratch = Scratch::new("field-none");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    // The kind is right, so the assertion gets past the first check; the refusal is not a
    // bounds failure at all, which is a wrong field rather than a wrong kind.
    let refused = fixture
        .assert_rejected_field(DictErrorKind::MagicMismatch, "total_len")
        .err();
    match refused {
        Some(FixtureError::WrongField { actual, .. }) => {
            assert!(
                actual.contains("magic"),
                "the refusal itself must be visible: {actual}"
            );
        }
        other => panic!("a refusal that names no field must be reported: {other:?}"),
    }
}

#[test]
fn test_assert_rejected_field_reports_a_wrong_kind_before_a_wrong_field() {
    let scratch = Scratch::new("field-kind");
    let (mut fixture, _) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Magic)
        .expect("applying the mutation");
    let refused = fixture
        .assert_rejected_field(DictErrorKind::Crc, "total_len")
        .err();
    match refused {
        Some(FixtureError::WrongKind {
            expected, actual, ..
        }) => {
            assert_eq!(expected, DictErrorKind::Crc);
            assert_eq!(actual, DictErrorKind::MagicMismatch);
        }
        other => panic!("a wrong expectation must be reported as the wrong kind: {other:?}"),
    }
}

#[test]
fn test_dict_error_kind_projects_the_variant_and_the_field() {
    let bounds = DictError::LengthOutOfRange {
        field: "word_off",
        value: 4_294_967_296,
    };
    assert_eq!(DictErrorKind::of(&bounds), DictErrorKind::LengthOutOfRange);
    assert_eq!(DictErrorKind::field(&bounds), Some("word_off"));

    // The kinds that name no field answer `None` rather than inventing one, which is what
    // lets a case tell "refused for another range" from "refused for another reason".
    let cases = [
        DictError::MagicMismatch,
        DictError::FormatVersion { found: 2 },
        DictError::Crc {
            expected: 1,
            actual: 2,
        },
        DictError::Fst(String::from("bad table")),
    ];
    for error in cases {
        assert_eq!(DictErrorKind::field(&error), None, "{error}");
    }
}

#[test]
fn test_user_store_fixture_places_the_two_shapes_of_damage() {
    let scratch = Scratch::new("store");
    let interrupted =
        UserStoreFixture::create(&scratch.path("interrupted"), UserStoreMutation::Interrupted)
            .expect("placing an interrupted store");
    assert_eq!(interrupted.mutation(), UserStoreMutation::Interrupted);
    assert_eq!(
        interrupted
            .path()
            .file_name()
            .and_then(|name| name.to_str()),
        Some(USER_STORE_NAME)
    );
    assert_eq!(
        fs::metadata(interrupted.path()).expect("stat").len(),
        0,
        "the file holds nothing"
    );
    // The loader opens it: the embedded store initialises a zero-length file rather than
    // reporting it, which is why the recovery pass reads the length before it opens it.
    interrupted
        .assert_opened()
        .expect("a zero-length file is initialised, not refused");
    let wrong = interrupted.assert_refused(DictErrorKind::Io).err();
    assert!(
        matches!(&wrong, Some(FixtureError::WrongVerdict { .. })),
        "{wrong:?}"
    );

    let damaged = UserStoreFixture::create(&scratch.path("garbage"), UserStoreMutation::Garbage)
        .expect("placing a damaged store");
    assert_eq!(damaged.mutation(), UserStoreMutation::Garbage);
    assert_eq!(
        fs::metadata(damaged.path()).expect("stat").len(),
        GARBAGE_BYTES as u64,
        "the file holds the whole pattern"
    );
    damaged
        .assert_refused(DictErrorKind::Io)
        .expect("bytes that are not a store are refused");
    let opened = damaged.assert_opened().err();
    assert!(
        matches!(&opened, Some(FixtureError::WrongVerdict { .. })),
        "{opened:?}"
    );
    let kind = damaged.assert_refused(DictErrorKind::MagicMismatch).err();
    match kind {
        Some(FixtureError::WrongKind { actual, .. }) => assert_eq!(actual, DictErrorKind::Io),
        other => panic!("the cause must be reported: {other:?}"),
    }
}

#[test]
fn test_garbage_bytes_are_reproducible_and_never_all_zero() {
    assert!(garbage(0).is_empty(), "no bytes for no length");
    let bytes = garbage(GARBAGE_BYTES);
    assert_eq!(
        bytes.len(),
        GARBAGE_BYTES,
        "the pattern is as long as asked"
    );
    assert_eq!(
        bytes,
        garbage(GARBAGE_BYTES),
        "the pattern is reproducible, so a case can compare the file it placed"
    );
    assert!(
        bytes.iter().any(|byte| *byte != 0),
        "an all-zero file is the one shape a store might mistake for a fresh one"
    );
}

#[test]
fn test_sha256_matches_the_known_empty_digest_and_refuses_a_missing_file() {
    let scratch = Scratch::new("digest");
    let empty = scratch.path("empty");
    fs::write(&empty, b"").expect("writing an empty file");
    assert_eq!(
        sha256(&empty).expect("hashing the empty file"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "the digest of an empty input is the published vector"
    );

    let one = scratch.path("one");
    fs::write(&one, b"a").expect("writing a file");
    let first = sha256(&one).expect("hashing it");
    assert_eq!(first.len(), 64, "the digest is written as hexadecimal");
    fs::write(&one, b"b").expect("writing other bytes");
    assert_ne!(
        first,
        sha256(&one).expect("hashing it"),
        "different bytes hash differently"
    );

    let missing = sha256(&scratch.path("nothing")).err();
    assert!(
        matches!(&missing, Some(FixtureError::Io { .. })),
        "{missing:?}"
    );
}
