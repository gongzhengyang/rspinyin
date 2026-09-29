//! The spec's threshold cells and the cross-check that binds the two documents.
//!
//! Responsibility: read the spec's own statement of every threshold and compare the
//! budget document against it. A disagreement is reported as a list, so one run
//! shows the whole drift instead of the first symptom.
//!
//! The spec states a threshold in one of two places. The cross-cutting budgets --
//! latency, memory, CPU, size, robustness -- are rows of the section 0.5.3 table.
//! The per-case ones are acceptance criteria of the task card that owns the case,
//! which is where a benchmark's own threshold belongs: it is not a product budget,
//! and the card that fixes the case is the cell that owns the number.
//!
//! Boundaries: this layer reads the spec and compares; it never opens
//! `budgets.json`, whose parsed form arrives as a [`Budgets`] built by
//! [`super::schema`]. The binding table stays in the module root, next to the cell
//! model it is written in, because it is the module's data contract rather than part
//! of the comparison.

use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};

use super::SpecCell::{MetricAfter, ThresholdAfter, ThresholdFirst, ThresholdZero};

use super::*;

/// Tolerance for float comparisons; both sides are short decimal literals.
const EPSILON: f64 = 1e-9;

/// The line that opens a task card's acceptance criteria.
const CRITERIA_HEADING: &str = "- **验收标准 (DoD)**：";

/// The spec's statement of every threshold, in both of the places it states one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spec {
    /// Rows of the section 0.5.3 budget table, keyed by `BUDGET-*` id.
    rows: BTreeMap<String, SpecRow>,
    /// Acceptance criteria of the task cards, keyed by `TASK-*#n`.
    criteria: BTreeMap<String, String>,
}

impl Spec {
    /// Reads both threshold sources out of the spec document.
    ///
    /// # Errors
    /// Returns an error when the section 0.5.3 heading is absent or the section
    /// holds no budget rows, so a renamed section fails loudly instead of validating
    /// nothing.
    pub fn parse(text: &str) -> Result<Self> {
        Ok(Self {
            rows: parse_spec_table(text)?,
            criteria: parse_card_criteria(text),
        })
    }

    /// The text a binding reads its number from, and the id to report it under.
    ///
    /// `metric` asks for a budget row's metric column rather than its threshold
    /// column. A task card states a criterion as one cell, so asking for a metric of
    /// a card is reported as a missing cell rather than answered with the whole
    /// criterion.
    ///
    /// Both lifetimes are `'a` because the returned id comes from one of two places:
    /// a card's id is the caller's `owner` string, a budget row's is the table's own
    /// `budget_id`. Tying them is what lets one return type cover both.
    pub(super) fn locate<'a>(&'a self, owner: &'a str, metric: bool) -> Option<(&'a str, &'a str)> {
        if let Some(text) = self.criteria.get(owner) {
            return if metric { None } else { Some((owner, text)) };
        }
        let row = self.rows.get(owner)?;
        let text = if metric { &row.metric } else { &row.threshold };
        Some((row.budget_id.as_str(), text.as_str()))
    }
}

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

/// Read the numbered acceptance criteria of every task card.
///
/// A criterion is keyed `TASK-1.02.06#2`, which is the form [`Binding`] names it in.
/// The criteria of a card are one line each and the list ends at the first line that
/// is not one of them, which is where the acceptance record the card gains later
/// begins.
fn parse_card_criteria(spec: &str) -> BTreeMap<String, String> {
    let mut criteria = BTreeMap::new();
    let mut card: Option<String> = None;
    let mut in_criteria = false;
    for line in spec.lines() {
        if let Some(rest) = line.strip_prefix("#### ") {
            card = rest
                .strip_prefix('`')
                .and_then(|rest| rest.split('`').next())
                .filter(|id| id.starts_with("TASK-"))
                .map(str::to_owned);
            in_criteria = false;
            continue;
        }
        if line == CRITERIA_HEADING {
            // The heading also appears in the document's own conventions section,
            // outside any card; there is no criterion to attribute there.
            in_criteria = card.is_some();
            continue;
        }
        if !in_criteria {
            continue;
        }
        let Some((index, text)) = parse_criterion(line) else {
            in_criteria = false;
            continue;
        };
        if let Some(id) = &card {
            criteria.insert(format!("{id}#{index}"), text.to_owned());
        }
    }
    criteria
}

/// Read one numbered acceptance criterion: its number and its text.
///
/// The criteria are indented under their heading, so a line that starts in the first
/// column is not one of them.
fn parse_criterion(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if trimmed.len() == line.len() {
        return None;
    }
    let (number, text) = trimmed.split_once(". ")?;
    Some((number.parse().ok()?, text.trim()))
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

/// Compare every threshold against the spec cell that owns it.
///
/// # Errors
/// Returns an error listing every disagreement found, so one run reports the
/// whole drift instead of the first symptom.
pub fn compare(budgets: &Budgets, spec: &Spec) -> Result<Report> {
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
        let (owner, cell) = (binding.1, binding.2);
        let Some((id, text)) = spec.locate(owner, matches!(cell, MetricAfter(_))) else {
            problems.push(format!(
                "{key}: {owner} is not a cell of the spec (renamed or removed?)"
            ));
            continue;
        };
        match cell {
            ThresholdFirst => check_number(&mut problems, key, value, id, "", text),
            ThresholdAfter(needle) | MetricAfter(needle) => {
                check_number(&mut problems, key, value, id, needle, text);
            }
            ThresholdZero(needle) => check_zero(&mut problems, key, value, id, needle, text),
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
        "{BUDGETS_FILE} disagrees with {SPEC_FILE} ({} problem(s)):\n  - {}\n\
         edit {BUDGETS_FILE} to match the spec, or change the spec first; the spec is authoritative",
        problems.len(),
        problems.join("\n  - ")
    )
}

/// Record a problem when the spec cell does not state the budgeted number.
fn check_number(
    problems: &mut Vec<String>,
    key: &str,
    value: f64,
    owner: &str,
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
                "{key} ({owner}): no number follows {anchor} in the spec cell \"{text}\""
            ));
        }
        Some(spec_value) if (spec_value - value).abs() > EPSILON => {
            problems.push(format!(
                "{key} ({owner}): the spec says {spec_value}, {BUDGETS_FILE} says {value}"
            ));
        }
        Some(_) => {}
    }
}

/// Record a problem when a zero-assertion in the spec is not honoured.
fn check_zero(
    problems: &mut Vec<String>,
    key: &str,
    value: f64,
    owner: &str,
    needle: &str,
    text: &str,
) {
    if !text.contains(needle) {
        problems.push(format!(
            "{key} ({owner}): the spec cell no longer mentions `{needle}`; review this binding"
        ));
    } else if value != 0.0 {
        problems.push(format!(
            "{key} ({owner}): the spec requires `{needle}`, so the budget must be 0, but {BUDGETS_FILE} says {value}"
        ));
    }
}
