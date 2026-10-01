//! Unit tests for the host's half of the phrase document.
//!
//! A document is a file, so these tests write one: each takes a directory of its own
//! under the system's temporary directory, empties it first and leaves it behind, which
//! is the shape the user-database tests use for the same reason. What the file *format*
//! is belongs to `ime-core` and is tested there; what is tested here is that this layer
//! resolves the path the configuration names, reads the document, merges it the way the
//! reader's rule says, hands exactly one row to the writer thread when a phrase is saved,
//! and degrades instead of failing when there is no document at all.
//!
//! A save is queued rather than written, so a test that asserts on the document waits for
//! the writer thread to get there: `wait_until` polls the very condition the test asserts
//! rather than sleeping for a guessed interval, which is the shape the deferred write
//! path's own tests use.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use ime_core::phrase::{PhraseTable, append_phrase};
use ime_dict::paths::{BaseDirs, FILE_MODE, Paths};
use ime_types::ImeError;

use super::{
    BUILT_IN_DOCUMENT, PHRASE_SHUTDOWN_TIMEOUT_CODE, PhraseHandle, PhraseStore, document_path,
    merge_documents,
};

/// How long a test waits for the writer thread before it gives up.
///
/// Generous on purpose: the assertions are about what the writer produces, not about how
/// fast a loaded machine is, and a shorter bound would only make the suite flaky.
const PATIENCE: Duration = Duration::from_secs(5);

/// The ceiling one host callback may spend.
///
/// A key callback runs on the fcitx5 main loop, which is why the bound is orders of
/// magnitude below a frame interval: a save is one row rendered and one queue hand-off,
/// and the write and the read-back that follow it belong to the writer thread. A save that
/// waited for the writer would miss this bound by thousands of times.
const HOST_CALLBACK_BUDGET: Duration = Duration::from_micros(200);

/// How many saves the budget assertion samples.
///
/// The bound is on the work rather than on the scheduler: a test thread that is descheduled
/// in the middle of a call measures the scheduler, so the fastest sample is the one
/// asserted. A save that did the work on the caller's thread cannot pass at any sample
/// count, because every one of its samples pays the read-back.
const BUDGET_SAMPLES: u32 = 16;

/// How many entries the document of the budget test holds.
///
/// The read-back a save used to pay scales with the table, so the assertion needs a document
/// large enough that doing that work on the host thread could not pass it. Four thousand
/// entries is past the point where the parse is measured in milliseconds.
const BUDGET_DOCUMENT_ENTRIES: u32 = 4000;

/// An empty directory of this test's own.
///
/// Emptied rather than merely created, so that a second run of the suite does not read
/// what the first one left behind. The directory is not removed afterwards: it holds
/// nothing a later run depends on, and a test that fails leaves its evidence in place.
fn scratch_dir(label: &str) -> PathBuf {
    let name = format!("rspinyin-phrases-{}-{label}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The built-in table on its own, which is what a degraded store falls back to.
fn built_in_table() -> PhraseTable {
    PhraseTable::load(BUILT_IN_DOCUMENT, 1000).0
}

/// A key of the alphabet the reader accepts, for the row at `index`.
///
/// The key alphabet is `a`..=`z` and `'`, so an index is spelled in base 26 rather than
/// written as digits, which the reader would skip as a row it cannot use.
fn key_of(index: u32) -> String {
    let mut key = String::new();
    let mut rest = index;
    for _ in 0..4 {
        let letter = u8::try_from(rest % 26).expect("a letter index below 26");
        key.push(char::from(b'a' + letter));
        rest /= 26;
    }
    key
}

/// Waits until `condition` holds, answering whether it did.
///
/// The writer publishes on a thread of its own, so a test that asserts on the document has
/// to give it the chance to get there; a fixed sleep would be a guess about how long that
/// takes, and this is a bounded wait on the very condition the test asserts.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::yield_now();
    }
    condition()
}

#[test]
fn test_merge_documents_reads_the_user_document_after_the_built_in_one() {
    // The merge rule is the reader's: the built-in rows are read first and the user's
    // after them, so a key the user defined keeps the user's body while a key only the
    // plugin ships stays where it was.
    let built_in = "rq\tbuilt-in\nsj\tbuilt-in\n";
    let user = "rq\tmine\n";
    let (table, report) = merge_documents(built_in, Some(user), 100, true);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.overridden, 1);
    let mine = table.longest_match("rq", 0);
    assert_eq!(mine.map(|hit| table.text(hit)), Some("mine"));
    let shipped = table.longest_match("sj", 0);
    assert_eq!(shipped.map(|hit| table.text(hit)), Some("built-in"));
}

#[test]
fn test_merge_documents_without_a_user_document_keeps_the_built_in_rows() {
    let (table, report) = merge_documents("rq\tbuilt-in\n", None, 100, true);
    assert_eq!(report.loaded, 1);
    let hit = table.longest_match("rq", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("built-in"));
}

#[test]
fn test_merge_documents_of_a_disabled_table_holds_nothing() {
    // `[phrases] enabled = false` is the user saying no phrase takes part in a decode.
    let built_in = "rq\tbuilt-in\n";
    let user = "zzz\tmine\n";
    let (table, report) = merge_documents(built_in, Some(user), 100, false);
    assert!(table.is_empty(), "a disabled table matches nothing");
    assert_eq!(report.loaded, 0);
}

#[test]
fn test_merge_documents_reports_the_rows_the_limit_drops() {
    // The reader keeps the rows a document declares first, so the row past the limit is
    // the last one read -- which is exactly the row a save appends.
    let built_in = "aa\tone\nbb\ttwo\n";
    let user = "cc\tthree\n";
    let (table, report) = merge_documents(built_in, Some(user), 2, true);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.over_limit, 1);
    assert_eq!(table.len(), 2);
    assert!(table.longest_match("cc", 0).is_none());
}

#[test]
fn test_load_reads_the_document_the_writer_produced() {
    // The two halves of the format meet here: the row is rendered by `ime-core`'s writer,
    // written to a file, and read back by this layer into a table.
    let dir = scratch_dir("load");
    let document = dir.join("phrases.tsv");
    let row = append_phrase("", "zzz", "2026-09-29").unwrap_or_default();
    fs::write(&document, row).expect("a document");

    let store = PhraseStore::load(Some(document), 100, true);
    assert!(!store.is_degraded(), "the document was read");
    let table = store.table();
    let hit = table.longest_match("zzz", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29"));
}

#[test]
fn test_load_of_a_missing_document_degrades_to_the_built_in_table() {
    let dir = scratch_dir("missing");
    let missing = dir.join("phrases.tsv");
    let store = PhraseStore::load(Some(missing), 100, true);
    assert!(
        store.is_degraded(),
        "a document that is not there is the degradation"
    );
    let expected = built_in_table().len();
    assert_eq!(
        store.table().len(),
        expected,
        "only the built-in rows are left"
    );
}

#[test]
fn test_load_counts_the_rows_the_reader_cannot_use() {
    let dir = scratch_dir("skips");
    let document = dir.join("phrases.tsv");
    fs::write(&document, "broken row\nzzz\tkept\n").expect("a document");

    let store = PhraseStore::load(Some(document), 100, true);
    assert!(
        !store.is_degraded(),
        "a document with a broken row was read"
    );
    assert_eq!(store.report().skipped, 1);
    let table = store.table();
    let hit = table.longest_match("zzz", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("kept"));
}

#[test]
fn test_append_writes_one_row_and_reloads_the_table() {
    let dir = scratch_dir("append");
    let document = dir.join("phrases.tsv");
    let store = PhraseStore::load(Some(document.clone()), 100, true);
    assert!(
        store.is_degraded(),
        "there is no document until one is written"
    );

    let saved = store.append("zzz", "2026-09-29");
    assert!(
        saved.is_ok(),
        "the document's directory exists, so the row is queued"
    );

    // The write is the writer thread's, so the assertions below wait for it rather than
    // assuming it landed with the call.
    let published = wait_until(|| !store.is_degraded());
    assert!(published, "the queued row is written and read back");
    assert_eq!(
        fs::read_to_string(&document).unwrap_or_default(),
        "zzz\t2026-09-29\n"
    );
    assert_eq!(
        store.table().len(),
        built_in_table().len() + 1,
        "the saved row is one entry more than the built-in table"
    );
    let table = store.table();
    let hit = table.longest_match("zzz", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29"));
    assert!(store.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_append_creates_the_document_private() {
    // A phrase is the user's own text, so the document is created the way every other
    // file this plugin writes is: 0600, in a directory the layout made 0700.
    let dir = scratch_dir("private");
    let document = dir.join("phrases.tsv");
    let store = PhraseStore::load(Some(document.clone()), 100, true);
    assert!(store.append("zzz", "2026-09-29").is_ok());

    // The mode is settled once the row has been read back: the write is the writer
    // thread's, and a check that ran earlier would be racing it.
    let published = wait_until(|| !store.is_degraded());
    assert!(published, "the queued row is written and read back");
    let mode = fs::metadata(&document).map(|meta| meta.permissions().mode());
    assert_eq!(mode.ok().map(|mode| mode & 0o777), Some(FILE_MODE));
}

#[test]
fn test_append_keeps_the_document_it_extends() {
    let dir = scratch_dir("extends");
    let document = dir.join("phrases.tsv");
    fs::write(&document, "# mine\nzzz\told\n").expect("a document");

    let store = PhraseStore::load(Some(document.clone()), 100, true);
    assert!(store.append("yyy", "new").is_ok());
    let published = wait_until(|| {
        let table = store.table();
        table.longest_match("yyy", 0).is_some()
    });
    assert!(published, "the queued row is written and read back");
    let written = fs::read_to_string(&document).unwrap_or_default();
    assert_eq!(written, "# mine\nzzz\told\nyyy\tnew\n");
    let table = store.table();
    let hit = table.longest_match("yyy", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("new"));
}

#[test]
fn test_append_hands_the_row_to_the_writer_without_writing_it() {
    // The deferred contract at the store's level: the call returns with the row queued and
    // the host thread has touched nothing. A document whose directory is not there is the
    // sharpest form of it -- the save is accepted, and the refusal is the writer thread's
    // to report on the diagnostic channel.
    let dir = scratch_dir("queued");
    let document = dir.join("absent").join("phrases.tsv");
    let store = PhraseStore::load(Some(document.clone()), 100, true);

    let saved = store.append("zzz", "2026-09-29");
    assert!(saved.is_ok(), "the row is queued rather than written");
    assert!(!document.exists(), "the host thread wrote nothing");

    assert!(store.shutdown(PATIENCE), "the writer stops");
    assert!(
        !document.exists(),
        "the write the missing directory refused left nothing behind"
    );
}

#[test]
fn test_append_stays_within_the_host_callback_budget() {
    // The host thread pays one row rendering and one queue hand-off, whatever the document
    // holds. The document here is large enough that the read-back a save used to pay could
    // not pass the bound, so a save that went back to doing the work itself fails here.
    let dir = scratch_dir("budget");
    let document = dir.join("phrases.tsv");
    let mut rows = String::new();
    for index in 0..BUDGET_DOCUMENT_ENTRIES {
        rows.push_str(&key_of(index));
        rows.push('\t');
        rows.push_str("2026-09-29");
        rows.push('\n');
    }
    fs::write(&document, rows).expect("a document");
    let store = PhraseStore::load(Some(document), 8000, true);

    let mut fastest = Duration::MAX;
    for index in 0..BUDGET_SAMPLES {
        let key = key_of(index);
        let start = Instant::now();
        let saved = store.append(&key, "2026-09-29");
        let elapsed = start.elapsed();
        assert!(saved.is_ok(), "the row is queued");
        fastest = fastest.min(elapsed);
    }
    assert!(
        fastest <= HOST_CALLBACK_BUDGET,
        "the fastest save cost {fastest:?}, past the {HOST_CALLBACK_BUDGET:?} a host callback may spend"
    );
    assert!(store.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_shutdown_writes_the_rows_that_were_still_queued() {
    // The graceful-stop guarantee: a row that was accepted is written before the writer
    // stops, so an unload that follows a save loses nothing.
    let dir = scratch_dir("drain");
    let document = dir.join("phrases.tsv");
    let store = PhraseStore::load(Some(document.clone()), 100, true);
    for (key, text) in [("aa", "one"), ("bb", "two"), ("cc", "three")] {
        assert!(store.append(key, text).is_ok(), "the row is queued");
    }

    assert!(store.shutdown(PATIENCE), "the writer drains and stops");
    assert_eq!(
        fs::read_to_string(&document).unwrap_or_default(),
        "aa\tone\nbb\ttwo\ncc\tthree\n",
        "every accepted row is on the document once the writer has stopped"
    );
}

#[test]
fn test_append_refuses_an_entry_the_reader_would_refuse() {
    let dir = scratch_dir("refused");
    let document = dir.join("phrases.tsv");
    let store = PhraseStore::load(Some(document.clone()), 100, true);

    let refused = store.append("r q", "2026-09-29");
    assert!(refused.is_err(), "a key outside the alphabet is refused");
    if let Err(error) = refused {
        let rendered = error.to_string();
        let expected = "config/invalid: phrases.file (";
        assert!(rendered.starts_with(expected), "{rendered}");
        let quotes_the_entry = rendered.contains("2026-09-29");
        assert!(!quotes_the_entry, "the entry is not quoted");
    }
    let is_created = document.exists();
    assert!(!is_created, "a refused entry writes nothing at all");
}

#[test]
fn test_append_without_a_document_reports_read_only_mode() {
    // The degradation ASM-15 names: the plugin stops writing and keeps working, so the
    // failure is the read-only code and not a phrase-table failure. A store with no
    // document refuses the save before it is queued, because there is nowhere to queue it
    // to and the caller is the only one left who can report it.
    let store = PhraseStore::load(None, 100, true);
    let refused = store.append("zzz", "2026-09-29");
    assert!(refused.is_err());
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode:"), "{rendered}");
        // The code itself carries a slash, so what has to name no path is the reason
        // after it: `data/readonly-mode` is the stable code and is meant to be there.
        let reason = rendered
            .strip_prefix("data/readonly-mode: ")
            .expect("the rendering carries the code and then the reason");
        assert!(!reason.contains('/'), "the reason names no path: {reason}");
        assert!(matches!(error, ImeError::DataReadonly { .. }));
    }
    assert!(
        store.shutdown(Duration::ZERO),
        "a store with no writer has nothing to wait for"
    );
}

#[test]
fn test_document_path_resolves_the_default_relative_and_absolute_values() {
    let bases = BaseDirs {
        config_home: PathBuf::from("/tmp/rspinyin-phrases-layout/config"),
        data_home: PathBuf::from("/tmp/rspinyin-phrases-layout/data"),
    };
    let layout = Paths::from_bases(&bases).expect("a layout");
    let config_dir = PathBuf::from("/tmp/rspinyin-phrases-layout/config/rspinyin");

    let default_path = document_path("", &layout);
    assert_eq!(default_path, config_dir.join("phrases.tsv"));
    assert_eq!(
        document_path("mine.tsv", &layout),
        config_dir.join("mine.tsv"),
        "a relative value is relative to the configuration directory"
    );
    assert_eq!(
        document_path("/etc/rspinyin/mine.tsv", &layout),
        PathBuf::from("/etc/rspinyin/mine.tsv")
    );
}

#[test]
fn test_handle_answers_the_table_of_the_store_it_wraps() {
    let dir = scratch_dir("handle");
    let document = dir.join("phrases.tsv");
    let handle = PhraseHandle::new(PhraseStore::load(Some(document), 100, true));

    assert!(handle.append("zzz", "2026-09-29").is_ok());
    let published = wait_until(|| {
        let table = handle.table();
        table.longest_match("zzz", 0).is_some()
    });
    assert!(published, "the queued row is written and read back");
    let table = handle.table();
    let hit = table.longest_match("zzz", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29"));
    assert!(handle.shutdown(PATIENCE), "the writer stops");
}

// ── the teardown drain ─────────────────────────────────────────────────────────────

#[test]
fn test_drain_phrase_writer_writes_the_rows_that_were_still_queued() {
    // The unload guarantee the teardown exists for: rows the session saved in its last
    // keystrokes are still in the queue when the addon goes away, and the drain is the
    // call that lands them. The document is read back, because that is where the next
    // load will read the rows from.
    let dir = scratch_dir("drain-unload");
    let document = dir.join("phrases.tsv");
    let mut store = PhraseStore::load(Some(document.clone()), 100, true);
    for (key, text) in [("aa", "one"), ("bb", "two"), ("cc", "three")] {
        assert!(store.append(key, text).is_ok(), "the row is queued");
    }

    assert!(
        store.drain_phrase_writer(PATIENCE),
        "the writer drains its queue and stops"
    );
    assert_eq!(
        fs::read_to_string(&document).unwrap_or_default(),
        "aa\tone\nbb\ttwo\ncc\tthree\n",
        "every queued row is on the document once the unload drained the writer"
    );
    let table = store.table();
    let hit = table.longest_match("cc", 0);
    assert_eq!(
        hit.map(|hit| table.text(hit)),
        Some("three"),
        "the drained store still answers for the rows that landed"
    );
}

#[test]
fn test_drain_phrase_writer_refuses_saves_after_the_writer_is_taken() {
    // The difference between the drain and an in-place stop: the drained store holds no
    // writer at all, so a save that somehow arrives after the unload began is refused
    // before it is queued rather than accepted into a queue nobody will drain again.
    let dir = scratch_dir("drain-taken");
    let mut store = PhraseStore::load(Some(dir.join("phrases.tsv")), 100, true);

    assert!(store.drain_phrase_writer(PATIENCE), "the idle writer stops");
    let refused = store.append("zzz", "2026-09-29");
    assert!(refused.is_err(), "a drained store queues no row");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode: "), "{rendered}");
        assert!(matches!(error, ImeError::DataReadonly { .. }));
    }
    assert!(
        store.drain_phrase_writer(PATIENCE),
        "a second drain finds no writer and answers at once"
    );
}

#[test]
fn test_drain_phrase_writer_with_an_empty_outbox_writes_nothing() {
    // The zero-work path: a writer with nothing queued stops without touching the
    // document, so an unload that follows a session with no save costs no write at all.
    let dir = scratch_dir("drain-empty");
    let document = dir.join("phrases.tsv");
    let mut store = PhraseStore::load(Some(document.clone()), 100, true);

    assert!(store.drain_phrase_writer(PATIENCE), "the idle writer stops");
    assert!(
        !document.exists(),
        "nothing queued means the document was never created"
    );
}

#[test]
fn test_drain_phrase_writer_without_a_writer_answers_at_once() {
    // No document, no writer, no wait: the teardown of a store that could never save
    // costs nothing, whatever budget it is handed.
    let mut store = PhraseStore::load(None, 100, true);
    assert!(
        store.drain_phrase_writer(Duration::ZERO),
        "a store with no writer has nothing to wait for"
    );
}

#[test]
fn test_phrase_shutdown_timeout_code_keeps_the_registered_spelling() {
    // The timeout code is registered in the design's runtime-diagnostic table and matched
    // by grep, so its spelling is part of the contract rather than a detail.
    assert_eq!(PHRASE_SHUTDOWN_TIMEOUT_CODE, "phrase/shutdown-timeout");
}

#[test]
fn test_handle_drain_phrase_writer_writes_the_queued_row() {
    // The handle is what the teardown reaches the store through, so the drain runs through
    // it too: the row the session queued is on the document when the call answers.
    let dir = scratch_dir("handle-drain");
    let document = dir.join("phrases.tsv");
    let handle = PhraseHandle::new(PhraseStore::load(Some(document.clone()), 100, true));
    assert!(
        handle.append("zzz", "2026-09-29").is_ok(),
        "the row is queued"
    );

    assert!(
        handle.drain_phrase_writer(PATIENCE),
        "the writer drains and stops"
    );
    assert_eq!(
        fs::read_to_string(&document).unwrap_or_default(),
        "zzz\t2026-09-29\n",
        "the queued row is on the document once the handle drained the writer"
    );
    assert!(
        handle.drain_phrase_writer(PATIENCE),
        "a second drain finds no writer and answers at once"
    );
}
