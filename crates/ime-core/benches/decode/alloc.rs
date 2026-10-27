//! The allocation benches: the two hot paths as the counting allocator sees them.
//!
//! Split out of `decode.rs` to keep that file inside the line limit. The group exists for
//! "how many allocations", the number the decode path's buffer reuse is stated in; the
//! timing benches in the parent module answer "how long" and cannot catch a count
//! changing on a fast machine.

use super::*;

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
pub(super) fn bench_allocations(
    criterion: &mut Criterion,
    dictionary: &SyntheticDict,
    model: &InMemoryLm,
) {
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
pub(super) fn bench_alloc_decode(
    criterion: &mut Criterion,
    dictionary: &SyntheticDict,
    model: &InMemoryLm,
) {
    let decoder = Decoder::default();
    let user = NoUser;
    let Some(case) = CASES.iter().find(|case| case.name == REUSE_CASE) else {
        return;
    };
    let request = DecodeRequest::new(case.raw);
    let mut scratch = DecodeScratch::new();
    let sources = DecodeSources {
        lexicon: dictionary,
        user_freq: &user,
        lm: model,
        english: None,
        hints: None,
    };
    decoder.decode_into(&mut scratch, &request, sources);
    let mut group = criterion.benchmark_group("alloc");
    group.bench_function("decode", |bencher| {
        let mut seen: Option<usize> = None;
        bencher.iter(|| {
            let before = (alloc_count::allocations(), alloc_count::bytes());
            let sources = DecodeSources {
                lexicon: dictionary,
                user_freq: &user,
                lm: model,
                english: None,
                hints: None,
            };
            decoder.decode_into(&mut scratch, black_box(&request), sources);
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
pub(super) fn bench_alloc_keystroke(
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
        english: None,
        confusables: None,
        symbols: None,
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
