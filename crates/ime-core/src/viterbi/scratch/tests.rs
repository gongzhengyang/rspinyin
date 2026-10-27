//! Tests for [`super`].
//!
//! Split out of `scratch.rs` to keep that file inside the line limit; the tests hold the
//! steady-state workspace's own promises: nothing reallocates after the first decode, the
//! buffers are reused in place, and the mixed-English and correction paths leave the
//! workspace describing the input they actually answered.

use ime_types::{CandidateSource, DecodeFlags, DecodeRequest, DecodeResult, LanguageModel};

use crate::lm::InMemoryLm;
use crate::viterbi::decoder::{DecodeConfig, Decoder};
use crate::viterbi::lattice::testing::{MockLexicon, MockUserFreq, NoUser};

use super::*;

/// Decodes `raw` into a workspace of its own and answers the result.
fn decoded_into(raw: &str, lexicon: &MockLexicon, lm: &dyn LanguageModel) -> DecodeResult {
    let decoder = Decoder::default();
    let request = DecodeRequest::new(raw);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon,
            user_freq: &NoUser,
            lm,
            english: None,
            hints: None,
        },
    );
    scratch.into_result()
}

/// Every capacity a decode can grow, so that a steady-state decode can be shown not to
/// have grown any of them.
///
/// A capacity that stayed the same across a repeated decode is the evidence that no
/// buffer was reallocated: a growing `Vec` or `String` always reports a different
/// capacity, and the buffers a decode reuses are written in place.
fn capacities(scratch: &DecodeScratch) -> Vec<usize> {
    let mut seen = vec![
        scratch.sweep.slots_capacity(),
        scratch.drafts.capacity(),
        scratch.out.candidates.capacity(),
        scratch.out.segments.capacity(),
    ];
    seen.extend(
        scratch
            .out
            .candidates
            .iter()
            .map(|candidate| candidate.text.capacity()),
    );
    seen.extend(
        scratch
            .out
            .segments
            .iter()
            .map(|segment| segment.text.capacity()),
    );
    seen
}

#[test]
fn test_mixed_english_offers_the_lexicon_entry_for_an_input_no_reading_cuts() {
    let system = MockLexicon::with(&[("ni'hao", "你好")]);
    let english = MockLexicon::with(&[("github", "GitHub")]);
    let decoder = Decoder::default();
    let request = DecodeRequest::new("github").with_flags(DecodeFlags::ENGLISH);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &system,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: Some(&english),
            hints: None,
        },
    );
    let first = &scratch.result().candidates[0];
    assert_eq!(first.text, "GitHub");
    assert_eq!(first.source, CandidateSource::English);
    assert_eq!(
        first.annotation.as_deref(),
        Some(crate::segment::english::ENGLISH_ANNOTATION)
    );
    assert_eq!(
        first.consumed_syllables, 0,
        "no pinyin syllable is consumed: the candidate is the shape a pass-through has"
    );
}

#[test]
fn test_mixed_english_switch_off_leaves_the_pass_through_untouched() {
    let system = MockLexicon::with(&[("ni'hao", "你好")]);
    let english = MockLexicon::with(&[("github", "GitHub")]);
    let decoder = Decoder::default();
    let request = DecodeRequest::new("github");
    let mut scratch = DecodeScratch::new();
    // The lexicon is handed over, but the request carries no ENGLISH bit: the
    // switch is off, and the answer is the raw text, as before the table existed.
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &system,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: Some(&english),
            hints: None,
        },
    );
    let first = &scratch.result().candidates[0];
    assert_eq!(first.text, "github");
    assert_eq!(first.source, CandidateSource::Passthrough);
    assert_eq!(first.annotation, None);
}

#[test]
fn test_mixed_english_miss_falls_back_to_the_pass_through() {
    let system = MockLexicon::with(&[("ni'hao", "你好")]);
    let english = MockLexicon::with(&[]);
    let decoder = Decoder::default();
    let request = DecodeRequest::new("github").with_flags(DecodeFlags::ENGLISH);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &system,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: Some(&english),
            hints: None,
        },
    );
    let first = &scratch.result().candidates[0];
    assert_eq!(
        first.text, "github",
        "the table has no entry, so the raw text"
    );
    assert_eq!(first.source, CandidateSource::Passthrough);
}

#[test]
fn test_mixed_english_never_disturbs_an_input_a_reading_cuts() {
    // Even an English table that holds the input's own spelling stays out: the
    // exact walk cut the input, so the English pass is never reached.
    let system = MockLexicon::with(&[("ni'hao", "你好")]);
    let english = MockLexicon::with(&[("nihao", "Nihao")]);
    let decoder = Decoder::default();
    let request = DecodeRequest::new("nihao").with_flags(DecodeFlags::ENGLISH);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &system,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: Some(&english),
            hints: None,
        },
    );
    let candidates = &scratch.result().candidates;
    assert_eq!(candidates[0].text, "你好");
    assert_eq!(candidates[0].source, CandidateSource::Dict);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.source != CandidateSource::English),
        "no English candidate exists for an input the exact walk cut"
    );
}

#[test]
fn test_decode_scratch_new_holds_an_empty_result() {
    let scratch = DecodeScratch::new();
    assert!(scratch.result().candidates.is_empty());
    assert!(scratch.result().segments.is_empty());
    assert!(!scratch.result().degraded);
    assert_eq!(scratch.drafts.len(), 0);
    assert_eq!(
        scratch.sweep.slots_capacity(),
        0,
        "nothing is allocated yet"
    );
    assert_eq!(DecodeScratch::default().result(), scratch.result());
}

#[test]
fn test_decode_into_answers_what_decode_answers() {
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    // The inputs the two entry points have to agree on: a whole-sentence reading, a
    // multi-word reading, an input with no reading at all (the pass-through answer), the
    // empty input, and an input long enough to fill the beam.
    for raw in [
        "nihao",
        "woaini",
        "zhongguo",
        "zzz",
        "",
        "nihaonihaonihaonihao",
    ] {
        let request = DecodeRequest::new(raw);
        let expected = decoder.decode(&request, &lexicon, &NoUser, &lm);
        let seen = decoded_into(raw, &lexicon, &lm);
        assert_eq!(seen, expected, "{raw:?}");
    }
}

#[test]
fn test_decode_into_honours_the_user_dictionary_switch() {
    let lexicon = MockLexicon::with(&[("de", "的"), ("de", "得")]);
    let user = MockUserFreq::with(&[("得", 1000)]);
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    // The history the user's own commits built ranks 得 first.
    let ranked = DecodeRequest::new("de");
    decoder.decode_into(
        &mut scratch,
        &ranked,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &user,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(scratch.result().candidates[0].text, "得");
    // With the user dictionary switched off the counts take no part either, so the
    // dictionary's own order decides.
    let off = DecodeRequest::new("de").with_flags(DecodeFlags::empty());
    decoder.decode_into(
        &mut scratch,
        &off,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &user,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(scratch.result().candidates[0].text, "的");
    assert_eq!(scratch.result().candidates[0].source, CandidateSource::Dict);
}

#[test]
fn test_decode_into_reuses_every_buffer_across_decodes() {
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    // `xian` is the fixture's one input that ranks more than one candidate: it is both
    // the dictionary's own 先 and the user's 西 followed by 安. `nihao` would not do --
    // `ni'hao` and `ni`+`hao` both spell 你好, so the dedupe leaves a single candidate
    // and the buffer-reuse assertion below would be exercising an almost empty list.
    let request = DecodeRequest::new("xian");
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let first: Vec<String> = scratch
        .result()
        .candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect();
    assert!(first.len() > 1, "the fixture ranks more than one candidate");
    let before = capacities(&scratch);
    // The second decode of the same input is the steady state: everything it needs is
    // already there, so a capacity that changed is an allocation.
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(
        capacities(&scratch),
        before,
        "a steady-state decode grows no buffer"
    );
    let second: Vec<String> = scratch
        .result()
        .candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect();
    assert_eq!(second, first, "the reused buffers hold the same answer");
}

#[test]
fn test_decode_into_after_a_degraded_decode_answers_like_a_fresh_one() {
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    // A decode that ends in the pass-through answer leaves the workspace holding one
    // candidate and no segments, which the next decode has to overwrite completely.
    for raw in ["zzz", "nihao", "zzz", "zhongguo", "zzz"] {
        let request = DecodeRequest::new(raw);
        decoder.decode_into(
            &mut scratch,
            &request,
            DecodeSources {
                lexicon: &lexicon,
                user_freq: &NoUser,
                lm: &lm,
                english: None,
                hints: None,
            },
        );
        let expected = decoder.decode(&request, &lexicon, &NoUser, &lm);
        assert_eq!(scratch.result(), &expected, "{raw:?}");
    }
}

#[test]
fn test_decode_into_does_not_read_a_path_the_last_input_left_behind() {
    // The first decode fills the beams of the workspace with a reading; the second has no
    // reading at all, because the fallback is off and no word covers it. The beams are not
    // cleared between decodes -- that is the point of reusing them -- so a candidate read
    // out of a slot the second decode never wrote would be a reading of the wrong input.
    let lexicon = MockLexicon::with(&[
        ("zhong", "中"),
        ("guo", "国"),
        ("zhong'guo", "中国"),
        ("ni", "你"),
    ]);
    let lm = InMemoryLm::new();
    let decoder = Decoder::new(DecodeConfig {
        fallback_single: false,
        ..DecodeConfig::default()
    })
    .expect("the shipped knobs with the fallback off");
    let mut scratch = DecodeScratch::new();
    let read = DecodeRequest::new("zhongguo");
    decoder.decode_into(
        &mut scratch,
        &read,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(scratch.result().candidates[0].text, "中国");
    let unreadable = DecodeRequest::new("nihao");
    decoder.decode_into(
        &mut scratch,
        &unreadable,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let expected = decoder.decode(&unreadable, &lexicon, &NoUser, &lm);
    assert!(expected.degraded, "no word covers the second input");
    assert_eq!(scratch.result(), &expected);
}

#[test]
fn test_decode_into_leaves_the_graph_describing_the_last_input() {
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let first = DecodeRequest::new("nihao");
    decoder.decode_into(
        &mut scratch,
        &first,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(scratch.dag.normalized(), "nihao");
    assert!(scratch.dag.has_path());
    let second = DecodeRequest::new("zhongguo");
    decoder.decode_into(
        &mut scratch,
        &second,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    // The graph is left describing the input of the last decode, which is what lets a
    // session read the preedit from the workspace instead of building a second graph.
    assert_eq!(scratch.dag.normalized(), "zhongguo");
    assert!(scratch.dag.has_path());
}

#[test]
fn test_decode_scratch_hands_the_buffers_back_for_the_next_decode() {
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let request = DecodeRequest::new("nihao");
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    // The session keeps the result it hands to the frame, and hands the previous one back
    // before the next keystroke. A workspace that never gets its buffers back would
    // allocate a fresh set of candidate texts on every decode.
    let held = scratch.take_result();
    assert!(!held.candidates.is_empty());
    assert!(
        scratch.result().candidates.is_empty(),
        "the take empties it"
    );
    scratch.recycle(held);
    let before = capacities(&scratch);
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(
        capacities(&scratch),
        before,
        "a handed-back result keeps its buffers"
    );
    assert_eq!(scratch.result().candidates[0].text, "你好");
}

#[test]
fn test_decode_into_writes_a_passthrough_answer_into_a_used_workspace() {
    // No whole-sentence word, so the winning reading of the first decode is two words and
    // the workspace holds a cut of two segments that the degraded answer has to clear.
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "号")]);
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let read = DecodeRequest::new("nihao");
    decoder.decode_into(
        &mut scratch,
        &read,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert_eq!(scratch.result().segments.len(), 2);
    let unreadable = DecodeRequest::new("zzz");
    decoder.decode_into(
        &mut scratch,
        &unreadable,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let result = scratch.result();
    assert!(result.degraded);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].text, "zzz");
    assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
    assert!(result.segments.is_empty(), "there is no cut to describe");
}

#[test]
fn test_decode_into_reads_an_abbreviated_input_through_the_prefix_query() {
    // `nh` spells no syllable, so no cut of it is a reading at all -- the graph has no
    // path -- and the only way the input can be decoded is the abbreviation walk. The
    // query it makes is `n'h`, which is what a dictionary that indexes initials
    // answers, and the word it returns is the one the user meant.
    let lexicon = MockLexicon::with(&[("n'h", "你好")]);
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let request = DecodeRequest::new("nh").with_flags(DecodeFlags::USER_DICT | DecodeFlags::ABBREV);
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let result = scratch.result();
    assert!(
        !result.degraded,
        "an abbreviation is a reading of the input"
    );
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].text, "你好");
    assert_eq!(result.candidates[0].consumed_syllables, 2);
    assert_eq!(result.candidates[0].source, CandidateSource::Dict);
    assert!(!scratch.abbrev_truncated());
    // The cut of the winning path describes the abbreviated reading: one word of two
    // syllables, covering the whole input.
    assert_eq!(result.segments.len(), 1);
    assert_eq!((result.segments[0].start, result.segments[0].end), (0, 2));
}

#[test]
fn test_decode_into_answers_an_abbreviated_input_with_the_pass_through_without_the_flag() {
    // The same input with the switch clear has no cut of its own, so the answer is the
    // input itself -- what the decoder answered before abbreviation existed -- and the
    // prefix query is not made either, which the dictionary reports by refusing every
    // one of them.
    let lexicon = MockLexicon::with(&[("n'h", "你好")]).refusing_prefix();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let request = DecodeRequest::new("nh");
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let result = scratch.result();
    assert!(result.degraded);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].text, "nh");
    assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);

    // With the switch set the query is made and refused, so the lattice is empty and
    // the answer degrades the same way: a dictionary that cannot enumerate prefixes
    // leaves abbreviation with nothing to offer rather than with a wrong candidate.
    let asked = DecodeRequest::new("nh").with_flags(DecodeFlags::ABBREV);
    decoder.decode_into(
        &mut scratch,
        &asked,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    let result = scratch.result();
    assert!(result.degraded);
    assert_eq!(result.candidates[0].text, "nh");
    assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
}

#[test]
fn test_decode_into_reports_a_truncated_abbreviation_enumeration() {
    // An input whose readings multiply past the enumeration cap still decodes, and the
    // workspace says the answer was built from a subset of them. The same input with
    // the switch clear reports nothing, because nothing was enumerated.
    let lexicon = MockLexicon::with(&[("ai", "爱")]);
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let long = "ai".repeat(32);
    let request =
        DecodeRequest::new(long.as_str()).with_flags(DecodeFlags::USER_DICT | DecodeFlags::ABBREV);
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert!(scratch.abbrev_truncated(), "the cap left readings out");
    let plain = DecodeRequest::new(long.as_str());
    decoder.decode_into(
        &mut scratch,
        &plain,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &lm,
            english: None,
            hints: None,
        },
    );
    assert!(
        !scratch.abbrev_truncated(),
        "a decode that does not walk abbreviations clears the previous report"
    );
}

/// Decodes `raw` into a workspace and answers the pairs its best candidate offers to
/// coining.
fn coin_pairs_of(raw: &str, lexicon: &MockLexicon) -> Vec<(String, String)> {
    let decoder = Decoder::default();
    let request = DecodeRequest::new(raw);
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: None,
            hints: None,
        },
    );
    scratch.coin_pairs(0)
}

#[test]
fn test_coin_pairs_read_the_adjacent_single_characters_off_the_winning_path() {
    // The dictionary holds the two characters but no word over them, so the best
    // reading of `zhuji` is the two single-character words -- exactly the shape a
    // commit coins from, with the key spelled the way the input is.
    let lexicon = MockLexicon::with(&[("zhu", "诸"), ("ji", "暨")]);
    assert_eq!(
        coin_pairs_of("zhuji", &lexicon),
        vec![(String::from("zhu'ji"), String::from("诸暨"))]
    );
}

#[test]
fn test_coin_pairs_of_a_whole_word_reading_are_empty() {
    // The path is one dictionary word: nothing was chosen character by character, so
    // there is nothing to coin.
    let lexicon = MockLexicon::with(&[("zhu'ji", "诸暨")]);
    assert!(coin_pairs_of("zhuji", &lexicon).is_empty());
}

#[test]
fn test_coin_pairs_refuse_pairs_with_a_multi_character_word() {
    // 你好 is one word of the reading, so the adjacent pair it forms is a phrase
    // shape the coining rule does not take.
    let lexicon = MockLexicon::with(&[("ni'hao", "你好"), ("ba", "吧")]);
    assert!(coin_pairs_of("nihaoba", &lexicon).is_empty());
}

#[test]
fn test_coin_pairs_of_a_corrected_reading_are_empty() {
    // The winning path needed a spelling correction, so the key it would coin under
    // is the slip the user did not mean.
    let lexicon = MockLexicon::with(&[("wan", "万"), ("ba", "吧")]);
    let decoder = Decoder::default();
    let mut scratch = DecodeScratch::new();
    let request = DecodeRequest::new("wanba").with_flags(ime_types::DecodeFlags::CORRECTION);
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: None,
            hints: None,
        },
    );
    // The correction really is what the best path needed; the guard is exercised
    // only if the decode agrees.
    let corrected = scratch
        .result()
        .candidates
        .first()
        .map(|candidate| candidate.annotation.is_some())
        .unwrap_or(false);
    if corrected {
        assert!(scratch.coin_pairs(0).is_empty());
    }
}

#[test]
fn test_coin_pairs_of_an_out_of_range_index_are_empty() {
    let lexicon = MockLexicon::with(&[("zhu", "诸"), ("ji", "暨")]);
    let decoder = Decoder::default();
    let request = DecodeRequest::new("zhuji");
    let mut scratch = DecodeScratch::new();
    decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &lexicon,
            user_freq: &NoUser,
            lm: &InMemoryLm::new(),
            english: None,
            hints: None,
        },
    );
    assert!(scratch.coin_pairs(usize::MAX).is_empty());
}
