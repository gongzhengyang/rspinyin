//! The component benches: the lattice walk and the unigram scan, measured alone.
//!
//! Split out of `decode.rs` to keep that file inside the line limit. A decode includes
//! both, so each exists to make a change to one of them measurable on its own rather
//! than as a difference inside a whole decode.

use super::*;

/// Times building the word lattice of the widest input the buffer accepts.
///
/// The lattice is where the dictionary meets the graph: every span of every reading of the
/// input is spelled into a key and looked up, and every word that comes back becomes an
/// edge. A decode includes this work, so the case exists to make a change to the edge
/// construction -- the capacity the edge vector is given, the character count an edge
/// carries -- measurable on its own rather than as a difference inside a whole decode.
///
/// The input is [`REUSE_CASE`], the twelve-syllable case the latency budget is stated for,
/// and the graph is built once outside the measurement: the case measures the walk, not the
/// segmentation that feeds it.
pub(super) fn bench_lattice(criterion: &mut Criterion, dictionary: &SyntheticDict) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let mut dag = SyllableDag::new();
    assert!(dag.build(case.raw).is_ok(), "the case input is segmentable");
    let user = NoUser;
    let mut readings = Readings::new();
    let mut keys = Vec::new();
    let mut group = criterion.benchmark_group("lattice");
    group.throughput(Throughput::Elements(1));
    group.bench_function("build", |bencher| {
        bencher.iter_batched(
            || (),
            |()| {
                let lattice = build_lattice(
                    black_box(&dag),
                    dictionary,
                    &user,
                    LatticeOptions {
                        fallback_single: true,
                        flags: DecodeFlags::empty(),
                        readings: &mut readings,
                        keys: &mut keys,
                        english: None,
                    },
                );
                black_box(lattice.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times rank-decay truncation of a full whole-sentence candidate window.
///
/// The list is synthetic and carries Q8.8 scores directly, so the case isolates the
/// decay walk from lattice construction and text materialization. A monotone list that
/// truncates partway exercises the expensive outcome: it walks every sentence and moves
/// the word-level tail into the freed slots.
pub(super) fn bench_rank_decay(criterion: &mut Criterion) {
    let candidates: Vec<(i32, bool)> = (0..64)
        .map(|rank| (10_000 - rank * 200, rank < 48))
        .collect();
    let mut group = criterion.benchmark_group("kbest");
    group.throughput(Throughput::Elements(candidates.len() as u64));
    group.bench_function("rank_decay", |bencher| {
        bencher.iter_batched(
            || candidates.clone(),
            |mut candidates| {
                let dropped = ime_core::viterbi::kbest::retain_rank_decayed_sentences(
                    &mut candidates,
                    |candidate| candidate.1,
                    |candidate| candidate.0,
                );
                black_box(dropped)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times one unigram lookup of the in-memory model.
///
/// The reference half of the language-model comparison: the dictionary-backed model answers
/// the same question out of the compiled dictionary's unigram table, where a word is found
/// by hashing it and binary-searching fixed-width records, while this one walks a map keyed
/// by the word's own text and compares strings at every step. Its counterpart case belongs
/// in `ime-dict`'s bench target, next to the container it measures, because this target may
/// not depend on that crate.
pub(super) fn bench_unigram(
    criterion: &mut Criterion,
    dictionary: &SyntheticDict,
    model: &InMemoryLm,
) {
    let words: Vec<&str> = dictionary.texts().take(UNIGRAM_LOOKUPS).collect();
    let mut group = criterion.benchmark_group("lm");
    group.throughput(Throughput::Elements(words.len() as u64));
    group.bench_function("unigram_memory", |bencher| {
        bencher.iter_batched(
            || (),
            |()| {
                let mut total = 0i32;
                for word in &words {
                    total = total.wrapping_add(model.unigram(black_box(*word)));
                }
                black_box(total)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}
