//! Tests for the budget validator and the benchmark-budget gate.
//!
//! Every test drives the real documents under the repository root and edits them in
//! memory, so nothing here depends on a fixture copy of the numbers that could drift
//! from the files the gate actually reads. The benchmark gate is driven over a
//! criterion directory the test writes itself, which is what makes "a case past its
//! budget fails" verifiable without running a benchmark.

use serde_json::{Map, Value};

use super::bench::{BenchUnit, Measurement, check, parse_estimates};
use super::meta::{RunMeta, cpu_model_from, governor_from};
use super::spec::{first_number_after, parse_spec_table};
use super::*;

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

/// The real spec, parsed into its threshold cells.
fn real_spec() -> Result<Spec> {
    let path = repo_root()?.join(SPEC_FILE);
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Spec::parse(&text)
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

/// A scratch criterion output directory under the workspace target directory.
///
/// The workspace's `target/` is ignored by git and disposable, so a test writes
/// there rather than into a system temporary directory: nothing it leaves behind can
/// reach a commit, and a crashed run cannot leave a file a later run would read as
/// its own.
struct Scratch {
    /// The directory the test owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates an empty `<root>/target/budget-tests/<name>`.
    fn new(name: &str) -> Result<Self> {
        let path = repo_root()?.join("target").join("budget-tests").join(name);
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// Writes one case's statistics, the way criterion lays them out.
    fn write_estimates(&self, group: &str, case: &str, mean: f64, std_dev: f64) -> Result<()> {
        let dir = self.path.join(group).join(case).join("new");
        fs::create_dir_all(&dir)?;
        let document = serde_json::json!({
            "mean": { "point_estimate": mean },
            "std_dev": { "point_estimate": std_dev },
        });
        fs::write(dir.join("estimates.json"), document.to_string())?;
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_parse_spec_table_reads_repository_section() -> Result<()> {
    let table = parse_spec_table(&fs::read_to_string(repo_root()?.join(SPEC_FILE))?)?;
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

/// A spec document holding one budget row and the given card sections.
fn spec_with_cards(cards: &str) -> String {
    format!("#### 0.5.3 budget\n\n| `BUDGET-LAT-09` | metric | P99 ≤ 7ms |\n\n{cards}")
}

#[test]
fn test_parse_card_criteria_reads_numbered_criteria() -> Result<()> {
    let spec = "#### `TASK-9.09.09` sample card\n\n- **验收标准 (DoD)**：\n  1. first. [自动]\n  2. second. [性能]\n\n- **验收记录**：\n";
    let parsed = Spec::parse(&spec_with_cards(spec))?;
    let Some((id, text)) = parsed.locate("TASK-9.09.09#2", false) else {
        anyhow::bail!("criterion 2 was not parsed");
    };
    assert_eq!(id, "TASK-9.09.09#2");
    assert_eq!(text, "second. [性能]");
    Ok(())
}

#[test]
fn test_parse_card_criteria_stops_at_the_acceptance_record() -> Result<()> {
    let spec = "#### `TASK-9.09.09` sample card\n\n- **验收标准 (DoD)**：\n  1. only one. [自动]\n\n\
                - **验收记录**（2026-09-29）：\n  - **交付物**：3 files, 42 lines.\n";
    let parsed = Spec::parse(&spec_with_cards(spec))?;
    assert!(parsed.locate("TASK-9.09.09#1", false).is_some());
    assert!(
        parsed.locate("TASK-9.09.09#2", false).is_none(),
        "a bullet of the acceptance record is not a criterion"
    );
    Ok(())
}

#[test]
fn test_parse_card_criteria_ignores_the_document_template() -> Result<()> {
    // The document's conventions section repeats the heading outside any card; the
    // criteria there belong to no task and must not be attributed to one.
    let spec = "#### `TASK-9.09.09` sample card\n\n- **验收标准 (DoD)**：\n  1. one. [自动]\n\n\
                ## 6 conventions\n\n- **验收标准 (DoD)**：…\n  1. template. [自动]\n";
    let parsed = Spec::parse(&spec_with_cards(spec))?;
    assert!(parsed.locate("TASK-9.09.09#1", false).is_some());
    assert!(parsed.locate("TASK-9.09.09#2", false).is_none());
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
fn test_budgets_from_json_rejects_an_unknown_bench_key() -> Result<()> {
    let text = edited("bench", |section| {
        section.insert("classify_us".to_owned(), Value::from(500.0));
    })?;
    assert!(
        Budgets::from_json(&text).is_err(),
        "a per-case threshold under the wrong unit must not be ignored"
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
    let message = format!("{:?}", compare(&budgets, &real_spec()?).err());
    assert!(message.contains("BUDGET-LAT-02"), "drift: {message}");
    assert!(message.contains("latency_ms.decode_p99"), "key: {message}");
    Ok(())
}

#[test]
fn test_validate_detects_a_case_threshold_drift() -> Result<()> {
    let mut budgets = real_budgets()?;
    budgets.bench.decode_holdout_s = 0.001;
    let message = format!("{:?}", compare(&budgets, &real_spec()?).err());
    assert!(message.contains("TASK-1.02.03#7"), "owner: {message}");
    assert!(message.contains("bench.decode_holdout_s"), "key: {message}");
    Ok(())
}

#[test]
fn test_compare_reports_zero_assertion_violation() -> Result<()> {
    let mut budgets = real_budgets()?;
    budgets.net_sockets = 1;
    let message = format!("{:?}", compare(&budgets, &real_spec()?).err());
    assert!(message.contains("net_sockets"), "zero assertion: {message}");
    Ok(())
}

#[test]
fn test_every_binding_names_a_real_spec_cell() -> Result<()> {
    let spec = real_spec()?;
    for binding in BINDINGS {
        let metric = matches!(binding.2, SpecCell::MetricAfter(_));
        assert!(
            spec.locate(binding.1, metric).is_some(),
            "{} is bound to {}, which is not a spec cell",
            binding.0,
            binding.1
        );
    }
    Ok(())
}

#[test]
fn test_case_thresholds_are_the_numbers_their_cards_state() -> Result<()> {
    let spec = real_spec()?;
    let budgets = real_budgets()?;
    let cases = [
        (
            "TASK-1.02.06#2",
            budgets.bench.passthrough_classify_ns,
            500.0,
        ),
        ("TASK-1.02.02#4", budgets.bench.buffer_ops_us, 1.0),
        ("TASK-1.02.03#7", budgets.bench.decode_holdout_s, 3.0),
    ];
    for (owner, value, expected) in cases {
        let Some((id, text)) = spec.locate(owner, false) else {
            anyhow::bail!("{owner} is not a spec cell");
        };
        assert_eq!(id, owner);
        assert_eq!(
            first_number_after(text, ""),
            Some(expected),
            "{owner}: the budget document and the card disagree on \"{text}\""
        );
        assert_eq!(value, expected, "{owner}: {BUDGETS_FILE} carries {value}");
    }
    Ok(())
}

#[test]
fn test_parse_estimates_reads_mean_and_std_dev() -> Result<()> {
    let document = r#"{"mean":{"point_estimate":1500.0,"standard_error":10.0},
                       "median":{"point_estimate":1400.0},
                       "std_dev":{"point_estimate":25.0}}"#;
    let measurement = parse_estimates(document)?;
    assert_eq!(measurement.mean, 1500.0);
    assert_eq!(measurement.std_dev, 25.0);
    Ok(())
}

#[test]
fn test_parse_estimates_rejects_a_document_without_a_mean() {
    assert!(
        parse_estimates(r#"{"std_dev":{"point_estimate":25.0}}"#).is_err(),
        "a document without a mean must fail rather than read as zero"
    );
    assert!(
        parse_estimates("not json at all").is_err(),
        "a document that is not JSON must fail"
    );
}

#[test]
fn test_measurement_p99_is_three_standard_deviations_above_the_mean() {
    let measurement = Measurement {
        mean: 1_000.0,
        std_dev: 100.0,
    };
    assert_eq!(measurement.p99(), 1_300.0);
}

#[test]
fn test_bench_unit_renders_in_its_own_unit() {
    assert_eq!(BenchUnit::Nanos.render(500.0), "500ns");
    assert_eq!(BenchUnit::Micros.render(1_000.0), "1us");
    assert_eq!(BenchUnit::Millis.render(3_000_000.0), "3ms");
    assert_eq!(BenchUnit::Seconds.render(1_000_000_000.0), "1s");
}

#[test]
fn test_check_passes_a_case_inside_its_budget() -> Result<()> {
    let scratch = Scratch::new("passes")?;
    scratch.write_estimates("decode", "12syl", 1_000_000.0, 100_000.0)?;
    let report = check(&real_budgets()?, &scratch.path, Some("decode"))?;
    assert_eq!(report.passed, vec![String::from("decode/12syl")]);
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    Ok(())
}

#[test]
fn test_check_reports_a_case_past_its_budget() -> Result<()> {
    let scratch = Scratch::new("violation")?;
    scratch.write_estimates("decode", "12syl", 4_000_000.0, 0.0)?;
    let report = check(&real_budgets()?, &scratch.path, Some("decode"))?;
    assert_eq!(report.violations.len(), 1);
    let message = &report.violations[0];
    assert!(message.contains("BUDGET-LAT-02 VIOLATED"), "{message}");
    assert!(message.contains("decode/12syl"), "{message}");
    assert!(message.contains("p99_est=4ms"), "{message}");
    assert!(message.contains("budget=3ms"), "{message}");
    Ok(())
}

#[test]
fn test_check_fails_a_case_whose_budget_was_lowered() -> Result<()> {
    // The reverse verification the design asks for: a run that is comfortably inside
    // its budget must fail once the budget itself is made impossible, which is what
    // shows the assertion reads the document rather than a constant.
    let scratch = Scratch::new("lowered")?;
    scratch.write_estimates("decode", "12syl", 1_000_000.0, 100_000.0)?;
    let mut budgets = real_budgets()?;
    assert!(
        check(&budgets, &scratch.path, Some("decode"))?
            .violations
            .is_empty()
    );

    budgets.latency_ms.decode_p99 = 0.001;
    let report = check(&budgets, &scratch.path, Some("decode"))?;
    assert_eq!(report.violations.len(), 1);
    assert!(
        report.violations[0].contains("VIOLATED"),
        "{:?}",
        report.violations
    );
    assert!(
        report.violations[0].contains("decode/12syl"),
        "{:?}",
        report.violations
    );
    Ok(())
}

#[test]
fn test_check_reports_a_bound_case_without_output() -> Result<()> {
    let scratch = Scratch::new("missing")?;
    scratch.write_estimates("decode", "2syl", 1_000.0, 0.0)?;
    let report = check(&real_budgets()?, &scratch.path, Some("decode"))?;
    assert!(report.missing.contains(&String::from("decode/12syl")));
    assert!(!report.missing.contains(&String::from("decode/2syl")));
    Ok(())
}

#[test]
fn test_check_reports_a_case_no_threshold_is_bound_to() -> Result<()> {
    let scratch = Scratch::new("unbound")?;
    scratch.write_estimates("segment", "dag_build", 1_000.0, 0.0)?;
    let report = check(&real_budgets()?, &scratch.path, None)?;
    assert_eq!(report.unbound, vec![String::from("segment/dag_build")]);
    Ok(())
}

#[test]
fn test_check_narrows_the_comparison_to_one_group() -> Result<()> {
    let scratch = Scratch::new("narrowed")?;
    scratch.write_estimates("decode", "12syl", 1_000_000.0, 0.0)?;
    scratch.write_estimates("input", "buffer_ops", 100.0, 0.0)?;
    let narrowed = check(&real_budgets()?, &scratch.path, Some("input"))?;
    assert_eq!(narrowed.passed, vec![String::from("input/buffer_ops")]);
    assert!(
        !narrowed.missing.contains(&String::from("decode/12syl")),
        "a case outside the requested group is not missing"
    );
    Ok(())
}

#[test]
fn test_cpu_model_from_reads_the_model_name_line() {
    let cpuinfo =
        "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Sample CPU @ 2.40GHz\n\n";
    assert_eq!(
        cpu_model_from(cpuinfo),
        Some(String::from("Sample CPU @ 2.40GHz"))
    );
    assert_eq!(cpu_model_from("processor\t: 0\n"), None);
    assert_eq!(cpu_model_from("model name\t:  \n"), None);
}

#[test]
fn test_governor_from_ignores_an_empty_file() {
    assert_eq!(
        governor_from("powersave\n"),
        Some(String::from("powersave"))
    );
    assert_eq!(governor_from("\n"), None);
    assert_eq!(governor_from(""), None);
}

#[test]
fn test_run_meta_renders_null_for_an_absent_value() {
    let meta = RunMeta {
        cpu_model: Some(String::from("Sample CPU")),
        scaling_governor: None,
        rustflags: Some(String::from("-C target-cpu=native")),
    };
    let document: Value = serde_json::from_str(&meta.to_json()).expect("the record is JSON");
    assert_eq!(document["cpu_model"], Value::from("Sample CPU"));
    assert_eq!(document["scaling_governor"], Value::Null);
    assert_eq!(document["rustflags"], Value::from("-C target-cpu=native"));
}

/// Writes a release directory holding the three artifacts the size gate measures.
fn release_dir(scratch: &Scratch, bytes: usize) -> Result<()> {
    for name in ["librspinyin.so", "librspinyin_ui.so", "base.dict"] {
        fs::write(scratch.path.join(name), vec![0u8; bytes])?;
    }
    Ok(())
}

#[test]
fn test_measured_artifacts_are_the_files_the_packager_writes() {
    // The gate and the packager are two views of one release layout, so the names must
    // come from the same place: a rename in one of them would otherwise leave the gate
    // measuring a file that no longer exists -- which reports as a missing artifact
    // rather than as the drift it is.
    let names = MEASURED_ARTIFACTS.iter().map(|entry| entry.0);
    let measured: Vec<&str> = names.collect();
    let mut expected = crate::package::ADDON_LIBRARIES.to_vec();
    expected.push(crate::package::DICTIONARY_FILE);
    assert_eq!(measured, expected);
}

#[test]
fn test_measure_reports_a_release_artifact_that_is_not_there() -> Result<()> {
    let scratch = Scratch::new("measure-missing")?;
    let failure = measure(&real_budgets()?, &scratch.path).expect_err("no artifacts at all");
    let message = failure.to_string();
    assert!(
        message.contains("dist/verify/artifact-missing"),
        "the failure carries the delivery-channel code: {message}"
    );
    assert!(message.contains("librspinyin.so"), "{message}");
    Ok(())
}

#[test]
fn test_measure_passes_a_release_inside_its_thresholds() -> Result<()> {
    let scratch = Scratch::new("measure-passes")?;
    release_dir(&scratch, 4096)?;

    let report = measure(&real_budgets()?, &scratch.path)?;
    assert_eq!(report.passed.len(), 3, "{:?}", report.passed);
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert!(
        report.passed[0].contains("BUDGET-SIZE-01"),
        "the report names the budget it measured against: {:?}",
        report.passed
    );
    assert!(
        report.passed[2].contains("BUDGET-SIZE-02"),
        "the dictionary is measured against its own budget: {:?}",
        report.passed
    );
    run_measure(&scratch.path)?;
    Ok(())
}

#[test]
fn test_measure_fails_a_release_past_a_lowered_threshold() -> Result<()> {
    // The reverse verification the design asks for: a release that is comfortably inside
    // its budget must fail once the budget itself is made impossible, which is what shows
    // the assertion reads the document rather than a constant.
    let scratch = Scratch::new("measure-lowered")?;
    release_dir(&scratch, 4096)?;
    let mut budgets = real_budgets()?;
    assert!(
        measure(&budgets, &scratch.path)?.violations.is_empty(),
        "the fixture is inside the real thresholds"
    );

    budgets.size_mb.so_stripped = 0.001;
    let report = measure(&budgets, &scratch.path)?;
    assert_eq!(report.violations.len(), 2, "both libraries are past it");
    assert_eq!(report.passed.len(), 1, "the dictionary is not");
    assert!(
        report.violations[0].contains("dist/verify/size-budget-exceeded"),
        "{:?}",
        report.violations
    );
    assert!(
        report.violations[0].contains("BUDGET-SIZE-01"),
        "{:?}",
        report.violations
    );
    assert!(
        report.violations[0].contains("librspinyin.so"),
        "{:?}",
        report.violations
    );
    Ok(())
}

#[test]
fn test_measure_fails_a_dictionary_past_its_budget() -> Result<()> {
    let scratch = Scratch::new("measure-dictionary")?;
    release_dir(&scratch, 4096)?;
    let mut budgets = real_budgets()?;
    budgets.size_mb.base_dict = 0.001;

    let report = measure(&budgets, &scratch.path)?;
    assert_eq!(report.violations.len(), 1);
    assert!(
        report.violations[0].contains("BUDGET-SIZE-02"),
        "{:?}",
        report.violations
    );
    Ok(())
}
