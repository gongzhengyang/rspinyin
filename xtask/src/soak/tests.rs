//! Tests for the soak tool.
//!
//! Everything here runs without a display server, a Fcitx5 session or eight hours: the
//! schedule, the script, the report and the verdict are all arithmetic and parsing over
//! data these tests write out by hand. What they cannot cover is the live loop in
//! [`super::driver`], whose correctness is a claim about a session rather than about a
//! number, and the two cases that need one are marked in the module documentation of the
//! tool rather than pretended at here.
//!
//! The three observations a report carries besides the memory series -- the pass counts, the
//! crash records and the host readings -- are tested in [`super::verdict_tests`], which
//! drives the fixtures below.

use std::fs;
use std::path::{Path, PathBuf};

use super::judge;
use super::plan::{MIN_STEADY_SAMPLES, PlanError, SECONDS_PER_HOUR};
use super::report::{
    CrashCount, CycleReport, Host, REPORT_VERSION, Sample, SoakReport, StopReason, Target,
};
use super::script::{self, Action};
use super::stats::RssStats;
use super::{Plan, PlanReport, SoakError, Step};
use crate::budget::{self, Budgets};
use crate::testd::keys::{KS_BACKSPACE, KS_ESCAPE, KS_SPACE};

/// The typing rate the soak defaults to, in keys per second.
const RATE_HZ: f64 = 10.0;

/// The sampling interval the soak defaults to, in seconds.
const INTERVAL_S: f64 = 10.0;

/// The warm-up the soak defaults to, in seconds.
const WARMUP_S: f64 = 60.0;

/// The eight-hour plan the passing reports below follow.
pub(super) fn eight_hour_plan() -> Plan {
    Plan::new(8.0, RATE_HZ, INTERVAL_S, WARMUP_S).expect("the plan the tool defaults to")
}

/// A plan of `hours`, at the tool's default rate, interval and warm-up.
fn plan_of(hours: f64) -> Plan {
    Plan::new(hours, RATE_HZ, INTERVAL_S, WARMUP_S).expect("a plan the tool accepts")
}

/// The budget document the repository ships.
pub(super) fn budgets() -> Budgets {
    budget::read_budgets(&budget::repo_root().expect("the repository root resolves"))
        .expect("the budget document is valid")
}

/// A directory under the target tree that a test may write into.
pub(super) fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/xtask-soak-scratch")
        .join(name);
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    dir
}

/// A series of readings ten seconds apart, starting at zero.
///
/// The closure is given the reading's index and answers its resident set size; the
/// anonymous counter follows it, so a case that is about the resident set does not also
/// have to say what the mappings were doing.
pub(super) fn series(readings: u64, rss_kb: impl Fn(u64) -> u64) -> Vec<Sample> {
    (0..readings)
        .map(|index| Sample {
            t_s: index as f64 * INTERVAL_S,
            rss_kb: Some(rss_kb(index)),
            anonymous_kb: Some(rss_kb(index)),
        })
        .collect()
}

/// Strokes one pass over the driver's keystroke cycle holds.
///
/// Taken from the script rather than written down, so that a test asserting a pass count is
/// asserting it against the cycle the driver really repeats.
pub(super) fn cycle_strokes() -> u64 {
    let strokes = script::compiled().expect("every character of the cycle is on the layout");
    u64::try_from(strokes.len()).expect("a cycle shorter than u64::MAX strokes")
}

/// A report of a run that followed `plan` and delivered `strokes` of it.
///
/// The cycle counts are derived from those two numbers rather than passed in, so a report
/// built here is the one the driver would have written; a test about a disagreement between
/// the copies edits the field afterwards.
fn report_of(plan: &Plan, strokes: u64, samples: Vec<Sample>) -> SoakReport {
    SoakReport {
        report_version: REPORT_VERSION,
        plan: PlanReport::of(plan),
        target: Target {
            pid: 4242,
            executable: "/usr/bin/fcitx5".to_owned(),
            display: ":99".to_owned(),
            window: 0x40_0001,
        },
        host: Host {
            cpus: 8,
            load1_at_start: Some(0.4),
            load1_at_end: Some(0.5),
        },
        started_at_unix_s: 1_790_899_200,
        elapsed_s: plan.duration_s(),
        delivered_strokes: strokes,
        cycles: CycleReport::of(plan, cycle_strokes(), strokes),
        crashes: Some(CrashCount {
            directory: "/home/tester/.local/share/rspinyin/crash".to_owned(),
            before: Some(0),
            after: Some(0),
        }),
        stopped_early: None,
        samples,
        notes: Vec::new(),
    }
}

/// A report of a run that followed `plan` and delivered everything it planned.
pub(super) fn report(plan: &Plan, samples: Vec<Sample>) -> SoakReport {
    report_of(plan, plan.planned_strokes(), samples)
}

// ── The schedule ────────────────────────────────────────────────────────────────

#[test]
fn test_plan_new_derives_the_counts_a_report_is_measured_against() {
    let plan = eight_hour_plan();
    assert_eq!(plan.duration_s(), 28_800.0);
    assert_eq!(plan.planned_strokes(), 288_000);
    assert_eq!(plan.planned_samples(), 2_881);
    assert_eq!(plan.steady_from(), 6);
    assert_eq!(plan.steady_samples(), 2_875);
    assert_eq!(plan.key_period().as_millis(), 100);
    assert_eq!(plan.sample_interval().as_secs(), 10);
    assert_eq!(plan.duration(), std::time::Duration::from_secs(28_800));
}

#[test]
fn test_plan_new_refuses_a_number_no_schedule_can_be_built_from() {
    assert!(matches!(
        Plan::new(0.0, RATE_HZ, INTERVAL_S, WARMUP_S),
        Err(PlanError::NotPositive {
            field: "--hours",
            ..
        })
    ));
    assert!(matches!(
        Plan::new(8.0, -1.0, INTERVAL_S, WARMUP_S),
        Err(PlanError::NotPositive {
            field: "--rate-hz",
            ..
        })
    ));
    assert!(matches!(
        Plan::new(8.0, RATE_HZ, f64::NAN, WARMUP_S),
        Err(PlanError::NotPositive {
            field: "--interval-s",
            ..
        })
    ));
    assert!(matches!(
        Plan::new(f64::INFINITY, RATE_HZ, INTERVAL_S, WARMUP_S),
        Err(PlanError::NotPositive {
            field: "--hours",
            ..
        })
    ));
    // A finite number of hours whose product with the seconds in an hour is beyond what a
    // duration can hold: refused where it is derived rather than at the conversion.
    assert!(matches!(
        Plan::new(1e18, RATE_HZ, INTERVAL_S, WARMUP_S),
        Err(PlanError::TooLong {
            field: "--hours",
            ..
        })
    ));
}

#[test]
fn test_plan_new_refuses_a_run_that_would_type_nothing() {
    // A hundredth of a second at one key a minute comes to no whole stroke, and a run that
    // types nothing would report a perfect pass rate over an empty schedule.
    let refused = Plan::new(0.000_01, 0.016, INTERVAL_S, WARMUP_S);
    assert!(
        matches!(refused, Err(PlanError::NoStrokes { .. })),
        "{refused:?}"
    );
}

#[test]
fn test_plan_new_refuses_a_warmup_that_leaves_no_steady_window() {
    // A warm-up as long as the run leaves one sample outside it, and one that ends five
    // minutes early leaves sixty-one.
    let refused = Plan::new(1.0, RATE_HZ, INTERVAL_S, 3_600.0);
    assert!(
        matches!(
            refused,
            Err(PlanError::WarmupTooLong {
                steady_samples: 1,
                min: MIN_STEADY_SAMPLES,
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(Plan::new(1.0, RATE_HZ, INTERVAL_S, 3_000.0).is_ok());
}

#[test]
fn test_plan_due_merges_the_two_schedules_and_gives_a_tie_to_the_sample() {
    // One second, ten strokes, a sample every half second: the two schedules meet at zero
    // and again at half a second, and the sample is answered first both times.
    let plan = Plan::new(1.0 / SECONDS_PER_HOUR, RATE_HZ, 0.5, 0.1).expect("a one second plan");
    assert_eq!(plan.planned_strokes(), 10);
    assert_eq!(plan.planned_samples(), 3);

    assert_eq!(
        plan.due(0, 0),
        Some((Step::Sample, std::time::Duration::ZERO))
    );
    assert_eq!(
        plan.due(0, 1),
        Some((Step::Stroke, std::time::Duration::ZERO))
    );
    assert_eq!(
        plan.due(4, 1),
        Some((Step::Stroke, std::time::Duration::from_millis(400)))
    );
    assert_eq!(
        plan.due(5, 1),
        Some((Step::Sample, std::time::Duration::from_millis(500)))
    );
    assert_eq!(
        plan.due(5, 2),
        Some((Step::Stroke, std::time::Duration::from_millis(500)))
    );
    assert_eq!(
        plan.due(9, 2),
        Some((Step::Stroke, std::time::Duration::from_millis(900)))
    );
    assert_eq!(
        plan.due(10, 2),
        Some((Step::Sample, std::time::Duration::from_millis(1_000)))
    );
    assert_eq!(plan.due(10, 3), None);
}

#[test]
fn test_plan_due_walks_a_whole_plan_and_lands_on_the_planned_counts() {
    // The counts a pass rate is a fraction of are the counts the schedule actually
    // produces: a plan whose loop took a different number of steps would make the
    // denominator a fiction.
    let plan = plan_of(0.05);
    let (mut strokes, mut samples) = (0_u64, 0_u64);
    let mut last = std::time::Duration::ZERO;
    while let Some((step, at)) = plan.due(strokes, samples) {
        assert!(at >= last, "the schedule never goes backwards");
        last = at;
        match step {
            Step::Stroke => strokes += 1,
            Step::Sample => samples += 1,
        }
    }
    assert_eq!(strokes, plan.planned_strokes());
    assert_eq!(samples, plan.planned_samples());
}

// ── The keystroke cycle ─────────────────────────────────────────────────────────

#[test]
fn test_script_compiled_expands_every_action_of_the_cycle() {
    let strokes = script::compiled().expect("every character of the cycle is on the layout");
    let expected: usize = script::CYCLE
        .iter()
        .map(|action| {
            script::strokes_of(*action)
                .expect("every action of the cycle expands")
                .len()
        })
        .sum();
    assert_eq!(strokes.len(), expected);
    assert!(!strokes.is_empty(), "a run with no stroke measures nothing");
    assert!(strokes.contains(&script::strokes_of(Action::Commit).expect("space")[0]));
    assert!(strokes.contains(&script::strokes_of(Action::Erase).expect("backspace")[0]));
    assert!(strokes.contains(&script::strokes_of(Action::Cancel).expect("escape")[0]));
}

#[test]
fn test_script_strokes_of_names_the_key_each_action_is_bound_to() {
    let first = |action| script::strokes_of(action).expect("the action expands")[0];
    assert_eq!(first(Action::Commit).keysym, KS_SPACE);
    assert_eq!(first(Action::Erase).keysym, KS_BACKSPACE);
    assert_eq!(first(Action::Cancel).keysym, KS_ESCAPE);
    assert_eq!(first(Action::Select('2')).keysym, u32::from(b'2'));
    // The toggle is the activation key with the control modifier held, which is what the
    // shipped configuration binds it to.
    let toggle = first(Action::Toggle);
    assert_eq!(toggle.keysym, KS_SPACE);
    assert_eq!(toggle.state, crate::testd::keys::CONTROL_MASK);
}

#[test]
fn test_script_strokes_of_refuses_a_character_the_layout_lacks() {
    assert!(script::strokes_of(Action::Compose("你好")).is_err());
    assert!(script::strokes_of(Action::Select('你')).is_err());
    assert!(script::strokes_of(Action::Compose("nihao")).is_ok());
}

#[test]
fn test_script_cycle_leaves_the_input_method_where_it_found_it() {
    // The activation toggle appears an even number of times, so a cycle cannot walk the
    // session into a disabled input method and leave it there for the rest of the run.
    let toggles = script::CYCLE
        .iter()
        .filter(|action| **action == Action::Toggle)
        .count();
    assert_eq!(toggles % 2, 0, "the cycle toggles {toggles} times");
}

// ── The report ──────────────────────────────────────────────────────────────────

#[test]
fn test_report_round_trips_through_the_file_it_writes() {
    let path = scratch("round-trip").join("soak.json");
    let written = report(&eight_hour_plan(), series(10, |index| 40_000 + index * 512));
    written.write(&path).expect("the report is writable");
    let read = SoakReport::read(&path).expect("the report is readable");
    assert_eq!(read, written);
    let text = fs::read_to_string(&path).expect("the file is readable");
    assert!(text.contains("\"report_version\": 2"), "{text}");
    assert!(
        text.contains("\n  \"samples\""),
        "the document is written pretty-printed so a person can read the series"
    );
    assert!(
        text.contains("\"rss_kb\""),
        "every reading of the series is kept, not only its last"
    );
}

#[test]
fn test_report_read_refuses_a_document_it_cannot_judge() {
    let dir = scratch("unusable");
    let missing = SoakReport::read(&dir.join("absent.json"));
    assert!(missing.is_err());

    let not_json = dir.join("not-json.json");
    fs::write(&not_json, b"this is not a report").expect("the file is writable");
    assert!(SoakReport::read(&not_json).is_err());

    let newer = dir.join("newer.json");
    let text = serde_json::to_string(&report(&eight_hour_plan(), series(4, |_| 40_000)))
        .expect("the report renders");
    fs::write(
        &newer,
        text.replace("\"report_version\":2", "\"report_version\":3"),
    )
    .expect("the file is writable");
    let refused = SoakReport::read(&newer);
    assert!(refused.is_err(), "a newer schema is not read as this one");
}

#[test]
fn test_plan_report_plan_refuses_a_document_whose_counts_disagree_with_itself() {
    let plan = eight_hour_plan();
    let stated = PlanReport::of(&plan);
    assert_eq!(
        stated.plan().expect("the plan is consistent").duration(),
        plan.duration()
    );
    let tampered = PlanReport {
        planned_strokes: stated.planned_strokes + 1,
        ..stated
    };
    assert!(
        tampered.plan().is_err(),
        "a report that cannot say what it planned cannot say what fraction of it was done"
    );
}

#[test]
fn test_report_pass_rate_is_a_fraction_of_the_plan_not_of_what_the_run_reached() {
    let plan = eight_hour_plan();
    let complete = report(&plan, series(10, |_| 40_000));
    assert_eq!(complete.pass_rate_pct(), Some(100.0));
    let tenth = SoakReport {
        delivered_strokes: plan.planned_strokes() / 10,
        ..complete
    };
    assert_eq!(tenth.pass_rate_pct(), Some(10.0));
}

#[test]
fn test_stop_reason_describe_carries_its_code_and_its_detail() {
    let stolen = StopReason::focus_stolen("the focus moved to 0x600003");
    assert!(
        stolen.describe().starts_with("soak/focus-stolen: "),
        "{stolen:?}"
    );
    assert!(stolen.describe().contains("0x600003"), "{stolen:?}");
    assert!(
        StopReason::process_died("gone")
            .describe()
            .starts_with("soak/process-died")
    );
}

// ── The statistics ──────────────────────────────────────────────────────────────

#[test]
fn test_rss_stats_of_a_flat_series_has_no_envelope() {
    let stats = RssStats::of(&series(10, |_| 40_000), WARMUP_S).expect("ten readings");
    assert_eq!(stats.readings, 10);
    assert_eq!(stats.envelope_kb, 0);
    assert_eq!(stats.steady_envelope_kb, 0);
    assert_eq!(stats.steady_readings, 4);
    assert_eq!(stats.slope_kib_per_hour, 0.0);
}

#[test]
fn test_rss_stats_of_a_rising_series_separates_the_warmup_from_the_drift() {
    // Every reading adds half a mebibyte, so the whole-run envelope is nine of them and the
    // steady window -- everything from sixty seconds on -- holds three.
    let stats =
        RssStats::of(&series(10, |index| 40_000 + index * 512), WARMUP_S).expect("ten readings");
    assert_eq!(stats.envelope_kb, 9 * 512);
    assert_eq!(stats.steady_envelope_kb, 3 * 512);
    assert_eq!(stats.steady_min_kb, 40_000 + 6 * 512);
    assert_eq!(stats.steady_max_kb, 40_000 + 9 * 512);
    // Half a mebibyte every ten seconds is 180MiB an hour.
    assert!(
        (stats.slope_kib_per_hour - 512.0 * 360.0).abs() < 1.0,
        "{stats:?}"
    );
}

#[test]
fn test_rss_stats_of_a_series_that_shrank_has_a_negative_slope() {
    let stats =
        RssStats::of(&series(10, |index| 40_000 - index * 512), WARMUP_S).expect("ten readings");
    assert!(stats.slope_kib_per_hour < 0.0, "{stats:?}");
    // The envelope is a magnitude: a run that gave memory back still moved.
    assert_eq!(stats.envelope_kb, 9 * 512);
}

#[test]
fn test_rss_stats_is_absent_rather_than_zero_when_too_little_was_read() {
    assert!(RssStats::of(&[], WARMUP_S).is_none());
    assert!(RssStats::of(&series(1, |_| 40_000), WARMUP_S).is_none());
    // Two readings that are both inside the warm-up leave no steady window.
    let warm = series(2, |_| 40_000);
    assert!(RssStats::of(&warm, WARMUP_S).is_none());
    assert!(RssStats::of(&warm, 0.0).is_some());
}

#[test]
fn test_rss_stats_ignores_a_reading_that_could_not_be_taken() {
    let mut samples = series(10, |_| 40_000);
    samples.push(Sample {
        t_s: 100.0,
        rss_kb: None,
        anonymous_kb: None,
    });
    let stats = RssStats::of(&samples, WARMUP_S).expect("ten readings");
    assert_eq!(stats.readings, 10, "the unreadable one is not a reading");
    assert_eq!(stats.envelope_kb, 0);
}

#[test]
fn test_rss_stats_reports_the_anonymous_envelope_only_when_every_reading_carries_one() {
    let complete = RssStats::of(&series(10, |_| 40_000), WARMUP_S).expect("ten readings");
    assert_eq!(complete.anonymous_envelope_kb, Some(0));
    let mut samples = series(10, |_| 40_000);
    if let Some(first) = samples.first_mut() {
        first.anonymous_kb = None;
    }
    let degraded = RssStats::of(&samples, WARMUP_S).expect("ten readings");
    assert_eq!(degraded.anonymous_envelope_kb, None);
}

// ── The verdict ─────────────────────────────────────────────────────────────────

#[test]
fn test_judge_passes_a_run_that_did_what_it_planned_and_stayed_flat() {
    let report = report(&eight_hour_plan(), series(10, |_| 40_000));
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(verdict.is_pass(), "{:?}", verdict.violations);
    assert!(
        verdict
            .passed
            .iter()
            .any(|line| line.contains("soak/rss-drift"))
    );
    assert!(
        verdict
            .passed
            .iter()
            .any(|line| line.contains("soak/pass-rate"))
    );
    assert!(
        verdict
            .passed
            .iter()
            .any(|line| line.contains("soak/cycle-coverage"))
    );
    assert!(
        verdict
            .passed
            .iter()
            .any(|line| line.contains("soak/crash-recorded"))
    );
}

#[test]
fn test_judge_fails_a_run_whose_resident_set_moved_past_the_ceiling() {
    // Half a mebibyte per reading, four steady readings: three mebibytes of drift against
    // the two the document states.
    let report = report(
        &eight_hour_plan(),
        series(10, |index| 40_000 + index * 1_024),
    );
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/rss-drift") && line.contains("3072KiB of 2048KiB")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_reads_the_ceiling_out_of_the_document_rather_than_a_constant() {
    // The same report, judged twice: once against the document the repository ships and
    // once against a copy whose drift ceiling has been made impossible. A verdict that did
    // not move would be one that never read the document.
    let report = report(&eight_hour_plan(), series(10, |index| 40_000 + index * 512));
    assert!(
        judge::judge(&report, &budgets())
            .expect("judgeable")
            .is_pass()
    );

    let root = budget::repo_root().expect("the repository root resolves");
    let text = fs::read_to_string(root.join(budget::BUDGETS_FILE)).expect("readable");
    let patched = text.replace("\"rss_drift_mb\": 2.0", "\"rss_drift_mb\": 0.001");
    assert_ne!(patched, text, "the threshold to patch is in the document");
    let impossible = Budgets::from_json(&patched).expect("the patched document is valid");
    let verdict = judge::judge(&report, &impossible).expect("judgeable");
    assert!(
        !verdict.is_pass(),
        "a one kibibyte ceiling is not met by 1536"
    );
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.contains("1536KiB of 1KiB")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_fails_a_run_that_was_planned_shorter_than_the_budget() {
    let report = report(&plan_of(1.0), series(10, |_| 40_000));
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/short-run") && line.contains("1.000h")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_fails_a_run_that_delivered_fewer_strokes_than_it_planned() {
    let plan = eight_hour_plan();
    let report = report_of(&plan, plan.planned_strokes() / 2, series(10, |_| 40_000));
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/pass-rate") && line.contains("50.00%")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_fails_a_run_that_stopped_early_and_says_why() {
    let plan = eight_hour_plan();
    let stopped = StopReason::focus_stolen("the focus moved from 0x400001 to 0x600003");
    let report = SoakReport {
        stopped_early: Some(stopped),
        ..report_of(&plan, 0, series(10, |_| 40_000))
    };
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict.violations.iter().any(
            |line| line.starts_with("soak/stopped-early") && line.contains("soak/focus-stolen")
        ),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_fails_a_run_whose_process_went_away() {
    let plan = eight_hour_plan();
    let mut samples = series(10, |_| 40_000);
    samples.push(Sample {
        t_s: 100.0,
        rss_kb: None,
        anonymous_kb: None,
    });
    let report = SoakReport {
        stopped_early: Some(StopReason::process_died("process 4242 exited")),
        ..report(&plan, samples)
    };
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/process-died")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_reports_an_unmeasured_drift_as_a_violation_not_a_pass() {
    // A report with one reading has no envelope at all. "Nothing was measured, therefore
    // nothing is wrong" is the reading this refusal exists to prevent.
    let plan = eight_hour_plan();
    let report = report(&plan, series(1, |_| 40_000));
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict.violations.iter().any(|line| {
            line.starts_with("soak/rss-drift") && line.contains("not a satisfied one")
        }),
        "{:?}",
        verdict.violations
    );
}

// ── The refusals ────────────────────────────────────────────────────────────────

#[test]
fn test_soak_error_messages_name_what_to_do_next() {
    let ambiguous = SoakError::AmbiguousProcess {
        pids: "101, 202".to_owned(),
    };
    let message = ambiguous.to_string();
    assert!(message.contains("101, 202"), "{message}");
    assert!(message.contains("--pid"), "{message}");

    let no_window = SoakError::NoFocusWindow {
        display: ":99".to_owned(),
        focus: 1,
    };
    let message = no_window.to_string();
    assert!(message.contains("--window"), "{message}");
    assert!(message.contains(":99"), "{message}");

    let no_process = SoakError::NoProcess { name: "fcitx5" };
    let message = no_process.to_string();
    assert!(message.contains("fcitx5"), "{message}");
    assert!(message.contains("--pid"), "{message}");
}
