//! Read-only mapping of a dictionary file.
//!
//! Responsibility: turn a path into a read-only mapping of exactly the length the
//! file had when it was opened, tell the kernel that the pages will be read at
//! random, and hand out views of those bytes that stay valid for as long as the
//! mapping does.
//!
//! Boundaries: this module owns the mapping and nothing else. It never parses a
//! header, never checks a checksum and never interprets a byte it maps. It is also
//! the crate's only `unsafe` module: the architecture rule allows `unsafe` in
//! `src/mmap.rs` and nowhere else in `ime-dict`, and `scripts/check-unsafe.sh`
//! enforces that with a file-precise allowlist.
//!
//! # Why the mapping is a type of its own
//!
//! Zero-copy dictionary access means every word the decoder sees is a `&str`
//! pointing into the mapping, so the value that owns the mapping must also own the
//! borrows taken from it. That is a self-referential value, which Rust cannot
//! express in safe code; the single conversion that makes it expressible lives in
//! [`MappedFile::static_bytes`], with its invariant written out next to it. Keeping
//! the mapping opaque -- no clone, no mutable borrow, no remapping -- is what lets
//! that conversion be justified once, here, instead of at every call site.
//!
//! # The string-pool view
//!
//! `WordPool` is the second view this module hands out: the `STRPOOL` section read as
//! text. The conversion it performs is unchecked, and the architecture rule confines
//! `unsafe` to this file, so the obligation travels in a value rather than in a
//! documented promise on a safe function -- the pool is built once, by the loader, out
//! of a container it has already accepted, and every later conversion is sound because
//! of how that value came to exist.
//!
//! # SIGBUS
//!
//! A mapping whose file is truncated by another process faults on the pages past
//! the new end of file, and that fault is `SIGBUS`, not a panic Rust could catch.
//! The defence that belongs in this module is to read the length from the open
//! handle once, map exactly that many bytes and never consult the path again: every
//! offset the container's section table can name then lies inside the mapping by
//! construction. Turning the remaining race into a diagnostic and a clean exit is
//! the diagnostics layer's business, because a signal handler is process-wide state
//! and this crate is a library inside someone else's process.

use std::fs::File;
use std::path::Path;

use ime_types::DictError;
use memmap2::{Advice, Mmap, MmapOptions};

/// A read-only mapping of one file.
///
/// The mapping is the backing store for every zero-copy view the dictionary layer
/// hands out, so the type is deliberately opaque: it exposes the bytes it owns and
/// nothing else, it is never cloned, never borrowed mutably and never remapped.
/// Those properties are what make [`MappedFile::static_bytes`] sound.
#[derive(Debug)]
pub struct MappedFile {
    mmap: Mmap,
}

impl MappedFile {
    /// Maps `path` read-only.
    ///
    /// The length is taken from the open handle and the mapping is built with
    /// exactly that length, so the mapping covers the whole file as it was when it
    /// was opened. A later truncation cannot make one of the container's offsets
    /// land outside the mapping silently: it faults, which is the case the
    /// diagnostics layer handles.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Io`] when the file cannot be opened, measured, mapped or
    /// advised, and [`DictError::LengthOutOfRange`] when the file is empty or larger
    /// than this process can address.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_dict::mmap::MappedFile;
    ///
    /// let path = std::env::temp_dir().join(format!("rspinyin-mmap-doc-{}", std::process::id()));
    /// std::fs::write(&path, b"RSPD").expect("writing the fixture");
    /// let map = MappedFile::open(&path).expect("mapping the fixture");
    /// assert_eq!(map.bytes(), b"RSPD");
    /// std::fs::remove_file(&path).expect("removing the fixture");
    /// ```
    // `MmapOptions::map` is an `unsafe fn`; the SAFETY note below covers it, and
    // `unsafe` is allowed in this file by the architecture rule.
    #[allow(unsafe_code)]
    pub fn open(path: &Path) -> Result<Self, DictError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            return Err(DictError::LengthOutOfRange {
                field: "file_len",
                value: 0,
            });
        }
        let len = usize::try_from(len).map_err(|_| DictError::LengthOutOfRange {
            field: "file_len",
            value: len,
        })?;
        // SAFETY: the mapping is created read-only, so nothing can be written
        // through it, and memmap2's requirement that the file not be modified
        // through another handle while it is mapped is upheld by the dictionary
        // compiler, which never writes a dictionary in place: it writes a sibling
        // temporary file and renames it over the destination.
        let mmap = unsafe { MmapOptions::new().len(len).map(&file) }?;
        if mmap.len() != len {
            return Err(DictError::LengthOutOfRange {
                field: "file_len",
                value: mmap.len() as u64,
            });
        }
        // Candidate-window lookups touch a few dozen bytes per keystroke out of a
        // dictionary of tens of megabytes, so kernel read-ahead would fetch far more
        // than is ever read. The advice is a hint the kernel may refuse; refusing it
        // is an environment fault, not a data fault, so it is reported rather than
        // swallowed.
        mmap.advise(Advice::Random)?;
        Ok(Self { mmap })
    }

    /// Returns the mapped bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.mmap
    }

    /// Returns the mapped bytes as a slice that stays valid for as long as the
    /// mapping does.
    ///
    /// # Soundness
    ///
    /// A caller must not use the returned slice after `self` is dropped: the borrow
    /// widened here is the one that ties the bytes to the mapping, and the compiler
    /// cannot check a lifetime that has been erased. Widening it is what lets the
    /// dictionary loader build a value that owns the mapping *and* the `fst::Map`,
    /// entry-table, string-pool and word-list views taken from it, which is the shape
    /// the zero-copy read path needs.
    ///
    /// The obligation is discharged by construction rather than by a caller's
    /// promise. `MappedFile` has no `Clone`, no `&mut` accessor and no remapping
    /// method, so a mapping that exists is neither replaced nor unmapped while a
    /// borrow of it is alive; and its one caller stores the mapping and every view in
    /// a single value whose fields are private, and orders that value so the views
    /// are dropped before the mapping.
    // `from_raw_parts` is the lifetime widening itself; the SAFETY note below
    // justifies it and `unsafe` is allowed in this file.
    #[allow(unsafe_code)]
    pub fn static_bytes(&self) -> &'static [u8] {
        // SAFETY: `as_ptr` and `len` describe the mapping `self` owns, and `Mmap` never
        // moves the pages it mapped, so the address stays valid and readable for as
        // long as the mapping is alive. `MappedFile` owns that mapping, is opaque,
        // cannot be cloned, cannot be borrowed mutably and cannot be remapped, which
        // is what keeps the returned slice valid for as long as the caller reaches it.
        unsafe { std::slice::from_raw_parts(self.mmap.as_ptr(), self.mmap.len()) }
    }
}

/// Reinterprets `bytes` as text without re-validating them.
///
/// # Safety
///
/// `bytes` must be valid UTF-8. The read path discharges that obligation by construction
/// rather than by a promise at the call site: the compiler builds the string pool out of Rust
/// `String`s, so its bytes are text, the loader verifies the section checksum before a word is
/// read, and the [`WordPool`] this function is reached through can only have been built out of
/// the pool of a container the loader accepted. `crate::entry` records the argument in full.
// `str::from_utf8_unchecked` is the operation this function exists for, and `unsafe` is
// allowed in this file by the architecture rule.
#[allow(unsafe_code)]
pub(crate) unsafe fn str_unchecked(bytes: &[u8]) -> &str {
    // The tripwire. A release build skips the scan, because paying it per word would put
    // milliseconds of work on a candidate sweep; a debug build catches the one case the
    // compiler and the checksum were both supposed to make impossible.
    debug_assert!(
        std::str::from_utf8(bytes).is_ok(),
        "a word of the string pool is not UTF-8"
    );
    // SAFETY: the caller guarantees the bytes are valid UTF-8, which is the only
    // requirement `from_utf8_unchecked` has. The returned reference borrows the same
    // bytes for as long as the input does, so it stays inside whatever memory the caller
    // borrowed them from.
    unsafe { std::str::from_utf8_unchecked(bytes) }
}

/// The string pool of a container the loader has accepted.
///
/// A value of this type *is* the proof that the bytes it borrows are the `STRPOOL` section of
/// a container the reader accepted: the pool a compiler wrote from Rust `String`s, in a file
/// whose checksums matched on the startup path. The proof cannot be manufactured out of
/// arbitrary bytes: [`WordPool::verified`] is crate-visible, and the lexicon's loader is its
/// only caller. Carrying the proof in a value is what lets [`WordPool::word`] hand back a
/// `&str` without scanning the bytes again -- the obligation [`str_unchecked`] states is
/// discharged once, where the value is built, instead of being restated as a precondition on
/// a safe function that no caller can be held to.
///
/// # Concurrency
///
/// `Send + Sync`: the bytes are immutable and shared, and the type holds nothing else.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WordPool<'a> {
    bytes: &'a [u8],
}

impl<'a> WordPool<'a> {
    /// Wraps the string pool of a container that has been accepted.
    ///
    /// The caller passes the `STRPOOL` section of a container the reader accepted, whose
    /// bytes the compiler wrote from Rust `String`s. That is why this constructor is
    /// crate-visible and why `crate::fst_index` is its only caller: a public constructor
    /// would have to either scan the pool on every call or trust whatever it was handed, and
    /// neither is something a safe API may do on behalf of a caller it cannot see.
    pub(crate) fn verified(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Returns the text stored at `start..end`, or `None` when that range leaves the pool.
    ///
    /// The range is checked against the pool here rather than by the caller, so no caller can
    /// name a range that is not part of the pool; what is left for the caller is the
    /// per-record contract the entry table checks before it asks.
    ///
    /// # Panics
    ///
    /// In a debug build, when the range is inside the pool but its bytes are not UTF-8: that
    /// combination means the compiler and the checksum both failed, which is a bug to catch
    /// while testing rather than a file to tolerate at run time. A release build does not
    /// check it; `crate::entry` states the trade-off.
    // The unchecked conversion below is the operation this type exists to make safe, and
    // `unsafe` is allowed in this file by the architecture rule.
    #[allow(unsafe_code)]
    pub(crate) fn word(&self, start: usize, end: usize) -> Option<&'a str> {
        let bytes = self.bytes.get(start..end)?;
        // SAFETY: the value's invariant is that `bytes` is the string pool of a container the
        // loader accepted, written by the compiler from Rust strings, so every range of it is
        // text; `get` keeps the slice inside the pool, so the returned reference borrows
        // memory the caller still reaches through `self`.
        Some(unsafe { str_unchecked(bytes) })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// Serial number for the scratch files; tests run in parallel threads.
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Returns a path in the temporary directory that no other test uses.
    fn unique_path(tag: &str) -> PathBuf {
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rspinyin-mmap-{}-{tag}-{serial}.bin",
            std::process::id()
        ))
    }

    /// Writes `bytes` to a unique temporary file and returns its path.
    fn scratch_file(tag: &str, bytes: &[u8]) -> PathBuf {
        let path = unique_path(tag);
        fs::write(&path, bytes).expect("writing the scratch file");
        path
    }

    #[test]
    fn test_open_maps_the_whole_file_and_exposes_it_read_only() {
        let bytes = b"RSPD\x01\x00\x00\x00payload";
        let path = scratch_file("whole", bytes);
        let map = MappedFile::open(&path).expect("mapping the file");
        assert_eq!(map.bytes(), bytes);
        assert_eq!(map.static_bytes(), bytes);
        // The widened view is the same pages, not a copy of them: that is the whole
        // point of mapping the dictionary instead of reading it.
        assert_eq!(map.bytes().as_ptr(), map.static_bytes().as_ptr());
        fs::remove_file(&path).expect("cleaning up");
    }

    #[test]
    fn test_open_rejects_an_empty_file() {
        let path = scratch_file("empty", b"");
        assert!(matches!(
            MappedFile::open(&path),
            Err(DictError::LengthOutOfRange {
                field: "file_len",
                ..
            })
        ));
        fs::remove_file(&path).expect("cleaning up");
    }

    #[test]
    fn test_open_reports_a_missing_file_as_an_io_error() {
        let path = unique_path("missing");
        assert!(matches!(MappedFile::open(&path), Err(DictError::Io(_))));
    }

    #[test]
    fn test_open_maps_a_file_of_a_single_byte() {
        let path = scratch_file("tiny", b"x");
        let map = MappedFile::open(&path).expect("mapping the file");
        assert_eq!(map.bytes().len(), 1);
        assert_eq!(map.static_bytes(), b"x");
        fs::remove_file(&path).expect("cleaning up");
    }

    #[test]
    fn test_mapping_keeps_the_bytes_of_a_deleted_file_readable() {
        let bytes = b"RSPD mapped and then deleted";
        let path = scratch_file("deleted", bytes);
        let map = MappedFile::open(&path).expect("mapping the file");
        fs::remove_file(&path).expect("deleting the mapped file");
        // The unlink drops the directory entry, not the inode this mapping holds.
        assert_eq!(map.bytes(), bytes);
        assert_eq!(map.static_bytes(), bytes);
    }

    /// The shape the dictionary loader needs: a value that owns the mapping and
    /// holds views derived from it, with the views declared first so that they are
    /// dropped before the mapping they borrow.
    struct Owner {
        view: &'static [u8],
        map: MappedFile,
    }

    #[test]
    fn test_static_bytes_survive_moving_the_owner_of_the_mapping() {
        let path = scratch_file("moved", b"RSPD move me");
        let map = MappedFile::open(&path).expect("mapping the file");
        let owner = Owner {
            view: map.static_bytes(),
            map,
        };
        // Moving the value moves the mapping's metadata, never the mapped pages, so
        // the view stays valid.
        let moved = owner;
        assert_eq!(moved.view, b"RSPD move me");
        assert_eq!(moved.map.bytes(), moved.view);
        fs::remove_file(&path).expect("cleaning up");
    }

    #[test]
    fn test_word_pool_serves_the_text_of_a_range_it_holds() {
        const POOL: &[u8] = "你好世界".as_bytes();
        let pool = WordPool::verified(POOL);
        let first = pool.word(0, 3).expect("the first word is in the pool");
        assert_eq!(first, "你");
        assert_eq!(
            pool.word(9, 12),
            Some("界"),
            "the last word ends at the pool end"
        );
        // The text is a window into the pool rather than a copy of it, which is the
        // property the whole read path is built on.
        assert!(POOL.as_ptr_range().contains(&first.as_ptr()));
    }

    #[test]
    fn test_word_pool_reports_a_range_that_leaves_the_pool() {
        let pool = WordPool::verified("你好".as_bytes());
        assert_eq!(pool.word(0, 7), None, "the range runs past the pool");
        assert_eq!(pool.word(6, 9), None, "the range starts past the pool");
        assert_eq!(pool.word(9, 3), None, "an inverted range is not a range");
        assert_eq!(
            pool.word(usize::MAX, usize::MAX),
            None,
            "an offset at the top of the width does not wrap"
        );
    }
}
