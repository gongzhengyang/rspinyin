//! Unit tests for the sweep: the storage it lends out, and the merge that cuts one
//! decode's candidate list to its limit.
//!
//! They live in a file of their own because the sweep's own file is at its line
//! budget. The merge is the one place a decode decides how many candidates survive,
//! which is why the candidate limit is pinned here rather than at the decoder that
//! carries the number.

use super::*;

use ime_types::DecodeRequest;

use crate::lm::InMemoryLm;
use crate::state::paging::{MAX_PAGES, MAX_PAGE_SIZE, MAX_REACHABLE_CANDIDATES, Paging};
use crate::viterbi::decoder::{DEFAULT_MAX_CANDIDATES, DecodeConfig, Decoder, MAX_CANDIDATES};
use crate::viterbi::lattice::WORDS_PER_KEY;
use crate::viterbi::lattice::testing::{NoUser, PageLexicon};

#[test]
fn test_sweep_storage_prepare_grows_to_the_sweep_it_will_hold() {
    let mut storage = SweepStorage::new();
    assert_eq!(storage.slots.len(), 0, "an empty storage holds nothing");
    storage.prepare(4, 16);
    assert_eq!(storage.slots.len(), 64);
    // A wider sweep than the storage holds grows it; a smaller one reuses it.
    storage.prepare(8, 16);
    assert_eq!(storage.slots.len(), 128);
    storage.prepare(2, 16);
    assert_eq!(
        storage.slots.len(),
        128,
        "a smaller sweep keeps the allocation"
    );
}

#[test]
fn test_sweep_storage_prepare_cuts_the_beam_and_the_nodes_to_their_ceilings() {
    let mut storage = SweepStorage::new();
    // A configuration past either ceiling cannot make the storage allocate without bound:
    // the ceilings are what the sweep clamps to as well.
    storage.prepare(usize::MAX, usize::from(MAX_BEAM_K) + 8);
    assert_eq!(storage.slots.len(), MAX_NODES * usize::from(MAX_BEAM_K));
    let mut zero = SweepStorage::new();
    zero.prepare(4, 0);
    assert_eq!(zero.slots.len(), 0, "a beam of no slots needs no storage");
}

/// Builds `count` drafts, one per word, scored so that the first one is the best.
///
/// The texts are all different, so the merge has nothing to fold together and the list
/// it is handed is the length the test asked for.
fn ranked_drafts(count: usize) -> Vec<Draft> {
    (0..count)
        .map(|index| Draft {
            text: format!("word{index}"),
            source: CandidateSource::Dict,
            syllables: 1,
            score: i32::try_from(count - index).expect("the test counts are small"),
        })
        .collect()
}

/// The scores of a draft list, in the order the merge left them.
fn scores(drafts: &[Draft]) -> Vec<i32> {
    drafts.iter().map(|draft| draft.score).collect()
}

#[test]
fn test_finish_keeps_the_best_candidates_up_to_the_shipped_limit() {
    // The boundary the window's reach draws: five pages of nine candidates are all
    // reachable, and a 46th word would be decoded and never shown. The merge is where
    // that boundary lives, so it is checked one below the limit, exactly at it, and one
    // past it: what survives is the head of the ranking, in order.
    let limit = usize::from(DEFAULT_MAX_CANDIDATES);
    assert_eq!(limit, 45, "the shipped limit is five pages of nine");
    for count in [limit - 1, limit, limit + 1] {
        let mut drafts = ranked_drafts(count);
        finish(&mut drafts, DEFAULT_MAX_CANDIDATES);
        assert_eq!(drafts.len(), count.min(limit), "{count} drafts");
        let seen = scores(&drafts);
        assert!(
            seen.windows(2).all(|pair| pair[0] > pair[1]),
            "{count} drafts must stay ordered best first: {seen:?}"
        );
        assert_eq!(
            seen[0],
            i32::try_from(count).expect("the test counts are small"),
            "{count} drafts must keep the best one"
        );
    }
}

#[test]
fn test_finish_keeps_a_list_past_the_window_reach_when_configured_for_it() {
    // One past the window's reach is still inside the decoder's ceiling, so a
    // configuration may ask for it and then nothing is dropped. `MAX_CANDIDATES` is the
    // point past which the configuration is refused rather than cut.
    let limit = DEFAULT_MAX_CANDIDATES + 1;
    assert!(
        limit <= MAX_CANDIDATES,
        "the ceiling is what makes {limit} a legal limit"
    );
    let mut drafts = ranked_drafts(usize::from(limit) + 1);
    finish(&mut drafts, limit);
    assert_eq!(drafts.len(), usize::from(limit));
}

#[test]
fn test_decode_keeps_the_candidate_list_inside_the_page_reach() {
    // A dictionary that answers every query with a full page of words, so the decode
    // ranks as many readings of the input as the beam holds.
    let lexicon = PageLexicon::new(WORDS_PER_KEY);
    let lm = InMemoryLm::new();
    let request = DecodeRequest::new("nihao");
    for max_candidates in [1u16, DEFAULT_MAX_CANDIDATES, MAX_CANDIDATES] {
        let cfg = DecodeConfig {
            beam_k: MAX_BEAM_K,
            max_candidates,
            ..DecodeConfig::default()
        };
        let decoder = Decoder::new(cfg).expect("the ceilings are hard");
        let result = decoder.decode(&request, &lexicon, &NoUser, &lm);
        assert!(
            !result.candidates.is_empty(),
            "a decode never answers with an empty list"
        );
        assert!(
            result.candidates.len() <= usize::from(max_candidates),
            "{} candidates for a limit of {max_candidates}",
            result.candidates.len()
        );
    }
    // The shipped limit is exactly the reach of the five-page window, and the page count
    // saturates there: a list one candidate longer still needs five pages, so its last
    // candidate could never be shown. That is the reason the limit is the number it is.
    assert_eq!(DEFAULT_MAX_CANDIDATES, MAX_REACHABLE_CANDIDATES);
    let paging = Paging::with_page_size(MAX_PAGE_SIZE);
    assert_eq!(paging.page_count(DEFAULT_MAX_CANDIDATES), MAX_PAGES);
    assert_eq!(paging.page_count(DEFAULT_MAX_CANDIDATES + 1), MAX_PAGES);
}
