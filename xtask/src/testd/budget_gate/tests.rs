//! Tests for [`super`]: the comparison, the report and the criterion reader.
//!
//! Every comparison here is driven by a budget document the test injects, written out below in
//! the schema `docs/dev/budgets.json` has, and its numbers are the test's own rather than the
//! repository's: a fixture that restated a real ceiling would be the second copy of the document
//! that the audit at the end of this file exists to catch, and a comparison driven by a number the
//! test chose is what shows the comparison reads its document rather than a constant.
//!
//! The criterion reader is driven the same way: a test writes the directory tree criterion leaves
//! behind and asserts against the statistics in it, so "a case past its budget fails the gate" is
//! verifiable without running a benchmark and without a machine that could run one fairly.
//!
//! Nothing here touches the display server, the clock or a real process. The one test that reads
//! the repository's own document is the audit at the end, because auditing this module's source
//! against that document is the whole of what it does.

use std::fs;
use std::path::PathBuf;

use crate::budget::Budgets;

use super::*;

/// The key the tests measure a duration against.
const DECODE_P99: &str = "latency_ms.decode_p99";

/// The key the tests measure a criterion case against.
const CLASSIFY_NS: &str = "bench.passthrough_classify_ns";

/// The key the tests measure the held-out pass against.
const HOLDOUT_S: &str = "bench.decode_holdout_s";

/// The key the tests measure a count against, which the document states as exactly zero.
const IDLE_REDRAWS: &str = "cpu_pct.idle_redraw_count";

/// The key the tests measure a size against.
const UI_RSS: &str = "memory_mb.ui_rss";

/// A key no budget document states.
const ABSENT_KEY: &str = "latency_ms.no_such_budget";

/// The budget document a test injects.
///
/// It satisfies the schema in every section, because the comparison walks every key the document
/// states: a fixture that filled in only the keys a test measured would report the rest as
/// `Missing` and make every count assertion an accident of which keys were written down.
const DOCUMENT: &str = r#"{
  "version": 1,
  "source": "the budget gate's own tests",
  "latency_ms": {
    "key_to_present_p50": 9.5,
    "key_to_present_p99": 9.5,
    "key_to_present_p99_144hz": 9.5,
    "decode_p99": 7.5,
    "decode_p999": 9.5,
    "raster_p99": 9.5,
    "first_key_to_visible_p99": 9.5,
    "addon_load": 9.5
  },
  "memory_mb": { "ui_rss": 13.5, "plugin_rss": 13.5, "dict_mmap_rss": 13.5 },
  "cpu_pct": {
    "idle": 5.5,
    "typing_10cps": 5.5,
    "idle_redraw_count": 0,
    "idle_poll_timer_count": 0
  },
  "size_mb": { "so_stripped": 13.5, "base_dict": 13.5 },
  "alloc_count": { "decode_steady": 5 },
  "robustness": { "soak_hours": 5.5, "rss_drift_mb": 5.5, "pass_rate_pct": 5.5 },
  "bench": {
    "passthrough_classify_ns": 555.5,
    "input_buffer_ops_us": 5.5,
    "decode_holdout_s": 5.5,
    "ui_wakeup_latency_us": 5.5
  },
  "net_sockets": 0
}"#;

/// The injected document, parsed and schema-checked the way the gate reads the real one.
fn fixture() -> Budgets {
    Budgets::from_json(DOCUMENT).expect("the injected document satisfies the schema")
}

/// A measurement stated in milliseconds.
fn millis(key: &'static str, value: f64) -> Measurement {
    Measurement {
        key,
        value,
        unit: Unit::Millis,
    }
}

/// A measurement stated as a count.
fn count(key: &'static str, value: f64) -> Measurement {
    Measurement {
        key,
        value,
        unit: Unit::Count,
    }
}

/// The verdict one key of a comparison came to.
fn entry(verdicts: &[(&'static str, Verdict)], key: &str) -> Verdict {
    verdicts
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, verdict)| *verdict)
        .unwrap_or_else(|| panic!("{key} is a key the document states"))
}

/// One measurement per key the document states, each exactly at its ceiling.
fn at_every_ceiling(budgets: &Budgets) -> Vec<Measurement> {
    budgets
        .thresholds()
        .iter()
        .filter_map(|threshold| {
            unit_of(threshold.0).map(|unit| Measurement {
                key: threshold.0,
                value: threshold.1,
                unit,
            })
        })
        .collect()
}

/// A criterion output tree a test writes, removed when the test ends.
struct FakeCriterion {
    /// The directory this guard owns.
    path: PathBuf,
}

impl FakeCriterion {
    /// An empty tree under the system's temporary directory.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-budget-gate-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self { path }
    }

    /// Writes one case's `estimates.json`, creating the group directory above it.
    fn write(&self, case: &str, text: &str) {
        let path = estimate_path(&self.path, case);
        let parent = path.parent().expect("an estimates path has a directory");
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("creating {}: {error}", parent.display()));
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    }
}

impl Drop for FakeCriterion {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// One case's statistics, in the shape criterion writes them.
///
/// Criterion writes more than the two numbers the gate reads; a document carrying only these is
/// enough for every case here, and the refusals below drive the shapes that are not this one.
fn estimates(mean: f64, std_dev: f64) -> String {
    format!(r#"{{"mean":{{"point_estimate":{mean}}},"std_dev":{{"point_estimate":{std_dev}}}}}"#)
}

#[test]
fn test_unit_label_names_the_suffix_the_document_uses() {
    assert_eq!(Unit::Nanos.label(), "ns");
    assert_eq!(Unit::Micros.label(), "us");
    assert_eq!(Unit::Millis.label(), "ms");
    assert_eq!(Unit::Seconds.label(), "s");
    assert_eq!(Unit::Mebibytes.label(), "MiB");
    assert_eq!(Unit::Percent.label(), "%");
    assert_eq!(Unit::Hours.label(), "h");
    assert_eq!(Unit::Count.label(), "", "a count carries no suffix");
}

#[test]
fn test_unit_scale_to_converts_inside_one_dimension() {
    assert_eq!(Unit::Nanos.scale_to(Unit::Millis), Some(1e-6));
    assert_eq!(Unit::Millis.scale_to(Unit::Nanos), Some(1e6));
    assert_eq!(Unit::Micros.scale_to(Unit::Millis), Some(1e-3));
    assert_eq!(Unit::Seconds.scale_to(Unit::Millis), Some(1e3));
    assert_eq!(Unit::Hours.scale_to(Unit::Seconds), Some(3600.0));
    assert_eq!(Unit::Mebibytes.scale_to(Unit::Mebibytes), Some(1.0));
    assert_eq!(Unit::Percent.scale_to(Unit::Percent), Some(1.0));
    assert_eq!(Unit::Count.scale_to(Unit::Count), Some(1.0));
}

#[test]
fn test_unit_scale_to_refuses_two_dimensions() {
    for (from, to) in [
        (Unit::Nanos, Unit::Mebibytes),
        (Unit::Mebibytes, Unit::Nanos),
        (Unit::Millis, Unit::Percent),
        (Unit::Hours, Unit::Count),
        (Unit::Percent, Unit::Count),
        (Unit::Seconds, Unit::Mebibytes),
    ] {
        assert_eq!(
            from.scale_to(to),
            None,
            "{from:?} and {to:?} measure different quantities"
        );
    }
}

#[test]
fn test_compare_passes_a_measurement_inside_its_ceiling() {
    let verdicts = compare(&[millis(DECODE_P99, 7.5)], &fixture());
    assert_eq!(
        entry(&verdicts, DECODE_P99),
        Verdict::Pass {
            budget: 7.5,
            measured: 7.5,
            ratio: 1.0,
        },
        "a measurement exactly at its ceiling is inside it"
    );
    assert!(!entry(&verdicts, DECODE_P99).is_failure());
}

#[test]
fn test_compare_fails_a_measurement_past_its_ceiling_and_states_the_ratio() {
    let verdicts = compare(&[millis(DECODE_P99, 7.6)], &fixture());
    let verdict = entry(&verdicts, DECODE_P99);
    assert_eq!(
        verdict,
        Verdict::Fail {
            budget: 7.5,
            measured: 7.6,
            ratio: 7.6 / 7.5,
        }
    );
    assert!(verdict.is_failure());
    let Verdict::Fail { ratio, .. } = verdict else {
        unreachable!("the verdict was just asserted to be a failure")
    };
    assert!((ratio - 1.0133).abs() < 1e-3, "{ratio}");
}

#[test]
fn test_compare_reports_missing_for_a_key_nothing_measured() {
    let verdicts = compare(&[], &fixture());
    assert!(
        !verdicts.is_empty(),
        "the document states budgets, so there is something to be missing"
    );
    assert!(
        verdicts
            .iter()
            .all(|(_, verdict)| *verdict == Verdict::Missing),
        "an unmeasured budget is missing, never passed: {verdicts:?}"
    );
}

#[test]
fn test_compare_answers_for_every_key_the_document_states_in_its_own_order() {
    let budgets = fixture();
    let thresholds = budgets.thresholds();
    let expected: Vec<&str> = thresholds.iter().map(|threshold| threshold.0).collect();
    let verdicts = compare(&[], &budgets);
    let answered: Vec<&str> = verdicts.iter().map(|(key, _)| *key).collect();
    assert_eq!(answered, expected);
}

#[test]
fn test_compare_judges_a_key_by_its_highest_measurement() {
    let verdicts = compare(
        &[
            millis(DECODE_P99, 1.0),
            millis(DECODE_P99, 7.6),
            millis(DECODE_P99, 2.0),
        ],
        &fixture(),
    );
    assert_eq!(
        entry(&verdicts, DECODE_P99),
        Verdict::Fail {
            budget: 7.5,
            measured: 7.6,
            ratio: 7.6 / 7.5,
        },
        "a ceiling is a ceiling for every case measured against it"
    );
}

#[test]
fn test_compare_reads_a_measurement_in_the_unit_the_document_states() {
    let verdicts = compare(
        &[Measurement {
            key: DECODE_P99,
            value: 7_600_000.0,
            unit: Unit::Nanos,
        }],
        &fixture(),
    );
    let Verdict::Fail {
        budget, measured, ..
    } = entry(&verdicts, DECODE_P99)
    else {
        panic!("7.6ms is past a 7.5ms ceiling")
    };
    assert_eq!(budget, 7.5);
    assert!(
        (measured - 7.6).abs() < 1e-9,
        "the nanoseconds are read as the milliseconds the document states: {measured}"
    );
}

#[test]
fn test_compare_refuses_a_measurement_in_a_unit_of_another_dimension() {
    let verdicts = compare(
        &[Measurement {
            key: DECODE_P99,
            value: 1.0,
            unit: Unit::Mebibytes,
        }],
        &fixture(),
    );
    assert_eq!(
        entry(&verdicts, DECODE_P99),
        Verdict::Missing,
        "a size is not a duration, and the number that would have been read is not a verdict"
    );
}

#[test]
fn test_compare_refuses_a_measurement_that_is_not_a_number() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        let verdicts = compare(
            &[Measurement {
                key: DECODE_P99,
                value,
                unit: Unit::Millis,
            }],
            &fixture(),
        );
        assert_eq!(
            entry(&verdicts, DECODE_P99),
            Verdict::Missing,
            "{value} is not a duration, and a NaN would compare as inside every ceiling"
        );
    }
}

#[test]
fn test_compare_states_a_zero_ceiling_ratio_without_a_nan() {
    let budgets = fixture();
    let clean = compare(&[count(IDLE_REDRAWS, 0.0)], &budgets);
    assert_eq!(
        entry(&clean, IDLE_REDRAWS),
        Verdict::Pass {
            budget: 0.0,
            measured: 0.0,
            ratio: 0.0,
        }
    );
    let dirty = compare(&[count(IDLE_REDRAWS, 3.0)], &budgets);
    let Verdict::Fail { ratio, .. } = entry(&dirty, IDLE_REDRAWS) else {
        panic!("three idle redraws are past a budget of none")
    };
    assert!(
        ratio.is_infinite(),
        "a measurement against a ceiling of zero is infinitely over it, not a NaN: {ratio}"
    );
}

#[test]
fn test_unbudgeted_names_the_measurements_the_document_does_not_carry() {
    let measurements = [
        Measurement {
            key: ABSENT_KEY,
            value: 1.0,
            unit: Unit::Millis,
        },
        millis(DECODE_P99, 1.0),
        Measurement {
            key: ABSENT_KEY,
            value: 2.0,
            unit: Unit::Millis,
        },
    ];
    assert_eq!(
        unbudgeted(&measurements, &fixture()),
        [ABSENT_KEY],
        "reported once, in the order it was measured"
    );
}

#[test]
fn test_unbudgeted_is_empty_when_every_measurement_is_bound() {
    let budgets = fixture();
    assert!(unbudgeted(&[millis(DECODE_P99, 1.0)], &budgets).is_empty());
    assert!(unbudgeted(&[], &budgets).is_empty());
}

#[test]
fn test_verdict_is_failure_is_true_only_for_a_fail() {
    let failed = Verdict::Fail {
        budget: 1.0,
        measured: 2.0,
        ratio: 2.0,
    };
    let passed = Verdict::Pass {
        budget: 1.0,
        measured: 1.0,
        ratio: 1.0,
    };
    assert!(failed.is_failure());
    assert!(!passed.is_failure());
    assert!(
        !Verdict::Missing.is_failure(),
        "a missing budget is enforced by the report, which is where a waiver can excuse it"
    );
}

#[test]
fn test_gate_report_judge_keeps_the_documents_order_and_its_unbound_keys() {
    let budgets = fixture();
    let thresholds = budgets.thresholds();
    let expected: Vec<&str> = thresholds.iter().map(|threshold| threshold.0).collect();
    let report = GateReport::judge(
        &[Measurement {
            key: ABSENT_KEY,
            value: 1.0,
            unit: Unit::Millis,
        }],
        &budgets,
    );
    let answered: Vec<&str> = report.entries().iter().map(|(key, _)| *key).collect();
    assert_eq!(answered, expected);
    assert_eq!(report.unbudgeted(), &[ABSENT_KEY][..]);
}

#[test]
fn test_gate_report_missing_lists_the_unmeasured_keys() {
    let budgets = fixture();
    let report = GateReport::judge(&[millis(DECODE_P99, 1.0)], &budgets);
    let missing = report.missing();
    assert!(!missing.contains(&DECODE_P99), "{missing:?}");
    assert!(missing.contains(&UI_RSS), "{missing:?}");
    assert_eq!(
        missing.len(),
        report.entries().len() - 1,
        "every key but the measured one is missing"
    );
}

#[test]
fn test_gate_report_lines_print_a_pass_a_failure_a_missing_key_and_a_waiver() {
    let budgets = fixture();
    let report = GateReport::judge(&[millis(DECODE_P99, 7.6)], &budgets);
    let lines = report.lines(&[UI_RSS]);
    let find = |prefix: String| {
        lines
            .iter()
            .find(|line| line.starts_with(prefix.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("the report has a line for {prefix}: {lines:?}"))
    };
    let decode = find(format!("{DECODE_P99}:"));
    assert!(decode.contains("7.60ms"), "{decode}");
    assert!(decode.contains("7.50ms"), "{decode}");
    assert!(decode.contains("past the"), "{decode}");
    let rss = find(format!("{UI_RSS}:"));
    assert!(rss.contains("waived"), "{rss}");
    let redraws = find(format!("{IDLE_REDRAWS}:"));
    assert!(redraws.contains("MISSING"), "{redraws}");
}

#[test]
fn test_the_report_marks_a_criterion_measurement_as_an_estimate() {
    let report = GateReport::judge(&[], &fixture());
    let lines = report.lines(&[]);
    let note = lines.last().expect("a report always ends with its note");
    assert_eq!(note, P99_ESTIMATE_NOTE);
    assert!(note.contains("mean + 3 sigma"), "{note}");
    assert!(note.contains("estimate"), "{note}");
    assert!(note.contains("P99"), "{note}");
}

#[test]
fn test_gate_report_enforce_accepts_a_run_that_measured_every_budget() {
    let budgets = fixture();
    let report = GateReport::judge(&at_every_ceiling(&budgets), &budgets);
    assert!(report.missing().is_empty(), "{:?}", report.missing());
    assert!(
        report.unbudgeted().is_empty(),
        "every measurement names a key the document carries"
    );
    assert!(report.enforce(&[]).is_ok());
}

#[test]
fn test_gate_report_enforce_fails_on_a_regression_and_carries_the_numbers() {
    let budgets = fixture();
    let mut measurements = at_every_ceiling(&budgets);
    for measurement in &mut measurements {
        if measurement.key == DECODE_P99 {
            measurement.value = 7.6;
        }
    }
    let report = GateReport::judge(&measurements, &budgets);
    let error = report
        .enforce(&[])
        .expect_err("7.6ms is past a 7.5ms ceiling");
    let message = error.to_string();
    assert!(
        matches!(
            error,
            GateError::OverBudget { key, budget, measured, ratio, .. }
                if key == DECODE_P99 && budget == 7.5 && measured == 7.6 && ratio == 7.6 / 7.5
        ),
        "{message}"
    );
    assert!(message.contains(DECODE_P99), "{message}");
    assert!(message.contains("7.6"), "{message}");
    assert!(message.contains("7.5"), "{message}");
    assert!(message.contains("1.01"), "{message}");
    assert!(
        message.contains("BUDGET-LAT-02"),
        "the refusal names the specification cell the budget belongs to: {message}"
    );
}

#[test]
fn test_gate_report_enforce_fails_when_a_budget_was_never_measured() {
    let report = GateReport::judge(&[], &fixture());
    let error = report
        .enforce(&[])
        .expect_err("nothing was measured, so nothing is inside its ceiling");
    let GateError::Unmeasured { keys } = &error else {
        panic!("an unmeasured budget is reported as such: {error}")
    };
    assert!(keys.contains(&DECODE_P99), "{keys:?}");
    assert!(
        error.to_string().contains("never measured"),
        "the refusal says a budget was unmeasured rather than past it: {error}"
    );
}

#[test]
fn test_gate_report_enforce_passes_a_waived_budget_and_the_report_says_so() {
    let report = GateReport::judge(&[], &fixture());
    let waivers = report.missing();
    assert!(!waivers.is_empty());
    assert!(
        report.enforce(&waivers).is_ok(),
        "an explicit waiver is the one way an unmeasured budget passes"
    );
    assert!(
        report.enforce(&waivers[..1]).is_err(),
        "a waiver for one key does not excuse the rest"
    );
    assert!(
        report
            .lines(&waivers)
            .iter()
            .any(|line| line.contains("waived")),
        "a waiver leaves a trace in the report"
    );
}

#[test]
fn test_gate_report_enforce_fails_on_a_measurement_the_document_does_not_carry() {
    let report = GateReport::judge(
        &[Measurement {
            key: ABSENT_KEY,
            value: 1.0,
            unit: Unit::Millis,
        }],
        &fixture(),
    );
    let error = report
        .enforce(&[])
        .expect_err("a number judged against a key nobody states is not a verdict");
    let message = error.to_string();
    assert!(matches!(error, GateError::Unbudgeted { .. }), "{message}");
    assert!(message.contains(ABSENT_KEY), "{message}");
}

#[test]
fn test_read_criterion_reads_mean_plus_three_sigma_in_nanoseconds() {
    let tree = FakeCriterion::new("sigma");
    tree.write("passthrough/classify", &estimates(500.0, 20.0));
    let cases = [CriterionCase("passthrough/classify", CLASSIFY_NS)];
    let measurements = read_criterion(&tree.path, &cases).expect("the tree is readable");
    assert_eq!(measurements.len(), 1);
    assert_eq!(measurements[0].key, CLASSIFY_NS);
    assert_eq!(
        measurements[0].unit,
        Unit::Nanos,
        "criterion states its statistics in nanoseconds"
    );
    assert_eq!(measurements[0].value, 500.0 + 3.0 * 20.0);
}

#[test]
fn test_read_criterion_reports_an_absent_directory_as_no_measurement() {
    let tree = FakeCriterion::new("absent");
    let cases = [CriterionCase("passthrough/classify", CLASSIFY_NS)];
    let measurements = read_criterion(&tree.path, &cases).expect("an absent tree is not an error");
    assert!(measurements.is_empty());
    let verdicts = compare(&measurements, &fixture());
    assert_eq!(
        entry(&verdicts, CLASSIFY_NS),
        Verdict::Missing,
        "a benchmark that never ran leaves its budget unmeasured, not passed"
    );
}

#[test]
fn test_read_criterion_skips_a_case_that_was_never_run() {
    let tree = FakeCriterion::new("partial");
    tree.write("decode/12syl", &estimates(1_000_000.0, 0.0));
    let cases = [
        CriterionCase("decode/12syl", DECODE_P99),
        CriterionCase("decode/holdout", HOLDOUT_S),
    ];
    let measurements = read_criterion(&tree.path, &cases).expect("the tree is readable");
    assert_eq!(measurements.len(), 1);
    assert_eq!(measurements[0].key, DECODE_P99);
}

#[test]
fn test_read_criterion_refuses_a_document_that_is_not_shaped_like_criterion() {
    let tree = FakeCriterion::new("shape");
    tree.write("decode/12syl", r#"{"mean":{"point_estimate":1.0}}"#);
    let cases = [CriterionCase("decode/12syl", DECODE_P99)];
    let error = read_criterion(&tree.path, &cases).expect_err("there is no std_dev");
    let message = error.to_string();
    assert!(
        matches!(error, GateError::CriterionFormat { .. }),
        "{message}"
    );
    assert!(message.contains("std_dev.point_estimate"), "{message}");
    assert!(
        message.contains("mean"),
        "the refusal names the shape it found rather than only what it wanted: {message}"
    );
}

#[test]
fn test_read_criterion_refuses_a_document_that_is_not_json() {
    let tree = FakeCriterion::new("json");
    tree.write("decode/12syl", "the statistics are a state of mind");
    let cases = [CriterionCase("decode/12syl", DECODE_P99)];
    let error = read_criterion(&tree.path, &cases).expect_err("that is not a document");
    let message = error.to_string();
    assert!(
        matches!(error, GateError::CriterionFormat { .. }),
        "{message}"
    );
    assert!(message.contains("not valid JSON"), "{message}");
}

#[test]
fn test_read_criterion_refuses_a_statistic_that_is_not_a_duration() {
    let tree = FakeCriterion::new("negative");
    tree.write("decode/12syl", &estimates(-1.0, 0.0));
    let cases = [CriterionCase("decode/12syl", DECODE_P99)];
    let error = read_criterion(&tree.path, &cases).expect_err("a negative duration is not one");
    let message = error.to_string();
    assert!(
        matches!(error, GateError::CriterionFormat { .. }),
        "{message}"
    );
    assert!(message.contains("not a duration"), "{message}");
}

#[test]
fn test_a_criterion_case_past_its_budget_is_a_gate_failure_with_its_ratio() {
    let tree = FakeCriterion::new("regression");
    // 7.6ms, stated the way criterion reports a duration.
    tree.write("decode/12syl", &estimates(7_600_000.0, 0.0));
    let cases = [CriterionCase("decode/12syl", DECODE_P99)];
    let budgets = fixture();
    let measurements = read_criterion(&tree.path, &cases).expect("the tree is readable");
    let report = GateReport::judge(&measurements, &budgets);
    let error = report
        .enforce(&report.missing())
        .expect_err("7.6ms is past a 7.5ms ceiling");
    let GateError::OverBudget {
        key,
        measured,
        ratio,
        ..
    } = &error
    else {
        panic!("the regression is a budget overrun: {error}")
    };
    assert_eq!(*key, DECODE_P99);
    assert!((measured - 7.6).abs() < 1e-9, "{measured}");
    assert!((ratio - 7.6 / 7.5).abs() < 1e-9, "{ratio}");
}

#[test]
fn test_require_idle_machine_accepts_an_idle_machine() {
    assert!(require_idle_machine(0).is_ok());
}

#[test]
fn test_require_idle_machine_refuses_a_contaminated_run() {
    let error = require_idle_machine(20).expect_err("twenty builds were running beside this one");
    let message = error.to_string();
    assert!(
        matches!(
            error,
            GateError::Contaminated {
                concurrent_agents: 20
            }
        ),
        "{message}"
    );
    assert!(message.contains("20"), "{message}");
    assert!(message.contains("idle machine"), "{message}");
    assert!(
        message.contains("re-run"),
        "the refusal tells the reader what to do next: {message}"
    );
}

#[test]
fn test_the_unit_table_covers_every_key_the_document_states() {
    let budgets = repository_document();
    let thresholds = budgets.thresholds();
    let keys: Vec<&str> = thresholds.iter().map(|threshold| threshold.0).collect();
    assert!(!keys.is_empty(), "the document states budgets");
    for key in &keys {
        assert!(
            unit_of(key).is_some(),
            "{key} is a budget the document states and this module states no unit for it, so it \
             could never be judged"
        );
    }
    for (key, _) in KEY_UNITS {
        assert!(
            keys.contains(key),
            "{key} is a unit this module states and the document states no budget for it"
        );
    }
    assert_eq!(
        KEY_UNITS.len(),
        keys.len(),
        "one unit per key the document states and no more"
    );
}

/// The repository's own budget document, as the gate reads it.
///
/// The audits below read the repository's copy: the document is the authority.
fn repository_document() -> Budgets {
    let root = crate::budget::repo_root().expect("xtask lives under the repository root");
    crate::budget::read_budgets(&root).expect("the budget document is valid")
}

#[test]
fn test_the_repository_document_answers_every_key_and_fails_the_one_past_its_ceiling() {
    let budgets = repository_document();
    let keys = budgets.thresholds().len();
    let mut measurements = at_every_ceiling(&budgets);
    assert_eq!(
        measurements.len(),
        keys,
        "every key the document states can be measured, so none is missing for want of a unit"
    );
    let verdicts = compare(&measurements, &budgets);
    assert_eq!(verdicts.len(), keys, "one verdict per key of the document");
    assert!(
        verdicts
            .iter()
            .all(|(_, verdict)| matches!(verdict, Verdict::Pass { .. })),
        "a measurement exactly at its ceiling is inside it: {verdicts:?}"
    );
    assert!(
        GateReport::judge(&measurements, &budgets)
            .enforce(&[])
            .is_ok(),
        "a run that measured every budget of the document and stayed inside it passes"
    );

    // The other direction, over the same document: one key past its ceiling is the failure, and
    // the refusal names the specification cell the budget belongs to as well as both numbers.
    let ceiling = budgets.memory_mb.ui_rss;
    for measurement in &mut measurements {
        if measurement.key == UI_RSS {
            measurement.value = ceiling * 2.0;
        }
    }
    let error = GateReport::judge(&measurements, &budgets)
        .enforce(&[])
        .expect_err("twice a ceiling is past it");
    let message = error.to_string();
    assert!(
        matches!(
            error,
            GateError::OverBudget { key, budget, measured, .. }
                if key == UI_RSS && budget == ceiling && measured == ceiling * 2.0
        ),
        "{message}"
    );
    assert!(message.contains("BUDGET-MEM-01"), "{message}");
    assert!(
        message.contains("MiB"),
        "the numbers carry their unit: {message}"
    );
}

#[test]
fn test_the_injected_document_restates_no_ceiling_of_the_repository_document() {
    // The fixture exists so that a comparison is driven by a number the test chose; a value it
    // shared with the repository's document would make an assertion about the comparison an
    // assertion about the document, and would be the second copy of a threshold.
    let ceilings = repository_document().thresholds();
    for injected in fixture().thresholds() {
        let Some(stated) = ceilings.iter().find(|threshold| threshold.0 == injected.0) else {
            continue;
        };
        if stated.1 == 0.0 {
            // The specification requires these counters to be exactly zero, so a document that
            // satisfies it states zero and so does any fixture of one.
            continue;
        }
        assert_ne!(injected.1, stated.1, "{} restates a ceiling", injected.0);
    }
}
