//! Unit tests for the schema migration framework.
//!
//! Responsibility: pin what bringing an older `config.toml` forward does -- which keys a
//! step adds, which keys it leaves alone, where the file the user had is kept, and every
//! document the framework refuses.
//!
//! Boundaries: every test names the file it owns and lives under the system temp
//! directory, so nothing here reads `$HOME` or a real XDG directory. The chain is driven
//! through [`run`] with an explicit target version, so a test does not depend on what this
//! build's own schema version happens to be.

use std::env;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::reload::FILE_NAME;
use crate::schema::MAX_DOCUMENT_KEYS;

/// A scratch directory for one test, named after the test so two tests never share one.
fn scratch(name: &str) -> PathBuf {
    let directory = env::temp_dir().join(format!("rspinyin-ime-config-migrate-{name}"));
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    directory
}

/// Writes `text` to `path`.
fn write(path: &Path, text: &str) {
    assert!(fs::write(path, text).is_ok(), "writing {path:?}");
}

/// The text of `path`, or an empty string when it cannot be read; the caller's assertion
/// is what fails the test.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Parses `text` as a configuration document.
fn document(text: &str) -> toml::Value {
    match toml::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("the test's own document is not TOML: {error}"),
    }
}

/// The value at a dotted key of a document, or `None` when the path is not there.
fn at<'a>(doc: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    let mut current = doc;
    for step in path.split('.') {
        current = current.as_table()?.get(step)?;
    }
    Some(current)
}

/// A version-1 document that carries a value a migration has to preserve and a key no
/// version knows, so that both are checked on every run.
const V1_DOCUMENT: &str = "\
schema_version = 1\n\
[ui]\n\
max_per_row = 7\n\
[engine]\n\
punct_mode = \"english\"\n\
[future]\n\
unknown_key = 5\n";

/// A step whose behaviour the test chooses, so that the framework's own rules can be
/// exercised without a real version to migrate to.
struct FakeStep {
    /// The version the step reads.
    from: u16,
    /// The version the step produces.
    to: u16,
    /// What the step does to the document.
    behaviour: Behaviour,
}

/// What a [`FakeStep`] does to the document it is handed.
enum Behaviour {
    /// Leaves the document alone.
    Nothing,
    /// Removes a top-level key.
    Remove(&'static str),
    /// Reports one diagnostic and changes nothing.
    Report,
    /// Fails.
    Fail,
}

impl MigrationStep for FakeStep {
    fn source_version(&self) -> u16 {
        self.from
    }

    fn target_version(&self) -> u16 {
        self.to
    }

    fn apply(&self, doc: &mut toml::Value) -> Result<Vec<ImeError>, ConfigError> {
        match self.behaviour {
            Behaviour::Nothing => {}
            Behaviour::Remove(key) => {
                if let Some(table) = doc.as_table_mut() {
                    table.remove(key);
                }
            }
            Behaviour::Report => {
                return Ok(vec![ImeError::ConfigInvalid {
                    key: String::from("engine"),
                    reason: String::from("the fake step has something to say"),
                }]);
            }
            Behaviour::Fail => {
                return Err(ConfigError::Invalid {
                    key: String::from("engine"),
                    reason: String::from("the fake step failed"),
                });
            }
        }
        Ok(Vec::new())
    }
}

/// The `config/invalid` key a failure names, or an empty string when it is another kind of
/// failure; the caller's assertion is what fails the test.
fn rejected(result: &Result<Option<MigrationReport>, ConfigError>) -> String {
    match result {
        Err(ConfigError::Invalid { key, .. }) => key.clone(),
        Err(other) => format!("unexpected failure: {other}"),
        Ok(_) => String::from("no failure"),
    }
}

/// Sets the permission bits of `path`.
fn set_mode(path: &Path, mode: u32) {
    assert!(
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).is_ok(),
        "setting the mode"
    );
}

/// Whether the process may create a file in `dir`.
///
/// Asked by writing, because the mode bits cannot answer it for a process the mode does
/// not bind -- which is what a test run as root is.
fn writable(dir: &Path) -> bool {
    let probe = dir.join("write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[test]
fn test_migrate_v1_document_adds_every_version_two_key() {
    let path = scratch("adds").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path).expect("a version-1 document migrates");
    let report = report.expect("a document that was migrated is reported");

    assert_eq!(report.from, 1);
    assert_eq!(report.to, 2);
    assert_eq!(
        at(&doc, "phrases.enabled"),
        Some(&toml::Value::Boolean(true))
    );
    assert_eq!(
        at(&doc, "phrases.file"),
        Some(&toml::Value::String(String::new()))
    );
    assert_eq!(
        at(&doc, "phrases.max_entries"),
        Some(&toml::Value::Integer(5_000))
    );
    assert_eq!(
        at(&doc, "scheme.scheme"),
        Some(&toml::Value::String(String::from("full")))
    );
    assert_eq!(
        at(&doc, "scheme.show_hint"),
        Some(&toml::Value::Boolean(true))
    );
    assert_eq!(
        at(&doc, "scheme.keep_full_pinyin"),
        Some(&toml::Value::Boolean(true))
    );
    assert_eq!(
        at(&doc, "script.enabled"),
        Some(&toml::Value::Boolean(false))
    );
    assert_eq!(
        at(&doc, "script.traditional"),
        Some(&toml::Value::Boolean(false))
    );
    assert_eq!(
        at(&doc, "script.hotkey"),
        Some(&toml::Value::String(String::from("ctrl+shift+f")))
    );
    assert_eq!(
        at(&doc, "data.backup_enabled"),
        Some(&toml::Value::Boolean(true))
    );
    assert_eq!(at(&doc, "data.backup_keep"), Some(&toml::Value::Integer(3)));
    assert_eq!(
        at(&doc, "data.export_dir"),
        Some(&toml::Value::String(String::new()))
    );
}

#[test]
fn test_migrate_v1_document_keeps_every_version_one_value() {
    let path = scratch("keeps").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a version-1 document migrates")
        .expect("a document that was migrated is reported");

    assert_eq!(at(&doc, "ui.max_per_row"), Some(&toml::Value::Integer(7)));
    assert_eq!(
        at(&doc, "engine.punct_mode"),
        Some(&toml::Value::String(String::from("english")))
    );
    // A key no version of this build knows survives the rewrite, which is what lets a
    // user go back to an older build and still find their settings.
    assert_eq!(
        at(&doc, "future.unknown_key"),
        Some(&toml::Value::Integer(5))
    );
    assert!(
        report.dropped.is_empty(),
        "nothing version 1 could hold is dropped: {:?}",
        report.dropped
    );
}

#[test]
fn test_migrate_v1_document_raises_the_schema_version() {
    let path = scratch("version").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    assert!(
        run(STEPS, 2, &mut doc, &path).is_ok(),
        "a version-1 document migrates"
    );
    assert_eq!(at(&doc, SCHEMA_VERSION_KEY), Some(&toml::Value::Integer(2)));
    assert!(
        read(&path).contains("schema_version = 2"),
        "the file on the disk states the version it is now at"
    );
}

#[test]
fn test_migrate_v1_document_keeps_the_original_verbatim() {
    let directory = scratch("original");
    let path = directory.join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a version-1 document migrates")
        .expect("a document that was migrated is reported");

    assert_eq!(
        report.backup,
        Some(directory.join(format!("{FILE_NAME}.v1"))),
        "the original is kept beside the file it came from"
    );
    assert_eq!(
        read(&directory.join(format!("{FILE_NAME}.v1"))),
        V1_DOCUMENT,
        "the copy is the bytes the user had, comments and key order included"
    );
    assert_ne!(
        read(&path),
        V1_DOCUMENT,
        "the file itself is the migrated document"
    );
    assert!(read(&path).contains("schema_version = 2"));
}

#[test]
fn test_migrate_v1_document_reports_the_keys_it_added() {
    let path = scratch("added").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a version-1 document migrates")
        .expect("a document that was migrated is reported");

    assert_eq!(
        report.added,
        [
            String::from("data.backup_enabled = true"),
            String::from("data.backup_keep = 3"),
            String::from("data.export_dir = \"\""),
            String::from("phrases.enabled = true"),
            String::from("phrases.file = \"\""),
            String::from("phrases.max_entries = 5000"),
            String::from("scheme.keep_full_pinyin = true"),
            String::from("scheme.scheme = \"full\""),
            String::from("scheme.show_hint = true"),
            String::from("script.enabled = false"),
            String::from("script.hotkey = \"ctrl+shift+f\""),
            String::from("script.traditional = false"),
        ],
        "every added key is named with the default it was given"
    );
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

#[test]
fn test_migrate_is_idempotent_on_its_own_output() {
    let path = scratch("idempotent").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    assert!(
        run(STEPS, 2, &mut doc, &path).is_ok(),
        "the first call migrates"
    );
    let migrated = read(&path);

    let second = run(STEPS, 2, &mut doc, &path).expect("a second call succeeds");
    assert!(
        second.is_none(),
        "a document already at version 2 is a no-op"
    );
    assert_eq!(read(&path), migrated, "the file is not rewritten");
    assert!(
        !path.with_file_name(format!("{FILE_NAME}.v2")).exists(),
        "a no-op keeps no second copy"
    );
}

#[test]
fn test_migrate_document_at_the_current_version_is_a_no_op() {
    let path = scratch("current").join(FILE_NAME);
    let text = format!("schema_version = {CONFIG_SCHEMA_VERSION}\n[ui]\nmax_per_row = 7\n");
    write(&path, &text);
    let mut doc = document(&text);

    let report = migrate(&mut doc, &path).expect("a document at this build's version is fine");
    assert!(report.is_none(), "there is nothing to migrate");
    assert_eq!(read(&path), text, "the file is not touched");
}

#[test]
fn test_migrate_document_without_a_schema_version_is_a_no_op() {
    let path = scratch("no-version").join(FILE_NAME);
    let text = "[ui]\nmax_per_row = 7\n";
    write(&path, text);
    let mut doc = document(text);

    let report = migrate(&mut doc, &path).expect("a document with no version is read as current");
    assert!(report.is_none(), "there is nothing to migrate");
    assert_eq!(read(&path), text, "the file is not touched");
}

#[test]
fn test_migrate_refuses_a_newer_schema_version() {
    let path = scratch("newer").join(FILE_NAME);
    let newer = CONFIG_SCHEMA_VERSION + 1;
    let text = format!("schema_version = {newer}\n[ui]\nmax_per_row = 7\n");
    write(&path, &text);
    let mut doc = document(&text);

    let result = migrate(&mut doc, &path);
    assert_eq!(
        rejected(&result),
        SCHEMA_VERSION_KEY,
        "a newer document is refused rather than coerced down"
    );
    assert_eq!(
        read(&path),
        text,
        "the file a newer build wrote is left alone"
    );
}

#[test]
fn test_migrate_refuses_a_schema_version_that_is_not_an_integer() {
    let path = scratch("version-not-a-number").join(FILE_NAME);
    let text = "schema_version = \"one\"\n";
    write(&path, text);
    let mut doc = document(text);

    let result = migrate(&mut doc, &path);
    assert_eq!(rejected(&result), SCHEMA_VERSION_KEY);
    assert_eq!(read(&path), text, "the file is left alone");
}

#[test]
fn test_migrate_refuses_a_schema_version_too_wide_for_the_field() {
    let path = scratch("version-too-wide").join(FILE_NAME);
    // Wider than the version field: a cast would truncate it into a version the document
    // does not mean.
    let text = "schema_version = 70000\n";
    write(&path, text);
    let mut doc = document(text);

    let result = migrate(&mut doc, &path);
    assert_eq!(rejected(&result), SCHEMA_VERSION_KEY);
    assert_eq!(read(&path), text, "the file is left alone");
}

#[test]
fn test_migrate_refuses_a_document_that_is_not_a_table() {
    let path = scratch("not-a-table").join(FILE_NAME);
    let mut doc = toml::Value::Integer(1);

    let result = migrate(&mut doc, &path);
    assert_eq!(rejected(&result), DOCUMENT_KEY);
    assert_eq!(doc, toml::Value::Integer(1), "the document is not touched");
}

#[test]
fn test_migrate_refuses_a_chain_with_no_step_for_a_version() {
    let path = scratch("no-step").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    // Nothing reads version 2, so version 3 cannot be reached from version 1.
    let result = run(STEPS, 3, &mut doc, &path);
    assert_eq!(rejected(&result), SCHEMA_VERSION_KEY);
    assert_eq!(
        at(&doc, SCHEMA_VERSION_KEY),
        Some(&toml::Value::Integer(1)),
        "a chain that cannot be walked leaves the document where it was"
    );
}

#[test]
fn test_migrate_refuses_a_step_that_skips_a_version() {
    let path = scratch("skipping-step").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);
    let skipping = FakeStep {
        from: 1,
        to: 3,
        behaviour: Behaviour::Nothing,
    };
    let steps: &[&dyn MigrationStep] = &[&skipping];

    let result = run(steps, 3, &mut doc, &path);
    assert_eq!(rejected(&result), SCHEMA_VERSION_KEY);
}

#[test]
fn test_migrate_leaves_the_document_alone_when_a_step_fails() {
    let path = scratch("failing-step").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);
    let failing = FakeStep {
        from: 1,
        to: 2,
        behaviour: Behaviour::Fail,
    };
    let steps: &[&dyn MigrationStep] = &[&failing];

    let result = run(steps, 2, &mut doc, &path);
    assert_eq!(rejected(&result), "engine");
    assert_eq!(
        doc,
        document(V1_DOCUMENT),
        "a step that fails half way leaves no half-migrated document behind"
    );
    assert_eq!(read(&path), V1_DOCUMENT, "and no file was written");
    assert!(!path.with_file_name(format!("{FILE_NAME}.v1")).exists());
}

#[test]
fn test_migrate_reports_a_key_a_step_could_not_carry_over() {
    let path = scratch("dropping-step").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);
    let dropping = FakeStep {
        from: 1,
        to: 2,
        behaviour: Behaviour::Remove("future"),
    };
    let steps: &[&dyn MigrationStep] = &[&dropping];

    let report = run(steps, 2, &mut doc, &path)
        .expect("the step succeeds")
        .expect("a document that was migrated is reported");

    assert_eq!(
        report.dropped,
        [String::from("future.unknown_key")],
        "a step cannot drop a setting without it being named"
    );
}

#[test]
fn test_migrate_collects_a_step_diagnostic_into_the_notes() {
    let path = scratch("reporting-step").join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);
    let reporting = FakeStep {
        from: 1,
        to: 2,
        behaviour: Behaviour::Report,
    };
    let steps: &[&dyn MigrationStep] = &[&reporting];

    let report = run(steps, 2, &mut doc, &path)
        .expect("the step succeeds")
        .expect("a document that was migrated is reported");

    assert_eq!(
        report.notes,
        [String::from(
            "config/invalid: engine (the fake step has something to say)"
        )],
        "what a step reports is carried in the notes under its stable code"
    );
}

#[test]
fn test_migrate_reports_a_section_that_is_not_a_table() {
    let path = scratch("section-not-a-table").join(FILE_NAME);
    let text = "schema_version = 1\n[data]\ndurability = \"immediate\"\n";
    write(&path, text);
    let mut doc = document(text);
    // A section the user wrote as a value rather than a table. The migration must not
    // overwrite it, and must not fail the whole run over one section.
    if let Some(table) = doc.as_table_mut() {
        table.insert(
            String::from("scheme"),
            toml::Value::String(String::from("not a table")),
        );
    }

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("one unusable section does not fail the migration")
        .expect("a document that was migrated is reported");

    assert_eq!(
        at(&doc, "scheme"),
        Some(&toml::Value::String(String::from("not a table"))),
        "the user's value is left exactly as it was"
    );
    assert_eq!(
        at(&doc, "data.durability"),
        Some(&toml::Value::String(String::from("immediate"))),
        "the sections that could be read were still migrated"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("scheme") && note.contains("config/invalid")),
        "{:?}",
        report.notes
    );
}

#[test]
fn test_migrate_unwritable_directory_keeps_the_migration_in_memory() {
    let directory = scratch("readonly");
    let path = directory.join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);
    set_mode(&directory, 0o500);
    if writable(&directory) {
        // A process the mode does not bind -- root -- cannot build this case. The
        // missing-file case below reaches the same outcome unconditionally.
        set_mode(&directory, 0o700);
        return;
    }

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a directory that cannot be written to is not a failure")
        .expect("the migration still happened");
    set_mode(&directory, 0o700);

    assert_eq!(report.backup, None, "nothing was kept");
    assert_eq!(
        at(&doc, SCHEMA_VERSION_KEY),
        Some(&toml::Value::Integer(2)),
        "the caller runs on the migrated document"
    );
    assert!(
        !report.notes.is_empty(),
        "the caller has to be able to record that nothing was written"
    );
    assert_eq!(read(&path), V1_DOCUMENT, "the file is left as it was");
}

#[test]
fn test_migrate_document_without_a_file_migrates_in_memory_only() {
    let path = scratch("no-file").join(FILE_NAME);
    let mut doc = document(V1_DOCUMENT);

    // The document was parsed from somewhere other than this path: there is nothing to
    // keep and nothing to replace, and neither is a failure.
    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a document with no file behind it still migrates")
        .expect("a document that was migrated is reported");

    assert_eq!(report.backup, None, "there was no file to keep");
    assert_eq!(at(&doc, SCHEMA_VERSION_KEY), Some(&toml::Value::Integer(2)));
    assert_eq!(
        report.notes.len(),
        1,
        "the caller is told that nothing was written: {:?}",
        report.notes
    );
    assert!(!path.exists(), "no file is created where there was none");
}

#[test]
fn test_migrate_a_document_at_the_key_limit_still_migrates() {
    // A document that is already as large as the loader allows is carried forward: the
    // migration has no key limit of its own, and whether the result is over the limit is
    // the loader's to report once the document is read again. The document is built from
    // the constant rather than from a fixture, so the test says nothing about the limit's
    // value.
    let mut text = String::from("schema_version = 1\n");
    for number in 0..MAX_DOCUMENT_KEYS {
        text.push_str(&format!("k{number} = {number}\n"));
    }
    let path = scratch("key-limit").join(FILE_NAME);
    write(&path, &text);
    let mut doc = document(&text);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a document at the key limit still migrates")
        .expect("a document that was migrated is reported");

    assert_eq!(report.from, 1);
    assert_eq!(report.to, 2);
    assert_eq!(
        report.added.len(),
        12,
        "what the migration adds does not depend on how large the document is"
    );
    assert!(
        report.dropped.is_empty(),
        "no key of a large document is lost: {:?}",
        report.dropped
    );
    assert_eq!(
        at(&doc, "k0"),
        Some(&toml::Value::Integer(0)),
        "the document's own keys are still there"
    );
    let last = format!("k{}", MAX_DOCUMENT_KEYS - 1);
    assert!(
        at(&doc, &last).is_some(),
        "the last key of a document at the limit survives: {last}"
    );
}

#[test]
fn test_migrate_keeps_both_originals_when_the_backup_name_is_taken() {
    let directory = scratch("taken-name");
    let path = directory.join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let earlier = directory.join(format!("{FILE_NAME}.v1"));
    write(&earlier, "the copy an earlier migration kept\n");
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a name that is taken costs a suffix, not a file")
        .expect("a document that was migrated is reported");

    assert_eq!(
        report.backup,
        Some(directory.join(format!("{FILE_NAME}.v1.1")))
    );
    assert_eq!(
        read(&earlier),
        "the copy an earlier migration kept\n",
        "the earlier copy is not overwritten"
    );
    assert_eq!(
        read(&directory.join(format!("{FILE_NAME}.v1.1"))),
        V1_DOCUMENT
    );
}

#[test]
fn test_migrate_backup_is_readable_only_by_its_owner() {
    let directory = scratch("backup-mode");
    let path = directory.join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    let mut doc = document(V1_DOCUMENT);

    let report = run(STEPS, 2, &mut doc, &path)
        .expect("a version-1 document migrates")
        .expect("a document that was migrated is reported");

    let backup = report.backup.expect("the original was kept");
    let mode = fs::metadata(&backup)
        .expect("the backup exists")
        .permissions();
    assert_eq!(
        mode.mode() & 0o777,
        0o600,
        "the copy of the user's configuration is readable by its owner alone"
    );
}

#[test]
fn test_migrate_keeps_the_mode_of_the_file_it_replaces() {
    let directory = scratch("kept-mode");
    let path = directory.join(FILE_NAME);
    write(&path, V1_DOCUMENT);
    set_mode(&path, 0o640);
    let mut doc = document(V1_DOCUMENT);

    assert!(run(STEPS, 2, &mut doc, &path).is_ok());

    let mode = fs::metadata(&path).expect("the file exists").permissions();
    assert_eq!(
        mode.mode() & 0o777,
        0o640,
        "a migration is not a reason for a restricted configuration to become readable"
    );
}

#[test]
fn test_migrate_leaves_an_unknown_top_level_key_alone() {
    let path = scratch("unknown-key").join(FILE_NAME);
    let text = "schema_version = 1\nsomething_new = \"kept\"\n";
    write(&path, text);
    let mut doc = document(text);

    assert!(run(STEPS, 2, &mut doc, &path).is_ok());
    assert_eq!(
        at(&doc, "something_new"),
        Some(&toml::Value::String(String::from("kept")))
    );
    assert!(
        read(&path).contains("something_new = \"kept\""),
        "the key a build this old does not know is written back out"
    );
}

#[test]
fn test_migration_step_v1_to_v2_refuses_a_document_of_another_version() {
    let mut doc = document("schema_version = 2\n[ui]\nmax_per_row = 7\n");

    let result = V1_TO_V2.apply(&mut doc);
    assert!(
        matches!(
            result,
            Err(ConfigError::Invalid { ref key, .. }) if key == SCHEMA_VERSION_KEY
        ),
        "{result:?}"
    );
}

#[test]
fn test_migration_step_v1_to_v2_refuses_a_document_without_a_version() {
    let mut doc = document("[ui]\nmax_per_row = 7\n");

    let result = V1_TO_V2.apply(&mut doc);
    assert!(
        matches!(
            result,
            Err(ConfigError::Invalid { ref key, .. }) if key == SCHEMA_VERSION_KEY
        ),
        "{result:?}"
    );
}

#[test]
fn test_steps_are_ordered_and_contiguous() {
    assert!(!STEPS.is_empty(), "a build reads at least one version");
    assert_eq!(
        STEPS[0].source_version(),
        1,
        "the chain starts at version 1"
    );

    let mut expected = STEPS[0].source_version();
    for step in STEPS {
        assert_eq!(
            step.source_version(),
            expected,
            "a chain with a gap cannot be walked"
        );
        assert_eq!(step.target_version(), expected + 1);
        expected = step.target_version();
    }
    assert_eq!(
        expected, 2,
        "version 2 is the version this chain was written for"
    );
}
