//! Criterion benchmarks for the dictionary model's user-decay and bigram arithmetic.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fst::MapBuilder;
use ime_dict::format::{
    BigramRecord, DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer,
};
use ime_dict::fst_index::{DictLm, FstLexicon};
use ime_dict::user_db::DecayParams;
use ime_types::LanguageModel;

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

/// Builds a one-word v2 model whose `BIGRAM` section holds the hit pair.
///
/// The benchmark measures table lookup only, so the container is the smallest valid
/// index the loader accepts. The encoded image is leaked for `'static` and loaded
/// through `from_image`: a bound benchmark reads nothing from the machine it runs on,
/// which is the property the bench-sources audit asserts over these sources.
fn bigram_model(hit: bool) -> Option<DictLm> {
    let mut index = MapBuilder::memory();
    let packed = pack_fst_value(0, 1).ok()?;
    index.insert("ni".as_bytes(), packed).ok()?;
    let mut container = writer::DictWriter::new();
    let fst = index.into_inner().ok()?;
    let sections = [
        (SectionKind::Fst, fst),
        (
            SectionKind::Entries,
            DictEntry::new(0, 3, 1, 0, 900)
                .with_characters(1)
                .encode()
                .to_vec(),
        ),
        (SectionKind::StrPool, "你".as_bytes().to_vec()),
        {
            let mut unigram = hash_word("你").to_le_bytes().to_vec();
            unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
            unigram.extend_from_slice(&[0; 2]);
            (SectionKind::Unigram, unigram)
        },
        (SectionKind::WordList, 0u32.to_le_bytes().to_vec()),
    ];
    if hit {
        let pair = u64::from(hash_word("你")) << 32 | u64::from(hash_word("中国"));
        container
            .add_section(
                SectionKind::Bigram,
                BigramRecord::new(pair, PROB_Q12_MAX).encode().to_vec(),
            )
            .ok()?;
    }
    for (kind, payload) in sections {
        container.add_section(kind, payload).ok()?;
    }
    let image = container.encode().ok()?;
    let image: &'static [u8] = Box::leak(image.into_boxed_slice());
    let lexicon = Box::leak(Box::new(FstLexicon::from_image(image).ok()?));
    Some(DictLm::new(lexicon))
}

/// Measures the known-pair lookup and the unknown-pair fallback.
fn bigram_bench(criterion: &mut Criterion) {
    let Some(hit_model) = bigram_model(true) else {
        return;
    };
    let Some(miss_model) = bigram_model(false) else {
        return;
    };
    let mut bigram = criterion.benchmark_group("decode");
    bigram.bench_function("bigram_hit", |bencher| {
        bencher.iter(|| black_box(&hit_model).bigram(black_box("你"), black_box("中国")))
    });
    bigram.bench_function("bigram_miss", |bencher| {
        bencher.iter(|| black_box(&miss_model).bigram(black_box("你"), black_box("没有")))
    });
    bigram.finish();
}

criterion_group!(benches, decode_bench, bigram_bench);
criterion_main!(benches);
