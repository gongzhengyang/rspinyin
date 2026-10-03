//! The record cap: how many crash records the crash directory keeps.
//!
//! The crash channel writes one record file per failure, so a panic loop must be bounded
//! by the channel itself and not by anything that runs outside it. These tests drive the
//! public write path the way a panic does -- one record after another, timestamps in
//! rising order -- and assert that the directory the failures fill stays bounded, that
//! what survives is the newest of what was written, and that a file that is not a record
//! is never touched by the prune.
//!
//! Every directory here lives under `/tmp` rather than `std::env::temp_dir()`, so that no
//! test depends on `$TMPDIR`; the timestamps are fixed values, so none depends on a clock.

use std::path::{Path, PathBuf};

use ime_diag::crash::context::CrashContext;
use ime_diag::crash::record::{
    CrashRecord, FILE_SUFFIX, MAX_CRASH_RECORDS, crash_file_name, prune_records, write_record,
};

/// A scratch root this file owns, one directory per test.
///
/// The lint is silenced here and not by inlining: the scratch setup is shared by every
/// test in the file, and an `expect` in a helper between `#[test]` functions is outside
/// the exemption clippy grants the annotated bodies themselves.
#[allow(clippy::expect_used)]
fn scratch_root(label: &str) -> PathBuf {
    let name = format!("rspinyin-crash-cap-{}-{label}", std::process::id());
    let root = PathBuf::from("/tmp").join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("creating the scratch root");
    root
}

/// A record stamped at `timestamp_unix_ms`, with no other content that varies.
fn record_at(timestamp_unix_ms: u64) -> CrashRecord {
    CrashRecord {
        timestamp_unix_ms,
        thread_name: String::from("test"),
        thread_id: 1,
        location: None,
        payload: String::from("a deliberate failure"),
        backtrace: String::from("   0: frame\n"),
        context: CrashContext::new(),
    }
}

/// The record-shaped files of `dir`, as sorted names.
///
/// The shape, not the suffix, is what is counted: a foreign file that happens to end in
/// `.txt` must not inflate the count a test asserts on. See `scratch_root` for why the
/// lint is silenced on the helper rather than dodged.
#[allow(clippy::expect_used)]
fn record_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("listing the crash directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| is_record_shaped(name))
        .collect();
    names.sort();
    names
}

/// Whether `name` has the `<timestamp>-<thread id>.txt` shape a record is written under.
fn is_record_shaped(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(FILE_SUFFIX) else {
        return false;
    };
    let Some((stamp, rest)) = stem.split_once('-') else {
        return false;
    };
    !stamp.is_empty()
        && !rest.is_empty()
        && stamp.bytes().all(|byte| byte.is_ascii_digit())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[test]
fn test_write_record_keeps_the_directory_at_the_record_limit() {
    let root = scratch_root("write-cap");
    let dir = root.join("crash");

    // One more record than the cap allows, each a millisecond after the last: the shape a
    // panic loop produces, injected here without a single real panic.
    let injections = MAX_CRASH_RECORDS + 1;
    for stamp in 1..=injections as u64 {
        write_record(&dir, &record_at(stamp)).expect("writing an injected record");
    }

    let names = record_names(&dir);
    assert_eq!(
        names.len(),
        MAX_CRASH_RECORDS,
        "{injections} injections must leave exactly {MAX_CRASH_RECORDS} records: {names:?}"
    );
    assert!(
        !names.contains(&crash_file_name(1, 1)),
        "the oldest record is the one the cap removes: {names:?}"
    );
    assert!(
        names.contains(&crash_file_name(injections as u64, 1)),
        "the newest record survives its own prune: {names:?}"
    );
}

#[test]
fn test_prune_records_keeps_the_newest_and_leaves_foreign_files_alone() {
    let root = scratch_root("prune-newest");
    let dir = root.join("crash");
    std::fs::create_dir_all(&dir).expect("creating the crash directory");

    // Seeded directly rather than through `write_record`, so the stamps the test asserts
    // on are the names' own and not the clock's.
    let seeded = (1..=(MAX_CRASH_RECORDS + 2) as u64)
        .map(|stamp| {
            let path = dir.join(crash_file_name(stamp, 1));
            std::fs::write(&path, b"rspinyin crash record\n").expect("seeding a record");
            crash_file_name(stamp, 1)
        })
        .collect::<Vec<_>>();
    let foreign = dir.join("notes.txt");
    std::fs::write(&foreign, b"not a record").expect("seeding a foreign file");

    prune_records(&dir);

    let names = record_names(&dir);
    assert_eq!(
        names.len(),
        MAX_CRASH_RECORDS,
        "the prune keeps the newest {MAX_CRASH_RECORDS} records: {names:?}"
    );
    for removed in seeded.iter().take(2) {
        assert!(
            !names.contains(removed),
            "the two oldest records go first: {removed}"
        );
    }
    assert!(
        foreign.is_file(),
        "a file that is not a record is never pruned"
    );
}

#[test]
fn test_prune_records_leaves_a_directory_under_the_cap_untouched() {
    let root = scratch_root("prune-under");
    let dir = root.join("crash");
    std::fs::create_dir_all(&dir).expect("creating the crash directory");
    for stamp in 1..5 {
        write_record(&dir, &record_at(stamp)).expect("writing a record");
    }

    prune_records(&dir);

    assert_eq!(
        record_names(&dir).len(),
        4,
        "a directory below the cap is pruned by nothing"
    );
}
