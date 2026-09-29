//! Unit tests for the histograms, the counter registry, the probes and the snapshot.
//!
//! Responsibility: pin what the design fixes about the probes -- the bucket bounds
//! and their resolution, the percentile that a rank rounds to and the order the
//! percentiles come out in, the exact diagnostic names of the counters and the metrics,
//! the size of a histogram, the no-op behaviour of a switched-off probe, the sample a
//! stamp stands for, the host path's ceiling as the budget document states it, and the
//! strictness of the snapshot parser.
//!
//! Boundaries: nothing here reads the clock for a decision or arranges an environment.
//! The tests that touch the filesystem either write into a scratch directory under the
//! workspace `target/` or read the repository's checked-in budget document, which is a
//! file and not an environment. The one place a duration is measured -- the end-to-end
//! latency token, and the stamp -- is asserted by the sample *being recorded*, never by
//! its value, so the test says the same thing on an idle machine and on a loaded one.

use std::fs;
use std::mem::size_of;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use super::*;

/// A scratch directory under the workspace `target/`.
///
/// The path comes from `CARGO_MANIFEST_DIR`, which the compiler resolves, so no test
/// reads an environment variable; the process id plus a counter keeps two runs apart.
fn scratch_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/diag-scratch")
        .join(format!("{tag}-{}-{unique}", std::process::id()));
    // A leftover from an interrupted run would otherwise be read back as this run's.
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// The 19 counter names, exactly as the boundary contract writes them.
const CONTRACT_COUNTERS: [&str; 19] = [
    "ui.frame.coalesced",
    "ui.control.dropped",
    "ui.select.timeout",
    "ui.click.debounced",
    "ui.stale-select",
    "ui.buffer.starvation",
    "ui.not-ready",
    "ui.thread.dead",
    "probe.lost",
    "decode.too-long",
    "decode.no-path",
    "dict.lookup.miss",
    "userdb.commit.slow",
    "data.readonly-mode",
    "config.invalid",
    "ui.theme.blur-unavailable",
    "platform.cursor.unresolved",
    "platform.x11.no-compositor",
    "platform.x11.no-argb-visual",
];

/// The eight metric names, in the order a report lists them.
const CONTRACT_METRICS: [&str; 8] = [
    "key_to_present",
    "decode",
    "raster_full",
    "raster_partial",
    "first_key_to_visible",
    "wakeup",
    "event_loop_key",
    "post_ui",
];

#[test]
fn test_histogram_new_records_nothing() {
    let histogram = Histogram::new();
    assert!(histogram.is_enabled());
    assert_eq!(histogram.count(), 0);
    assert_eq!(histogram.snapshot(), HistSnapshot::default());
}

#[test]
fn test_histogram_record_counts_sums_and_places_each_sample() {
    let histogram = Histogram::new();
    for _ in 0..10 {
        histogram.record(Duration::from_micros(10));
    }
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 10);
    assert_eq!(snapshot.sum_us, 100);
    assert_eq!(snapshot.mean_us(), 10.0);
    // 10µs falls in the bucket whose upper bound is 13µs, so every percentile of a
    // single-valued distribution is that bound and never below the sample.
    assert_eq!(snapshot.p50_us, 13);
    assert_eq!(snapshot.p90_us, 13);
    assert_eq!(snapshot.p99_us, 13);
    assert_eq!(snapshot.p999_us, 13);
    assert_eq!(snapshot.max_us, 13);
}

#[test]
fn test_histogram_percentile_rounds_up_to_the_next_sample() {
    let histogram = Histogram::new();
    for _ in 0..99 {
        histogram.record(Duration::from_micros(10));
    }
    histogram.record(Duration::from_micros(5_000));
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 100);
    // P999 of a hundred samples is the hundredth sample, which is the slow one: the
    // rank rounds up rather than down, so the tail is not hidden by the rounding.
    assert_eq!(snapshot.p99_us, 13);
    assert_eq!(snapshot.p999_us, 6_765);
    assert_eq!(snapshot.max_us, 6_765);
}

#[test]
fn test_histogram_percentiles_never_decrease() {
    // One distribution spread across the table: the percentiles of a histogram have to
    // come out in the order of the percentiles themselves, or a report would show a tail
    // below its own median.
    let histogram = Histogram::new();
    for micros in [1_u64, 4, 9, 16, 25, 100, 1_000, 5_000, 20_000, 90_000] {
        for _ in 0..10 {
            histogram.record(Duration::from_micros(micros));
        }
    }
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 100);
    let stated = [
        snapshot.p50_us,
        snapshot.p90_us,
        snapshot.p99_us,
        snapshot.p999_us,
        snapshot.max_us,
    ];
    for window in stated.windows(2) {
        assert!(window[0] <= window[1], "{stated:?} must not decrease");
    }
    // Every one of them is the upper bound of a bucket, so none of them understates a
    // sample: the slowest sample of all is inside the highest non-empty bucket.
    assert!(snapshot.p50_us >= 1);
    assert!(snapshot.max_us >= 90_000);
}

#[test]
fn test_histogram_snapshot_of_an_empty_histogram_is_all_zero() {
    let histogram = Histogram::new();
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 0);
    assert_eq!(snapshot.mean_us(), 0.0);
    for percentile in Percentile::ALL {
        assert_eq!(snapshot.percentile(percentile), 0);
    }
    assert_eq!(snapshot.max_us, 0);
}

#[test]
fn test_histogram_sample_past_the_ceiling_lands_in_the_last_bucket() {
    let histogram = Histogram::new();
    histogram.record(Duration::from_millis(200));
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 1);
    assert_eq!(snapshot.p50_us, TOP_US);
    assert_eq!(snapshot.max_us, TOP_US);
}

#[test]
fn test_histogram_sub_microsecond_sample_lands_in_the_first_bucket() {
    let histogram = Histogram::new();
    histogram.record(Duration::from_nanos(400));
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count, 1);
    assert_eq!(snapshot.p50_us, 1);
    assert_eq!(snapshot.sum_us, 0);
}

#[test]
fn test_histogram_disabled_records_nothing() {
    let histogram = Histogram::new();
    histogram.set_enabled(false);
    assert!(!histogram.is_enabled());
    histogram.record(Duration::from_micros(500));
    assert_eq!(histogram.count(), 0);
    assert_eq!(histogram.snapshot(), HistSnapshot::default());
}

#[test]
fn test_histogram_size_matches_the_design_footprint() {
    // 64 buckets and 3 counters, with the enable flag in the tail padding: the design
    // budgets 536 bytes for a histogram, and a probe that grew past it would be
    // spending memory on measurement.
    assert_eq!(size_of::<Histogram>(), 536);
}

#[test]
fn test_probes_state_is_inline_and_bounded() {
    // What keeps a sample off the heap is that the probes' whole state is inline: every
    // field is an atomic or a clock reading, and the buckets are a fixed array. The
    // number of allocations one `record` call performs cannot be counted from this
    // crate -- an allocator hook is `unsafe`, which `unsafe_code = "deny"` refuses -- so
    // this pins the structure and the delivery report records the gap.
    assert!(
        size_of::<Probes>() <= 8 * 1024,
        "the probes' own state is {} bytes",
        size_of::<Probes>()
    );
    // The state does not grow with the number of samples, either: a thousand samples
    // land in the same fixed table, which is what lets the probes run for hours.
    let probes = Probes::new();
    for _ in 0..1_000 {
        let _sample = probes.begin(Metric::PostUi);
    }
    assert_eq!(probes.post_ui.count(), 1_000);
}

#[test]
fn test_bounds_start_at_one_microsecond_and_saturate_at_the_ceiling() {
    let bounds = super::histogram::BOUNDS;
    assert_eq!(bounds.len(), BUCKETS);
    assert_eq!(bounds[0], 1);
    assert_eq!(bounds[BUCKETS - 1], TOP_US);
    // The listed prefix of the sequence the design fixes: the resolution is a
    // microsecond up to 13µs, which is where an input method's latencies live.
    assert_eq!(&bounds[..6], &[1, 2, 3, 5, 8, 13]);
    for window in bounds.windows(2) {
        assert!(window[0] <= window[1], "bounds must not decrease");
    }
}

#[test]
fn test_counter_names_match_the_contract() {
    assert_eq!(Counter::ALL.len(), CONTRACT_COUNTERS.len());
    for (counter, name) in Counter::ALL.into_iter().zip(CONTRACT_COUNTERS) {
        assert_eq!(counter.name(), name);
    }
}

#[test]
fn test_counter_from_name_round_trips_every_name_and_refuses_others() {
    for counter in Counter::ALL {
        assert_eq!(Counter::from_name(counter.name()), Some(counter));
        assert_eq!(counter.index(), counter as usize);
    }
    assert_eq!(Counter::from_name("ui.frame.merged"), None);
    assert_eq!(Counter::from_name(""), None);
}

#[test]
fn test_counter_count_matches_the_variant_list() {
    assert_eq!(Counter::ALL.len(), COUNTER_COUNT);
    assert_eq!(COUNTER_COUNT, CONTRACT_COUNTERS.len());
}

#[test]
fn test_metric_names_match_the_contract() {
    assert_eq!(Metric::ALL.len(), CONTRACT_METRICS.len());
    for (metric, name) in Metric::ALL.into_iter().zip(CONTRACT_METRICS) {
        assert_eq!(metric.name(), name);
    }
    assert_eq!(Metric::ALL.len(), METRIC_COUNT);
}

#[test]
fn test_metric_from_name_round_trips_every_name_and_refuses_others() {
    for metric in Metric::ALL {
        assert_eq!(Metric::from_name(metric.name()), Some(metric));
        assert_eq!(metric.index(), metric as usize);
    }
    assert_eq!(Metric::from_name("key_to_paint"), None);
    assert_eq!(Metric::from_name(""), None);
}

#[test]
fn test_metric_histogram_reaches_the_matching_field() {
    let probes = Probes::new();
    probes.decode.record(Duration::from_micros(250));
    assert_eq!(Metric::Decode.histogram(&probes).count(), 1);
    for metric in Metric::ALL {
        if metric != Metric::Decode {
            assert_eq!(metric.histogram(&probes).count(), 0, "{}", metric.name());
        }
    }
    let snapshot = probes.snapshot();
    assert_eq!(Metric::Decode.snapshot(&snapshot.metrics).count, 1);
    assert_eq!(Metric::Wakeup.snapshot(&snapshot.metrics).count, 0);
}

#[test]
fn test_metric_unit_prints_microseconds_for_the_wakeup_budget() {
    assert_eq!(Metric::Wakeup.unit(), Unit::Micros);
    assert_eq!(Metric::KeyToPresent.unit(), Unit::Millis);
    assert_eq!(Unit::Micros.format(47.0), "47us");
    assert_eq!(Unit::Millis.format(1_130.0), "1.13ms");
    assert_eq!(Unit::Micros.suffix(), "us");
    assert_eq!(Unit::Millis.suffix(), "ms");
}

#[test]
fn test_metric_post_ui_records_into_its_own_histogram_in_microseconds() {
    // The last segment of the host path: what a post cost the host thread, stated in
    // microseconds because a post is a hand-off and a write and would read as `0.00ms`.
    let probes = Probes::new();
    probes.post_ui.record(Duration::from_micros(120));
    let snapshot = probes.snapshot();
    assert_eq!(snapshot.metrics.post_ui.count, 1);
    assert_eq!(snapshot.metrics.post_ui.p50_us, 144);
    assert_eq!(Metric::PostUi.unit(), Unit::Micros);
    assert_eq!(Metric::PostUi.name(), "post_ui");
    for metric in Metric::ALL {
        if metric != Metric::PostUi {
            assert_eq!(
                metric.snapshot(&snapshot.metrics).count,
                0,
                "{}",
                metric.name()
            );
        }
    }
}

#[test]
fn test_metric_post_ui_zero_length_sample_lands_in_the_first_bucket() {
    // A post that returned before the clock moved is still a sample: it is counted, and
    // it is counted at the table's floor rather than before the table starts.
    let probes = Probes::new();
    probes.post_ui.record(Duration::ZERO);
    let snapshot = probes.snapshot().metrics.post_ui;
    assert_eq!(snapshot.count, 1);
    assert_eq!(snapshot.p50_us, 1);
    assert_eq!(snapshot.max_us, 1);
    assert_eq!(snapshot.sum_us, 0);
}

#[test]
fn test_probes_bump_counts_each_counter_separately() {
    let probes = Probes::new();
    probes.bump(Counter::FrameCoalesced);
    probes.bump(Counter::FrameCoalesced);
    probes.bump(Counter::ProbeLost);
    assert_eq!(probes.counter(Counter::FrameCoalesced), 2);
    assert_eq!(probes.counter(Counter::ProbeLost), 1);
    assert_eq!(probes.counter(Counter::SelectTimeout), 0);
}

#[test]
fn test_probes_disabled_records_nothing_at_all() {
    let probes = Probes::new();
    probes.set_enabled(false);
    assert!(!probes.is_enabled());
    probes.bump(Counter::DataReadonlyMode);
    probes.session_started();
    let token = probes.begin_key_to_present();
    probes.end_key_to_present(token);
    probes.key_to_present.record(Duration::from_micros(900));
    let snapshot = probes.snapshot();
    assert_eq!(snapshot.keys, 0);
    assert_eq!(snapshot.sessions, 0);
    assert_eq!(snapshot.counter(Counter::DataReadonlyMode), 0);
    assert_eq!(snapshot.metrics.key_to_present.count, 0);
    assert!(token.start().is_none());
    assert_eq!(token.seq(), 0);
}

#[test]
fn test_probes_enabled_again_records_after_being_switched_off() {
    let probes = Probes::new();
    probes.set_enabled(false);
    probes.decode.record(Duration::from_micros(10));
    probes.set_enabled(true);
    probes.decode.record(Duration::from_micros(10));
    assert_eq!(probes.decode.count(), 1);
}

#[test]
fn test_probes_begin_and_end_record_one_sample_and_count_the_key() {
    let probes = Probes::new();
    let first = probes.begin_key_to_present();
    let second = probes.begin_key_to_present();
    assert_eq!(first.seq(), 0);
    assert_eq!(second.seq(), 1);
    assert!(first.start().is_some());
    probes.end_key_to_present(first);
    probes.end_key_to_present(second);
    let snapshot = probes.snapshot();
    assert_eq!(snapshot.keys, 2);
    assert_eq!(snapshot.metrics.key_to_present.count, 2);
}

#[test]
fn test_probes_end_key_to_present_drops_a_sample_whose_receipt_arrives_switched_off() {
    let probes = Probes::new();
    let token = probes.begin_key_to_present();
    probes.set_enabled(false);
    probes.end_key_to_present(token);
    assert_eq!(probes.key_to_present.count(), 0);
}

#[test]
fn test_stamp_records_one_sample_of_the_metric_it_names() {
    let probes = Probes::new();
    {
        let _decode = probes.begin(Metric::Decode);
    }
    {
        let _key = probes.begin(Metric::EventLoopKey);
    }
    assert_eq!(probes.decode.count(), 1);
    assert_eq!(probes.event_loop_key.count(), 1);
    for metric in Metric::ALL {
        if metric != Metric::Decode && metric != Metric::EventLoopKey {
            assert_eq!(metric.histogram(&probes).count(), 0, "{}", metric.name());
        }
    }
}

#[test]
fn test_stamp_records_nothing_when_the_probes_are_switched_off() {
    let probes = Probes::new();
    probes.set_enabled(false);
    {
        let _sample = probes.begin(Metric::PostUi);
    }
    assert_eq!(probes.post_ui.count(), 0);
    assert_eq!(probes.snapshot().metrics.post_ui, HistSnapshot::default());
}

#[test]
fn test_stamp_whose_scope_spans_the_switch_records_nothing() {
    // A sample that spans the switch is dropped rather than half-counted, the way an
    // end-to-end sample whose receipt arrives after the switch is.
    let probes = Probes::new();
    let sample = probes.begin(Metric::PostUi);
    probes.set_enabled(false);
    drop(sample);
    assert_eq!(probes.post_ui.count(), 0);
    // Switching back on leaves the dropped sample dropped, and the next one recorded.
    probes.set_enabled(true);
    assert_eq!(probes.post_ui.count(), 0);
    {
        let _sample = probes.begin(Metric::PostUi);
    }
    assert_eq!(probes.post_ui.count(), 1);
}

#[test]
fn test_probes_snapshot_carries_every_metric_and_counter() {
    let probes = Probes::new();
    probes.session_started();
    let snapshot = probes.snapshot();
    assert_eq!(snapshot.sessions, 1);
    assert_eq!(snapshot.counters.len(), COUNTER_COUNT);
    for metric in Metric::ALL {
        assert_eq!(
            metric.snapshot(&snapshot.metrics).count,
            0,
            "{}",
            metric.name()
        );
    }
    for counter in Counter::ALL {
        assert_eq!(snapshot.counter(counter), 0);
    }
}

#[test]
fn test_snapshot_text_round_trips_through_the_parser() {
    let mut metrics = Metrics::default();
    for (index, metric) in Metric::ALL.into_iter().enumerate() {
        let sample = metric.snapshot_mut(&mut metrics);
        sample.count = 10 + index as u64;
        sample.sum_us = 1_000 + index as u64;
        sample.p50_us = 2_100;
        sample.p90_us = 5_800;
        sample.p99_us = 11_300;
        sample.p999_us = 18_700;
        sample.max_us = 25_000;
    }
    let mut counters = [0_u64; COUNTER_COUNT];
    for (index, counter) in Counter::ALL.into_iter().enumerate() {
        counters[counter.index()] = index as u64 * 3;
    }
    let snapshot = ProbeSnapshot {
        sampled: Duration::from_micros(312_400_000),
        sessions: 47,
        keys: 1_832,
        metrics,
        counters,
    };
    let text = snapshot.to_text();
    assert!(text.starts_with(SNAPSHOT_HEADER));
    assert_eq!(text.lines().filter(|line| line.is_empty()).count(), 0);
    let parsed = ProbeSnapshot::parse(&text).expect("the writer's own text parses");
    assert_eq!(parsed, snapshot);
}

#[test]
fn test_snapshot_parse_reads_a_hand_written_snapshot() {
    let text = format!(
        "{SNAPSHOT_HEADER}\nsampled_us=1000000\nsessions=2\nkeys=3\n\
         metric.decode.count=3\nmetric.decode.sum_us=900\nmetric.decode.p50_us=13\n\
         metric.decode.p90_us=13\nmetric.decode.p99_us=13\nmetric.decode.p999_us=13\n\
         metric.decode.max_us=13\ncounter.probe.lost=1\n"
    );
    let parsed = ProbeSnapshot::parse(&text).expect("a complete record parses");
    assert_eq!(parsed.sampled, Duration::from_secs(1));
    assert_eq!(parsed.sessions, 2);
    assert_eq!(parsed.keys, 3);
    assert_eq!(parsed.metrics.decode.count, 3);
    assert_eq!(parsed.metrics.decode.sum_us, 900);
    assert_eq!(parsed.counter(Counter::ProbeLost), 1);
    // A metric the text did not carry stays zero rather than being guessed at.
    assert_eq!(parsed.metrics.raster_full.count, 0);
    assert_eq!(parsed.metrics.wakeup.count, 0);
}

#[test]
fn test_snapshot_parse_refuses_a_text_that_does_not_open_with_the_header() {
    let error = ProbeSnapshot::parse("sampled_us=1\n").expect_err("no header");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("expected"));
    let error = ProbeSnapshot::parse("").expect_err("empty text has no header");
    assert!(error.to_string().contains("empty snapshot"));
}

#[test]
fn test_snapshot_parse_refuses_an_unknown_key() {
    let text = format!("{SNAPSHOT_HEADER}\nsampled_ms=1\n");
    let error = ProbeSnapshot::parse(&text).expect_err("`sampled_ms` is not a field");
    assert!(error.to_string().contains("sampled_ms"));
}

#[test]
fn test_snapshot_parse_refuses_a_repeated_key() {
    let text = format!("{SNAPSHOT_HEADER}\nkeys=1\nkeys=2\n");
    let error = ProbeSnapshot::parse(&text).expect_err("a key written twice");
    assert!(error.to_string().contains("written twice"));
    assert!(error.to_string().contains("line 3"));
}

#[test]
fn test_snapshot_parse_refuses_an_unknown_metric_field() {
    let text = format!("{SNAPSHOT_HEADER}\nmetric.decode.median_us=1\n");
    let error = ProbeSnapshot::parse(&text).expect_err("`median_us` is not a field");
    assert!(error.to_string().contains("median_us"));
    assert!(error.to_string().contains("p50_us"));
}

#[test]
fn test_snapshot_parse_refuses_an_unknown_metric_and_counter() {
    let text = format!("{SNAPSHOT_HEADER}\nmetric.decode_typing.count=1\n");
    let error = ProbeSnapshot::parse(&text).expect_err("not a metric");
    assert!(error.to_string().contains("is not a metric"));
    let text = format!("{SNAPSHOT_HEADER}\ncounter.ui.frame.merged=1\n");
    let error = ProbeSnapshot::parse(&text).expect_err("not a counter");
    assert!(error.to_string().contains("is not a counter"));
}

#[test]
fn test_snapshot_parse_refuses_a_line_that_is_not_a_record() {
    let text = format!("{SNAPSHOT_HEADER}\nkeys\n");
    let error = ProbeSnapshot::parse(&text).expect_err("no `=`");
    assert!(error.to_string().contains("key=value"));
}

#[test]
fn test_snapshot_parse_refuses_a_value_that_is_not_a_whole_number() {
    let text = format!("{SNAPSHOT_HEADER}\nkeys=17.5\n");
    let error = ProbeSnapshot::parse(&text).expect_err("not a whole number");
    assert!(error.to_string().contains("17.5"));
    let text = format!("{SNAPSHOT_HEADER}\nkeys=-1\n");
    assert!(ProbeSnapshot::parse(&text).is_err());
}

#[test]
fn test_snapshot_write_to_creates_a_private_file_that_reads_back() {
    let dir = scratch_dir("snapshot");
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    let path = dir.join("probe.txt");
    let snapshot = ProbeSnapshot {
        sampled: Duration::from_secs(1),
        sessions: 1,
        keys: 2,
        metrics: Metrics::default(),
        counters: [0_u64; COUNTER_COUNT],
    };
    snapshot.write_to(&path).expect("the snapshot is writable");
    let mode = fs::metadata(&path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "a snapshot is not readable by another account");
    let text = fs::read_to_string(&path).expect("the file is readable");
    assert_eq!(ProbeSnapshot::parse(&text).expect("it parses"), snapshot);
    // A shorter snapshot written over a longer one does not leave the longer one's
    // tail behind, which is what a reader would then fail to parse.
    ProbeSnapshot::default()
        .write_to(&path)
        .expect("the second write succeeds");
    let text = fs::read_to_string(&path).expect("the file is readable");
    assert_eq!(
        ProbeSnapshot::parse(&text).expect("it parses"),
        ProbeSnapshot::default()
    );
}

/// The repository's budget document, as text.
///
/// Read rather than restated: a threshold is written down in exactly one place, and this
/// is that place. The file is checked in, so reading it depends on no environment.
fn budget_document() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/dev/budgets.json");
    fs::read_to_string(&path).expect("the budget document is readable")
}

/// One row of the budget document's latency section, in microseconds.
///
/// The document states that section in milliseconds, which its own section name says, and
/// a histogram records microseconds. The two units meet here, in the test that reads the
/// file -- never in the probe, which states no threshold at all.
fn latency_row_us(document: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\":");
    let start = document
        .find(&needle)
        .expect("the budget document states the row")
        + needle.len();
    let after = &document[start..];
    let end = after.find([',', '\n']).unwrap_or(after.len());
    let millis: f64 = after[..end]
        .trim()
        .parse()
        .expect("a threshold is a number");
    (millis * 1_000.0) as u64
}

#[test]
fn test_host_path_total_is_asserted_against_the_budget_documents_rows() {
    // The ceiling the host path is measured against is the one the document states for a
    // key press reaching the candidate frame -- the pair of rows `BUDGET-LAT-01` is
    // written as. The numbers are read out of the file, so a threshold that moved there
    // moves here too, and one that was removed fails this test instead of quietly
    // un-asserting the path.
    //
    // The metric is the host thread's whole journey through a key press, which the
    // performance work calls `key_to_post` and the specification calls `event_loop_key`:
    // one span, sampled at the ABI boundary and completed once the post has returned.
    let document = budget_document();
    let p50_us = latency_row_us(&document, "key_to_present_p50");
    let p99_us = latency_row_us(&document, "key_to_present_p99");
    assert!(p50_us > 0, "the document states a median");
    assert!(p99_us >= p50_us, "the tail is not below the median");
    // A ceiling above the histogram's top bucket could not be asserted at all: every
    // sample slower than the top bucket is counted in it.
    assert!(p99_us < TOP_US, "the ceiling has to be inside the table");

    let probes = Probes::new();
    // A key whose host-side journey took a quarter of the median budget is inside both
    // rows. The sample goes through the histogram a real key is recorded into.
    probes
        .event_loop_key
        .record(Duration::from_micros(p50_us / 4));
    let sample = *Metric::EventLoopKey.snapshot(&probes.snapshot().metrics);
    assert!(sample.percentile(Percentile::P50) <= p50_us);
    assert!(sample.percentile(Percentile::P99) <= p99_us);

    // One that spent twice the whole tail budget on the host thread is not, which is what
    // shows the comparison reads the document's number rather than a constant of its own.
    probes
        .event_loop_key
        .record(Duration::from_micros(p99_us * 2));
    let sample = *Metric::EventLoopKey.snapshot(&probes.snapshot().metrics);
    assert!(sample.percentile(Percentile::P99) > p99_us);
    assert!(sample.percentile(Percentile::P50) <= p50_us);
}
