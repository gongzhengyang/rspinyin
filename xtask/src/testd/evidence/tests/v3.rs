//! The tests-v3.0 record and the batch checklist.
//!
//! Split out of `tests.rs` to keep that file inside the line limit. They hold the v3 contract's
//! own promises: the record round-trips through JSON with the ladder's vocabulary intact, a
//! declared screenshot that never landed is a red row and a flawed batch rather than a
//! rewritten verdict, damaged JSON is refused as a value and never unwrapped through, and the
//! checklist is published beside the index and reads back from the run directory alone.

use super::*;

/// A v3 record for `tc`, declaring what the caller names.
///
/// The defaults are the runner's own: the tier is the case table's, and the dimensions, the
/// screenshot declarations and the budget keys are whatever the caller passes -- a test that
/// declares nothing gets the honest "nothing declared" record the runner writes today.
fn v3_of(tc: &str, tier: Tier) -> AssertionV3 {
    AssertionV3 {
        case: tc.to_owned(),
        tier,
        dims: Vec::new(),
        requires_screenshots: false,
        screenshots_expected: Vec::new(),
        budget_keys: Vec::new(),
        claims: Vec::new(),
        diffs: Vec::new(),
    }
}

#[test]
fn test_v3_record_round_trips_through_json() {
    let record = AssertionV3 {
        case: String::from("TC-UI-07"),
        tier: Tier::Acceptance,
        dims: vec![Dim::CorePath, Dim::Boundary],
        requires_screenshots: true,
        screenshots_expected: vec![String::from("01_default.png")],
        budget_keys: vec![String::from("BUDGET-LAT-01")],
        claims: vec![
            Claim {
                name: String::from("first_candidate"),
                expect: String::from("你"),
                actual: String::from("你"),
                pass: true,
            },
            Claim {
                name: String::from("candidate_count"),
                expect: String::from("5"),
                actual: String::from("4"),
                pass: false,
            },
        ],
        diffs: Vec::new(),
    };
    let text = record.to_json().expect("the record is JSON");
    let read = AssertionV3::from_json(&text).expect("the record is read back");
    assert_eq!(read, record, "the round trip loses nothing");
    // The tier and the dimensions are the ladder's own vocabulary, in both directions.
    let document: Value = serde_json::from_str(&text).expect("the text is JSON");
    assert_eq!(document["tier"], Value::from("acceptance"), "{text}");
    assert_eq!(
        document["dims"],
        Value::from(vec![Value::from("d1"), Value::from("d2")]),
        "{text}"
    );
}

#[test]
fn test_checklist_round_trips_through_json_with_its_red_rows() {
    let complete = ChecklistRow {
        module: String::from("core"),
        case: String::from("TC-CORE-01"),
        tier: Some(Tier::Smoke),
        status: CaseStatus::Pass,
        claims: 3,
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
    };
    let red = ChecklistRow {
        module: String::from("ui"),
        case: String::from("TC-UI-07"),
        tier: Some(Tier::Acceptance),
        status: CaseStatus::Pass,
        claims: 2,
        failed_claims: 0,
        assertions: true,
        trace: false,
        screenshots_expected: 2,
        screenshots_present: 1,
        screenshots_missing: vec![String::from("02_selected.png")],
        gap: None,
        withheld: None,
        dims: Vec::new(),
        budget_keys: Vec::new(),
    };
    let checklist = Checklist::of(
        "run-20260921-100000",
        BatchVerdict::Flawed,
        vec![complete, red],
    );
    let text = checklist.to_json().expect("the checklist is JSON");
    let read = Checklist::from_json(&text).expect("the checklist is read back");
    assert_eq!(read, checklist, "the round trip loses nothing");
    assert_eq!(read.format, CHECKLIST_FORMAT_VERSION);
    assert_eq!(
        read.red_rows()
            .map(|row| row.case.as_str())
            .collect::<Vec<_>>(),
        vec!["TC-UI-07"],
        "only the row whose declared evidence is absent is red"
    );
}

#[test]
fn test_v3_missing_screenshots_lists_only_the_names_not_on_disk() {
    let scratch = Scratch::new("v3-shots");
    let dir = scratch.path.join("TC-UI-07");
    fs::create_dir_all(&dir).expect("the bundle directory");
    fs::write(dir.join("01_default.png"), b"png").expect("the screenshot that landed");

    let record = AssertionV3 {
        requires_screenshots: true,
        screenshots_expected: vec![
            String::from("01_default.png"),
            String::from("02_selected.png"),
        ],
        ..v3_of("TC-UI-07", Tier::Smoke)
    };
    assert_eq!(
        record.missing_screenshots(&dir),
        vec![String::from("02_selected.png")],
        "the name that landed is accounted for, the one that did not is the red list"
    );

    let undeclared = v3_of("TC-UI-01", Tier::Smoke);
    assert!(
        undeclared.missing_screenshots(&scratch.path).is_empty(),
        "a case that declares nothing owes nothing, wherever it is pointed"
    );
}

#[test]
fn test_v3_damaged_json_is_refused_as_a_value_and_not_unwrapped() {
    // A record missing its tier is not a record this build understands: it is refused as a
    // value, and the caller decides what a damaged record means.
    match AssertionV3::from_json("{\"case\": \"TC-UI-07\"}") {
        Err(EvidenceError::Json { detail, .. }) => assert!(detail.contains("tier"), "{detail}"),
        other => panic!("a record without a tier must be refused, got {other:?}"),
    }
    // A tier is an enum, so a misspelled one is refused at the same place.
    match AssertionV3::from_json("{\"case\":\"TC\",\"tier\":\"smokee\",\"dims\":[],\"claims\":[]}")
    {
        Err(EvidenceError::Json { .. }) => {}
        other => panic!("a misspelled tier must be refused, got {other:?}"),
    }
    match Checklist::from_json("[]") {
        Err(EvidenceError::Json { .. }) => {}
        other => panic!("a checklist must be an object, got {other:?}"),
    }
}

#[test]
fn test_journal_records_a_declared_screenshot_gap_as_a_red_row() {
    let scratch = Scratch::new("v3-red-row");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");

    let mut record = v3_of("TC-UI-07", Tier::Acceptance);
    record.dims = vec![Dim::CorePath];
    record.requires_screenshots = true;
    record.screenshots_expected = vec![
        String::from("01_default.png"),
        String::from("02_selected.png"),
    ];
    record.budget_keys = vec![String::from("BUDGET-LAT-01")];
    record.claims = vec![
        Claim {
            name: String::from("window_shown"),
            expect: String::from("the frame is on screen"),
            actual: String::from("shown"),
            pass: true,
        },
        Claim {
            name: String::from("selection_highlight"),
            expect: String::from("the first row is highlighted"),
            actual: String::from("not verifiable on this machine"),
            pass: false,
        },
    ];
    let recorded = evidence("ui", "TC-UI-07", CaseStatus::Pass);
    let recorded = CaseEvidence {
        v3: Some(record),
        ..recorded
    };
    let bundle = run
        .write_case(&recorded, None, None)
        .expect("writing the bundle");
    // One of the two declared screenshots lands; the other stays owed.
    fs::write(bundle.dir.join("01_default.png"), b"png").expect("the screenshot that landed");

    let mut journal = RunJournal::new(started());
    journal.record(&recorded, Ok(bundle));

    let row = &journal.rows()[0];
    assert_eq!(row.tier, Some(Tier::Acceptance));
    assert_eq!((row.claims, row.failed_claims), (2, 1));
    assert_eq!(
        row.status,
        CaseStatus::Pass,
        "a screenshot gap is a platform defect, not a rewritten verdict"
    );
    assert_eq!(
        row.screenshots_missing,
        vec![String::from("02_selected.png")]
    );
    assert!(!row.is_complete());

    let summary = journal.summary();
    assert_eq!(summary.screenshot_gaps, 1);
    assert_eq!(summary.verdict(), BatchVerdict::Flawed);

    let checklist = journal.checklist(&run);
    assert_eq!(
        checklist
            .red_rows()
            .map(|row| row.case.as_str())
            .collect::<Vec<_>>(),
        vec!["TC-UI-07"]
    );
    let row = &checklist.rows[0];
    assert_eq!(row.screenshots_expected, 2);
    assert_eq!(row.screenshots_present, 1);
    assert_eq!(row.evidence_label(), "missing: 02_selected.png");

    // The checklist is published beside the index and reads back from the run directory.
    journal.publish(&run, &results).expect("publishing the run");
    let text = fs::read_to_string(run.root().join(CHECKLIST_FILE)).expect("the checklist is text");
    let document: Value = serde_json::from_str(&text).expect("the checklist is JSON");
    assert_eq!(document["verdict"], Value::from("flawed"), "{text}");
    let read = Checklist::from_json(&text).expect("the checklist is read back");
    assert_eq!(read.rows.len(), 1);
}
