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
    let registry = registry_path(&results);
    assert_eq!(
        RunIndex::open(&results)
            .expect("opening the index")
            .latest_run(),
        None,
        "an empty tree points at nothing"
    );
    register_run(&registry, entry_of(&older, started(), BatchVerdict::Pass))
        .expect("registering the older run");
    assert_eq!(
        RunIndex::open(&results).expect("reopening").latest_run(),
        Some(older.name()),
        "the only run is the newest one"
    );

    let later = Utc::from_unix_seconds(1_700_000_060);
    let newer = RunDir::create(&results, later).expect("creating the newer run");
    register_run(&registry, entry_of(&newer, later, BatchVerdict::Fail))
        .expect("registering the newer run");
    let index = RunIndex::open(&results).expect("reopening the index again");
    assert_eq!(
        index.latest_run(),
        Some(newer.name()),
        "a later batch takes the pointer"
    );
    assert_eq!(index.runs().len(), 2, "both runs are held");

    // Registering an older run again does not drag the pointer backwards: the newest is a
    // property of the names the registry holds, not of the order they arrived in. And a
    // name registered twice is one entry -- the later line is the statement a reader sees.
    let mut again = entry_of(&older, started(), BatchVerdict::Pass);
    again.cases = 5;
    register_run(&registry, again).expect("re-registering the older run");
    let index = RunIndex::open(&results).expect("reopening the index");
    assert_eq!(
        index.latest_run(),
        Some(newer.name()),
        "re-registering an older run leaves the pointer where it was"
    );
    assert_eq!(
        index.runs().len(),
        2,
        "a run that is already held is replaced by its later line rather than appended to"
    );
    let held = index
        .runs()
        .iter()
        .find(|held| held.run == older.name())
        .expect("the re-registered run is held");
    assert_eq!(held.cases, 5, "the later line is the one a reader sees");
}

#[test]
fn test_run_index_appends_the_registry_ci_resolves_latest_run_from() {
    let scratch = Scratch::new("index-document");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let registry = registry_path(&results);
    register_run(&registry, entry_of(&run, started(), BatchVerdict::Pass))
        .expect("registering the run");

    // The registry is append-only: the bytes before the new line are the bytes that were
    // there, and one line holds one entry.
    assert_eq!(mode_of(&registry), 0o600, "the registry is the owner's");
    let text = fs::read_to_string(&registry).expect("the registry is text");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one run, one line");
    let entry: Value = serde_json::from_str(lines[0]).expect("each line is one entry");
    assert_eq!(entry["run"], Value::from(run.name()));
    assert_eq!(
        entry["path"],
        Value::from(format!("{RESULTS_DIR}/{RUNS_DIR}/{}", run.name()))
    );
    assert_eq!(entry["verdict"], Value::from("pass"));
    assert_eq!(entry["cases"], Value::from(1));

    // Appending a second run leaves the first line byte for byte.
    let before = text.clone();
    let later = Utc::from_unix_seconds(1_700_000_060);
    let newer = RunDir::create(&results, later).expect("creating the newer run");
    register_run(&registry, entry_of(&newer, later, BatchVerdict::Fail))
        .expect("registering the newer run");
    let after = fs::read_to_string(&registry).expect("the registry is text");
    assert!(
        after.starts_with(&before),
        "the earlier lines were never rewritten: {after}"
    );
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
fn test_run_index_skips_a_line_it_cannot_parse_and_keeps_the_rest() {
    // Tolerance is the registry's contract: one damaged line is one lost entry, not a lost
    // history. The entries around it are read, the file is left byte for byte as it was
    // found, and appending after the damage still works.
    let scratch = Scratch::new("index-damaged");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let registry = registry_path(&results);
    register_run(&registry, entry_of(&run, started(), BatchVerdict::Pass))
        .expect("registering the run");
    let damaged = format!(
        "{}\n{{ not json at all\n",
        fs::read_to_string(&registry).expect("the registry is text")
    );
    fs::write(&registry, &damaged).expect("the damaged line is planted");

    let index = RunIndex::open(&results).expect("the registry reads around the damage");
    assert_eq!(index.runs().len(), 1, "the healthy entry survives");
    assert_eq!(index.latest_run(), Some(run.name()));
    assert_eq!(
        fs::read_to_string(&registry).expect("the registry is text"),
        damaged,
        "a read never rewrites the registry"
    );

    // A later build with a format this one does not know lands the same way: the line is
    // skipped with a warning, not refused with the history held hostage.
    let later = Utc::from_unix_seconds(1_700_000_060);
    let newer = RunDir::create(&results, later).expect("creating the newer run");
    register_run(&registry, entry_of(&newer, later, BatchVerdict::Fail))
        .expect("registering after damage");
    let index = RunIndex::open(&results).expect("the registry reads");
    assert_eq!(
        index.latest_run(),
        Some(newer.name()),
        "the registry keeps working after the damage"
    );
}

#[test]
fn test_run_index_adopts_the_document_an_older_build_wrote() {
    // The registry's earlier shape was one versioned document holding every run -- written
    // pretty, so no line of it parses on its own. Reading adopts the whole text as the
    // document it is, and the history an older build wrote becomes this build's history.
    let scratch = Scratch::new("index-adopted");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let entry = entry_of(&run, started(), BatchVerdict::Pass);
    let document = format!(
        r#"{{"format": 1, "latest_run": "{}", "runs": [{}]}}"#,
        entry.run,
        serde_json::to_string(&entry).expect("the entry renders")
    );
    fs::create_dir_all(&results).expect("creating the results tree");
    fs::write(registry_path(&results), &document).expect("the old document is planted");

    let index = RunIndex::open(&results).expect("the old document is adopted");
    assert_eq!(
        index.runs().len(),
        1,
        "the document's runs are the registry's"
    );
    assert_eq!(index.latest_run(), Some(run.name()));
    assert_eq!(
        fs::read_to_string(registry_path(&results)).expect("the registry is text"),
        document,
        "adoption reads; it does not rewrite"
    );
}

#[test]
fn test_registering_onto_a_document_migrates_it_without_losing_a_run() {
    // A document has no lines to append after, so the one exception to append-only is the
    // migration: every run the document held becomes a line, and the new entry lands beside
    // them. Every entry survives; only the wrapper changes.
    let scratch = Scratch::new("index-migrate");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    let entry = entry_of(&run, started(), BatchVerdict::Pass);
    let document = format!(
        "{{\"format\": 1, \"latest_run\": \"{}\", \"runs\": [{}]}}\n",
        entry.run,
        serde_json::to_string(&entry).expect("the entry renders")
    );
    fs::create_dir_all(&results).expect("creating the results tree");
    fs::write(registry_path(&results), &document).expect("the old document is planted");

    let later = Utc::from_unix_seconds(1_700_000_060);
    let newer = RunDir::create(&results, later).expect("creating the newer run");
    register_run(
        &registry_path(&results),
        entry_of(&newer, later, BatchVerdict::Fail),
    )
    .expect("registering onto the document");

    let text = fs::read_to_string(registry_path(&results)).expect("the registry is text");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "one line per run: {text}");
    let index = RunIndex::open(&results).expect("the migrated registry reads");
    assert_eq!(index.runs().len(), 2, "the document's run came along");
    assert_eq!(index.latest_run(), Some(newer.name()));

    // The migration ran once: the registry is lines now, and the next registration appends.
    let before = text.clone();
    let later_still = Utc::from_unix_seconds(1_700_000_120);
    let third = RunDir::create(&results, later_still).expect("creating the third run");
    register_run(
        &registry_path(&results),
        entry_of(&third, later_still, BatchVerdict::Pass),
    )
    .expect("registering after the migration");
    let after = fs::read_to_string(registry_path(&results)).expect("the registry is text");
    assert!(
        after.starts_with(&before),
        "the migrated lines were never rewritten"
    );
    assert_eq!(RunIndex::open(&results).expect("reads").runs().len(), 3);
}
