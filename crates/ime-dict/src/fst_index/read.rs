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
//! done so), ranks a word, or allocates a word list of its own -- a lookup writes into the
//! buffer its caller already holds, which is what keeps the decode path off the allocator.
//! Turning a record into its text is the entry table's job rather than this module's: the
//! conversion is the one step every candidate passes through, and keeping it in a place of
//! its own is what lets it be tested on its own.

use ime_types::DictError;

use crate::format::{DictEntry, ENTRY_SIZE, read_u32, unpack_fst_value};

use super::{FstLexicon, WORD_ID_SIZE, WordBuf, out_of_range};

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

    /// Resolves `key` to the word ids it owns, and how many of them there are.
    ///
    /// Answers `None` for a key the index does not hold, which is not a failure: a miss is
    /// an empty word list. Every bounds check the read path makes about the packed value
    /// lives here, so the readers of a key share one copy of them.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the packed range leaves the word list.
    fn word_ids(&self, key: &str) -> Result<Option<(&[u8], usize)>, DictError> {
        let Some(packed) = self.fst.get(key.as_bytes()) else {
            return Ok(None);
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
        Ok(Some((ids, count)))
    }

    /// Reads at most `limit` words of `key` into `out`, which the caller owns.
    ///
    /// The decode path asks for a bounded head of the list, so materializing the whole
    /// thing into a fresh vector per lookup was pure waste: this variant writes into a
    /// buffer the caller already holds and stops at `limit`, which is what removes the
    /// allocation from the lookup path. The buffer keeps the words inline while they fit
    /// and spills to the heap only for a list longer than that, so a long list is served
    /// whole rather than cut short.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the packed range leaves the word list,
    /// when a word id leaves the entry table, or when a record's text leaves the string
    /// pool. An unknown key is not an error: it writes no words.
    pub(super) fn read_words_into<'a>(
        &'a self,
        key: &str,
        limit: usize,
        out: &mut WordBuf<'a>,
    ) -> Result<(), DictError> {
        let Some((ids, count)) = self.word_ids(key)? else {
            return Ok(());
        };
        for index in 0..count.min(limit) {
            let id = read_u32(ids, index * WORD_ID_SIZE, "wordlist_entry")?;
            let entry = self.entry(id)?;
            // The record becomes a word through the entry table, which checks it against the
            // string pool on every access; this loop never slices the pool itself, so there is
            // exactly one place where a damaged record can turn into a word.
            out.push(self.entry_to_ref(&entry)?);
        }
        Ok(())
    }
}
