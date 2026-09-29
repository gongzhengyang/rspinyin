//! The spec's budget table and the cross-check that binds the two documents.
//!
//! Responsibility: find the budget section of `docs/dev/features.md`, read its rows, and
//! compare every threshold of the budget document against the row that owns it. A
//! disagreement is reported as a list, so one run shows the whole drift instead of the
//! first symptom.
//!
//! Boundaries: this layer reads the spec and compares; it never opens `budgets.json`,
//! whose parsed form arrives as a [`Budgets`] built by [`super::schema`]. The binding
//! table stays in the module root, next to the cell model it is written in, because it is
//! the module's data contract rather than part of the comparison.

use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};

use super::SpecCell::{MetricAfter, ThresholdAfter, ThresholdFirst, ThresholdZero};

use super::*;

/// Tolerance for float comparisons; both sides are short decimal literals.
const EPSILON: f64 = 1e-9;

/// Parse the budget table out of the spec document.
///
/// The table is located by its `#### <section>` heading and read until the first
/// line that is not a table row. Rows whose first cell is not a backticked
/// `BUDGET-*` identifier (the header and separator rows) are skipped.
///
/// # Errors
/// Returns an error when the section heading is absent or when the section holds
/// no budget rows, so a renamed section fails loudly instead of validating
/// nothing.
pub fn parse_spec_table(spec: &str) -> Result<BTreeMap<String, SpecRow>> {
    let mut rows = BTreeMap::new();
    let mut in_section = false;
    let mut seen_table = false;
    for line in spec.lines() {
        let trimmed = line.trim();
        if !in_section {
            if let Some(rest) = trimmed.strip_prefix("#### ") {
                if rest.starts_with(SPEC_SECTION) {
                    in_section = true;
                }
            }
            continue;
        }
        if trimmed.starts_with('|') {
            seen_table = true;
            if let Some((id, row)) = parse_spec_row(trimmed) {
                rows.insert(id, row);
            }
            continue;
        }
        if seen_table {
            break;
        }
    }
    ensure!(
        in_section,
        "section `{SPEC_SECTION}` not found in the spec document"
    );
    ensure!(
        !rows.is_empty(),
        "section `{SPEC_SECTION}` has no `BUDGET-*` table rows"
    );
    Ok(rows)
}

/// Read one spec table row, if it is a budget row.
fn parse_spec_row(line: &str) -> Option<(String, SpecRow)> {
    let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
    if cells.len() < 3 {
        return None;
    }
    let id = cells[0].strip_prefix('`')?.strip_suffix('`')?;
    if !id.starts_with("BUDGET-") {
        return None;
    }
    let row = SpecRow {
        budget_id: id.to_owned(),
        metric: cells[1].to_owned(),
        threshold: cells[2].to_owned(),
    };
    Some((id.to_owned(), row))
}

/// Return the first decimal number in `text` at or after `needle`.
///
/// An empty `needle` starts the search at the beginning of the cell. The spec
/// writes thresholds as prose (`P99 ≤ 16ms`, `连续 8 小时`, `重绘次数 = 0`), so the
/// scan ignores the comparison operator and takes the number that follows the
/// anchor.
pub(super) fn first_number_after(text: &str, needle: &str) -> Option<f64> {
    let start = if needle.is_empty() {
        0
    } else {
        text.find(needle)? + needle.len()
    };
    let tail = text.get(start..)?;
    let mut chars = tail.char_indices().peekable();
    let mut previous: Option<char> = None;
    while let Some((offset, ch)) = chars.next() {
        if !ch.is_ascii_digit() {
            previous = Some(ch);
            continue;
        }
        // A digit run introduced by `P` is a percentile label, not a threshold.
        // The spec packs several percentiles into one cell (`144Hz 环境 P99 ≤ 12ms`),
        // so anchoring on `144Hz` would otherwise read the `99` of `P99`.
        let is_percentile_label = matches!(previous, Some('P' | 'p'));
        let mut end = offset + 1;
        while let Some(&(next_offset, next_ch)) = chars.peek() {
            if next_ch.is_ascii_digit() || next_ch == '.' {
                end = next_offset + 1;
                chars.next();
            } else {
                break;
            }
        }
        if is_percentile_label {
            previous = Some('9');
            continue;
        }
        return tail.get(offset..end)?.parse::<f64>().ok();
    }
    None
}

/// Compare every threshold against the spec table.
///
/// # Errors
/// Returns an error listing every disagreement found, so one run reports the
/// whole drift instead of the first symptom.
pub fn compare(budgets: &Budgets, table: &BTreeMap<String, SpecRow>) -> Result<Report> {
    let thresholds = budgets.thresholds();
    let mut problems: Vec<String> = Vec::new();

    for threshold in &thresholds {
        let (key, value) = (threshold.0, threshold.1);
        let Some(binding) = BINDINGS.iter().find(|b| b.0 == key) else {
            problems.push(format!(
                "{key}: no spec binding is declared for this threshold; add it to BINDINGS"
            ));
            continue;
        };
        let (id, cell) = (binding.1, binding.2);
        let Some(row) = table.get(id) else {
            problems.push(format!(
                "{key}: {id} is missing from the spec table (renamed or removed?)"
            ));
            continue;
        };
        match cell {
            ThresholdFirst => check_number(&mut problems, key, value, row, "", &row.threshold),
            ThresholdAfter(needle) => {
                check_number(&mut problems, key, value, row, needle, &row.threshold);
            }
            MetricAfter(needle) => {
                check_number(&mut problems, key, value, row, needle, &row.metric);
            }
            ThresholdZero(needle) => check_zero(&mut problems, key, value, row, needle),
        }
    }
    for binding in BINDINGS {
        if !thresholds.iter().any(|t| t.0 == binding.0) {
            problems.push(format!(
                "{}: bound in BINDINGS but absent from {BUDGETS_FILE}",
                binding.0
            ));
        }
    }

    if problems.is_empty() {
        return Ok(Report {
            version: budgets.version,
            checked: thresholds.len(),
        });
    }
    bail!(
        "{BUDGETS_FILE} disagrees with {SPEC_FILE} section {SPEC_SECTION} ({} problem(s)):\n  - {}\n\
         edit {BUDGETS_FILE} to match the spec table, or change the spec table first; the spec is authoritative",
        problems.len(),
        problems.join("\n  - ")
    )
}

/// Record a problem when the spec cell does not state the budgeted number.
fn check_number(
    problems: &mut Vec<String>,
    key: &str,
    value: f64,
    row: &SpecRow,
    needle: &str,
    text: &str,
) {
    match first_number_after(text, needle) {
        None => {
            let anchor = if needle.is_empty() {
                "the cell".to_owned()
            } else {
                format!("`{needle}`")
            };
            problems.push(format!(
                "{key} ({}): no number follows {anchor} in the spec cell \"{text}\"",
                row.budget_id
            ));
        }
        Some(spec_value) if (spec_value - value).abs() > EPSILON => {
            problems.push(format!(
                "{key} ({}): the spec says {spec_value}, {BUDGETS_FILE} says {value}",
                row.budget_id
            ));
        }
        Some(_) => {}
    }
}

/// Record a problem when a zero-assertion in the spec is not honoured.
fn check_zero(problems: &mut Vec<String>, key: &str, value: f64, row: &SpecRow, needle: &str) {
    if !row.threshold.contains(needle) {
        problems.push(format!(
            "{key} ({}): the spec cell no longer mentions `{needle}`; review this binding",
            row.budget_id
        ));
    } else if value != 0.0 {
        problems.push(format!(
            "{key} ({}): the spec requires `{needle}`, so the budget must be 0, but {BUDGETS_FILE} says {value}",
            row.budget_id
        ));
    }
}
