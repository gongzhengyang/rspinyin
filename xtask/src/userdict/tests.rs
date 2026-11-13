//! Tests of the user-dictionary maintenance CLI, driven against scratch stores.
//!
//! Every test builds its own store under a process-unique scratch directory, seeds it
//! through the store's public write paths, and drives the same functions the
//! subcommands call -- so the round trips, the idempotency and the privacy gate are
//! asserted on the code that ships, with no display server and no real user data.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ime_dict::user_db::{Clock, UserDb};
use ime_types::UserFreqSource;

use super::*;

/// A clock frozen at one stamp, so seeded records carry a known last-used time.
struct FixedClock(u64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }

    fn now_nanos(&self) -> u64 {
        self.0.saturating_mul(1_000_000)
    }
}

/// The seeded words and their commit counts, in seed order.
const SEED_WORDS: [(&str, u32); 3] = [("甲", 5), ("乙", 3), ("丙", 1)];

/// The coined pair the seed enrolls: pinyin key, word, commits.
const SEED_COINED: (&str, &str, u32) = ("jia", "甲", 2);

/// A stamp in November 2023, far from every real "now" a bucket test might use.
const SEED_TIME_MS: u64 = 1_700_000_000_000;

/// The "now" a fresh record is judged against: half a day after the seed, so every
/// seeded record is inside the narrowest bucket and exactly on no bucket boundary.
const FRESH_NOW_MS: u64 = SEED_TIME_MS + DAY_MS / 2;

/// Distinguishes the scratch directories of calls that share one process, and of runs
/// that were handed a recycled process id.
static SCRATCH_SEQ: AtomicU32 = AtomicU32::new(0);

/// A scratch directory unique to this call: the process id, a millisecond stamp, and a
/// per-process counter together make a name no other test -- in this run, a concurrent
/// run, or a later one that was handed the same pid -- can be building at the same
/// moment.
///
/// A memory-backed file system is preferred when the host provides one: every
/// durability point the store takes is an `fsync`, and on a real disk that is a device
/// flush dominating these tests' wall clock without serving any assertion -- the code
/// paths under test are the same on a memory-backed one. The ordinary temporary
/// directory is the fallback.
fn scratch(tag: &str) -> PathBuf {
    let base = if Path::new("/dev/shm").is_dir() {
        PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let name = format!(
        "rspinyin-userdict-{tag}-{}-{}-{}",
        std::process::id(),
        millis,
        SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed),
    );
    let dir = base.join(name);
    match fs::create_dir_all(&dir) {
        Ok(()) => dir,
        // Creation can only fail here on a name collision with a leftover, which the
        // stamp and the counter make practically impossible; one cleared retry is the
        // belt for that belt-and-braces name.
        Err(_) => {
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("creating the scratch directory");
            dir
        }
    }
}

/// Opens a scratch store seeded with [`SEED_WORDS`] and [`SEED_COINED`], flushed, and
/// returns its path and handle.
///
/// The commit is structural, not incidental: reading an unflushed seed races the
/// store's own flush thread, because the fixed clock makes the first record's interval
/// trigger fire at once -- and a document rendered while that flush is in flight
/// depends on the scheduler. One explicit flush makes the store static before any
/// test reads it.
fn seeded_store(tag: &str) -> (PathBuf, UserDb) {
    let path = scratch(tag).join("user.redb");
    let mut store = UserDb::open_with(&path, Box::new(FixedClock(SEED_TIME_MS)))
        .expect("opening the scratch store");
    for (word, times) in SEED_WORDS {
        for _ in 0..times {
            store.record(word, 0);
        }
    }
    let (key, word, times) = SEED_COINED;
    for _ in 0..times {
        store.record_coin(key, word);
    }
    store.final_commit().expect("flushing the seed");
    (path, store)
}

/// The export file a round trip travels through, beside the store it came from.
fn export_path(tag: &str) -> PathBuf {
    scratch(tag).join("roundtrip.tsv")
}

/// The first line of a report entry, read from a `weight`-leading TSV row.
fn word_of(row: &str) -> &str {
    row.split('\t').next_back().expect("a fourth column")
}

#[test]
fn test_list_report_orders_entries_by_weight_descending() {
    let (_path, store) = seeded_store("list-order");
    let document = read_document(&store).expect("reading the document");
    let report = list_report(&document, DEFAULT_LIMIT);
    let mut lines = report.lines();
    assert_eq!(lines.next(), Some("weight\tlast_used_unix\tpinned\tword"));
    let words: Vec<&str> = lines.take(3).map(word_of).collect();
    assert_eq!(words, ["甲", "乙", "丙"]);
}

#[test]
fn test_list_report_limit_cuts_both_sections() {
    let (_path, store) = seeded_store("list-limit");
    let document = read_document(&store).expect("reading the document");
    let report = list_report(&document, 1);
    // The entry header, one entry, the coined header, one coined row.
    assert_eq!(report.lines().count(), 4);
    assert!(report.contains("coined_key\tcommits\tword"));
}

#[test]
fn test_export_forget_import_roundtrip_restores_the_document() {
    let (_path, mut store) = seeded_store("roundtrip");
    let file = export_path("roundtrip");
    // The one fixture durability point: the round trip's equality is stated over
    // committed stamps -- an import never lowers a creation stamp, so the document it
    // brings back has to carry committed rows for the restore to match.
    store.final_commit().expect("flushing the seed");
    // The document travels through a file as an operator's export would; it is
    // rendered once here and written without a second durability round, which the
    // round trip's assertions do not depend on.
    let mut exported = Vec::new();
    let rows = store
        .export_tsv(&mut exported)
        .expect("rendering the export");
    assert_eq!(rows, 4);
    fs::write(&file, &exported).expect("writing the export");
    let before = read_document(&store).expect("reading the document");

    let outcome = forget_word(&mut store, "乙").expect("forgetting");
    assert_eq!(outcome, ForgetOutcome::Removed);
    let after_forget = read_document(&store).expect("reading the document");
    assert_eq!(after_forget.entries.len(), 2);
    assert!(after_forget.entries.iter().all(|row| row.word != "乙"));

    let report = import_file(&store, &file).expect("importing back");
    assert_eq!(report.applied, 4);
    let restored = read_document(&store).expect("reading the document");
    assert_eq!(restored, before);
}

#[test]
fn test_import_is_idempotent_on_repeated_imports() {
    let (_path, store) = seeded_store("idempotent");
    let file = export_path("idempotent");
    // Rendered and written without a durability round: idempotency is a statement
    // about the merge, not about the file sync, which the round trip covers.
    let mut exported = Vec::new();
    store
        .export_tsv(&mut exported)
        .expect("rendering the export");
    fs::write(&file, &exported).expect("writing the export");
    let first = import_file(&store, &file).expect("first import");
    let after_first = read_document(&store).expect("reading the document");
    let second = import_file(&store, &file).expect("second import");
    let after_second = read_document(&store).expect("reading the document");
    assert_eq!(first.applied, second.applied);
    assert_eq!(after_first, after_second);
}

#[test]
fn test_import_refuses_a_document_without_the_marker() {
    let (_path, store) = seeded_store("refuse");
    let file = scratch("refuse").join("junk.tsv");
    fs::write(&file, "hello\tworld\n").expect("writing the junk document");
    let before = read_document(&store).expect("reading the document");
    let report = import_file(&store, &file).expect("the store answers with a report");
    assert!(!report.recognised);
    assert_eq!(report.applied, 0);
    assert_eq!(read_document(&store).expect("reading the document"), before);
}

#[test]
fn test_confirmation_gate_table() {
    let cases: [(bool, bool, Option<&str>, bool); 6] = [
        (true, false, None, true),
        (true, true, None, true),
        (false, false, Some("yes"), false),
        (false, true, Some("yes"), true),
        (false, true, Some("no"), false),
        (false, true, Some("YES"), false),
    ];
    for (yes, interactive, answer, expected) in cases {
        let given = answer.map(String::from);
        let confirmed_now = confirmed(yes, interactive, move || given.clone());
        assert_eq!(
            confirmed_now, expected,
            "yes={yes} interactive={interactive} answer={answer:?}"
        );
    }
}

#[test]
fn test_ensure_confirmed_refuses_without_yes_in_a_pipe() {
    // A test's stdin is a pipe, so the gate must refuse rather than prompt.
    let error = ensure_confirmed(false, "forget test").expect_err("must refuse");
    assert!(error.to_string().contains("--yes"));
    ensure_confirmed(true, "forget test").expect("--yes must pass");
}

#[test]
fn test_stats_report_counts_and_buckets_agree_with_the_store() {
    let (_path, store) = seeded_store("stats-counts");
    let document = read_document(&store).expect("reading the document");
    let fresh = stats_report(&document, 4096, FRESH_NOW_MS, false);
    assert!(fresh.contains("entries\t3\n"));
    assert!(fresh.contains("coined_entries\t1\n"));
    assert!(fresh.contains("bytes\t4096\n"));
    assert!(fresh.contains("last_24h\t3\n"));
    assert!(fresh.contains("last_7d\t0\n"));

    let stale = stats_report(&document, 4096, SEED_TIME_MS + MONTH_MS + DAY_MS, false);
    assert!(stale.contains("older\t3\n"));
    assert!(stale.contains("last_24h\t0\n"));
    assert!(stale.contains("never\t0\n"));
}

#[test]
fn test_stats_default_hides_word_text_and_full_unlocks_it() {
    let (_path, store) = seeded_store("stats-full");
    let document = read_document(&store).expect("reading the document");
    let plain = stats_report(&document, 4096, FRESH_NOW_MS, false);
    assert!(!plain.contains("甲"));
    assert!(!plain.contains("jia"));
    let full = stats_report(&document, 4096, FRESH_NOW_MS, true);
    assert!(full.contains("words_last_24h"));
    assert!(full.contains("5\t甲"));
}

#[test]
fn test_pin_then_unpin_round_trips_the_pinned_flag() {
    let (_path, mut store) = seeded_store("pin");
    // The seed leaves every word unpinned, so this toggle pins the word and the
    // answer reflects the post-state.
    assert!(pin_word(&mut store, "jia", "甲").expect("pinning"));
    let document = read_document(&store).expect("reading the document");
    let row = document
        .entries
        .iter()
        .find(|row| row.word == "甲")
        .expect("the seeded word");
    assert!(row.pinned);

    unpin_key(&mut store, "jia").expect("unpinning");
    let document = read_document(&store).expect("reading the document");
    let row = document
        .entries
        .iter()
        .find(|row| row.word == "甲")
        .expect("the seeded word");
    assert!(!row.pinned);
}

#[test]
fn test_forget_unknown_word_reports_not_found() {
    let (_path, mut store) = seeded_store("forget-miss");
    let outcome = forget_word(&mut store, "never-seen").expect("forgetting");
    assert_eq!(outcome, ForgetOutcome::NotFound);
}
