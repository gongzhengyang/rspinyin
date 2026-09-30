//! The decode path's allocation budget, asserted against a counting allocator.
//!
//! The reuse work the decoder's buffers were shaped for has a number attached to it:
//! a decode into a workspace that has already decoded once must not touch the
//! allocator at all. Nothing but a `#[global_allocator]` can see that number, and a
//! `#[global_allocator]` can only be installed by a binary crate -- a library's
//! `#[cfg(test)]` tests are compiled into a binary whose root is the library's own
//! `lib.rs`, so the slot is not theirs. This file is a binary target of its own,
//! which is what makes the assertion possible.
//!
//! # What is asserted, and what is only recorded
//!
//! Three properties are asserted, and none of them is a timing:
//!
//! 1. A steady-state decode stays inside the ceiling the budget states. The ceiling is the
//!    re-anchored specimen figure, not an aspiration: the workspace's design claim was zero
//!    allocations, the measured truth -- after the edge-vector pre-sizing was corrected and
//!    the draft-text capacity hint withdrawn -- is a small constant, and the residual (the
//!    edge vector itself plus text capacities that change hands between draft slots) is the
//!    registered follow-up work. A decode that starts costing more than the ceiling, per
//!    syllable or per anything else, fails here and fails the gate.
//! 2. The pass-through answer never costs more than a real decode. The lattice is built
//!    before the dictionary is asked anything, so the degraded path pays the fixed
//!    allocations and skips the candidate buffers; a change that put a second, dearer
//!    allocation profile behind the degraded answer would show here.
//! 3. The first decode into a fresh workspace allocates, and more than the steady state.
//!    That is the boundary half of the claim: the buffers have to come from somewhere, and
//!    a counter that read zero there would be a counter that was never armed rather than a
//!    path that allocated nothing.
//!
//! The measured counts are also written to `target/alloc-report.txt`, in the
//! line-oriented form `xtask budget` reads, so that the thresholds in
//! `docs/dev/budgets.json` -- not a literal in this file -- decide what the counts mean.
//! Writing the report happens before the assertions: a run whose assertion fails has
//! to leave the numbers behind for the gate to judge, not only a red line in a log.
//!
//! # How to run it
//!
//! The counters are process-global and monotonic, so a reading is only the work of the
//! code between two reads if nothing else in the process allocates in that window.
//! `cargo nextest` gives every test a process of its own, which is exactly this; run
//! through `cargo test`, the tests of one binary share a process and pollute each
//! other's readings. This file is written for the nextest run the project already
//! gates on, and its assertions are not meaningful under a shared-process runner.
//!
//! # Determinism
//!
//! Nothing here reads a dictionary file, the clock, the environment or a display. The
//! dictionaries are in-memory implementations of the frozen contract whose lookups
//! allocate nothing -- a fixture that materialized a `Vec` per lookup would be counted
//! as if the decoder had allocated -- and the model is the in-memory one, empty, whose
//! answer for an unknown word is the contract's own floor.

use std::hint::black_box;
use std::path::{Path, PathBuf};

use alloc_count::Counting;
use ime_core::lm::InMemoryLm;
use ime_core::viterbi::{DecodeScratch, Decoder};
use ime_types::{
    DecodeRequest, ImeError, LanguageModel, Lexicon, SyllableId, UserFreqSource, WordFlags,
    WordIter, WordRef,
};

/// The process allocator, counting every allocation the tests make.
///
/// The counters it keeps are read before and after the work under test; the difference
/// is what the decode cost. Nothing in this file needs `unsafe`: the `GlobalAlloc`
/// implementation lives in the counter's own crate, and installing an allocator is a
/// safe attribute.
#[global_allocator]
static ALLOCATOR: Counting = Counting::new();

/// The short input: two syllables, the smallest composition that still produces more
/// than one candidate.
const SHORT_INPUT: &str = "nihao";

/// The long input: twelve syllables, the shape the decode budget is stated for and the
/// widest working set a legal input produces.
const LONG_INPUT: &str = "nihaozhongguorenminfuwuyuanderenduo";

/// Word texts a lookup is answered from, in a table rather than behind an allocation.
///
/// The texts are picked by the key's length, so that spans of different lengths spell
/// different words and the candidate list the sweep builds holds more than one entry.
/// A single fixture table is enough for that, and a table is what keeps the lookup
/// itself allocation-free.
const WORDS: [&str; 8] = ["春", "风", "又", "绿", "江", "南", "岸", "月"];

/// The first line of the report the budget gate reads.
const REPORT_HEADER: &str = "rspinyin-alloc-report 1";

/// The words one lookup is answered with, and why a key of this length spells it.
///
/// The index is the key's length rather than its content because the content is what
/// the segmentation layer owns: two keys of the same length spelling the same word is
/// invisible to the buffers under test, while a lookup that had to build a string to
/// answer with would be counted against the decoder.
fn word_for(key: &str) -> &'static str {
    WORDS[key.len() % WORDS.len()]
}

/// Syllables a key spells, which is the number of its `'`-separated parts.
///
/// The lattice counts a span's syllables along the graph rather than from the entry's
/// own claim, so this is bookkeeping for the fixture rather than a value the decoder
/// reads; it is filled in correctly anyway, so that a later reader of the entries is
/// not misled by a placeholder.
fn syllables_of(key: &str) -> u8 {
    let parts = key.split('\'').count();
    u8::try_from(parts).unwrap_or(u8::MAX)
}

/// Builds the word reference one fixture lookup answers with.
fn word_of(key: &str) -> WordRef<'static> {
    WordRef {
        text: word_for(key),
        weight: 1,
        syl_count: syllables_of(key),
        flags: WordFlags::empty(),
    }
}

/// A dictionary that answers every key with one word, and never allocates.
///
/// The decode path cannot tell it from a mapped dictionary: it answers by key, the
/// words it hands back borrow the fixture rather than a mapping, and every span of
/// every reading of an input finds a word, which is what keeps the measurement on the
/// decode's own buffers instead of on the fallback path.
///
/// # Concurrency
///
/// `Send` and `Sync`: it holds no state at all, every method is a pure function of its
/// argument, and nothing is reentrant about it. It never blocks.
struct Everywhere;

impl Lexicon for Everywhere {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = [word_of(key)];
        // The iterator copies the entry into its own inline storage, so the local array
        // does not outlive the call and the borrow the return type carries is the
        // fixture's, not the array's.
        Ok(WordIter::from_slice(&words))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        // The contract leaves prefix enumeration to a later phase; the abbreviation
        // walk is the only reader and the requests below carry no abbreviation flag.
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        // Unreachable while `lookup` answers every key: no span is left uncovered, so
        // the fallback is never consulted. It is answered in the same allocation-free
        // shape regardless, so that a change to the lattice cannot turn it into the
        // one allocating call on the path.
        let words = [WordRef {
            text: WORDS[0],
            weight: 1,
            syl_count: 1,
            flags: WordFlags::empty(),
        }];
        Ok(WordIter::from_slice(&words))
    }
}

/// The words a lookup with no entries returns.
const NO_WORDS: &[WordRef<'static>] = &[];

/// A dictionary with no words at all.
///
/// Every span of every reading comes back empty and the fallback comes back empty, so
/// the lattice has no edge and the decode ends in the pass-through answer. It is the
/// boundary fixture: the question is what *that* path allocates, and whether it is the
/// same as what a real decode allocates.
///
/// # Concurrency
///
/// `Send` and `Sync`: stateless, pure, never blocking, for the same reason as
/// [`Everywhere`].
struct Nothing;

impl Lexicon for Nothing {
    fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(NO_WORDS))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(NO_WORDS))
    }
}

/// A user-frequency source that has recorded nothing.
///
/// The decode path reads it once per lattice edge while the user dictionary switch is
/// on, which is the shipped default, so the fixture has to answer rather than be
/// absent.
///
/// # Concurrency
///
/// `Send` and `Sync`: stateless, pure, never blocking.
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

/// One steady-state decode, counted.
///
/// The workspace is warmed with one decode of `request` first, so that what the second
/// decode allocates is exactly what the workspace failed to reuse -- the number the
/// budget is stated for. The requested bytes are answered beside the count, because a
/// count of two says nothing about whether the allocations are a lattice's edge vector
/// or a candidate text that grew.
///
/// The answers are `(allocations, requested bytes, whether the result is degraded)`. The
/// third is the caller's check that the fixture took the path under test: a pass-through
/// answer exercises a different, shorter path, and a count measured on it is not the
/// count the real decode's budget is stated for.
fn steady_state(
    decoder: &Decoder,
    request: &DecodeRequest,
    lexicon: &dyn Lexicon,
    lm: &dyn LanguageModel,
) -> (usize, usize, bool) {
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(&mut scratch, request, lexicon, &NoUser, lm);
    let degraded = scratch.result().degraded;
    let before_allocations = alloc_count::allocations();
    let before_bytes = alloc_count::bytes();
    decoder.decode_into(&mut scratch, black_box(request), lexicon, &NoUser, lm);
    black_box(scratch.result().candidates.len());
    (
        alloc_count::allocations() - before_allocations,
        alloc_count::bytes() - before_bytes,
        degraded,
    )
}

/// Where the report is written: `target/alloc-report.txt`.
///
/// Resolved from this file's own crate rather than from the environment, so a run needs
/// no agreement about a target directory to find its own output. The gate that reads
/// the file resolves the same path from the repository root, which is the same
/// directory as long as the workspace is built in place.
///
/// # Panics
///
/// When this crate is not two levels below a repository root, which is a layout the
/// tests cannot run in.
fn report_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ime-core sits two levels below the repository root")
        .join("target")
        .join("alloc-report.txt")
}

/// Writes the measurements of one run, in the line-oriented form the gate reads.
///
/// The records are written in a fixed order and nothing but whole numbers is written,
/// which is the whole format: a report a human can read, a gate can parse strictly,
/// and no library had to be added to produce.
///
/// # Panics
///
/// When the report cannot be written. A measurement that could not be recorded is a
/// failed run rather than a quiet one: the gate treats an absent report as a budget
/// nobody measured, which is a failure, and a test that left it absent while claiming
/// to have measured would be the same failure with the cause hidden.
fn write_report(records: &[(&str, u64)]) {
    let mut text = String::from(REPORT_HEADER);
    text.push('\n');
    for (name, value) in records {
        text.push_str(name);
        text.push('=');
        text.push_str(&value.to_string());
        text.push('\n');
    }
    let path = report_path();
    std::fs::create_dir_all(path.parent().expect("the report path has a parent"))
        .expect("the target directory is writable");
    std::fs::write(&path, text).expect("the report is writable");
}

/// The steady-state ceiling the budget states, in allocations per decode.
///
/// This is the specimen figure the budget was re-anchored on: the widest input the
/// assertions decode (twelve syllables, the widest legal working set) costs this many
/// allocations once its workspace is warm, and `docs/dev/budgets.json` states the same
/// number as `alloc_count.decode_steady`, which is what `xtask budget --alloc` judges the
/// written report against. The two were re-anchored together on the measured values after
/// the edge-vector pre-sizing was corrected and the draft-text capacity hint withdrawn;
/// what still allocates is the edge vector itself plus the small text buffers whose
/// capacities change hands as the dedupe and the sort rearrange the draft slots. Retiring
/// those is the workspace's registered follow-up, and a decode that starts costing more
/// than this ceiling -- per syllable, per candidate, per anything -- fails the gate.
const BUDGETED_STEADY: usize = 13;

/// The first decode into a fresh workspace allocates, and more than the steady state.
///
/// This is the boundary half of the budget: the buffers a steady-state decode reuses
/// have to be filled once, and a counter that reported zero for the first decode would
/// be reporting a window it never measured rather than a path that allocated nothing.
#[test]
fn test_decode_into_first_decode_costs_more_than_the_steady_state() {
    let decoder = Decoder::default();
    let lm = InMemoryLm::new();
    let request = DecodeRequest::new(LONG_INPUT);
    let mut scratch = DecodeScratch::new();

    let before = alloc_count::allocations();
    decoder.decode_into(&mut scratch, &request, &Everywhere, &NoUser, &lm);
    let cold = alloc_count::allocations() - before;
    assert!(
        !scratch.result().degraded,
        "the fixture decodes the long input, so the measured path is the decode"
    );
    assert!(
        cold > 0,
        "the first decode fills empty buffers, so it has to allocate"
    );

    let (steady, _, degraded) = steady_state(&decoder, &request, &Everywhere, &lm);
    assert!(!degraded, "the steady state measures the same real decode");
    assert!(
        steady < cold,
        "the workspace is what removes the allocations: steady state {steady} against a \
         first decode of {cold}"
    );
}

/// A steady-state decode stays inside the budgeted ceiling, whatever the input's length.
///
/// The ceiling is what catches the enemy this test exists for: a decode that grew a buffer
/// per syllable would keep every functional test green and every timing threshold satisfied
/// on a fast machine, and it is the ceiling -- not a per-input equality -- that such a
/// growth passes. The two shapes are both held to the one ceiling the budget states rather
/// than to each other, because the residual the workspace still pays is a handful of text
/// buffers whose capacity depends on how wide the candidate list is; equalising the two
/// shapes is the registered follow-up work, and until it lands the honest assertion is the
/// ceiling both of them sit far inside.
#[test]
fn test_decode_into_steady_state_stays_inside_the_budgeted_ceiling() {
    let decoder = Decoder::default();
    let lm = InMemoryLm::new();
    let short = DecodeRequest::new(SHORT_INPUT);
    let long = DecodeRequest::new(LONG_INPUT);

    let (short_allocations, _, short_degraded) = steady_state(&decoder, &short, &Everywhere, &lm);
    let (long_allocations, _, long_degraded) = steady_state(&decoder, &long, &Everywhere, &lm);
    assert!(
        !short_degraded && !long_degraded,
        "both inputs really decode"
    );
    assert!(
        short_allocations <= BUDGETED_STEADY,
        "a two-syllable decode costs {short_allocations} allocations against a ceiling of \
         {BUDGETED_STEADY}: the workspace stopped reusing a buffer it used to"
    );
    assert!(
        long_allocations <= BUDGETED_STEADY,
        "a twelve-syllable decode costs {long_allocations} allocations against a ceiling of \
         {BUDGETED_STEADY}: the widest input grew past the budget"
    );
}

/// A decode that ends in the pass-through answer never costs more than a real one.
///
/// The lattice is built before the dictionary is asked anything, so an input with no words
/// pays the fixed allocations a real decode pays and skips the ones its candidates cost.
/// The result is marked degraded, which is the contract's own word for the answer (the
/// decoder's tests pin the same flag), and pinning it here is what tells a future change
/// that silently stopped marking the fallback apart from a change that merely moved a
/// buffer.
#[test]
fn test_decode_into_pass_through_never_costs_more_than_a_real_decode() {
    let decoder = Decoder::default();
    let lm = InMemoryLm::new();
    let input = DecodeRequest::new(SHORT_INPUT);

    let (real, _, real_degraded) = steady_state(&decoder, &input, &Everywhere, &lm);
    assert!(!real_degraded, "the readable input really decodes");
    let (pass_through, _, degraded) = steady_state(&decoder, &input, &Nothing, &lm);
    assert!(
        degraded,
        "a dictionary with no words drives the decode into the pass-through answer, and \
         the result says so with `degraded`"
    );
    assert!(
        pass_through <= real,
        "the degraded path skips the candidate buffers a real decode fills, so it cannot \
         cost more: pass-through {pass_through} against a real decode of {real}"
    );
}

/// A steady-state decode sits at the ceiling the budget states, and the run is recorded.
///
/// This is the run that leaves `target/alloc-report.txt` behind, and it pins the measured
/// steady state to the number the budget document and the gate both judge: the counts are
/// written before anything is asserted, so that a failing assertion still leaves the
/// numbers the budget gate judges. Pinning the exact figure here (and not only the ceiling)
/// is deliberate -- a decode that starts costing one allocation fewer has earned a
/// re-anchoring of the budget, and one that costs more has broken it, and both ought to
/// stop this test rather than drift under it.
#[test]
fn test_decode_into_steady_state_costs_the_budgeted_allocation() {
    let decoder = Decoder::default();
    let lm = InMemoryLm::new();
    let short = DecodeRequest::new(SHORT_INPUT);
    let long = DecodeRequest::new(LONG_INPUT);
    let mut scratch = DecodeScratch::new();

    // The first decode is the cold-start figure the report records: what filling the
    // workspace once costs, which is the number the reuse is worth.
    let before = alloc_count::allocations();
    decoder.decode_into(&mut scratch, &long, &Everywhere, &NoUser, &lm);
    let cold = alloc_count::allocations() - before;
    assert!(
        !scratch.result().degraded,
        "the fixture decodes the long input, so the recorded cold start is a decode"
    );

    let (steady, bytes, degraded) = steady_state(&decoder, &long, &Everywhere, &lm);
    assert!(!degraded, "the steady state measures a real decode");
    let (steady_short, _, short_degraded) = steady_state(&decoder, &short, &Everywhere, &lm);
    assert!(!short_degraded, "the short input really decodes");

    write_report(&[
        ("decode_cold", cold as u64),
        ("decode_steady", steady as u64),
        ("decode_steady_short", steady_short as u64),
        ("bytes_decode_steady", bytes as u64),
    ]);

    assert_eq!(
        steady, BUDGETED_STEADY,
        "a steady-state decode of the widest input costs what the budget was re-anchored \
         on: fewer means the budget can come down, more means the workspace stopped \
         reusing a buffer, and either way the document and this test move together"
    );
    assert!(
        steady_short < steady,
        "the short input builds a smaller graph and holds fewer candidates, so it never \
         costs the widest input's steady state: {steady_short} against {steady}"
    );
    assert!(
        cold > steady,
        "the first decode fills the workspace, so it costs more than what reusing it \
         costs: cold {cold} against steady {steady}"
    );
}
