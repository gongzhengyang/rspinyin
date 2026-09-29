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
/// `bytes` must be valid UTF-8. The dictionary read path discharges that obligation by
/// construction rather than by a promise: the compiler builds the string pool out of Rust
/// `String`s, so its bytes are text, and the loader verifies the section checksum before a
/// word is read. [`pool_text`] is the wrapper the read path calls; `crate::entry` records
/// the argument in full.
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

/// Interprets a word's bytes from a verified string pool as text.
///
/// This is the read path's entry point. The bytes are not scanned again here: a linear
/// UTF-8 check per word would sit on every candidate the decoder produces, while the pool
/// was built from Rust `String`s when the dictionary was compiled and checksummed when it
/// was loaded.
///
/// # Preconditions
///
/// `bytes` must be a range of the `STRPOOL` section of a container that passed its
/// checksum. That is a property of the container the caller already holds rather than a
/// promise about this call, which is why this wrapper is safe: its only caller is the
/// entry table, which slices the pool of a loaded dictionary. A debug build checks the
/// property anyway.
///
/// # Panics
///
/// In a debug build, when `bytes` are not valid UTF-8.
// `from_utf8_unchecked` is called below; `unsafe` is allowed in this file by the
// architecture rule, and the wrapper is what keeps the token out of the callers.
#[allow(unsafe_code)]
pub(crate) fn pool_text(bytes: &[u8]) -> &str {
    // SAFETY: the caller passes bytes sliced out of the string pool of a container whose
    // checksum the loader verified and whose bytes the compiler wrote from Rust strings,
    // which is the precondition [`str_unchecked`] states; `crate::entry` records the
    // threat model that argument rests on.
    unsafe { str_unchecked(bytes) }
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
}
