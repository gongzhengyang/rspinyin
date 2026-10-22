//! The document half of the interchange: the TSV bytes the store writes, and the
//! parser that reads them back.
//!
//! Responsibility: own the format itself -- the markers, the column layouts, the writer
//! that renders rows while the ceiling holds, and the validator that refuses a document
//! before any of its rows is trusted. What a trusted row means to the store, and how a
//! document reaches a file, is the store half's ([`super`]) concern; what the bytes look
//! like is this module's.
//!
//! Boundaries: nothing here opens a file, takes a lock or touches the clock. The ceiling
//! a document is written under arrives from the store half, which is where the test hook
//! that moves it lives too.

use std::fmt::Write as _;

use ime_types::ImeError;

use super::export_limit;
use crate::user_db::UserRecord;

/// The first line of an export, and the marker an import is recognised by.
const EXPORT_MARKER: &str = "# rspinyin user dictionary export v2";
/// The marker a schema-1 build wrote. An import still recognises it: a document a
/// user exported under v1 has to come back into the store that upgraded.
const EXPORT_MARKER_V1: &str = "# rspinyin user dictionary export v1";

/// The line that names the columns, written under the marker.
const EXPORT_COLUMNS: &str = "# Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned <TAB> dee_q16 <TAB> tick";

/// The line that opens the coined-word section, written before its first row.
///
/// A section with no rows writes no line at all, so an export of a store that never
/// coined anything is byte-identical to what the format wrote before the section
/// existed. The line names the columns the rows below it carry, as the header names the
/// frequency columns; a parser keys the rows that follow it off this exact text.
const EXPORT_COINED_MARKER: &str =
    "# Coined words: key <TAB> word <TAB> commits <TAB> last_used_unix";

/// What the `key` column holds, written under the columns.
///
/// The column holds the word the user committed, because that is the key every other call
/// uses -- `freq`, `record` and the commit path that feeds them. The pinyin the user
/// typed is not a key any lookup would ever match, so an export that wrote it would
/// produce a document nothing could be imported back into.
const EXPORT_KEY_NOTE: &str =
    "# `key` is the word the user committed, which is what the frequency was recorded against.";

/// One coined-word row as a document carries it.
///
/// The `key` is the pinyin the pair was spelled with and `word` the text the commit path
/// enrolled under it -- the two halves of the composite row key the store keeps them
/// under, carried separately so the document never shows the separator. The visibility
/// is the store module's: the store's import path takes these rows as parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::user_db) struct CoinedExport {
    /// The pinyin key the word was coined under.
    pub(in crate::user_db) key: String,
    /// The word itself, as committed.
    pub(in crate::user_db) word: String,
    /// How many commits confirmed the pair.
    pub(in crate::user_db) commits: u32,
    /// The row's use stamp, as the store holds it.
    pub(in crate::user_db) last_used_unix: u64,
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
pub(super) struct ExportDocument {
    text: String,
    bytes: u64,
    rows: u64,
    /// The ceiling in force when the document was started, so that a row costs no more than
    /// one comparison.
    limit: u64,
    /// The store the document is read from, so that a key the format cannot carry is
    /// reported against the file that holds it.
    path: std::path::PathBuf,
    /// A row's own line, reused so that a store of many words does not allocate per word.
    line: String,
    /// Whether the coined section's marker line has been written. The section opens with
    /// its first row, so an empty one costs the document nothing and the exports of
    /// stores that never coined are byte-identical to the format the section joined.
    coined_opened: bool,
}

impl ExportDocument {
    /// Starts a document with its three comment lines.
    pub(super) fn new(path: std::path::PathBuf) -> Self {
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
            coined_opened: false,
        }
    }

    /// Renders one record and appends it while the document still fits.
    ///
    /// # Errors
    /// Returns [`ImeError::DictCorrupt`] when the record's key cannot be carried by the
    /// format.
    pub(super) fn push(&mut self, row: &UserRecord) -> Result<(), ImeError> {
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
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.key,
            row.weight,
            row.last_used_unix,
            row.created_unix,
            u8::from(row.pinned),
            row.dee_q16,
            row.tick
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

    /// Renders one coined row and appends it while the document still fits.
    ///
    /// The section's marker precedes the first row that reaches it, and no marker is
    /// written for a section that stays empty. The row counts toward the document's row
    /// count like a frequency row does.
    ///
    /// # Errors
    /// Returns [`ImeError::DictCorrupt`] when the row's key or its word cannot be
    /// carried by the format.
    pub(super) fn push_coined(&mut self, row: &CoinedExport) -> Result<(), ImeError> {
        if !is_exportable(&row.key) || !is_exportable(&row.word) {
            return Err(ImeError::DictCorrupt {
                path: self.path.clone(),
            });
        }
        if !self.coined_opened {
            self.coined_opened = true;
            self.bytes = self
                .bytes
                .saturating_add(u64::try_from(EXPORT_COINED_MARKER.len() + 1).unwrap_or(u64::MAX));
            self.text.push_str(EXPORT_COINED_MARKER);
            self.text.push('\n');
        }
        self.line.clear();
        // Writing into a `String` cannot fail, and the result is discarded rather than
        // unwrapped: there is no panic path to take and no error to report.
        let _ = writeln!(
            self.line,
            "{}\t{}\t{}\t{}",
            row.key, row.word, row.commits, row.last_used_unix
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
    pub(super) fn finish(self) -> Result<(String, u64), ImeError> {
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
pub(super) struct Parsed {
    /// Whether the document carried this format's marker.
    pub(super) recognised: bool,
    /// The rows that passed validation; empty when the document was refused.
    pub(super) rows: Vec<UserRecord>,
    /// The coined rows that passed validation; empty when the document carried none or
    /// was refused.
    pub(super) coined: Vec<CoinedExport>,
    /// How many rows failed validation.
    pub(super) rejected: u64,
}

impl Parsed {
    /// A document that is not an export of this format, with every data row it holds counted
    /// as rejected.
    pub(super) fn refused(document: &str) -> Self {
        Self {
            recognised: false,
            rows: Vec::new(),
            coined: Vec::new(),
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
/// A document under either marker is accepted: the v2 document is what this build
/// writes, and the v1 one is what a schema-1 build wrote -- its rows read as un-ticked,
/// un-decayed records, which is what they are. The rows below the coined marker are read
/// as coined rows; any comment line other than that marker ends the section, so a
/// document's sections cannot leak into each other.
///
/// # Panics
/// Never: every field is parsed with a checked conversion and every failure is a rejected
/// row.
pub(super) fn parse_export(document: &str) -> Parsed {
    let marker = document
        .lines()
        .map(str::trim_end)
        .find(|line| !line.is_empty());
    let recognised = marker == Some(EXPORT_MARKER) || marker == Some(EXPORT_MARKER_V1);
    if !recognised {
        return Parsed::refused(document);
    }
    let v1 = marker == Some(EXPORT_MARKER_V1);
    let mut rows = Vec::new();
    let mut coined = Vec::new();
    let mut rejected = 0u64;
    let mut in_coined = false;
    for line in document.lines().map(str::trim_end) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            in_coined = line == EXPORT_COINED_MARKER;
            continue;
        }
        if in_coined {
            match parse_coined_row(line) {
                Some(row) => coined.push(row),
                None => rejected = rejected.saturating_add(1),
            }
        } else {
            match parse_row(line, v1) {
                Some(row) => rows.push(row),
                None => rejected = rejected.saturating_add(1),
            }
        }
    }
    if rejected > 0 {
        rows.clear();
        coined.clear();
    }
    Parsed {
        recognised: true,
        rows,
        coined,
        rejected,
    }
}

/// Parses one data row, or answers `None` when it is not one.
///
/// The key is the only column that may hold anything but ASCII digits, and it is taken as
/// written: a key the store could not export is one an import would not find either. A v1
/// row carries the five columns the v1 format defined; a v2 row adds the decay
/// accumulator and the tick.
fn parse_row(line: &str, v1: bool) -> Option<UserRecord> {
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
    let (dee_q16, tick) = if v1 {
        // A v1 record has no decay history: the accumulator is the un-decayed identity
        // and the tick is zero, the same reading the migration gives a v1 row.
        if columns.next().is_some() {
            return None;
        }
        (i64::from(weight) << 16, 0)
    } else {
        let dee_q16 = columns.next()?.parse::<i64>().ok()?;
        let tick = columns.next()?.parse::<u64>().ok()?;
        if columns.next().is_some() {
            return None;
        }
        (dee_q16, tick)
    };
    if !is_exportable(key) {
        return None;
    }
    Some(UserRecord {
        key: String::from(key),
        weight,
        last_used_unix,
        created_unix,
        pinned,
        tick,
        dee_q16,
    })
}

/// Parses one coined row, or answers `None` when it is not one.
///
/// The four columns are the key, the word, the commit count and the use stamp. The key
/// and the word answer to the same carriage rules a frequency key does: a word with a
/// tab, a carriage return or a newline in it would break the row it sits in exactly as a
/// key would, and either one starting with `#` would come back as a comment.
fn parse_coined_row(line: &str) -> Option<CoinedExport> {
    let mut columns = line.split('\t');
    let key = columns.next()?;
    let word = columns.next()?;
    let commits = columns.next()?.parse::<u32>().ok()?;
    let last_used_unix = columns.next()?.parse::<u64>().ok()?;
    if columns.next().is_some() {
        return None;
    }
    if !is_exportable(key) || !is_exportable(word) {
        return None;
    }
    Some(CoinedExport {
        key: String::from(key),
        word: String::from(word),
        commits,
        last_used_unix,
    })
}
