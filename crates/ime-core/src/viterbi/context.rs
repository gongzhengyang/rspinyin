//! The context re-rank: the word the last commit ended with, reordering a near-tie
//! page.
//!
//! Responsibility: apply the one piece of context that survives a commit -- the word
//! the user's previous composition ended with -- to the candidate list a decode just
//! assembled. Inside a band of scores the ranking could not tell apart, a candidate
//! the model's bigram continues well moves ahead of one it does not; outside the
//! band, nothing moves. The session owns the context word and calls this after a
//! decode, before the frame it derives from the list.
//!
//! Boundaries: this is a list transform and nothing else. It reads no input buffer,
//! keeps no state, and never touches a file, a clock or the environment; the same
//! list, the same context and the same model answer the same order on every run,
//! which is what makes the re-rank as reproducible as the ranking under it. The
//! candidate type is the frozen contract's, so the re-rank changes no wire shape:
//! it is a permutation of the list the session already had.
//!
//! # Why a band at all
//!
//! Context is weak evidence and the ranking is strong evidence. A candidate the
//! user's last sentence makes likely is not worth displacing a candidate whose score
//! is genuinely better, and a swap the user sees as arbitrary is worse than no swap
//! at all. The band is what separates the two: within it the scores are close enough
//! that the engine itself had to break the tie by generation order, and a preference
//! the user's own last word expresses is exactly the tie-breaker a predictor should
//! use. Outside it the base order stands, and the whole-sentence-first contract of
//! the decode keeps holding for everything the band does not reach.
//!
//! # What a candidate is looked up as
//!
//! The bigram is asked for the candidate's own text. For a one-word candidate -- the
//! shape of everything the band can realistically flip -- the text is the word, and
//! the lookup is the model's own. For a whole-sentence candidate the text is several
//! words joined, which no bigram entry is keyed by, so the lookup misses and the
//! sentence stays where the ranking put it: the strongest reading of the input is
//! never moved by context at all.

use ime_types::{Candidate, LanguageModel};

use super::sweep::Draft;
use crate::lm::Q;

/// The near-tie band a contextual swap is allowed inside, in Q16.16 path-score
/// units: one unit of the Q8.8 grid the ranking itself is scored on.
///
/// Two candidates within this band differ by less than the smallest step the
/// scoring grid resolves -- less than one rank doubling of the model's own falloff
/// -- which is the operational meaning of "the ranking could not tell them apart".
/// The value rides on the frozen candidate's display score, which is the integer
/// path score divided by the Q16.16 one, so the band reaches the comparison as
/// `CONTEXT_TIE_BAND_Q16` divided by `Q * Q`. The task card's sketch of this
/// constant wrote a value (`1 << 12`) that its own comment contradicts; the
/// comment, one Q8.8 unit, is the intent, and one unit is the reading of it that
/// keeps real score gaps out of the band -- the wider sketch would have let
/// context override almost any pair on a page.
pub const CONTEXT_TIE_BAND_Q16: i64 = 1 << 8;

/// How many candidates the re-rank reads.
///
/// The bound librime's contextual search applies to the same pass: the head of the
/// list is what a user chooses from, and a swap past it would reorder a candidate
/// the user reaches only by paging, where the surprise of the move outweighs the
/// preference it expresses.
pub const CONTEXT_RERANK_CANDIDATES: usize = 32;

/// Reorders the head of `candidates` by how well the model continues from `context`.
///
/// The list must be ordered best first, as every decode leaves it. The re-rank finds
/// the run at the top whose scores sit within [`CONTEXT_TIE_BAND_Q16`] of the best
/// score, reads at most the first [`CONTEXT_RERANK_CANDIDATES`] candidates, and
/// stable-sorts that run by the bigram bonus the model answers for `context` and the
/// candidate's own text, best continuation first. A candidate the model says nothing
/// about keeps its place among its equals, and everything below the band is left
/// untouched, so a real score gap is never overridden.
///
/// `context` is the word the user's last commit ended with; `None`, or an empty
/// string, leaves the list exactly as it was -- the session's first composition has
/// no context, and the identity is what it gets.
///
/// # Panics
///
/// Never.
pub fn rerank_with_context(
    candidates: &mut [Candidate],
    context: Option<&str>,
    lm: &dyn LanguageModel,
) {
    rerank_aligned(candidates, &mut [], context, lm);
}

/// The re-rank of [`rerank_with_context`], over the drafts the candidates came from.
///
/// `drafts` is the per-candidate workspace the decode filled the list from, in the same
/// order, and it is permuted by exactly the same moves: the pairs a commit coins from are
/// read out of the draft at the highlight's position, so a candidate the re-rank moved
/// must not take another draft's word boundaries with it. The function is crate-visible
/// because the drafts are the workspace's own type; a caller outside it uses the public
/// wrapper, which passes no drafts and permutes the list alone.
///
/// # Panics
///
/// Never.
pub(crate) fn rerank_aligned(
    candidates: &mut [Candidate],
    drafts: &mut [Draft],
    context: Option<&str>,
    lm: &dyn LanguageModel,
) {
    let Some(context) = context else {
        return;
    };
    if context.is_empty() {
        return;
    }
    let end = candidates.len().min(CONTEXT_RERANK_CANDIDATES);
    // The scores arrive as the display floats the frozen contract carries -- the
    // integer path score over the Q16.16 one -- so the band crosses to that scale
    // here, once, where the whole comparison lives. The subtraction and the compare
    // are exact reproductions of the integer order at the magnitudes path scores
    // reach, and the run below is what decides the band, not the arithmetic around
    // it.
    let Some(best) = candidates.first().map(|candidate| candidate.score) else {
        return;
    };
    let band = CONTEXT_TIE_BAND_Q16 as f32 / ((Q * Q) as f32);
    let floor = best - band;
    // The list is ordered best first, so the band is a prefix: the candidates while
    // their score has not dropped below the floor.
    let band_end = candidates[..end].partition_point(|candidate| candidate.score >= floor);
    if band_end < 2 {
        // One candidate in the band is nothing to reorder, and a tie the band cannot
        // hold two of is not a tie the context is allowed to break.
        return;
    }
    // The sort is stable, so candidates the model rates the same keep the order the
    // ranking gave them, which is the order the generation counters broke the score
    // ties by. The permutation is computed once over indices and then applied to both
    // slices through the same walks: sorting the slices under one comparator would
    // compare them against each other, and cloning to permute would put an allocation
    // on the key path the re-rank has never cost. The stack arrays are sized by the
    // candidate bound the band already enforces.
    let mut order = [0usize; CONTEXT_RERANK_CANDIDATES];
    for (at, slot) in order.iter_mut().enumerate().take(band_end) {
        *slot = at;
    }
    order[..band_end].sort_by_key(|&at| std::cmp::Reverse(lm.bigram(context, candidates[at].text.as_str())));
    let drafts_align = drafts.len() >= band_end;
    for at in 0..band_end {
        // Where the element now at `at` belongs: the chain is followed until it lands
        // at or after `at`, which is the shape of an in-place permutation that no
        // element twice.
        let mut to = order[at];
        while to < at {
            to = order[to];
        }
        if to != at {
            candidates.swap(at, to);
            if drafts_align {
                drafts.swap(at, to);
            }
        }
    }
}

/// Tests for the re-rank, against an in-memory model and literal candidate lists.
#[cfg(test)]
mod tests;
