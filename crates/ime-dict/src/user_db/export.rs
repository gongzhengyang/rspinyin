//! The TSV interchange format: exporting the user's words, and reading them back.
//!
//! Responsibility: own the one document format the store speaks to the outside world --
//! its header, its columns, its size ceiling, the atomic file write that produces it and
//! the parser that accepts it -- plus the one write transaction an import needs.
//!
//! Boundaries: this module owns no policy about *which* words exist; it reads what the
//! store holds and writes what a document says, and every value it produces is validated
//! before it is trusted, because a document a user hands back is untrusted input like any
//! other (`ASM-A-13`, `ASM-A-09`). The store's own ceiling (`ASM-A-09`, 8 MiB) is enforced
//! on both directions, so an export never leaves a half-written file behind and an import
//! never reads an unbounded document into memory.
//!
//! # The document
//!
//! ```text
//! # rspinyin user dictionary export v2
//! # Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned <TAB> dee_q16 <TAB> tick
//! # `key` is the word the user committed, which is what the frequency was recorded against.
//! nihao<TAB>128<TAB>1759161600000<TAB>1756570000000<TAB>0<TAB>8388608<TAB>412
//! # Coined words: key <TAB> word <TAB> commits <TAB> last_used_unix
//! zhu'ji<TAB>诸暨<TAB>3<TAB>0
//! ```
//!
//! Rows come out in key order, which is the order the store's table holds them in, so two
//! exports of an unchanged store are byte-identical and can be diffed. A line starting with
//! `#` is a comment; a document whose first non-blank line is not the marker above is not an
//! export of this format and is refused whole.
//!
//! The frequency rows are followed by the coined-word section whenever the store holds
//! any: one comment naming the four columns of the rows that follow it, written by the
//! commit path that enrolled the pairs the user spelled. The section travels in both
//! directions -- an import writes the rows back under the same merge rule the frequency
//! rows obey, never lowering a count the user already earned. A build too old to know the
//! section reads its rows as malformed frequency rows and refuses the document whole,
//! which is the same forward break every column extension of the marker made.
//!
//! The `dee_q16` column is the decay accumulator in Q16.16 and `tick` the record's
//! logical stamp -- the two inputs the store's decay reads. A v2 import accepts the v1
//! document as well: its five columns map onto the first five of these, and its rows
//! enter the store un-ticked and un-decayed, which is what a v1 record is.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use super::flush::Delta;
use super::hydrate::COMPOSITE_SEPARATOR;
use super::*;

mod document;

use self::document::{CoinedExport, ExportDocument, Parsed, parse_export};

/// Ceiling on one export document, in bytes (`ASM-A-09`).
///
/// The same number bounds an import: a document larger than this is not one of ours, and
/// reading it whole would put a file of unknown size into the plugin's resident set.
pub const EXPORT_LIMIT_BYTES: u64 = 8 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// Test-only: the export ceiling in force for the current thread.
    ///
    /// The real ceiling is eight mebibytes, and a test that reached it with real records
    /// would have to build a store of some two hundred thousand words to do it. The hook
    /// makes an otherwise unreachable branch reachable without pretending the ceiling is a
    /// configuration value.
    static EXPORT_LIMIT: std::cell::Cell<u64> = const { Cell::new(EXPORT_LIMIT_BYTES) };
}

/// The export ceiling in force for this thread.
#[cfg(test)]
fn export_limit() -> u64 {
    EXPORT_LIMIT.with(|limit| limit.get())
}

/// The export ceiling in force: the constant, in a build that has no hook to move it.
#[cfg(not(test))]
fn export_limit() -> u64 {
    EXPORT_LIMIT_BYTES
}

/// Sets the export ceiling for the current thread. Test-only.
#[cfg(test)]
pub(super) fn set_export_limit(limit: u64) {
    EXPORT_LIMIT.with(|held| held.set(limit));
}

/// What one import did.
///
/// An import is all-or-nothing: the document is read and validated in full before a single
/// row reaches the store, so a document with one malformed row writes nothing at all. What
/// happened is reported rather than raised, because a document with a bad row is an outcome
/// of the operation and not a failure of the store -- and the frozen error list has no
/// variant that could name it without blaming the wrong container.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Rows written to the store; zero when the document was refused.
    pub applied: u64,
    /// Rows the document held that passed validation.
    pub accepted: u64,
    /// Rows the document held that failed validation.
    pub rejected: u64,
    /// Whether the document carried this format's marker. A document that does not is
    /// refused whole, however well formed its rows look.
    pub recognised: bool,
}

impl UserDb {
    /// Writes the learned words to `path` as TSV, returning the row count.
    ///
    /// The document is built in memory first and written to a temporary file beside `path`,
    /// which is then renamed over it: a rename is atomic, so a reader of `path` sees either
    /// the previous export or the new one and never a half-written file. The temporary file
    /// carries mode [`FILE_MODE`] from the moment it exists, because an export is the user's
    /// own vocabulary and is as private as the store it came from. The count covers the
    /// frequency rows and the coined rows alike.
    ///
    /// # Errors
    /// Returns [`ImeError::ExportTooLarge`] when the document would pass
    /// [`EXPORT_LIMIT_BYTES`] -- nothing is written in that case, temporary file included --
    /// [`ImeError::DictCorrupt`] when the store holds a key or a coined word the format
    /// cannot carry, and [`ImeError::DictUnavailable`] naming `path` when the file cannot be
    /// written or renamed.
    pub fn export_to_file(&self, path: impl AsRef<Path>) -> Result<u64, ImeError> {
        let path = path.as_ref();
        let (document, rows) = self.inner.render_export()?;
        let temp = temporary_path(path);
        if let Err(error) = write_private(&temp, document.as_bytes()) {
            let _ = std::fs::remove_file(&temp);
            return Err(store_error(path, &error));
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            // The export did not land; the temporary file is this call's own litter and
            // nothing else refers to it.
            let _ = std::fs::remove_file(&temp);
            return Err(store_error(path, &error));
        }
        Ok(rows)
    }

    /// Reads an export document and writes its rows into the store, returning what it did.
    ///
    /// Nothing is applied unless the whole document is well formed: the marker line has to
    /// be present, every data row has to hold the columns its section names -- seven for
    /// the frequency rows, four for the coined rows below the coined marker -- the three
    /// numeric columns have to be decimal, and the pin
    /// column has to be
    /// `0` or `1`. A document that fails any of those is refused whole and the store is left
    /// exactly as it was.
    ///
    /// A row for a key the store already holds is merged rather than replaced: the larger
    /// weight, the later use, the earlier creation time and the pin flag of either side
    /// survive. An import must never lower a count the user already earned by typing. A
    /// coined row merges the same way, by the larger commit count and the later use.
    ///
    /// # Errors
    /// Returns [`ImeError::ExportTooLarge`] when the document is larger than
    /// [`EXPORT_LIMIT_BYTES`], [`ImeError::DataReadonly`] when the store cannot be written,
    /// and [`ImeError::DictUnavailable`] when reading the store or writing it fails.
    pub fn import_tsv(&self, reader: &mut dyn Read) -> Result<ImportReport, ImeError> {
        let limit = export_limit();
        let mut bytes = Vec::new();
        reader
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| store_error(&self.inner.path, &error))?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if size > limit {
            return Err(ImeError::ExportTooLarge { bytes: size, limit });
        }
        let parsed = match std::str::from_utf8(&bytes) {
            Ok(document) => parse_export(document),
            // A document that is not UTF-8 cannot be one of ours: every column this format
            // writes is ASCII or the committed word itself.
            Err(_) => Parsed::refused(&String::from_utf8_lossy(&bytes)),
        };
        if !parsed.recognised || parsed.rejected > 0 {
            return Ok(ImportReport {
                applied: 0,
                accepted: 0,
                rejected: parsed.rejected,
                recognised: parsed.recognised,
            });
        }
        let accepted = parsed.rows.len().saturating_add(parsed.coined.len());
        let applied = self.inner.apply_import(&parsed.rows, &parsed.coined)?;
        Ok(ImportReport {
            applied,
            accepted: u64::try_from(accepted).unwrap_or(u64::MAX),
            rejected: 0,
            recognised: true,
        })
    }
}

impl Inner {
    /// Writes the learned words as TSV, returning how many rows were written.
    ///
    /// # Errors
    /// As [`UserDb::export_to_file`], plus whatever `writer` raises.
    pub(super) fn write_export(&self, writer: &mut dyn Write) -> Result<u64, ImeError> {
        let (document, rows) = self.render_export()?;
        writer
            .write_all(document.as_bytes())
            .map_err(|error| store_error(&self.path, &error))?;
        Ok(rows)
    }

    /// Renders the whole document, or reports that it does not fit the ceiling.
    ///
    /// The size is counted even once the buffer has stopped growing, so the reported figure
    /// is the size the document would have had and not the ceiling it hit -- a caller who
    /// has to trim a word list wants to know by how much. The buffer itself never passes
    /// [`EXPORT_LIMIT_BYTES`], so a store far over the ceiling costs the ceiling and not
    /// twice the store.
    ///
    /// # Errors
    /// Returns [`ImeError::ExportTooLarge`] when the document passes the ceiling and
    /// [`ImeError::DictCorrupt`] when the store holds a key the format cannot carry.
    pub(super) fn render_export(&self) -> Result<(String, u64), ImeError> {
        let (removed, unclaimed) = self.pending_snapshot();
        let mut unclaimed = self.stamp_snapshot(unclaimed);
        let mut document = ExportDocument::new(self.path.clone());
        self.scan_export(&removed, &mut unclaimed, &mut document)?;
        // A word recorded a keystroke ago and not yet flushed is still a word the user has
        // learned; it joins the document after the stored rows, in key order like them.
        let mut fresh: Vec<(&Box<str>, &Delta)> = unclaimed.iter().collect();
        fresh.sort_by(|left, right| left.0.cmp(right.0));
        for (key, delta) in fresh {
            document.push(&UserRecord {
                key: String::from(key.as_ref()),
                weight: delta.count,
                last_used_unix: delta.last_used_ms,
                created_unix: 0,
                pinned: false,
                tick: delta.tick,
                dee_q16: i64::from(delta.count) << 16,
            })?;
        }
        self.scan_coined(&mut document)?;
        document.finish()
    }

    /// Renders the coined-word section, when the store holds any coined words.
    ///
    /// One read transaction covers the table and the pending coins are folded in from a
    /// copy taken before it, so the section cannot mix two generations of a row: a coin
    /// waiting for its flush adds its window's commits to the row the file already
    /// holds, and one the file has never seen enters as its own row. A row a forget is
    /// waiting for is left out, whichever half of it is in the file.
    ///
    /// # Errors
    /// Returns [`ImeError::DictCorrupt`] when a row the store holds cannot be carried --
    /// a composite the coin path could not have written, or a word the format has no
    /// column for -- and [`ImeError::DictUnavailable`] when the transaction fails.
    fn scan_coined(&self, document: &mut ExportDocument) -> Result<(), ImeError> {
        let (coins, removed) = self.coin_snapshot();
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let rows = match txn.open_table(USER_LEXICON) {
            Ok(table) => table,
            // A store that has never flushed a coin holds no table yet, which is an
            // empty section and not a failure.
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
            Err(error) => return Err(store_error(&self.path, &error)),
        };
        // The rows collect into a map keyed by the composite, so a pending coin folds
        // into its row instead of doubling it, and the iteration is the map's key order
        // rather than whatever order the two sources were read in.
        let mut merged: BTreeMap<Box<str>, CoinedExport> = BTreeMap::new();
        if let Ok(mut iter) = rows.iter().map(|walk| walk) {
            while let Some(Ok((row_key, row))) = iter.next() {
                let composite = Box::<str>::from(row_key.value());
                if removed.contains(composite.as_ref()) {
                    continue;
                }
                let (commits, last_used) = row.value();
                let Some((key, word)) = composite.split_once(COMPOSITE_SEPARATOR) else {
                    return Err(ImeError::DictCorrupt {
                        path: self.path.clone(),
                    });
                };
                let mut rendered = CoinedExport {
                    key: String::from(key),
                    word: String::from(word),
                    commits,
                    last_used_unix: last_used,
                };
                if let Some((_, count)) = coins.get(composite.as_ref()) {
                    rendered.commits = rendered.commits.saturating_add(*count);
                }
                merged.insert(composite, rendered);
            }
        }
        for (composite, (word, count)) in coins {
            if merged.contains_key(composite.as_ref()) {
                continue;
            }
            let Some((key, _)) = composite.split_once(COMPOSITE_SEPARATOR) else {
                return Err(ImeError::DictCorrupt {
                    path: self.path.clone(),
                });
            };
            let key = String::from(key);
            merged.insert(
                composite,
                CoinedExport {
                    key,
                    word: String::from(word.as_ref()),
                    commits: count,
                    last_used_unix: 0,
                },
            );
        }
        for row in merged.values() {
            document.push_coined(row)?;
        }
        Ok(())
    }

    /// Copies the pending coins and their tombstones out of the pending lock.
    ///
    /// The same reason [`Inner::pending_snapshot`] copies: the scan must not hold the
    /// delta lock across a read transaction. The maps are bounded by the pending
    /// capacity, so the copy is bounded by it too.
    fn coin_snapshot(&self) -> (HashMap<Box<str>, (Box<str>, u32)>, HashSet<Box<str>>) {
        let pending = lock(&self.pending);
        (pending.coined.clone(), pending.coined_removed.clone())
    }

    /// Renders every stored record into `document`, in key order.
    ///
    /// One read transaction covers both tables, so the document cannot mix two generations
    /// of the store. A record with no metadata row is one an older build wrote: it becomes
    /// an unpinned record with no creation time, which is exactly what it was. The words
    /// table is read through the schema view, so a store whose migration failed exports
    /// its v1 rows with the un-ticked, un-decayed columns they read as.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when a table fails and whatever
    /// [`ExportDocument::push`] reports.
    fn scan_export(
        &self,
        removed: &HashSet<Box<str>>,
        unclaimed: &mut HashMap<Box<str>, Delta>,
        document: &mut ExportDocument,
    ) -> Result<(), ImeError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error(&self.path, &error))?;
        let words = self.open_words(&txn)?;
        let meta = txn
            .open_table(USER_META)
            .map_err(|error| store_error(&self.path, &error))?;
        let mut fold = |key: &str, row: StoredRow| -> Result<(), ImeError> {
            if removed.contains(key) {
                return Ok(());
            }
            let held = meta
                .get(key)
                .map_err(|error| store_error(&self.path, &error))?;
            let (created_unix, pinned) = held.map_or((0, 0), |guard| guard.value());
            let mut record = UserRecord {
                key: String::from(key),
                weight: row.commits,
                last_used_unix: row.last_used_ms,
                created_unix,
                pinned: pinned != 0,
                tick: row.tick,
                dee_q16: row.dee_q16,
            };
            if let Some(delta) = unclaimed.remove(key) {
                record.weight = record.weight.saturating_add(delta.count);
                record.last_used_unix = record.last_used_unix.max(delta.last_used_ms);
                record.tick = record.tick.max(delta.tick);
            }
            document.push(&record)?;
            Ok(())
        };
        words.for_each(&self.path, &mut fold)?;
        Ok(())
    }

    /// Writes imported rows into the store in one transaction, returning how many landed.
    ///
    /// The pending delta is flushed first, so the value an import merges with is the value
    /// the store actually holds rather than the one it held at the last flush. The coined
    /// rows land in the same transaction as the frequency rows, under the same merge rule:
    /// the larger commit count and the later use survive, and the in-memory composite map
    /// learns the rows the store took so `is_user_word` and a later forget see them.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] when the store is read-only or the flush fails,
    /// and [`ImeError::DictUnavailable`] when a transaction fails.
    pub(super) fn apply_import(
        &self,
        rows: &[UserRecord],
        coined: &[CoinedExport],
    ) -> Result<u64, ImeError> {
        if rows.is_empty() && coined.is_empty() {
            return Ok(0);
        }
        if self.readonly.load(Ordering::Acquire) {
            return Err(ImeError::DataReadonly {
                reason: String::from("the user database is read-only"),
            });
        }
        // The flush runs on this thread with the writer held aside: the value an import merges
        // with has to be the value the store holds rather than the one it held at the last
        // flush, and an import is a management operation -- nothing calls it while the user is
        // typing -- so it may wait for that to be true.
        self.flush_now(Durability::Immediate)?;
        let mut merged: Vec<(Box<str>, Stored, bool)> = Vec::with_capacity(rows.len());
        let mut adopted: Vec<(Box<str>, Box<str>)> = Vec::with_capacity(coined.len());
        let mut txn = self
            .db
            .begin_write()
            .map_err(|error| store_error(&self.path, &error))?;
        {
            let mut words = txn
                .open_table(USER_WORDS_V2)
                .map_err(|error| store_error(&self.path, &error))?;
            let mut meta = txn
                .open_table(USER_META)
                .map_err(|error| store_error(&self.path, &error))?;
            let mut lexicon = txn
                .open_table(USER_LEXICON)
                .map_err(|error| store_error(&self.path, &error))?;
            for row in rows {
                let previous = words
                    .get(row.key.as_str())
                    .map_err(|error| store_error(&self.path, &error))?
                    .map_or(
                        StoredRow {
                            commits: 0,
                            last_used_ms: 0,
                            tick: 0,
                            dee_q16: 0,
                        },
                        |guard| {
                            let (commits, last_used_ms, tick, dee) = guard.value();
                            StoredRow {
                                commits,
                                last_used_ms,
                                tick,
                                dee_q16: dee_q16(dee),
                            }
                        },
                    );
                let weight = previous.commits.max(row.weight);
                let last_used = previous.last_used_ms.max(row.last_used_unix);
                let merged_row = Stored {
                    commits: weight,
                    // The accumulator follows the weight it belongs to, and a tick only
                    // ever moves forward: a document from an older build carries no tick
                    // (zero) and cannot rewind the clock of a record the store holds.
                    dee_q16: previous.dee_q16.max(row.dee_q16),
                    tick: previous.tick.max(row.tick),
                };
                words
                    .insert(
                        row.key.as_str(),
                        (weight, last_used, merged_row.tick, merged_row.dee_bits()),
                    )
                    .map_err(|error| store_error(&self.path, &error))?;
                let (created, pinned) = meta
                    .get(row.key.as_str())
                    .map_err(|error| store_error(&self.path, &error))?
                    .map_or((0, 0), |guard| guard.value());
                // A pin is never taken away by an import: the flag means "the user asked for
                // this word to stay", and one side of the merge saying so is enough.
                let pinned = pinned | u64::from(row.pinned);
                meta.insert(
                    row.key.as_str(),
                    (earliest_stamp(created, row.created_unix), pinned),
                )
                .map_err(|error| store_error(&self.path, &error))?;
                merged.push((Box::from(row.key.as_str()), merged_row, pinned != 0));
            }
            // The coined rows merge under the frequency rows' own rule: a count the user
            // already earned is never lowered, and the later use wins.
            for row in coined {
                let composite: Box<str> =
                    format!("{}{}{}", row.key, COMPOSITE_SEPARATOR, row.word).into();
                let previous = lexicon
                    .get(composite.as_ref())
                    .map_err(|error| store_error(&self.path, &error))?
                    .map_or((0u32, 0u64), |guard| guard.value());
                lexicon
                    .insert(
                        composite.as_ref(),
                        (
                            previous.0.max(row.commits),
                            previous.1.max(row.last_used_unix),
                        ),
                    )
                    .map_err(|error| store_error(&self.path, &error))?;
                adopted.push((composite, Box::from(row.word.as_str())));
            }
        }
        txn.set_durability(Durability::Immediate);
        txn.commit()
            .map_err(|error| store_error(&self.path, &error))?;
        for (key, row, pinned) in &merged {
            self.adopt_imported(key, *row, *pinned);
        }
        // The composite map the reads consult learns the rows the transaction took, so
        // `is_user_word` answers for an imported coin at once and a later forget finds
        // its rows without a scan.
        {
            let mut coined_map = lock(&self.coined_words);
            for (composite, word) in &adopted {
                let entries = coined_map.entry(word.clone()).or_default();
                if !entries.contains(composite) {
                    entries.push(composite.clone());
                }
            }
        }
        let landed = rows.len().saturating_add(adopted.len());
        Ok(u64::try_from(landed).unwrap_or(u64::MAX))
    }
}

/// The earlier of two creation stamps, treating a zero as "not stamped".
///
/// A record written before this build stamped creation times carries zero, which is older
/// than every real stamp and must not win the comparison: an import would then erase the
/// time the store did know.
fn earliest_stamp(held: u64, imported: u64) -> u64 {
    match (held, imported) {
        (0, other) => other,
        (kept, 0) => kept,
        (kept, other) => kept.min(other),
    }
}

/// The path the temporary file is written to before it is renamed over `path`.
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Writes `bytes` to `path`, creating it with mode [`FILE_MODE`].
///
/// The file is created private rather than created and then tightened: an export holds the
/// user's vocabulary, and a file that is world-readable for the moment between the two
/// calls has already leaked it.
///
/// # Errors
/// Returns the underlying `std::io::Error` when the file cannot be created or written.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(path)?;
    file.write_all(bytes)?;
    // The bytes are published by a rename over the final path: flushing them to the
    // device first is what keeps the name from ever leading the contents, the same
    // discipline the config migration and the dictionary writer follow.
    file.sync_all()
}
