//! Criterion benchmarks for the user-decay arithmetic used by decode reads.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ime_dict::user_db::DecayParams;

/// The decay policy whose cold and warm decode costs the budget names.
const PARAMS: DecayParams = DecayParams {
    half_life_ticks: 139,
    floor_q16: 1 << 16,
};

/// A representative multi-commit accumulator in Q16.16.
const ACCUMULATOR: i64 = 32 << 16;

/// Measures the first fold of a stored record and its cached effective frequency.
///
/// The cold case advances the logical tick each iteration so the fold always does work;
/// the warm case reads the same already-folded age, matching the store's cache-hit shape.
fn decode_bench(criterion: &mut Criterion) {
    let mut cold = criterion.benchmark_group("decode");
    let mut tick = 0u64;
    cold.bench_function("user_decay_cold", |bencher| {
        bencher.iter(|| {
            tick = tick.wrapping_add(1);
            black_box(PARAMS.fold(black_box(ACCUMULATOR), 0, black_box(tick)))
        });
    });
    cold.bench_function("user_decay_warm", |bencher| {
        bencher.iter(|| black_box(PARAMS.effective_freq(black_box(32), black_box(ACCUMULATOR))));
    });
    cold.finish();
}

criterion_group!(benches, decode_bench);
criterion_main!(benches);
