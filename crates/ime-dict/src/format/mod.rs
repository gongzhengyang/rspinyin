//! The binary dictionary container, version 1.
//!
//! Responsibility: own the on-disk layout of `base.dict` and the primitives the
//! writer and the reader are both built from -- the 64-byte header, the section
//! table, the fixed-width [`DictEntry`] record, the word hash the unigram table is
//! keyed by, and the checksums that make the container self-verifying.
//!
//! Boundaries: this module owns the byte layout and nothing else. It performs no
//! filesystem access (that is [`writer`] and [`reader`]), knows nothing about
//! syllables, FSTs or weights, and every parse function is a pure function over a
//! borrowed slice that reports [`DictError`] instead of indexing out of range.
//!
//! # Layout
//!
//! ```text
//! offset  size  field
//!      0     4  magic           b"RSPD"
//!      4     2  format_version  1
//!      6     2  flags           bit0: a BIGRAM section is present (v1: never)
//!      8     4  header_size     64
//!     12     4  section_count   6
//!     16     8  total_len       file length in bytes
//!     24     4  file_crc32      CRC32 over [64, total_len)
//!     28     4  entry_count     number of word entries
//!     32    32  reserved        all zero
//!     64   144  section table   6 entries x 24 bytes
//!    208   ...  sections        ascending kind, each 8-byte aligned
//! ```
//!
//! Every integer is little-endian. `file_crc32` deliberately does not cover the
//! header: a checksum cannot cover its own storage.
//!
//! # Section table entry
//!
//! ```text
//!  0   4  kind   1=FST 2=ENTRIES 3=STRPOOL 4=UNIGRAM 5=BIGRAM 6=WORDLIST
//!  4   4  crc32  CRC32 of the section bytes
//!  8   8  offset byte offset in the file (8-byte aligned)
//! 16   8  len    section length in bytes (0 = the section is absent)
//! ```
//!
//! # Why `WORDLIST` is a section of its own
//!
//! Multi-key expansion (one word reachable under several syllable keys) would
//! inflate a key-ordered `ENTRIES` table by the expansion factor. Splitting the
//! key -> word mapping into `WORDLIST` keeps `ENTRIES` at one record per word and
//! puts the expansion cost on a 4-byte word-list pair instead of a 16-byte entry.

pub mod reader;
pub mod writer;

use ime_types::DictError;

/// Container magic, the first four bytes of every dictionary file.
pub const MAGIC: [u8; 4] = *b"RSPD";

/// Container format version this module reads and writes.
pub const FORMAT_VERSION: u16 = 1;

/// Size of the fixed header in bytes.
pub const HEADER_SIZE: usize = 64;

/// Number of section-table entries; the layout has no room for more.
pub const SECTION_COUNT: usize = 6;

/// Size of one section-table entry in bytes.
pub const SECTION_ENTRY_SIZE: usize = 24;

/// Size of the whole section table in bytes.
pub const SECTION_TABLE_SIZE: usize = SECTION_COUNT * SECTION_ENTRY_SIZE;

/// File offset of the first section: header plus section table, padded to a
/// multiple of [`SECTION_ALIGN`].
pub const SECTIONS_OFFSET: u64 = 208;

/// Alignment every section offset must satisfy.
pub const SECTION_ALIGN: u64 = 8;

/// `flags` bit 0: a `BIGRAM` section is present. Always clear in version 1.
pub const FLAG_BIGRAM_PRESENT: u16 = 0b0000_0001;

/// Every bit of `flags` defined by version 1.
pub const FLAG_MASK: u16 = FLAG_BIGRAM_PRESENT;

/// Upper bound on `DictEntry::word_len`, in UTF-8 bytes (32 Han characters).
pub const MAX_WORD_LEN: u16 = 96;

/// Upper bound on `DictEntry::syl_count`.
pub const MAX_SYL_COUNT: u8 = 16;

/// Upper bound on the number of words stored under one key. Surplus words are
/// dropped by the compiler and reported; the FST value reserves 24 bits for the
/// count, so the limit is a ranking decision rather than a format limit.
pub const MAX_WORDS_PER_KEY: u32 = 32;

/// Width in bits of the word count inside a packed FST value.
pub const FST_COUNT_BITS: u32 = 24;

/// Upper bound on the word-list start index a packed FST value can address.
///
/// The value is stored shifted left by [`FST_COUNT_BITS`], so the start index must
/// leave room for the count in the low bits. `1 << 40` would need 65 bits once shifted
/// and would silently wrap to zero, making the packed value indistinguishable from an
/// empty one — the bound is therefore one less than the field width suggests.
pub const MAX_FST_START: u64 = (1 << 40) - 1;

/// Word flag: a surname.
pub const FLAG_SURNAME: u8 = 0b0000_0001;

/// Word flag: a place name.
pub const FLAG_PLACE: u8 = 0b0000_0010;

/// Word flag: a domain term.
pub const FLAG_TERM: u8 = 0b0000_0100;

/// Word flag: a user word (never set by the offline compiler).
pub const FLAG_USER: u8 = 0b0000_1000;

/// Every bit of `DictEntry::flags` defined by version 1.
///
/// A record carrying a bit outside this mask is not rejected: the bit is a flag a later version
/// defined, and a word that carries one is still a word. The header's `flags` is the opposite
/// case and is still checked against [`FLAG_MASK`], because it describes the container's own
/// structure: a reader that does not understand a bit in it cannot parse the file safely.
pub const ENTRY_FLAG_MASK: u8 = FLAG_SURNAME | FLAG_PLACE | FLAG_TERM | FLAG_USER;

/// Encoded size of one `UNIGRAM` element: `u32 hash, u16 prob_q12, u16 pad`.
pub const UNIGRAM_ENTRY_SIZE: usize = 8;

/// Denominator of the Q12 unigram score.
///
/// [`reader::Reader::unigram`] returns `weight / max_weight` in this scale, so the
/// most frequent word of a dictionary scores exactly `PROB_Q12_MAX` and
/// `log2(score / PROB_Q12_MAX)` is a log probability in `[-12, 0]` bits.
pub const PROB_Q12_MAX: u16 = 4096;

/// The six sections of the container, in the order they are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum SectionKind {
    /// `fst::Map<u64>`: syllable key -> `(wordlist_start, count)`.
    Fst = 1,
    /// `[DictEntry; entry_count]`, ascending `word_id`.
    Entries = 2,
    /// Concatenated UTF-8 word text, without length prefixes.
    StrPool = 3,
    /// `[(u32 hash, u16 prob_q12, u16 pad); entry_count]`, ascending `hash`.
    Unigram = 4,
    /// Reserved for the Phase 3 bigram model; always empty in version 1.
    Bigram = 5,
    /// `[u32; pair_count]` of `word_id`s, grouped by key. The only section that
    /// grows with multi-key expansion.
    WordList = 6,
}

impl SectionKind {
    /// Every kind, in ascending order.
    pub const ALL: [SectionKind; SECTION_COUNT] = [
        SectionKind::Fst,
        SectionKind::Entries,
        SectionKind::StrPool,
        SectionKind::Unigram,
        SectionKind::Bigram,
        SectionKind::WordList,
    ];

    /// Returns the kind a raw `kind` field denotes, or `None` if it is not one of
    /// the six kinds version 1 defines.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_dict::format::SectionKind;
    ///
    /// assert_eq!(SectionKind::from_raw(1), Some(SectionKind::Fst));
    /// assert_eq!(SectionKind::from_raw(0), None);
    /// assert_eq!(SectionKind::from_raw(7), None);
    /// ```
    pub fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_raw() == raw)
    }

    /// Returns the raw `kind` field of this section.
    pub fn as_raw(self) -> u32 {
        self as u32
    }

    /// Returns the section name as it appears in diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            SectionKind::Fst => "FST",
            SectionKind::Entries => "ENTRIES",
            SectionKind::StrPool => "STRPOOL",
            SectionKind::Unigram => "UNIGRAM",
            SectionKind::Bigram => "BIGRAM",
            SectionKind::WordList => "WORDLIST",
        }
    }
}

/// The parsed 64-byte header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Container format version; always [`FORMAT_VERSION`] for a file this crate
    /// is willing to read.
    pub format_version: u16,
    /// Bit set of `FLAG_*`; unknown bits are rejected.
    pub flags: u16,
    /// Declared file length, which must equal the actual byte count.
    pub total_len: u64,
    /// CRC32 of `[HEADER_SIZE, total_len)`.
    pub file_crc32: u32,
    /// Number of word entries in `ENTRIES` and `UNIGRAM`.
    pub entry_count: u32,
}

/// One entry of the section table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionEntry {
    /// Which section this entry describes.
    pub kind: SectionKind,
    /// CRC32 of the section bytes.
    pub crc32: u32,
    /// Byte offset in the file; zero when the section is absent.
    pub offset: u64,
    /// Length in bytes; zero means the section is absent.
    pub len: u64,
}

impl SectionEntry {
    /// Returns `true` when the section carries no bytes.
    pub fn is_absent(&self) -> bool {
        self.len == 0
    }
}

/// One word record: the fixed-width row of the `ENTRIES` section.
///
/// The field order and width are the container contract, so the struct is
/// `repr(C)` with an 8-byte alignment and is encoded field by field rather than
/// by casting: the crate is `unsafe`-free outside `mmap.rs`, and a cast of a
/// borrowed buffer to a struct would be exactly the unchecked read the format
/// must not perform.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DictEntry {
    /// Byte offset of the word text inside `STRPOOL`.
    pub word_off: u32,
    /// Length of the word text in bytes, at most [`MAX_WORD_LEN`].
    pub word_len: u16,
    /// Number of syllables the canonical key consumes, in `1..=MAX_SYL_COUNT`.
    pub syl_count: u8,
    /// Bit set of `FLAG_*`.
    ///
    /// Bits this version does not define are preserved rather than rejected, so a record
    /// written by a later compiler still decodes; see [`ENTRY_FLAG_MASK`].
    pub flags: u8,
    /// Ranking weight used to order words that share a key.
    pub weight: u32,
    /// Reserved padding, always zero; keeps the record 16 bytes wide.
    pub _pad: u32,
}

/// Encoded width of [`DictEntry`], asserted at compile time below.
pub const ENTRY_SIZE: usize = 16;

const _: () = assert!(std::mem::size_of::<DictEntry>() == ENTRY_SIZE);

impl DictEntry {
    /// Builds an entry with zeroed padding.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_dict::format::DictEntry;
    ///
    /// let entry = DictEntry::new(0, 6, 2, 0, 3000);
    /// assert_eq!(entry.encode().len(), 16);
    /// ```
    pub fn new(word_off: u32, word_len: u16, syl_count: u8, flags: u8, weight: u32) -> Self {
        Self {
            word_off,
            word_len,
            syl_count,
            flags,
            weight,
            _pad: 0,
        }
    }

    /// Encodes the entry as its 16 little-endian bytes.
    pub fn encode(self) -> [u8; ENTRY_SIZE] {
        let mut out = [0u8; ENTRY_SIZE];
        out[0..4].copy_from_slice(&self.word_off.to_le_bytes());
        out[4..6].copy_from_slice(&self.word_len.to_le_bytes());
        out[6] = self.syl_count;
        out[7] = self.flags;
        out[8..12].copy_from_slice(&self.weight.to_le_bytes());
        out[12..16].copy_from_slice(&self._pad.to_le_bytes());
        out
    }

    /// Decodes an entry from the first [`ENTRY_SIZE`] bytes of `bytes`.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when `bytes` is shorter than
    /// [`ENTRY_SIZE`], or when the decoded record violates the layout contract
    /// (a `word_len` outside `1..=MAX_WORD_LEN`, a `syl_count` outside
    /// `1..=MAX_SYL_COUNT`, or a non-zero padding word).
    pub fn decode(bytes: &[u8]) -> Result<Self, DictError> {
        let entry = Self {
            word_off: read_u32(bytes, 0, "word_off")?,
            word_len: read_u16(bytes, 4, "word_len")?,
            syl_count: *bytes.get(6).ok_or(DictError::LengthOutOfRange {
                field: "syl_count",
                value: 0,
            })?,
            flags: *bytes.get(7).ok_or(DictError::LengthOutOfRange {
                field: "flags",
                value: 0,
            })?,
            weight: read_u32(bytes, 8, "weight")?,
            _pad: read_u32(bytes, 12, "_pad")?,
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Checks the per-record invariants the container promises.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] naming the offending field when
    /// `word_len` exceeds [`MAX_WORD_LEN`], `syl_count` is zero or above
    /// [`MAX_SYL_COUNT`], or `_pad` is not zero.
    pub fn validate(&self) -> Result<(), DictError> {
        if self.word_len == 0 || self.word_len > MAX_WORD_LEN {
            return Err(DictError::LengthOutOfRange {
                field: "word_len",
                value: u64::from(self.word_len),
            });
        }
        if self.syl_count == 0 || self.syl_count > MAX_SYL_COUNT {
            return Err(DictError::LengthOutOfRange {
                field: "syl_count",
                value: u64::from(self.syl_count),
            });
        }
        // `flags` is deliberately left unchecked: a bit this version does not define is a flag
        // a later version added, and refusing the record would turn a newer dictionary into an
        // unreadable one, while keeping the byte lets the reader pick out the bits it knows.
        if self._pad != 0 {
            return Err(DictError::LengthOutOfRange {
                field: "_pad",
                value: u64::from(self._pad),
            });
        }
        Ok(())
    }
}

/// Computes the CRC32 of `bytes`, the checksum every section and the file body
/// carry.
///
/// # Examples
///
/// ```
/// use ime_dict::format::crc32;
///
/// assert_eq!(crc32(b""), 0);
/// assert_ne!(crc32(b"RSPD"), 0);
/// ```
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

/// Hashes a word into the `u32` the `UNIGRAM` section is sorted and searched by.
///
/// The hash is FNV-1a over the word's UTF-8 bytes: fixed, documented and
/// independent of the standard library's (deliberately randomised) `Hasher`, so
/// two compiles of the same input produce the same table. Collisions are possible
/// and are resolved by comparing the word text: the section is sorted by
/// `(hash, word)`, and `word_id` is the index in that order, which is what keeps
/// `ENTRIES` and `UNIGRAM` addressable by the same index.
///
/// # Examples
///
/// ```
/// use ime_dict::format::hash_word;
///
/// assert_eq!(hash_word(""), 0x811c_9dc5);
/// assert_eq!(hash_word("a"), hash_word("a"));
/// assert_ne!(hash_word("a"), hash_word("b"));
/// ```
pub fn hash_word(word: &str) -> u32 {
    const OFFSET_BASIS: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    word.as_bytes().iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(PRIME)
    })
}

/// Packs a word-list range into the value an FST entry stores.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] when `count` does not fit the 24 bits
/// the layout reserves for it, or when `start` is beyond [`MAX_FST_START`].
pub fn pack_fst_value(start: u64, count: u32) -> Result<u64, DictError> {
    if u64::from(count) >= (1u64 << FST_COUNT_BITS) {
        return Err(DictError::LengthOutOfRange {
            field: "fst_count",
            value: u64::from(count),
        });
    }
    if start > MAX_FST_START {
        return Err(DictError::LengthOutOfRange {
            field: "fst_start",
            value: start,
        });
    }
    Ok((start << FST_COUNT_BITS) | u64::from(count))
}

/// Splits a packed FST value back into `(wordlist_start, count)`.
pub fn unpack_fst_value(value: u64) -> (u64, u32) {
    let mask = (1u64 << FST_COUNT_BITS) - 1;
    (value >> FST_COUNT_BITS, (value & mask) as u32)
}

/// Reads a little-endian `u16` at `at`.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `field` when the slice is too
/// short.
pub(crate) fn read_u16(bytes: &[u8], at: usize, field: &'static str) -> Result<u16, DictError> {
    let raw = bytes.get(at..at + 2).ok_or(DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    let raw: [u8; 2] = raw.try_into().map_err(|_| DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    Ok(u16::from_le_bytes(raw))
}

/// Reads a little-endian `u32` at `at`.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `field` when the slice is too
/// short.
pub(crate) fn read_u32(bytes: &[u8], at: usize, field: &'static str) -> Result<u32, DictError> {
    let raw = bytes.get(at..at + 4).ok_or(DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    let raw: [u8; 4] = raw.try_into().map_err(|_| DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    Ok(u32::from_le_bytes(raw))
}

/// Reads a little-endian `u64` at `at`.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `field` when the slice is too
/// short.
pub(crate) fn read_u64(bytes: &[u8], at: usize, field: &'static str) -> Result<u64, DictError> {
    let raw = bytes.get(at..at + 8).ok_or(DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    let raw: [u8; 8] = raw.try_into().map_err(|_| DictError::LengthOutOfRange {
        field,
        value: at as u64,
    })?;
    Ok(u64::from_le_bytes(raw))
}

/// Parses the 64-byte header of `bytes`.
///
/// # Errors
/// Returns [`DictError::MagicMismatch`] when the magic is wrong,
/// [`DictError::FormatVersion`] when the version is not [`FORMAT_VERSION`], and
/// [`DictError::LengthOutOfRange`] when the file is shorter than the header, when
/// `header_size`, `section_count` or `total_len` disagree with the layout, when
/// `flags` sets an undefined bit, or when `reserved` is not zero.
pub fn parse_header(bytes: &[u8]) -> Result<Header, DictError> {
    if bytes.len() < HEADER_SIZE {
        return Err(DictError::LengthOutOfRange {
            field: "file_len",
            value: bytes.len() as u64,
        });
    }
    let magic = bytes.get(0..4).unwrap_or_default();
    if magic != MAGIC.as_slice() {
        return Err(DictError::MagicMismatch);
    }
    let format_version = read_u16(bytes, 4, "format_version")?;
    if format_version != FORMAT_VERSION {
        return Err(DictError::FormatVersion {
            found: format_version,
        });
    }
    let flags = read_u16(bytes, 6, "flags")?;
    if flags & !FLAG_MASK != 0 {
        return Err(DictError::LengthOutOfRange {
            field: "flags",
            value: u64::from(flags),
        });
    }
    let header_size = read_u32(bytes, 8, "header_size")?;
    if header_size as usize != HEADER_SIZE {
        return Err(DictError::LengthOutOfRange {
            field: "header_size",
            value: u64::from(header_size),
        });
    }
    let section_count = read_u32(bytes, 12, "section_count")?;
    if section_count as usize != SECTION_COUNT {
        return Err(DictError::LengthOutOfRange {
            field: "section_count",
            value: u64::from(section_count),
        });
    }
    let total_len = read_u64(bytes, 16, "total_len")?;
    if total_len != bytes.len() as u64 {
        return Err(DictError::LengthOutOfRange {
            field: "total_len",
            value: total_len,
        });
    }
    let reserved = bytes.get(32..HEADER_SIZE).unwrap_or_default();
    if reserved.iter().any(|byte| *byte != 0) {
        return Err(DictError::LengthOutOfRange {
            field: "reserved",
            value: 1,
        });
    }
    Ok(Header {
        format_version,
        flags,
        total_len,
        file_crc32: read_u32(bytes, 24, "file_crc32")?,
        entry_count: read_u32(bytes, 28, "entry_count")?,
    })
}

/// Parses the six section-table entries that follow the header.
///
/// The table is validated structurally here -- kind order, alignment, bounds --
/// but not against the section bytes; [`reader::Reader::parse`] does that, and
/// only there is the file body available.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] when the table is truncated, when the
/// kinds are not exactly `1..=6` in ascending order, when a present section's
/// offset is unaligned or points inside the header, or when an absent section
/// carries a non-zero offset.
pub fn parse_section_table(bytes: &[u8]) -> Result<[SectionEntry; SECTION_COUNT], DictError> {
    let table = bytes
        .get(HEADER_SIZE..HEADER_SIZE + SECTION_TABLE_SIZE)
        .ok_or(DictError::LengthOutOfRange {
            field: "section_table",
            value: bytes.len() as u64,
        })?;
    let mut sections = [SectionEntry {
        kind: SectionKind::Fst,
        crc32: 0,
        offset: 0,
        len: 0,
    }; SECTION_COUNT];
    for (index, expected) in SectionKind::ALL.into_iter().enumerate() {
        let at = index * SECTION_ENTRY_SIZE;
        let raw_kind = read_u32(table, at, "section_kind")?;
        let kind = SectionKind::from_raw(raw_kind).ok_or(DictError::LengthOutOfRange {
            field: "section_kind",
            value: u64::from(raw_kind),
        })?;
        if kind != expected {
            return Err(DictError::LengthOutOfRange {
                field: "section_order",
                value: u64::from(raw_kind),
            });
        }
        let offset = read_u64(table, at + 8, "section_offset")?;
        let len = read_u64(table, at + 16, "section_len")?;
        if len == 0 {
            if offset != 0 {
                return Err(DictError::LengthOutOfRange {
                    field: "section_offset",
                    value: offset,
                });
            }
        } else {
            if offset % SECTION_ALIGN != 0 || offset < SECTIONS_OFFSET {
                return Err(DictError::LengthOutOfRange {
                    field: "section_offset",
                    value: offset,
                });
            }
            let end = offset.checked_add(len).ok_or(DictError::LengthOutOfRange {
                field: "section_len",
                value: len,
            })?;
            if end > bytes.len() as u64 {
                return Err(DictError::LengthOutOfRange {
                    field: "section_len",
                    value: len,
                });
            }
        }
        sections[index] = SectionEntry {
            kind,
            crc32: read_u32(table, at + 4, "section_crc32")?,
            offset,
            len,
        };
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
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
            _pad: 1,
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
}
