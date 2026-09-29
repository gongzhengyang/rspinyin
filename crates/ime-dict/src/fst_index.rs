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
use ime_types::{DictError, ImeError, Lexicon, SyllableId, WordIter, WordRef};

use crate::format::{
    SectionEntry, SectionKind, UNIGRAM_ENTRY_SIZE, read_u16, read_u32, reader::{Reader, Verify},
};
use crate::mmap::MappedFile;

mod read;

/// Width of one `WORDLIST` element: a `word_id` is a little-endian `u32`.
const WORD_ID_SIZE: usize = 4;

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
/// perform, so records are decoded field by field instead.
///
/// Declaration order is drop order: the views are dropped before the mapping that backs
/// them, so no destructor can run against an unmapped range even if a view ever gains
/// one that reads its bytes.
#[derive(Debug)]
pub struct FstLexicon {
    fst: fst::Map<&'static [u8]>,
    entries: &'static [u8],
    strpool: &'static [u8],
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
        Ok(Self {
            fst,
            entries: section(image, sections, SectionKind::Entries)?,
            strpool: section(image, sections, SectionKind::StrPool)?,
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

    /// Runs one container query and reports its failure across the trait boundary.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::DictUnavailable`] when the container cannot serve the
    /// query.
    fn words_for(&self, key: &str) -> Result<Vec<WordRef<'_>>, ImeError> {
        let words = self.read_words(key);
        words.map_err(|cause| self.unavailable(cause))
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
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = self.words_for(key)?;
        Ok(WordIter::from_vec(words))
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
    /// of its own. An identifier outside the syllable table has no key to read; it is
    /// reported as [`ImeError::Unsupported`], the contract's code for a query this
    /// lexicon cannot serve.
    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let Some(key) = syllable_at(syl) else {
            return Err(ImeError::Unsupported);
        };
        let mut words = self.words_for(key)?;
        words.truncate(limit);
        Ok(WordIter::from_vec(words))
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
