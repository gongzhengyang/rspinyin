//! The run index and the legacy-layout refusal.
//!
//! Split out of `tests.rs` to keep that file inside the line limit. Both are about what a run
//! leaves behind for the *next* one: the document `latest_run` is resolved from, the refusal a
//! flat `results/screenshots` tree earns, and the rule that a document this harness does not
//! understand is left exactly as it was found.

use super::*;

#[test]
fn test_refuse_legacy_layout_refuses_the_flat_directories_only() {
    // The rule is about the first segment under `results`, so a case directory that happens to
    // be named after a harness is not refused: `results/runs/<run>/ui/screenshots/...` is a
    // legitimate case name, and a check that searched the whole path would reject it.
    let run_layout = Path::new("results/runs/run-20231114-221320/core/TC-CORE-01");
    assert!(refuse_legacy_layout(run_layout).is_ok());
    let nested = Path::new("results/runs/run-20231114-221320/ui/screenshots/TC-UI-16");
    assert!(
        refuse_legacy_layout(nested).is_ok(),
        "a case directory may carry a legacy name without being one"
    );
    // A path with no `results` segment at all is not this rule's business.
    assert!(refuse_legacy_layout(Path::new("/tmp/scratch/core/TC-CORE-01")).is_ok());

    for legacy in LEGACY_SUBDIRS {
        let flat = Path::new(RESULTS_DIR).join(legacy);
        match refuse_legacy_layout(&flat) {
            Err(EvidenceError::LegacyLayout { found, .. }) => assert_eq!(found, legacy),
            other => panic!("{flat:?} must be refused, got {other:?}"),
        }
        // A file inside the legacy directory is refused as well: a harness that wrote one flat
        // case there and then asked about the case would otherwise be let through.
        let inside = flat.join("core").join("TC-CORE-01");
        assert!(
            refuse_legacy_layout(&inside).is_err(),
            "{inside:?} names the legacy directory"
        );
    }
}

#[test]
fn test_run_index_points_latest_run_at_the_newest_batch() {
    let scratch = Scratch::new("index-latest");
    let results = scratch.results();
    let older = RunDir::create(&results, started()).expect("creating the older run");
    let mut index = RunIndex::open(&results).expect("opening the index");
    assert_eq!(index.latest_run(), None, "an empty tree points at nothing");
    index.record(entry_of(&older, started(), BatchVerdict::Pass));
    index.write().expect("writing the index");
    assert_eq!(
        RunIndex::open(&results).expect("reopening").latest_run(),
        Some(older.name()),
        "the only run is the newest one"
    );

    let later = Utc::from_unix_seconds(1_700_000_060);
    let newer = RunDir::create(&results, later).expect("creating the newer run");
    let mut index = RunIndex::open(&results).expect("reopening the index");
    index.record(entry_of(&newer, later, BatchVerdict::Fail));
    index.write().expect("writing the index");
    let index = RunIndex::open(&results).expect("reopening the index again");
    assert_eq!(
        index.latest_run(),
        Some(newer.name()),
        "a later batch takes the pointer"
    );
    assert_eq!(index.runs().len(), 2, "both runs are held");

    // Recording an older run again does not drag the pointer backwards: the newest is a
    // property of the names the index holds, not of the order they arrived in.
    let mut index = RunIndex::open(&results).expect("reopening the index");
    index.record(entry_of(&older, started(), BatchVerdict::Pass));
    index.write().expect("writing the index");
    let index = RunIndex::open(&results).expect("reopening the index");
    assert_eq!(
        index.latest_run(),
        Some(newer.name()),
        "re-recording an older run leaves the pointer where it was"
    );
    assert_eq!(
        index.runs().len(),
        2,
        "a run that is already held is replaced rather than appended"
    );
}

#[test]
fn test_run_index_writes_the_document_ci_resolves_latest_run_from() {
    let scratch = Scratch::new("index-document");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let mut index = RunIndex::open(&results).expect("opening the index");
    index.record(entry_of(&run, started(), BatchVerdict::Pass));
    index.write().expect("writing the index");

    let path = results.join(RESULTS_INDEX_FILE);
    assert_eq!(mode_of(&path), 0o600, "the index is the owner's");
    let document = read_json(&path);
    assert_eq!(
        document["format"],
        Value::from(INDEX_FORMAT_VERSION),
        "the document names the format it is written in"
    );
    assert_eq!(document["latest_run"], Value::from(run.name()));
    let runs = document["runs"].as_array().expect("the runs are a list");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run"], Value::from(run.name()));
    assert_eq!(
        runs[0]["path"],
        Value::from(format!("{RESULTS_DIR}/{RUNS_DIR}/{}", run.name()))
    );
    assert_eq!(runs[0]["verdict"], Value::from("pass"));
    assert_eq!(runs[0]["cases"], Value::from(1));
}

#[test]
fn test_run_index_refuses_a_legacy_results_root() {
    let scratch = Scratch::new("index-legacy");
    let results = scratch.results();
    for legacy in LEGACY_SUBDIRS {
        let flat = results.join(legacy);
        match RunIndex::open(&flat) {
            Err(EvidenceError::LegacyLayout { found, .. }) => assert_eq!(found, legacy),
            other => panic!("{flat:?} must be refused, got {other:?}"),
        }
        assert!(
            !flat.exists(),
            "a refused root is not created: {flat:?} must stay absent"
        );
    }
}

#[test]
fn test_run_index_refuses_a_document_it_does_not_understand() {
    // A document from a later build: the format is one this harness cannot read, so it is
    // refused rather than overwritten -- it holds every run before this one, and a run that
    // quietly replaced it would take the history with it.
    let scratch = Scratch::new("index-unknown");
    let results = scratch.results();
    fs::create_dir_all(&results).expect("creating the results tree");
    let path = results.join(RESULTS_INDEX_FILE);
    let unknown = "{\"format\": 99, \"latest_run\": null, \"runs\": []}";
    fs::write(&path, unknown).expect("a document from a later build");

    match RunIndex::open(&results) {
        Err(EvidenceError::Index { detail, .. }) => assert!(detail.contains("99"), "{detail}"),
        other => panic!("an unknown format must be refused, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&path).expect("the document is still there"),
        unknown,
        "a refused document is left as it was: it holds every run before this one"
    );
}
