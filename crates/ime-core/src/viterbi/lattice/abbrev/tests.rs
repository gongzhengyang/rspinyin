//! Unit tests for the abbreviation walk: which readings become edges, what one prefix
//! query answers, where the walk stops, and what it costs when it is off.

use ime_types::{CandidateSource, DecodeFlags, Lexicon};

use crate::lm::{InMemoryLm, Scorer};
use crate::segment::abbrev::ABBREV_PENALTY_Q8;
use crate::segment::{MAX_RAW_LEN, Readings, SyllableDag};

use super::super::testing::{MockLexicon, NoUser, PageLexicon};
use super::super::{Lattice, LatticeOptions, MAX_TOTAL_EDGES, WORDS_PER_KEY, build_lattice};

/// Builds the lattice of `raw` against `lexicon` under `flags`, fallbacks on.
///
/// The graph is built rather than required to have a path: an abbreviated input such as
/// `nh` has no full-pinyin reading at all, which is exactly the input the abbreviation
/// walk exists for, and the lattice is built over the graph either way.
fn lattice_of<'a, L: Lexicon>(raw: &str, lexicon: &'a L, flags: DecodeFlags) -> Lattice<'a> {
    let mut readings = Readings::new();
    let mut dag = SyllableDag::new();
    let _ = dag.build(raw);
    build_lattice(
        &dag,
        lexicon,
        &NoUser,
        LatticeOptions {
            fallback_single: true,
            flags,
            readings: &mut readings,
        },
    )
}

/// Returns the texts of the abbreviation edges leaving `node`.
fn abbreviated<'dict>(lattice: &Lattice<'dict>, node: usize) -> Vec<&'dict str> {
    lattice
        .edges_from(node)
        .iter()
        .filter(|edge| edge.penalty_q8 != 0)
        .map(|edge| edge.word.text)
        .collect()
}

#[test]
fn test_build_lattice_abbrev_adds_an_edge_for_a_prefix_hit() {
    // `nh` has no full-pinyin reading at all -- no syllable is spelled `nh` -- so the
    // word keyed `n'h` can only be reached through the abbreviation walk, and the key is
    // the query the reading spells, which is what a dictionary that indexes
    // abbreviations stores.
    let lexicon = MockLexicon::with(&[("n'h", "你好")]);
    let off = lattice_of("nh", &lexicon, DecodeFlags::empty());
    assert!(off.is_empty(), "nothing spells `nh` out");
    let on = lattice_of("nh", &lexicon, DecodeFlags::ABBREV);
    let hit = on
        .edges_from(0)
        .iter()
        .find(|edge| edge.word.text == "你好")
        .expect("the abbreviation walk reaches the word");
    assert_eq!(hit.end, 2, "the reading consumes the whole input");
    assert_eq!(hit.syllables, 2, "counted from the reading, not the entry");
    assert_eq!(hit.characters, 2);
    assert_eq!(hit.penalty_q8, ABBREV_PENALTY_Q8);
    assert_eq!(hit.source, CandidateSource::Dict);
    assert!(!on.lookup_failed());
    assert!(!on.abbrev_truncated());
}

#[test]
fn test_build_lattice_abbrev_spells_a_z_initial_both_ways() {
    // `z` stands for `z` and for `zh`, so the reading of `zg` spells two queries. A
    // dictionary keyed `zh'g` is reached through the second one, and one keyed `z'g`
    // through the first, which is what shows the enumeration covers both.
    let long = MockLexicon::with(&[("zh'g", "中国")]);
    let lattice = lattice_of("zg", &long, DecodeFlags::ABBREV);
    let hit = lattice
        .edges_from(0)
        .iter()
        .find(|edge| edge.word.text == "中国")
        .expect("the `zh` spelling reaches the word");
    assert_eq!(hit.end, 2);
    assert_eq!(hit.syllables, 2);
    assert_eq!(hit.penalty_q8, ABBREV_PENALTY_Q8);

    let short = MockLexicon::with(&[("z'g", "中国")]);
    let lattice = lattice_of("zg", &short, DecodeFlags::ABBREV);
    assert_eq!(abbreviated(&lattice, 0), vec!["中国"]);
}

#[test]
fn test_build_lattice_abbrev_keeps_only_a_word_of_the_readings_length() {
    // A prefix query answers every word whose key starts with the query, and a key can
    // carry more syllables than the reading spells: `n'h` is a prefix of `n'h'm`. Only a
    // word of exactly the reading's length covers the span the reading covers, so 你好吗
    // is offered at the node the three-syllable reading reaches and not at the two.
    let lexicon = MockLexicon::with(&[("n'h", "你好"), ("n'h'm", "你好吗")]);
    let lattice = lattice_of("nhm", &lexicon, DecodeFlags::ABBREV);
    let edges = lattice.edges_from(0);
    let two = edges
        .iter()
        .filter(|edge| edge.word.text == "你好")
        .collect::<Vec<_>>();
    assert!(!two.is_empty());
    assert!(
        two.iter().all(|edge| edge.end == 2),
        "the two-syllable span"
    );
    let three = edges
        .iter()
        .filter(|edge| edge.word.text == "你好吗")
        .collect::<Vec<_>>();
    assert!(!three.is_empty());
    assert!(
        three.iter().all(|edge| edge.end == 3),
        "the three-syllable span"
    );
}

#[test]
fn test_build_lattice_abbrev_ranks_below_a_full_spelling_at_equal_weight() {
    // Two readings of `nihao` reach a two-character word: the exact key `ni'hao` holds
    // 你好 and the abbreviated query `ni'h` holds 你号. The words are as long as each
    // other and weigh the same, so the two edges score the same before the penalty -- and
    // the full spelling is the one that must win.
    let lexicon = MockLexicon::with(&[("ni'hao", "你好"), ("ni'h", "你号")]);
    let lattice = lattice_of("nihao", &lexicon, DecodeFlags::ABBREV);
    let edges = lattice.edges_from(0);
    let full = edges
        .iter()
        .position(|edge| edge.word.text == "你好")
        .expect("the exact key is read");
    let short = edges
        .iter()
        .position(|edge| edge.word.text == "你号")
        .expect("the abbreviated query is read");
    assert!(
        full < short,
        "the full spelling is offered before the abbreviation"
    );
    assert_eq!(edges[full].penalty_q8, 0);
    assert_eq!(edges[short].penalty_q8, ABBREV_PENALTY_Q8);
    // What the sweep adds up: one segment each, the same word length and a model with
    // nothing to say about either word, so the penalty is the whole difference. The
    // arithmetic is spelled out here because the edge carries the penalty rather than
    // applying it -- the sweep owns an edge's score -- so this is the boundary where the
    // two sides meet, and a sweep that stopped reading `penalty_q8` would leave the
    // ordering above resting on the edge order alone.
    let scorer = Scorer::default();
    let lm = InMemoryLm::new();
    let spelled = scorer.edge_score(&lm, &NoUser, None, "你好", edges[full].characters);
    let abbreviated = scorer.edge_score(&lm, &NoUser, None, "你号", edges[short].characters)
        - edges[short].penalty_q8;
    assert_eq!(
        spelled - abbreviated,
        ABBREV_PENALTY_Q8,
        "the penalty is the whole difference"
    );
    assert!(abbreviated < spelled);
}

#[test]
fn test_build_lattice_abbrev_stops_at_the_edge_ceiling() {
    // An input of initials reaches a page of words per query, so the abbreviation walk
    // would add tens of thousands of edges; the ceiling is what keeps the lattice -- and
    // the sweep that walks it -- inside the decode budget.
    let lexicon = PageLexicon::new(WORDS_PER_KEY);
    let raw = format!("a{}", "z".repeat(MAX_RAW_LEN - 1));
    let exact = lattice_of(raw.as_str(), &lexicon, DecodeFlags::empty());
    let extended = lattice_of(raw.as_str(), &lexicon, DecodeFlags::ABBREV);
    assert!(
        exact.len() < MAX_TOTAL_EDGES,
        "the exact walk is not what reaches the ceiling"
    );
    assert_eq!(
        extended.len() - exact.len(),
        MAX_TOTAL_EDGES,
        "the extension walks fill the ceiling exactly"
    );
    // The priority the design fixes: an edge the exact walk found is never dropped, and
    // the abbreviation edges are added behind it, node by node, until the ceiling stops
    // them -- so the nodes near the end of the input hold none.
    for node in 0..extended.node_count() {
        let found = exact.edges_from(node);
        assert_eq!(
            extended.edges_from(node).get(..found.len()),
            Some(found),
            "node {node} keeps what the exact walk found, in order"
        );
    }
    assert!(
        abbreviated(&extended, MAX_RAW_LEN - 1).is_empty(),
        "the walk stopped before the end of the input"
    );
}

#[test]
fn test_build_lattice_abbrev_reports_a_truncated_enumeration() {
    // `aiai...` reads as `a` or as `ai` at every step, so its readings multiply with the
    // input; past the enumeration cap the vaguest of them are dropped, and the lattice
    // says so. An input whose readings all fit reports nothing.
    let lexicon = MockLexicon::with(&[]);
    let long = "ai".repeat(MAX_RAW_LEN / 2);
    let truncated = lattice_of(long.as_str(), &lexicon, DecodeFlags::ABBREV);
    assert!(truncated.abbrev_truncated(), "the cap left readings out");
    let short = lattice_of("nihao", &lexicon, DecodeFlags::ABBREV);
    assert!(!short.abbrev_truncated());
}

#[test]
fn test_build_lattice_abbrev_never_reaches_the_prefix_query_with_the_flag_clear() {
    // The dictionary refuses every prefix query. With the switch clear the refusal never
    // happens, which is the structural form of "the walk costs nothing when it is off":
    // no reading is enumerated and no query is made. With the switch set the same build
    // reports the refusal, so the assertion above is about the gate rather than about a
    // dictionary that could not have answered anyway.
    let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好")]).refusing_prefix();
    let off = lattice_of("nihao", &lexicon, DecodeFlags::empty());
    assert!(!off.lookup_failed());
    assert!(!off.abbrev_truncated());
    let on = lattice_of("nihao", &lexicon, DecodeFlags::ABBREV);
    assert!(on.lookup_failed());
    assert!(
        !on.abbrev_truncated(),
        "a refused query is not a cut enumeration"
    );
}

#[test]
fn test_build_lattice_abbrev_walks_only_a_node_a_path_reaches() {
    // Node 1 of `zhongguo` sits inside the syllable `zhong`, and nothing reaches it: the
    // graph has no edge that ends there, and the query that would create one (`z`) has no
    // answer in this dictionary. The same query from a node a path does reach -- node 0 of
    // `hongguo` -- does produce edges, so what is skipped is the node nothing reaches
    // rather than the query.
    let lexicon = MockLexicon::with(&[("h", "H"), ("hong", "红"), ("guo", "国")]);
    let dead = lattice_of("zhongguo", &lexicon, DecodeFlags::ABBREV);
    assert!(abbreviated(&dead, 1).is_empty());
    let live = lattice_of("hongguo", &lexicon, DecodeFlags::ABBREV);
    assert_eq!(abbreviated(&live, 0), vec!["H", "红"]);
}
