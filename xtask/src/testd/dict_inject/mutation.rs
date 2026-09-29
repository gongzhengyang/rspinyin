//! The vocabulary of container damage: one named mutation per field, and the refusal each
//! one has to produce.
//!
//! Responsibility: name one way of breaking a container per variant, the step at which that
//! break has to be caught, and the kind of error that step has to raise. It is a value
//! module: it writes no byte, opens no file and knows no offset.
//!
//! Boundaries: [`super::mutate`] turns a variant into an image, [`super::DictFixture`]
//! applies it and reads the result back, and [`super::error`] names what the harness itself
//! refuses to do. Splitting the vocabulary out of the fixture is what keeps the fixture
//! about copies, steps and assertions.
//!
//! # The shapes a damaged file arrives in
//!
//! The variants are not a list of fields; they are the list of *ways a file becomes
//! unusable*, and a shape that no variant produces is a shape nothing here has proved the
//! loader survives. Between them they cover: a wrong magic; a version this build does not
//! read; a length field carrying a value no file could have; a checksum that no longer
//! matches its bytes; an offset or a length that leaves the image, in either direction --
//! before the section it claims to describe or past the end of the file; a string pool cut
//! shorter than the records that name it; an entry count that disagrees with the tables it
//! describes; and a file that is empty, torn short or longer than it declares.

use ime_dict::format::SectionKind;
use ime_types::DictError;

/// One way of breaking a container, named after the field it breaks.
///
/// Every variant is a change a damaged disk, a truncated download or a hand edit could
/// produce, and every one of them has to end in a typed error rather than a panic or an
/// out-of-bounds read.
///
/// A variant that patches bytes patches exactly one field, so that the refusal can only be
/// about the field the case named. The two that rebuild the container, and the three that
/// change its length, say so in their own documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DictMutation {
    /// Overwrites the first byte of the four-byte magic.
    ///
    /// The header is deliberately outside the file checksum -- a checksum cannot cover its
    /// own storage -- so this one needs no checksum work to be the only thing wrong.
    Magic,
    /// Writes `version` into the header's `format_version` field.
    ///
    /// The shape a dictionary written by a newer build arrives in. A version the loader
    /// already reads is refused by [`super::DictFixture::mutate`] as inert.
    FormatVersion {
        /// The version to write.
        version: u16,
    },
    /// Cuts the image down to `at` bytes, leaving the header's `total_len` describing the
    /// file as it was.
    ///
    /// This is a torn write: the length the header declares and the length the file has no
    /// longer agree, which is what the load-time check catches. Truncating *after* the file
    /// is mapped is the case that raises `SIGBUS`, and that one belongs to the crash
    /// handler.
    Truncate {
        /// The byte count to leave.
        at: usize,
    },
    /// Flips one byte inside the named section, leaving its recorded checksum alone.
    ///
    /// This mutation *is* the checksum break: it is the one that answers "would a corrupted
    /// payload be caught", and it must not have its checksum recomputed.
    SectionCrc {
        /// The section whose last byte is flipped.
        kind: SectionKind,
    },
    /// Writes `len` into the `word_len` field of the entry at `index`.
    ///
    /// A value inside `1..=MAX_WORD_LEN` is refused by [`super::DictFixture::mutate`] as
    /// inert: the point of the mutation is a length the container's own contract forbids.
    EntryLength {
        /// Which record to patch.
        index: u32,
        /// The length to write.
        len: u16,
    },
    /// Writes `off` into the `word_off` field of the entry at `index`.
    ///
    /// The offset has to be at or past the end of the string pool. An offset *inside* the
    /// pool is left to [`DictMutation::EntryLength`], which is the mutation that varies the
    /// other half of the same range.
    EntryOffset {
        /// Which record to patch.
        index: u32,
        /// The offset to write.
        off: u32,
    },
    /// Rewrites the index so that `key` names a word-list range starting past the end of
    /// the word list.
    ///
    /// The container is rebuilt with the real `fst` writer and the real container writer
    /// for this one, because an FST packs its keys into shared states and a single value is
    /// not addressable on its own; the rebuild is also what leaves every checksum correct.
    WordlistRange {
        /// The key whose range is moved past the end.
        key: String,
    },
    /// Writes an empty file.
    ///
    /// The residue of a write that was interrupted before the first byte reached the disk.
    EmptyFile,
    /// Writes `len` into the header's `total_len` field, leaving the file's own size alone.
    ///
    /// Two shapes arrive through this field, and they are the same check from either side.
    /// A value no file could have -- the field is eight bytes wide and the file is not --
    /// is the length field overflowing; a value *smaller* than the file is a file with more
    /// bytes than it admits to, which is what a resumed or appended download leaves behind.
    /// A value equal to the file's length is refused as inert.
    DeclaredLength {
        /// The length to declare.
        len: u64,
    },
    /// Writes `len` into the section-table `len` field of the named section.
    ///
    /// The offset is left where it was, so what the table now describes is a section
    /// running past the end of the file -- or, for a length near the top of the field, one
    /// whose end cannot be computed at all. A section the container does not have, and a
    /// length the section already carries, are refused as inapplicable and inert.
    SectionLength {
        /// The section whose length is rewritten.
        kind: SectionKind,
        /// The length to write.
        len: u64,
    },
    /// Writes `offset` into the section-table `offset` field of the named section.
    ///
    /// The direction this mutation covers is the one the length mutations cannot reach: a
    /// section that claims to *start* somewhere it cannot, inside the header or before the
    /// first section of the layout. A section the container does not have, and an offset
    /// the section already carries, are refused as inapplicable and inert.
    SectionOffset {
        /// The section whose offset is rewritten.
        kind: SectionKind,
        /// The offset to write.
        offset: u64,
    },
    /// Writes `count` into the header's `entry_count` field.
    ///
    /// The field is the container's own statement of how many records `ENTRIES` and
    /// `UNIGRAM` hold, and both tables are sized against it. It is also the field a hostile
    /// file uses to make the loader compute a length that does not fit: a count at the top
    /// of the width makes the table length overflow before a single record is read. A count
    /// the container already carries is refused as inert.
    EntryCount {
        /// The count to write.
        count: u32,
    },
    /// Cuts the string pool down to `keep` bytes, leaving every record's `word_off` where
    /// it was.
    ///
    /// Nothing in the layout says how long the pool has to be, so this container parses:
    /// the length of the pool is checked against a record only when that record is
    /// converted. What the mutation produces is therefore a file whose records name text
    /// the pool no longer holds, and the refusal arrives one step later than the load. The
    /// container is rebuilt with the real writer, which is what leaves the checksums
    /// correct; a cut at or past every record's end is refused as inert, because a cut the
    /// records survive proves nothing about the pool.
    StrPoolTruncate {
        /// The byte count to leave in the pool.
        keep: usize,
    },
    /// Appends `bytes` zero bytes to the image, leaving the header's `total_len` alone.
    ///
    /// The mirror of [`DictMutation::Truncate`]: the file has more bytes than it declares,
    /// which is the shape a write that was resumed past its end leaves behind. A zero-byte
    /// append is refused as inert.
    Append {
        /// How many bytes to add.
        bytes: usize,
    },
}

impl DictMutation {
    /// Every variant, with a placeholder payload where the variant carries one.
    ///
    /// A case uses this to assert its matrix covers the vocabulary rather than a count it
    /// typed by hand: a variant added without a case would otherwise be invisible, and an
    /// uncovered shape is a shape nothing has proved the loader survives.
    pub const ALL: [DictMutation; 14] = [
        DictMutation::Magic,
        DictMutation::FormatVersion { version: 0 },
        DictMutation::Truncate { at: 0 },
        DictMutation::SectionCrc {
            kind: SectionKind::Fst,
        },
        DictMutation::EntryLength { index: 0, len: 0 },
        DictMutation::EntryOffset { index: 0, off: 0 },
        DictMutation::WordlistRange { key: String::new() },
        DictMutation::EmptyFile,
        DictMutation::DeclaredLength { len: 0 },
        DictMutation::SectionLength {
            kind: SectionKind::Fst,
            len: 0,
        },
        DictMutation::SectionOffset {
            kind: SectionKind::Fst,
            offset: 0,
        },
        DictMutation::EntryCount { count: 0 },
        DictMutation::StrPoolTruncate { keep: 0 },
        DictMutation::Append { bytes: 0 },
    ];

    /// The name the mutation is reported under.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::FormatVersion { .. } => "format-version",
            Self::Truncate { .. } => "truncate",
            Self::SectionCrc { .. } => "section-crc",
            Self::EntryLength { .. } => "entry-length",
            Self::EntryOffset { .. } => "entry-offset",
            Self::WordlistRange { .. } => "wordlist-range",
            Self::EmptyFile => "empty-file",
            Self::DeclaredLength { .. } => "declared-length",
            Self::SectionLength { .. } => "section-length",
            Self::SectionOffset { .. } => "section-offset",
            Self::EntryCount { .. } => "entry-count",
            Self::StrPoolTruncate { .. } => "strpool-truncate",
            Self::Append { .. } => "append",
        }
    }

    /// The step at which the container this mutation produces has to be refused.
    ///
    /// [`super::DictFixture::assert_rejected`] compares this against the step that actually
    /// raised the refusal, so a mutation that named the wrong step fails instead of passing
    /// on an error it never meant to test.
    pub fn refusal_point(&self) -> RefusalPoint {
        match self {
            Self::Magic
            | Self::FormatVersion { .. }
            | Self::Truncate { .. }
            | Self::SectionCrc { .. }
            | Self::EmptyFile
            | Self::DeclaredLength { .. }
            | Self::SectionLength { .. }
            | Self::SectionOffset { .. }
            | Self::EntryCount { .. }
            | Self::Append { .. } => RefusalPoint::Load,
            Self::EntryLength { .. } => RefusalPoint::RecordDecode,
            Self::EntryOffset { .. } | Self::StrPoolTruncate { .. } => RefusalPoint::EntryToRef,
            Self::WordlistRange { .. } => RefusalPoint::Lookup,
        }
    }
}

/// The step a mutated container has to be refused at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalPoint {
    /// Opening the container: the header, the section table or a checksum.
    ///
    /// A container refused here never becomes a lexicon, which is what the load-time
    /// promise covers.
    Load,
    /// Decoding the record the entry table holds at an index.
    ///
    /// The container's own record validation runs here, so a `word_len` outside its
    /// contract never becomes a `DictEntry` at all.
    RecordDecode,
    /// Converting a decoded record into a word.
    ///
    /// A record that decoded cleanly can still name a range outside the string pool -- or
    /// the pool can be too short for a record that was always in contract -- and this is
    /// the step that checks the two against each other.
    EntryToRef,
    /// Reading the words of a key.
    ///
    /// An FST value is read here, so a word-list range that leaves its section is caught by
    /// the lookup rather than by the load.
    Lookup,
}

/// The `DictError` variants, as a value a case can name in an assertion.
///
/// `DictError` is neither `Clone` nor `PartialEq` -- it wraps `std::io::Error` -- so a case
/// cannot compare one against an expected value directly. This is the projection that lets
/// it: the variant, without the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictErrorKind {
    /// The four-byte magic is not the container's.
    MagicMismatch,
    /// The header's format version is not the one this build reads.
    FormatVersion,
    /// A length, an offset or an alignment disagrees with the layout.
    LengthOutOfRange,
    /// A checksum does not match the bytes it covers.
    Crc,
    /// The index section is not a valid FST.
    Fst,
    /// The file could not be read at all.
    Io,
}

impl DictErrorKind {
    /// Projects `error` onto its variant.
    pub fn of(error: &DictError) -> Self {
        match error {
            DictError::MagicMismatch => Self::MagicMismatch,
            DictError::FormatVersion { .. } => Self::FormatVersion,
            DictError::LengthOutOfRange { .. } => Self::LengthOutOfRange,
            DictError::Crc { .. } => Self::Crc,
            DictError::Fst(_) => Self::Fst,
            DictError::Io(_) => Self::Io,
        }
    }

    /// The field a bounds failure names, or `None` for any other kind of error.
    ///
    /// A case uses this to assert *which* range was refused: every length and offset
    /// failure shares one variant, so the variant alone cannot tell a broken entry count
    /// from a broken section length.
    pub fn field(error: &DictError) -> Option<&'static str> {
        match error {
            DictError::LengthOutOfRange { field, .. } => Some(*field),
            _ => None,
        }
    }
}

/// One refusal: what was raised, and at which step.
#[derive(Debug)]
pub struct Refusal {
    /// The step that raised it.
    pub point: RefusalPoint,
    /// The error itself, so a case can name the field that was out of range.
    pub error: DictError,
}
