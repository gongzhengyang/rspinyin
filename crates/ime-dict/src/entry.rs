//! Zero-copy access to the words that entry records point at.
//!
//! Responsibility: turn one decoded [`DictEntry`] into a [`WordRef`] whose text borrows
//! the mapped `STRPOOL` bytes, and check the record on every access, so that a tampered
//! `word_off` or `word_len` is reported as a [`DictError`] instead of being dereferenced.
//! Nothing on this path allocates: the text is a `&str` into the mapping, which is what
//! keeps candidate generation free of a copy per word.
//!
//! Boundaries: this module decodes no record and reads no index. Locating an entry -- the
//! FST lookup, the word-list indirection, the `ENTRIES` offset arithmetic -- belongs to
//! the lexicon; this module starts from a record that is already in hand. It also
//! verifies no checksum: accepting a file is the loader's job, and what is left to check
//! *per access* is everything a record read out of an accepted file can still get wrong.
//!
//! # What is checked on every access
//!
//! 1. `word_len` is in `1..=MAX_WORD_LEN`. A zero length names no word at all, and a
//!    longer one is past the 32-character ceiling the format promises.
//! 2. `syl_count` is in `1..=MAX_SYL_COUNT`, the range the decoder indexes its DAG with.
//! 3. `word_off + word_len` neither overflows nor leaves `STRPOOL`. The sum is formed in
//!    the offset's own width first, so a record near `u32::MAX` is reported as an overflow
//!    instead of wrapping into a range that happens to sit inside the pool.
//! 4. The bytes are valid UTF-8, which is established when the dictionary is compiled and
//!    trusted when it is read -- see below.
//!
//! Undefined `flags` bits are deliberately not rejected: a pool written by a newer
//! compiler stays readable, and the bits this version knows survive the conversion.
//!
//! # UTF-8: validated at build time, trusted at read time
//!
//! Scanning each word again here would put a linear check on every candidate the decoder
//! produces -- tens of nanoseconds per word, milliseconds over a full candidate sweep --
//! so the bytes are taken as text without a second look. Two things make that sound: the
//! compiler appends Rust `String`s to the pool, so it cannot write bytes that are not
//! text, and the loader verifies the file's checksums before a word is read (its
//! header-only mode exists for tooling and is not the startup path). Reaching the
//! conversion with bytes that are not text therefore requires a pool the compiler could
//! not have produced *and* a checksum that matches.
//!
//! The conversion itself is `crate::mmap::WordPool::word`, which is safe because the pool it
//! slices can only have been built by the loader out of a container it accepted. That is what
//! keeps this module inside safe Rust without leaving a precondition for a caller to honour:
//! the obligation is discharged where the pool comes into existence, not restated at every
//! conversion.
//!
//! # Threat model
//!
//! The defence is aimed at accidental damage: a bad sector, a truncated file, a compiler
//! bug. A local attacker who can rewrite `base.dict` and recompute its checksums is
//! deliberately outside the threat model -- such an attacker already has the user's
//! privileges, and nothing this process checks could take them back. The bounds checks
//! above are what keeps even a file like that from being read out of bounds.

use std::sync::{Mutex, MutexGuard};

use ime_types::{DictError, WordFlags, WordRef};

use crate::format::{DictEntry, MAX_SYL_COUNT, MAX_WORD_LEN};
use crate::mmap::WordPool;

/// How many malformed entries the once-only record keeps.
///
/// The record exists so that a damaged dictionary produces one diagnostic per broken
/// range instead of one per lookup. It is fed by untrusted input, so a file whose entry
/// table is damaged wholesale must not be able to grow it without bound; the cap is far
/// larger than what it takes to explain a failure.
const MAX_REPORTED_ENTRIES: usize = 64;

/// One entry the table refused to dereference.
///
/// A record carries no identifier of its own, so it is named by the fields that failed:
/// the same broken entry reached again yields the same pair, and is reported once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MalformedEntry {
    /// The byte offset the record named inside the string pool.
    pub word_off: u32,
    /// The byte length the record named.
    pub word_len: u16,
}

/// The read side of the entry table: the string pool its records point into, plus the
/// once-only record of the entries that failed their bounds check.
///
/// The pool is held as a [`WordPool`], the value that carries the proof it came out of a
/// container the loader accepted; that is what lets a conversion hand back a `&str` without
/// scanning it, and what keeps the unchecked part of the read path in the one module the
/// architecture rule allows it in.
///
/// # Concurrency
///
/// `Send + Sync`. The pool is immutable and shared, and the record is only touched when a
/// record fails its check, so a successful conversion takes no lock and the decode hot
/// path stays free of contention.
#[derive(Debug)]
pub(crate) struct EntryTable<'pool> {
    pool: WordPool<'pool>,
    reported: Mutex<Vec<MalformedEntry>>,
}

impl<'pool> EntryTable<'pool> {
    /// Builds a table over the string pool of a container that has already been accepted.
    ///
    /// The pool is borrowed for as long as the table lives, so the text every conversion
    /// returns stays valid for as long as the mapping behind it does.
    pub(crate) fn new(pool: WordPool<'pool>) -> Self {
        Self {
            pool,
            reported: Mutex::new(Vec::new()),
        }
    }

    /// Returns the word `entry` names, borrowed from the pool.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] naming the offending field when `word_len`
    /// is zero or above [`MAX_WORD_LEN`], when `syl_count` is zero or above
    /// [`MAX_SYL_COUNT`], or when `word_off + word_len` overflows or leaves the pool. The
    /// first failure of a given record is recorded in [`EntryTable::malformed_entries`];
    /// repeats of it are not recorded again.
    ///
    /// # Panics
    ///
    /// In a debug build, when the range is inside the pool but its bytes are not UTF-8:
    /// that combination means the compiler and the checksum both failed, which is a bug
    /// to catch while testing rather than a file to tolerate at run time. A release build
    /// does not check it; the module documentation states the trade-off.
    // The budget for this conversion is tens of nanoseconds, and it is called from another
    // module of the crate, so the body has to stay inlinable across codegen units.
    #[inline]
    pub(crate) fn entry_to_ref(&self, entry: &DictEntry) -> Result<WordRef<'_>, DictError> {
        Ok(WordRef {
            text: self.word_text(entry)?,
            weight: entry.weight,
            syl_count: entry.syl_count,
            flags: WordFlags::from_bits_truncate(entry.flags),
        })
    }

    /// Returns the malformed entries recorded so far, oldest first.
    ///
    /// The snapshot is for the diagnostics layer, which is where these become log records:
    /// this crate emits none of its own, and the read path must not wait on a formatter.
    pub(crate) fn malformed_entries(&self) -> Vec<MalformedEntry> {
        let reported = self.lock();
        reported.clone()
    }

    /// Checks `entry` and returns the text it names inside the pool.
    ///
    /// The checks run cheapest first, and the range is formed in the offset's own width
    /// before it is widened, so an offset near the top of the field is caught as an
    /// overflow instead of wrapping into a range that happens to sit inside the pool.
    fn word_text(&self, entry: &DictEntry) -> Result<&'pool str, DictError> {
        if entry.word_len == 0 || entry.word_len > MAX_WORD_LEN {
            return Err(self.report(entry, "word_len", u64::from(entry.word_len)));
        }
        if entry.syl_count == 0 || entry.syl_count > MAX_SYL_COUNT {
            return Err(self.report(entry, "syl_count", u64::from(entry.syl_count)));
        }
        let offset = u64::from(entry.word_off);
        let Some(end) = entry.word_off.checked_add(u32::from(entry.word_len)) else {
            return Err(self.report(entry, "word_off", offset));
        };
        let (Ok(start), Ok(end)) = (usize::try_from(entry.word_off), usize::try_from(end)) else {
            return Err(self.report(entry, "word_off", offset));
        };
        let Some(text) = self.pool.word(start, end) else {
            return Err(self.report(entry, "word_off", offset));
        };
        Ok(text)
    }

    /// Records `entry` as malformed unless it is already recorded, and returns the error
    /// that describes the failure.
    fn report(&self, entry: &DictEntry, field: &'static str, value: u64) -> DictError {
        let malformed = MalformedEntry {
            word_off: entry.word_off,
            word_len: entry.word_len,
        };
        let mut reported = self.lock();
        if reported.len() < MAX_REPORTED_ENTRIES && !reported.contains(&malformed) {
            reported.push(malformed);
        }
        DictError::LengthOutOfRange { field, value }
    }

    /// Borrows the record, recovering from a poisoned lock.
    ///
    /// The critical section only reads or pushes onto a vector, so it cannot panic and
    /// this module cannot poison the lock; recovering keeps a panic elsewhere in the
    /// process from turning a diagnostics-only structure into a second failure.
    fn lock(&self) -> MutexGuard<'_, Vec<MalformedEntry>> {
        self.reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;
    use crate::format::FLAG_SURNAME;

    /// A pool of four three-byte words, so that every offset the tests name is a UTF-8
    /// boundary.
    const POOL: &[u8] = "你好世界".as_bytes();

    /// A record that names the first word of [`POOL`], with every field in contract.
    fn first_word() -> DictEntry {
        DictEntry::new(0, 3, 1, 0, 900)
    }

    /// Returns the field name of a bounds failure, or `None` for any other error.
    fn bounds_field(err: &DictError) -> Option<&'static str> {
        match err {
            DictError::LengthOutOfRange { field, .. } => Some(*field),
            _ => None,
        }
    }

    #[test]
    fn test_entry_to_ref_borrows_the_word_from_the_pool() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let word = table
            .entry_to_ref(&first_word())
            .expect("the record is valid");
        assert_eq!(word.text, "你");
        assert_eq!(word.weight, 900);
        assert_eq!(word.syl_count, 1);
        assert!(word.flags.is_empty());
        // The text is a window into the mapping rather than a copy of it: its bytes lie
        // inside the pool the table was built over.
        assert!(POOL.as_ptr_range().contains(&word.text.as_ptr()));
        // A conversion that succeeded is not a diagnostic.
        assert!(table.malformed_entries().is_empty());
    }

    #[test]
    fn test_entry_to_ref_reads_the_last_word_of_the_pool() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let last = DictEntry::new(9, 3, 1, 0, 700);
        let word = table
            .entry_to_ref(&last)
            .expect("the range ends at the pool end");
        assert_eq!(word.text, "界");
        assert_eq!(word.text.len(), 3, "the length is in bytes, not characters");
    }

    #[test]
    fn test_entry_to_ref_reports_a_record_that_leaves_the_pool() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let cases = [
            // The offset starts exactly at the end of the pool, so no byte is available.
            DictEntry::new(12, 3, 1, 0, 1),
            // The range overflows the offset field before it is widened.
            DictEntry::new(u32::MAX, MAX_WORD_LEN, 1, 0, 1),
            // The offset is inside the pool but the range runs past its end.
            DictEntry::new(9, 6, 1, 0, 1),
            // The offset is a single byte past the pool.
            DictEntry::new(13, 1, 1, 0, 1),
        ];
        for entry in cases {
            let served = table.entry_to_ref(&entry);
            let Err(err) = served else {
                panic!("{entry:?} names a range outside the pool");
            };
            assert_eq!(bounds_field(&err), Some("word_off"), "{entry:?}");
        }
        assert_eq!(
            table.malformed_entries().len(),
            cases.len(),
            "every distinct broken range is recorded"
        );
    }

    #[test]
    fn test_entry_to_ref_reports_a_record_of_an_empty_pool() {
        let table = EntryTable::new(WordPool::verified(b""));
        let served = table.entry_to_ref(&first_word());
        let Err(err) = served else {
            panic!("an empty pool holds no word");
        };
        assert_eq!(bounds_field(&err), Some("word_off"), "{err}");
    }

    #[test]
    fn test_entry_to_ref_reports_a_word_length_outside_the_contract() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let cases = [
            // A zero length names no word at all.
            DictEntry::new(0, 0, 1, 0, 1),
            // One byte past the 32-character ceiling the format promises.
            DictEntry::new(0, MAX_WORD_LEN + 1, 1, 0, 1),
        ];
        for entry in cases {
            let served = table.entry_to_ref(&entry);
            let Err(err) = served else {
                panic!("{entry:?} has a length outside the contract");
            };
            assert_eq!(bounds_field(&err), Some("word_len"), "{entry:?}");
        }
        assert_eq!(table.malformed_entries().len(), cases.len());
    }

    #[test]
    fn test_entry_to_ref_reports_a_syllable_count_outside_the_contract() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let cases = [
            DictEntry::new(0, 3, 0, 0, 1),
            DictEntry::new(0, 3, MAX_SYL_COUNT + 1, 0, 1),
        ];
        for entry in cases {
            let served = table.entry_to_ref(&entry);
            let Err(err) = served else {
                panic!("{entry:?} has a syllable count outside the contract");
            };
            assert_eq!(bounds_field(&err), Some("syl_count"), "{entry:?}");
        }
    }

    #[test]
    fn test_entry_to_ref_ignores_flag_bits_it_does_not_know() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let known = DictEntry::new(0, 3, 1, FLAG_SURNAME, 900);
        let word = table.entry_to_ref(&known).expect("a surname is a word");
        assert!(word.flags.contains(WordFlags::SURNAME));

        // A bit a later format version may define must not make the word unreadable.
        let future = DictEntry::new(0, 3, 1, 0b1111_0000, 900);
        let word = table
            .entry_to_ref(&future)
            .expect("unknown bits are ignored");
        assert!(word.flags.is_empty());
        assert_eq!(word.text, "你");
    }

    #[test]
    fn test_entry_to_ref_records_a_malformed_entry_once() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let broken = DictEntry::new(12, 3, 1, 0, 900);
        for _ in 0..100 {
            assert!(
                table.entry_to_ref(&broken).is_err(),
                "the record stays broken on every access"
            );
        }
        let recorded = table.malformed_entries();
        assert_eq!(recorded.len(), 1, "one broken range, one diagnostic");
        assert_eq!(
            recorded[0],
            MalformedEntry {
                word_off: 12,
                word_len: 3
            }
        );
    }

    #[test]
    fn test_entry_to_ref_records_distinct_malformed_entries_separately() {
        let table = EntryTable::new(WordPool::verified(POOL));
        let past_the_end = DictEntry::new(12, 3, 1, 0, 1);
        let empty_word = DictEntry::new(0, 0, 1, 0, 1);
        assert!(table.entry_to_ref(&past_the_end).is_err());
        assert!(table.entry_to_ref(&empty_word).is_err());
        assert!(table.entry_to_ref(&past_the_end).is_err());

        let recorded = table.malformed_entries();
        assert_eq!(recorded.len(), 2);
        assert_eq!(
            recorded[0],
            MalformedEntry {
                word_off: 12,
                word_len: 3
            }
        );
        assert_eq!(
            recorded[1],
            MalformedEntry {
                word_off: 0,
                word_len: 0
            }
        );
    }

    #[test]
    fn test_entry_to_ref_stops_recording_at_the_cap() {
        // The record is fed by untrusted input, so a file whose entries are broken
        // wholesale must not be able to grow it without bound.
        let table = EntryTable::new(WordPool::verified(POOL));
        let base = u32::try_from(POOL.len()).expect("the fixture is small");
        let beyond = u32::try_from(MAX_REPORTED_ENTRIES).expect("the cap is small");
        for index in 0..beyond + 8 {
            let broken = DictEntry::new(base + index, 1, 1, 0, 1);
            assert!(table.entry_to_ref(&broken).is_err(), "{broken:?}");
        }
        assert_eq!(table.malformed_entries().len(), MAX_REPORTED_ENTRIES);
    }

    #[test]
    fn test_entry_to_ref_records_once_when_two_threads_report_the_same_entry() {
        // The lexicon is shared across threads, so the table has to be `Send + Sync`; the
        // shared handle below is what checks that, and the record is what the lock in the
        // table protects.
        let table = Arc::new(EntryTable::new(WordPool::verified(POOL)));
        let broken = DictEntry::new(12, 3, 1, 0, 900);
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let table = Arc::clone(&table);
                thread::spawn(move || {
                    let mut failures = 0usize;
                    for _ in 0..50 {
                        if table.entry_to_ref(&broken).is_err() {
                            failures += 1;
                        }
                    }
                    failures
                })
            })
            .collect();

        let mut failures = 0usize;
        for worker in workers {
            failures += worker.join().expect("the worker finished");
        }
        assert_eq!(failures, 100, "every access reports the failure");
        assert_eq!(table.malformed_entries().len(), 1);
    }

    #[test]
    fn test_entry_to_ref_walks_every_record_of_a_large_pool() {
        let (pool, entries, words) = synthetic_table(5_000);
        let table = EntryTable::new(WordPool::verified(&pool));
        assert_eq!(entries.len(), words.len());
        for (index, entry) in entries.iter().enumerate() {
            let word = table
                .entry_to_ref(entry)
                .expect("the fixture is in contract");
            assert_eq!(word.text, words[index].as_str(), "record {index}");
            assert_eq!(word.syl_count, entry.syl_count, "record {index}");
            assert_eq!(word.weight, entry.weight, "record {index}");
            assert_eq!(word.flags.bits(), entry.flags, "record {index}");
        }
        assert!(
            table.malformed_entries().is_empty(),
            "a pool in contract produces no diagnostic"
        );
    }

    /// Builds a string pool and the records that name every word in it.
    ///
    /// The words mix ASCII and Han text, so the walk crosses multi-byte boundaries, and
    /// every field the conversion carries varies with the index, so a record read for the
    /// wrong word cannot pass unnoticed.
    fn synthetic_table(count: usize) -> (Vec<u8>, Vec<DictEntry>, Vec<String>) {
        let mut pool = Vec::new();
        let mut entries = Vec::with_capacity(count);
        let mut words = Vec::with_capacity(count);
        for index in 0..count {
            let text = if index % 5 == 0 {
                format!("汉{index}")
            } else {
                format!("w{index}")
            };
            let offset = u32::try_from(pool.len()).expect("the fixture fits in a u32");
            let len = u16::try_from(text.len()).expect("the fixture fits in a u16");
            pool.extend_from_slice(text.as_bytes());
            let syllables = u8::try_from(index % 16 + 1).expect("the range is 1..=16");
            let flags = u8::try_from(index % 4).expect("the fixture uses defined bits only");
            let weight = u32::try_from(index).expect("the fixture fits in a u32");
            entries.push(DictEntry::new(offset, len, syllables, flags, weight));
            words.push(text);
        }
        (pool, entries, words)
    }
}
