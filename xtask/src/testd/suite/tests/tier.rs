//! Tests for the tier ladder: what a tier selects, what its planning refuses, and what a
//! case the environment gate withheld leaves behind.
//!
//! They hold the three rules the ladder rests on. Selection: a tier is a column of the case
//! table, and planning refuses a tier it cannot fill. The gate: an acceptance run's smoke
//! phase has to be entirely green before the acceptance set starts. The withheld case: a
//! case whose requirement this machine cannot exercise is skipped with a failing
//! `eligibility` claim, so the run reports it instead of judging it. None of them runs a
//! command except where a bundle has to exist, and every scratch directory removes itself.

use std::path::PathBuf;
use std::time::Instant;

use crate::testd::env_gate::Executability;
use crate::testd::evidence::{CaseStatus, RunDir, RunJournal, Tier};

// Named from the modules that define them rather than through a glob of globs: two levels
// of re-import is not something a reader can follow, and the runner's own planning surface
// is what these tests are about.
use crate::testd::suite::cases::{CORE_CASES, CORE_MODULE, CaseLine, of_tier};
use crate::testd::suite::command::CommandRun;
use crate::testd::suite::{
    SuiteArgs, SuiteError, TierFilter, plan_phases, run_case, skipped_case, skipped_line,
    smoke_gate_green,
};

// The shared helpers of the suite's own test module: the scratch guard, the fixed clock and
// the injected command runs.
use super::*;

/// The suite arguments a tier test runs under: module `core`, nothing else selected.
fn args_of(tier: TierFilter) -> SuiteArgs {
    SuiteArgs {
        results_root: PathBuf::from("results"),
        only: Vec::new(),
        module: String::from(CORE_MODULE),
        tier,
    }
}

/// A run journal holding one recorded smoke case, judged from the runs given.
fn journal_with(tag: &str, runs: &[CommandRun]) -> RunJournal {
    let scratch = Scratch::new(tag);
    let run_dir = RunDir::create(&scratch.path, started()).expect("a fresh run directory");
    let mut journal = RunJournal::new(started());
    let evidence = run_case(case("TC-CORE-01"), environment().facts(), runs, 9);
    let bundle = run_dir.write_case(&evidence, None, None);
    journal.record(&evidence, bundle);
    journal
}

#[test]
fn test_of_tier_selects_only_the_tiers_own_rows() {
    let all: Vec<&CaseLine> = CORE_CASES.iter().collect();
    assert_eq!(
        of_tier(&all, Tier::Smoke).len(),
        30,
        "the P0 baseline is the smoke set"
    );
    assert!(
        of_tier(&all, Tier::Acceptance).is_empty(),
        "no acceptance case is transcribed yet; the ladder's later tiers arrive as edits"
    );
    assert!(of_tier(&all, Tier::Stress).is_empty());
}

#[test]
fn test_the_acceptance_tier_runs_the_smoke_set_as_its_gate() {
    assert_eq!(
        TierFilter::Acceptance.phases(),
        &[Tier::Smoke, Tier::Acceptance]
    );
    assert_eq!(TierFilter::Smoke.phases(), &[Tier::Smoke]);
    assert_eq!(TierFilter::Stress.phases(), &[Tier::Stress]);
}

#[test]
fn test_planning_the_smoke_tier_selects_the_whole_fast_set() {
    let phases = plan_phases(&args_of(TierFilter::Smoke)).expect("the smoke tier is filled");
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0].0, Tier::Smoke);
    assert_eq!(phases[0].1.len(), 30);
}

#[test]
fn test_planning_the_acceptance_tier_keeps_an_empty_acceptance_plane() {
    let phases = plan_phases(&args_of(TierFilter::Acceptance)).expect("the gate is filled");
    assert_eq!(
        phases.len(),
        1,
        "an empty acceptance plane is a state of the table, not an error: the gate ran"
    );
    assert_eq!(phases[0].0, Tier::Smoke);
}

#[test]
fn test_planning_refuses_a_requested_tier_the_table_cannot_fill() {
    let refusal =
        plan_phases(&args_of(TierFilter::Stress)).expect_err("no stress case is transcribed yet");
    assert_eq!(refusal.code(), "suite/tier/empty");
    assert!(refusal.to_string().contains("stress"), "{refusal}");
    assert!(refusal.to_string().contains(CORE_MODULE), "{refusal}");
}

#[test]
fn test_planning_the_tiers_stays_inside_the_scheduler_budget() {
    // The minimum of fifty plannings is the number the machine's own noise cannot raise:
    // the planning is a walk over thirty static rows, so its true cost is microseconds and
    // only a regression to something enormous should ever fail this.
    let args = args_of(TierFilter::Acceptance);
    let mut best = u64::MAX;
    for _ in 0..50 {
        let began = Instant::now();
        let phases = plan_phases(&args).expect("the acceptance gate is filled");
        best = best.min(began.elapsed().as_millis() as u64);
        assert!(!phases.is_empty());
    }
    assert!(
        best <= 1_000,
        "planning the tiers took {best} ms at best; the scheduler's own budget is 1 s"
    );
}

#[test]
fn test_a_case_the_gate_cannot_clear_is_skipped_with_the_refusal_itself() {
    let verdict = Executability::Unverifiable {
        needs: String::from("a live X11 session"),
        reason: String::from("no display server is running"),
    };
    let evidence = skipped_case(case("TC-CORE-01"), &[], &verdict);
    assert_eq!(evidence.status, CaseStatus::Skipped);
    assert_eq!(evidence.duration_ms, 0, "nothing ran, so nothing took time");
    let assertion = &evidence.assertions[0];
    assert_eq!(assertion.name, "eligibility");
    assert!(
        !assertion.ok,
        "the refusal is a failed assertion, not a silence"
    );
    let actual = assertion.actual.as_str();
    assert!(actual.starts_with("needs-environment:"), "{actual}");
    assert!(actual.contains("no display server is running"), "{actual}");
    let record = evidence
        .v3
        .as_ref()
        .expect("a skipped case carries its v3 record");
    assert_eq!(record.tier, Tier::Smoke);
    let claim = &record.claims[0];
    assert_eq!(claim.name, "eligibility");
    assert!(!claim.pass);
    assert!(
        claim.actual.starts_with("needs-environment:"),
        "the checklist reads the outcome from this prefix: {}",
        claim.actual
    );
}

#[test]
fn test_a_case_blocked_on_missing_code_is_withheld_as_pending_impl() {
    let verdict = Executability::Blocked {
        task: String::from("the soak harness task"),
        reason: String::from("the harness does not exist yet"),
    };
    let evidence = skipped_case(case("TC-CORE-01"), &[], &verdict);
    let actual = evidence.assertions[0].actual.as_str();
    assert!(actual.starts_with("pending-impl:"), "{actual}");
    assert!(actual.contains("the soak harness task"), "{actual}");
}

#[test]
fn test_the_skipped_line_names_the_verdict_and_the_remedy() {
    let scratch = Scratch::new("skipped-line");
    let run_dir = RunDir::create(&scratch.path, started()).expect("a fresh run directory");
    let verdict = Executability::Unverifiable {
        needs: String::from("a live X11 session"),
        reason: String::from("no display server is running"),
    };
    // The table's own rows declare no requirement, so the remedy text is borrowed from the
    // declaration a screen case would carry: the line has to name what lifts the refusal.
    let mut declared = *case("TC-CORE-01");
    declared.requirement = Some("`X11 会话`");
    let evidence = skipped_case(&declared, environment().facts(), &verdict);
    let bundle = run_dir.write_case(&evidence, None, None);
    let line = skipped_line(&declared, &bundle, &verdict);
    assert!(line.contains("TC-CORE-01"), "{line}");
    assert!(line.contains("skipped"), "{line}");
    assert!(line.contains("remedy:"), "{line}");
    assert!(line.contains("complete"), "{line}");
}

#[test]
fn test_the_smoke_gate_is_green_only_when_every_smoke_case_passed() {
    let journal = journal_with("gate-green", &[exited(0)]);
    assert!(smoke_gate_green(&journal));
    let journal = journal_with(
        "gate-flawed",
        &[not_started("the machine refused the process")],
    );
    assert!(
        !smoke_gate_green(&journal),
        "a smoke case that could not be judged closes the gate"
    );
}

#[test]
fn test_the_run_case_record_carries_the_cases_tier() {
    let evidence = run_case(case("TC-CORE-01"), environment().facts(), &[exited(0)], 9);
    let record = evidence.v3.as_ref().expect("a run case carries its record");
    assert_eq!(record.tier, Tier::Smoke);
}

#[test]
fn test_the_tier_refusals_are_stable_codes() {
    let refusal = SuiteError::EmptyTier {
        tier: "stress",
        module: String::from(CORE_MODULE),
    };
    assert_eq!(refusal.code(), "suite/tier/empty");
    let refusal = SuiteError::MachineBusy {
        reason: String::from("two builds are running"),
    };
    assert_eq!(refusal.code(), "suite/purity/busy");
    assert!(
        refusal.to_string().contains("two builds are running"),
        "{refusal}"
    );
}
