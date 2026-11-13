//! Assembling and writing a container file.
//!
//! Responsibility: turn a set of named section payloads into the byte image
//! described by [`crate::format`], and put that image on disk in one piece.
//!
//! Boundaries: the writer is told what the sections contain; it never computes
//! one. It knows nothing about words, syllables or weights, and it performs no
//! parsing of what it is handed beyond the checks the layout itself requires
//! (a section length that is a multiple of the record size, at most one payload
//! per kind, and fixed-width section rows.
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

use crate::format::aux::encode_block;
use crate::format::{
    self, AUX_ROW_SIZE, BIGRAM_ENTRY_SIZE, ENTRY_SIZE, FLAG_AUX_PRESENT, FLAG_BIGRAM_PRESENT,
    FLAG_REVERSE_PRESENT, FORMAT_VERSION, HEADER_SIZE, MAGIC, SECTION_COUNT, SECTION_ENTRY_SIZE,
    SECTIONS_OFFSET, SectionEntry, SectionKind, align_up,
};

/// Suffix appended to the destination path while the image is being written.
pub const TEMP_SUFFIX: &str = ".tmp";

/// Collects section payloads and writes them as one container.
///
/// Sections may be added in any order; the layout always places them in
/// ascending [`SectionKind`] order, which is what the reader validates. A payload
/// that is empty is treated as an absent section: its table entry carries a zero
/// length and a zero offset, exactly as a section the compiler chose not to emit.
///
/// The AUX column is not a table section: the six-slot table is fixed by the
/// v1/v2 layout, so the column is stored as a self-describing block at the
/// aligned tail of the file and announced by [`FLAG_AUX_PRESENT`] in the
/// header. See [`crate::format::aux`]. The reverse index follows the same
/// rule, stacked after the AUX block; see [`crate::reverse`].
#[derive(Debug, Default)]
pub struct DictWriter {
    sections: BTreeMap<SectionKind, Vec<u8>>,
    aux: Option<Vec<u8>>,
    reverse: Option<Vec<u8>>,
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
    /// [`format::UNIGRAM_ENTRY_SIZE`]-byte records, when the `BIGRAM` payload is
    /// not a whole number of [`BIGRAM_ENTRY_SIZE`]-byte records, or when the
    /// `WORDLIST` payload is not a whole number of `u32`s.
    // `is_multiple_of` stabilized in Rust 1.87, while the workspace MSRV is 1.85.
    #[allow(clippy::manual_is_multiple_of)]
    pub fn add_section(&mut self, kind: SectionKind, bytes: Vec<u8>) -> Result<(), DictError> {
        let width = match kind {
            SectionKind::Entries => ENTRY_SIZE,
            SectionKind::Unigram => format::UNIGRAM_ENTRY_SIZE,
            SectionKind::WordList => 4,
            SectionKind::Fst | SectionKind::StrPool => 1,
            SectionKind::Bigram => BIGRAM_ENTRY_SIZE,
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

    /// Adds the AUX column payload: one [`AUX_ROW_SIZE`]-byte row per word
    /// record, in the `ENTRIES` ordering.
    ///
    /// The payload is taken by value. The writer checks only what the layout
    /// itself requires -- a whole number of rows, one row per entry, added once
    /// -- and leaves the row contents to the compiler, exactly as it does for
    /// the FST payload; the reader re-validates every row of the untrusted
    /// image on parse.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the payload is empty, is
    /// not a whole number of [`AUX_ROW_SIZE`]-byte rows, does not hold one row
    /// per `ENTRIES` record, or when the column was added twice.
    // `is_multiple_of` stabilized in Rust 1.87, while the workspace MSRV is 1.85.
    #[allow(clippy::manual_is_multiple_of)]
    pub fn add_aux_column(&mut self, rows: Vec<u8>) -> Result<(), DictError> {
        if rows.is_empty() {
            return Err(DictError::LengthOutOfRange {
                field: "aux_len",
                value: 0,
            });
        }
        if rows.len() % AUX_ROW_SIZE != 0 {
            return Err(DictError::LengthOutOfRange {
                field: "aux_len",
                value: rows.len() as u64,
            });
        }
        let row_count = rows.len() / AUX_ROW_SIZE;
        let entries = self.entry_count()? as usize;
        if row_count != entries {
            return Err(DictError::LengthOutOfRange {
                field: "aux_count",
                value: row_count as u64,
            });
        }
        if self.aux.is_some() {
            return Err(DictError::LengthOutOfRange {
                field: "aux_duplicate",
                value: 1,
            });
        }
        self.aux = Some(rows);
        Ok(())
    }

    /// Adds the reverse-lookup index payload: the self-describing block
    /// [`crate::reverse`] lays out, header included.
    ///
    /// The payload is taken by value. The writer checks only what the layout
    /// itself requires -- a present header of the right magic and a length
    /// that exactly matches the counts it declares -- and leaves the row
    /// contents to the compiler; the reader re-validates every row of the
    /// untrusted image on parse.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the payload is empty,
    /// shorter than its header, carries the wrong magic, does not hold exactly
    /// the characters, keys and postings its header declares, or when the
    /// index was added twice.
    pub fn add_reverse_index(&mut self, bytes: Vec<u8>) -> Result<(), DictError> {
        if bytes.is_empty() {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_len",
                value: 0,
            });
        }
        if bytes.len() < crate::reverse::REVERSE_HEADER_SIZE
            || bytes.get(0..4) != Some(&crate::reverse::REVERSE_MAGIC[..])
        {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_magic",
                value: bytes.len() as u64,
            });
        }
        let char_count = format::read_u32(&bytes, 8, "reverse_char_count")? as usize;
        let key_count = format::read_u32(&bytes, 12, "reverse_key_count")? as usize;
        let posting_count = format::read_u32(&bytes, 16, "reverse_posting_count")? as usize;
        let expected = crate::reverse::REVERSE_HEADER_SIZE
            + char_count * crate::reverse::REVERSE_CHAR_ROW_SIZE
            + key_count * crate::reverse::REVERSE_KEY_ROW_SIZE
            + posting_count * 4;
        if bytes.len() != expected {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_len",
                value: bytes.len() as u64,
            });
        }
        if self.reverse.is_some() {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_duplicate",
                value: 1,
            });
        }
        self.reverse = Some(bytes);
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
    ///
    /// The AUX and REVERSE bits are set exactly when the matching tail payload
    /// was added, which is the same flag-iff-present contract the `BIGRAM` bit
    /// follows.
    pub fn flags(&self) -> u16 {
        let has_bigram = self
            .sections
            .get(&SectionKind::Bigram)
            .is_some_and(|bytes| !bytes.is_empty());
        let mut flags = 0;
        if has_bigram {
            flags |= FLAG_BIGRAM_PRESENT;
        }
        if self.aux.is_some() {
            flags |= FLAG_AUX_PRESENT;
        }
        if self.reverse.is_some() {
            flags |= FLAG_REVERSE_PRESENT;
        }
        flags
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

        // The AUX column, when present, is the aligned tail of the file: it has
        // no table slot, so the reader finds it by this same rule (aligned past
        // the last table section) and validates the block header there. The
        // reverse index, when present too, stacks after it by the same rule,
        // so the walk ends at whichever tail block was written last.
        let mut tail_end = cursor;
        if let Some(rows) = &self.aux {
            let offset = align_up(cursor)?;
            let block = encode_block(entry_count, rows)?;
            let block_len = block.len() as u64;
            let total_len = offset
                .checked_add(block_len)
                .ok_or(DictError::LengthOutOfRange {
                    field: "total_len",
                    value: offset,
                })?;
            tail_end = total_len;
            let total = usize::try_from(total_len).map_err(|_| DictError::LengthOutOfRange {
                field: "total_len",
                value: total_len,
            })?;
            image.resize(total, 0);
            let start = usize::try_from(offset).map_err(|_| DictError::LengthOutOfRange {
                field: "section_offset",
                value: offset,
            })?;
            let end = start + block.len();
            let slot = image
                .get_mut(start..end)
                .ok_or(DictError::LengthOutOfRange {
                    field: "section_offset",
                    value: offset,
                })?;
            slot.copy_from_slice(&block);
            // The header was written against the six-section total above; the
            // length is corrected now that the tail is in place.
            let header_len = image.get_mut(16..24).ok_or(DictError::LengthOutOfRange {
                field: "header",
                value: total_len,
            })?;
            header_len.copy_from_slice(&total_len.to_le_bytes());
        }

        // The reverse index, when present, is the next aligned tail block: it
        // stacks after the AUX column's end, and the reader resolves it by the
        // same rule this placement follows.
        if let Some(bytes) = &self.reverse {
            let offset = align_up(tail_end)?;
            let block_len = bytes.len() as u64;
            let total_len = offset
                .checked_add(block_len)
                .ok_or(DictError::LengthOutOfRange {
                    field: "total_len",
                    value: offset,
                })?;
            let total = usize::try_from(total_len).map_err(|_| DictError::LengthOutOfRange {
                field: "total_len",
                value: total_len,
            })?;
            image.resize(total, 0);
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
            // The header was written against the earlier total above; the
            // length is corrected again now that this tail is in place. The
            // file checksum below is computed after this, so it covers the
            // block.
            let header_len = image.get_mut(16..24).ok_or(DictError::LengthOutOfRange {
                field: "header",
                value: total_len,
            })?;
            header_len.copy_from_slice(&total_len.to_le_bytes());
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
    /// a crash or a full disk leaves the previous file in place. A failed write
    /// removes the temporary file again: a full disk would otherwise leave a
    /// partial image next to the destination that no later run has reason to
    /// clean up.
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
        let written = (|| {
            let mut file = File::create(&temp)?;
            file.write_all(&image)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if written.is_err() {
            // The temporary can only exist past this point when the write or the
            // flush failed; the rename either succeeded (no error) or never moved
            // it. Removing it is best-effort: the original error is the one worth
            // reporting.
            let _ = fs::remove_file(&temp);
        }
        written
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

#[cfg(test)]
mod tests;
