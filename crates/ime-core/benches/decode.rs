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
//! This file measures. Comparing the measurements against the thresholds is
//! `xtask budget --check`, which reads the same numbers out of
//! `docs/dev/budgets.json` and the criterion output this run leaves in
//! `target/criterion`.
//!
//! Nothing here touches the network, the clock, `$HOME` or a compiled dictionary:
//! the corpus is generated from the constants in [`fixtures`] and the held-out set
//! is embedded in the binary, so a run is reproducible and needs no write access
//! outside `target/`.

mod fixtures;

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput};
use ime_core::lm::{InMemoryLm, LOG2_FLOOR, Scorer};
use ime_core::segment::SyllableDag;
use ime_core::state::{Effect, Session, SessionConfig, SessionEnv};
use ime_core::viterbi::{DecodeScratch, Decoder};
use ime_types::{DecodeRequest, KeyAction, UserFreqSource};

use fixtures::{CASES, SyntheticDict, holdout_keys};

/// Samples the quick mode runs with.
const QUICK_SAMPLES: usize = 20;

/// Samples the full mode runs with.
const FULL_SAMPLES: usize = 200;

/// Lattice edges the `lm/edge_score` case scores per iteration.
const EDGE_SCORES: usize = 1_000;

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

/// Runs every case of this benchmark target.
///
/// The dictionary and the model are built once, before any measurement, and the
/// held-out set is embedded in the binary, so a run needs no file and no `$HOME`.
///
/// # Errors
/// Returns a description of the first benchmark input that cannot be cut into
/// syllables, and whatever criterion reports about the command line.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dictionary = fixtures::build()?;
    let model = model(&dictionary);

    let mut criterion = Criterion::default().configure_from_args();
    if let Some(samples) = sample_size() {
        criterion = criterion.sample_size(samples);
    }

    bench_decode(&mut criterion, &dictionary, &model);
    bench_decode_reuse(&mut criterion, &dictionary, &model);
    bench_session(&mut criterion, &dictionary, &model);
    bench_holdout(&mut criterion, &dictionary, &model);
    bench_segment(&mut criterion);
    bench_edge_score(&mut criterion, &dictionary, &model);

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

/// Times the two entry points against each other, on the input the latency budget is stated
/// for.
///
/// `viterbi` builds a workspace for every decode, which is what `decode/12syl` measures under
/// its own name; `viterbi_into` keeps one workspace and decodes into it, which is the shape a
/// session runs once per keystroke. Both run the same walk over the same lattice, so the
/// difference between them is what building the workspace costs -- the sweep's beams, the
/// candidate drafts and the result buffers -- and nothing else.
///
/// The allocation count the reuse is judged by cannot be asserted here: a counting allocator
/// needs `#[global_allocator]` and an `unsafe impl GlobalAlloc`, and `unsafe` is confined to
/// the two FFI directories and `ime-dict/src/mmap.rs`. What the pair reports instead is the
/// time the reuse is worth; the buffers themselves are pinned by the unit tests beside
/// `DecodeScratch`, which show that a steady-state decode grows no capacity.
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
                decoder.decode_into(&mut scratch, black_box(&request), dictionary, &user, model);
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
/// the session the setup half left composing, so the reported time is the mean over
/// those keystrokes.
///
/// The seed is typed in the setup half, which keeps the allocations it costs outside
/// the measurement: what the iteration measures is a session that is already
/// composing, which is the state nine keystrokes in ten arrive in.
///
/// The allocation count the reuse is judged by cannot be asserted here, for the same
/// reason as in [`bench_decode_reuse`]: counting allocations needs a
/// `#[global_allocator]` and an `unsafe impl GlobalAlloc`, and `unsafe` is confined to
/// the two FFI directories and `ime-dict/src/mmap.rs`. What this case reports is the
/// time a keystroke costs; the buffers it reuses are pinned by the unit tests beside
/// the session machine, which show that a repeat frame grows no capacity.
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
