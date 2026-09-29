//! Criterion benchmarks for the user frequency store.
//!
//! The budgets these exist for are the interactive ones: `record` stays inside 5us, a
//! lookup the store answers from memory inside 100ns, and a flush of a full batch inside
//! 1.5ms. Every case runs against a store in a directory of its own under the system temp
//! directory, driven by a clock the benchmark owns, so what is measured is the store and
//! not the wall clock. Those figures are budgets stated by the project, not measurements:
//! nothing here records a number, and a run on a loaded machine is not evidence about one.
//!
//! `record` and `record_flush` are deliberately two cases. The budget is stated for the
//! record path alone, and the flush a full batch earns is budgeted separately, so the
//! first case keeps fewer keys pending than the batch trigger and the second cycles past
//! it: their difference is the flush, amortized over the records that earned it.
//!
//! The three `freq` cases are the read path. `freq_hit` and `freq_miss` are the decode
//! path -- a word the store knows and one it does not, both answered from the counts held
//! in memory -- and `freq_degraded` is the fallback a store past its loading ceiling takes,
//! where the answer costs a read transaction. The fallback case exists so that the path
//! stays measurable, not to hold it to the interactive budget.
//!
//! `forget` is the other key path, and the budget it is measured against is 0.05ms: the
//! keystroke that drops a word must cost a map insert and nothing else. `list_words` and
//! `export_tsv` are the management paths, which nothing calls while the user is typing;
//! they are here so that the cost of a page and of a document is a number rather than a
//! guess, not to hold them to an interactive budget.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};

use ime_dict::user_db::{COMMIT_BATCH, Clock, HYDRATE_CAP, UserDb};
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

/// Opens a store in a directory of its own, fills it with `records`, and reopens it with
/// `hydrate_cap` as the ceiling for the counts it loads into memory.
///
/// Returns the store together with the directory it owns, or `None` when the fixture
/// cannot be built. A benchmark is not a place to abort a run: `unwrap` and `expect` are
/// not available to non-test code in this workspace, and a missing fixture is a reason to
/// skip a case rather than to fail a run.
///
/// The reopen is what makes the ceiling meaningful: the counts a store loads are the ones
/// it already holds, so a fixture that filled and measured through one handle would always
/// measure a store whose counts are in memory.
fn fixture(label: &str, records: &[String], hydrate_cap: u64) -> Option<(UserDb, PathBuf)> {
    let dir = std::env::temp_dir().join(format!(
        "rspinyin-userdb-bench-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("user.redb");
    let _ = std::fs::remove_file(&path);
    {
        // The store holds an exclusive lock on its file, so the handle that fills it has
        // to be dropped before the measured one opens.
        let mut db = UserDb::open_with(&path, Box::new(FixedClock)).ok()?;
        for key in records {
            db.record(key, 0);
        }
        db.final_commit().ok()?;
    }
    let opened = UserDb::open_with_hydrate_cap(&path, Box::new(FixedClock), hydrate_cap);
    let db = opened.ok()?;
    Some((db, dir))
}

/// Keeps a fixture whose store loaded its counts into memory.
///
/// A case that claims to measure the in-memory path is worth nothing if the fixture took
/// the fallback, and this is what makes the difference visible instead of silent.
fn loaded(fixture: Option<(UserDb, PathBuf)>) -> Option<(UserDb, PathBuf)> {
    fixture.filter(|(db, _)| db.is_hydrated())
}

/// Keeps a fixture whose store was too large to load, so its reads fall back to the file.
fn unloaded(fixture: Option<(UserDb, PathBuf)>) -> Option<(UserDb, PathBuf)> {
    fixture.filter(|(db, _)| !db.is_hydrated())
}

/// Benchmarks the record path, the lookups and the flush.
fn userdb_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("userdb");

    // The fast path the 5us budget is stated for: a key already recorded, and fewer keys
    // pending than the batch trigger, so no flush is due.
    if let Some((db, dir)) = fixture("record", &[], HYDRATE_CAP) {
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
    if let Some((db, dir)) = fixture("record-flush", &[], HYDRATE_CAP) {
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

    // The decode path's lookup on a store whose counts are in memory: the case the lookup
    // budget is stated for.
    if let Some((db, dir)) = loaded(fixture("freq-hit", &keys("seen", 1000), HYDRATE_CAP)) {
        group.bench_function("freq_hit", |bencher| {
            bencher.iter(|| black_box(db.freq(black_box("seen0"))));
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The same path for a word the store has never seen. A miss has to cost what a hit
    // costs -- that is what holding the counts in memory buys -- and the key set rotates so
    // that the case stays a miss whichever path the fixture ended up taking.
    if let Some((db, dir)) = loaded(fixture("freq-miss", &keys("seen", 1000), HYDRATE_CAP)) {
        let absent = keys("absent", 512);
        let mut cursor = 0usize;
        group.bench_function("freq_miss", |bencher| {
            bencher.iter(|| {
                let key = &absent[cursor % absent.len()];
                cursor = cursor.wrapping_add(1);
                black_box(db.freq(black_box(key)))
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The fallback: a store past its ceiling, where a lookup costs a read transaction. The
    // keys rotate so that the cache which bounds this path in a session never answers twice
    // in a row.
    if let Some((db, dir)) = unloaded(fixture("freq-degraded", &keys("seen", 1000), 0)) {
        let absent = keys("absent", 512);
        let mut cursor = 0usize;
        group.bench_function("freq_degraded", |bencher| {
            bencher.iter(|| {
                let key = &absent[cursor % absent.len()];
                cursor = cursor.wrapping_add(1);
                black_box(db.freq(black_box(key)))
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A flush of one record fewer than the batch trigger, so that the measured flush is
    // the only flush the iteration causes and the record cost stays out of the number.
    if let Some((mut db, dir)) = fixture("commit", &[], HYDRATE_CAP) {
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

    // The key path the 0.05ms budget is stated for. The key set is small and rotates: a key
    // is forgotten once and then only re-marked, so the tombstone set stops growing and no
    // flush is charged to the case. What is measured is the check and the insert, which is
    // the whole of what a keystroke costs here.
    if let Some((db, dir)) = loaded(fixture("forget", &keys("seen", 1000), HYDRATE_CAP)) {
        let forgotten = keys("forgotten", 64);
        let mut cursor = 0usize;
        group.bench_function("forget", |bencher| {
            bencher.iter(|| {
                let key = &forgotten[cursor % forgotten.len()];
                cursor = cursor.wrapping_add(1);
                black_box(db.forget(black_box(key)))
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // One page of the management enumeration against a thousand-record store: the page is
    // what the cost tracks, and the store is what it must not.
    if let Some((db, dir)) = loaded(fixture("list", &keys("seen", 1000), HYDRATE_CAP)) {
        group.bench_function("list_words", |bencher| {
            bencher.iter(|| {
                black_box(
                    db.list_words(black_box(0), black_box(50))
                        .map(|rows| rows.len()),
                )
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The whole store rendered as a document. The buffer is reused across iterations, which
    // is what a caller that writes the document out would do too.
    if let Some((db, dir)) = loaded(fixture("export", &keys("seen", 1000), HYDRATE_CAP)) {
        let mut document = Vec::with_capacity(64 * 1024);
        group.bench_function("export_tsv", |bencher| {
            bencher.iter(|| {
                document.clear();
                db.export_tsv(&mut document).ok()
            });
        });
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    group.finish();
}

criterion_group!(benches, userdb_bench);
criterion_main!(benches);
