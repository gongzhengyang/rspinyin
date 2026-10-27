//! Tests for the correction algebra: the neighbour table's shape, the one-step
//! property, the cap, and the determinism of the order.

use ime_types::{DecodeError, DecodeFlags};

use super::{
    CORRECTION_PENALTY_Q8, CorrectionSet, MAX_CORRECTIONS, NEIGHBORS, corrections_into,
    corrections_of, is_enabled, neighbor_row,
};
use crate::segment::syllable::{MAX_SYLLABLE_LEN, SYLLABLES, lookup, syllable_at};
use crate::segment::{EdgeKind, SyllableDag};
use crate::{fuzzy, segment};

/// Misspellings and legal spellings alike: the property sweep runs over all of
/// them, because the function is total and its guarantees must hold on every
/// input a span can carry.
const CORPUS: [&str; 10] = [
    "zhegn", "eo", "xiagn", "zhongx", "abc", "zhuangx", "wo", "nihao", "zheng", "lüx",
];

/// Answers whether `output` is one neighbour substitution or one adjacent
/// transposition away from `input`, checked with string logic alone.
///
/// Deliberately independent of the expansion loop: this is the property the card
/// pins ("one step away"), and a checker that shared the loop's code could not
/// catch the loop breaking it.
fn is_one_edit_away(input: &str, output: &str) -> bool {
    let (from, to) = (input.as_bytes(), output.as_bytes());
    if from.len() != to.len() {
        return false;
    }
    let diffs: Vec<usize> = from
        .iter()
        .zip(to)
        .enumerate()
        .filter_map(|(i, (x, y))| (x != y).then_some(i))
        .collect();
    match diffs.as_slice() {
        // One byte differs: it must have been replaced by a keyboard neighbour.
        [i] => neighbor_row(from[*i]).is_some_and(|row| row.contains(&to[*i])),
        // Two adjacent bytes differ: they must have been swapped, and the swap
        // must have moved something (two equal letters transpose to themselves).
        [i, j] => *j == i + 1 && from[*i] == to[*j] && from[*j] == to[*i] && from[*i] != from[*j],
        _ => false,
    }
}

#[test]
fn test_neighbor_table_covers_the_alphabet_symmetrically() {
    assert_eq!(NEIGHBORS.len(), 26, "one row per letter");
    for (index, row) in NEIGHBORS.iter().enumerate() {
        let letter = b'a' + index as u8;
        assert!(!row.is_empty(), "{:?}", letter as char);
        for &neighbor in row.iter() {
            assert!(
                neighbor.is_ascii_lowercase(),
                "row of {} holds a non-letter",
                letter as char
            );
            assert_ne!(neighbor, letter, "a key is not its own neighbour");
            let back = NEIGHBORS[usize::from(neighbor - b'a')];
            assert!(
                back.contains(&letter),
                "{} lists {} but not the way back",
                letter as char,
                neighbor as char
            );
        }
    }
}

#[test]
fn test_corrections_of_finds_the_transposition_and_the_substitution() {
    let spelled: Vec<&str> = corrections_of("zhegn")
        .iter()
        .filter_map(|id| syllable_at(*id))
        .collect();
    assert_eq!(spelled, ["zheng"], "the g/n slip is corrected: {spelled:?}");

    let spelled: Vec<&str> = corrections_of("eo")
        .iter()
        .filter_map(|id| syllable_at(*id))
        .collect();
    // Pinned in full because the order is contract: substitutions walk left to
    // right and take their neighbours in table order, transpositions follow.
    assert_eq!(
        spelled,
        ["wo", "ei"],
        "the w/e slip is corrected: {spelled:?}"
    );
}

#[test]
fn test_corrections_of_answers_only_legal_syllables_within_the_cap() {
    for text in SYLLABLES.iter().copied().chain(CORPUS) {
        let out = corrections_of(text);
        assert!(
            out.len() <= MAX_CORRECTIONS,
            "{text} expanded to {} corrections",
            out.len()
        );
        let mut seen = std::collections::HashSet::new();
        for id in out.iter().copied() {
            let spelling = syllable_at(id)
                .unwrap_or_else(|| panic!("{text} reached an id the table cannot spell"));
            assert_eq!(lookup(spelling), Some(id), "{text} reached a stale id");
            assert!(seen.insert(id), "{text} reached {spelling} twice");
        }
    }
}

#[test]
fn test_corrections_of_is_one_edit_away_and_never_the_input() {
    for text in SYLLABLES.iter().copied().chain(CORPUS) {
        for id in corrections_of(text) {
            let spelling = syllable_at(id).expect("a legal syllable");
            assert_ne!(spelling, text, "{text} corrected to itself");
            assert!(
                is_one_edit_away(text, spelling),
                "{spelling} is not one edit away from {text}"
            );
        }
    }
}

#[test]
fn test_corrections_of_is_deterministic_and_into_clears_first() {
    for text in CORPUS {
        let first = corrections_of(text);
        let second = corrections_of(text);
        assert_eq!(first, second, "two runs disagreed on {text}");
    }
    // The into form is the reuse path: a set left dirty by an earlier span must
    // not leak its leftovers into the next answer.
    let mut out: CorrectionSet = corrections_of("zhegn");
    assert!(!out.is_empty());
    corrections_into(&mut out, "zhuangx");
    assert!(out.is_empty(), "a 7-byte span corrects to nothing: {out:?}");
    corrections_into(&mut out, "eo");
    let spelled: Vec<&str> = out.iter().filter_map(|id| syllable_at(*id)).collect();
    assert_eq!(spelled, ["wo", "ei"]);
}

#[test]
fn test_corrections_of_skips_non_letter_bytes_and_overlong_spans() {
    // The two-byte `ü` must never be split by a substitution or a transposition,
    // and no neighbour of the letters around it lands on a syllable here.
    assert!(corrections_of("lüx").is_empty());
    // Both edits preserve length, so a span longer than the longest syllable the
    // table holds is refused before the walk.
    let long = "zhuangx";
    assert_eq!(long.len(), MAX_SYLLABLE_LEN + 1);
    assert!(corrections_of(long).is_empty());
    assert!(corrections_of("").is_empty());
}

#[test]
fn test_correction_penalty_far_exceeds_fuzzy_and_gates_on_its_bit() {
    // The value is spelled 998 << 8 to keep the shifted shape of its sibling
    // penalties; pinned here so a silent retuning shows up as a test failure.
    assert_eq!(CORRECTION_PENALTY_Q8, 998 << 8);
    // The ladder relationships read as constant-valued assertions for the same
    // reason the pin above does not: the operands are constants, and the pin is
    // the point -- a retuning that broke the ordering must fail here. Silenced
    // rather than restated as literals, which could drift from the constants.
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(CORRECTION_PENALTY_Q8 > fuzzy::FUZZY_PENALTY_Q8);
        assert!(fuzzy::FUZZY_PENALTY_Q8 > segment::abbrev::ABBREV_PENALTY_Q8);
    }
    assert!(!is_enabled(DecodeFlags::default()));
    assert!(!is_enabled(DecodeFlags::FUZZY | DecodeFlags::ABBREV));
    assert!(is_enabled(DecodeFlags::CORRECTION | DecodeFlags::USER_DICT));
}

/// Builds a graph for `raw` under `flags`, failing the test if the build errs.
fn built(raw: &str, flags: DecodeFlags) -> SyllableDag {
    let mut dag = SyllableDag::new();
    assert_eq!(dag.build_for(raw, flags), Ok(()), "building {raw:?}");
    dag
}

#[test]
fn test_dag_expands_correction_edges_only_after_an_exact_miss() {
    // Without the bit the input has no path at all; with it, the transposed
    // spelling reaches the syllable it slipped from.
    let mut off = SyllableDag::new();
    assert!(matches!(
        off.build_for("zhegn", DecodeFlags::empty()),
        Err(DecodeError::NoPath { .. })
    ));
    let zheng = lookup("zheng").expect("a syllable of the table");
    let on = built("zhegn", DecodeFlags::CORRECTION);
    let edge = on
        .edges_from(0)
        .iter()
        .find(|edge| edge.syllable == zheng)
        .unwrap_or_else(|| panic!("the corrected syllable is reachable"));
    assert_eq!(edge.end, 5, "the edge covers the bytes the user typed");
    assert_eq!(edge.penalty_q8, CORRECTION_PENALTY_Q8);
    assert_eq!(edge.kind, EdgeKind::Normal);
    // The misspelled spans are expanded each on its own: `gn` transposes onto the
    // interjection `ng`, which is a legal correction of that span alone, while
    // the exact syllable `n` at the end carries no correction edge at all -- only
    // its exact one, at zero penalty.
    let ng = lookup("ng").expect("a syllable of the table");
    assert!(
        on.edges_from(3).iter().any(|edge| edge.syllable == ng),
        "the gn span is corrected on its own"
    );
    let n = lookup("n").expect("a syllable of the table");
    assert!(
        on.edges_from(4)
            .iter()
            .all(|edge| edge.syllable == n && edge.penalty_q8 == 0),
        "an exact syllable is never corrected"
    );
}

#[test]
fn test_dag_with_correction_on_matches_the_exact_build_on_an_exact_hit() {
    // The structural guarantee: an input the exact build can cut never enters
    // the correction pass, so its graph is byte-identical with the bit clear.
    for raw in ["nihao", "xian", "zhuang", "ni'hao"] {
        let off = built(raw, DecodeFlags::empty());
        let on = built(raw, DecodeFlags::CORRECTION);
        for node in 0..=usize::from(off.len()) {
            assert_eq!(
                off.edges_from(node),
                on.edges_from(node),
                "exact hit {raw:?} changed at node {node}"
            );
        }
    }
}

#[test]
fn test_dag_correction_edge_carries_a_forced_boundary() {
    // The `'` after the misspelled span pins the boundary for the corrected edge
    // exactly as it would for an exact one: the edge reaches past the marker.
    let dag = built("zhegn'guo", DecodeFlags::CORRECTION);
    let zheng = lookup("zheng").expect("a syllable of the table");
    let edge = dag
        .edges_from(0)
        .iter()
        .find(|edge| edge.syllable == zheng)
        .unwrap_or_else(|| panic!("the corrected syllable is reachable"));
    assert_eq!(edge.end, 6, "the edge ends past the marker");
    assert_eq!(edge.kind, EdgeKind::Forced);
}

#[test]
fn test_dag_correction_names_the_syllable_and_keeps_the_bytes() {
    // The variant mechanism fuzzy matching established: the edge covers the bytes
    // the input spells and names the syllable the dictionary is asked for, so
    // every preedit offset keeps pointing at the byte it pointed at.
    let dag = built("zhegn", DecodeFlags::CORRECTION);
    let zheng = lookup("zheng").expect("a syllable of the table");
    let edge = *dag
        .edges_from(0)
        .iter()
        .find(|edge| edge.syllable == zheng)
        .expect("the corrected edge");
    assert_eq!(dag.normalized(), "zhegn");
    assert_eq!(dag.syllable_text(0, edge), "zhegn");
    assert_eq!(syllable_at(edge.syllable), Some("zheng"));
}

#[test]
fn test_dag_still_answers_no_path_when_nothing_corrects() {
    // Correction widens a misspelling; it does not invent a reading for input
    // that no one-step edit can reach. No neighbour of `t` is a syllable, so no
    // span of `ttt` corrects onto anything.
    let mut dag = SyllableDag::new();
    assert!(matches!(
        dag.build_for("ttt", DecodeFlags::CORRECTION),
        Err(DecodeError::NoPath { .. })
    ));
    assert!(!dag.has_path());
}
