//! Unit tests for the exact walk: what one node's spans look up, what the fallbacks
//! fill in, and what the lattice answers about the ranges it holds.
//!
//! They live in a file of their own because the walk's own file is at its line budget,
//! and they read the walk through its public entry points only. The abbreviation walk
//! beside it keeps its tests in `abbrev/tests.rs`, where the walk itself lives.

use super::testing::{MockLexicon, NoUser};
use super::*;

/// The options a test builds with: the fallbacks on or off, and no abbreviation.
///
/// The abbreviation suite's tests pass the flag instead; every test here pins what the
/// exact walk produces, which is what the abbreviation edges are added beside.
fn options<'a>(
    readings: &'a mut Readings,
    keys: &'a mut Vec<u8>,
    fallback_single: bool,
) -> LatticeOptions<'a, 'static> {
    LatticeOptions {
        fallback_single,
        flags: DecodeFlags::empty(),
        readings,
        keys,
        english: None,
    }
}

/// Builds the lattice of `raw` against `lexicon`, with fallbacks on.
fn lattice_of<'a>(raw: &str, lexicon: &'a MockLexicon) -> Lattice<'a> {
    let mut dag = SyllableDag::new();
    assert!(dag.build(raw).is_ok(), "building {raw:?}");
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    build_lattice(
        &dag,
        lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    )
}

/// Returns the texts of the edges leaving `node`.
fn texts<'a>(lattice: &'a Lattice<'_>, node: usize) -> Vec<&'a str> {
    lattice
        .edges_from(node)
        .iter()
        .map(|edge| edge.word.text)
        .collect()
}

#[test]
fn test_build_lattice_finds_the_words_of_every_span() {
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
    let lattice = lattice_of("nihao", &lexicon);
    assert_eq!(texts(&lattice, 0), vec!["你", "你好"]);
    // `hao` starts at byte 2 of `nihao`, and the terminal node has no edges.
    assert_eq!(texts(&lattice, 2), vec!["好"]);
    assert!(lattice.edges_from(5).is_empty());
    assert!(!lattice.is_empty());
}

#[test]
fn test_build_lattice_reads_every_spelling_of_an_ambiguous_input() {
    // `xian` cuts four ways, and the readings are not interchangeable: the
    // one-syllable reading is 先 and the two-syllable one is 西安. A span is
    // therefore looked up once per spelling. Node indices are byte offsets, so the
    // 西 of `xi` and the 先 of `xian` both leave node 0, while the 安 of `xi'an`
    // starts two bytes in and leaves node 2.
    let lexicon = MockLexicon::with(&[("xian", "先"), ("xi", "西"), ("an", "安")]);
    let lattice = lattice_of("xian", &lexicon);
    assert_eq!(texts(&lattice, 0), vec!["西", "先"]);
    assert_eq!(texts(&lattice, 2), vec!["安"]);
    // Both readings reach the terminal node: 先 in one edge, and 西安 through
    // the `xi` edge followed by the `an` edge out of node 2.
    let direct = lattice
        .edges_from(0)
        .iter()
        .any(|edge| usize::from(edge.end) == 4);
    assert!(direct);
    assert!(!lattice.edges_from(2).is_empty());
}

#[test]
fn test_build_lattice_keeps_only_the_strongest_words_of_a_key() {
    let rows: Vec<(&'static str, &'static str)> = (0..WORDS_PER_KEY + 2)
        .map(|index| {
            (
                "shi",
                ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"][index],
            )
        })
        .collect();
    let lexicon = MockLexicon::with(&rows);
    let lattice = lattice_of("shi", &lexicon);
    let kept = lattice.edges_from(0);
    assert_eq!(kept.len(), WORDS_PER_KEY);
    assert_eq!(kept[0].word.text, "一");
    assert_eq!(kept[WORDS_PER_KEY - 1].word.text, "八");
}

#[test]
fn test_build_lattice_stops_spelling_spans_at_the_word_length_limit() {
    // Seven `a`s segment one way only -- a chain of seven syllables -- so the walk
    // can spell a span of any length up to the limit and no longer. The dictionary
    // holds a word for the six-syllable span and one for the seven-syllable span.
    let rows = [("a'a'a'a'a'a", "六"), ("a'a'a'a'a'a'a", "七")];
    let lexicon = MockLexicon::with(&rows);
    let lattice = lattice_of("aaaaaaa", &lexicon);
    // Two edges only: the six-syllable key is spelled from node 0 and from node 1,
    // and the seven-syllable key is never looked up, so no edge carries 七.
    assert_eq!(lattice.len(), 2);
    assert_eq!(texts(&lattice, 0), vec!["六"]);
    assert_eq!(texts(&lattice, 1), vec!["六"]);
    assert_eq!(lattice.edges_from(0)[0].syllables, MAX_WORD_SYLLABLES);
}

#[test]
fn test_build_lattice_falls_back_only_where_no_word_covers_the_syllable() {
    let lexicon = MockLexicon::with(&[("ni", "你")])
        .single("ni", &["伱"])
        .single("hao", &["好", "号", "浩", "郝"]);
    let lattice = lattice_of("nihao", &lexicon);
    // `ni` has a word of its own, so the fallback list is not used for it;
    // `hao` has none, so it is, and only its first three candidates.
    assert_eq!(texts(&lattice, 0), vec!["你"]);
    assert_eq!(texts(&lattice, 2), vec!["好", "号", "浩"]);
}

#[test]
fn test_build_lattice_without_the_fallback_switch_leaves_spans_uncovered() {
    let lexicon = MockLexicon::with(&[("ni", "你")]).single("hao", &["好"]);
    let mut dag = SyllableDag::new();
    assert!(dag.build("nihao").is_ok());
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    let lattice = build_lattice(
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, false),
    );
    assert_eq!(texts(&lattice, 0), vec!["你"]);
    assert!(lattice.edges_from(2).is_empty());
}

#[test]
fn test_build_lattice_labels_a_user_word() {
    // `coined` names the word, not the lookup key: the frozen `UserFreqSource` is
    // queried with the word text (`Scorer::edge_score` passes `uf.freq(word)`), so a
    // mock that recorded the key here would label nothing.
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好")]).coined("好");
    let lattice = lattice_of("nihao", &lexicon);
    assert_eq!(lattice.edges_from(0)[0].source, CandidateSource::Dict);
    assert_eq!(lattice.edges_from(2)[0].source, CandidateSource::UserDict);
}

#[test]
fn test_build_lattice_reports_a_refused_lookup() {
    let lexicon = MockLexicon::with(&[("ni", "你")]).failing("hao");
    let lattice = lattice_of("nihao", &lexicon);
    assert!(lattice.lookup_failed());
    // The failing key simply contributes nothing, and the fallback still
    // leaves a path to the end.
    assert_eq!(texts(&lattice, 0), vec!["你"]);
}

#[test]
fn test_build_lattice_gives_every_node_a_range_and_orders_the_edges() {
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
    let lattice = lattice_of("nihao", &lexicon);
    assert_eq!(lattice.node_count(), 6);
    let mut seen = 0usize;
    for node in 0..lattice.node_count() {
        let (first, last) = lattice.edge_range(node);
        assert!(first <= last);
        assert_eq!(last - first, lattice.edges_from(node).len());
        for index in first..last {
            assert!(lattice.edge_at(index).is_some());
        }
        seen += last - first;
    }
    assert_eq!(seen, lattice.len());
    // A node past the end, and an edge index past the end, are answered
    // rather than panicking.
    assert!(lattice.edges_from(lattice.node_count()).is_empty());
    assert!(lattice.edge_at(lattice.len()).is_none());
}

#[test]
fn test_build_lattice_of_an_empty_graph_has_no_edges() {
    let lexicon = MockLexicon::with(&[]);
    let dag = SyllableDag::new();
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    let lattice = build_lattice(
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    assert!(lattice.is_empty());
    assert_eq!(lattice.len(), 0);
    assert_eq!(lattice.node_count(), 1);
    assert!(lattice.edges_from(0).is_empty());
}

#[test]
fn test_build_lattice_counts_characters_and_syllables_of_each_edge() {
    let lexicon = MockLexicon::with(&[("zhong", "中"), ("zhong'guo", "中国")]);
    let lattice = lattice_of("zhongguo", &lexicon);
    let edges = lattice.edges_from(0);
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0].word.text, "中");
    assert_eq!(edges[0].syllables, 1);
    assert_eq!(edges[0].characters, 1);
    assert_eq!(edges[1].word.text, "中国");
    assert_eq!(edges[1].syllables, 2);
    assert_eq!(edges[1].characters, 2);
}

#[test]
fn test_lattice_edge_stays_within_its_size_budget() {
    // The edge is pushed once per word per span, so its width is what the edge vector
    // costs and what the sweep touches once per path that uses the edge. It cannot reach
    // the 24 bytes the design asks for: the borrowed `WordRef` alone is 24 bytes -- a
    // `&str` is a pointer and a length, plus the weight, the syllable count and the
    // flags -- and the penalty is another four. The assertion is therefore the width this
    // build actually has, which is what stops a field from being added unnoticed.
    //
    // The one field beyond the original shape is the key range the coining path reads:
    // eight bytes naming where the edge's canonical key spelling sits in the arena, so
    // that a commit after the decode can spell the same key a fresh lookup would. The
    // range is what makes the user-lexicon entry land under the key the next decode
    // actually looks up, which is why it rides the edge rather than being rebuilt from
    // the input -- a rebuilt spelling would be wrong for every input typed without its
    // separators.
    let width = core::mem::size_of::<LatticeEdge<'_>>();
    let word = core::mem::size_of::<WordRef<'_>>();
    assert!(
        width <= 48,
        "an edge is {width} bytes; the borrowed word is {word} of them, the penalty 4, \
         and the key range 8"
    );
    // The borrowed word is the floor: no edge is narrower than the word it holds.
    assert!(width > word);
}

#[test]
fn test_lattice_with_capacity_sizes_the_edge_vector_for_the_graph() {
    let lattice: Lattice<'_> = Lattice::with_capacity(4, true);
    assert!(lattice.is_empty());
    assert_eq!(lattice.node_count(), 0);
    assert_eq!(
        lattice.edges.capacity(),
        4 * (WORDS_PER_KEY + usize::from(MAX_WORD_SYLLABLES) - 1 + FALLBACK_SINGLES),
        "the fallbacks and the multi-syllable span margin are part of the estimate when \
         the fallback is on"
    );
    // A node count past the ceiling is cut to it, so a hand-built configuration cannot
    // make the lattice allocate without bound.
    let bounded: Lattice<'_> = Lattice::with_capacity(usize::MAX, false);
    assert_eq!(
        bounded.edges.capacity(),
        MAX_LATTICE_NODES * (WORDS_PER_KEY + usize::from(MAX_WORD_SYLLABLES) - 1)
    );
}

#[test]
fn test_build_lattice_into_replaces_the_contents_and_keeps_the_buffer() {
    let lexicon = MockLexicon::with(&[
        ("ni", "你"),
        ("hao", "好"),
        ("ni'hao", "你好"),
        ("zhong", "中"),
    ]);
    let mut dag = SyllableDag::new();
    assert!(dag.build("nihao").is_ok());
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    let mut lattice = Lattice::with_capacity(usize::from(dag.len()) + 1, true);
    build_lattice_into(
        &mut lattice,
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    assert_eq!(texts(&lattice, 0), vec!["你", "你好"]);
    let grown = lattice.edges.capacity();
    // A second build into the same lattice describes the second input and leaves the
    // buffer the first one grew in place.
    assert!(dag.build("zhongguo").is_ok());
    build_lattice_into(
        &mut lattice,
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    assert_eq!(texts(&lattice, 0), vec!["中"]);
    assert_eq!(lattice.node_count(), usize::from(dag.len()) + 1);
    assert!(
        lattice.edges.capacity() >= grown,
        "the edge vector keeps its allocation"
    );
    assert!(!lattice.lookup_failed());
}

#[test]
fn test_build_lattice_into_clears_a_refusal_of_the_previous_build() {
    let lexicon = MockLexicon::with(&[("ni", "你")]).failing("hao");
    let mut dag = SyllableDag::new();
    assert!(dag.build("nihao").is_ok());
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    let mut lattice = Lattice::default();
    build_lattice_into(
        &mut lattice,
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    assert!(lattice.lookup_failed(), "the second syllable is refused");
    let mut whole = SyllableDag::new();
    assert!(whole.build("ni").is_ok());
    build_lattice_into(
        &mut lattice,
        &whole,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    assert!(
        !lattice.lookup_failed(),
        "a build that reads everything clears the previous refusal"
    );
    assert_eq!(texts(&lattice, 0), vec!["你"]);
}

#[test]
fn test_build_lattice_leaves_the_abbreviation_flag_clear_without_the_switch() {
    // The exact walk never sets the truncation flag, whatever the dictionary answers:
    // the flag is the abbreviation enumeration's own report.
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
    let lattice = lattice_of("nihao", &lexicon);
    assert!(!lattice.abbrev_truncated());
    assert!(!lattice.lookup_failed());
}

#[test]
fn test_build_lattice_carries_a_correction_penalty_that_sits_inside_a_word() {
    // `awnba` spells nothing; the correction pass widens `awn` onto `wan`, and the
    // word the dictionary stores under `wan'ba` covers the widened span with more
    // word after it. The word's edge carries the correction penalty -- not the zero
    // of the exact span it ends with -- because a reading widened in the middle is
    // as widened as one widened at the end, and the penalty is what keeps it below
    // everything the input spells and what the annotation is read off of. The one
    // exact span the word also covers keeps its zero.
    let lexicon = MockLexicon::with(&[("wan", "完"), ("ba", "把"), ("wan'ba", "完把")]);
    let mut dag = SyllableDag::new();
    assert!(
        dag.build_for("awnba", DecodeFlags::default() | DecodeFlags::CORRECTION)
            .is_ok(),
        "the miss opens the correction gate"
    );
    let mut readings = Readings::new();
    let mut keys: Vec<u8> = Vec::new();
    let lattice = build_lattice(
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, &mut keys, true),
    );
    let correction = crate::segment::correct::CORRECTION_PENALTY_Q8;
    // The single corrected syllable.
    let wan = lattice
        .edges_from(0)
        .iter()
        .find(|edge| usize::from(edge.end) == 3)
        .expect("the corrected syllable is an edge");
    assert_eq!(wan.penalty_q8, correction);
    assert_eq!(wan.syllables, 1);
    // The word whose interior span is the corrected one.
    let word = lattice
        .edges_from(0)
        .iter()
        .find(|edge| usize::from(edge.end) == 5 && edge.syllables == 2)
        .expect("the word over the corrected span is an edge");
    assert_eq!(word.penalty_q8, correction);
    // The exact span that follows the correction.
    let ba = lattice
        .edges_from(3)
        .iter()
        .find(|edge| usize::from(edge.end) == 5)
        .expect("the exact span after the correction is an edge");
    assert_eq!(ba.penalty_q8, 0);
}
