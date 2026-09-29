//! Criterion benchmark for the passthrough classifier.
//!
//! The budget this measures is the 500ns **per call** the design names, so one
//! iteration of this benchmark is exactly one `classify` call: the case is chosen by a
//! counter that walks the corpus. `xtask budget --check` compares criterion's
//! per-iteration mean against that per-call budget, so a benchmark that swept the whole
//! corpus per iteration would be measuring thirteen calls while the budget stayed the
//! cost of one -- and would report a violation on a build that met it.
//!
//! Nothing here touches the filesystem, the clock or the network, and the corpus is
//! fixed, so a run is reproducible.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
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

/// Times one call of [`classify`], walking the corpus one case per iteration.
///
/// The counter and the `black_box` around the result keep the optimizer from
/// discarding the calls; both are a few instructions against a function that allocates
/// on one of its branches, so neither shows up against the budget they are measured
/// beside.
fn bench_classify(criterion: &mut Criterion) {
    let cases = corpus();
    let mut group = criterion.benchmark_group("passthrough");
    group.bench_function("classify", |bencher| {
        let mut next = 0usize;
        bencher.iter(|| {
            let (raw, flags, surrounding) = cases[next % cases.len()];
            next = next.wrapping_add(1);
            black_box(classify(black_box(raw), flags, surrounding))
        });
    });
    group.finish();
}

criterion_group!(benches, bench_classify);
criterion_main!(benches);
