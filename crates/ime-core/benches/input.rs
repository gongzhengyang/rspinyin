//! Criterion benchmarks for the input buffer.
//!
//! The `input/buffer_ops` case times one `push_char` followed by one `backspace`:
//! the two operations the input path runs on every keystroke. They are measured
//! together because the pair is a single allocation-free steady state -- the buffer
//! ends each iteration exactly where it started -- so the reported time is the cost
//! of a keystroke round trip, and the per-operation budget holds with a factor of
//! two to spare whenever the round trip stays inside it.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ime_core::input::InputBuffer;

/// Benchmarks the keystroke round trip of the input buffer.
fn bench_buffer_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("input");
    group.bench_function("buffer_ops", |b| {
        // One buffer for the whole run: its capacity is warm, so the measurement
        // never includes the first allocation.
        let mut buf = InputBuffer::new();
        b.iter(|| {
            let pushed = buf.push_char(black_box('n'));
            let removed = buf.backspace();
            black_box((pushed, removed))
        });
    });
    group.finish();
}

criterion_group!(benches, bench_buffer_ops);
criterion_main!(benches);
