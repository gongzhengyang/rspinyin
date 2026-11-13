//! Unit tests for the retired-key list and its prune.
//!
//! Responsibility: pin the guards the mechanism is delivered with -- the coverage of the
//! shipped template against the shipped list, the idempotence of the prune, the
//! boundary paths that lead nowhere, the ordering rule that a value migration is
//! persisted before any key is pruned, and the round trip of a document that carries a
//! retired key through load, prune and write-back without residue.
//!
//! Boundaries: the shipped list of this build is empty, so every test that exercises
//! the pruning itself drives [`prune_with`] over a key the test retires for its own
//! duration, and the tests over the shipped entry point pin that the empty list is
//! exactly nothing. Every test names the file it owns under the system temp directory,
//! so nothing here reads `$HOME` or the clock.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::retired::{prune_retired, prune_with};
use super::{DEFAULT_CONFIG_TOML, FILE_NAME};
use crate::migrate;
use crate::schema::{Config, PunctMode};

/// The list one test retires for its own duration.
///
/// `engine.old_switch` is a key no build of this crate has ever read, so retiring it
/// exercises the mechanism without touching a real key: the shipped list stays empty,
/// and the fixture key is a dead spelling by construction.
const TEST_RETIRED: &[&[&str]] = &[&["engine", "old_switch"]];

/// A version-1 document whose values the migration must carry forward, with a key the
/// test retires beside them.
const V1_DOCUMENT: &str = "schema_version = 1\n\
                           [engine]\n\
                           punct_mode = \"english\"\n\
                           old_switch = true\n";

/// A scratch directory for one test, named after the test so two tests never share one.
fn scratch(name: &str) -> PathBuf {
    let directory = env::temp_dir().join(format!("rspinyin-ime-config-retired-{name}"));
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    directory
}

/// Writes `text` to `path`.
fn write(path: &Path, text: &str) {
    assert!(fs::write(path, text).is_ok(), "writing {path:?}");
}

/// The text of `path`, or an empty string when it cannot be read; the caller's
/// assertion is what fails the test.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// The document `text` parses to.
fn parse(text: &str) -> toml::Value {
    toml::from_str(text).expect("a legal document")
}

#[test]
fn test_prune_retired_default_template_removes_nothing() {
    // The coverage guard: the template is the documentation of every key a user may
    // write, so a key the list retires and the template still spells would hand every
    // fresh install a dead key for the next load to prune away. With the list empty the
    // guard pins the other half as well -- nothing in the template can have been
    // retired behind this build's back, because this build retires nothing.
    let mut document = parse(DEFAULT_CONFIG_TOML);

    assert_eq!(prune_retired(&mut document), 0);
}

#[test]
fn test_prune_retired_unknown_deep_keys_leaves_the_document_unchanged() {
    // The empty-list form of the coverage guard: keys unknown to every version of this
    // build, nested more deeply than any real section, are carried and never pruned. A
    // prune that walked the document instead of the list would show up as a change
    // here.
    let text = "schema_version = 2\n\
                [engine]\npunct_mode = \"english\"\nfuture_flag = true\n\
                [theme]\naccent = \"#112233\"\n\
                [unknown_section]\n\
                [unknown_section.deeper_still]\nunknown_key = 5\n";
    let mut document = parse(text);
    let before = document.clone();

    assert_eq!(prune_retired(&mut document), 0);
    assert_eq!(document, before, "the prune changed nothing");
}

#[test]
fn test_prune_retired_twice_second_pass_removes_nothing() {
    // Idempotence: the first prune removes the listed key, the second finds nothing
    // left and the document stands as the first prune left it, so a second load or a
    // reload of the same file cannot churn.
    let text = "schema_version = 2\n\
                [engine]\nold_switch = true\npunct_mode = \"english\"\n";
    let mut document = parse(text);

    assert_eq!(prune_with(TEST_RETIRED, &mut document), 1);
    let once = document.clone();
    assert_eq!(prune_with(TEST_RETIRED, &mut document), 0);
    assert_eq!(document, once, "the second prune changed nothing");
}

#[test]
fn test_prune_retired_a_path_that_leads_nowhere_is_left_alone() {
    // The boundary: a path whose middle segment is a value that is not a table, and a
    // path whose first section the document does not have. Both name a key that cannot
    // exist, both prune nothing, and neither touches what the document holds where the
    // walk stopped.
    let text = "schema_version = 2\n[engine]\npunct_mode = \"english\"\n";
    let mut document = parse(text);
    let before = document.clone();
    let probe: &[&[&str]] = &[
        &["engine", "punct_mode", "sub"],
        &["missing", "section", "key"],
    ];

    assert_eq!(prune_with(probe, &mut document), 0);
    assert_eq!(document, before, "the walk left the document as it was");
}

#[test]
fn test_prune_retired_runs_after_the_migration_has_persisted() {
    // The ordering rule, driven in the pipeline's own order: migrate first, prune
    // second. The migration of a version-1 document that names a file persists the
    // migrated document before it returns, so the carried values are on the disk
    // before the prune runs -- and the prune, which changes the document and never the
    // file, cannot take them back out of it. An interrupted run therefore finds the
    // values on the disk whatever it was interrupted by.
    let path = scratch("order").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut document = parse(V1_DOCUMENT);

    migrate::migrate(&mut document, &path)
        .expect("a version-1 document migrates")
        .expect("a document that was migrated is reported");

    let on_disk = read(&path);
    assert!(
        on_disk.contains("punct_mode = \"english\""),
        "the carried value is on the disk before the prune runs: {on_disk}"
    );
    assert!(
        on_disk.contains("schema_version = 2"),
        "the migrated version is on the disk before the prune runs"
    );
    assert!(
        on_disk.contains("old_switch"),
        "the key the prune will take is still there before the prune runs"
    );

    assert_eq!(prune_with(TEST_RETIRED, &mut document), 1);
    assert_eq!(
        document
            .get("engine")
            .and_then(|engine| engine.get("punct_mode")),
        Some(&toml::Value::String(String::from("english"))),
        "the value the migration carried survives the prune in the document"
    );
    assert!(
        read(&path).contains("punct_mode = \"english\""),
        "the prune changes the document, never the file the migration wrote"
    );
}

#[test]
fn test_prune_retired_round_trip_load_prune_write_back_leaves_no_residue() {
    // The fixture round trip: a file that spells a retired key is loaded, the key is
    // pruned, the document is written back, and the file -- and every load of it after
    // -- holds no trace of the key while the key beside it keeps the value it had. The
    // resolution comparison is the invariant the mechanism is delivered under: pruning
    // changes nothing but the dead spelling itself.
    let path = scratch("round-trip").join(FILE_NAME);
    let text = "schema_version = 2\n\
                [engine]\nold_switch = true\npunct_mode = \"english\"\n";
    write(&path, text);

    // Loaded, as the loader parses the file, and resolved, as the loader answers it.
    // The retired key is ignored on this side of the prune too, so the resolution is
    // the exact baseline the prune must not move.
    let mut document = parse(&read(&path));
    let (before, _) = Config::from_document(text).expect("a legal document");

    assert_eq!(prune_with(TEST_RETIRED, &mut document), 1);

    let pruned = toml::to_string(&document).expect("a pruned document renders");
    write(&path, &pruned);

    let (after, warnings) = Config::load_at(&path, 7);
    assert!(
        warnings.is_empty(),
        "the written-back file reads clean: {warnings:?}"
    );
    assert_eq!(
        after, before,
        "load(prune(doc)) resolves to load(doc), key for key"
    );
    assert_eq!(
        after.engine.punct_mode,
        PunctMode::English,
        "the key beside the retired one keeps its value"
    );
    assert!(
        !read(&path).contains("old_switch"),
        "no residue of the retired key is left in the file"
    );
}
