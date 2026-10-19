//! Criterion benchmark for the commit path's script conversion.
//!
//! One claim is measured here: converting the final commit text through the script
//! table — the one allocation the traditional mode adds behind the commit door —
//! stays inside the commit-path budget the design states for it. The conversion runs
//! once per commit, never per keystroke and never inside the decode, so the input is
//! a realistic committed sentence rather than a synthetic lattice.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use ime_core::script::to_traditional;
use ime_dict::script::ScriptIndex;

/// The committed sentence the conversion is measured on.
///
/// Long enough to cover more than one mapped pair, short enough to be a commit a
/// chat window sees every day.
const COMMITTED: &str = "去银行取钱，然后回家吃饭。";

/// Measures one conversion of the committed sentence through the built-in table.
///
/// The index is built once, outside the loop: the commit path shares one process-wide
/// table, so building it is not the commit's cost to carry.
fn script_bench(c: &mut Criterion) {
    let index = ScriptIndex::seed();
    let mut group = c.benchmark_group("commit");
    group.bench_function("script_on", |bencher| {
        bencher.iter(|| black_box(to_traditional(black_box(COMMITTED), black_box(&index)).len()))
    });
    group.finish();
}

criterion_group!(benches, script_bench);
criterion_main!(benches);
