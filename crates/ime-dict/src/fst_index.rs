//! The FST lexicon: a compiled dictionary queried through its index.
//!
//! Responsibility: map `base.dict`, verify it through the container reader, keep the five
//! zero-copy views the read path needs -- the FST, the entry table, the string pool, the
//! word list and the unigram table -- and answer the [`Lexicon`] queries the decoder makes.
//! It does not compile, write or repair a dictionary, it does not know how a word is
//! scored, and it never copies a word: every [`WordRef`] it hands out borrows the mapping.
//!
//! # The read path
//!
//! One lookup is one FST lookup plus one word-list read per candidate:
//!
//! ```text
//! key -> fst::Map::get -> packed (wordlist_start, count)
//!     -> WORDLIST[start .. start + count] -> word_id
//!     -> ENTRIES[word_id] -> (word_off, word_len, syl_count, flags, weight) -> STRPOOL -> &str
//! ```
//!
//! The compiler writes each key's words into `WORDLIST` in descending weight, so the
//! iteration order *is* the ranking order and the read path never sorts. The indirection
//! through `WORDLIST` is what lets one word answer to several keys: a polyphone word is
//! reachable under every key it is read by, which is why the word list is a section of its
//! own.
//!
//! # Trust model
//!
//! A dictionary is untrusted input. Every offset read out of the FST, the word list, the
//! entry table or a word record is bounds-checked before it is used, and a value that
//! leaves its section is reported as [`DictError::LengthOutOfRange`] rather than read, so a
//! corrupted or hand-edited file can make a lookup fail but cannot make one read out of
//! bounds. The checksums the loader verifies stop an *accidental* corruption; they are
//! deliberately not what the bounds checks rely on, since a crafted file can carry
//! consistent checksums.

use std::path::{Path, PathBuf};

use ime_core::segment::syllable_at;
use ime_types::lexicon::WORD_ITER_INLINE;
use ime_types::{DictError, ImeError, Lexicon, SyllableId, WordFlags, WordIter, WordRef};

use crate::entry::{EntryTable, MalformedEntry};
use crate::format::{
    DictEntry, MAX_WORDS_PER_KEY, SectionEntry, SectionKind, UNIGRAM_ENTRY_SIZE, read_u16,
    read_u32,
    reader::{Reader, Verify},
};
use crate::mmap::{MappedFile, WordPool};

mod read;

/// Width of one `WORDLIST` element: a `word_id` is a little-endian `u32`.
const WORD_ID_SIZE: usize = 4;

/// The word a slot holds before anything is written into it.
///
/// [`WordRef`] has no `Default` -- an empty word is not a dictionary entry -- but the
/// inline slots of a [`WordBuf`] need a value to start from. Only the slots below the
/// live length are read, so the placeholder is never handed out.
const EMPTY_WORD: WordRef<'static> = WordRef {
    text: "",
    weight: 0,
    syl_count: 0,
    flags: WordFlags::empty(),
};

/// A word list a lookup writes into, held inline up to [`WORD_ITER_INLINE`] words.
///
/// The decode path reads the head of a key's list, and the head is short: eight words
/// cover the keys the lattice reads in the common case, so materializing a fresh vector
/// per lookup paid an allocation for nothing. A list longer than the inline capacity
/// spills to the heap rather than being cut short, so everything the caller asked for is
/// still there.
///
/// The spill is a plain `Vec` rather than a `SmallVec` because this crate does not depend
/// on `smallvec`: the contract crate's dependency list is fixed at three crates, and this
/// crate's manifest is not the read path's to change. Replacing the two storage fields
/// with `SmallVec<[WordRef<'a>; WORD_ITER_INLINE]>` would be a manifest change with no
/// behaviour change.
struct WordBuf<'a> {
    /// The words while they fit.
    inline: [WordRef<'a>; WORD_ITER_INLINE],
    /// Live length of `inline`; unused once `spill` holds the list.
    len: usize,
    /// The list once it is longer than the inline capacity holds.
    spill: Vec<WordRef<'a>>,
}

impl<'a> WordBuf<'a> {
    /// Creates an empty buffer.
    ///
    /// Nothing is allocated: the inline slots are a plain array, and an empty `Vec` has no
    /// backing store until something is pushed into it.
    fn new() -> Self {
        Self {
            inline: [EMPTY_WORD; WORD_ITER_INLINE],
            len: 0,
            spill: Vec::new(),
        }
    }

    /// Appends one word, moving the list to the heap once the inline slots are full.
    fn push(&mut self, word: WordRef<'a>) {
        if self.spill.is_empty() {
            if let Some(slot) = self.inline.get_mut(self.len) {
                *slot = word;
                self.len += 1;
                return;
            }
            // The slots are full, so the list is longer than a decode ever reads: it moves
            // to the heap rather than being cut short. The slots are copied over first,
            // which keeps the words in the order the key stores them -- strongest first.
            self.spill = Vec::with_capacity(WORD_ITER_INLINE.saturating_mul(2));
            self.spill.extend_from_slice(&self.inline[..self.len]);
        }
        self.spill.push(word);
    }

    /// Turns the buffer into the iterator the contract hands out.
    ///
    /// A list that fit the inline slots is copied into the iterator's own array, so this
    /// lookup needed no allocation and the buffer is free for the next one.
    fn into_word_iter(self) -> WordIter<'a> {
        if self.spill.is_empty() {
            WordIter::from_slice(&self.inline[..self.len])
        } else {
            WordIter::from_vec(self.spill)
        }
    }
}

/// Builds the bounds failure every check in this module reports.
///
/// `field` names the part of the container that was out of range and `value` is the
/// number that was rejected, so a diagnostic can say what failed without the error
/// carrying any of the file's contents.
fn out_of_range(field: &'static str, value: u64) -> DictError {
    DictError::LengthOutOfRange { field, value }
}

/// A dictionary queried through its FST index, over a read-only mapping.
///
/// The value owns the mapping and every view into it, so a [`WordRef`] it returns
/// borrows the mapping for as long as the lexicon lives and nothing is copied on the
/// read path.
///
/// The views are byte slices rather than typed ones: reinterpreting a borrowed buffer as
/// `&[DictEntry]` or `&[u32]` would be the unchecked read the format layer refuses to
/// perform, so records are decoded field by field instead. The string pool is the exception,
/// and it is not a slice either: it is held as the value that carries the proof it came out
/// of a container the loader accepted, which is what lets the read path hand out `&str`
/// without scanning it.
///
/// Declaration order is drop order: the views are dropped before the mapping that backs
/// them, so no destructor can run against an unmapped range even if a view ever gains
/// one that reads its bytes.
#[derive(Debug)]
pub struct FstLexicon {
    fst: fst::Map<&'static [u8]>,
    entries: &'static [u8],
    /// The string pool plus the once-only list of the records that failed their check.
    ///
    /// Every word this lexicon hands out is a `&str` into the pool this table holds, which is
    /// why the table -- and not the raw `STRPOOL` bytes -- is what the read path keeps.
    table: EntryTable<'static>,
    wordlist: &'static [u8],
    unigram: &'static [u8],
    entry_count: u32,
    /// Where the dictionary was loaded from, so that a failure can name the file the
    /// user has to replace.
    path: PathBuf,
    /// The read-only mapping every view above borrows from.
    ///
    /// Held as a field rather than as a local because dropping it would unmap the file and
    /// leave the `'static` views dangling, and it is declared last because fields drop in
    /// declaration order — so it outlives the views it owns. Nothing reads it after
    /// construction, which is the dead-code warning this allowance silences: the field's
    /// whole purpose is its lifetime.
    #[allow(dead_code)]
    mmap: MappedFile,
}

impl FstLexicon {
    /// Maps `path` and builds the lexicon over it, verifying the whole container.
    ///
    /// This is the load the addon performs at startup: the header, the section table
    /// and every checksum are verified before the first lookup, which costs one CRC32
    /// pass over the file body.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`FstLexicon::load_with`].
    pub fn load(path: &Path) -> Result<Self, DictError> {
        Self::load_with(path, Verify::Full)
    }

    /// Maps `path` and builds the lexicon over it, verifying as much as `verify` asks
    /// for.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Io`] when the file cannot be mapped,
    /// [`DictError::MagicMismatch`] when it is not a dictionary,
    /// [`DictError::FormatVersion`] when its version is not the one this build reads,
    /// [`DictError::LengthOutOfRange`] when a length, offset or alignment disagrees
    /// with the layout, [`DictError::Crc`] when a checksum fails under
    /// [`Verify::Full`], and [`DictError::Fst`] when the index section is not a valid
    /// FST.
    pub fn load_with(path: &Path, verify: Verify) -> Result<Self, DictError> {
        let mmap = MappedFile::open(path)?;
        // The views borrow the mapping for `'static`: that is the widening
        // `MappedFile::static_bytes` documents, and it holds because the lexicon owns
        // the mapping for its whole life, never borrows it mutably and never remaps it.
        let image = mmap.static_bytes();
        // The reader owns the layout checks -- magic, version, section table, section
        // lengths and, under full verification, every checksum. It is dropped at the end
        // of this function, which is why the views are sliced out of the image itself
        // rather than borrowed from the reader.
        let reader = Reader::parse(image, verify)?;
        let sections = reader.sections();
        let fst_bytes = section(image, sections, SectionKind::Fst)?;
        if fst_bytes.is_empty() {
            return Err(out_of_range("fst_len", 0));
        }
        let fst = match fst::Map::new(fst_bytes) {
            Ok(fst) => fst,
            Err(err) => return Err(DictError::Fst(err.to_string())),
        };
        // The pool is wrapped only here, and only after `Reader::parse` has accepted the
        // container: this call is what turns "these bytes came out of a dictionary that
        // passed its checks" into the value every later word conversion rests on.
        let strpool = section(image, sections, SectionKind::StrPool)?;
        Ok(Self {
            fst,
            entries: section(image, sections, SectionKind::Entries)?,
            table: EntryTable::new(WordPool::verified(strpool)),
            wordlist: section(image, sections, SectionKind::WordList)?,
            unigram: section(image, sections, SectionKind::Unigram)?,
            entry_count: reader.entry_count(),
            path: path.to_path_buf(),
            mmap,
        })
    }

    /// Returns the number of word records in the container.
    pub fn entry_count(&self) -> u32 {
        self.entry_count
    }

    /// Returns the `(hash, prob_q12)` record at `index` of the unigram table.
    ///
    /// The table is sorted by [`crate::format::hash_word`], so a caller can binary-search
    /// it for a word and score with the record it finds. This accessor is why the lexicon
    /// keeps the table in view at all: the language model is built on it.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when `index` leaves the table.
    pub fn unigram_at(&self, index: u32) -> Result<(u32, u16), DictError> {
        if index >= self.entry_count {
            return Err(out_of_range("unigram_index", u64::from(index)));
        }
        // A u32 widens to a usize on every target in the platform baseline; the fallible
        // form is used because the pair has no `From` impl and a 16-bit target would
        // otherwise truncate a section offset.
        let Ok(offset) = usize::try_from(index) else {
            return Err(out_of_range("unigram_index", u64::from(index)));
        };
        let Some(at) = offset.checked_mul(UNIGRAM_ENTRY_SIZE) else {
            return Err(out_of_range("unigram_offset", u64::from(index)));
        };
        Ok((
            read_u32(self.unigram, at, "unigram_hash")?,
            read_u16(self.unigram, at + 4, "unigram_prob")?,
        ))
    }

    /// Returns the word record `entry` names, borrowed from the mapped string pool.
    ///
    /// This is the conversion every candidate goes through: the record is checked against the
    /// pool on every call, so a record whose offset or length was damaged is reported instead
    /// of dereferenced, and the text that comes back is a `&str` into the mapping rather than a
    /// copy of it.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] naming the offending field when `word_len` is
    /// zero or above [`crate::format::MAX_WORD_LEN`], when `syl_count` is zero or above
    /// [`crate::format::MAX_SYL_COUNT`], or when `word_off + word_len` overflows or leaves the
    /// string pool. The first failure of a given record is recorded once and can be read
    /// through [`FstLexicon::malformed_entries`].
    ///
    /// # Panics
    ///
    /// In a debug build, when the range is inside the pool but its bytes are not UTF-8: that
    /// combination means the compiler and the container's checksum both failed, which is a bug
    /// to catch while testing rather than a file to tolerate at run time.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_dict::format::{DictEntry, SectionKind, pack_fst_value, writer};
    /// use ime_dict::fst_index::FstLexicon;
    ///
    /// // A container with one key, one record and one word: the smallest dictionary the
    /// // read path accepts.
    /// let mut index = fst::MapBuilder::memory();
    /// let packed = pack_fst_value(0, 1).expect("packing the word-list range");
    /// index.insert("ni".as_bytes(), packed).expect("inserting the key");
    /// let mut container = writer::DictWriter::new();
    /// for (kind, payload) in [
    ///     (SectionKind::Fst, index.into_inner().expect("finishing the index")),
    ///     (SectionKind::Entries, DictEntry::new(0, 3, 1, 0, 900).encode().to_vec()),
    ///     (SectionKind::StrPool, "\u{4f60}".as_bytes().to_vec()),
    ///     (SectionKind::Unigram, vec![0u8; 8]),
    ///     (SectionKind::WordList, 0u32.to_le_bytes().to_vec()),
    /// ] {
    ///     container.add_section(kind, payload).expect("adding a section");
    /// }
    /// let image = container.encode().expect("encoding the container");
    ///
    /// let dir = std::env::temp_dir().join(format!("rspinyin-lexicon-doc-{}", std::process::id()));
    /// std::fs::create_dir_all(&dir).expect("creating the scratch directory");
    /// let path = dir.join("base.dict");
    /// std::fs::write(&path, image).expect("writing the container");
    /// let lexicon = FstLexicon::load(&path).expect("loading the container");
    ///
    /// // A record inside the pool becomes the word it names.
    /// let record = DictEntry::new(0, 3, 1, 0, 900);
    /// let word = lexicon.entry_to_ref(&record).expect("the record is in contract");
    /// assert_eq!(word.text, "\u{4f60}");
    /// assert_eq!(word.weight, 900);
    ///
    /// // A record that points past the pool is reported, never read.
    /// let past = DictEntry::new(6, 3, 1, 0, 900);
    /// assert!(lexicon.entry_to_ref(&past).is_err());
    /// assert_eq!(lexicon.malformed_entries().len(), 1);
    ///
    /// std::fs::remove_dir_all(&dir).expect("removing the scratch directory");
    /// ```
    pub fn entry_to_ref(&self, entry: &DictEntry) -> Result<WordRef<'_>, DictError> {
        self.table.entry_to_ref(entry)
    }

    /// Returns the word records the read path has refused so far, oldest first.
    ///
    /// The snapshot is for the diagnostics layer, which turns one broken range into one log
    /// record instead of one per lookup; this crate emits no log records of its own.
    pub fn malformed_entries(&self) -> Vec<MalformedEntry> {
        self.table.malformed_entries()
    }

    /// Wraps a container failure as the cross-boundary error the trait reports.
    ///
    /// The dictionary path travels with the error because the contract's dictionary variants
    /// carry it, and the cause is kept rather than collapsed, so a bounds failure still says
    /// what was out of range.
    fn unavailable(&self, cause: DictError) -> ImeError {
        ImeError::DictUnavailable {
            path: self.path.clone(),
            cause,
        }
    }
}

impl Lexicon for FstLexicon {
    /// Reads the words of `key`, holding the head of the list inline.
    ///
    /// The read stops at [`MAX_WORDS_PER_KEY`], the container's own ceiling on one key's
    /// list, which the compiler enforces when it writes the file. The frozen signature has
    /// no `limit`, so a lookup cannot ask for less than that -- the lattice reads eight of
    /// what it gets -- and the ceiling is therefore the most this method can be narrowed
    /// to without a contract change.
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let mut buf = WordBuf::new();
        self.read_words_into(key, MAX_WORDS_PER_KEY as usize, &mut buf)
            .map_err(|cause| self.unavailable(cause))?;
        Ok(buf.into_word_iter())
    }

    /// Prefix enumeration is a later phase: the signature is frozen now so that
    /// callers do not have to change when abbreviation expansion lands, and the
    /// implementation is one streamed walk over the prefix range of the same FST.
    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    /// Reads the single-character words of one syllable.
    ///
    /// The compiler emits one key per word and a one-character word's key is its own
    /// reading, so the bare syllable is an ordinary key and the fallback needs no index
    /// of its own. The caller's `limit` is pushed all the way down, so a fallback
    /// materializes exactly what it returns and allocates nothing; the implementation this
    /// replaced read the whole list and then truncated it. An identifier outside the
    /// syllable table has no key to read; it is reported as [`ImeError::Unsupported`], the
    /// contract's code for a query this lexicon cannot serve.
    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let Some(key) = syllable_at(syl) else {
            return Err(ImeError::Unsupported);
        };
        let mut buf = WordBuf::new();
        self.read_words_into(key, limit, &mut buf)
            .map_err(|cause| self.unavailable(cause))?;
        Ok(buf.into_word_iter())
    }
}

/// Slices one section out of the mapped image.
///
/// The section table has already been verified by [`Reader::parse`], which checks every
/// offset, alignment and checksum; the bounds are re-checked here because the table is
/// untrusted input and this function reports a [`DictError`] rather than indexing out of
/// range. The slice borrows the image for `'static`, not for the reader's borrow, which is
/// what lets the views outlive the parse; an absent section yields an empty slice, exactly
/// as it does in the reader, and only the FST section is required.
fn section(
    image: &'static [u8],
    sections: &[SectionEntry],
    kind: SectionKind,
) -> Result<&'static [u8], DictError> {
    let index = kind.as_raw() as usize - 1;
    let Some(entry) = sections.get(index) else {
        return Err(out_of_range("section_kind", u64::from(kind.as_raw())));
    };
    if entry.is_absent() {
        return Ok(&[]);
    }
    let Ok(start) = usize::try_from(entry.offset) else {
        return Err(out_of_range("section_offset", entry.offset));
    };
    let Ok(len) = usize::try_from(entry.len) else {
        return Err(out_of_range("section_len", entry.len));
    };
    let Some(end) = start.checked_add(len) else {
        return Err(out_of_range("section_len", entry.len));
    };
    let Some(bytes) = image.get(start..end) else {
        return Err(out_of_range("section_offset", entry.offset));
    };
    Ok(bytes)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod word_buf_tests;
