//! The batch report and the cross-run registry: what a finished run tells the executor.
//!
//! Split out of `tests.rs` to keep that file inside the line limit. The report's tests hold
//! it to the checklist it is regenerated from -- the four verdicts, the budget keys only a
//! failure drags in, the blockers with their traces -- and the registry's tests hold it to
//! the append-only, damage-tolerant contract: a read never rewrites, a damaged line costs
//! one warning, and a registry an older build wrote keeps its history.

use super::*;

/// A checklist row for `case`, as a passing Smoke case publishes it.
fn row(case: &str, status: CaseStatus) -> ChecklistRow {
    ChecklistRow {
        module: String::from("core"),
        case: String::from(case),
        tier: Some(Tier::Smoke),
        status,
        claims: 2,
        failed_claims: 0,
        assertions: true,
        trace: false,
        screenshots_expected: 0,
        screenshots_present: 0,
        screenshots_missing: Vec::new(),
        gap: None,
        withheld: None,
        dims: Vec::new(),
        budget_keys: Vec::new(),
    }
}

/// The checklist of one run named `run`, from the rows given.
fn checklist_of(run: &str, rows: Vec<ChecklistRow>) -> Checklist {
    let verdict = if rows.iter().any(|row| row.status == CaseStatus::Fail) {
        BatchVerdict::Fail
    } else {
        BatchVerdict::Pass
    };
    Checklist::of(run, verdict, rows)
}

#[test]
fn test_the_report_maps_the_four_verdicts_the_way_the_gate_withheld_them() {
    // The vocabulary the card fixes: a pass, a failure, a case the implementation has not
    // reached, and a case this machine cannot judge. The last two are both recorded
    // "skipped" in the ledger -- what tells them apart is the eligibility claim's own
    // prefix, which the checklist row carries.
    let mut pending = row("TC-ADD-001", CaseStatus::Skipped);
    pending.withheld = Some(String::from("pending-impl"));
    let mut environment = row("TC-UI-16", CaseStatus::Skipped);
    environment.withheld = Some(String::from("needs-environment"));
    let report = FinalReport::of(&checklist_of(
        "run-1",
        vec![
            row("TC-CORE-01", CaseStatus::Pass),
            row("TC-CORE-02", CaseStatus::Fail),
            pending,
            environment,
        ],
    ));
    assert_eq!(report.verdicts["TC-CORE-01"].verdict, CaseOutcome::Pass);
    assert_eq!(report.verdicts["TC-CORE-02"].verdict, CaseOutcome::Fail);
    assert_eq!(
        report.verdicts["TC-ADD-001"].verdict,
        CaseOutcome::PendingImpl,
        "the gate withheld it for the implementation"
    );
    assert_eq!(
        report.verdicts["TC-UI-16"].verdict,
        CaseOutcome::Unverifiable,
        "the gate withheld it for the environment"
    );

    // A case whose evidence could not be written was judged by nothing, so the report says
    // so rather than counting it either way.
    let report = FinalReport::of(&checklist_of(
        "run-1",
        vec![row("TC-CORE-03", CaseStatus::Flawed)],
    ));
    assert_eq!(
        report.verdicts["TC-CORE-03"].verdict,
        CaseOutcome::Unverifiable
    );
}

#[test]
fn test_the_report_drags_in_budget_keys_only_from_failing_cases() {
    // The budget-violation list is names-only and comes from the failing side alone: a
    // passing case that touched a budget is a measurement, not a violation.
    let mut failing = row("TC-CORE-02", CaseStatus::Fail);
    failing.failed_claims = 1;
    failing.budget_keys = vec![String::from("BUDGET-LAT-02"), String::from("BUDGET-LAT-01")];
    let mut passing = row("TC-CORE-01", CaseStatus::Pass);
    passing.budget_keys = vec![String::from("BUDGET-LAT-01")];
    let report = FinalReport::of(&checklist_of("run-1", vec![failing, passing]));
    assert_eq!(
        report.budget_violations,
        vec![String::from("BUDGET-LAT-01"), String::from("BUDGET-LAT-02")],
        "the failing case's keys, deduplicated and ordered"
    );
}

#[test]
fn test_the_report_names_blockers_with_their_traces_and_the_red_rows() {
    // Two kinds of blocker: the isolated defect a failing case is -- its trace named, so a
    // reader goes straight to it -- and the platform defect a red row records, evidence the
    // card declared and the harness never wrote.
    let mut failing = row("TC-CORE-02", CaseStatus::Fail);
    failing.failed_claims = 1;
    failing.trace = true;
    let mut gapped = row("TC-UI-07", CaseStatus::Pass);
    gapped.screenshots_expected = 1;
    gapped.screenshots_present = 0;
    gapped.screenshots_missing = vec![String::from("02_selected.png")];
    let report = FinalReport::of(&checklist_of("run-9", vec![failing, gapped]));
    assert_eq!(report.blockers.len(), 2, "{report:?}");

    let defect = &report.blockers[0];
    assert_eq!(defect.case, "TC-CORE-02");
    assert!(defect.reason.contains("1/2"), "{}", defect.reason);
    assert_eq!(
        defect.trace.as_deref(),
        Some("results/runs/run-9/core/TC-CORE-02/trace.json"),
        "the trace is named where the layout puts it"
    );

    let gap = &report.blockers[1];
    assert_eq!(gap.case, "TC-UI-07");
    assert!(
        gap.reason.contains("02_selected.png"),
        "the gap names what is missing: {}",
        gap.reason
    );
    assert_eq!(gap.trace, None, "an evidence gap is not a case failure");
}

#[test]
fn test_an_empty_batch_renders_an_empty_report() {
    // A run of zero cases has no verdict to give, and the report says that in as many
    // words rather than dressing emptiness up as a pass.
    let report = FinalReport::of(&checklist_of("run-0", Vec::new()));
    assert!(report.verdicts.is_empty());
    assert!(report.budget_violations.is_empty());
    assert!(report.blockers.is_empty());
    let text = report.render();
    assert!(text.contains("批次报告"), "{text}");
    assert!(text.contains("用例：0"), "{text}");
    assert!(text.contains("预算违例：无"), "{text}");
    assert!(text.contains("阻塞项：无"), "{text}");
    assert!(
        !text.contains("## 阻塞项"),
        "no blockers, no blocker section"
    );
}

#[test]
fn test_the_report_renders_the_vocabulary_a_chinese_reader_acts_on() {
    // The report is documentation this project writes in Chinese; ids, keys and paths stay
    // as the ledger wrote them.
    let mut failing = row("TC-CORE-02", CaseStatus::Fail);
    failing.failed_claims = 2;
    failing.trace = true;
    failing.budget_keys = vec![String::from("BUDGET-LAT-02")];
    failing.dims = vec![Dim::CorePath, Dim::Boundary];
    let report = FinalReport::of(&checklist_of("run-1", vec![failing]));
    let text = report.render();
    assert!(text.contains("失败"), "{text}");
    assert!(text.contains("BUDGET-LAT-02"), "{text}");
    assert!(text.contains("TC-CORE-02"), "{text}");
    assert!(
        text.contains("①、②"),
        "the dimensions print as the legend's: {text}"
    );
    assert!(
        text.contains("results/runs/run-1/core/TC-CORE-02/trace.json"),
        "{text}"
    );
}

#[test]
fn test_a_published_run_regenerates_its_report_from_the_run_directory_alone() {
    // The regeneration property: the report is rendered from the checklist the run
    // published, by reading that document back -- the generator owns no state a second
    // copy could drift from.
    let scratch = Scratch::new("report-regenerate");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let mut evidence = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    evidence.v3 = Some(AssertionV3 {
        case: String::from("TC-CORE-01"),
        tier: Tier::Smoke,
        dims: vec![Dim::CorePath],
        requires_screenshots: false,
        screenshots_expected: Vec::new(),
        budget_keys: Vec::new(),
        claims: Vec::new(),
        diffs: Vec::new(),
    });
    let bundle = run
        .write_case(&evidence, None, None)
        .expect("writing the bundle");
    let mut journal = RunJournal::new(started());
    journal.record(&evidence, Ok(bundle));
    journal.publish(&run, &results).expect("publishing the run");

    let text = fs::read_to_string(run.root().join(CHECKLIST_FILE)).expect("the checklist is text");
    let report = FinalReport::of(&Checklist::from_json(&text).expect("the checklist parses"));
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.verdicts["TC-CORE-01"].tier, Some(Tier::Smoke));
    assert_eq!(report.verdicts["TC-CORE-01"].dims, vec![Dim::CorePath]);
    assert_eq!(report.verdicts["TC-CORE-01"].verdict, CaseOutcome::Pass);
    assert!(report.blockers.is_empty());
}

#[test]
fn test_register_run_appends_and_a_read_skips_damaged_lines() {
    // The registry's two contracts in one tree: appending never touches the bytes already
    // there, and reading skips what it cannot parse with a warning instead of refusing the
    // history around it.
    let scratch = Scratch::new("report-registry");
    let results = scratch.results();
    let registry = registry_path(&results);
    let first = RunDir::create(&results, started()).expect("creating the first run");
    register_run(&registry, entry_of(&first, started(), BatchVerdict::Pass))
        .expect("registering the first run");
    let before = fs::read_to_string(&registry).expect("the registry is text");

    let later = Utc::from_unix_seconds(1_700_000_060);
    let second = RunDir::create(&results, later).expect("creating the second run");
    register_run(&registry, entry_of(&second, later, BatchVerdict::Fail))
        .expect("registering the second run");
    let mut after = fs::read_to_string(&registry).expect("the registry is text");
    after.push_str("{\"this\": \"is not a run entry\"}\n");
    fs::write(&registry, &after).expect("the damaged line is planted");

    let index = RunIndex::open(&results).expect("the registry reads around the damage");
    assert_eq!(index.runs().len(), 2, "the healthy entries survive");
    assert_eq!(index.latest_run(), Some(second.name()));
    assert_eq!(
        mode_of(&registry),
        0o600,
        "the registry is the owner's, as the rest of the tree is"
    );
    assert!(
        fs::read_to_string(&registry)
            .expect("the registry is text")
            .starts_with(&before),
        "the first registration's bytes were never rewritten"
    );
}

#[test]
fn test_a_registry_render_is_never_the_bottleneck() {
    // The card's budget: rendering a batch report takes at most 100 ms. A checklist of five
    // hundred rows is the batch a long acceptance run publishes, and the assertion is the
    // fastest of ten passes -- the render's own cost, not the scheduler's.
    let rows: Vec<ChecklistRow> = (0..500)
        .map(|index| {
            let mut line = row(&format!("TC-CORE-{index:03}"), CaseStatus::Pass);
            line.dims = vec![Dim::CorePath];
            line
        })
        .collect();
    let report = FinalReport::of(&checklist_of("run-bulk", rows));
    let budget = std::time::Duration::from_millis(100);
    // The same settle the other timing tests keep: a nextest run starts while the build's
    // last jobs drain, and the settle keeps the builder out of the quantity measured.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let fastest = (0..10)
        .map(|_| {
            let began = std::time::Instant::now();
            let text = report.render();
            assert!(text.contains("批次报告"), "the render is the report");
            began.elapsed()
        })
        .min()
        .expect("ten passes always yield a minimum");
    assert!(
        fastest <= budget,
        "the fastest of ten renders took {fastest:?}, over the 100 ms budget"
    );
}
