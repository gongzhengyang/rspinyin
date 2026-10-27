//! Opening and verifying a container file.
//!
//! Responsibility: turn a byte image into a structurally verified view of the six
//! sections -- every length, offset, alignment and checksum checked before any
//! offset is trusted -- and expose the records through zero-copy accessors.
//!
//! Boundaries: the reader interprets the container, not the dictionary. It knows
//! what a `DictEntry` is but not what a syllable key means, it never scores
//! anything, and it never iterates `ENTRIES` on open: opening costs one CRC32 pass
//! over the file body (about 8ms for a 6.4MB entry table) plus a 208-byte header
//! parse, which is what the addon-load budget allows.
//!
//! # Trust model
//!
//! A dictionary file is untrusted input. Every accessor re-checks the bounds of
//! the record it is asked for and returns [`DictError`] instead of reading out of
//! range, so a truncated or hand-edited file can produce an error but never an
//! out-of-bounds access or a panic.

use std::fs;
use std::path::Path;

use ime_types::DictError;

use crate::format::{
    self, BIGRAM_ENTRY_SIZE, BigramRecord, DictEntry, ENTRY_SIZE, FLAG_BIGRAM_PRESENT,
    FORMAT_VERSION_V1, HEADER_SIZE, Header, SECTION_COUNT, SectionEntry, SectionKind,
    UNIGRAM_ENTRY_SIZE, crc32, hash_word, parse_header, parse_section_table, unpack_fst_value,
};

/// How much of a container [`Reader::parse`] verifies while opening it.
///
/// The engine's `verify_dict_on_load` setting selects between the two: `Full` is
/// the default and costs one CRC32 pass over the file body, `Header` skips the
/// checksums and leaves them to a background pass when a file is large enough for
/// the difference to matter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verify {
    /// Parse the header and the section table and check every length, offset and
    /// alignment, without computing any checksum.
    Header,
    /// Additionally verify the file checksum and every present section checksum.
    #[default]
    Full,
}

/// A parsed container: the image plus its verified header and section table.
///
/// The type parameter is the byte source. [`Reader::open`] produces a
/// `Reader<Vec<u8>>` that owns its image; a caller that already holds a mapping
/// can build a `Reader<Mmap>` with [`Reader::parse`] and keep the zero-copy
/// property end to end.
#[derive(Debug)]
pub struct Reader<D = Vec<u8>> {
    data: D,
    header: Header,
    sections: [SectionEntry; SECTION_COUNT],
}

impl Reader<Vec<u8>> {
    /// Reads `path` and opens it with full verification.
    ///
    /// # Errors
    /// Returns [`DictError::Io`] when the file cannot be read and any of the
    /// structural or checksum errors of [`Reader::parse`] when it is not a valid
    /// container.
    pub fn open(path: &Path) -> Result<Self, DictError> {
        Self::open_with(path, Verify::Full)
    }

    /// Reads `path` and opens it, verifying as much as `verify` asks for.
    ///
    /// # Errors
    /// Returns [`DictError::Io`] when the file cannot be read and any of the
    /// structural or checksum errors of [`Reader::parse`] when it is not a valid
    /// container.
    pub fn open_with(path: &Path, verify: Verify) -> Result<Self, DictError> {
        let bytes = fs::read(path)?;
        Self::parse(bytes, verify)
    }
}

impl<D: AsRef<[u8]>> Reader<D> {
    /// Parses `data` as a version-1 container.
    ///
    /// # Errors
    /// Returns [`DictError::MagicMismatch`] for a wrong magic,
    /// [`DictError::FormatVersion`] when the version is not 1,
    /// [`DictError::LengthOutOfRange`] when the header, the section table or a
    /// section length disagrees with the layout (including a file that is
    /// truncated or longer than it claims), and [`DictError::Crc`] when
    /// verification is on and a checksum does not match.
    pub fn parse(data: D, verify: Verify) -> Result<Self, DictError> {
        let header = parse_header(data.as_ref())?;
        let sections = parse_section_table(data.as_ref())?;
        let reader = Self {
            data,
            header,
            sections,
        };

        let entries = reader.section(SectionKind::Entries)?;
        let expected_entries = (header.entry_count as usize)
            .checked_mul(ENTRY_SIZE)
            .ok_or(DictError::LengthOutOfRange {
                field: "entries_len",
                value: header.entry_count as u64,
            })?;
        if entries.len() != expected_entries {
            return Err(DictError::LengthOutOfRange {
                field: "entries_len",
                value: entries.len() as u64,
            });
        }
        let unigram = reader.section(SectionKind::Unigram)?;
        let expected_unigram = (header.entry_count as usize)
            .checked_mul(UNIGRAM_ENTRY_SIZE)
            .ok_or(DictError::LengthOutOfRange {
                field: "unigram_len",
                value: header.entry_count as u64,
            })?;
        if unigram.len() != expected_unigram {
            return Err(DictError::LengthOutOfRange {
                field: "unigram_len",
                value: unigram.len() as u64,
            });
        }
        if reader.section(SectionKind::WordList)?.len() % 4 != 0 {
            return Err(DictError::LengthOutOfRange {
                field: "wordlist_len",
                value: reader.section(SectionKind::WordList)?.len() as u64,
            });
        }
        let has_bigram = header.flags & FLAG_BIGRAM_PRESENT != 0;
        if header.format_version == FORMAT_VERSION_V1 && has_bigram {
            return Err(DictError::LengthOutOfRange {
                field: "bigram_flags",
                value: u64::from(header.flags),
            });
        }
        if has_bigram != !reader.sections[SectionKind::Bigram.as_raw() as usize - 1].is_absent() {
            return Err(DictError::LengthOutOfRange {
                field: "bigram_flags",
                value: u64::from(header.flags),
            });
        }
        if has_bigram {
            let bigram = reader.section(SectionKind::Bigram)?;
            if bigram.len() % BIGRAM_ENTRY_SIZE != 0 {
                return Err(DictError::LengthOutOfRange {
                    field: "bigram_len",
                    value: bigram.len() as u64,
                });
            }
            for index in 1..bigram.len() / BIGRAM_ENTRY_SIZE {
                let (previous, record) = (
                    reader.bigram(index as u32 - 1)?,
                    reader.bigram(index as u32)?,
                );
                if record._pad != 0 {
                    return Err(DictError::LengthOutOfRange {
                        field: "bigram_pad",
                        value: u64::from(record._pad),
                    });
                }
                if previous.pair_key > record.pair_key {
                    return Err(DictError::LengthOutOfRange {
                        field: "bigram_order",
                        value: record.pair_key,
                    });
                }
            }
        }

        if verify == Verify::Full {
            let body =
                reader
                    .data
                    .as_ref()
                    .get(HEADER_SIZE..)
                    .ok_or(DictError::LengthOutOfRange {
                        field: "file_len",
                        value: reader.data.as_ref().len() as u64,
                    })?;
            let actual = crc32(body);
            if actual != header.file_crc32 {
                return Err(DictError::Crc {
                    expected: header.file_crc32,
                    actual,
                });
            }
            for entry in reader.sections.iter().filter(|entry| !entry.is_absent()) {
                let bytes = reader.section(entry.kind)?;
                let actual = crc32(bytes);
                if actual != entry.crc32 {
                    return Err(DictError::Crc {
                        expected: entry.crc32,
                        actual,
                    });
                }
            }
        }
        Ok(reader)
    }

    /// Returns the verified header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Returns the number of word entries in the container.
    pub fn entry_count(&self) -> u32 {
        self.header.entry_count
    }

    /// Returns the verified section table.
    pub fn sections(&self) -> &[SectionEntry; SECTION_COUNT] {
        &self.sections
    }

    /// Returns the bytes of `kind`, or an empty slice when the section is absent.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the recorded range does not
    /// lie inside the image, which the parse step already rejects but a
    /// hand-built [`SectionEntry`] could still describe.
    pub fn section(&self, kind: SectionKind) -> Result<&[u8], DictError> {
        let entry = self.sections[kind.as_raw() as usize - 1];
        if entry.is_absent() {
            return Ok(&[]);
        }
        let start = usize::try_from(entry.offset).map_err(|_| DictError::LengthOutOfRange {
            field: "section_offset",
            value: entry.offset,
        })?;
        let len = usize::try_from(entry.len).map_err(|_| DictError::LengthOutOfRange {
            field: "section_len",
            value: entry.len,
        })?;
        let end = start.checked_add(len).ok_or(DictError::LengthOutOfRange {
            field: "section_len",
            value: entry.len,
        })?;
        self.data
            .as_ref()
            .get(start..end)
            .ok_or(DictError::LengthOutOfRange {
                field: "section_offset",
                value: entry.offset,
            })
    }

    /// Decodes the word record at `index`.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when `index` is past the entry
    /// count or when the record does not satisfy the layout contract.
    pub fn entry(&self, index: u32) -> Result<DictEntry, DictError> {
        if index >= self.header.entry_count {
            return Err(DictError::LengthOutOfRange {
                field: "entry_index",
                value: u64::from(index),
            });
        }
        let entries = self.section(SectionKind::Entries)?;
        let at = index as usize * ENTRY_SIZE;
        let bytes = entries
            .get(at..at + ENTRY_SIZE)
            .ok_or(DictError::LengthOutOfRange {
                field: "entry_offset",
                value: at as u64,
            })?;
        DictEntry::decode(bytes)
    }

    /// Returns the text of `entry`, borrowed from the string pool.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the recorded range leaves the
    /// string pool or the bytes are not valid UTF-8.
    pub fn word(&self, entry: &DictEntry) -> Result<&str, DictError> {
        let pool = self.section(SectionKind::StrPool)?;
        let start = entry.word_off as usize;
        let end =
            start
                .checked_add(entry.word_len as usize)
                .ok_or(DictError::LengthOutOfRange {
                    field: "word_off",
                    value: u64::from(entry.word_off),
                })?;
        let bytes = pool.get(start..end).ok_or(DictError::LengthOutOfRange {
            field: "word_off",
            value: u64::from(entry.word_off),
        })?;
        std::str::from_utf8(bytes).map_err(|_| DictError::LengthOutOfRange {
            field: "word_utf8",
            value: u64::from(entry.word_len),
        })
    }

    /// Returns the text of the word at `index`.
    ///
    /// # Errors
    /// Returns the errors of [`Reader::entry`] and [`Reader::word`].
    pub fn word_at(&self, index: u32) -> Result<&str, DictError> {
        let entry = self.entry(index)?;
        self.word(&entry)
    }

    /// Returns the `(hash, prob_q12)` record at `index` of the unigram table.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when `index` is past the entry
    /// count.
    pub fn unigram(&self, index: u32) -> Result<(u32, u16), DictError> {
        if index >= self.header.entry_count {
            return Err(DictError::LengthOutOfRange {
                field: "unigram_index",
                value: u64::from(index),
            });
        }
        let table = self.section(SectionKind::Unigram)?;
        let at = index as usize * UNIGRAM_ENTRY_SIZE;
        Ok((
            format::read_u32(table, at, "unigram_hash")?,
            format::read_u16(table, at + 4, "unigram_prob")?,
        ))
    }

    /// Looks a word up in the unigram table.
    ///
    /// The table is sorted by [`hash_word`], so the search is a binary search; a
    /// hash collision is resolved by comparing the candidate's text, which is
    /// possible because a `word_id` is the index into the hash-ordered `ENTRIES`
    /// table as well.
    ///
    /// # Errors
    /// Returns the errors of [`Reader::unigram`] and [`Reader::word_at`] for a
    /// table that is internally inconsistent.
    pub fn unigram_lookup(&self, word: &str) -> Result<Option<(u32, u16)>, DictError> {
        let target = hash_word(word);
        let mut low = 0usize;
        let mut high = self.header.entry_count as usize;
        while low < high {
            let middle = low + (high - low) / 2;
            let (hash, prob) = self.unigram(middle as u32)?;
            if hash == target {
                if self.word_at(middle as u32)? == word {
                    return Ok(Some((hash, prob)));
                }
                // A colliding word sits next to this one; scan the equal-hash run.
                let mut index = middle;
                while index > 0 {
                    index -= 1;
                    let (hash, prob) = self.unigram(index as u32)?;
                    if hash != target {
                        break;
                    }
                    if self.word_at(index as u32)? == word {
                        return Ok(Some((hash, prob)));
                    }
                }
                let mut index = middle + 1;
                while index < self.header.entry_count as usize {
                    let (hash, prob) = self.unigram(index as u32)?;
                    if hash != target {
                        break;
                    }
                    if self.word_at(index as u32)? == word {
                        return Ok(Some((hash, prob)));
                    }
                    index += 1;
                }
                return Ok(None);
            }
            if hash < target {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(None)
    }

    /// Returns the number of bigram records in the container.
    pub fn bigram_count(&self) -> usize {
        self.section(SectionKind::Bigram)
            .map_or(0, |table| table.len() / BIGRAM_ENTRY_SIZE)
    }

    /// Returns the bigram record at `index`.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when `index` is past the section or
    /// when the fixed-width row is truncated.
    pub fn bigram(&self, index: u32) -> Result<BigramRecord, DictError> {
        let table = self.section(SectionKind::Bigram)?;
        let at = usize::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(BIGRAM_ENTRY_SIZE))
            .ok_or(DictError::LengthOutOfRange {
                field: "bigram_index",
                value: u64::from(index),
            })?;
        if at + BIGRAM_ENTRY_SIZE > table.len() {
            return Err(DictError::LengthOutOfRange {
                field: "bigram_index",
                value: u64::from(index),
            });
        }
        Ok(BigramRecord {
            pair_key: format::read_u64(table, at, "bigram_pair_key")?,
            prob_q12: format::read_u16(table, at + 8, "bigram_prob")?,
            _pad: format::read_u16(table, at + 10, "bigram_pad")?,
        })
    }

    /// Returns the number of `word_id` pairs in the word-list section.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the section range is invalid.
    pub fn word_list_len(&self) -> Result<usize, DictError> {
        Ok(self.section(SectionKind::WordList)?.len() / 4)
    }

    /// Returns the `word_id` at `index` of the word-list section.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when `index` is past the end of the
    /// section.
    pub fn word_list_id(&self, index: usize) -> Result<u32, DictError> {
        let table = self.section(SectionKind::WordList)?;
        let at = index.checked_mul(4).ok_or(DictError::LengthOutOfRange {
            field: "wordlist_index",
            value: index as u64,
        })?;
        format::read_u32(table, at, "wordlist_entry")
    }

    /// Builds the zero-copy FST map over the `FST` section.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the section is absent and
    /// [`DictError::Fst`] when its bytes are not a valid FST.
    pub fn fst_map(&self) -> Result<fst::Map<&[u8]>, DictError> {
        let bytes = self.section(SectionKind::Fst)?;
        if bytes.is_empty() {
            return Err(DictError::LengthOutOfRange {
                field: "fst_len",
                value: 0,
            });
        }
        fst::Map::new(bytes).map_err(|err| DictError::Fst(err.to_string()))
    }

    /// Returns the words stored under `key`, in ranking order.
    ///
    /// This is the read path a `Lexicon` implementation follows: one FST lookup
    /// for the packed range, then one word-list read per candidate.
    ///
    /// # Errors
    /// Returns the errors of [`Reader::fst_map`], [`Reader::word_list_id`] and
    /// [`Reader::word_at`]; an unknown key is not an error and yields an empty
    /// vector.
    pub fn key_words(&self, key: &str) -> Result<Vec<&str>, DictError> {
        let map = self.fst_map()?;
        let Some(packed) = map.get(key.as_bytes()) else {
            return Ok(Vec::new());
        };
        let (start, count) = unpack_fst_value(packed);
        let mut words = Vec::with_capacity(count as usize);
        for offset in 0..u64::from(count) {
            let index = start
                .checked_add(offset)
                .ok_or(DictError::LengthOutOfRange {
                    field: "wordlist_start",
                    value: start,
                })?;
            let index = usize::try_from(index).map_err(|_| DictError::LengthOutOfRange {
                field: "wordlist_start",
                value: start,
            })?;
            let id = self.word_list_id(index)?;
            words.push(self.word_at(id)?);
        }
        Ok(words)
    }
}

#[cfg(test)]
mod tests;
