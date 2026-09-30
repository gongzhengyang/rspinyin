//! Tests for the three observations a soak report carries besides the memory series.
//!
//! The pass counts, the crash records and the host readings are the parts of a long run that
//! the resident set cannot show: a plugin that crashed eight times and stayed alive has a flat
//! curve, and so does a run that delivered every stroke it planned without ever closing the
//! state loop. Everything here runs without a display server, a Fcitx5 session or eight
//! hours, because all three are arithmetic and parsing over data these tests write by hand.
//!
//! They live in their own file rather than beside the rest of the tool's tests because
//! together the two would be longer than the project's file limit allows; the fixtures they
//! drive are the ones in [`super::tests`], so a report built here is the one the driver would
//! have written.

use std::fs;

use anyhow::Result;

use super::judge;
use super::report::{CrashCount, CycleReport, SoakReport};
use super::tests::{budgets, cycle_strokes, eight_hour_plan, report, scratch, series};
use super::{Phase, Plan, SoakError, assert_report};

#[test]
fn test_cycle_report_of_counts_whole_passes_and_never_divides_by_zero() {
    let plan = eight_hour_plan();
    let per_cycle = cycle_strokes();
    let report = CycleReport::of(&plan, per_cycle, plan.planned_strokes());
    assert_eq!(report.strokes_per_cycle, per_cycle);
    // Whole passes only: a plan whose last pass is half finished has walked the passes it
    // counts and no more, which is what makes the count a claim about the state loop.
    assert_eq!(report.planned, plan.planned_strokes() / per_cycle);
    assert!(report.planned * per_cycle <= plan.planned_strokes());
    assert_eq!(report.delivered, report.planned);
    report
        .verify(&plan, plan.planned_strokes())
        .expect("the counts are the ones the run's numbers come to");

    // A cycle of no strokes comes to no whole pass of anything, and is answered as zero
    // passes rather than by dividing by it.
    let empty = CycleReport::of(&plan, 0, plan.planned_strokes());
    assert_eq!((empty.planned, empty.delivered), (0, 0));
    assert!(
        empty.verify(&plan, plan.planned_strokes()).is_err(),
        "a count nothing can be a fraction of is refused"
    );
}

#[test]
fn test_cycle_report_verify_refuses_a_count_that_disagrees_with_its_strokes() {
    // The counts are one number in two places, and a report whose copies disagree cannot say
    // how much of its cycle it walked; the refusal is what keeps a stored count from being
    // believed over the strokes it was derived from.
    let plan = eight_hour_plan();
    let honest = report(&plan, series(10, |_| 40_000));
    assert_eq!(
        honest.cycles().expect("the counts agree").delivered,
        honest.cycles.delivered
    );

    let tampered = SoakReport {
        cycles: CycleReport {
            delivered: honest.cycles.delivered + 1,
            ..honest.cycles
        },
        ..honest.clone()
    };
    let refused = tampered
        .cycles()
        .expect_err("one pass more than the strokes hold");
    assert!(
        refused.to_string().contains("delivered passes"),
        "{refused}"
    );
}

#[test]
fn test_crash_count_recorded_is_absent_rather_than_zero_when_a_reading_is_missing() {
    let complete = CrashCount {
        directory: "/data/rspinyin/crash".to_owned(),
        before: Some(3),
        after: Some(5),
    };
    assert_eq!(complete.recorded(), Some(2));
    // A run in which nothing appeared is a measurement of zero, and it is the only case that
    // is: a reading nobody took must not read the same way.
    let quiet = CrashCount {
        after: Some(3),
        ..complete.clone()
    };
    assert_eq!(quiet.recorded(), Some(0));
    assert_eq!(
        CrashCount {
            after: None,
            ..complete.clone()
        }
        .recorded(),
        None
    );
    assert_eq!(
        CrashCount {
            before: None,
            ..complete
        }
        .recorded(),
        None
    );
}

#[test]
fn test_judge_fails_a_run_too_short_to_complete_a_pass_over_the_cycle() {
    // A run shorter than one pass delivers every stroke it planned and never closes the state
    // loop once: the pass rate is 100% and the run has exercised one phase of the session for
    // its whole duration. That is the case the cycle count exists for.
    let plan = Plan::new(0.0002, 10.0, 0.1, 0.1).expect("a plan of a few strokes");
    assert!(plan.planned_strokes() < cycle_strokes());
    let report = report(&plan, series(4, |_| 40_000));
    assert_eq!(report.cycles.planned, 0);
    assert_eq!(report.pass_rate_pct(), Some(100.0));
    let verdict = judge::judge(&report, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/cycle-coverage")
                && line.contains("no whole pass over the cycle")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_judge_fails_a_run_in_which_the_plugin_recorded_a_crash() {
    // The plugin's crash path does not kill the daemon: a panic is caught at the C ABI
    // boundary and written to a record, so a crashing plugin is one whose resident set is
    // still flat. A verdict that watched only the process would call this run a pass.
    let plan = eight_hour_plan();
    let crashed = SoakReport {
        crashes: Some(CrashCount {
            directory: "/data/rspinyin/crash".to_owned(),
            before: Some(3),
            after: Some(5),
        }),
        ..report(&plan, series(10, |_| 40_000))
    };
    let verdict = judge::judge(&crashed, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/crash-recorded")
                && line.contains("2 record(s) appeared in /data/rspinyin/crash")),
        "{:?}",
        verdict.violations
    );

    // Records that were already there are not this run's, so a directory that held three
    // before and three after is a pass with the number printed beside it.
    let quiet = SoakReport {
        crashes: Some(CrashCount {
            directory: "/data/rspinyin/crash".to_owned(),
            before: Some(3),
            after: Some(3),
        }),
        ..report(&plan, series(10, |_| 40_000))
    };
    let verdict = judge::judge(&quiet, &budgets()).expect("the report is judgeable");
    assert!(verdict.is_pass(), "{:?}", verdict.violations);
    assert!(
        verdict
            .passed
            .iter()
            .any(|line| line.contains("held 3 record(s) at both ends")),
        "{:?}",
        verdict.passed
    );
}

#[test]
fn test_judge_reports_a_crash_count_nobody_took_as_a_violation() {
    // "Nothing was measured, therefore nothing is wrong" is the reading this refusal exists
    // to prevent, and it applies to the crash count as much as to the drift.
    let plan = eight_hour_plan();
    let unmeasured = SoakReport {
        crashes: None,
        ..report(&plan, series(10, |_| 40_000))
    };
    let verdict = judge::judge(&unmeasured, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/crash-unmeasured")
                && line.contains("data directory could not be resolved")),
        "{:?}",
        verdict.violations
    );

    // The directory resolved and one end of it could not be read: the same refusal, because
    // a directory that could not be read is not one in which nothing was recorded.
    let unreadable = SoakReport {
        crashes: Some(CrashCount {
            directory: "/data/rspinyin/crash".to_owned(),
            before: Some(0),
            after: None,
        }),
        ..report(&plan, series(10, |_| 40_000))
    };
    let verdict = judge::judge(&unreadable, &budgets()).expect("the report is judgeable");
    assert!(!verdict.is_pass());
    assert!(
        verdict
            .violations
            .iter()
            .any(|line| line.starts_with("soak/crash-unmeasured")
                && line.contains("/data/rspinyin/crash")),
        "{:?}",
        verdict.violations
    );
}

#[test]
fn test_soak_error_names_the_phases_a_cycle_that_does_not_close_leaves() {
    // The refusal has to say where the cycle stopped, because the person reading it is the
    // one who can fix the script, and "the cycle is not closed" without the two phase names
    // gives them nothing to look at.
    let refusal = SoakError::CycleNotClosed {
        from: Phase::Idle,
        to: Phase::Composing,
    };
    let message = refusal.to_string();
    assert!(message.contains("Composing"), "{message}");
    assert!(message.contains("Idle"), "{message}");
    assert!(message.contains("measure the driver"), "{message}");
}

#[test]
fn test_assert_report_fails_a_report_whose_drift_is_past_the_ceiling() -> Result<()> {
    // The end-to-end path the soak job takes: a report file on disk, the repository's own
    // budget document, and the entry point both `soak --assert` and `budget --soak-report`
    // reach. The reverse half is the point -- a flat eight-hour report passes, the same
    // report with a ceiling three mebibytes wide fails -- which is what shows the gate
    // judges the file rather than reporting on it.
    let dir = scratch("assert-report");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;

    let drifting = dir.join("drifting.json");
    report(
        &eight_hour_plan(),
        series(10, |index| 40_000 + index * 1_024),
    )
    .write(&drifting)?;
    let failure =
        assert_report(&drifting).expect_err("three mebibytes against a two mebibyte ceiling");
    assert!(failure.to_string().contains("soak/rss-drift"), "{failure}");

    let flat = dir.join("flat.json");
    report(&eight_hour_plan(), series(10, |_| 40_000)).write(&flat)?;
    assert_report(&flat).expect("a flat eight-hour report is inside every budget");

    // A report whose own counts disagree with its strokes is refused rather than judged:
    // the verdict would otherwise be about a run that never happened.
    let honest = report(&eight_hour_plan(), series(10, |_| 40_000));
    let tampered = SoakReport {
        cycles: CycleReport {
            delivered: 0,
            ..honest.cycles
        },
        ..honest.clone()
    };
    let inconsistent = dir.join("inconsistent.json");
    tampered.write(&inconsistent)?;
    let failure = assert_report(&inconsistent).expect_err("the counts disagree");
    assert!(
        failure.to_string().contains("delivered passes"),
        "{failure}"
    );

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}
