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
    self, DictEntry, ENTRY_SIZE, FLAG_BIGRAM_PRESENT, HEADER_SIZE, Header, SECTION_COUNT,
    SectionEntry, SectionKind, UNIGRAM_ENTRY_SIZE, crc32, hash_word, parse_header,
    parse_section_table, unpack_fst_value,
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
        if has_bigram != !reader.sections[SectionKind::Bigram.as_raw() as usize - 1].is_absent() {
            return Err(DictError::LengthOutOfRange {
                field: "bigram_flags",
                value: u64::from(header.flags),
            });
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
mod tests {
    use super::*;
    use crate::format::writer::DictWriter;

    /// Builds a small but complete container: three words, three keys.
    ///
    /// Word ids follow hash order, exactly as the compiler assigns them, so the
    /// unigram table is sorted by hash and `ENTRIES` and `UNIGRAM` share an index.
    fn sample_image() -> Vec<u8> {
        let mut words: Vec<&str> = vec!["中", "中国", "中心"];
        words.sort_by_key(|word| (hash_word(word), *word));
        let id_of = |needle: &str| {
            words
                .iter()
                .position(|word| *word == needle)
                .expect("the fixture holds this word") as u32
        };

        let mut strpool = Vec::new();
        let mut entries = Vec::new();
        let mut unigram = Vec::new();
        for (index, word) in words.iter().enumerate() {
            let offset = strpool.len() as u32;
            strpool.extend_from_slice(word.as_bytes());
            let syllables = if word.chars().count() == 1 { 1 } else { 2 };
            entries.extend_from_slice(
                &DictEntry::new(offset, word.len() as u16, syllables, 0, 100 - index as u32)
                    .encode(),
            );
            unigram.extend_from_slice(&hash_word(word).to_le_bytes());
            unigram.extend_from_slice(&format::PROB_Q12_MAX.to_le_bytes());
            unigram.extend_from_slice(&0u16.to_le_bytes());
        }

        // Keys in byte order, each range pointing at its words in the word list.
        let all: Vec<u32> = (0..words.len() as u32).collect();
        let keys: [(&str, &[u32]); 3] = [
            ("zhong", &all),
            ("zhong'guo", &[id_of("中国")]),
            ("zhong'xin", &[id_of("中心")]),
        ];
        let mut wordlist = Vec::new();
        let mut fst_builder = fst::MapBuilder::memory();
        for (key, ids) in keys {
            let start = (wordlist.len() / 4) as u64;
            for id in ids {
                wordlist.extend_from_slice(&id.to_le_bytes());
            }
            fst_builder
                .insert(
                    key,
                    format::pack_fst_value(start, ids.len() as u32).expect("packing"),
                )
                .expect("inserting");
        }
        let fst_bytes = fst_builder.into_inner().expect("finishing the FST");

        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::Fst, fst_bytes)
            .expect("fst");
        writer
            .add_section(SectionKind::Entries, entries)
            .expect("entries");
        writer
            .add_section(SectionKind::StrPool, strpool)
            .expect("strpool");
        writer
            .add_section(SectionKind::Unigram, unigram)
            .expect("unigram");
        writer
            .add_section(SectionKind::WordList, wordlist)
            .expect("wordlist");
        writer.encode().expect("encoding")
    }

    /// Returns the word id the reader stores under `word`, by scanning the table.
    fn id_of(reader: &Reader<Vec<u8>>, word: &str) -> u32 {
        for index in 0..reader.entry_count() {
            if reader.word_at(index).expect("word") == word {
                return index;
            }
        }
        panic!("{word} is not in the container");
    }

    #[test]
    fn test_round_trip_reads_back_every_section_byte_for_byte() {
        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::Fst, vec![7u8; 24])
            .expect("fst");
        writer
            .add_section(SectionKind::StrPool, b"abcdefghij".to_vec())
            .expect("strpool");
        writer
            .add_section(SectionKind::WordList, vec![0u8; 8])
            .expect("wordlist");
        let image = writer.encode().expect("encoding");

        let reader = Reader::parse(image.clone(), Verify::Full).expect("parsing");
        for kind in [
            SectionKind::Fst,
            SectionKind::StrPool,
            SectionKind::WordList,
            SectionKind::Entries,
            SectionKind::Unigram,
            SectionKind::Bigram,
        ] {
            let expected = writer.section(kind).unwrap_or_default();
            assert_eq!(
                reader.section(kind).expect("section"),
                expected,
                "{kind:?} must round-trip byte for byte"
            );
        }
    }

    #[test]
    fn test_parse_reads_entries_words_and_keys() {
        let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
        assert_eq!(reader.entry_count(), 3);

        let single = reader.entry(id_of(&reader, "中")).expect("entry");
        assert_eq!(reader.word(&single).expect("word"), "中");
        assert_eq!(single.syl_count, 1);
        let compound = reader.entry(id_of(&reader, "中国")).expect("entry");
        assert_eq!(reader.word(&compound).expect("word"), "中国");
        assert_eq!(compound.syl_count, 2);
        assert_eq!(compound.word_len as usize, "中国".len());

        let words = reader.key_words("zhong'guo").expect("lookup");
        assert_eq!(words, vec!["中国"]);
        let all = reader.key_words("zhong").expect("lookup");
        assert_eq!(all.len(), 3);
        assert!(reader.key_words("meiyou").expect("missing key").is_empty());
    }

    #[test]
    fn test_unigram_lookup_finds_words_by_hash() {
        let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
        let found = reader.unigram_lookup("中心").expect("lookup");
        assert_eq!(found, Some((hash_word("中心"), format::PROB_Q12_MAX)));
        assert_eq!(reader.unigram_lookup("不存在").expect("miss"), None);
    }

    #[test]
    fn test_parse_rejects_a_corrupted_magic() {
        let mut image = sample_image();
        image[0] = b'X';
        assert!(matches!(
            Reader::parse(image, Verify::Full),
            Err(DictError::MagicMismatch)
        ));
    }

    #[test]
    fn test_parse_rejects_an_unsupported_format_version() {
        let mut image = sample_image();
        image[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            Reader::parse(image, Verify::Full),
            Err(DictError::FormatVersion { found: 2 })
        ));
    }

    #[test]
    fn test_parse_rejects_a_corrupted_section_byte() {
        for kind in [
            SectionKind::Fst,
            SectionKind::Entries,
            SectionKind::StrPool,
            SectionKind::Unigram,
            SectionKind::WordList,
        ] {
            let mut image = sample_image();
            let entry = parse_section_table(&image)
                .expect("table")
                .iter()
                .copied()
                .find(|entry| entry.kind == kind)
                .expect("section present");
            let at = entry.offset as usize + entry.len as usize - 1;
            image[at] ^= 0xFF;
            assert!(
                matches!(
                    Reader::parse(image, Verify::Full),
                    Err(DictError::Crc { .. })
                ),
                "{kind:?}: a flipped byte must fail the checksum"
            );
        }
    }

    #[test]
    fn test_parse_rejects_a_corrupted_section_table() {
        let mut image = sample_image();
        // Point the FST section at the header: inside the file, but not a section.
        image[HEADER_SIZE + 8..HEADER_SIZE + 16].copy_from_slice(&8u64.to_le_bytes());
        assert!(matches!(
            Reader::parse(image, Verify::Full),
            Err(DictError::LengthOutOfRange {
                field: "section_offset",
                ..
            })
        ));
    }

    #[test]
    fn test_parse_rejects_an_entry_count_that_disagrees_with_the_table() {
        let mut image = sample_image();
        image[28..32].copy_from_slice(&7u32.to_le_bytes());
        assert!(matches!(
            Reader::parse(image, Verify::Full),
            Err(DictError::LengthOutOfRange {
                field: "entries_len",
                ..
            })
        ));
    }

    #[test]
    fn test_parse_rejects_a_truncated_file() {
        let image = sample_image();
        let short = image[..image.len() - 8].to_vec();
        assert!(matches!(
            Reader::parse(short, Verify::Full),
            Err(DictError::LengthOutOfRange { .. })
        ));
        let empty: Vec<u8> = Vec::new();
        assert!(matches!(
            Reader::parse(empty, Verify::Full),
            Err(DictError::LengthOutOfRange {
                field: "file_len",
                ..
            })
        ));
    }

    #[test]
    fn test_header_only_verification_skips_checksums_but_keeps_structure() {
        let mut image = sample_image();
        image[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(
            Reader::parse(image.clone(), Verify::Full).is_err(),
            "full verification must reject the wrong section checksum"
        );
        let reader = Reader::parse(image, Verify::Header).expect("header-only parse");
        assert_eq!(reader.entry_count(), 3);
    }

    #[test]
    fn test_accessors_reject_out_of_range_requests() {
        let reader = Reader::parse(sample_image(), Verify::Full).expect("parsing");
        assert!(matches!(
            reader.entry(3),
            Err(DictError::LengthOutOfRange {
                field: "entry_index",
                ..
            })
        ));
        assert!(matches!(
            reader.unigram(99),
            Err(DictError::LengthOutOfRange {
                field: "unigram_index",
                ..
            })
        ));
        assert!(matches!(
            reader.word_list_id(9),
            Err(DictError::LengthOutOfRange {
                field: "wordlist_entry",
                ..
            })
        ));
        // The word list holds one entry per (key, word) pair, not one per word: a word
        // reachable under several keys appears several times, which is the whole reason
        // the section exists separately from the entry table.
        assert_eq!(reader.word_list_len().expect("word list length"), 5);

        // A record whose text range leaves the pool is reported, not read.
        let escaping = DictEntry::new(1_000, 3, 1, 0, 0);
        assert!(matches!(
            reader.word(&escaping),
            Err(DictError::LengthOutOfRange {
                field: "word_off",
                ..
            })
        ));
    }

    #[test]
    fn test_word_rejects_a_range_that_is_not_utf8() {
        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::StrPool, vec![0xFF, 0xFE, 0xFD])
            .expect("strpool");
        writer
            .add_section(
                SectionKind::Entries,
                DictEntry::new(0, 3, 1, 0, 1).encode().to_vec(),
            )
            .expect("entries");
        writer
            .add_section(SectionKind::Unigram, vec![0u8; UNIGRAM_ENTRY_SIZE])
            .expect("unigram");
        let image = writer.encode().expect("encoding");
        let reader = Reader::parse(image, Verify::Full).expect("parsing");
        assert!(matches!(
            reader.word_at(0),
            Err(DictError::LengthOutOfRange {
                field: "word_utf8",
                ..
            })
        ));
    }

    #[test]
    fn test_fst_map_reports_an_absent_section() {
        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::StrPool, b"ab".to_vec())
            .expect("strpool");
        let image = writer.encode().expect("encoding");
        let reader = Reader::parse(image, Verify::Full).expect("parsing");
        assert!(matches!(
            reader.fst_map(),
            Err(DictError::LengthOutOfRange {
                field: "fst_len",
                ..
            })
        ));
    }

    #[test]
    fn test_open_reports_a_missing_file_as_an_io_error() {
        let path = std::env::temp_dir().join("rspinyin-dict-reader-does-not-exist.dict");
        let result = Reader::open(&path);
        assert!(
            matches!(result, Err(DictError::Io(_))),
            "a missing file must surface as an IO error"
        );
    }
}
