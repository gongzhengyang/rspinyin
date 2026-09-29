//! The mutation matrix: one case per way of breaking a container, with the kind of error and
//! the field each one has to be refused on, plus the sweeps that check the container is
//! covered end to end and that nothing panics.
//!
//! This module is where a reader asking "what is covered" should start. The fixture's own
//! mechanics -- the scratch copy, the refusals the harness raises, the user store -- are in
//! the sibling module.
//!
//! Nothing here reads the clock, the environment, a display server or a real dictionary: the
//! container is built by the shared support module with the real writer. The one case that
//! touches `data/compiled/` reads the compiled dictionary when a build has produced one and
//! asserts the directory is unchanged either way.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ime_dict::format::{
    ENTRY_SIZE, FORMAT_VERSION, HEADER_SIZE, SECTION_COUNT, SECTION_ENTRY_SIZE, SectionKind,
    reader::{Reader, Verify},
};
use ime_dict::fst_index::FstLexicon;

use super::mutate::{FILE_CRC_OFFSET, SECTION_CRC_FIELD, garbage, named_span};
use super::support::{
    Scratch, Source, compiled_dictionary, compiled_dir, dir_names, fixture, source,
};
use super::{DictErrorKind, DictFixture, DictMutation, FixtureError, sha256};

/// The eight mutations the channel was first specified to cover.
///
/// They are the baseline set, and the matrix has grown past them. The case below asserts none
/// of them has been dropped: a shape that was once covered and is not any more is exactly the
/// regression this list exists to catch.
const BASELINE_MUTATIONS: [&str; 8] = [
    "magic",
    "format-version",
    "truncate",
    "section-crc",
    "entry-length",
    "entry-offset",
    "wordlist-range",
    "empty-file",
];

/// One mutation, the kind of refusal it has to produce, and the field that refusal has to
/// name.
///
/// The field is what tells one bounds failure from another: every length and offset failure
/// shares `DictError::LengthOutOfRange`, so the variant alone cannot say whether the entry
/// count or a section length was the thing that left its range.
struct Case {
    /// The mutation to apply.
    mutation: DictMutation,
    /// The kind of error the refusal has to be.
    kind: DictErrorKind,
    /// The field the refusal has to name, for the kinds that name one.
    field: Option<&'static str>,
}

/// The cases whose parameters do not depend on the container they are applied to.
///
/// These are the ones a compiled dictionary can be swept with as well as the container the
/// support module builds: every value here is a constant, not an offset read out of a
/// particular file.
fn fixed_cases() -> Vec<Case> {
    vec![
        Case {
            mutation: DictMutation::Magic,
            kind: DictErrorKind::MagicMismatch,
            field: None,
        },
        Case {
            mutation: DictMutation::FormatVersion {
                version: FORMAT_VERSION + 1,
            },
            kind: DictErrorKind::FormatVersion,
            field: None,
        },
        Case {
            // The header promises the file's own length, and the file is now shorter.
            mutation: DictMutation::Truncate { at: HEADER_SIZE },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("total_len"),
        },
        Case {
            mutation: DictMutation::SectionCrc {
                kind: SectionKind::Fst,
            },
            kind: DictErrorKind::Crc,
            field: None,
        },
        Case {
            mutation: DictMutation::EmptyFile,
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("file_len"),
        },
        Case {
            // A length field carrying a value no file could have.
            mutation: DictMutation::DeclaredLength { len: u64::MAX },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("total_len"),
        },
        Case {
            // A section length whose end cannot even be computed.
            mutation: DictMutation::SectionLength {
                kind: SectionKind::Fst,
                len: u64::MAX,
            },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("section_len"),
        },
        Case {
            // A section claiming to start inside the header, before the first section of
            // the layout: the direction the length mutations cannot reach.
            mutation: DictMutation::SectionOffset {
                kind: SectionKind::Fst,
                offset: HEADER_SIZE as u64,
            },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("section_offset"),
        },
        Case {
            // The count at the top of its width: the tables it describes cannot be that
            // long, and the length the loader computes from it does not fit a `usize` on a
            // 32-bit target either.
            mutation: DictMutation::EntryCount { count: u32::MAX },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("entries_len"),
        },
        Case {
            // An empty pool: no record can be served by it.
            mutation: DictMutation::StrPoolTruncate { keep: 0 },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("word_off"),
        },
        Case {
            // The mirror of `Truncate`: the file has a byte its header does not admit to.
            mutation: DictMutation::Append { bytes: 1 },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("total_len"),
        },
        Case {
            // A record that names no word at all.
            mutation: DictMutation::EntryLength { index: 0, len: 0 },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("word_len"),
        },
        Case {
            // An offset at the top of its width, so the range it names overflows before it
            // is widened.
            mutation: DictMutation::EntryOffset {
                index: 0,
                off: u32::MAX,
            },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("word_off"),
        },
    ]
}

/// The cases whose parameters come from the container: a key, a byte count, an offset.
fn source_cases(source: &Source) -> Vec<Case> {
    vec![
        Case {
            // The key survives the rebuild; only its range moves past the word list.
            mutation: DictMutation::WordlistRange {
                key: String::from(source.key),
            },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("wordlist_start"),
        },
        Case {
            // A pool cut two bytes short of the first record's end: the container still
            // parses, so only a conversion can notice.
            mutation: DictMutation::StrPoolTruncate { keep: 2 },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("word_off"),
        },
        Case {
            // A record one byte past the pool's end, which is the smallest offset the
            // mutation is defined to accept.
            mutation: DictMutation::EntryOffset {
                index: 0,
                off: source.pool_len + 1,
            },
            kind: DictErrorKind::LengthOutOfRange,
            field: Some("word_off"),
        },
    ]
}

/// Every case the matrix covers.
fn cases(source: &Source) -> Vec<Case> {
    let mut all = fixed_cases();
    all.extend(source_cases(source));
    all
}

/// Whether `at` is one of the container's checksum fields.
///
/// A mutation that patches a record has to repair the checksums that cover it, so those bytes
/// are allowed to differ. Nothing else is.
fn is_checksum_field(at: usize) -> bool {
    if (FILE_CRC_OFFSET..FILE_CRC_OFFSET + 4).contains(&at) {
        return true;
    }
    (0..SECTION_COUNT).any(|index| {
        let crc = HEADER_SIZE + index * SECTION_ENTRY_SIZE + SECTION_CRC_FIELD;
        (crc..crc + 4).contains(&at)
    })
}

/// Applies every case to `fixture` and asserts each one is refused as the case names it.
///
/// The fixture is driven through the same public surface a case outside this module would
/// use, so the sweep is an assertion about the channel rather than about an internal helper.
fn sweep(fixture: &mut DictFixture, cases: &[Case]) {
    for case in cases {
        let mutation = &case.mutation;
        fixture
            .mutate(mutation.clone())
            .unwrap_or_else(|error| panic!("{mutation:?}: {error}"));
        match case.field {
            Some(field) => fixture
                .assert_rejected_field(case.kind, field)
                .unwrap_or_else(|error| panic!("{mutation:?}: {error}")),
            None => fixture
                .assert_rejected(case.kind)
                .unwrap_or_else(|error| panic!("{mutation:?}: {error}")),
        }
    }
}

#[test]
fn test_every_mutation_is_refused_with_the_kind_and_field_its_case_names() {
    let scratch = Scratch::new("matrix");
    let (mut fixture, source) = fixture(&scratch);
    let origin = scratch.path("base.dict");
    let digest = sha256(&origin).expect("hashing the source");
    let cases = cases(&source);
    assert!(
        cases.len() >= BASELINE_MUTATIONS.len(),
        "the matrix covers every shape of the baseline, and the ones it left out"
    );

    sweep(&mut fixture, &cases);

    assert_eq!(
        sha256(&origin).expect("hashing the source"),
        digest,
        "a whole sweep of mutations leaves the source byte for byte as it was"
    );
    assert_eq!(
        scratch.names(),
        vec!["base.dict", "copy.dict"],
        "the sweep leaves the source and its copy and nothing else"
    );
}

#[test]
fn test_the_matrix_covers_every_variant_and_keeps_the_baseline_ones() {
    let source = source();
    let covered: Vec<&'static str> = cases(&source)
        .iter()
        .map(|case| case.mutation.name())
        .collect();
    for variant in DictMutation::ALL {
        assert!(
            covered.contains(&variant.name()),
            "{} has no case, so nothing has proved the loader survives it",
            variant.name()
        );
    }
    let mut distinct = covered.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        DictMutation::ALL.len(),
        "the matrix covers the vocabulary rather than a subset of it: {distinct:?}"
    );
    for baseline in BASELINE_MUTATIONS {
        assert!(
            covered.contains(&baseline),
            "the {baseline} mutation is part of the baseline coverage and must not be dropped"
        );
    }
}

#[test]
fn test_a_declared_length_that_is_not_the_file_length_is_refused() {
    let scratch = Scratch::new("declared-length");
    let (mut fixture, source) = fixture(&scratch);
    // Both directions of the same check, and the field at the top of its width. A declared
    // length below the file's own is the shape a resumed download leaves behind; one above
    // it is a torn write; `u64::MAX` is the field overflowing.
    let cases = [
        DictMutation::DeclaredLength {
            len: source.image.len() as u64 - 1,
        },
        DictMutation::DeclaredLength {
            len: source.image.len() as u64 + 1,
        },
        DictMutation::DeclaredLength { len: u64::MAX },
    ];
    for mutation in cases {
        fixture
            .mutate(mutation.clone())
            .unwrap_or_else(|error| panic!("{mutation:?}: {error}"));
        fixture
            .assert_rejected_field(DictErrorKind::LengthOutOfRange, "total_len")
            .unwrap_or_else(|error| panic!("{mutation:?}: {error}"));
    }
}

#[test]
fn test_an_appended_byte_is_refused_as_a_file_longer_than_it_declares() {
    let scratch = Scratch::new("append");
    let (mut fixture, source) = fixture(&scratch);
    fixture
        .mutate(DictMutation::Append { bytes: 1 })
        .expect("applying the mutation");
    assert_eq!(
        fs::metadata(fixture.path()).expect("stat").len(),
        source.image.len() as u64 + 1,
        "the file grew and the header was not told"
    );
    fixture
        .assert_rejected_field(DictErrorKind::LengthOutOfRange, "total_len")
        .expect("a file longer than it declares must be caught");
}

#[test]
fn test_a_section_length_and_offset_that_leave_the_layout_are_refused() {
    let scratch = Scratch::new("section-fields");
    let (mut fixture, _) = fixture(&scratch);
    for kind in [
        SectionKind::Fst,
        SectionKind::Entries,
        SectionKind::StrPool,
        SectionKind::Unigram,
        SectionKind::WordList,
    ] {
        let length = DictMutation::SectionLength {
            kind,
            len: u64::MAX,
        };
        fixture
            .mutate(length.clone())
            .unwrap_or_else(|error| panic!("{length:?}: {error}"));
        fixture
            .assert_rejected_field(DictErrorKind::LengthOutOfRange, "section_len")
            .unwrap_or_else(|error| panic!("{length:?}: {error}"));

        // An offset that is aligned but sits inside the header: the layout's first section
        // starts after the table, so this one is before every section there can be.
        let offset = DictMutation::SectionOffset {
            kind,
            offset: HEADER_SIZE as u64,
        };
        fixture
            .mutate(offset.clone())
            .unwrap_or_else(|error| panic!("{offset:?}: {error}"));
        fixture
            .assert_rejected_field(DictErrorKind::LengthOutOfRange, "section_offset")
            .unwrap_or_else(|error| panic!("{offset:?}: {error}"));
    }

    // The section the container does not have is the refusal that belongs to the fixture
    // rather than to the loader: there is no field to write.
    for mutation in [
        DictMutation::SectionLength {
            kind: SectionKind::Bigram,
            len: u64::MAX,
        },
        DictMutation::SectionOffset {
            kind: SectionKind::Bigram,
            offset: HEADER_SIZE as u64,
        },
    ] {
        assert!(
            matches!(
                fixture.mutate(mutation.clone()).err(),
                Some(FixtureError::Inapplicable { .. })
            ),
            "{mutation:?} names a section the container does not have"
        );
    }
}

#[test]
fn test_an_entry_count_the_tables_do_not_match_is_refused() {
    let scratch = Scratch::new("entry-count");
    let (mut fixture, _) = fixture(&scratch);
    // The container holds two records, so a count of three describes tables that are not
    // there -- and a count at the top of the width describes tables that cannot be.
    for count in [3u32, u32::MAX] {
        fixture
            .mutate(DictMutation::EntryCount { count })
            .unwrap_or_else(|error| panic!("count={count}: {error}"));
        fixture
            .assert_rejected_field(DictErrorKind::LengthOutOfRange, "entries_len")
            .unwrap_or_else(|error| panic!("count={count}: {error}"));
    }
    assert!(
        matches!(
            fixture.mutate(DictMutation::EntryCount { count: 2 }).err(),
            Some(FixtureError::InertMutation { .. })
        ),
        "the count the container already carries is not a case"
    );
}

#[test]
fn test_a_shortened_string_pool_is_caught_when_a_record_becomes_a_word() {
    let scratch = Scratch::new("strpool");
    let (mut fixture, source) = fixture(&scratch);
    fixture
        .mutate(DictMutation::StrPoolTruncate { keep: 2 })
        .expect("applying the mutation");
    // The container parses: nothing in the layout says how long the pool has to be, which is
    // exactly why the refusal has to come from the conversion rather than from the load.
    let lexicon = FstLexicon::load(fixture.path()).expect("the container is otherwise valid");
    assert_eq!(lexicon.entry_count(), 2, "the records are all still there");
    fixture
        .assert_rejected_field(DictErrorKind::LengthOutOfRange, "word_off")
        .expect("a record naming text the pool lost must be caught");

    // The second record names a range past the cut too, and the walk reaches it.
    let reader = Reader::open(fixture.path()).expect("reading the container");
    assert!(
        reader.entry(1).is_ok(),
        "the record decodes; only the pool is short"
    );

    // A cut at or past every record's end leaves a usable container, so it is refused as
    // inert rather than reported as a case.
    assert!(
        matches!(
            fixture
                .mutate(DictMutation::StrPoolTruncate {
                    keep: source.pool_len as usize,
                })
                .err(),
            Some(FixtureError::InertMutation { .. })
        ),
        "a cut the records survive proves nothing"
    );
}

#[test]
fn test_a_single_flipped_byte_anywhere_in_the_container_is_refused() {
    let scratch = Scratch::new("flip");
    let (fixture, source) = fixture(&scratch);
    // Every byte of the container is covered by a check -- the magic, the version, the
    // header's own numbers, the section table's, the reserved area, and the checksums over
    // the body -- so a flip anywhere has to be refused rather than reach the read path.
    for at in 0..source.image.len() {
        let mut image = source.image.clone();
        image[at] ^= 0xff;
        fs::write(fixture.path(), &image).expect("writing the damaged container");
        let parsed = Reader::parse(image, Verify::Full);
        assert!(parsed.is_err(), "a flipped byte at {at} must be refused");
        let mapped = FstLexicon::load(fixture.path());
        assert!(
            mapped.is_err(),
            "a flipped byte at {at} must not become a lexicon"
        );
    }
}

#[test]
fn test_bytes_that_are_not_a_container_are_never_accepted() {
    let scratch = Scratch::new("noise");
    let path = scratch.path("noise.dict");
    // The lengths straddle the two structural thresholds: the 64-byte header and the 208-byte
    // first section offset.
    for len in [0usize, 1, 63, 64, 207, 208, 209, 512, 4096] {
        fs::write(&path, garbage(len)).expect("writing the bytes");
        assert!(
            Reader::open(&path).is_err(),
            "{len} bytes of noise must not parse as a container"
        );
        assert!(
            FstLexicon::load(&path).is_err(),
            "{len} bytes of noise must not become a lexicon"
        );
    }
}

#[test]
fn test_no_mutation_panics_the_harness_or_the_loader() {
    let scratch = Scratch::new("no-panic");
    let (mut fixture, source) = fixture(&scratch);
    for case in cases(&source) {
        let mutation = case.mutation.clone();
        // A panic is what a fixture with an unchecked offset would produce, and it would
        // abort the test binary rather than fail one case. Both halves of the drive are
        // inside the guard: the byte work that builds the file, and the loader that reads it
        // back. A signal is *not* covered here: a mapping faulted past the end of its file
        // raises `SIGBUS`, which no unwind can take back, and that path belongs to the crash
        // handler.
        let driven = catch_unwind(AssertUnwindSafe(|| {
            fixture.mutate(mutation.clone())?;
            fixture.refusal().map(|_| ())
        }));
        match driven {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("{mutation:?}: {error}"),
            Err(_) => panic!("{mutation:?} panicked the harness or the loader"),
        }
    }
}

#[test]
fn test_every_patched_mutation_changes_only_its_own_field_and_the_checksums() {
    let scratch = Scratch::new("span");
    let (mut fixture, source) = fixture(&scratch);
    let mut patched = 0usize;
    for case in cases(&source) {
        let mutation = case.mutation.clone();
        let span = named_span(&mutation, &source.image)
            .unwrap_or_else(|error| panic!("{mutation:?}: {error}"));
        let Some(span) = span else {
            continue;
        };
        patched += 1;
        fixture
            .mutate(mutation.clone())
            .unwrap_or_else(|error| panic!("{mutation:?}: {error}"));
        let after = fs::read(fixture.path()).expect("reading the copy");
        assert_eq!(
            after.len(),
            source.image.len(),
            "{mutation:?} names a field, so it may not change the file's length"
        );
        for (at, (before, now)) in source.image.iter().zip(after.iter()).enumerate() {
            if before == now {
                continue;
            }
            assert!(
                span.contains(&at) || is_checksum_field(at),
                "{mutation:?} changed the byte at {at}, which is outside {span:?} and is not \
                 one of the container's checksum fields"
            );
        }
    }
    assert!(
        patched >= 9,
        "the diff check must not be vacuous: only {patched} mutations name a field"
    );
}

#[test]
fn test_the_mutations_that_change_the_length_leave_the_rest_of_the_image_alone() {
    let scratch = Scratch::new("resize");
    let (mut fixture, source) = fixture(&scratch);
    let at = source.image.len() - 8;

    fixture
        .mutate(DictMutation::Truncate { at })
        .expect("applying the mutation");
    let truncated = fs::read(fixture.path()).expect("reading the copy");
    assert_eq!(
        truncated,
        source.image[..at],
        "a truncation cuts the tail and changes nothing before it"
    );

    fixture
        .mutate(DictMutation::Append { bytes: 4 })
        .expect("applying the mutation");
    let appended = fs::read(fixture.path()).expect("reading the copy");
    assert_eq!(
        &appended[..source.image.len()],
        source.image.as_slice(),
        "an append adds a tail and changes nothing before it"
    );
    assert_eq!(appended.len(), source.image.len() + 4);
    assert!(
        appended[source.image.len()..].iter().all(|byte| *byte == 0),
        "the tail is the zero bytes the mutation asked for"
    );

    fixture
        .mutate(DictMutation::EmptyFile)
        .expect("applying the mutation");
    assert!(
        fs::read(fixture.path())
            .expect("reading the copy")
            .is_empty(),
        "an emptied file holds nothing"
    );
}

#[test]
fn test_named_span_names_the_field_each_patched_mutation_writes() {
    let source = source();
    let constants = [
        (DictMutation::Magic, 0usize..4usize),
        (
            DictMutation::FormatVersion {
                version: FORMAT_VERSION + 1,
            },
            4..6,
        ),
        (DictMutation::DeclaredLength { len: u64::MAX }, 16..24),
        (DictMutation::EntryCount { count: 3 }, 28..32),
        (
            DictMutation::SectionOffset {
                kind: SectionKind::Fst,
                offset: 8,
            },
            HEADER_SIZE + 8..HEADER_SIZE + 16,
        ),
        (
            DictMutation::SectionLength {
                kind: SectionKind::Fst,
                len: u64::MAX,
            },
            HEADER_SIZE + 16..HEADER_SIZE + 24,
        ),
    ];
    for (mutation, expected) in constants {
        assert_eq!(
            named_span(&mutation, &source.image).expect("the container is readable"),
            Some(expected),
            "{mutation:?}"
        );
    }

    // The two record mutations name a field inside `ENTRIES`, whose offset the container
    // chooses rather than the layout, so what is checkable is the shape of the record.
    let length = |index| DictMutation::EntryLength { index, len: 0 };
    let offset = |index| DictMutation::EntryOffset { index, off: 0 };
    let word_len = named_span(&length(0), &source.image)
        .expect("the container is readable")
        .expect("the mutation names a range");
    assert_eq!(word_len.len(), 2, "`word_len` is two bytes wide");
    let second = named_span(&length(1), &source.image)
        .expect("the container is readable")
        .expect("the mutation names a range");
    assert_eq!(
        second.start,
        word_len.start + ENTRY_SIZE,
        "records follow one another at the record width"
    );
    let word_off = named_span(&offset(0), &source.image)
        .expect("the container is readable")
        .expect("the mutation names a range");
    assert_eq!(word_off.len(), 4, "`word_off` is four bytes wide");
    assert_eq!(
        word_off.start + 4,
        word_len.start,
        "`word_off` is the field before `word_len` in the record"
    );
}

#[test]
fn test_named_span_answers_none_for_a_mutation_that_is_not_a_patch() {
    let source = source();
    let cases = [
        DictMutation::Truncate { at: 8 },
        DictMutation::EmptyFile,
        DictMutation::Append { bytes: 1 },
        DictMutation::WordlistRange {
            key: String::from(source.key),
        },
        DictMutation::StrPoolTruncate { keep: 2 },
    ];
    for mutation in cases {
        assert_eq!(
            named_span(&mutation, &source.image).expect("the container is readable"),
            None,
            "{mutation:?} rebuilds the container or changes its length"
        );
    }
    // A mutation that names a section the container does not have is reported rather than
    // answered with a range that would not describe it.
    let absent = DictMutation::SectionOffset {
        kind: SectionKind::Bigram,
        offset: 8,
    };
    assert!(
        matches!(
            named_span(&absent, &source.image),
            Err(FixtureError::Inapplicable { .. })
        ),
        "a range cannot be named for a section that is not there"
    );
}

#[test]
fn test_the_compiled_artifact_is_read_only_and_its_directory_gains_nothing() {
    let dir = compiled_dir();
    let before = dir_names(&dir);
    let artifact = compiled_dictionary();

    // The compiled dictionary is a build artifact rather than a source, so a checkout that
    // has not run the compiler does not have one. The directory assertion below holds either
    // way; the sweep holds whenever the artifact is there.
    if artifact.is_file() {
        let digest = sha256(&artifact).expect("hashing the compiled dictionary");
        let scratch = Scratch::new("artifact");
        let mut fixture = DictFixture::from_compiled(&artifact, &scratch.path("copy.dict"))
            .expect("a compiled dictionary is a container the fixture can copy");
        sweep(&mut fixture, &fixed_cases());
        drop(fixture);
        assert_eq!(
            sha256(&artifact).expect("hashing it again"),
            digest,
            "the compiled dictionary is read and never written"
        );
    }

    assert_eq!(
        dir_names(&dir),
        before,
        "no mutation may leave a copy, a temporary or a quarantine file next to the \
         compiled dictionary"
    );
}
