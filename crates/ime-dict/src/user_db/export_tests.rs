//! Tests for the TSV interchange format: what an export writes, and what an import takes.
//!
//! Every test owns its database in a directory of its own under the system temp directory
//! and drives the clock through [`TestClock`], so nothing here depends on a real XDG
//! directory, on the wall clock, or on the state left by another test.

use std::os::unix::fs::PermissionsExt;

use super::export::set_export_limit;
use super::tests::{TestClock, open_in, temp_dir};
use super::*;

/// The header an export of this build writes, as the tests read it back.
const MARKER: &str = "# rspinyin user dictionary export v1";

/// Renders the store's export into a string.
fn document_of(db: &UserDb) -> String {
    let mut out = Vec::new();
    let rows = db.export_tsv(&mut out).expect("exporting");
    assert!(rows > 0, "the fixture has something to export");
    String::from_utf8(out).expect("the document is UTF-8")
}

/// An import document holding `rows`, under this build's marker and column line.
fn document(rows: &str) -> String {
    format!(
        "{MARKER}\n# Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned\n{rows}"
    )
}

#[test]
fn test_export_tsv_writes_the_header_and_every_record() {
    let dir = temp_dir("export-header");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    for key in ["b", "a"] {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");

    let document = document_of(&db);
    let lines: Vec<&str> = document.lines().collect();
    assert_eq!(lines.len(), 5, "three comment lines and two records");
    assert_eq!(lines[0], MARKER);
    assert!(
        lines[1].starts_with("# Columns: key <TAB>"),
        "the columns are documented in the document itself: {}",
        lines[1]
    );
    assert!(lines[2].starts_with('#'));
    // Rows come out in key order, and the creation stamp is the first use.
    assert_eq!(lines[3], "a\t1\t2000\t2000\t0");
    assert_eq!(lines[4], "b\t1\t1000\t1000\t0");
}

#[test]
fn test_export_tsv_carries_a_record_the_flush_has_not_written() {
    let dir = temp_dir("export-pending");
    let db = open_in(&dir, TestClock::default());
    db.record("fresh", 0);

    let document = document_of(&db);
    assert!(
        document.contains("fresh\t1\t0\t0\t0\n"),
        "an unflushed word is part of the export: {document}"
    );
}

#[test]
fn test_export_round_trips_into_an_empty_store() {
    let source_dir = temp_dir("round-trip-source");
    let clock = TestClock::default();
    let mut source = open_in(&source_dir, clock.clone());
    for key in ["ni'hao", "shi'jie", "zhong'guo"] {
        clock.advance_ms(1_000);
        source.record(key, 0);
    }
    source.record("ni'hao", 0);
    source.final_commit().expect("flushing");
    let before = source.record_count().expect("counting");
    let expected: Vec<u32> = ["ni'hao", "shi'jie", "zhong'guo"]
        .iter()
        .map(|key| source.freq(key))
        .collect();
    let document = document_of(&source);
    drop(source);

    let target_dir = temp_dir("round-trip-target");
    let target = open_in(&target_dir, TestClock::default());
    let report = target
        .import_tsv(&mut document.as_bytes())
        .expect("importing a document this build wrote");
    assert!(report.recognised);
    assert_eq!(report.rejected, 0);
    assert_eq!(report.accepted, 3);
    assert_eq!(report.applied, 3);
    assert_eq!(
        target.record_count().expect("counting"),
        before,
        "the round trip kept every record"
    );
    for (key, weight) in ["ni'hao", "shi'jie", "zhong'guo"].iter().zip(expected) {
        assert_eq!(target.freq(key), weight, "{key} kept its count");
    }
}

#[test]
fn test_export_to_file_writes_a_private_file_and_leaves_no_temporary() {
    let dir = temp_dir("export-file");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    clock.advance_ms(1_000);
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");

    let path = dir.join("words.tsv");
    let rows = db.export_to_file(&path).expect("exporting to a file");
    assert_eq!(rows, 1);
    let mode = std::fs::metadata(&path)
        .expect("reading the export's mode")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, FILE_MODE, "an export is as private as the store");
    assert!(
        !dir.join("words.tsv.tmp").exists(),
        "the temporary file is renamed away, not left behind"
    );

    // A second export replaces the file rather than appending to it.
    clock.advance_ms(1_000);
    db.record("shi'jie", 0);
    db.final_commit().expect("flushing");
    assert_eq!(db.export_to_file(&path).expect("exporting again"), 2);
    let written = std::fs::read_to_string(&path).expect("reading the export");
    assert_eq!(written.matches('\n').count(), 5);
    assert!(written.starts_with(MARKER));
}

#[test]
fn test_export_past_the_size_limit_is_refused_without_writing_anything() {
    let dir = temp_dir("export-limit");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    for key in ["a", "b", "c"] {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    let full = document_of(&db).len() as u64;

    set_export_limit(full - 1);
    let mut out = Vec::new();
    let error = db
        .export_tsv(&mut out)
        .expect_err("the document does not fit the ceiling");
    set_export_limit(EXPORT_LIMIT_BYTES);
    assert!(out.is_empty(), "a refused export writes nothing at all");
    match error {
        ImeError::ExportTooLarge { bytes, limit } => {
            assert_eq!(limit, full - 1, "the report names the ceiling in force");
            assert_eq!(bytes, full, "and the size the document would have had");
        }
        other => panic!("expected dict/export-too-large, got {other}"),
    }

    // The file path refuses the same way, and leaves no file and no temporary behind.
    let path = dir.join("words.tsv");
    set_export_limit(full - 1);
    let refused = db
        .export_to_file(&path)
        .expect_err("the file is refused too");
    set_export_limit(EXPORT_LIMIT_BYTES);
    assert!(matches!(refused, ImeError::ExportTooLarge { .. }));
    assert!(!path.exists(), "a refused export leaves no partial file");
    assert!(!dir.join("words.tsv.tmp").exists());
}

#[test]
fn test_export_refuses_a_key_the_format_cannot_carry() {
    let dir = temp_dir("export-bad-key");
    let db = open_in(&dir, TestClock::default());
    {
        let txn = db.inner.db.begin_write().expect("writing");
        {
            let mut words = txn.open_table(USER_WORDS).expect("opening the table");
            words
                .insert("a\tb", (1u32, 1u64))
                .expect("inserting a key the format cannot carry");
        }
        txn.commit().expect("committing");
    }

    let mut out = Vec::new();
    let error = db
        .export_tsv(&mut out)
        .expect_err("a key with a tab in it would break the row it sits in");
    assert!(matches!(error, ImeError::DictCorrupt { .. }));
    assert!(out.is_empty(), "nothing was written before the refusal");
}

#[test]
fn test_import_refuses_a_malformed_row_without_applying_anything() {
    let dir = temp_dir("import-malformed");
    let db = open_in(&dir, TestClock::default());
    let text = document("good\t3\t1000\t900\t0\nbad\t3\t1000\n");

    let report = db
        .import_tsv(&mut text.as_bytes())
        .expect("a refused document is not a store failure");
    assert!(report.recognised, "the marker was there");
    assert_eq!(report.rejected, 1, "the short row is counted");
    assert_eq!(report.accepted, 0);
    assert_eq!(report.applied, 0, "an import is all or nothing");
    assert_eq!(db.record_count().expect("counting"), 0);
    assert_eq!(
        db.freq("good"),
        0,
        "the well-formed row was not applied either"
    );
}

#[test]
fn test_import_refuses_a_document_that_is_not_an_export() {
    let dir = temp_dir("import-foreign");
    let db = open_in(&dir, TestClock::default());
    let text = "hello\tworld\nsecond\trow\n";

    let report = db
        .import_tsv(&mut text.as_bytes())
        .expect("a foreign document is not a store failure");
    assert!(!report.recognised, "the marker was missing");
    assert_eq!(report.rejected, 2);
    assert_eq!(report.applied, 0);
    assert_eq!(db.record_count().expect("counting"), 0);
}

#[test]
fn test_import_refuses_a_document_that_is_not_utf8() {
    let dir = temp_dir("import-binary");
    let db = open_in(&dir, TestClock::default());
    let mut bytes: Vec<u8> = vec![0xff, 0xfe, 0x00];
    bytes.push(b'\n');

    let report = db
        .import_tsv(&mut bytes.as_slice())
        .expect("a binary file is not a store failure");
    assert!(!report.recognised);
    assert_eq!(report.applied, 0);
    assert_eq!(db.record_count().expect("counting"), 0);
}

#[test]
fn test_import_refuses_a_bad_pin_flag_and_a_non_decimal_weight() {
    let dir = temp_dir("import-bad-columns");
    let db = open_in(&dir, TestClock::default());
    let text =
        document("one\t3\t1000\t900\t2\nnegative\t-3\t1000\t900\t0\nfloat\t3.5\t1000\t900\t0\n");

    let report = db
        .import_tsv(&mut text.as_bytes())
        .expect("a refused document is not a store failure");
    assert!(report.recognised);
    assert_eq!(report.rejected, 3, "every row is validated on its own");
    assert_eq!(report.applied, 0);
    assert_eq!(db.record_count().expect("counting"), 0);
}

#[test]
fn test_import_merges_with_what_the_store_already_holds() {
    let dir = temp_dir("import-merge");
    let mut db = open_in(&dir, TestClock::default());
    db.record("word", 0);
    db.record("word", 0);
    db.final_commit().expect("flushing");
    assert_eq!(db.freq("word"), 2);

    let higher = document("word\t9\t5000\t100\t1\n");
    let report = db
        .import_tsv(&mut higher.as_bytes())
        .expect("importing a stronger record");
    assert_eq!(report.applied, 1);
    assert_eq!(db.freq("word"), 9, "the larger weight wins");
    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].weight, 9);
    assert_eq!(rows[0].last_used_unix, 5000, "the later use wins");
    assert_eq!(rows[0].created_unix, 100, "the earlier creation wins");
    assert!(rows[0].pinned, "and the pin is carried over");

    let lower = document("word\t1\t10\t100\t0\n");
    db.import_tsv(&mut lower.as_bytes())
        .expect("importing a weaker record");
    assert_eq!(
        db.freq("word"),
        9,
        "an import never lowers a count the user earned by typing"
    );
    let rows = db.list_words(0, 10).expect("listing");
    assert_eq!(rows[0].created_unix, 100);
    assert!(rows[0].pinned, "and a pin is never taken away");
}

#[test]
fn test_import_into_a_readonly_store_reports_readonly() {
    let dir = temp_dir("import-readonly");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    inject_failure();
    let _ = db.final_commit().expect_err("the injected flush fails");
    assert!(db.is_readonly(), "the store degraded");

    let text = document("word\t3\t1000\t900\t0\n");
    let error = db
        .import_tsv(&mut text.as_bytes())
        .expect_err("a read-only store takes nothing");
    assert!(matches!(error, ImeError::DataReadonly { .. }));
    assert!(
        error.to_string().starts_with("data/readonly-mode"),
        "the diagnostic carries the frozen code: {error}"
    );
    assert_eq!(db.freq("word"), 0);
}

#[test]
fn test_import_refuses_a_document_past_the_size_limit() {
    let dir = temp_dir("import-limit");
    let db = open_in(&dir, TestClock::default());
    let text = document("word\t3\t1000\t900\t0\n");

    set_export_limit(16);
    let error = db
        .import_tsv(&mut text.as_bytes())
        .expect_err("the document is larger than the format's ceiling");
    set_export_limit(EXPORT_LIMIT_BYTES);
    match error {
        ImeError::ExportTooLarge { bytes, limit } => {
            assert!(bytes > limit);
            assert_eq!(limit, 16);
        }
        other => panic!("expected dict/export-too-large, got {other}"),
    }
    assert_eq!(db.record_count().expect("counting"), 0);
}
