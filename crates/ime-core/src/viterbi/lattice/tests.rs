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
fn options(readings: &mut Readings, fallback_single: bool) -> LatticeOptions<'_> {
    LatticeOptions {
        fallback_single,
        flags: DecodeFlags::empty(),
        readings,
    }
}

/// Builds the lattice of `raw` against `lexicon`, with fallbacks on.
fn lattice_of<'a>(raw: &str, lexicon: &'a MockLexicon) -> Lattice<'a> {
    let mut dag = SyllableDag::new();
    assert!(dag.build(raw).is_ok(), "building {raw:?}");
    let mut readings = Readings::new();
    build_lattice(&dag, lexicon, &NoUser, options(&mut readings, true))
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
    let lattice = build_lattice(&dag, &lexicon, &NoUser, options(&mut readings, false));
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
    let lattice = build_lattice(&dag, &lexicon, &NoUser, options(&mut readings, true));
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
fn test_lattice_with_capacity_sizes_the_edge_vector_for_the_graph() {
    let lattice: Lattice<'_> = Lattice::with_capacity(4, true);
    assert!(lattice.is_empty());
    assert_eq!(lattice.node_count(), 0);
    assert_eq!(
        lattice.edges.capacity(),
        4 * (WORDS_PER_KEY + FALLBACK_SINGLES),
        "the fallbacks are part of the estimate when they are on"
    );
    // A node count past the ceiling is cut to it, so a hand-built configuration cannot
    // make the lattice allocate without bound.
    let bounded: Lattice<'_> = Lattice::with_capacity(usize::MAX, false);
    assert_eq!(bounded.edges.capacity(), MAX_LATTICE_NODES * WORDS_PER_KEY);
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
    let mut lattice = Lattice::with_capacity(usize::from(dag.len()) + 1, true);
    build_lattice_into(
        &mut lattice,
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, true),
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
        options(&mut readings, true),
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
    let mut lattice = Lattice::default();
    build_lattice_into(
        &mut lattice,
        &dag,
        &lexicon,
        &NoUser,
        options(&mut readings, true),
    );
    assert!(lattice.lookup_failed(), "the second syllable is refused");
    let mut whole = SyllableDag::new();
    assert!(whole.build("ni").is_ok());
    build_lattice_into(
        &mut lattice,
        &whole,
        &lexicon,
        &NoUser,
        options(&mut readings, true),
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
