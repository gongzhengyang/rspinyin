//! The table plumbing of the quality closure: reading a table's data lines, parsing its
//! optional columns, and naming the row a diagnostic refuses.
//!
//! Responsibility: the shape every table this closure reads shares -- one row per line,
//! blank lines and `#` comments skipped, columns separated by tabs -- and the English
//! diagnostic for a row that does not fit it. Both halves live together because a reader
//! that refused a row has to say which row, and the only thing that knows a row's number
//! is the loop that read it.
//!
//! Boundaries: this layer knows nothing about what a row means. It never decides whether a
//! row is acceptable, never counts anything and never writes; the readers above it own the
//! formats, and this module only parses a column they asked for and names the row they
//! refused.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::{Origin, ReadingSource};

/// Reads a table as UTF-8 text, naming the file when it cannot be read.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not UTF-8.
pub(super) fn read_table(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

/// Iterates the data lines of a table, skipping blanks and `#` comments, with one-based
/// line numbers so a diagnostic can name the row it refused.
///
/// The tuner's reader keeps a copy of this idiom for its own tables. It is private to
/// that module, and a few lines of iterator shared between two tools that never read each
/// other's files are worth less than the coupling that sharing them would add.
pub(super) fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// Parses a weight column that is present and not empty.
///
/// The parse error is replaced by a diagnostic naming the row and the value: the
/// standard error says only that a number could not be parsed, and a reader cannot see
/// which of the table's rows said it.
///
/// # Errors
///
/// Returns an error when the column is not an unsigned integer.
pub(super) fn parse_weight(path: &Path, line: usize, raw: &str) -> Result<Option<u32>> {
    let parsed = raw.parse::<u32>();
    let weight = parsed.with_context(|| bad_weight(path, line, raw))?;
    Ok(Some(weight))
}

/// Parses a `source` column that is present and not empty.
///
/// # Errors
///
/// Returns an error when the column names no reading layer.
pub(super) fn parse_layer(path: &Path, line: usize, raw: &str) -> Result<ReadingSource> {
    ReadingSource::parse(raw).with_context(|| unknown_layer(path, line, raw))
}

/// Parses an `origin` column that is present and not empty.
///
/// # Errors
///
/// Returns an error when the column is outside the three origins the format allows.
pub(super) fn parse_origin(path: &Path, line: usize, raw: &str) -> Result<Origin> {
    Origin::parse(raw).with_context(|| bad_origin(path, line, raw))
}

/// The diagnostic for a `source` column that names no layer.
fn unknown_layer(path: &Path, line: usize, raw: &str) -> String {
    format!(
        "{}:{line}: `{raw}` is not a layer; expected L1, L3b or L3c",
        path.display()
    )
}

/// The diagnostic for a pinyin column that has no legal segmentation.
pub(super) fn unsegmentable(path: &Path, line: usize) -> String {
    format!(
        "{}:{line}: the pinyin column does not segment into syllables",
        path.display()
    )
}

/// The diagnostic for a weight column that is not a number.
fn bad_weight(path: &Path, line: usize, raw: &str) -> String {
    format!("{}:{line}: `{raw}` is not a weight", path.display())
}

/// The diagnostic for an origin column outside the three the format allows.
fn bad_origin(path: &Path, line: usize, raw: &str) -> String {
    format!(
        "{}:{line}: `{raw}` is not one of unihan, manual, corpus",
        path.display()
    )
}
