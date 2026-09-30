//! Tests for the memory-budget gate.
//!
//! Every test drives the real budget document under the repository root and edits it in
//! memory, so nothing here depends on a fixture copy of the ceilings that could drift
//! from the file the gate actually reads. The reverse verification -- a reading that is
//! comfortably inside its budget failing once the budget itself is made impossible -- is
//! what shows the comparison reads the document rather than a constant.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ime_diag::probe::{MemorySnapshot, ProbeSnapshot};

use super::memory::{judge, run as run_memory};
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
        .join(format!("memory-{tag}-{unique}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A memory section whose three growths are all inside their ceilings.
fn inside_every_ceiling() -> MemorySnapshot {
    MemorySnapshot {
        rss_kib: Some(40_960),
        dirty_kib: Some(2_048),
        baseline_rss_kib: Some(30_720),
        baseline_dirty_kib: Some(1_024),
        ui_baseline_rss_kib: Some(39_936),
        dictionary_baseline_dirty_kib: Some(2_000),
    }
}

/// The real budget document with one memory ceiling replaced.
fn with_ceiling(key: &str, value: f64) -> Result<Budgets> {
    let text = fs::read_to_string(repo_root()?.join(BUDGETS_FILE))?;
    let needle = format!("\"{key}\": ");
    let start = text
        .find(&needle)
        .with_context(|| format!("{key} is not in {BUDGETS_FILE}"))?
        + needle.len();
    let after = &text[start..];
    let end = after.find([',', '\n']).context("the value ends")?;
    let patched = format!("{}{value}{}", &text[..start], &after[end..]);
    assert_ne!(patched, text, "the ceiling to patch is in the document");
    Budgets::from_json(&patched)
}

/// The real budget document with a line inserted into its memory section.
fn with_memory_line(line: &str) -> Result<String> {
    let text = fs::read_to_string(repo_root()?.join(BUDGETS_FILE))?;
    let patched = text.replace(
        "\"dict_mmap_rss\": 25.0",
        &format!("\"dict_mmap_rss\": 25.0,\n    {line}"),
    );
    assert_ne!(patched, text, "the section to patch is in the document");
    Ok(patched)
}

#[test]
fn test_memory_gate_passes_a_snapshot_inside_every_ceiling() -> Result<()> {
    let report = judge(&inside_every_ceiling(), &real_budgets()?)?;
    assert_eq!(report.passed.len(), 3, "{:?}", report.passed);
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    // The message names the budget the reading was judged against, so a report a person
    // reads says which ceiling held rather than only that something did.
    assert!(
        report.passed[0].contains("BUDGET-MEM-02"),
        "{:?}",
        report.passed
    );
    assert!(
        report.passed[0].contains("memory_mb.plugin_rss"),
        "{:?}",
        report.passed
    );
    Ok(())
}

#[test]
fn test_memory_gate_fails_a_budget_past_a_lowered_ceiling() -> Result<()> {
    // The reverse verification: the same reading passes against the real ceiling and
    // must fail against one made impossible, which is what shows the number comes from
    // the document.
    let memory = inside_every_ceiling();
    assert!(
        judge(&memory, &real_budgets()?)?.violations.is_empty(),
        "the fixture is inside the real ceilings"
    );
    let report = judge(&memory, &with_ceiling("ui_rss", 0.001)?)?;
    assert_eq!(report.violations.len(), 1, "{:?}", report.violations);
    assert_eq!(report.passed.len(), 2, "{:?}", report.passed);
    assert!(
        report.violations[0].starts_with("budget/memory-exceeded:"),
        "the failure carries the delivery-channel code: {:?}",
        report.violations
    );
    assert!(
        report.violations[0].contains("BUDGET-MEM-01"),
        "{:?}",
        report.violations
    );
    assert!(
        report.violations[0].contains("memory_mb.ui_rss"),
        "{:?}",
        report.violations
    );
    Ok(())
}

#[test]
fn test_memory_gate_reports_a_budget_the_snapshot_does_not_measure() -> Result<()> {
    // A budget the document states whose growth nothing measured is not a pass: an
    // empty field is a reading nobody took, and reading it as zero would pass every
    // ceiling there is.
    let memory = MemorySnapshot {
        rss_kib: Some(40_960),
        dirty_kib: Some(2_048),
        baseline_rss_kib: Some(30_720),
        ..MemorySnapshot::default()
    };
    let report = judge(&memory, &real_budgets()?)?;
    assert_eq!(report.passed.len(), 1, "{:?}", report.passed);
    assert_eq!(report.missing.len(), 2, "{:?}", report.missing);
    for key in ["memory_mb.ui_rss", "memory_mb.dict_mmap_rss"] {
        assert!(
            report.missing.iter().any(|line| line.contains(key)),
            "{key} is reported as unmeasured: {:?}",
            report.missing
        );
    }
    // The message names the mark that would have taken the baseline, because an
    // unmeasured budget is a wiring that has not happened yet and the reader of the
    // failure is the person who can do it.
    assert!(
        report
            .missing
            .iter()
            .any(|line| line.contains("mark_ui_baseline")),
        "{:?}",
        report.missing
    );
    assert!(
        report.missing[0].starts_with("budget/memory-unmeasured:"),
        "{:?}",
        report.missing
    );
    Ok(())
}

#[test]
fn test_memory_gate_measures_the_growth_each_budget_names() -> Result<()> {
    // The three budgets are three different windows, and a gate that measured one of
    // them three times would pass the other two on the strength of the wrong number.
    // Each half of this test makes exactly one window too large.
    let plugin_over = MemorySnapshot {
        rss_kib: Some(102_400),
        dirty_kib: Some(4_096),
        baseline_rss_kib: Some(1_024),
        baseline_dirty_kib: Some(4_000),
        ui_baseline_rss_kib: Some(101_376),
        dictionary_baseline_dirty_kib: Some(4_096),
    };
    let report = judge(&plugin_over, &real_budgets()?)?;
    assert_eq!(report.violations.len(), 1, "{:?}", report.violations);
    assert!(
        report.violations[0].contains("memory_mb.plugin_rss"),
        "{:?}",
        report.violations
    );

    let dictionary_over = MemorySnapshot {
        rss_kib: Some(102_400),
        dirty_kib: Some(51_200),
        baseline_rss_kib: Some(101_376),
        baseline_dirty_kib: Some(1_024),
        ui_baseline_rss_kib: Some(101_376),
        dictionary_baseline_dirty_kib: Some(1_024),
    };
    let report = judge(&dictionary_over, &real_budgets()?)?;
    assert_eq!(report.violations.len(), 1, "{:?}", report.violations);
    assert!(
        report.violations[0].contains("memory_mb.dict_mmap_rss"),
        "the dictionary is judged in its own window: {:?}",
        report.violations
    );
    Ok(())
}

#[test]
fn test_every_memory_threshold_the_document_states_is_bound() -> Result<()> {
    // The gate's binding table and the document's section are two views of one set, and
    // this is the audit in the direction a missing binding would hide: every `memory_mb`
    // key of the real document has to be one the gate judges. A key added to the
    // document without a binding would leave `passed` shorter than the document's own
    // list, which is what this asserts.
    let budgets = real_budgets()?;
    let stated: Vec<&'static str> = budgets
        .thresholds()
        .iter()
        .filter(|threshold| threshold.0.starts_with("memory_mb."))
        .map(|threshold| threshold.0)
        .collect();
    assert_eq!(stated.len(), 3, "the document states three: {stated:?}");
    let report = judge(&inside_every_ceiling(), &budgets)?;
    assert_eq!(report.passed.len(), stated.len(), "{:?}", report.passed);
    Ok(())
}

#[test]
fn test_the_documents_memory_section_is_closed() -> Result<()> {
    // A fourth `memory_mb` budget cannot be added to the file alone, and one cannot be
    // removed from it alone: the document's own schema refuses an unknown key and a
    // missing one. Adding a budget therefore takes an edit to the parser, to the typed
    // document and to the gate's table, and the audit above is what fails if the last
    // of those is forgotten.
    let failure = Budgets::from_json(&with_memory_line("\"ui_scratch\": 1.0")?)
        .expect_err("an unknown memory key is refused");
    assert!(failure.to_string().contains("ui_scratch"), "{failure}");

    let text = fs::read_to_string(repo_root()?.join(BUDGETS_FILE))?;
    let removed = text.replace("\"ui_rss\": 18.0,\n", "");
    assert_ne!(removed, text, "the row to remove is in the document");
    let failure = Budgets::from_json(&removed).expect_err("a missing memory key is refused");
    assert!(failure.to_string().contains("ui_rss"), "{failure}");
    Ok(())
}

#[test]
fn test_run_reads_the_snapshot_file_it_is_given() -> Result<()> {
    // End to end, over a file the two writers produce: the snapshot's own records and
    // the memory section after them, in one file, read by one command.
    let dir = scratch_dir("run")?;
    let path = dir.join("probe.txt");
    let mut text = ProbeSnapshot::default().to_text();
    text.push_str(&inside_every_ceiling().to_text());
    fs::write(&path, &text)?;

    run_memory(Some(&path))?;
    assert_eq!(
        MemorySnapshot::parse(&text)?,
        inside_every_ceiling(),
        "the file carries the section the gate judged"
    );

    // A file that is not a snapshot at all is reported as one rather than read for
    // whatever memory records it happens to hold.
    let stray = dir.join("stray.txt");
    fs::write(&stray, "memory.rss_kib=1\n")?;
    assert!(run_memory(Some(&stray)).is_err(), "no snapshot header");
    assert!(run_memory(Some(&dir.join("absent.txt"))).is_err());
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn test_run_fails_a_file_whose_growth_is_past_a_ceiling() -> Result<()> {
    let dir = scratch_dir("run-over")?;
    let path = dir.join("probe.txt");
    let memory = MemorySnapshot {
        rss_kib: Some(102_400),
        dirty_kib: Some(51_200),
        baseline_rss_kib: Some(1_024),
        baseline_dirty_kib: Some(1_024),
        ui_baseline_rss_kib: Some(1_024),
        dictionary_baseline_dirty_kib: Some(1_024),
    };
    let mut text = ProbeSnapshot::default().to_text();
    text.push_str(&memory.to_text());
    fs::write(&path, &text)?;
    let failure = run_memory(Some(&path)).expect_err("all three ceilings are past");
    let message = failure.to_string();
    assert!(
        message.contains("memory budget(s) past their ceiling"),
        "{message}"
    );
    assert!(message.contains("budget/memory-exceeded"), "{message}");
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn test_run_fails_a_file_whose_budgets_nothing_measured() -> Result<()> {
    // A snapshot with no memory section at all is the shape a plugin that has not been
    // wired for the readings writes, and it must not read as three passes.
    let dir = scratch_dir("run-unmeasured")?;
    let path = dir.join("probe.txt");
    fs::write(&path, ProbeSnapshot::default().to_text())?;
    let failure = run_memory(Some(&path)).expect_err("nothing measured the budgets");
    let message = failure.to_string();
    assert!(message.contains("were never measured"), "{message}");
    assert!(message.contains("budget/memory-unmeasured"), "{message}");
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}
