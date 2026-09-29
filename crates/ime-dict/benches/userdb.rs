//! Criterion benchmarks for the user frequency store.
//!
//! The budgets these exist for are the interactive ones: `record` stays inside 5us, a
//! cached `freq` inside 100ns, and a flush of a full batch inside 1.5ms. Every case runs
//! against a store in a directory of its own under the system temp directory, driven by a
//! clock the benchmark owns, so what is measured is the store and not the wall clock.
//!
//! `record` and `record_flush` are deliberately two cases. The budget is stated for the
//! record path alone, and the flush a full batch earns is budgeted separately, so the
//! first case keeps fewer keys pending than the batch trigger and the second cycles past
//! it: their difference is the flush, amortized over the records that earned it.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};

use ime_dict::user_db::{COMMIT_BATCH, Clock, UserDb};
use ime_types::UserFreqSource;
use redb::Durability;

/// A clock that never advances.
///
/// A still clock is what separates the two things `record` does: with it the interval
/// trigger can never fire, so a case that keeps fewer keys pending than [`COMMIT_BATCH`]
/// measures the record path, and a case that cycles past the batch measures the same path
/// with the flush it earned.
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        0
    }

    fn now_nanos(&self) -> u64 {
        0
    }
}

/// `count` keys sharing a prefix, so that a case can cycle a key set it owns.
fn keys(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

/// Opens a store in a directory of its own and fills it with `records`.
///
/// Returns the store together with the directory it owns, or `None` when the fixture
/// cannot be built. A benchmark is not a place to abort a run: `unwrap` and `expect` are
/// not available to non-test code in this workspace, and a missing fixture is a reason to
/// skip a case rather than to fail a run.
fn fixture(label: &str, records: &[String]) -> Option<(UserDb, PathBuf)> {
    let dir = std::env::temp_dir().join(format!(
        "rspinyin-userdb-bench-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("user.redb");
    let _ = std::fs::remove_file(&path);
    let mut db = UserDb::open_with(&path, Box::new(FixedClock)).ok()?;
    for key in records {
        db.record(key, 0);
    }
    db.final_commit().ok()?;
    Some((db, dir))
}

/// Benchmarks the record path, the cached and uncached lookups, and the flush.
fn userdb_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("userdb");

    // The fast path the 5us budget is stated for: a key already in the cache, and fewer
    // keys pending than the batch trigger, so no flush is due.
    if let Some((db, dir)) = fixture("record", &[]) {
        let hot = keys("hot", 8);
        let mut cursor = 0usize;
        group.bench_function("record", |bencher| {
            bencher.iter(|| {
                let key = &hot[cursor % hot.len()];
                cursor = cursor.wrapping_add(1);
                db.record(black_box(key), 0);
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The same path with the flush it earns: every thirty-second distinct key starts one,
    // so the reported cost is the record path plus its share of the flush budget.
    if let Some((db, dir)) = fixture("record-flush", &[]) {
        let many = keys("many", COMMIT_BATCH * 2);
        let mut cursor = 0usize;
        group.bench_function("record_flush", |bencher| {
            bencher.iter(|| {
                let key = &many[cursor % many.len()];
                cursor = cursor.wrapping_add(1);
                db.record(black_box(key), 0);
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A cached lookup: the case the 100ns budget is stated for.
    if let Some((db, dir)) = fixture("freq-hit", &keys("seen", 1)) {
        let key = "seen0".to_string();
        let _ = db.freq(&key);
        group.bench_function("freq_hit", |bencher| {
            bencher.iter(|| black_box(db.freq(black_box(&key))));
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A miss: a key the store has never seen, so every call opens a read transaction over
    // a table that holds a thousand records. Nothing is cached, because a key with no
    // frequency has nothing to cache.
    if let Some((db, dir)) = fixture("freq-miss", &keys("seen", 1000)) {
        group.bench_function("freq_miss", |bencher| {
            bencher.iter(|| black_box(db.freq(black_box("absent"))));
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A flush of one record fewer than the batch trigger, so that the measured flush is
    // the only flush the iteration causes and the record cost stays out of the number.
    if let Some((mut db, dir)) = fixture("commit", &[]) {
        let batch = keys("batch", COMMIT_BATCH);
        group.bench_function("commit", |bencher| {
            bencher.iter(|| {
                for key in batch.iter().take(COMMIT_BATCH - 1) {
                    db.record(key, 0);
                }
                db.commit(Durability::Eventual)
                    .map(|report| report.written)
                    .ok()
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    group.finish();
}

criterion_group!(benches, userdb_bench);
criterion_main!(benches);
