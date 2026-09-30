//! The allocation-budget gate: a decode run's counted allocations against the document.
//!
//! Responsibility: read the report the decode test writes, judge every count it carries
//! against the ceiling `docs/dev/budgets.json` states, and fail on a budget that is past
//! its ceiling or that nothing measured. A steady-state decode that started allocating
//! again keeps every functional test green and every timing threshold satisfied on a
//! fast machine; a count is the only observation that catches it, and a count nobody
//! judges is a number a refactor is free to move.
//!
//! # Where the numbers come from
//!
//! Counting allocations needs a `#[global_allocator]`, and only a binary target can
//! install one. The producer is therefore `ime-core`'s own integration test, which
//! writes `target/alloc-report.txt` in the line-oriented form this module parses; the
//! gate is the consumer, and the two meet at the file. This is the same producer-and-
//! gate split the memory budgets run on, where the plugin writes the snapshot and
//! `--memory` judges it: neither side needs the other's code, only its format.
//!
//! # `Missing` is not `Pass`
//!
//! Every field of a [`AllocReadings`] is an `Option`, and an empty one means nothing
//! measured that count. A budget the document states whose count nobody measured is
//! reported as unmeasured and fails the gate: "nothing was measured, therefore nothing
//! is wrong" is the reading this gate exists to refuse. The report's unbudgeted records
//! are required too, because they are the evidence that the file is a run and not a
//! stub that survived from an earlier one.
//!
//! # The ceilings are read, never restated
//!
//! The binding table names *which* record each budget is judged against and nothing
//! else; every ceiling comes out of the document. A key the table names and the
//! document does not carry is reported as a defect of this module rather than skipped,
//! and so is an `alloc_count.*` key the document carries that the table does not name
//! -- a threshold nobody asserts is a threshold nobody keeps.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use super::{BUDGETS_FILE, Budgets, bench, repo_root};

/// The first line every allocation report starts with.
const HEADER: &str = "rspinyin-alloc-report 1";

/// The report the decode test writes, relative to the repository root.
///
/// The test resolves the same path from its own crate, so the two agree by construction
/// as long as the workspace is built in place; a build whose target directory is
/// elsewhere is served by passing the path explicitly.
const REPORT_FILE: &str = "target/alloc-report.txt";

/// The prefix of the budget document's section that holds the allocation ceilings.
const SECTION: &str = "alloc_count.";

/// One count a report carries, and the field of [`AllocReadings`] it fills.
///
/// Both halves are named, never numbered: the name is what the file's records are keyed
/// by, and the function pointers are the only place the record names and the struct's
/// fields are tied together.
struct Record {
    /// The record's name in the report file.
    name: &'static str,
    /// Reads the count back out of a parsed report.
    read: fn(&AllocReadings) -> Option<u64>,
    /// Fills the field, for the parser.
    write: fn(&mut AllocReadings, u64),
}

/// Every record a report is required to carry.
///
/// The budgeted record and the evidence records are one list on purpose: requiring the
/// evidence is what keeps a half-written file from passing, and a run that measured one
/// count but not the others is a run worth asking about rather than one to trust.
const RECORDS: &[Record] = &[
    Record {
        name: "decode_cold",
        read: |r| r.decode_cold,
        write: |r, v| r.decode_cold = Some(v),
    },
    Record {
        name: "decode_steady",
        read: |r| r.decode_steady,
        write: |r, v| r.decode_steady = Some(v),
    },
    Record {
        name: "decode_steady_short",
        read: |r| r.decode_steady_short,
        write: |r, v| r.decode_steady_short = Some(v),
    },
    Record {
        name: "bytes_decode_steady",
        read: |r| r.bytes_decode_steady,
        write: |r, v| r.bytes_decode_steady = Some(v),
    },
];

/// One allocation budget, and the record it is judged against.
///
/// Both fields are names: the dotted path of the threshold in the budget document, and
/// the record of the report it is judged against. Neither is a number, so this table
/// cannot restate a ceiling.
struct Budgeted {
    /// Dotted path of the threshold in the budget document.
    key: &'static str,
    /// The record the threshold is judged against.
    record: &'static str,
}

/// The allocation budgets the gate asserts.
const BUDGETED: &[Budgeted] = &[Budgeted {
    key: "alloc_count.decode_steady",
    record: "decode_steady",
}];

/// The counts one decode run recorded, as the report carried them.
///
/// Fields are public so a test can build a report with one record missing and show that
/// the gate refuses it, which is the property the gate exists for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllocReadings {
    /// Allocations the first decode into a fresh workspace cost: the cost the buffer
    /// reuse is worth.
    pub decode_cold: Option<u64>,
    /// Allocations one decode costs in a workspace that has already decoded once. The
    /// record the allocation budget is judged against.
    pub decode_steady: Option<u64>,
    /// The same steady state on the shortest input the assertions use, so that a count
    /// that grew with the input's length would show as a difference between the two
    /// records rather than as a ceiling that happens to hold.
    pub decode_steady_short: Option<u64>,
    /// Bytes the allocator was asked for by one steady-state decode.
    ///
    /// Recorded and never judged: it is the churn of the decode rather than the peak of
    /// its working set, and the working-set budget is stated in a unit this counter does
    /// not observe.
    pub bytes_decode_steady: Option<u64>,
}

/// Outcome of an allocation-budget check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllocReport {
    /// Budgets that were measured and are inside their ceiling, one message each.
    pub passed: Vec<String>,
    /// Budgets whose measurement is past its ceiling, one message each.
    pub violations: Vec<String>,
    /// Records the report does not carry, one message each.
    pub missing: Vec<String>,
}

/// Judges a decode run's counts against every allocation budget of `budgets`.
///
/// `readings` is what the report carried and `budgets` is the validated document. The
/// document is a parameter rather than something this function reads for itself, so a
/// test can drive the comparison with a ceiling it made impossible -- which is what
/// shows the assertion reads the document rather than a constant.
///
/// # Errors
///
/// Returns an error when a binding names a key the document does not carry, and when
/// the document carries an `alloc_count.*` key no binding names. Both are defects in
/// this module rather than in the documents, and both are reported rather than skipped.
pub fn judge(readings: &AllocReadings, budgets: &Budgets) -> Result<AllocReport> {
    let mut report = AllocReport::default();
    for record in RECORDS {
        let Some(measured) = (record.read)(readings) else {
            report.missing.push(format!(
                "budget/alloc-unmeasured: `{}`: the report carries no such record; the decode \
                 test writes it when it runs",
                record.name
            ));
            continue;
        };
        let Some(budget) = BUDGETED.iter().find(|b| b.record == record.name) else {
            continue;
        };
        let Some(ceiling) = bench::threshold(budgets, budget.key) else {
            bail!(
                "{key}: bound in the gate's table but absent from {BUDGETS_FILE}",
                key = budget.key
            );
        };
        let summary = format!(
            "{} {} {measured} of {ceiling} allocations",
            bench::owner(budget.key),
            budget.key
        );
        if measured as f64 <= ceiling {
            report.passed.push(summary);
        } else {
            report
                .violations
                .push(format!("budget/alloc-exceeded: {summary}"));
        }
    }
    for threshold in budgets.thresholds() {
        if threshold.0.starts_with(SECTION) && !BUDGETED.iter().any(|b| b.key == threshold.0) {
            bail!(
                "{}: {BUDGETS_FILE} states an allocation budget this gate has no measurement \
                 for; add it to the gate's table or remove it from the document",
                threshold.0
            );
        }
    }
    Ok(report)
}

/// Entry point for the allocation-budget gate.
///
/// `report` names the file to read, or `None` for `target/alloc-report.txt`, which the
/// decode test writes when it runs. The command fails when the file cannot be read or
/// parsed, when a budget is past its ceiling, and when a record the report must carry is
/// absent.
///
/// # Errors
///
/// Returns an error when the report cannot be read or does not parse, when the budget
/// document cannot be read or does not satisfy its schema, and when the report holds a
/// violation or is missing a record.
pub fn run(report: Option<&Path>) -> Result<()> {
    let path = match report {
        Some(path) => path.to_path_buf(),
        None => default_path()?,
    };
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {}; the decode test writes it when it runs (`cargo nextest run -p \
             ime-core --test alloc_budget`)",
            path.display()
        )
    })?;
    let readings =
        parse(&text).with_context(|| format!("{}: unusable allocation report", path.display()))?;
    let budgets = super::read_budgets(&repo_root()?)?;
    let report = judge(&readings, &budgets)?;
    for line in &report.passed {
        println!("budget: {line} within budget");
    }
    for line in &report.missing {
        println!("budget: {line}");
    }
    ensure!(
        report.violations.is_empty(),
        "{} allocation budget(s) past their ceiling:\n  {}",
        report.violations.len(),
        report.violations.join("\n  ")
    );
    ensure!(
        report.missing.is_empty(),
        "{} allocation record(s) the report must carry were never measured:\n  {}",
        report.missing.len(),
        report.missing.join("\n  ")
    );
    println!(
        "budget: {} allocation budget(s) within budget",
        report.passed.len()
    );
    Ok(())
}

/// Where the report lives when the caller names no file.
///
/// # Errors
///
/// Returns an error when the repository root cannot be resolved.
fn default_path() -> Result<PathBuf> {
    Ok(repo_root()?.join(REPORT_FILE))
}

/// Parses a report's text, refusing anything the format does not name.
///
/// The format is one `name=value` record per line under a header line, carrying whole
/// numbers and nothing else. An unknown record, a record written twice, a line that is
/// not `name=value` and a value that is not a whole number are refused with the line
/// number, rather than silently contributing a zero to a report that then reads as a
/// pass.
///
/// # Errors
///
/// Returns an error when the header is absent or not the one the format fixes, and for
/// every malformed line.
pub fn parse(text: &str) -> Result<AllocReadings> {
    let mut lines = text.lines();
    let header = lines.next().context("the report is empty")?;
    ensure!(
        header.trim() == HEADER,
        "expected the header `{HEADER}`, found `{header}`"
    );
    let mut readings = AllocReadings::default();
    for (offset, line) in lines.enumerate() {
        let number = offset + 2;
        if line.trim().is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            bail!("line {number}: expected `name=value`, found `{line}`");
        };
        let value = value
            .trim()
            .parse::<u64>()
            .with_context(|| format!("line {number}: `{value}` is not a whole number"))?;
        assign(&mut readings, name, value)
            .with_context(|| format!("line {number}: unusable record"))?;
    }
    Ok(readings)
}

/// Fills the field `name` names, refusing a record that is unknown or already written.
///
/// # Errors
///
/// Returns an error when no record carries `name`, and when the field it fills already
/// holds a value, which is the same record written twice.
fn assign(readings: &mut AllocReadings, name: &str, value: u64) -> Result<()> {
    let Some(record) = RECORDS.iter().find(|record| record.name == name) else {
        bail!("unknown record `{name}`");
    };
    if (record.read)(readings).is_some() {
        bail!("`{name}` is written twice");
    }
    (record.write)(readings, value);
    Ok(())
}
