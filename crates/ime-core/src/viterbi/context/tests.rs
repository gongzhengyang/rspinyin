//! Unit tests for the context re-rank: the band it swaps inside, the stable order it
//! keeps, and the bounds it reads.
//!
//! They drive the one public function against literal candidate lists and a model
//! whose bigrams are a table, so every expectation is a fact about the re-rank and
//! not about a model's fallback arithmetic.

use super::*;

use ime_types::CandidateSource;

/// A model that answers only what its table holds, and a miss floor everywhere else.
///
/// The floor stands in for what both shipped models answer on a miss -- the word's
/// own unigram minus the miss penalty -- which is a score below every real
/// continuation, so a candidate the context says nothing about never reads as a
/// better continuation than one it names. The tests write the floor as one constant,
/// which is what makes "no opinion" and "equal opinion" the same thing here: a test
/// that pins the stable order can rely on two misses scoring alike.
struct TableLm {
    bigrams: Vec<(&'static str, &'static str, i32)>,
}

/// The score a miss answers with, below every table score the tests use.
const MISS_FLOOR: i32 = -4096;

impl TableLm {
    /// A model that continues `continuation` from `context` with `score`.
    fn one(context: &'static str, continuation: &'static str, score: i32) -> Self {
        Self {
            bigrams: vec![(context, continuation, score)],
        }
    }
}

impl LanguageModel for TableLm {
    fn unigram(&self, _word: &str) -> i32 {
        0
    }

    fn bigram(&self, prev: &str, word: &str) -> i32 {
        self.bigrams
            .iter()
            .find(|(seen_prev, seen_word, _)| *seen_prev == prev && *seen_word == word)
            .map(|(_, _, score)| *score)
            .unwrap_or(MISS_FLOOR)
    }
}

/// One candidate with the text and the base score the test names.
fn candidate(text: &str, score: f32) -> Candidate {
    Candidate {
        index: 1,
        text: String::from(text),
        annotation: None,
        source: CandidateSource::Dict,
        score,
        consumed_syllables: 2,
    }
}

/// The texts of a list, in the order the re-rank left them.
fn texts(candidates: &[Candidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect()
}

#[test]
fn test_rerank_with_context_moves_a_continuation_up_inside_the_band() {
    // 预算 outranks 银行 on the base scores, but the previous commit was 银行, and
    // the model continues 银行 after it: inside the band the context is the
    // tie-breaker, and the continuation moves to the front.
    let lm = TableLm::one("银行", "银行", -64);
    let mut candidates = vec![candidate("预算", 10.0), candidate("银行", 10.001)];
    rerank_with_context(&mut candidates, Some("银行"), &lm);
    assert_eq!(texts(&candidates), ["银行", "预算"]);
}

#[test]
fn test_rerank_with_context_leaves_a_real_score_gap_alone() {
    // The same model, the same context, but the top score sits a whole band above
    // the next one: the run of near-ties is one candidate long, and a band that
    // cannot hold the continuation holds no swap either. Nothing moves, which is
    // what keeps the decode's own ranking the authority outside the band.
    let lm = TableLm::one("银行", "银行", -64);
    let mut candidates = vec![
        candidate("预算", 4096.0),
        candidate("银行", 10.0),
        candidate("医院", 9.999),
    ];
    rerank_with_context(&mut candidates, Some("银行"), &lm);
    assert_eq!(texts(&candidates), ["预算", "银行", "医院"]);
}

#[test]
fn test_rerank_with_context_without_context_is_the_identity() {
    let lm = TableLm::one("银行", "银行", -64);
    let mut candidates = vec![candidate("预算", 10.0), candidate("银行", 10.001)];
    let before = texts(&candidates);
    rerank_with_context(&mut candidates, None, &lm);
    assert_eq!(texts(&candidates), before, "no context moves nothing");
    // An empty context word is the same as none: a session that has committed
    // nothing has nothing to continue from.
    rerank_with_context(&mut candidates, Some(""), &lm);
    assert_eq!(texts(&candidates), before, "an empty context moves nothing");
}

#[test]
fn test_rerank_with_context_keeps_the_ranking_order_on_equal_bonuses() {
    // Neither candidate continues from the context, and the double scores them the
    // same: the stable sort leaves the base order standing, which is what makes the
    // re-rank reproducible -- the same list and the same model, the same order.
    let lm = TableLm::one("银行", "别的", -64);
    let mut candidates = vec![candidate("预算", 10.0), candidate("银行", 10.001)];
    rerank_with_context(&mut candidates, Some("银行"), &lm);
    assert_eq!(texts(&candidates), ["预算", "银行"]);
}

#[test]
fn test_rerank_with_context_never_touches_the_list_below_the_band() {
    // Four candidates: two in the band, two below it. The context swaps the band
    // pair and leaves the tail exactly where the ranking put it, whatever the model
    // thinks of it.
    let lm = TableLm::one("银行", "银行", -64);
    let mut candidates = vec![
        candidate("预算", 10.0),
        candidate("银行", 9.999),
        candidate("医院", 5.0),
        candidate("医院", 4.999),
    ];
    rerank_with_context(&mut candidates, Some("银行"), &lm);
    assert_eq!(texts(&candidates), ["银行", "预算", "医院", "医院"]);
}

#[test]
fn test_rerank_with_context_reads_at_most_the_bound_it_declares() {
    // A page past the re-rank's bound: the context would continue the last
    // candidate best of all, but the bound is what librime's own pass applies, and
    // a swap the user reaches only by paging is a swap nobody asked for.
    let lm = TableLm::one("银行", "银行", -64);
    // Every candidate inside the bound carries the same score, so the whole head
    // is one band and the stable sort has real work it could do there -- which is
    // exactly what the bound stops at the edge.
    let mut candidates: Vec<Candidate> = (0..CONTEXT_RERANK_CANDIDATES)
        .map(|_index| candidate("其他", 10.0))
        .collect();
    let tail = candidate("银行", 1.0);
    candidates.push(tail);
    rerank_with_context(&mut candidates, Some("银行"), &lm);
    assert_eq!(
        candidates.last().map(|candidate| candidate.text.as_str()),
        Some("银行"),
        "the candidate past the bound stays past it"
    );
    assert_eq!(
        candidates.first().map(|candidate| candidate.text.as_str()),
        Some("其他"),
        "every candidate inside the bound misses, so the base order stands"
    );
}

#[test]
fn test_rerank_with_context_is_identical_across_one_hundred_runs() {
    // The candidate order is part of the contract, so the re-rank is pinned the way
    // the decode under it is: the same list, the same context and the same model a
    // hundred times over, one order.
    let lm = TableLm::one("银行", "银行", -64);
    let reference = {
        let mut candidates = vec![
            candidate("预算", 10.0),
            candidate("医院", 9.999),
            candidate("银行", 9.998),
        ];
        rerank_with_context(&mut candidates, Some("银行"), &lm);
        texts(&candidates)
    };
    for _ in 0..100 {
        let mut candidates = vec![
            candidate("预算", 10.0),
            candidate("医院", 9.999),
            candidate("银行", 9.998),
        ];
        rerank_with_context(&mut candidates, Some("银行"), &lm);
        assert_eq!(texts(&candidates), reference);
    }
}

#[test]
fn test_rerank_aligned_carries_the_drafts_through_the_same_moves() {
    // The drafts are the pair source the commit reads at the highlight's position, so a
    // candidate the re-rank moves must take its own draft with it. The band here holds
    // the first two candidates, whose base scores tie; the model continues the second
    // one, and the swap has to move the word boundaries with the text.
    let model = TableLm::one("银行", "预算", 100);
    let mut candidates = vec![candidate("银行", 10.0), candidate("预算", 10.0)];
    let mut drafts = vec![
        draft_of("银行"),
        draft_of("预算"),
        draft_of("the third stays"),
    ];

    crate::viterbi::context::rerank_aligned(&mut candidates, &mut drafts, Some("银行"), &model);

    assert_eq!(
        texts(&candidates),
        vec!["预算", "银行"],
        "context wins the tie"
    );
    assert_eq!(
        drafts[0].text(),
        "预算",
        "the draft moved with its candidate"
    );
    assert_eq!(drafts[1].text(), "银行", "and so did the other one");
    assert_eq!(
        drafts[2].text(),
        "the third stays",
        "below the band nothing moved"
    );
}

/// One draft with the text the test names and no spans: the re-rank permutes whole
/// drafts, so the text is the identity the assertions read.
fn draft_of(text: &str) -> crate::viterbi::sweep::Draft {
    crate::viterbi::sweep::Draft {
        text: String::from(text),
        source: CandidateSource::Dict,
        syllables: 2,
        score: 0,
        corrected: false,
        readings: String::new(),
        words: crate::viterbi::sweep::WordSpans::EMPTY,
    }
}
