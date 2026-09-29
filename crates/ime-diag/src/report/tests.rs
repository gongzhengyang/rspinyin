//! Unit tests for the budget dashboard: the threshold table, the verdicts, and both
//! renderings.
//!
//! Responsibility: pin what the design fixes about a report -- that a threshold comes
//! from the document and a metric is judged at the percentile the document states,
//! that an impossible threshold turns a pass into a failure, that a small or lossy
//! sample is called out, and that neither rendering can carry anything but the
//! contract's own names and whole numbers.
//!
//! Boundaries: the numbers here are made up, and one test reads the repository's
//! budget document to check that the binding table and the document agree about which
//! thresholds exist. Nothing here measures anything, so nothing here depends on the
//! machine it runs on.

use std::fs;
use std::path::Path;

use super::*;
use crate::probe::{Counter, HistSnapshot, Metric, Percentile, ProbeSnapshot, Probes};
use crate::report::render::json_string;

/// A distribution with the given percentiles and a plausible sample behind it.
fn distribution(p50: u64, p90: u64, p99: u64, p999: u64) -> HistSnapshot {
    HistSnapshot {
        count: 1_832,
        sum_us: 4_000_000,
        p50_us: p50,
        p90_us: p90,
        p99_us: p99,
        p999_us: p999,
        max_us: p999.max(p99),
    }
}

/// A snapshot with one metric filled in and every other metric empty.
fn snapshot_with(metric: Metric, sample: HistSnapshot) -> ProbeSnapshot {
    let mut metrics = crate::probe::Metrics::default();
    *metric.snapshot_mut(&mut metrics) = sample;
    ProbeSnapshot {
        sampled: Duration::from_secs(312),
        sessions: 47,
        keys: 1_832,
        metrics,
        counters: [0_u64; COUNTER_COUNT],
    }
}

/// The repository's budget document, as text.
fn budget_document() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/dev/budgets.json");
    fs::read_to_string(&path).expect("the budget document is readable")
}

/// The last segment of a dotted threshold path, which is the document's own key.
fn leaf(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

#[test]
fn test_limits_from_thresholds_keeps_only_the_keys_a_metric_is_bound_to() {
    let limits = Limits::from_thresholds(&[
        ("bench.decode_holdout_s", 3.0),
        ("latency_ms.decode_p99", 3_000.0),
        ("latency_ms.key_to_present_p50", 4_000.0),
    ]);
    assert_eq!(limits.len(), 2);
    let checks: Vec<&BudgetCheck> = limits.checks(Metric::Decode).collect();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].percentile, Percentile::P99);
    assert_eq!(checks[0].limit_us, 3_000.0);
    assert_eq!(limits.checks(Metric::Wakeup).count(), 0);
}

#[test]
fn test_limits_without_a_threshold_leave_every_metric_unbudgeted() {
    let limits = Limits::from_thresholds(&[("bench.decode_holdout_s", 3.0)]);
    assert!(limits.is_empty());
    assert_eq!(limits, Limits::unbudgeted());
    let report = ProbeReport::compare(
        &snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800)),
        &limits,
    );
    for row in &report.rows {
        assert_eq!(row.verdict, Verdict::Unbudgeted, "{}", row.metric.name());
    }
    assert_eq!(report.verdict(), Verdict::Unbudgeted);
    assert!(report.violations().is_empty());
}

#[test]
fn test_compare_passes_a_metric_inside_its_budget() {
    let limits = Limits::from_thresholds(&[("latency_ms.decode_p99", 3_000.0)]);
    let report = ProbeReport::compare(
        &snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800)),
        &limits,
    );
    assert_eq!(report.verdict(), Verdict::Pass);
    assert!(report.violations().is_empty());
    let row = &report.rows[Metric::Decode.index()];
    assert_eq!(row.metric, Metric::Decode);
    assert_eq!(row.verdict, Verdict::Pass);
    assert_eq!(row.checks.len(), 1);
}

#[test]
fn test_compare_fails_a_metric_past_an_impossible_budget() {
    // The reverse validation the design asks for: an impossible threshold must turn
    // the same measurement into a failure, which is what shows the comparison reads
    // the threshold instead of a constant.
    let sample = distribution(2_100, 5_800, 11_300, 18_700);
    let generous = Limits::from_thresholds(&[("latency_ms.key_to_present_p99", 16_000.0)]);
    assert_eq!(
        ProbeReport::compare(&snapshot_with(Metric::KeyToPresent, sample), &generous).verdict(),
        Verdict::Pass
    );
    let impossible = Limits::from_thresholds(&[("latency_ms.key_to_present_p99", 100.0)]);
    let report = ProbeReport::compare(&snapshot_with(Metric::KeyToPresent, sample), &impossible);
    assert_eq!(report.verdict(), Verdict::Fail(Percentile::P99));
    let violations = report.violations();
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("key_to_present P99"));
    assert!(violations[0].contains("latency_ms.key_to_present_p99"));
}

#[test]
fn test_compare_judges_every_threshold_a_metric_carries() {
    let limits = Limits::from_thresholds(&[
        ("latency_ms.key_to_present_p50", 4_000.0),
        ("latency_ms.key_to_present_p99", 16_000.0),
    ]);
    let inside = ProbeReport::compare(
        &snapshot_with(
            Metric::KeyToPresent,
            distribution(2_100, 5_800, 11_300, 18_700),
        ),
        &limits,
    );
    assert_eq!(inside.verdict(), Verdict::Pass);
    // The median is the one that moved: the verdict names the lowest percentile that
    // failed, so a reader fixing a regression is told which one it is.
    let outside = ProbeReport::compare(
        &snapshot_with(
            Metric::KeyToPresent,
            distribution(9_000, 12_000, 15_000, 19_000),
        ),
        &limits,
    );
    assert_eq!(outside.verdict(), Verdict::Fail(Percentile::P50));
    assert_eq!(outside.violations().len(), 1);
}

#[test]
fn test_compare_calls_a_sample_smaller_than_the_floor_insufficient() {
    let mut snapshot = snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800));
    snapshot.keys = MIN_KEYS - 1;
    let report = ProbeReport::compare(&snapshot, &Limits::unbudgeted());
    assert_eq!(report.notes.len(), 1);
    assert!(report.notes[0].contains("insufficient samples"));
    assert!(report.notes[0].contains(&(MIN_KEYS - 1).to_string()));
    let mut snapshot = snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800));
    snapshot.keys = MIN_KEYS;
    let report = ProbeReport::compare(&snapshot, &Limits::unbudgeted());
    assert!(report.notes.is_empty());
}

#[test]
fn test_compare_calls_a_lossy_sample_incomplete() {
    let mut snapshot = snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800));
    // `snapshot_with` reports 1832 keys, so the tolerance line is at 183.2 lost samples.
    // 200 is over it; 183 would be just under and would assert the opposite of what the
    // name says.
    snapshot.counters[Counter::ProbeLost.index()] = 200;
    let report = ProbeReport::compare(&snapshot, &Limits::unbudgeted());
    assert_eq!(report.notes.len(), 1);
    assert!(report.notes[0].contains("incomplete samples"));
    // One lost sample in a thousand is inside the tolerance the design allows.
    let mut snapshot = snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800));
    snapshot.counters[Counter::ProbeLost.index()] = 18;
    let report = ProbeReport::compare(&snapshot, &Limits::unbudgeted());
    assert!(report.notes.is_empty());
}

#[test]
fn test_probes_report_judges_what_the_probes_measured() {
    let probes = Probes::new();
    probes.decode.record(Duration::from_micros(11_000));
    let limits = Limits::from_thresholds(&[("latency_ms.decode_p99", 3_000.0)]);
    let report = probes.report(&limits);
    assert_eq!(report.verdict(), Verdict::Fail(Percentile::P99));
    assert_eq!(report.counter(Counter::ProbeLost), 0);
    // The rows are what a second snapshot of the same probes judges, so the report is
    // the comparison and not a second implementation of it. The sampling window is
    // left out: it is the one field two snapshots cannot agree on.
    assert_eq!(
        report.rows,
        ProbeReport::compare(&probes.snapshot(), &limits).rows
    );
}

#[test]
fn test_verdict_labels_and_details() {
    assert_eq!(Verdict::Pass.label(), "PASS");
    assert_eq!(Verdict::Pass.detail(), "PASS");
    assert_eq!(Verdict::Unbudgeted.label(), "UNBUDGETED");
    assert_eq!(Verdict::Unbudgeted.detail(), "-");
    assert_eq!(Verdict::Fail(Percentile::P999).label(), "FAIL");
    assert_eq!(Verdict::Fail(Percentile::P999).detail(), "FAIL(P999)");
}

#[test]
fn test_render_text_names_every_metric_and_carries_the_verdict() {
    let limits = Limits::from_thresholds(&[
        ("latency_ms.key_to_present_p99", 16_000.0),
        ("latency_ms.decode_p99", 3_000.0),
    ]);
    let mut snapshot = snapshot_with(
        Metric::KeyToPresent,
        distribution(2_100, 5_800, 11_300, 18_700),
    );
    snapshot.metrics.decode = distribution(420, 1_100, 2_400, 4_800);
    let report = ProbeReport::compare(&snapshot, &limits);
    let text = report.render_text();
    for metric in Metric::ALL {
        assert!(text.contains(metric.name()), "{}", metric.name());
    }
    assert!(text.contains("sampled 5m12s"));
    assert!(text.contains("47 sessions"));
    assert!(text.contains("1832 keys"));
    assert!(text.contains("16.00ms(P99)"));
    assert!(text.contains("verdict:"));
    // The row that is inside its budget says so, and the one that is not names the
    // percentile it missed.
    assert!(text.contains("PASS"));
    assert!(!text.contains("FAIL"));
}

#[test]
fn test_render_text_reports_a_failure_and_wraps_the_counter_list() {
    let limits = Limits::from_thresholds(&[("latency_ms.first_key_to_visible_p99", 7_000.0)]);
    let snapshot = snapshot_with(
        Metric::FirstKeyToVisible,
        distribution(3_200, 5_100, 7_400, 9_100),
    );
    let report = ProbeReport::compare(&snapshot, &limits);
    let text = report.render_text();
    assert!(text.contains("FAIL(P99)"));
    assert!(text.contains("verdict: 1 of 1 thresholds missed"));
    assert!(text.contains("first_key_to_visible P99"));
    assert!(text.contains("ui.frame.coalesced=0"));
    // Nineteen counters do not fit on one line, so the list continues under it.
    assert!(text.contains("\n          "));
    // A metric the document budgets nothing for is shown without a budget rather
    // than against a made-up one.
    assert!(text.contains("raster_partial"));
}

#[test]
fn test_render_json_is_a_document_with_every_metric_and_counter() {
    let limits = Limits::from_thresholds(&[("latency_ms.wakeup_p99", 50.0)]);
    let snapshot = snapshot_with(Metric::Wakeup, distribution(18, 31, 47, 62));
    let report = ProbeReport::compare(&snapshot, &limits);
    let json = report.render_json();
    assert!(json.starts_with('{'));
    assert!(json.ends_with("}\n"));
    assert!(json.contains("\"verdict\": \"PASS\""));
    assert!(json.contains("\"failed_percentile\": null"));
    assert!(json.contains("\"metrics\""));
    assert!(json.contains("\"counters\""));
    assert!(json.contains("\"ui.frame.coalesced\": 0"));
    assert!(json.contains("\"latency_ms.wakeup_p99\""));
    assert!(json.contains("\"limit_us\": 50.000"));
    for metric in Metric::ALL {
        assert!(json.contains(metric.name()), "{}", metric.name());
    }
    for counter in Counter::ALL {
        assert!(json.contains(counter.name()), "{}", counter.name());
    }
}

#[test]
fn test_render_json_names_the_percentile_a_failure_was_found_at() {
    let limits = Limits::from_thresholds(&[("latency_ms.decode_p99", 100.0)]);
    let snapshot = snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800));
    let report = ProbeReport::compare(&snapshot, &limits);
    let json = report.render_json();
    assert!(json.contains("\"verdict\": \"FAIL\""));
    assert!(json.contains("\"failed_percentile\": \"P99\""));
    assert!(json.contains("\"violations\": [\"decode P99"));
}

#[test]
fn test_renderings_carry_nothing_but_ascii_names_and_numbers() {
    // A report is built from durations and counters, and the only strings in it are
    // the names the contract fixes. Anything a user typed is outside the ASCII range,
    // so this is the property that says a report cannot leak one.
    let mut snapshot = snapshot_with(
        Metric::KeyToPresent,
        distribution(2_100, 5_800, 11_300, 18_700),
    );
    snapshot.counters[Counter::ProbeLost.index()] = 4;
    snapshot.keys = 3;
    let limits = Limits::from_thresholds(&[
        ("latency_ms.key_to_present_p99", 16_000.0),
        ("latency_ms.decode_p99", 3_000.0),
    ]);
    let report = ProbeReport::compare(&snapshot, &limits);
    assert!(report.render_text().is_ascii());
    assert!(report.render_json().is_ascii());
    assert_eq!(report.notes.len(), 2);
    assert!(report.render_text().contains("note: insufficient samples"));
    assert!(report.render_text().contains("note: incomplete samples"));
}

#[test]
fn test_json_string_escapes_what_the_grammar_requires() {
    assert_eq!(json_string("ui.frame.coalesced"), "\"ui.frame.coalesced\"");
    assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
    assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
    assert_eq!(json_string("a\nb"), "\"a\\nb\"");
    assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
}

#[test]
fn test_budget_bindings_are_unique_and_stated_in_latency_milliseconds() {
    let mut keys: Vec<&str> = BUDGET_BINDINGS.iter().map(|binding| binding.key).collect();
    let count = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), count, "a threshold is bound to two metrics");
    let mut pairs: Vec<(Metric, Percentile)> = BUDGET_BINDINGS
        .iter()
        .map(|binding| (binding.metric, binding.percentile))
        .collect();
    let count = pairs.len();
    pairs.sort_unstable();
    pairs.dedup();
    assert_eq!(
        pairs.len(),
        count,
        "a metric is judged twice at one percentile"
    );
    for binding in BUDGET_BINDINGS {
        // The unit the report converts from: every bound threshold is a row of the
        // document's `latency_ms` section, so the conversion is milliseconds to
        // microseconds and nothing else.
        assert!(
            binding.key.starts_with("latency_ms."),
            "{} is not a latency threshold",
            binding.key
        );
    }
    assert_eq!(Metric::ALL.len(), crate::probe::METRIC_COUNT);
}

#[test]
fn test_budget_bindings_agree_with_the_document_about_which_keys_are_pending() {
    // The binding table names thresholds the document may not carry yet, and the
    // pending list names exactly those. If the document gains one, this test says so
    // rather than letting the list quietly describe a document that no longer exists.
    let document = budget_document();
    for binding in BUDGET_BINDINGS {
        let present = document.contains(&format!("\"{}\"", leaf(binding.key)));
        let pending = PENDING_KEYS.contains(&binding.key);
        assert_eq!(
            present, !pending,
            "{}: in the document = {present}, listed as pending = {pending}",
            binding.key
        );
    }
    assert_eq!(PENDING_KEYS.len(), 2);
}

#[test]
fn test_pending_keys_are_bound_to_a_metric() {
    // A pending key that no metric is bound to would be a comment, not a threshold:
    // the document could state it and nothing would ever judge it.
    for key in PENDING_KEYS {
        assert!(
            BUDGET_BINDINGS.iter().any(|binding| binding.key == *key),
            "{key} is pending but no metric is bound to it"
        );
    }
}
