//! Criterion benchmarks for the decode path, and the cases the decode budget is
//! asserted against.
//!
//! The case this file exists for is `decode/12syl`: one decode of a twelve-syllable
//! input, which is the shape `BUDGET-LAT-02` (P99 3ms) is stated for. The other
//! cases widen the picture -- the shorter inputs show where the cost comes from, the
//! sixty-four-byte case is the worst input the buffer accepts at all, `dag_build`
//! separates segmentation from the rest, and `lm/edge_score` times the operation the
//! sweep runs once per lattice edge.
//!
//! `decode/holdout` decodes the whole held-out evaluation set in one iteration,
//! which is the shape the held-out budget is stated for; its sample count is the one
//! thing that makes it slower to run than the rest, because one iteration is already
//! thousands of decodes.
//!
//! `decode/correction-off`, `decode/correction-on-hit` and `decode/correction-on-miss`
//! are the spelling-correction switch's three modes on the reuse case: the mainline
//! decode, the same input with the bit set -- which the miss gate is to leave
//! untouched -- and a one-slip misspelling of it, which is the shape the correction
//! pass's own budget is stated for.
//!
//! `decode/viterbi` and `decode/viterbi_into` measure the same walk through the two entry
//! points, on the input the latency budget is stated for: the first builds a workspace per
//! decode, the second keeps one and decodes into it. The difference between them is what the
//! workspace costs, which is the number the decode-reuse work is judged by.
//!
//! `session/keystroke` moves the same question up one layer, to the session the engine
//! drives: one keystroke re-segments, re-decodes, rebuilds the preedit and builds the frame
//! the window draws from, and the number it reports is the cost of that whole path, which is
//! what a user waits for between two letters.
//!
//! `lattice/build` separates the dictionary walk from the rest of a decode: it is the one
//! part of the pipeline that reads the dictionary once per span of every reading of the
//! input, and it is where the character count an edge carries is computed.
//!
//! `lm/unigram_memory` times one unigram lookup of the in-memory model, which is the
//! reference half of the language-model comparison: the dictionary-backed model answers the
//! same question out of the compiled dictionary's unigram table, and its case lives in that
//! crate's own bench target, because this one may not depend on it.
//!
//! `alloc/decode` and `alloc/keystroke` are the same two paths seen by the process's
//! counting allocator: the case reads the counter around the routine and consumes the
//! delta, so a run reports the cost of the work with the observation inside it, and a
//! case whose allocation count stopped being the same on every iteration fails rather
//! than reporting. The thresholds the counts are judged against live in
//! `docs/dev/budgets.json`, and the runs that assert them are the integration test in
//! `tests/alloc_budget.rs` and `xtask budget`, which read each other's numbers.
//!
//! This file measures. Comparing the measurements against the thresholds is
//! `xtask budget --check`, which reads the same numbers out of
//! `docs/dev/budgets.json` and the criterion output this run leaves in
//! `target/criterion`.
//!
//! Nothing here touches the network, the clock, `$HOME` or a compiled dictionary:
//! the corpus is generated from the constants in [`fixtures`] and the held-out set
//! is embedded in the binary, so a run is reproducible and needs no write access
//! outside `target/`. The case table is checked against its own inputs before the
//! first measurement, so a case cannot report a number for a shape other than the
//! one its name claims.

// The two split-out bench modules live under `decode/` beside this file: an explicit path
// keeps them out of the target auto-discovery a bare `benches/*.rs` file would trigger.
#[path = "decode/alloc.rs"]
mod alloc;
#[path = "decode/components.rs"]
mod components;
mod fixtures;

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput};
use ime_core::lm::{InMemoryLm, LOG2_FLOOR, Scorer};
use ime_core::segment::{Readings, SyllableDag};
use ime_core::state::{Effect, Session, SessionConfig, SessionEnv};
use ime_core::viterbi::lattice::LatticeOptions;
use ime_core::viterbi::{DecodeScratch, DecodeSources, Decoder, build_lattice};
use ime_types::{DecodeFlags, DecodeRequest, KeyAction, LanguageModel, UserFreqSource};

use fixtures::{CASES, SyntheticDict, holdout_keys};

/// Samples the quick mode runs with.
const QUICK_SAMPLES: usize = 20;

/// Samples the full mode runs with.
const FULL_SAMPLES: usize = 200;

/// Lattice edges the `lm/edge_score` case scores per iteration.
const EDGE_SCORES: usize = 1_000;

/// Unigram lookups the `lm/unigram_memory` case makes per iteration.
///
/// The same count as the edge-score case, so that both throughput columns are the cost of
/// one lookup and can be read side by side.
const UNIGRAM_LOOKUPS: usize = EDGE_SCORES;

/// The case the reuse pair measures: the twelve-syllable input of `BUDGET-LAT-02`, which is
/// the shape a decode's working set is largest for.
const REUSE_CASE: &str = "12syl";

/// Unigram score of the corpus's most frequent word, in Q8.8 log probability.
const UNIGRAM_TOP: i32 = -256;

/// Unigram score one doubling of the rank costs, in Q8.8 log probability.
const UNIGRAM_FALLOFF: i32 = -128;

/// The input the session case has typed when the measurement starts.
///
/// The eight-syllable case of the decode table, so the session holds a real candidate
/// list and the frame it builds carries a full page rather than a fallback reading.
const SESSION_SEED: &str = "jintiantianqizhenbucuohao";

/// The keystrokes one iteration of the session case measures.
///
/// Ten characters, which with the seed leave the input at thirty-five bytes: inside
/// the sixty-four-byte cap the buffer enforces, so none of the ten is dropped as too
/// long and the case measures a keystroke rather than a diagnostic.
const SESSION_KEYS: &str = "womenmingt";

/// The slipped spelling the on-miss case decodes: the reuse case with the first two
/// letters of its seventh syllable transposed, `wan` → `awn`.
///
/// The transposition has to be exactly this one for the case to be a miss at all: no
/// syllable starts with `wn`, so the misspelled span admits no exact reading and the
/// miss gate opens -- while the region's other one-letter reading, the interjection
/// `a`, dead-ends because nothing follows it. Of the span's one-step corrections only
/// `wan` itself is a syllable, and the word the corpus carries for it is the word the
/// exact spelling decodes to, so the case measures the correction pass on a real miss
/// whose reading is the exact one. The preflight in [`bench_correction`] holds this
/// constant against the case table, so editing one spelling without the other fails
/// the run instead of measuring a different slip.
const MISS_INPUT: &str = "womenmingtianqugongyuanawnbazhongguoxue";

/// Runs every case of this benchmark target.
///
/// The dictionary and the model are built once, before any measurement, and the
/// held-out set is embedded in the binary, so a run needs no file and no `$HOME`.
/// Building them also checks the case table: every input is cut and compared with the
/// size its case declares, so a run whose cases are not the shapes they name stops
/// before the first measurement instead of reporting a number for a shape the budget
/// was not stated for.
///
/// # Errors
/// Returns a description of the first benchmark input that cannot be cut into
/// syllables or that does not spell the size its case declares, and whatever
/// criterion reports about the command line.
use self::alloc::bench_allocations;
use self::components::{bench_lattice, bench_rank_decay, bench_unigram};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dictionary = fixtures::build()?;
    let model = model(&dictionary);

    let mut criterion = Criterion::default().configure_from_args();
    if let Some(samples) = sample_size() {
        criterion = criterion.sample_size(samples);
    }

    bench_decode(&mut criterion, &dictionary, &model);
    bench_candidate_supply(&mut criterion, &dictionary, &model);
    bench_decode_reuse(&mut criterion, &dictionary, &model);
    bench_fuzzy(&mut criterion, &dictionary, &model);
    bench_correction(&mut criterion, &dictionary, &model);
    bench_context_rerank(&mut criterion, &dictionary, &model);
    bench_allocations(&mut criterion, &dictionary, &model);
    bench_session(&mut criterion, &dictionary, &model);
    bench_holdout(&mut criterion, &dictionary, &model);
    bench_segment(&mut criterion);
    bench_correction_build(&mut criterion);
    bench_lattice(&mut criterion, &dictionary);
    bench_rank_decay(&mut criterion);
    bench_edge_score(&mut criterion, &dictionary, &model);
    bench_unigram(&mut criterion, &dictionary, &model);

    criterion.final_summary();
    Ok(())
}

/// Sample count for this run, or `None` to keep criterion's own default.
///
/// The design fixes two modes: `RSPINYIN_BENCH_MODE=quick` drops the sample count to
/// twenty for the pass CI runs, and `RSPINYIN_BENCH_MODE=full` raises it to two
/// hundred for a run whose numbers are meant to be kept.
///
/// The mode is an environment variable rather than a flag because criterion parses
/// the command line itself with a strict parser and refuses an option it does not
/// know, so a `--full` would stop the benchmark before it started. Criterion's own
/// `--quick` remains available; it switches the sampler rather than the sample
/// count, which is why it does not appear here.
fn sample_size() -> Option<usize> {
    match std::env::var("RSPINYIN_BENCH_MODE").as_deref() {
        Ok("quick") => Some(QUICK_SAMPLES),
        Ok("full") => Some(FULL_SAMPLES),
        _ => None,
    }
}

/// The model the cases score with: one unigram per word of the corpus.
///
/// There are no bigrams, which is the shape of the shipped model rather than a
/// simplification: the v1 dictionary carries an empty `BIGRAM` section, so a decode
/// against it scores every edge with the unigram term and the miss penalty.
fn model(dictionary: &SyntheticDict) -> InMemoryLm {
    let mut model = InMemoryLm::new();
    for (rank, text) in dictionary.texts().enumerate() {
        model.insert_unigram(text, unigram_at(rank));
    }
    model
}

/// The unigram score of the word at `rank`, in Q8.8 log probability.
///
/// Zipf-shaped, like the weights: the most frequent word carries the top score and
/// every doubling of the rank costs the same amount, down to the contract's own
/// floor for a word the model has nothing to say about.
fn unigram_at(rank: usize) -> i32 {
    let doublings = i32::try_from(usize::BITS - rank.leading_zeros()).unwrap_or(0);
    (UNIGRAM_TOP + UNIGRAM_FALLOFF * doublings).max(LOG2_FLOOR)
}

/// Times one decode per case, each against the generated corpus.
///
/// The request is cloned in the setup half of `iter_batched`, so the allocation a
/// request costs is outside what the routine measures; the decoder and the
/// dictionary are built once and reused, so no case pays for either.
fn bench_decode(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let decoder = Decoder::default();
    let user = NoUser;
    let mut group = criterion.benchmark_group("decode");
    for case in CASES {
        let request = DecodeRequest::new(case.raw);
        group.bench_function(case.name, |bencher| {
            bencher.iter_batched(
                || request.clone(),
                |request| {
                    let result = decoder.decode(black_box(&request), dictionary, &user, model);
                    black_box(result.candidates.len())
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Times the three rungs of the candidate-materialization supply ladder.
///
/// Each case is the full decode the session would pay when paging reaches that depth.
/// The growing totals make the extra materialization visible; the report is criterion's,
/// while `xtask budget --check` remains the authority for comparing it with a budget.
fn bench_candidate_supply(
    criterion: &mut Criterion,
    dictionary: &SyntheticDict,
    model: &InMemoryLm,
) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let request = DecodeRequest::new(case.raw);
    let decoder = Decoder::default();
    let user = NoUser;
    let mut group = criterion.benchmark_group("decode");
    group.throughput(Throughput::Elements(1));
    for quota in [
        ime_core::viterbi::decoder::INITIAL_CANDIDATE_QUOTA,
        ime_core::viterbi::decoder::EXPANDED_CANDIDATE_QUOTA,
        ime_core::viterbi::decoder::SUPPLY_CANDIDATE_CAP,
    ] {
        group.bench_function(format!("supply-{quota}"), |bencher| {
            bencher.iter_batched(
                || request.clone(),
                |request| {
                    let result = decoder.decode_with_candidate_quota(
                        black_box(&request),
                        dictionary,
                        &user,
                        model,
                        quota,
                    );
                    black_box(result.map(|decoded| decoded.candidates.len()))
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Times the fuzzy classes' two modes against the same input.
///
/// The widening's cost is conditional by design: a section with no class selected
/// adds nothing to the decode path, and the whole cost lands on the enabled mode.
/// Both modes are measured on the same case, so the pair reads as one claim -- the
/// gap between the two is what the classes cost, and the closed group is the proof
/// that the closed path paid nothing.
fn bench_fuzzy(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let decoder = Decoder::default();
    let user = NoUser;
    let off = DecodeRequest::new(case.raw);
    let mut on = DecodeRequest::new(case.raw);
    // Every class at once, which is the widest the section can project: the master
    // switch plus the eight class bits, the mask the projection computes for
    // `[fuzzy] enabled = true`.
    on.flags
        .insert(ime_core::fuzzy::projection(true, DecodeFlags::empty()));
    let mut abbrev = DecodeRequest::new(case.raw);
    abbrev.flags.insert(DecodeFlags::ABBREV);
    let mut group = criterion.benchmark_group("decode");
    group.bench_function("fuzzy-off", |bencher| {
        bencher.iter_batched(
            || off.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("fuzzy-on", |bencher| {
        bencher.iter_batched(
            || on.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    // The abbreviation switch shares the budget the two fuzzy modes bound: its walk
    // runs beside the fuzzy one and under the same edge ceiling, and the enabled mode
    // is what the budget is stated against.
    group.bench_function("abbrev-on", |bencher| {
        bencher.iter_batched(
            || abbrev.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times the spelling-correction switch's three modes against the reuse case.
///
/// Three modes, one claim: the switch costs an exact hit nothing, and a miss is what
/// its own budget is stated for. `correction-off` is the mainline decode;
/// `correction-on-hit` is the same input with the bit set, which the miss gate is
/// supposed to leave byte-identical; `correction-on-miss` is the slipped spelling of
/// [`MISS_INPUT`], the shape the correction pass's exclusive budget is stated for.
///
/// The preflight refuses to measure before the claims hold on the case table's own
/// inputs: the slip is exactly one transposition of the exact spelling, the exact
/// build of the miss has no path at all, the corrected build answers it, and the
/// corrected miss decodes for real -- a full reading of the input whose winning path
/// crossed a correction edge, the annotation being the record of that. (The exact
/// spelling's own decode is deliberately not held up as the expected reading: it has
/// cuts through the interjections that the misspelled input has lost, so the two
/// candidate lists are legitimately different, and only the on-hit pair has to agree.)
///
/// # Panics
///
/// When a preflight claim does not hold, because a number measured past it would be
/// a number for a shape the switch's budget was not stated for.
fn bench_correction(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    // The slip is one adjacent transposition of the exact spelling -- the `wa` of
    // its seventh syllable exchanged for `aw`, the two bytes everything else in the
    // preflight depends on -- which is what makes the on-hit and on-miss numbers
    // comparable at all.
    assert_eq!(
        &case.raw[23..25],
        "wa",
        "the exact spelling carries `wa` where the slip carries `aw`: {case:?}"
    );
    assert_eq!(
        &MISS_INPUT[23..25],
        "aw",
        "the slip is the `wa` → `aw` transposition"
    );
    assert_eq!(
        case.raw.len(),
        MISS_INPUT.len(),
        "the slip preserves the length of the exact spelling"
    );
    let differing = case
        .raw
        .bytes()
        .zip(MISS_INPUT.bytes())
        .filter(|(exact, slipped)| exact != slipped)
        .count();
    assert_eq!(
        differing, 2,
        "the slip is one adjacent transposition: {case:?} against {MISS_INPUT:?}"
    );
    // The miss gate is the whole design: an input that cuts exactly never enters the
    // correction pass, so the gate has to open on the miss and only on the miss.
    let mut dag = SyllableDag::new();
    assert!(
        dag.build(MISS_INPUT).is_err(),
        "the miss input must not cut exactly, or the gate never opens"
    );
    assert!(
        dag.build_for(MISS_INPUT, DecodeFlags::default() | DecodeFlags::CORRECTION)
            .is_ok(),
        "the correction pass must find a reading for the miss input"
    );
    // Both corrected decodes answer with a real reading: the exact hit decodes to
    // the same first candidate with the bit on, and the miss is not the pass-through
    // echo but a full reading of the input whose winning path crossed a correction
    // edge -- which the annotation is the record of.
    let decoder = Decoder::default();
    let user = NoUser;
    let off = DecodeRequest::new(case.raw);
    let mut on_hit = DecodeRequest::new(case.raw);
    on_hit.flags.insert(DecodeFlags::CORRECTION);
    let on_miss = {
        let mut request = DecodeRequest::new(MISS_INPUT);
        request.flags.insert(DecodeFlags::CORRECTION);
        request
    };
    let exact_top = top_text(&decoder, &off, dictionary, &user, model);
    assert_eq!(
        top_text(&decoder, &on_hit, dictionary, &user, model),
        exact_top,
        "an exact hit must decode to the same reading with the bit on"
    );
    let miss_result = decoder.decode(&on_miss, dictionary, &user, model);
    assert!(
        !miss_result.degraded && miss_result.candidates.len() > 1,
        "the corrected miss is a real decode, not the pass-through echo: {miss_result:?}"
    );
    let miss_top = miss_result.candidates.first();
    assert_eq!(
        miss_top.map(|candidate| candidate.consumed_syllables),
        Some(u16::try_from(case.syllables).unwrap_or(u16::MAX)),
        "the corrected miss reads the whole input"
    );
    // The annotation names the correction note; it may also carry the reading hint the
    // sweep prepends, so the property asserted is the note's presence and not the whole
    // string -- the hint's own shape is the sweep's business and pinned there.
    let correction_note = format!(
        "{}{MISS_INPUT}",
        ime_core::segment::correct::CORRECTION_ANNOTATION_PREFIX
    );
    let miss_annotation = miss_top.and_then(|candidate| candidate.annotation.clone());
    assert!(
        miss_annotation
            .as_deref()
            .is_some_and(|annotation| annotation.contains(correction_note.as_str())),
        "the winning path crossed a correction edge, and the annotation says so: \
         {miss_annotation:?}"
    );
    let mut group = criterion.benchmark_group("decode");
    group.bench_function("correction-off", |bencher| {
        bencher.iter_batched(
            || off.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("correction-on-hit", |bencher| {
        bencher.iter_batched(
            || on_hit.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("correction-on-miss", |bencher| {
        bencher.iter_batched(
            || on_miss.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// The text of one decode's first candidate, for the preflight comparisons.
fn top_text(
    decoder: &Decoder,
    request: &DecodeRequest,
    dictionary: &SyntheticDict,
    user: &dyn UserFreqSource,
    model: &InMemoryLm,
) -> String {
    decoder
        .decode(request, dictionary, user, model)
        .candidates
        .first()
        .map(|candidate| candidate.text.clone())
        .unwrap_or_default()
}

/// Times the context re-rank on the reuse case's own candidate list.
///
/// This is the post-pass the session runs once per decode when it holds a context
/// word: one band walk over the head of the list and one bigram lookup per
/// candidate inside it. The list and the model are built in the setup half, so the
/// measurement is the re-rank alone -- the number the context switch's exclusive
/// budget is stated for. The context word is a word the list holds, so the model
/// has continuations to find; a word it has none for would measure an early exit.
fn bench_context_rerank(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let request = DecodeRequest::new(case.raw);
    let decoder = Decoder::default();
    let user = NoUser;
    let result = decoder.decode(&request, dictionary, &user, model);
    let list = result.candidates.clone();
    // The context is the text of the list's own head, which the model holds
    // unigrams for -- the shape of the word a landed commit leaves behind.
    let context = list
        .first()
        .map(|candidate| candidate.text.clone())
        .unwrap_or_default();
    let mut group = criterion.benchmark_group("decode");
    group.bench_function("context_rerank", |bencher| {
        bencher.iter_batched(
            || list.clone(),
            |mut candidates| {
                ime_core::viterbi::context::rerank_with_context(
                    &mut candidates,
                    Some(&context),
                    model,
                );
                black_box(candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times the two entry points against each other, on the input the latency budget is stated
/// for.
///
/// `viterbi` builds a workspace for every decode, which is what `decode/12syl` measures under
/// its own name; `viterbi_into` keeps one workspace and decodes into it, which is the shape a
/// session runs once per keystroke. Both run the same walk over the same lattice, so the
/// difference between them is what building the workspace costs -- the sweep's beams, the
/// candidate drafts and the result buffers -- and nothing else. What the reuse is worth in
/// allocations is `alloc/decode`'s to report, and the budget it is judged against is the
/// integration test's to assert.
fn bench_decode_reuse(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let request = DecodeRequest::new(case.raw);
    let decoder = Decoder::default();
    let user = NoUser;
    let mut scratch = DecodeScratch::new();
    let mut group = criterion.benchmark_group("decode");
    group.bench_function("viterbi", |bencher| {
        bencher.iter_batched(
            || request.clone(),
            |request| {
                let result = decoder.decode(black_box(&request), dictionary, &user, model);
                black_box(result.candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("viterbi_into", |bencher| {
        bencher.iter_batched(
            || request.clone(),
            |request| {
                decoder.decode_into(
                    &mut scratch,
                    black_box(&request),
                    DecodeSources {
                        lexicon: dictionary,
                        user_freq: &user,
                        lm: model,
                        english: None,
                        hints: None,
                    },
                );
                black_box(scratch.result().candidates.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times one full pass over the held-out evaluation set.
///
/// One iteration decodes every entry of the set, which is what the held-out budget
/// bounds, so the reported mean is a whole pass and the throughput column is the
/// same pass divided by its entry count. The requests are built once, before the
/// measurement.
fn bench_holdout(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let requests: Vec<DecodeRequest> = holdout_keys()
        .iter()
        .map(|key| DecodeRequest::new(key.as_str()))
        .collect();
    let decoder = Decoder::default();
    let user = NoUser;
    let mut group = criterion.benchmark_group("decode");
    group.throughput(Throughput::Elements(requests.len() as u64));
    group.bench_function("holdout", |bencher| {
        bencher.iter_batched(
            || (),
            |()| {
                let mut decoded = 0usize;
                for request in &requests {
                    decoded += decoder
                        .decode(black_box(request), dictionary, &user, model)
                        .candidates
                        .len();
                }
                black_box(decoded)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times building the segmentation graph, which a decode includes but does not
/// separate from the rest.
///
/// The inputs rotate over the case table, so the number covers the short inputs and
/// the sixty-four-byte one alike; one graph is reused across iterations, which is
/// what the engine does across keystrokes, so the measurement does not include an
/// allocation the decode path never pays.
fn bench_segment(criterion: &mut Criterion) {
    let mut dag = SyllableDag::new();
    let mut group = criterion.benchmark_group("segment");
    group.bench_function("dag_build", |bencher| {
        let mut next = 0usize;
        bencher.iter_batched(
            || {
                let raw = CASES.get(next).map_or("", |case| case.raw);
                next = (next + 1) % CASES.len().max(1);
                raw
            },
            |raw| dag.build(black_box(raw)).is_ok(),
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times the correction pass's expansion on its own, against the same miss input.
///
/// The exclusive cost the correction switch's budget is stated for is the expansion:
/// the walk that asks every non-exact span of a missed input for its one-step
/// corrections. `correction-base` is the exact build of the miss, which fails fast
/// and is what a closed switch would pay; `correction-miss` is the same build with
/// the bit set, which fails, expands and re-checks. The difference between the two
/// means is the expansion's own cost, the number the budget is written against --
/// the `decode/correction-*` trio, in contrast, reports a whole decode, where the
/// wider graph's lattice and sweep costs ride along.
fn bench_correction_build(criterion: &mut Criterion) {
    let mut dag = SyllableDag::new();
    let base = DecodeFlags::default();
    let miss = DecodeFlags::default() | DecodeFlags::CORRECTION;
    let mut group = criterion.benchmark_group("segment");
    group.bench_function("correction-base", |bencher| {
        bencher.iter(|| dag.build_for(black_box(MISS_INPUT), base).is_err())
    });
    group.bench_function("correction-miss", |bencher| {
        bencher.iter(|| dag.build_for(black_box(MISS_INPUT), miss).is_ok())
    });
    group.finish();
}

/// Times the operation the sweep runs once per lattice edge.
///
/// One iteration scores [`EDGE_SCORES`] words in sequence, each with the one before
/// it as its predecessor, so the conditional term is looked up on every call; the
/// throughput column is the per-edge cost, which is the number the design's edge
/// budget is stated for.
fn bench_edge_score(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let words: Vec<&str> = dictionary.texts().take(EDGE_SCORES).collect();
    let scorer = Scorer::default();
    let user = NoUser;
    let mut group = criterion.benchmark_group("lm");
    group.throughput(Throughput::Elements(words.len() as u64));
    group.bench_function("edge_score", |bencher| {
        bencher.iter_batched(
            || (),
            |()| {
                let mut total = 0i32;
                let mut previous: Option<&str> = None;
                for word in &words {
                    let score = scorer.edge_score(model, &user, previous, black_box(*word), 2);
                    total = total.wrapping_add(score);
                    previous = Some(*word);
                }
                black_box(total)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Times one keystroke against a live session.
///
/// This is the case the frame path is judged by. Every keystroke re-segments,
/// re-decodes, rebuilds the preedit and builds the `UiFrame` the window draws from --
/// the preedit snapshot, the page of candidates and the mode strip -- and that frame
/// is what the session hands over per key. One iteration types [`SESSION_KEYS`] into
/// the session the setup half left composing, so the reported time is what those
/// keystrokes cost together. The cost of one keystroke is that number divided by their
/// count, which is the quantity the case is named for: criterion reports a mean per
/// iteration, and one iteration is the whole run of keys.
///
/// The seed is typed in the setup half, which keeps the allocations it costs outside
/// the measurement: what the iteration measures is a session that is already
/// composing, which is the state nine keystrokes in ten arrive in. What a keystroke
/// allocates is `alloc/keystroke`'s to report, under the same counting allocator the
/// decode case reads.
///
/// # Panics
///
/// When a keystroke of the iteration does not reach the frame path -- one dropped as
/// too long, or a session that is not composing -- because the case would then be
/// measuring a diagnostic rather than a keystroke.
fn bench_session(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let cfg = SessionConfig::default();
    let decoder = Decoder::default();
    let user = NoUser;
    let env = SessionEnv {
        decoder: &decoder,
        lexicon: dictionary,
        user_freq: &user,
        lm: model,
        english: None,
        confusables: None,
        symbols: None,
    };
    let mut group = criterion.benchmark_group("session");
    group.bench_function("keystroke", |bencher| {
        bencher.iter_batched(
            || {
                let mut session = Session::new();
                for ch in SESSION_SEED.chars() {
                    let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
                }
                session
            },
            |mut session| {
                let mut frames = 0usize;
                for ch in SESSION_KEYS.chars() {
                    let effects = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
                    frames += effects
                        .iter()
                        .filter(|effect| matches!(effect, Effect::SendFrame(_)))
                        .count();
                }
                assert_eq!(
                    frames,
                    SESSION_KEYS.len(),
                    "each keystroke repaints the window"
                );
                black_box(frames)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// A user-frequency source that has recorded nothing.
///
/// The benchmark measures the engine, not the user's history, and a source that
/// answers zero is what the decode path sees before the first commit.
struct NoUser;

impl UserFreqSource for NoUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}
