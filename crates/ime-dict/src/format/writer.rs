//! Assembling and writing a container file.
//!
//! Responsibility: turn a set of named section payloads into the byte image
//! described by [`crate::format`], and put that image on disk in one piece.
//!
//! Boundaries: the writer is told what the sections contain; it never computes
//! one. It knows nothing about words, syllables or weights, and it performs no
//! parsing of what it is handed beyond the checks the layout itself requires
//! (a section length that is a multiple of the record size, at most one payload
//! per kind, no `BIGRAM` payload in version 1).
//!
//! # Atomic replacement
//!
//! [`DictWriter::finish`] never opens the destination for writing. It writes the
//! image to `<path>.tmp`, flushes it with `sync_all`, and only then renames it
//! over the destination. A reader therefore observes either the previous file or
//! the complete new one, and a crash mid-write leaves the destination untouched.
//! The dictionary-update path depends on that guarantee.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use ime_types::DictError;

use crate::format::{
    self, ENTRY_SIZE, FLAG_BIGRAM_PRESENT, FORMAT_VERSION, HEADER_SIZE, MAGIC, SECTION_ALIGN,
    SECTION_COUNT, SECTION_ENTRY_SIZE, SECTIONS_OFFSET, SectionEntry, SectionKind,
};

/// Suffix appended to the destination path while the image is being written.
pub const TEMP_SUFFIX: &str = ".tmp";

/// Collects section payloads and writes them as one container.
///
/// Sections may be added in any order; the layout always places them in
/// ascending [`SectionKind`] order, which is what the reader validates. A payload
/// that is empty is treated as an absent section: its table entry carries a zero
/// length and a zero offset, exactly as a section the compiler chose not to emit.
#[derive(Debug, Default)]
pub struct DictWriter {
    sections: BTreeMap<SectionKind, Vec<u8>>,
}

impl DictWriter {
    /// Creates a writer with no sections.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_dict::format::SectionKind;
    /// use ime_dict::format::writer::DictWriter;
    ///
    /// let mut writer = DictWriter::new();
    /// writer.add_section(SectionKind::Fst, vec![1, 2, 3, 4, 5, 6, 7, 8]).expect("add");
    /// let image = writer.encode().expect("encode");
    /// assert_eq!(&image[0..4], b"RSPD");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one section payload.
    ///
    /// The payload is taken by value: the compiler builds each section in its own
    /// buffer and hands it over without a copy.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the same kind is added twice,
    /// when the `ENTRIES` payload is not a whole number of [`ENTRY_SIZE`]-byte
    /// records, when the `UNIGRAM` payload is not a whole number of
    /// [`format::UNIGRAM_ENTRY_SIZE`]-byte records, or when the `WORDLIST`
    /// payload is not a whole number of `u32`s. A `BIGRAM` payload is rejected in
    /// version 1, whose `flags` contract states that the section is never emitted.
    pub fn add_section(&mut self, kind: SectionKind, bytes: Vec<u8>) -> Result<(), DictError> {
        if kind == SectionKind::Bigram && !bytes.is_empty() {
            return Err(DictError::LengthOutOfRange {
                field: "bigram_len",
                value: bytes.len() as u64,
            });
        }
        let width = match kind {
            SectionKind::Entries => ENTRY_SIZE,
            SectionKind::Unigram => format::UNIGRAM_ENTRY_SIZE,
            SectionKind::WordList => 4,
            SectionKind::Fst | SectionKind::StrPool | SectionKind::Bigram => 1,
        };
        if bytes.len() % width != 0 {
            return Err(DictError::LengthOutOfRange {
                field: "section_len",
                value: bytes.len() as u64,
            });
        }
        if self.sections.insert(kind, bytes).is_some() {
            return Err(DictError::LengthOutOfRange {
                field: "section_duplicate",
                value: kind.as_raw() as u64,
            });
        }
        Ok(())
    }

    /// Returns the number of word records the container will declare.
    ///
    /// The count is derived from the `ENTRIES` payload rather than passed in
    /// separately, so the header can never disagree with the table it describes.
    pub fn entry_count(&self) -> Result<u32, DictError> {
        let bytes = self.sections.get(&SectionKind::Entries).map_or(0, Vec::len);
        let count = bytes / ENTRY_SIZE;
        u32::try_from(count).map_err(|_| DictError::LengthOutOfRange {
            field: "entry_count",
            value: count as u64,
        })
    }

    /// Returns the `flags` word the header will carry.
    pub fn flags(&self) -> u16 {
        let has_bigram = self
            .sections
            .get(&SectionKind::Bigram)
            .is_some_and(|bytes| !bytes.is_empty());
        if has_bigram { FLAG_BIGRAM_PRESENT } else { 0 }
    }

    /// Returns the section bytes this writer will lay out, for callers that want
    /// to inspect a payload before it is written.
    pub fn section(&self, kind: SectionKind) -> Option<&[u8]> {
        self.sections.get(&kind).map(Vec::as_slice)
    }

    /// Builds the complete container image in memory.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when a section offset or the total
    /// length would not fit the 64-bit fields, which can only happen for an
    /// image far larger than the size budget allows.
    pub fn encode(&self) -> Result<Vec<u8>, DictError> {
        // One slot per section, in kind order. Every slot carries its own kind even when
        // the section is absent: the reader checks the kind at each index and treats a
        // zero *length*, not a zero kind, as "absent".
        let mut table = SectionKind::ALL.map(|kind| SectionEntry {
            kind,
            crc32: 0,
            offset: 0,
            len: 0,
        });

        // First pass: place every present section on its alignment boundary and
        // record where it lands. Absent sections keep the zero entry, which the
        // reader accepts only for a zero length.
        let mut cursor = SECTIONS_OFFSET;
        for (index, kind) in SectionKind::ALL.into_iter().enumerate() {
            let Some(bytes) = self.sections.get(&kind).filter(|bytes| !bytes.is_empty()) else {
                continue;
            };
            let offset = align_up(cursor)?;
            let len = bytes.len() as u64;
            table[index] = SectionEntry {
                kind,
                crc32: format::crc32(bytes),
                offset,
                len,
            };
            cursor = offset.checked_add(len).ok_or(DictError::LengthOutOfRange {
                field: "total_len",
                value: offset,
            })?;
        }
        let total_len = cursor;
        let total = usize::try_from(total_len).map_err(|_| DictError::LengthOutOfRange {
            field: "total_len",
            value: total_len,
        })?;

        // Second pass: emit header, table and sections into the image.
        let mut image = vec![0u8; total];
        let entry_count = self.entry_count()?;
        let header = image
            .get_mut(..HEADER_SIZE)
            .ok_or(DictError::LengthOutOfRange {
                field: "header",
                value: total_len,
            })?;
        header[0..4].copy_from_slice(&MAGIC);
        header[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        header[6..8].copy_from_slice(&self.flags().to_le_bytes());
        header[8..12].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        header[12..16].copy_from_slice(&(SECTION_COUNT as u32).to_le_bytes());
        header[16..24].copy_from_slice(&total_len.to_le_bytes());
        header[28..32].copy_from_slice(&entry_count.to_le_bytes());
        // Bytes 24..28 (the file checksum) and 32..64 (reserved) stay zero here.

        for (index, entry) in table.iter().enumerate() {
            // The slot comes from the position, not from `entry.kind`: deriving it from
            // the kind would let a row write into another section's slot, which is how a
            // table of all-zero rows once reached the reader.
            let at = HEADER_SIZE + index * SECTION_ENTRY_SIZE;
            let slot =
                image
                    .get_mut(at..at + SECTION_ENTRY_SIZE)
                    .ok_or(DictError::LengthOutOfRange {
                        field: "section_table",
                        value: at as u64,
                    })?;
            let mut row = [0u8; SECTION_ENTRY_SIZE];
            row[0..4].copy_from_slice(&entry.kind.as_raw().to_le_bytes());
            row[4..8].copy_from_slice(&entry.crc32.to_le_bytes());
            row[8..16].copy_from_slice(&entry.offset.to_le_bytes());
            row[16..24].copy_from_slice(&entry.len.to_le_bytes());
            slot.copy_from_slice(&row);
        }

        for kind in SectionKind::ALL {
            let Some(bytes) = self.sections.get(&kind).filter(|bytes| !bytes.is_empty()) else {
                continue;
            };
            let index = kind.as_raw() as usize - 1;
            let offset = table[index].offset;
            let start = usize::try_from(offset).map_err(|_| DictError::LengthOutOfRange {
                field: "section_offset",
                value: offset,
            })?;
            let end = start + bytes.len();
            let slot = image
                .get_mut(start..end)
                .ok_or(DictError::LengthOutOfRange {
                    field: "section_offset",
                    value: offset,
                })?;
            slot.copy_from_slice(bytes);
        }

        // The file checksum covers everything after the header, so it is computed
        // last and then written into the one field it deliberately excludes.
        let file_crc32 = format::crc32(&image[HEADER_SIZE..]);
        image
            .get_mut(24..28)
            .ok_or(DictError::LengthOutOfRange {
                field: "file_crc32",
                value: 24,
            })?
            .copy_from_slice(&file_crc32.to_le_bytes());
        Ok(image)
    }

    /// Writes the container to `path`, replacing any existing file atomically.
    ///
    /// The image goes to `<path>.tmp`, is flushed with `File::sync_all`, and is
    /// then renamed onto `path`. The destination is never opened for writing, so
    /// a crash or a full disk leaves the previous file in place.
    ///
    /// # Errors
    /// Returns [`DictError::Io`] when the parent directory cannot be created, the
    /// temporary file cannot be written or flushed, or the rename fails, and
    /// [`DictError::LengthOutOfRange`] when the image cannot be built.
    pub fn finish(&self, path: &Path) -> Result<(), DictError> {
        let image = self.encode()?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let temp = temp_path(path);
        let mut file = File::create(&temp)?;
        file.write_all(&image)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    }
}

/// Returns the temporary path a writer uses for `path`.
///
/// The suffix is appended to the file name rather than to the whole path, so a
/// destination whose last component has no extension still gets a sibling
/// temporary file in the same directory -- a rename across directories is not
/// atomic on every filesystem, and the same-directory placement is what makes it
/// atomic here.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(TEMP_SUFFIX);
    path.with_file_name(name)
}

/// Rounds `offset` up to the next [`SECTION_ALIGN`] boundary.
fn align_up(offset: u64) -> Result<u64, DictError> {
    let remainder = offset % SECTION_ALIGN;
    if remainder == 0 {
        return Ok(offset);
    }
    offset
        .checked_add(SECTION_ALIGN - remainder)
        .ok_or(DictError::LengthOutOfRange {
            field: "section_offset",
            value: offset,
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
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
        entries
            .extend_from_slice(&format::DictEntry::new(3, 6, 2, format::FLAG_PLACE, 300).encode());

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
    fn test_add_section_rejects_a_bigram_payload_in_version_one() {
        let mut writer = DictWriter::new();
        let result = writer.add_section(SectionKind::Bigram, vec![1, 2, 3, 4]);
        assert!(matches!(
            result,
            Err(DictError::LengthOutOfRange {
                field: "bigram_len",
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
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            format::parse_header(&bytes),
            Err(DictError::FormatVersion { found: 2 })
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
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
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
}
