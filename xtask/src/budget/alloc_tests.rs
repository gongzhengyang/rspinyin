//! Tests for the allocation-budget gate.
//!
//! Every test drives the real budget document under the repository root and a report it
//! wrote itself, so nothing here depends on a copy of the ceilings that could drift from
//! the files the gate actually reads. The reverse verification -- a count that is
//! comfortably inside its budget failing once the budget itself is made impossible --
//! is what shows the comparison reads the document rather than a constant.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::alloc::{AllocReadings, judge, parse, run as run_alloc};
use super::*;

/// The real budget document, typed.
fn real_budgets() -> Result<Budgets> {
    let path = repo_root()?.join(BUDGETS_FILE);
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Budgets::from_json(&text)
}

/// A scratch directory under the workspace `target/`.
///
/// The workspace's `target/` is ignored by git and disposable, so a test writes there
/// rather than into a system temporary directory: nothing it leaves behind can reach a
/// commit.
fn scratch_dir(tag: &str) -> Result<PathBuf> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = repo_root()?
        .join("target")
        .join("budget-tests")
        .join(format!("alloc-{tag}-{unique}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A report whose one budgeted count is at `steady` and whose evidence records are
/// filled in around it.
fn full_report(steady: u64) -> AllocReadings {
    AllocReadings {
        decode_cold: Some(steady.saturating_add(64)),
        decode_steady: Some(steady),
        decode_steady_short: Some(steady),
        bytes_decode_steady: Some(12_288),
    }
}

/// Renders readings as the text a report file carries.
fn report_text(readings: &AllocReadings) -> String {
    let mut text = String::from("rspinyin-alloc-report 1\n");
    for (name, value) in [
        ("decode_cold", readings.decode_cold),
        ("decode_steady", readings.decode_steady),
        ("decode_steady_short", readings.decode_steady_short),
        ("bytes_decode_steady", readings.bytes_decode_steady),
    ] {
        if let Some(value) = value {
            text.push_str(&format!("{name}={value}\n"));
        }
    }
    text
}

/// The real budget document with the allocation ceiling replaced.
fn with_ceiling(value: u64) -> Result<Budgets> {
    let mut budgets = real_budgets()?;
    budgets.alloc_count.decode_steady = value;
    Ok(budgets)
}

#[test]
fn test_judge_passes_a_steady_state_at_its_ceiling() -> Result<()> {
    let ceiling = real_budgets()?.alloc_count.decode_steady;
    let report = judge(&full_report(ceiling), &real_budgets()?)?;
    assert!(
        report.violations.is_empty(),
        "a count at its ceiling is inside it: {:?}",
        report.violations
    );
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    assert_eq!(report.passed.len(), 1, "one budget, one verdict");
    Ok(())
}

#[test]
fn test_judge_fails_a_count_past_its_ceiling() -> Result<()> {
    // The ceiling is lowered below the measured count rather than the count raised above
    // the ceiling: what this shows is that the verdict comes from the document and not
    // from a constant beside the comparison.
    let budgets = with_ceiling(0)?;
    let report = judge(&full_report(1), &budgets)?;
    assert_eq!(report.violations.len(), 1, "{report:?}");
    assert!(
        report.violations[0].starts_with("budget/alloc-exceeded:"),
        "the violation carries the delivery-channel code: {:?}",
        report.violations[0]
    );
    assert!(
        report.violations[0].contains("alloc_count.decode_steady"),
        "the violation names the threshold it is about: {:?}",
        report.violations[0]
    );
    Ok(())
}

#[test]
fn test_judge_reports_an_absent_record_as_unmeasured() -> Result<()> {
    let mut readings = full_report(0);
    readings.decode_steady = None;
    let report = judge(&readings, &real_budgets()?)?;
    assert!(report.passed.is_empty(), "{:?}", report.passed);
    assert!(
        report
            .missing
            .iter()
            .any(|line| line.contains("decode_steady")),
        "the missing record is named: {:?}",
        report.missing
    );
    Ok(())
}

#[test]
fn test_judge_names_every_record_an_empty_report_omits() -> Result<()> {
    let report = judge(&AllocReadings::default(), &real_budgets()?)?;
    assert!(report.passed.is_empty(), "{report:?}");
    assert!(report.violations.is_empty(), "{report:?}");
    assert_eq!(
        report.missing.len(),
        4,
        "a report that measured nothing is a failed run, not a pass: {:?}",
        report.missing
    );
    Ok(())
}

#[test]
fn test_schema_refuses_an_allocation_threshold_the_gate_does_not_name() {
    // The binding table and the section are kept in step by the schema: a threshold the
    // gate has no measurement for cannot enter the document unnoticed, because the
    // schema refuses any key the typed document does not carry. That is what stands in
    // for the reverse check the gate runs over the thresholds it is handed, which a
    // typed document with no such key cannot reach.
    let text = serde_json::json!({
        "version": 1, "source": "x",
        "latency_ms": {}, "memory_mb": {}, "cpu_pct": {}, "size_mb": {},
        "robustness": {}, "bench": {},
        "alloc_count": { "decode_steady": 0, "keystroke_steady": 8 },
        "net_sockets": 0
    })
    .to_string();
    assert!(
        Budgets::from_json(&text).is_err(),
        "a threshold the schema does not name must be refused, not ignored"
    );
}

#[test]
fn test_parse_reads_the_records_the_test_writes() -> Result<()> {
    let readings = parse(&report_text(&full_report(3)))?;
    assert_eq!(readings.decode_cold, Some(67));
    assert_eq!(readings.decode_steady, Some(3));
    assert_eq!(readings.decode_steady_short, Some(3));
    assert_eq!(readings.bytes_decode_steady, Some(12_288));
    Ok(())
}

#[test]
fn test_parse_refuses_a_report_that_is_not_its_own() {
    assert!(parse("").is_err(), "an empty file is not a report");
    assert!(
        parse("alloc-report 1\ndecode_steady=0\n").is_err(),
        "a foreign header is not this format"
    );
    assert!(
        parse("rspinyin-alloc-report 1\ndecode_steady\n").is_err(),
        "a line without a value is refused"
    );
    assert!(
        parse("rspinyin-alloc-report 1\ndecode_steady=many\n").is_err(),
        "a value that is not a whole number is refused"
    );
    assert!(
        parse("rspinyin-alloc-report 1\nstedy=0\n").is_err(),
        "a misspelled record must not be read as an absent one"
    );
    assert!(
        parse("rspinyin-alloc-report 1\ndecode_steady=0\ndecode_steady=1\n").is_err(),
        "a record written twice is refused rather than resolved"
    );
}

#[test]
fn test_run_judges_a_report_file_end_to_end() -> Result<()> {
    let dir = scratch_dir("run")?;
    let path = dir.join("alloc-report.txt");
    fs::write(&path, report_text(&full_report(0)))?;
    run_alloc(Some(&path))?;
    Ok(())
}

#[test]
fn test_run_fails_when_the_report_is_absent() -> Result<()> {
    let dir = scratch_dir("absent")?;
    let stray = dir.join("alloc-report.txt");
    assert!(
        run_alloc(Some(&stray)).is_err(),
        "a report that was never written is a budget nobody measured"
    );
    Ok(())
}

#[test]
fn test_run_fails_when_the_report_is_past_its_ceiling() -> Result<()> {
    let dir = scratch_dir("exceeded")?;
    let path = dir.join("alloc-report.txt");
    fs::write(&path, report_text(&full_report(u64::MAX)))?;
    let failure = run_alloc(Some(&path)).expect_err("no ceiling is that high");
    let message = format!("{failure:#}");
    assert!(
        message.contains("budget/alloc-exceeded"),
        "the failure carries the delivery-channel code: {message}"
    );
    Ok(())
}
