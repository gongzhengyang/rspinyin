//! Tests for the budget validator.
//!
//! Every test drives the real documents under the repository root and edits them in
//! memory, so nothing here depends on a fixture copy of the numbers that could drift
//! from the files the gate actually reads.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::*;
use crate::budget::spec::first_number_after;

/// The real budget document, as a mutable JSON value.
fn real_json() -> Result<Value> {
    let path = repo_root()?.join(BUDGETS_FILE);
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// The real budget document, typed.
fn real_budgets() -> Result<Budgets> {
    Budgets::from_json(&real_json()?.to_string())
}

/// The real spec table, parsed.
fn real_table() -> Result<BTreeMap<String, SpecRow>> {
    let path = repo_root()?.join(SPEC_FILE);
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    parse_spec_table(&text)
}

/// Apply `edit` to one section of the real document and re-serialise it.
fn edited(name: &str, edit: impl FnOnce(&mut Map<String, Value>)) -> Result<String> {
    let mut root = real_json()?;
    let section = root
        .as_object_mut()
        .context("top level must be an object")?
        .get_mut(name)
        .and_then(Value::as_object_mut)
        .with_context(|| format!("{name} must be an object"))?;
    edit(section);
    Ok(root.to_string())
}

#[test]
fn test_parse_spec_table_reads_repository_section() -> Result<()> {
    let table = real_table()?;
    let ids = "BUDGET-LAT-01 BUDGET-LAT-02 BUDGET-LAT-03 BUDGET-LAT-04 BUDGET-LAT-05 BUDGET-MEM-01 \
               BUDGET-MEM-02 BUDGET-MEM-03 BUDGET-CPU-01 BUDGET-CPU-02 BUDGET-SIZE-01 \
               BUDGET-SIZE-02 BUDGET-NET-01 BUDGET-ROB-01";
    for id in ids.split_whitespace() {
        let row = table.get(id).with_context(|| format!("{id} missing"))?;
        assert!(!row.metric.is_empty(), "{id}: empty metric column");
        assert!(!row.threshold.is_empty(), "{id}: empty threshold column");
    }
    Ok(())
}

#[test]
fn test_parse_spec_table_without_section_is_error() {
    let result = parse_spec_table("# rspinyin\n\nno budget table here\n");
    assert!(result.is_err(), "a document without the section must fail");
}

#[test]
fn test_parse_spec_table_skips_header_and_separator_rows() -> Result<()> {
    let spec = "#### 0.5.3 budget\n\n| id | metric | threshold |\n|---|---|---|\n\
                | `BUDGET-LAT-09` | sample metric | P99 ≤ 7ms |\n\ntrailing prose\n";
    let table = parse_spec_table(spec)?;
    assert_eq!(table.len(), 1);
    let row = table.get("BUDGET-LAT-09").context("row was not parsed")?;
    assert_eq!(row.threshold, "P99 ≤ 7ms");
    Ok(())
}

#[test]
fn test_first_number_after_handles_prose_cells() {
    assert_eq!(
        first_number_after("P50 ≤ 4ms，P99 ≤ 16ms", "P99"),
        Some(16.0)
    );
    assert_eq!(
        first_number_after("144Hz 环境 P99 ≤ 12ms", "144Hz"),
        Some(12.0)
    );
    assert_eq!(
        first_number_after("连续 8 小时（RSS 漂移 ≤ 2MB）", "RSS"),
        Some(2.0)
    );
    assert_eq!(first_number_after("≤ 120ms", ""), Some(120.0));
    assert_eq!(
        first_number_after("重绘次数 = 0；无轮询定时器", "重绘次数"),
        Some(0.0)
    );
    assert_eq!(first_number_after("no digits here", ""), None);
    assert_eq!(first_number_after("P99 ≤ 3ms", "P999"), None);
}

#[test]
fn test_budgets_from_json_rejects_missing_section() {
    let json = r#"{"version":1,"source":"x","latency_ms":{}}"#;
    assert!(
        Budgets::from_json(json).is_err(),
        "a document without memory_mb must fail"
    );
}

#[test]
fn test_budgets_from_json_rejects_unknown_key() -> Result<()> {
    let text = edited("latency_ms", |section| {
        section.insert("decode_p98".to_owned(), Value::from(9.0));
    })?;
    assert!(
        Budgets::from_json(&text).is_err(),
        "a misspelled threshold must not be ignored"
    );
    Ok(())
}

#[test]
fn test_budgets_from_json_rejects_non_numeric_value() -> Result<()> {
    let text = edited("latency_ms", |section| {
        section.insert("decode_p99".to_owned(), Value::from("3ms"));
    })?;
    assert!(
        Budgets::from_json(&text).is_err(),
        "a string where a number is required must fail"
    );
    Ok(())
}

#[test]
fn test_budgets_from_json_rejects_unsupported_version() -> Result<()> {
    let mut root = real_json()?;
    root.as_object_mut()
        .context("top level must be an object")?
        .insert("version".to_owned(), Value::from(2));
    assert!(
        Budgets::from_json(&root.to_string()).is_err(),
        "an unsupported schema version must fail"
    );
    Ok(())
}

#[test]
fn test_validate_accepts_repository_files() -> Result<()> {
    let report = validate(&repo_root()?)?;
    assert_eq!(report.checked, BINDINGS.len(), "all bindings compared");
    assert_eq!(report.version, 1, "the budget schema version is 1");
    Ok(())
}

#[test]
fn test_validate_detects_threshold_drift() -> Result<()> {
    let mut budgets = real_budgets()?;
    budgets.latency_ms.decode_p99 = 0.001;
    let message = format!("{:?}", compare(&budgets, &real_table()?).err());
    assert!(message.contains("BUDGET-LAT-02"), "drift: {message}");
    assert!(message.contains("latency_ms.decode_p99"), "key: {message}");
    Ok(())
}

#[test]
fn test_compare_reports_zero_assertion_violation() -> Result<()> {
    let mut budgets = real_budgets()?;
    budgets.net_sockets = 1;
    let message = format!("{:?}", compare(&budgets, &real_table()?).err());
    assert!(message.contains("net_sockets"), "zero assertion: {message}");
    Ok(())
}

#[test]
fn test_every_binding_names_a_real_spec_row() -> Result<()> {
    let table = real_table()?;
    for binding in BINDINGS {
        assert!(
            table.contains_key(binding.1),
            "{} is bound to {}, which is not in the spec table",
            binding.0,
            binding.1
        );
    }
    Ok(())
}
