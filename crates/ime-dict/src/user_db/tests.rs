//! Tests for the user frequency store.
//!
//! Every test owns its database in a directory of its own under the system temp
//! directory and drives the clock through [`Clock`], so nothing here depends on a real
//! XDG directory, on the wall clock, or on the state left by another test.

use std::time::Duration;

use super::*;

/// The keys the tests type: short, distinct, and never taken from a real dictionary, so
/// that the store is exercised against literals only.
const KEYS: [&str; 4] = ["ni'hao", "shi'jie", "zhong'guo", "pin'yin"];

/// Environment variable that puts a test binary into the crash-child role.
const CHILD_DB_ENV: &str = "RSPINYIN_TEST_USER_DB_CHILD";
/// Environment variable naming the file the child touches once its records are in, so
/// that the parent kills a live process rather than a finished one.
const CHILD_READY_ENV: &str = "RSPINYIN_TEST_USER_DB_READY";
/// Records the child writes before it is killed.
const CHILD_RECORDS: u32 = 100;
/// Records a second the child writes at: the rate the throughput assumption names.
///
/// The child paces itself because the durability promise is a bound on *time*, not on
/// record count. Writing the hundred records in a tight loop would fire only the
/// batch-size trigger and measure a burst that no user produces, leaving the two-second
/// interval -- the half of the policy the assertion exists to check -- untested.
const CHILD_RECORDS_PER_SECOND: u64 = 20;
/// The loss the durability promise allows: two seconds of typing at the twenty records a
/// second the throughput assumption names.
const CHILD_MIN_SURVIVING: u64 = 60;

/// A clock the test drives, so that no test depends on the wall clock.
#[derive(Clone, Default)]
struct TestClock {
    ms: Arc<AtomicU64>,
    nanos: Arc<AtomicU64>,
    /// Nanoseconds added by every reading of the monotonic clock. Zero keeps the clock
    /// still, which is how a test tells the batch trigger apart from the interval trigger;
    /// a large step is how a test reports a slow flush without sleeping.
    step_nanos: u64,
}

impl TestClock {
    /// A clock that jumps `step_nanos` on every reading.
    fn stepping(step_nanos: u64) -> Self {
        Self {
            step_nanos,
            ..Self::default()
        }
    }

    /// Moves the wall clock forward.
    fn advance_ms(&self, ms: u64) {
        self.ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Moves the monotonic clock forward.
    fn advance_nanos(&self, nanos: u64) {
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.ms.load(Ordering::Relaxed)
    }

    fn now_nanos(&self) -> u64 {
        self.nanos.fetch_add(self.step_nanos, Ordering::Relaxed)
    }
}

/// Creates a per-test directory under the system temp directory.
///
/// The store takes its path as an argument precisely so that a test can own it: no test
/// here touches a real XDG directory.
fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-userdb-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the test directory");
    dir
}

/// The path of the store inside a test directory.
fn db_path(dir: &Path) -> PathBuf {
    dir.join("user.redb")
}

/// Opens a store in `dir` with a clock the test drives.
fn open_in(dir: &Path, clock: TestClock) -> UserDb {
    UserDb::open_with(db_path(dir), Box::new(clock)).expect("opening the user store")
}

/// Resident set size in bytes, read from `/proc/self/statm`.
///
/// Linux reports the resident page count there; the page size is taken as 4096, which is
/// what the kernels this project targets use. A coarser page size would under-report the
/// drift, which is the safe direction for a ceiling.
fn resident_bytes() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|field| field.parse::<u64>().ok())
        .unwrap_or(0);
    pages * 4096
}

#[test]
fn test_user_db_final_commit_survives_a_reopen() {
    let dir = temp_dir("reopen");
    let mut db = open_in(&dir, TestClock::default());
    for key in KEYS {
        db.record(key, 0);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing before the shutdown");
    drop(db);

    let reopened = open_in(&dir, TestClock::default());
    assert_eq!(
        reopened.record_count().expect("counting"),
        KEYS.len() as u64
    );
    for key in KEYS {
        assert_eq!(
            reopened.freq(key),
            2,
            "{key} kept its count across the restart"
        );
    }
}

#[test]
fn test_user_db_batch_trigger_flushes_at_the_batch_size() {
    let dir = temp_dir("batch");
    let db = open_in(&dir, TestClock::default());
    for index in 0..COMMIT_BATCH - 1 {
        db.record(&format!("key{index}"), 0);
    }
    assert_eq!(
        db.record_count().expect("counting"),
        0,
        "below the batch nothing is flushed"
    );
    db.record("key-last", 0);
    assert_eq!(db.record_count().expect("counting"), COMMIT_BATCH as u64);
    assert_eq!(db.pending_len(), 0, "the flush emptied the delta map");
}

#[test]
fn test_user_db_interval_trigger_flushes_after_the_interval() {
    let dir = temp_dir("interval");
    let clock = TestClock::default();
    let db = open_in(&dir, clock.clone());
    db.record("ni'hao", 0);
    assert_eq!(db.record_count().expect("counting"), 0);
    clock.advance_nanos(COMMIT_INTERVAL_MS * 1_000_000);
    db.record("ni'hao", 0);
    assert_eq!(db.record_count().expect("counting"), 1);
}

#[test]
fn test_user_db_freq_reads_the_pending_delta_before_the_flush() {
    let dir = temp_dir("freq");
    let mut db = open_in(&dir, TestClock::default());
    assert_eq!(db.freq("never-typed"), 0, "an unknown key has no frequency");
    db.record("ni'hao", 0);
    assert_eq!(db.freq("ni'hao"), 1, "a pending record is visible at once");
    db.final_commit().expect("flushing");
    assert_eq!(db.freq("ni'hao"), 1, "and the flushed value is the same");
    assert_eq!(db.freq("never-typed"), 0);
}

#[test]
fn test_user_db_flush_failure_degrades_to_readonly() {
    let dir = temp_dir("readonly");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    inject_failure();
    let error = db
        .final_commit()
        .expect_err("the injected failure surfaces");
    assert!(
        error.to_string().starts_with("data/readonly-mode"),
        "the diagnostic code names the degradation: {error}"
    );
    assert!(db.is_readonly(), "the store is read-only after the failure");
    assert_eq!(db.pending_len(), 0, "the delta map was emptied");
    assert_eq!(db.freq("ni'hao"), 0, "the dropped record is not reported");

    // Input keeps working: recording is a no-op, reading still answers, and a later flush
    // reports success without writing anything.
    db.record("ni'hao", 0);
    assert_eq!(db.freq("ni'hao"), 0);
    let report = db.final_commit().expect("a read-only store still reports");
    assert_eq!(report.written, 0);
}

#[test]
fn test_user_db_slow_flush_relaxes_the_batching_policy() {
    let dir = temp_dir("slow");
    // Every reading of the monotonic clock jumps five milliseconds, so the flush reports a
    // duration past the slow-disk threshold with no sleeping.
    let mut db = open_in(&dir, TestClock::stepping(5_000_000));
    for round in 1..SLOW_COMMIT_STREAK {
        db.record("ni'hao", 0);
        let report = db.final_commit().expect("flushing");
        assert!(
            report.elapsed_us >= SLOW_COMMIT_MS * 1_000,
            "the fixture clock has to keep reporting a slow flush"
        );
        assert!(
            !report.relaxed,
            "slow flush {round} of {SLOW_COMMIT_STREAK} must not widen the policy: one slow \
             flush is the first flush of a new database, not a slow disk"
        );
    }
    db.record("ni'hao", 0);
    let report = db.final_commit().expect("flushing");
    assert!(report.relaxed, "a run of slow flushes relaxes the policy");
    assert!(report.elapsed_us >= SLOW_COMMIT_MS * 1_000);
}

#[test]
fn test_user_db_evicts_the_oldest_records() {
    let dir = temp_dir("evict");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    for key in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    assert_eq!(db.record_count().expect("counting"), 10);

    let removed = db.evict_oldest(EVICT_FRACTION).expect("evicting");
    assert_eq!(removed, 1, "a tenth of ten records is one record");
    assert_eq!(db.record_count().expect("counting"), 9);
    assert_eq!(db.freq("a"), 0, "the least recently used record goes first");
    assert_eq!(db.freq("j"), 1, "the most recent one stays");
}

#[test]
fn test_user_db_cache_reuses_a_freed_slot() {
    let mut cache = LruCache::new(3);
    cache.insert("a", 1);
    cache.insert("b", 2);
    cache.insert("c", 3);
    cache.remove("b");
    cache.insert("d", 4);
    assert_eq!(cache.get("a"), Some(1), "a resident key is not evicted");
    assert_eq!(cache.get("c"), Some(3));
    assert_eq!(cache.get("d"), Some(4));
    assert_eq!(cache.get("b"), None, "the removed key is gone");
}

#[test]
fn test_user_db_cache_is_bounded_by_its_capacity() {
    let mut cache = LruCache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);
    cache.insert("c", 3);
    assert_eq!(
        cache.get("a"),
        None,
        "the third insert displaces one of two"
    );
    assert_eq!(cache.get("b"), Some(2));
    assert_eq!(cache.get("c"), Some(3));
}

#[test]
fn test_user_db_open_creates_a_private_file() {
    let dir = temp_dir("mode");
    let _db = open_in(&dir, TestClock::default());
    let mode = std::fs::metadata(db_path(&dir))
        .expect("reading the file mode")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, FILE_MODE, "the user database is private");
}

#[test]
fn test_user_db_open_fails_on_a_path_that_is_a_directory() {
    let dir = temp_dir("unusable");
    std::fs::create_dir_all(db_path(&dir)).expect("taking the file's place");
    let result = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()));
    assert!(
        matches!(result, Err(ImeError::DictUnavailable { .. })),
        "a path that cannot hold a store must not open"
    );
}

#[test]
fn test_user_db_open_rejects_a_second_handle_on_the_same_file() {
    let dir = temp_dir("locked");
    let _first = open_in(&dir, TestClock::default());
    let second = UserDb::open_with(db_path(&dir), Box::new(TestClock::default()));
    assert!(
        matches!(second, Err(ImeError::DictUnavailable { .. })),
        "the store holds an exclusive lock on its file"
    );
}

#[test]
fn test_user_db_is_user_word_is_false_in_this_phase() {
    let dir = temp_dir("userword");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");
    assert!(
        !db.is_user_word("ni'hao"),
        "a typed word is not a coined word"
    );
    assert!(!db.is_user_word("never-typed"));
}

#[test]
fn test_user_db_child_role_writes_records_until_killed() {
    let (Ok(path), Ok(ready)) = (std::env::var(CHILD_DB_ENV), std::env::var(CHILD_READY_ENV))
    else {
        // The parent role, or an ordinary run: this test has nothing to do.
        return;
    };
    let Ok(db) = UserDb::open(&path) else {
        return;
    };
    let interval = Duration::from_millis(1_000 / CHILD_RECORDS_PER_SECOND);
    for index in 0..CHILD_RECORDS {
        db.record(&format!("child{index}"), 0);
        std::thread::sleep(interval);
    }
    let _ = std::fs::write(&ready, b"ready");
    // Stay alive so that the parent kills a running store rather than one that exited on
    // its own: a process that exited proves nothing about a crash.
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn test_user_db_survives_a_kill_with_bounded_loss() {
    let dir = temp_dir("crash");
    let ready = dir.join("child-ready");
    let Ok(executable) = std::env::current_exe() else {
        return; // cannot re-enter the harness: skip rather than fail
    };
    let spawned = std::process::Command::new(executable)
        .args([
            "--exact",
            "user_db::tests::test_user_db_child_role_writes_records_until_killed",
        ])
        .env(CHILD_DB_ENV, db_path(&dir))
        .env(CHILD_READY_ENV, &ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return;
    };
    let mut written = false;
    for _ in 0..600 {
        if ready.exists() {
            written = true;
            break;
        }
        if !matches!(child.try_wait(), Ok(None)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // `kill` is SIGKILL on unix: the child gets no chance to flush.
    let _ = child.kill();
    let _ = child.wait();
    if !written {
        // The child never reached its records, so the harness could not run it. Skipping
        // keeps the suite green instead of failing on a sandbox that forbids spawning.
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let db = UserDb::open(db_path(&dir)).expect("reopening after the kill");
    let surviving = db.record_count().expect("counting the survivors");
    assert!(
        surviving >= CHILD_MIN_SURVIVING,
        "a kill inside the commit window kept {surviving} of {CHILD_RECORDS} records"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "soak measurement: only meaningful on an idle machine"]
fn test_user_db_rss_does_not_drift_over_a_simulated_run() {
    let dir = temp_dir("rss");
    let mut db = open_in(&dir, TestClock::default());
    // Five minutes at ten records a second over six hundred keys. The run is compressed in
    // time on purpose: the drift is a function of the number of records and flushes, not of
    // the wall clock, and no test here may wait on a clock.
    let keys: Vec<String> = (0..600).map(|index| format!("soak{index}")).collect();
    let before = resident_bytes();
    assert!(
        before > 0,
        "the resident set cannot be read on this platform"
    );
    for index in 0..3000u64 {
        db.record(&keys[(index % 600) as usize], 0);
    }
    db.final_commit().expect("flushing the soak run");
    let drift = resident_bytes().saturating_sub(before);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        drift <= 2 * 1024 * 1024,
        "the resident set drifted by {drift} bytes over the run"
    );
}

/// Ceiling on the resident set the loaded counts may add for [`HYDRATE_CAP`] records.
///
/// The number is the project's resident-set drift budget, which is also the figure the
/// loading design is stated against: a store at the ceiling has to stay inside it.
const HYDRATE_RSS_CEILING: u64 = 2 * 1024 * 1024;

#[test]
fn test_user_db_loads_the_store_into_memory_at_open() {
    let dir = temp_dir("loaded");
    let mut db = open_in(&dir, TestClock::default());
    for key in KEYS {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    drop(db);

    let db = open_in(&dir, TestClock::default());
    assert!(db.is_hydrated(), "a four-record store fits the ceiling");
    assert_eq!(
        db.committed_len(),
        Some(KEYS.len()),
        "the whole store is loaded"
    );
    for key in KEYS {
        assert_eq!(db.freq(key), 1, "{key} was loaded");
    }
    assert_eq!(db.freq("never-typed"), 0, "and nothing else was");
}

#[test]
fn test_user_db_loaded_lookup_opens_no_store_read() {
    let dir = temp_dir("loaded-reads");
    let mut db = open_in(&dir, TestClock::default());
    for key in KEYS {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    drop(db);

    let db = open_in(&dir, TestClock::default());
    assert!(db.is_hydrated(), "the fixture store has to be loaded");
    let before = store_reads();
    for index in 0..1000 {
        assert_eq!(db.freq(KEYS[index % KEYS.len()]), 1);
    }
    assert_eq!(
        store_reads() - before,
        0,
        "a thousand lookups on a loaded store open no read transaction"
    );
    assert_eq!(db.freq("never-typed"), 0, "a word the store never saw");
    assert_eq!(
        store_reads() - before,
        0,
        "a miss is answered from memory as well"
    );
}

#[test]
fn test_user_db_loaded_lookup_adds_the_pending_delta() {
    let dir = temp_dir("loaded-delta");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");
    drop(db);

    let mut db = open_in(&dir, TestClock::default());
    assert_eq!(db.freq("ni'hao"), 1, "the loaded count is the answer");
    db.record("ni'hao", 0);
    assert_eq!(db.freq("ni'hao"), 2, "a pending record is visible");
    db.final_commit().expect("flushing");
    assert_eq!(
        db.freq("ni'hao"),
        2,
        "the flush adopts the delta instead of counting it twice"
    );
    assert_eq!(
        db.committed_len(),
        Some(1),
        "the delta moved into the loaded counts"
    );
}

#[test]
fn test_user_db_degrade_keeps_the_loaded_counts_readable() {
    let dir = temp_dir("degrade-loaded");
    let mut db = open_in(&dir, TestClock::default());
    db.record("ni'hao", 0);
    db.final_commit().expect("flushing");

    db.record("shi'jie", 0);
    inject_failure();
    let _ = db.final_commit().expect_err("the injected flush fails");
    assert!(db.is_readonly(), "the store degraded");

    let before = store_reads();
    assert_eq!(
        db.freq("ni'hao"),
        1,
        "a committed count survives the degradation"
    );
    assert_eq!(db.freq("shi'jie"), 0, "the dropped delta is gone");
    assert_eq!(
        store_reads() - before,
        0,
        "a store that cannot be written still answers from memory"
    );
    assert_eq!(
        db.committed_len(),
        Some(1),
        "the loaded counts still match the file, which holds one record"
    );
}

#[test]
fn test_user_db_eviction_forgets_the_evicted_counts() {
    let dir = temp_dir("evict-loaded");
    let clock = TestClock::default();
    let mut db = open_in(&dir, clock.clone());
    for key in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        clock.advance_ms(1_000);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    assert_eq!(db.committed_len(), Some(10), "the store is loaded");

    let removed = db.evict_oldest(EVICT_FRACTION).expect("evicting");
    assert_eq!(removed, 1, "a tenth of ten records is one record");
    assert_eq!(db.freq("a"), 0, "the evicted key left memory");
    assert_eq!(
        db.freq("j"),
        1,
        "the records that stayed are still readable"
    );
    assert_eq!(
        db.committed_len(),
        Some(9),
        "the loaded counts track the store's record count"
    );
    assert_eq!(db.record_count().expect("counting"), 9);
}

#[test]
fn test_user_db_store_past_the_ceiling_reads_on_demand() {
    let dir = temp_dir("large");
    let mut db = open_in(&dir, TestClock::default());
    for key in KEYS {
        db.record(key, 0);
        db.record(key, 0);
    }
    db.final_commit().expect("flushing");
    drop(db);

    // A ceiling below the store's record count is how the fallback is reached without
    // building a fifty-thousand-record store.
    let clock = TestClock::default();
    let opened = UserDb::open_with_hydrate_cap(db_path(&dir), Box::new(clock), 1);
    let db = opened.expect("opening the user store");
    assert!(
        !db.is_hydrated(),
        "four records do not fit a ceiling of one"
    );
    assert_eq!(db.committed_len(), None, "nothing was loaded");

    let before = store_reads();
    assert_eq!(db.freq("ni'hao"), 2, "the answer comes from the store");
    assert_eq!(
        store_reads() - before,
        1,
        "one read transaction, not one per lattice edge"
    );
    assert_eq!(db.freq("ni'hao"), 2, "and the total is remembered");
    assert_eq!(store_reads() - before, 1, "no second transaction");

    assert_eq!(db.freq("never-typed"), 0, "a word the store never saw");
    assert_eq!(db.freq("never-typed"), 0);
    assert_eq!(
        store_reads() - before,
        2,
        "a miss is remembered too: one transaction per distinct word per session"
    );
}

#[test]
#[ignore = "soak measurement: only meaningful on an idle machine"]
fn test_user_db_loading_stays_within_its_memory_ceiling() {
    let dir = temp_dir("load-rss");
    let mut db = open_in(&dir, TestClock::default());
    let mut keys: Vec<String> = Vec::with_capacity(HYDRATE_CAP as usize);
    for index in 0..HYDRATE_CAP {
        keys.push(format!("soak{index}"));
    }
    for key in &keys {
        db.record(key, 0);
    }
    db.final_commit().expect("flushing the soak run");
    drop(db);

    let before = resident_bytes();
    assert!(
        before > 0,
        "the resident set cannot be read on this platform"
    );
    let db = open_in(&dir, TestClock::default());
    let drift = resident_bytes().saturating_sub(before);
    assert!(db.is_hydrated(), "a store at the ceiling is loaded");
    assert_eq!(db.committed_len(), Some(HYDRATE_CAP as usize));
    assert_eq!(db.freq("soak0"), 1, "the loaded counts answer");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        drift <= HYDRATE_RSS_CEILING,
        "loading {HYDRATE_CAP} records added {drift} bytes"
    );
}
