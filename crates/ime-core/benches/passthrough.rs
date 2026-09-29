//! Criterion benchmark for the passthrough classifier.
//!
//! The budget this measures is the 500ns per call the design names. Throughput is
//! reported in elements, so criterion's `mean` is the cost of one `classify` call
//! rather than the cost of one sweep over the corpus, which is what the budget
//! assertion compares against.
//!
//! Nothing here touches the filesystem, the clock or the network, and the corpus is
//! fixed, so a run is reproducible.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ime_core::passthrough::{PassthroughFlags, PunctMode, classify};

/// One benchmark case: the text a key press produced, the flags, and the text
/// around the caret.
type Case = (&'static str, PassthroughFlags, Option<&'static str>);

/// The corpus, spread over every rule of the decision order: an empty key, pinyin,
/// the uppercase rule, the punctuation table, and the URL and email heuristics both
/// in the key's own text and under the caret.
fn corpus() -> Vec<Case> {
    let defaults = PassthroughFlags {
        auto_english_on_uppercase: true,
        passthrough_url: true,
        punct_mode: PunctMode::Chinese,
        full_width: false,
        temp_english: false,
    };
    vec![
        ("", defaults, None),
        ("n", defaults, None),
        ("nihao", defaults, None),
        ("ni'hao", defaults, None),
        ("N", defaults, None),
        (",", defaults, None),
        ("\"", defaults, Some("他说：“你好")),
        ("'", defaults, None),
        ("@", defaults, None),
        ("www.example.com", defaults, None),
        ("n", defaults, Some("https://example.com/a")),
        ("n", defaults, Some("foo@bar")),
        ("n", defaults, Some("今天天气不错 ")),
    ]
}

/// Times one call of [`classify`] over the whole corpus.
///
/// The consumed-count accumulator keeps the optimizer from discarding the calls
/// whose result is otherwise unused; it is a few instructions against a function
/// that allocates on one of its branches.
fn bench_classify(criterion: &mut Criterion) {
    let cases = corpus();
    let mut group = criterion.benchmark_group("passthrough");
    group.throughput(Throughput::Elements(cases.len() as u64));
    group.bench_function("classify", |bencher| {
        bencher.iter(|| {
            let mut consumed = 0u32;
            for (raw, flags, surrounding) in &cases {
                if classify(black_box(raw), *flags, *surrounding).is_consumed() {
                    consumed += 1;
                }
            }
            black_box(consumed)
        });
    });
    group.finish();
}

criterion_group!(benches, bench_classify);
criterion_main!(benches);
