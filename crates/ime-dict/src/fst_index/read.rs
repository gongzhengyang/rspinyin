//! The per-record read path: one word list, one entry record, one word text.
//!
//! Responsibility: decode the three levels of indirection a key resolves through -- the
//! word ids its packed FST value points at, the fixed-width entry record behind each id,
//! and the text that record names in the string pool -- and report every value that
//! leaves its section instead of reading it.
//!
//! Boundaries: this module reads and does not decide. The container load, the trait
//! implementation and the error translation stay in the parent, so what is left here is
//! the walk over the views; nothing here verifies the container (the loader has already
//! done so), ranks a word, or allocates more than the vector one key's candidates are
//! collected into.

use ime_types::{DictError, WordFlags, WordRef};

use crate::format::{DictEntry, ENTRY_SIZE, MAX_WORDS_PER_KEY, read_u32, unpack_fst_value};

use super::{FstLexicon, WORD_ID_SIZE, out_of_range};

impl FstLexicon {
    /// Decodes the word record at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when `index` leaves the entry table or
    /// when the record does not satisfy the layout contract.
    fn entry(&self, index: u32) -> Result<DictEntry, DictError> {
        if index >= self.entry_count {
            return Err(out_of_range("entry_index", u64::from(index)));
        }
        let Ok(entry_at) = usize::try_from(index) else {
            return Err(out_of_range("entry_index", u64::from(index)));
        };
        let Some(at) = entry_at.checked_mul(ENTRY_SIZE) else {
            return Err(out_of_range("entry_index", u64::from(index)));
        };
        // The reader has verified that `ENTRIES` is exactly `entry_count` records
        // long, so this range is inside the section; the check stays because a section
        // table is untrusted input and this function reports rather than panics.
        let Some(bytes) = self.entries.get(at..at + ENTRY_SIZE) else {
            return Err(out_of_range("entry_offset", u64::from(index)));
        };
        DictEntry::decode(bytes)
    }

    /// Returns the text of `entry`, borrowed from the mapped string pool.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the recorded range leaves the
    /// string pool or when the bytes are not valid UTF-8.
    fn word<'a>(&'a self, entry: &DictEntry) -> Result<&'a str, DictError> {
        let Ok(start) = usize::try_from(entry.word_off) else {
            return Err(out_of_range("word_off", u64::from(entry.word_off)));
        };
        let Some(end) = start.checked_add(usize::from(entry.word_len)) else {
            return Err(out_of_range("word_off", u64::from(entry.word_off)));
        };
        let Some(bytes) = self.strpool.get(start..end) else {
            return Err(out_of_range("word_off", u64::from(entry.word_off)));
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Err(out_of_range("word_utf8", u64::from(entry.word_len)));
        };
        Ok(text)
    }

    /// Reads the word list of `key` out of the mapped sections.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the packed range leaves the word list,
    /// when a word id leaves the entry table, or when a record's text leaves the string
    /// pool. An unknown key is not an error: it yields no words.
    pub(super) fn read_words(&self, key: &str) -> Result<Vec<WordRef<'_>>, DictError> {
        let Some(packed) = self.fst.get(key.as_bytes()) else {
            return Ok(Vec::new());
        };
        let (start, count) = unpack_fst_value(packed);
        let Ok(start) = usize::try_from(start) else {
            return Err(out_of_range("wordlist_start", start));
        };
        let Ok(count) = usize::try_from(count) else {
            return Err(out_of_range("wordlist_count", u64::from(count)));
        };
        // The packed range is checked in bytes against the word list itself. This is
        // the check that keeps a crafted FST value from reading past the section, so it
        // runs before a single word id is decoded.
        let Some(from) = start.checked_mul(WORD_ID_SIZE) else {
            return Err(out_of_range("wordlist_start", start as u64));
        };
        let Some(ids_end) = start.checked_add(count) else {
            return Err(out_of_range("wordlist_count", count as u64));
        };
        let Some(to) = ids_end.checked_mul(WORD_ID_SIZE) else {
            return Err(out_of_range("wordlist_count", count as u64));
        };
        let Some(ids) = self.wordlist.get(from..to) else {
            return Err(out_of_range("wordlist_start", start as u64));
        };

        // `count * WORD_ID_SIZE` bytes were checked above, so this reservation is bounded
        // by the section that exists and the loop cannot leave the slice. The capacity is
        // only a hint: the container's per-key ceiling is a ranking decision, not a format
        // limit, so a longer list still reads.
        let mut words = Vec::with_capacity(count.min(MAX_WORDS_PER_KEY as usize));
        for index in 0..count {
            let id = read_u32(ids, index * WORD_ID_SIZE, "wordlist_entry")?;
            let entry = self.entry(id)?;
            words.push(WordRef {
                text: self.word(&entry)?,
                weight: entry.weight,
                syl_count: entry.syl_count,
                flags: WordFlags::from_bits_truncate(entry.flags),
            });
        }
        Ok(words)
    }
}
