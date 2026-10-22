//! The eligibility tests: the card annotation that overrides the table, the three states
//! the gate reports a case in, and the ledger landing a pending case gets.
//!
//! Split out of `tests.rs` to keep that file inside the line limit. The physical-evidence
//! test is `#[ignore]`d: it publishes a run into the repository's own `results/` tree, so
//! it is run once per delivery rather than on every `nextest` pass.

use super::*;

/// The attribute line of a pending case, as `docs/dev/tests/add.md` writes it.
fn pending_attribute_line() -> &'static str {
    "- **基本属性**：`REQ-ADD-01` ｜ `ASM-T-07` ｜ add ｜ ①④ ｜ `xtask dictc` ｜ \
     `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ \
     锚定：`TASK-1.01.01`/`TASK-1.01.02` ｜ 锚点：`xtask/src/dictc/normalize.rs`（规划）"
}

#[test]
fn test_eligibility_reads_a_pending_annotation_over_the_table() {
    // The card's own annotation decides: a case whose subject does not exist is pending
    // implementation wherever it runs, even on a machine that could otherwise serve it.
    let document = format!(
        "#### TC-ADD-001 词库权重分层归一化与质量门\n\n{}\n",
        pending_attribute_line()
    );
    let cases = parse_cases(&document);
    assert_eq!(cases.len(), 1, "{cases:?}");
    let case = &cases[0];
    assert_eq!(case.id, "TC-ADD-001");
    let anchored = case
        .pending_impl
        .as_deref()
        .expect("the annotation anchors the case");
    assert_eq!(anchored, "TASK-1.01.01/TASK-1.01.02");

    let env = EnvCapabilities::from_observation(&x11());
    let eligibility = eligibility_of(case, &env);
    assert_eq!(
        eligibility,
        super::CaseEligibility::PendingImpl {
            task: anchored.to_owned()
        },
        "the annotation decides over the table, on any machine"
    );
    assert_eq!(eligibility.label(), "pending-impl");

    // The claim the ledger records: it names the anchor, and it never passes.
    let claim = eligibility.claim().expect("a withheld case owes a claim");
    assert_eq!(claim.name, "eligibility");
    assert_eq!(claim.actual, "pending-impl: TASK-1.01.01/TASK-1.01.02");
    assert!(!claim.pass, "nothing was judged, so nothing passed");
}

#[test]
fn test_eligibility_maps_the_table_verdicts_to_the_three_states() {
    // A runnable case on a machine that serves it: no claim, the case's own assertions are
    // its record.
    let env = EnvCapabilities::from_observation(&x11());
    let runnable = case("TC-CORE-01", "core", EnvRequirement::X11Session);
    let eligibility = eligibility_of(&runnable, &env);
    assert_eq!(eligibility, super::CaseEligibility::Runnable);
    assert!(
        eligibility.claim().is_none(),
        "a runnable case makes no claim"
    );

    // A case the table cannot settle anywhere: the remedy is the row's own text, verbatim.
    let nothing = EnvCapabilities::from_observation(&nothing());
    let soak = case("TC-CORE-07", "core", EnvRequirement::LongRun);
    let eligibility = eligibility_of(&soak, &nothing);
    match &eligibility {
        super::CaseEligibility::NeedsEnvironment { remedy } => {
            assert_eq!(
                *remedy,
                soak.requirement.remedy(),
                "the remedy is cited verbatim"
            );
        }
        other => panic!("an unverifiable row lands as needs-environment, got {other:?}"),
    }
    assert_eq!(eligibility.label(), "needs-environment");
    let claim = eligibility.claim().expect("a withheld case owes a claim");
    assert!(
        claim.actual.starts_with("needs-environment: "),
        "the claim names its state: {}",
        claim.actual
    );

    // A case whose implementation the table itself waits on lands as pending too, with the
    // tasks the row names.
    let window = case("TC-UI-16", "ui", EnvRequirement::CandidateWindowUi);
    let eligibility = eligibility_of(&window, &env);
    match &eligibility {
        super::CaseEligibility::PendingImpl { task } => {
            assert!(task.contains("TASK-1.05"), "the row's own tasks: {task}");
        }
        other => panic!("a blocked row lands as pending-impl, got {other:?}"),
    }
}

#[test]
fn test_parse_refuses_a_pending_annotation_without_an_anchor() {
    // A pending case the card cannot anchor is a card defect: it is not recorded as pending,
    // and the decision table -- which withholds it -- answers instead.
    let document = concat!(
        "#### TC-ADD-099 无锚的待实现\n\n",
        "- **基本属性**：`REQ-ADD-09` ｜ add ｜ 可执行性：`[待实现]` ｜ 锚点：`规划`\n",
    );
    let cases = parse_cases(document);
    assert_eq!(cases.len(), 1, "{cases:?}");
    assert!(
        cases[0].pending_impl.is_none(),
        "no anchor, no pending verdict: {:?}",
        cases[0].pending_impl
    );

    // The source anchor is a different field and is never read as the task anchor, and a
    // case whose line declares nothing pending stays None even when it names tasks.
    let anchored_not_pending = concat!(
        "#### TC-ADD-098 可执行但锚定\n\n",
        "- **基本属性**：`REQ-ADD-09` ｜ add ｜ 可执行性：`[可执行]` ｜ ",
        "锚定：`TASK-9.09.09` ｜ 锚点：`规划`\n",
    );
    let cases = parse_cases(anchored_not_pending);
    assert!(
        cases[0].pending_impl.is_none(),
        "{:?}",
        cases[0].pending_impl
    );
}

#[test]
fn test_pending_impl_lands_in_the_ledger_as_a_skipped_claim() {
    use crate::testd::evidence::{
        AssertionV3, BatchVerdict, CHECKLIST_FILE, CaseEvidence, CaseStatus, Claim, RunDir,
        RunJournal, Tier, Utc,
    };

    // The landing the ledger contract fixes: a pending case is recorded skipped -- neither a
    // pass nor a failure -- its claim names the anchor, and the batch's checklist carries no
    // red row for it.
    let claim = super::CaseEligibility::PendingImpl {
        task: String::from("TASK-1.01.01/TASK-1.01.02"),
    }
    .claim()
    .expect("a pending case owes a claim");

    // A scratch tree of the run's own, removed when the test ends.
    let root = std::env::temp_dir().join(format!("rspinyin-gate-pending-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("creating the scratch directory");
    let results = root.join("results");

    let started = Utc::from_unix_seconds(1_700_000_000);
    let run = RunDir::create(&results, started).expect("creating the run");
    let evidence = CaseEvidence {
        tc: String::from("TC-ADD-001"),
        module: String::from("add"),
        status: CaseStatus::Skipped,
        started_at: started,
        duration_ms: 0,
        assertions: Vec::new(),
        visual: Vec::new(),
        healed: Vec::new(),
        environment: Vec::new(),
        v3: Some(AssertionV3 {
            case: String::from("TC-ADD-001"),
            tier: Tier::Smoke,
            claims: vec![claim],
            ..AssertionV3 {
                case: String::new(),
                tier: Tier::Smoke,
                dims: Vec::new(),
                requires_screenshots: false,
                screenshots_expected: Vec::new(),
                budget_keys: Vec::new(),
                claims: Vec::new(),
                diffs: Vec::new(),
            }
        }),
    };
    assert!(
        !evidence.status.needs_trace(),
        "nothing was judged, so there is no failure trace to write"
    );
    let bundle = run.write_case(&evidence, None, None).expect("the bundle");

    let mut journal = RunJournal::new(started);
    journal.record(&evidence, Ok(bundle));

    let row = &journal.rows()[0];
    assert_eq!(row.status, CaseStatus::Skipped);
    assert_eq!(row.failed_claims, 1, "the eligibility claim did not hold");
    assert!(row.is_complete(), "a skipped case is not a red row");

    let summary = journal.summary();
    assert_eq!(summary.skipped, 1);
    assert_eq!(
        summary.cases(),
        0,
        "a skipped case is counted by neither side"
    );
    assert_eq!(
        summary.verdict(),
        BatchVerdict::Pass,
        "skipped does not red the batch"
    );

    journal.publish(&run, &results).expect("publishing");
    let text =
        std::fs::read_to_string(run.root().join(CHECKLIST_FILE)).expect("the checklist is text");
    assert!(
        text.contains("\"skipped\""),
        "the status is machine-readable: {text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_the_real_suite_reads_its_pending_annotations_anchored() {
    // The probe against the suite as it actually stands: a pending declaration is only
    // recorded when the card names its tasks, so the real shard must parse with every
    // pending card anchored -- and the add shard must be part of the suite at all, which is
    // what keeps this test honest when the shard is renamed or moved.
    let root = repo_root().expect("the repository root");
    let cases = read_cases(&root).expect("reading the case suite");
    let add: Vec<_> = cases.iter().filter(|case| case.module == "add").collect();
    assert!(!add.is_empty(), "the add shard is part of the suite");
    for case in &add {
        if let Some(anchored) = &case.pending_impl {
            assert!(
                anchored.split('/').all(|task| task.starts_with("TASK-")),
                "{} anchors only task ids: {anchored}",
                case.id
            );
        }
    }
}

#[test]
#[ignore = "it writes the repository's own results tree: run it once per delivery for the physical evidence"]
fn test_publish_a_real_pending_card_as_physical_evidence() {
    use crate::testd::evidence::{
        AssertionV3, CaseEvidence, CaseStatus, RunDir, RunJournal, Tier, Utc,
    };

    // The physical landing of this gate: one real `[待实现]` card of the add shard, judged
    // by the gate, recorded skipped with its anchor claim, and published as a run whose
    // checklist counts no red row and no failure.
    let root = repo_root().expect("the repository root");
    let cases = read_cases(&root).expect("reading the case suite");
    let card = cases
        .iter()
        .find(|case| case.id == "TC-ADD-001")
        .expect("the add shard's first card is in the suite");
    let task = card
        .pending_impl
        .clone()
        .expect("the card is pending while its tasks are in flight");

    let eligibility = eligibility_of(card, &EnvCapabilities::probe());
    let claim = eligibility.claim().expect("a pending card owes a claim");
    assert_eq!(claim.actual, format!("pending-impl: {task}"));

    let results = root.join("results");
    let started = Utc::now();
    let run = RunDir::create(&results, started).expect("creating the run");
    let evidence = CaseEvidence {
        tc: card.id.clone(),
        module: card.module.clone(),
        status: CaseStatus::Skipped,
        started_at: Utc::now(),
        duration_ms: 0,
        assertions: Vec::new(),
        visual: Vec::new(),
        healed: Vec::new(),
        environment: Vec::new(),
        v3: Some(AssertionV3 {
            case: card.id.clone(),
            tier: Tier::Smoke,
            claims: vec![claim],
            ..AssertionV3 {
                case: String::new(),
                tier: Tier::Smoke,
                dims: Vec::new(),
                requires_screenshots: false,
                screenshots_expected: Vec::new(),
                budget_keys: Vec::new(),
                claims: Vec::new(),
                diffs: Vec::new(),
            }
        }),
    };
    let bundle = run.write_case(&evidence, None, None).expect("the bundle");
    let verdict_file = bundle.assertions.clone();

    let mut journal = RunJournal::new(started);
    journal.record(&evidence, Ok(bundle));
    journal.publish(&run, &results).expect("publishing the run");
    println!(
        "pending-impl evidence: {} (run {})",
        verdict_file.display(),
        run.name()
    );
}
