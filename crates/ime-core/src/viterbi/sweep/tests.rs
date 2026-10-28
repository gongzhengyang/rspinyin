//! Unit tests for the sweep: the storage it lends out, and the merge that cuts one
//! decode's candidate list to its limit.
//!
//! They live in a file of their own because the sweep's own file is at its line
//! budget. The merge is the one place a decode decides how many candidates survive,
//! which is why the candidate limit is pinned here rather than at the decoder that
//! carries the number.

use super::*;

use ime_types::{DecodeRequest, DecodeResult};

use crate::confusable::ConfusableTable;
use crate::lm::InMemoryLm;
use crate::state::paging::{MAX_PAGE_SIZE, MAX_PAGES, MAX_REACHABLE_CANDIDATES, Paging};
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
            corrected: false,
            whole_sentence: false,
            readings: String::new(),
            words: WordSpans::EMPTY,
        })
        .collect()
}

/// The scores of a draft list, in the order the merge left them.
fn scores(drafts: &[Draft]) -> Vec<i32> {
    drafts.iter().map(|draft| draft.score).collect()
}

/// The empty result a freeze writes into, as a decode's first call finds it.
fn empty_result() -> DecodeResult {
    DecodeResult {
        candidates: Vec::new(),
        segments: Vec::new(),
        degraded: false,
    }
}

/// One draft with the fields the annotation tests read, and no correction mark.
fn plain_draft(text: &str, score: i32) -> Draft {
    Draft {
        text: String::from(text),
        source: CandidateSource::Dict,
        syllables: 2,
        score,
        corrected: false,
        whole_sentence: false,
        readings: String::new(),
        words: WordSpans::EMPTY,
    }
}

#[test]
fn test_result_of_into_annotates_a_corrected_draft_and_leaves_an_exact_one_quiet() {
    let drafts = vec![
        Draft {
            corrected: true,
            ..plain_draft("真诚", 100)
        },
        plain_draft("整好", 90),
    ];
    let mut out = empty_result();
    result_of_into(&mut out, &drafts, false, "zhegn", None);
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("纠错：zhegn"),
        "a corrected candidate names the spelling the user typed"
    );
    assert_eq!(
        out.candidates[1].annotation, None,
        "an exact candidate is annotated with nothing"
    );
}

#[test]
fn test_result_of_into_rewrites_a_slot_annotation_across_decodes() {
    let corrected = vec![Draft {
        corrected: true,
        ..plain_draft("真诚", 100)
    }];
    let mut out = empty_result();
    // First decode: the corrected candidate seeds the slot's annotation buffer.
    result_of_into(&mut out, &corrected, false, "zhegn", None);
    assert_eq!(out.candidates[0].annotation.as_deref(), Some("纠错：zhegn"));
    // Second decode of a different slip: the slot must follow the new spelling
    // rather than keep the previous input's annotation behind it.
    result_of_into(&mut out, &corrected, false, "zhoeng", None);
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("纠错：zhoeng")
    );
    // Third decode, exact input, same candidate count: the slot that held an
    // annotation now holds an exact reading, and the annotation is gone.
    let exact = vec![plain_draft("真诚", 100)];
    result_of_into(&mut out, &exact, false, "zheng", None);
    assert_eq!(out.candidates[0].annotation, None);
}

#[test]
fn test_result_of_into_annotates_a_fuzzy_draft_with_the_standard_reading() {
    // A fuzzy candidate has no correction mark but does have a standard reading:
    // the widened edge's key is what the word is really read as, and the hint says
    // so without naming a spelling the user never typed.
    let drafts = vec![Draft {
        readings: String::from("shuo'fu"),
        ..plain_draft("说服", 100)
    }];
    let mut out = empty_result();
    result_of_into(&mut out, &drafts, false, "shuo'fu", None);
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("音:shuo'fu"),
        "a fuzzy candidate names the reading it was found under"
    );
}

#[test]
fn test_result_of_into_prefaces_a_corrected_draft_with_its_standard_reading() {
    // Both facts travel on one annotation: the reading the word is found under
    // first, then the spelling the user actually typed -- the pair explains the
    // candidate without the window needing a second field.
    let drafts = vec![Draft {
        corrected: true,
        readings: String::from("zheng"),
        ..plain_draft("真诚", 100)
    }];
    let mut out = empty_result();
    result_of_into(&mut out, &drafts, false, "zhegn", None);
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("音:zheng｜纠错：zhegn")
    );
}

#[test]
fn test_result_of_into_appends_a_confusable_hint_after_the_engine_hints() {
    let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are usable");
    let drafts = vec![plain_draft("按奈", 100)];
    let mut out = empty_result();
    result_of_into(&mut out, &drafts, false, "an'nai", Some(&table));
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("字:→按捺"),
        "a wrong form the table holds is named with the correct one"
    );
    // On a draft that also carries an engine hint the table's part comes last.
    let drafts = vec![Draft {
        corrected: true,
        readings: String::from("an'nai"),
        ..plain_draft("按奈", 100)
    }];
    let mut out = empty_result();
    result_of_into(&mut out, &drafts, false, "an'nai", Some(&table));
    assert_eq!(
        out.candidates[0].annotation.as_deref(),
        Some("音:an'nai｜纠错：an'nai｜字:→按捺")
    );
}

#[test]
fn test_result_of_into_leaves_a_confusable_miss_and_a_detached_table_quiet() {
    let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are usable");
    let mut out = empty_result();
    // A text the table does not hold is annotated with nothing, whether the table
    // is attached or not.
    let drafts = vec![plain_draft("按捺", 100)];
    result_of_into(&mut out, &drafts, false, "an'nai", Some(&table));
    assert_eq!(out.candidates[0].annotation, None, "a miss stays quiet");
    let drafts = vec![plain_draft("按奈", 100)];
    result_of_into(&mut out, &drafts, false, "an'nai", None);
    assert_eq!(
        out.candidates[0].annotation, None,
        "without the table the channel is off, not merely silent"
    );
}

#[test]
fn test_result_of_into_changes_no_order_text_or_score_for_its_hints() {
    // The hint channel is annotation-only: the same drafts through the same list
    // come out in the same order with the same texts and scores, with the table
    // attached and without it.
    let drafts = vec![
        Draft {
            readings: String::from("an'nai"),
            ..plain_draft("按奈", 100)
        },
        plain_draft("按捺", 90),
    ];
    let mut hinted = empty_result();
    let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are usable");
    result_of_into(&mut hinted, &drafts, false, "an'nai", Some(&table));
    let mut quiet = empty_result();
    result_of_into(&mut quiet, &drafts, false, "an'nai", None);
    assert_eq!(
        hinted
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.index, c.score))
            .collect::<Vec<_>>(),
        quiet
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.index, c.score))
            .collect::<Vec<_>>(),
        "hints decorate; they never reorder"
    );
}

#[test]
fn test_is_widened_follows_the_fuzzy_and_correction_penalties_only() {
    use crate::segment::abbrev::ABBREV_PENALTY_Q8;
    assert!(is_widened(crate::fuzzy::FUZZY_PENALTY_Q8));
    assert!(is_widened(crate::segment::correct::CORRECTION_PENALTY_Q8));
    assert!(
        !is_widened(ABBREV_PENALTY_Q8),
        "typing initials is a choice, not a slip: no reading is hinted at"
    );
    assert!(!is_widened(0), "an exact edge is not widened");
}

#[test]
fn test_append_standard_reading_joins_readings_and_skips_unusable_ranges() {
    // The reading is the whole key the word was found under, one arena slice per
    // widened word; two widened words join the way two keys do.
    let keys = b"wan'ba";
    let mut readings = String::new();
    append_standard_reading(&mut readings, keys, (0, 3));
    append_standard_reading(&mut readings, keys, (4, 2));
    assert_eq!(readings, "wan'ba");
    // A range past the arena and an empty range append nothing, and a slice that
    // is not UTF-8 -- which the build cannot produce, but the reader refuses
    // anyway -- is skipped rather than failed on.
    let mut readings = String::new();
    append_standard_reading(&mut readings, keys, (100, 2));
    append_standard_reading(&mut readings, keys, (0, 100));
    append_standard_reading(&mut readings, keys, (0, 0));
    append_standard_reading(&mut readings, b"\xff\xfe", (0, 2));
    assert_eq!(
        readings, "",
        "an unusable range is skipped, never a failure"
    );
}

#[test]
fn test_finish_merges_a_corrected_twin_into_one_annotated_candidate() {
    // The same text reached twice -- once over a correction edge, once exactly --
    // folds into one candidate that keeps the better score and the correction mark:
    // the mark says the text was reachable past a slip, whatever the best path was.
    let mut drafts = vec![
        Draft {
            corrected: true,
            ..plain_draft("真诚", 50)
        },
        plain_draft("真诚", 90),
    ];
    finish(&mut drafts, DEFAULT_MAX_CANDIDATES);
    assert_eq!(drafts.len(), 1, "the twins fold into one candidate");
    assert!(
        drafts[0].corrected,
        "the correction mark survives the merge"
    );
    assert_eq!(drafts[0].score, 90, "the better score wins the fold");
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
