//! The AUX column build pass: per-word refinement codes from the chaizi table.
//!
//! Responsibility: read the self-built decomposition table
//! (`data/raw/chaizi.tsv`), derive one aux-code row per word from the word's
//! first character, and emit the column in the `ENTRIES` ordering the container
//! stores. The column is the offline half of the refinement feature: the
//! runtime filters the current candidate list by these recorded codes, so the
//! read path costs one binary search plus one row slice, never a reverse
//! lookup.
//!
//! Boundaries: this module owns the table format and the row derivation, and
//! nothing else. It does not assemble the block (that is
//! `ime_dict::format::aux::encode_block` through the writer), does not reorder
//! the word list itself, and never rewrites a word: the rows are a pure
//! function of the final word list and the table, so recompiling the same
//! sources reproduces the same bytes.
//!
//! # Why the rows follow the `ENTRIES` ordering
//!
//! The container addresses an aux row by `word_id`, which is the index of the
//! word in the hash-then-text order `build::compile` established. The pass
//! therefore applies the identical sort to the final word list; the alignment
//! test pins it against the compiled `ENTRIES` records themselves.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use ime_dict::format::{AUX_ROW_SIZE, hash_word};

use crate::dictc::budget::SegmentBudgets;
use crate::dictc::build::Compiled;
use crate::dictc::source::Word;

/// Default decomposition table, relative to the repository root.
pub(crate) const DEFAULT_CHAIZI: &str = "data/raw/chaizi.tsv";

/// The aux column's per-mille share of the container ceiling.
///
/// The column is accounted separately from the payload sections because it has
/// no section-table slot; its ceiling follows the same per-mille discipline the
/// size ledger applies to the six payload sections, derived from the one
/// container ceiling in `docs/dev/budgets.json`. Four bytes per word at the
/// compiled scale is about 1.4 MiB -- 70 per-mille of the 20 MiB ceiling -- so
/// 80 leaves headroom without inviting unbounded growth.
const AUX_SHARE_PER_MILLE: u32 = 80;

/// Denominator of the per-mille share.
const PER_MILLE: u128 = 1000;

/// The compiled column: rows in `ENTRIES` order plus the coverage counters the
/// build report prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Column {
    /// One [`AUX_ROW_SIZE`]-byte row per word, `ENTRIES` ordering.
    pub(crate) rows: Vec<u8>,
    /// Words whose first character carries a code.
    pub(crate) coded: usize,
    /// Characters the table holds, for the build log.
    pub(crate) table_chars: usize,
}

/// Reads the decomposition table; `None` when the file is absent.
///
/// A missing table is the ordinary state of a checkout that predates it: the
/// container is built without the column and the runtime degrades. A file that
/// is there but malformed stops the build, because a wrong code compiled into
/// a shipped container is indistinguishable from a right one to the reader.
///
/// # Errors
/// Returns an error when a row does not have exactly two TAB-separated
/// columns, does not name exactly one Han character, its code is not one to
/// four lowercase letters, or a character repeats. The error names the line.
pub(crate) fn load_table(path: &Path) -> Result<Option<BTreeMap<char, String>>> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(None);
    };
    let mut table = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (character, code) = line
            .split_once('\t')
            .with_context(|| format!("{}:{line_no}: expected two columns", path.display()))?;
        let mut characters = character.chars();
        let (first, second) = (characters.next(), characters.next());
        ensure!(
            first.is_some() && second.is_none() && first.is_some_and(is_han),
            "{}:{line_no}: `{character}` is not one Han character",
            path.display()
        );
        ensure!(
            !code.is_empty()
                && code.len() <= 4
                && code.bytes().all(|byte| byte.is_ascii_lowercase()),
            "{}:{line_no}: code `{code}` is not 1-4 lowercase letters",
            path.display()
        );
        let character = first.unwrap_or('\0');
        ensure!(
            table.insert(character, code.to_owned()).is_none(),
            "{}:{line_no}: `{character}` carries a second code",
            path.display()
        );
    }
    Ok(Some(table))
}

/// Whether `character` is in the Han range the word list is drawn from.
fn is_han(character: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&character)
}

/// Builds the column for the final word list.
///
/// The words are sorted by the container's own `(hash, text)` order -- the
/// order `ENTRIES` records -- and each row is the code of the word's first
/// character, zero-filled; a first character outside the table reads as an
/// empty row. The pass never drops words: a column entry exists for every
/// `ENTRIES` record, coded or not.
pub(crate) fn build_column(words: &[Word], table: &BTreeMap<char, String>) -> Column {
    let mut ordered: Vec<&Word> = words.iter().collect();
    ordered.sort_by(|left, right| {
        hash_word(&left.text)
            .cmp(&hash_word(&right.text))
            .then_with(|| left.text.cmp(&right.text))
    });
    let mut rows = Vec::with_capacity(ordered.len() * AUX_ROW_SIZE);
    let mut coded = 0usize;
    for word in ordered {
        let mut row = [0u8; AUX_ROW_SIZE];
        if let Some(code) = word
            .text
            .chars()
            .next()
            .and_then(|character| table.get(&character))
        {
            row[..code.len()].copy_from_slice(code.as_bytes());
            coded += 1;
        }
        rows.extend_from_slice(&row);
    }
    Column {
        rows,
        coded,
        table_chars: table.len(),
    }
}

/// Enforces the aux column's per-mille ceiling against the budget document.
///
/// The column is not one of the container's six table sections, so the size
/// ledger does not see it; the ceiling is derived here from the same container
/// ceiling, and a build past it fails the same way a section would.
///
/// # Errors
/// Returns an error when the budget document cannot be read or the column is
/// past its share of the container ceiling.
pub(crate) fn enforce_budget(root: &Path, bytes: u64) -> Result<()> {
    let budgets = SegmentBudgets::load(root)?;
    let scaled = u128::from(budgets.container()) * u128::from(AUX_SHARE_PER_MILLE);
    let ceiling = u64::try_from(scaled / PER_MILLE).unwrap_or(u64::MAX);
    println!(
        "dictc: aux column budget: {bytes} bytes against {ceiling} \
         ({AUX_SHARE_PER_MILLE} per-mille of the container ceiling)"
    );
    ensure!(
        bytes <= ceiling,
        "dictc: aux column {bytes} bytes is past its {ceiling}-byte ceiling \
         ({AUX_SHARE_PER_MILLE} per-mille of the container ceiling); the remedy is \
         a narrower decomposition table, not a raised ceiling"
    );
    Ok(())
}

/// Runs the whole pass against the final word list: allowlist gate, table
/// load, column build, and budget enforcement, with the build log lines.
///
/// A missing table is the ordinary state of a checkout that predates it; the
/// container is built without the column and the runtime degrades. A present
/// table passes through `allowlist_check` first, because a source that is not
/// registered must stop the build before any of it is compiled.
///
/// # Errors
/// Returns the errors of [`load_table`], [`enforce_budget`], and the
/// allowlist check.
pub(crate) fn build_from(
    words: &[Word],
    root: &Path,
    chaizi: &Path,
    allowlist_check: &dyn Fn(&Path) -> Result<()>,
) -> Result<Option<Column>> {
    if !chaizi.is_file() {
        println!(
            "dictc: aux table {} is missing; building without the aux column",
            chaizi.display()
        );
        return Ok(None);
    }
    allowlist_check(chaizi)?;
    let table = load_table(chaizi)?.with_context(|| {
        format!(
            "dictc: aux table {} exists but could not be parsed",
            chaizi.display()
        )
    })?;
    let column = build_column(words, &table);
    enforce_budget(root, column.rows.len() as u64)?;
    println!(
        "dictc: aux column: {} of {} words coded from {} table characters",
        column.coded,
        words.len(),
        column.table_chars
    );
    Ok(Some(column))
}

/// Checks a compiled container's word count against a column built for it.
///
/// A mismatch means the pass ran against a word list `compile` did not see
/// (trimmed or filtered after the fact), which would misalign every row. No
/// column is trivially aligned.
///
/// # Errors
/// Returns an error when the column does not hold one row per entry.
pub(crate) fn check_alignment(compiled: &Compiled, column: Option<&Column>) -> Result<()> {
    let Some(column) = column else {
        return Ok(());
    };
    let rows = column.rows.len() / AUX_ROW_SIZE;
    ensure!(
        rows == compiled.stats.words as usize,
        "dictc: aux column holds {rows} rows for {} entries; the pass ran against \
         a different word list than the compiler",
        compiled.stats.words
    );
    Ok(())
}

#[cfg(test)]
mod tests;
