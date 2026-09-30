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

mod fixtures;

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput};
use ime_core::lm::{InMemoryLm, LOG2_FLOOR, Scorer};
use ime_core::segment::{Readings, SyllableDag};
use ime_core::state::{Effect, Session, SessionConfig, SessionEnv};
use ime_core::viterbi::lattice::LatticeOptions;
use ime_core::viterbi::{DecodeScratch, Decoder, build_lattice};
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
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dictionary = fixtures::build()?;
    let model = model(&dictionary);

    let mut criterion = Criterion::default().configure_from_args();
    if let Some(samples) = sample_size() {
        criterion = criterion.sample_size(samples);
    }

    bench_decode(&mut criterion, &dictionary, &model);
    bench_decode_reuse(&mut criterion, &dictionary, &model);
    bench_allocations(&mut criterion, &dictionary, &model);
    bench_session(&mut criterion, &dictionary, &model);
    bench_holdout(&mut criterion, &dictionary, &model);
    bench_segment(&mut criterion);
    bench_lattice(&mut criterion, &dictionary);
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

/// Times the two hot paths the way the counting allocator sees them.
///
/// Criterion answers "how long"; the `alloc` group exists for "how many times", which
/// is the number the decode path's buffer reuse is stated in and the one a timing
/// threshold on a fast machine cannot catch changing. Criterion has no counter, so each
/// case reads the process allocator around its routine and consumes the delta: what it
/// reports as the time is therefore the cost of the work with the observation inside
/// it, which is the honest price of the measurement.
///
/// The assertion the group makes is not a threshold. A threshold lives in
/// `docs/dev/budgets.json`, and the runs that assert it are the integration test that
/// installs the same allocator and `xtask budget`, which reads the counts that test
/// writes. What a criterion run can assert on its own is that the count is the same on
/// every iteration: a path whose allocation cost depends on how many times it has run
/// is a workspace that is not reusing what it holds, and it would otherwise hide behind
/// a mean that looks normal.
fn bench_allocations(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    bench_alloc_decode(criterion, dictionary, model);
    bench_alloc_keystroke(criterion, dictionary, model);
}

/// Times and counts one steady-state decode, the path `decode/viterbi_into` measures.
///
/// The workspace is warm before the first sample, which is the state the assertion is
/// about: a first iteration that filled the buffers would make the count a property of
/// the sample order rather than of the path.
///
/// # Panics
///
/// When two iterations allocate different amounts, which is a workspace that stopped
/// reusing what it holds.
fn bench_alloc_decode(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
    let decoder = Decoder::default();
    let user = NoUser;
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let request = DecodeRequest::new(case.raw);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(&mut scratch, &request, dictionary, &user, model);
    let mut group = criterion.benchmark_group("alloc");
    group.bench_function("decode", |bencher| {
        let mut seen: Option<usize> = None;
        bencher.iter(|| {
            let before = (alloc_count::allocations(), alloc_count::bytes());
            decoder.decode_into(&mut scratch, black_box(&request), dictionary, &user, model);
            black_box(scratch.result().candidates.len());
            let counted = (
                alloc_count::allocations() - before.0,
                alloc_count::bytes() - before.1,
            );
            assert_same_every_iteration(&mut seen, counted.0);
            black_box(counted)
        });
    });
    group.finish();
}

/// Times and counts the keystrokes of a session that is already composing, the path
/// `session/keystroke` measures.
///
/// The session is built and seeded in the setup half, so its allocations stay outside
/// both the timing and the counted window, and one iteration is the whole run of
/// [`SESSION_KEYS`] -- the same shape the timing case reports, so the two numbers can be
/// read side by side.
///
/// # Panics
///
/// When two iterations allocate different amounts, or when a keystroke does not reach
/// the frame path, because the case would then be measuring a diagnostic.
fn bench_alloc_keystroke(
    criterion: &mut Criterion,
    dictionary: &SyntheticDict,
    model: &InMemoryLm,
) {
    let cfg = SessionConfig::default();
    let decoder = Decoder::default();
    let user = NoUser;
    let env = SessionEnv {
        decoder: &decoder,
        lexicon: dictionary,
        user_freq: &user,
        lm: model,
    };
    let mut group = criterion.benchmark_group("alloc");
    group.bench_function("keystroke", |bencher| {
        let mut seen: Option<usize> = None;
        bencher.iter_batched(
            || {
                let mut session = Session::new();
                for ch in SESSION_SEED.chars() {
                    let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
                }
                session
            },
            |mut session| {
                let before = (alloc_count::allocations(), alloc_count::bytes());
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
                let counted = (
                    alloc_count::allocations() - before.0,
                    alloc_count::bytes() - before.1,
                );
                assert_same_every_iteration(&mut seen, counted.0);
                black_box(counted)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Asserts that `counted` is what every iteration before this one allocated.
///
/// The first iteration only sets the bar: the case has nothing to compare it with, and
/// a comparison against a number invented for the purpose would be the assertion
/// asserting itself.
fn assert_same_every_iteration(seen: &mut Option<usize>, counted: usize) {
    match *seen {
        None => *seen = Some(counted),
        Some(first) => assert_eq!(
            counted, first,
            "every iteration of a warm path allocates the same amount: {counted} against \
             the {first} the first iteration produced"
        ),
    }
}

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
fn bench_lattice(criterion: &mut Criterion, dictionary: &SyntheticDict) {
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let mut dag = SyllableDag::new();
    assert!(dag.build(case.raw).is_ok(), "the case input is segmentable");
    let user = NoUser;
    let mut readings = Readings::new();
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
                    },
                );
                black_box(lattice.len())
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
fn bench_unigram(criterion: &mut Criterion, dictionary: &SyntheticDict, model: &InMemoryLm) {
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
