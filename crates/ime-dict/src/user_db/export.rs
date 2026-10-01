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
//! # rspinyin user dictionary export v1
//! # Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned
//! # `key` is the word the user committed, which is what the frequency was recorded against.
//! nihao<TAB>128<TAB>1759161600000<TAB>1756570000000<TAB>0
//! ```
//!
//! Rows come out in key order, which is the order the store's table holds them in, so two
//! exports of an unchanged store are byte-identical and can be diffed. A line starting with
//! `#` is a comment; a document whose first non-blank line is not the marker above is not an
//! export of this format and is refused whole.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use super::*;

use super::flush::Delta;

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

/// The first line of an export, and the marker an import is recognised by.
const EXPORT_MARKER: &str = "# rspinyin user dictionary export v1";

/// The line that names the columns, written under the marker.
const EXPORT_COLUMNS: &str =
    "# Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned";

/// What the `key` column holds, written under the columns.
///
/// The column holds the word the user committed, because that is the key every other call
/// uses -- [`UserFreqSource::freq`], [`UserFreqSource::record`] and the commit path that
/// feeds them. The pinyin the user typed is not a key any lookup would ever match, so an
/// export that wrote it would produce a document nothing could be imported back into.
const EXPORT_KEY_NOTE: &str =
    "# `key` is the word the user committed, which is what the frequency was recorded against.";

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
    /// own vocabulary and is as private as the store it came from.
    ///
    /// # Errors
    /// Returns [`ImeError::ExportTooLarge`] when the document would pass
    /// [`EXPORT_LIMIT_BYTES`] -- nothing is written in that case, temporary file included --
    /// [`ImeError::DictCorrupt`] when the store holds a key the format cannot carry, and
    /// [`ImeError::DictUnavailable`] naming `path` when the file cannot be written or
    /// renamed.
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
    /// be present, every data row has to hold exactly the five tab-separated columns
    /// [`EXPORT_COLUMNS`] names, the three numeric columns have to be decimal, and the pin
    /// column has to be
    /// `0` or `1`. A document that fails any of those is refused whole and the store is left
    /// exactly as it was.
    ///
    /// A row for a key the store already holds is merged rather than replaced: the larger
    /// weight, the later use, the earlier creation time and the pin flag of either side
    /// survive. An import must never lower a count the user already earned by typing.
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
        let applied = self.inner.apply_import(&parsed.rows)?;
        Ok(ImportReport {
            applied,
            accepted: u64::try_from(parsed.rows.len()).unwrap_or(u64::MAX),
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
            })?;
        }
        document.finish()
    }

    /// Renders every stored record into `document`, in key order.
    ///
    /// One read transaction covers both tables, so the document cannot mix two generations
    /// of the store. A record with no metadata row is one an older build wrote: it becomes
    /// an unpinned record with no creation time, which is exactly what it was.
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
        let words = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(&self.path, &error))?;
        let meta = txn
            .open_table(USER_META)
            .map_err(|error| store_error(&self.path, &error))?;
        for entry in words
            .iter()
            .map_err(|error| store_error(&self.path, &error))?
        {
            let (key, value) = entry.map_err(|error| store_error(&self.path, &error))?;
            let key = key.value();
            if removed.contains(key) {
                continue;
            }
            let held = meta
                .get(key)
                .map_err(|error| store_error(&self.path, &error))?;
            let (created_unix, pinned) = held.map_or((0, 0), |guard| guard.value());
            let mut row = UserRecord {
                key: String::from(key),
                weight: value.value().0,
                last_used_unix: value.value().1,
                created_unix,
                pinned: pinned != 0,
            };
            if let Some(delta) = unclaimed.remove(key) {
                row.weight = row.weight.saturating_add(delta.count);
                row.last_used_unix = row.last_used_unix.max(delta.last_used_ms);
            }
            document.push(&row)?;
        }
        Ok(())
    }

    /// Writes imported rows into the store in one transaction, returning how many landed.
    ///
    /// The pending delta is flushed first, so the value an import merges with is the value
    /// the store actually holds rather than the one it held at the last flush.
    ///
    /// # Errors
    /// Returns [`ImeError::DataReadonly`] when the store is read-only or the flush fails,
    /// and [`ImeError::DictUnavailable`] when a transaction fails.
    pub(super) fn apply_import(&self, rows: &[UserRecord]) -> Result<u64, ImeError> {
        if rows.is_empty() {
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
        let mut merged: Vec<(Box<str>, u32, bool)> = Vec::with_capacity(rows.len());
        let mut txn = self
            .db
            .begin_write()
            .map_err(|error| store_error(&self.path, &error))?;
        {
            let mut words = txn
                .open_table(USER_WORDS)
                .map_err(|error| store_error(&self.path, &error))?;
            let mut meta = txn
                .open_table(USER_META)
                .map_err(|error| store_error(&self.path, &error))?;
            for row in rows {
                let held = words
                    .get(row.key.as_str())
                    .map_err(|error| store_error(&self.path, &error))?
                    .map_or((0, 0), |guard| guard.value());
                let weight = held.0.max(row.weight);
                let last_used = held.1.max(row.last_used_unix);
                words
                    .insert(row.key.as_str(), (weight, last_used))
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
                merged.push((Box::from(row.key.as_str()), weight, pinned != 0));
            }
        }
        txn.set_durability(Durability::Immediate);
        txn.commit()
            .map_err(|error| store_error(&self.path, &error))?;
        for (key, weight, pinned) in &merged {
            self.adopt_imported(key, *weight, *pinned);
        }
        Ok(u64::try_from(rows.len()).unwrap_or(u64::MAX))
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

/// Whether a key can be written to the document and read back unchanged.
///
/// A key holding a tab, a carriage return or a newline would break the row it sits in, and
/// one starting with `#` would come back as a comment: either way the export would not be
/// the round trip it claims to be, so the store reports the key instead of writing a
/// document it cannot read.
fn is_exportable(key: &str) -> bool {
    !key.is_empty() && !key.starts_with('#') && !key.contains(['\t', '\r', '\n'])
}

/// The document being rendered, its size and its row count.
///
/// The buffer stops growing at the ceiling while the size keeps counting, so the reported
/// figure is the size the document would have had.
struct ExportDocument {
    text: String,
    bytes: u64,
    rows: u64,
    /// The ceiling in force when the document was started, so that a row costs no more than
    /// one comparison.
    limit: u64,
    /// The store the document is read from, so that a key the format cannot carry is
    /// reported against the file that holds it.
    path: PathBuf,
    /// A row's own line, reused so that a store of many words does not allocate per word.
    line: String,
}

impl ExportDocument {
    /// Starts a document with its three comment lines.
    fn new(path: PathBuf) -> Self {
        let mut text = String::with_capacity(4096);
        for line in [EXPORT_MARKER, EXPORT_COLUMNS, EXPORT_KEY_NOTE] {
            text.push_str(line);
            text.push('\n');
        }
        Self {
            bytes: u64::try_from(text.len()).unwrap_or(u64::MAX),
            text,
            rows: 0,
            limit: export_limit(),
            path,
            line: String::with_capacity(128),
        }
    }

    /// Renders one record and appends it while the document still fits.
    ///
    /// # Errors
    /// Returns [`ImeError::DictCorrupt`] when the record's key cannot be carried by the
    /// format.
    fn push(&mut self, row: &UserRecord) -> Result<(), ImeError> {
        if !is_exportable(&row.key) {
            return Err(ImeError::DictCorrupt {
                path: self.path.clone(),
            });
        }
        self.line.clear();
        // Writing into a `String` cannot fail, and the result is discarded rather than
        // unwrapped: there is no panic path to take and no error to report.
        let _ = writeln!(
            self.line,
            "{}\t{}\t{}\t{}\t{}",
            row.key,
            row.weight,
            row.last_used_unix,
            row.created_unix,
            u8::from(row.pinned)
        );
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(self.line.len()).unwrap_or(u64::MAX));
        if self.bytes <= self.limit {
            self.text.push_str(&self.line);
        }
        self.rows = self.rows.saturating_add(1);
        Ok(())
    }

    /// Returns the document and its row count, or reports that it does not fit.
    ///
    /// # Errors
    /// Returns [`ImeError::ExportTooLarge`] when the document passed the ceiling.
    fn finish(self) -> Result<(String, u64), ImeError> {
        if self.bytes > self.limit {
            return Err(ImeError::ExportTooLarge {
                bytes: self.bytes,
                limit: self.limit,
            });
        }
        Ok((self.text, self.rows))
    }
}

/// What reading one document produced.
struct Parsed {
    /// Whether the document carried this format's marker.
    recognised: bool,
    /// The rows that passed validation; empty when the document was refused.
    rows: Vec<UserRecord>,
    /// How many rows failed validation.
    rejected: u64,
}

impl Parsed {
    /// A document that is not an export of this format, with every data row it holds counted
    /// as rejected.
    fn refused(document: &str) -> Self {
        Self {
            recognised: false,
            rows: Vec::new(),
            rejected: data_lines(document).count().try_into().unwrap_or(u64::MAX),
        }
    }
}

/// Iterates the lines that carry data: no blank lines and no comments.
fn data_lines(document: &str) -> impl Iterator<Item = &str> {
    document
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Reads an export document, validating every row before any of them is trusted.
///
/// # Panics
/// Never: every field is parsed with a checked conversion and every failure is a rejected
/// row.
fn parse_export(document: &str) -> Parsed {
    let marker = document
        .lines()
        .map(str::trim_end)
        .find(|line| !line.is_empty());
    if marker != Some(EXPORT_MARKER) {
        return Parsed::refused(document);
    }
    let mut rows = Vec::new();
    let mut rejected = 0u64;
    for line in data_lines(document) {
        match parse_row(line) {
            Some(row) => rows.push(row),
            None => rejected = rejected.saturating_add(1),
        }
    }
    if rejected > 0 {
        rows.clear();
    }
    Parsed {
        recognised: true,
        rows,
        rejected,
    }
}

/// Parses one data row, or answers `None` when it is not one.
///
/// The key is the only column that may hold anything but ASCII digits, and it is taken as
/// written: a key the store could not export is one an import would not find either.
fn parse_row(line: &str) -> Option<UserRecord> {
    let mut columns = line.split('\t');
    let key = columns.next()?;
    let weight = columns.next()?.parse::<u32>().ok()?;
    let last_used_unix = columns.next()?.parse::<u64>().ok()?;
    let created_unix = columns.next()?.parse::<u64>().ok()?;
    let pinned = match columns.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    if columns.next().is_some() || !is_exportable(key) {
        return None;
    }
    Some(UserRecord {
        key: String::from(key),
        weight,
        last_used_unix,
        created_unix,
        pinned,
    })
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
