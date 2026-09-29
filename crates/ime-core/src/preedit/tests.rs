//! Unit tests for the preedit builder.
//!
//! Responsibility: pin the spelling the text is made of, the four span invariants, the
//! size bound and the caret mapping from the raw input into the rendered text -- on the
//! fixed cases a reader can check by hand, and on random sessions.
//!
//! The random cases are the ones that carry the weight. `InputBuffer` lets the caret sit
//! wherever an edit left it, and the header draws whatever this builder answers, so a
//! position that renders badly is a visible defect rather than an internal one.
//!
//! Boundaries: everything here is in memory. No dictionary, no clock, no display server,
//! and every graph is built from the same string the buffer holds.

use ime_types::DecodeError;
use proptest::prelude::*;

use super::*;
use crate::input::BackspaceOutcome;
use crate::segment::MAX_RAW_LEN;
use crate::segment::syllable::normalize;

/// Builds a buffer holding `raw`, with the caret where the last keystroke left
/// it: at the end of the input.
fn buffer(raw: &str) -> InputBuffer {
    let mut buf = InputBuffer::new();
    for ch in raw.chars() {
        assert!(buf.push_char(ch).is_ok(), "pushing {ch:?} of {raw:?}");
    }
    buf
}

/// Builds a buffer holding `raw` with the caret `back` characters from the end.
fn buffer_with_caret_back(raw: &str, back: usize) -> InputBuffer {
    let mut buf = buffer(raw);
    for _ in 0..back {
        assert!(buf.move_caret(-1), "the caret steps back");
    }
    buf
}

/// Builds the graph of `raw`, which the caller asserts is segmentable.
fn dag_for(raw: &str) -> SyllableDag {
    let mut dag = SyllableDag::new();
    assert_eq!(dag.build(raw), Ok(()), "building {raw:?}");
    dag
}

/// Builds the graph of `raw`, which the caller asserts is not segmentable.
fn unsegmentable_dag(raw: &str) -> SyllableDag {
    let mut dag = SyllableDag::new();
    assert!(
        matches!(dag.build(raw), Err(DecodeError::NoPath { .. })),
        "{raw:?} must not be segmentable"
    );
    dag
}

/// The span a test expects.
fn span(start: u16, end: u16, kind: SpanKind) -> PreeditSpan {
    PreeditSpan { start, end, kind }
}

/// The span list a test expects, parsed from the notation [`parse_spans`] defines.
fn spans(spec: &str) -> Vec<PreeditSpan> {
    parse_spans(spec).expect("a well-formed span list")
}

/// Parses the span notation: one term per span, `S` for a syllable, `|` for a
/// separator, `P` for pass-through and `C` for the caret, each followed by its
/// `start..end` byte range.
///
/// Answers `None` for a term the notation does not define, so a typo in a test
/// fails loudly instead of being parsed into a different expectation.
fn parse_spans(spec: &str) -> Option<Vec<PreeditSpan>> {
    let mut parsed = Vec::new();
    for term in spec.split_whitespace() {
        let (kind, range) = term.split_at(1);
        let (start, end) = range.split_once("..")?;
        let kind = match kind {
            "S" => SpanKind::Syllable,
            "|" => SpanKind::Separator,
            "P" => SpanKind::Passthrough,
            "C" => SpanKind::Cursor,
            _ => return None,
        };
        parsed.push(span(start.parse().ok()?, end.parse().ok()?, kind));
    }
    Some(parsed)
}

/// Asserts the four invariants the module documents, naming the input in every
/// message so a failure points at the case that broke it.
fn assert_invariants(preedit: &Preedit, raw: &str) {
    let text = preedit.text.as_str();
    let caret = preedit.caret as usize;
    assert!(caret <= text.len(), "{raw:?}: caret past {text:?}");
    assert!(
        text.is_char_boundary(caret),
        "{raw:?}: caret splits a character"
    );

    // The size bound the module documents: every syllable covers at least one input
    // byte and every syllable but the last contributes a separator, and the two cannot
    // both be maximal -- a syllable whose spelling grows needs a second input byte to
    // grow into. It is what says how far past the header's budget a preedit can reach.
    //
    // Written as a strict `<` against the doubled length rather than `<= 2n - 1`: the two
    // are the same for a non-empty input, and the strict form cannot underflow.
    if !raw.is_empty() {
        assert!(
            text.len() < 2 * raw.len(),
            "{raw:?}: {text:?} is longer than the size bound"
        );
    }

    if text.is_empty() {
        assert!(preedit.spans.is_empty(), "empty text has no span");
        assert_eq!(caret, 0, "empty text has no caret");
        return;
    }

    let cursor: Vec<&PreeditSpan> = preedit
        .spans
        .iter()
        .filter(|span| span.kind == SpanKind::Cursor)
        .collect();
    assert_eq!(cursor.len(), 1, "{raw:?}: one cursor span");
    assert_eq!(cursor[0].start, cursor[0].end);
    assert_eq!(usize::from(cursor[0].start), caret, "{raw:?}: caret");

    for pair in preedit.spans.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        assert!(left.start <= right.start, "{raw:?}: spans unsorted");
        if left.start == right.start {
            assert_eq!(left.kind, SpanKind::Cursor, "{raw:?}: start shared");
        }
    }

    let tiled: Vec<&PreeditSpan> = preedit
        .spans
        .iter()
        .filter(|span| span.kind != SpanKind::Cursor)
        .collect();
    let mut at = 0usize;
    for span in tiled {
        assert!(span.end > span.start, "{raw:?}: zero width");
        assert_eq!(usize::from(span.start), at);
        at = usize::from(span.end);
    }
    assert_eq!(at, text.len(), "{raw:?}: spans stop short");
}

/// The fixed input set of the snapshot test: the raw input, the text it renders
/// as, and the spans, with the caret at the end of the input.
const SNAPSHOTS: &[(&str, &str, &str)] = &[
    ("ni", "ni", "S0..2 C2..2"),
    ("nihao", "ni'hao", "S0..2 |2..3 S3..6 C6..6"),
    (
        "ni'hao'a",
        "ni'hao'a",
        "S0..2 |2..3 S3..6 |6..7 S7..8 C8..8",
    ),
    ("zhongguo", "zhong'guo", "S0..5 |5..6 S6..9 C9..9"),
    ("zzz", "zzz", "P0..3 C3..3"),
];

#[test]
fn test_build_preedit_matches_the_snapshot_for_the_fixed_input_set() {
    assert_eq!(SNAPSHOTS.len(), 5, "the snapshot set is five inputs");
    for &(raw, text, expected) in SNAPSHOTS {
        // The illegal input fails to build: that is the state the header sees for
        // a string the segmentation cannot cut.
        let mut dag = SyllableDag::new();
        let _ = dag.build(raw);
        let preedit = build_preedit(&buffer(raw), &dag);
        assert_eq!(preedit.text, text, "text of {raw:?}");
        assert_eq!(preedit.caret, text.len() as u32, "caret of {raw:?}");
        assert_eq!(preedit.spans, spans(expected), "spans of {raw:?}");
        assert_invariants(&preedit, raw);
    }
}

#[test]
fn test_build_preedit_empty_input_returns_an_empty_preedit() {
    let preedit = build_preedit(&InputBuffer::new(), &SyllableDag::new());
    assert_eq!(preedit, empty_preedit());
    assert!(preedit.spans.is_empty(), "no span without input");
    assert_invariants(&preedit, "");
}

#[test]
fn test_build_preedit_inserts_the_cut_the_user_did_not_type() {
    let typed = build_preedit(&buffer("nihao"), &dag_for("nihao"));
    let marked = build_preedit(&buffer("ni'hao"), &dag_for("ni'hao"));
    assert_eq!(typed.text, "ni'hao");
    // A marker the user typed pins the boundary the segmentation would have drawn
    // there anyway, so the two renderings are indistinguishable.
    assert_eq!(typed, marked);
}

#[test]
fn test_build_preedit_drops_the_characters_normalization_folds_away() {
    let raw = "'ni''hao'";
    let preedit = build_preedit(&buffer(raw), &dag_for(raw));
    // Three markers carried no boundary information and left no trace, and the
    // caret still ends at the end of the rendered text.
    assert_eq!(preedit.text, "ni'hao");
    assert_eq!(preedit.caret, 6);
    assert_invariants(&preedit, raw);
}

#[test]
fn test_build_preedit_ends_the_text_when_the_input_ends_on_a_folded_marker() {
    let preedit = build_preedit(&buffer("ni'"), &dag_for("ni'"));
    assert_eq!(preedit.text, "ni");
    assert_eq!(preedit.caret, 2);
    assert_eq!(preedit.spans, spans("S0..2 C2..2"));
}

#[test]
fn test_build_preedit_without_a_path_renders_the_input_verbatim() {
    let preedit = build_preedit(&buffer("zzz"), &unsegmentable_dag("zzz"));
    assert_eq!(preedit.text, "zzz");
    assert_eq!(preedit.caret, 3);
    assert_eq!(preedit.spans, spans("P0..3 C3..3"));

    // A graph that was never built is the state before the first segmentation and
    // has no path either, so it takes the same branch -- and that branch keeps the
    // case the user typed, which is the text the pass-through candidate commits.
    let preedit = build_preedit(&buffer("Ni'ZZZ"), &SyllableDag::new());
    assert_eq!(preedit.text, "Ni'ZZZ");
    assert_eq!(preedit.caret, 6);
    assert_invariants(&preedit, "Ni'ZZZ");
}

#[test]
fn test_build_preedit_places_the_caret_of_a_passthrough_input() {
    // The pass-through text is the input itself, so the caret is an offset into that
    // same string and an edit that left it in the middle must not push it to the end.
    let buf = buffer_with_caret_back("zzz", 1);
    assert_eq!(buf.caret(), 2);
    let preedit = build_preedit(&buf, &unsegmentable_dag("zzz"));
    assert_eq!(preedit.text, "zzz");
    assert_eq!(preedit.caret, 2);
    assert_eq!(preedit.spans, spans("P0..3 C2..2"));
    assert_invariants(&preedit, "zzz");
}

#[test]
fn test_build_preedit_places_the_caret_around_a_boundary_the_user_typed() {
    // The marker belongs to no syllable, so the cut line the header draws for it is
    // the separator this builder inserts. A caret in front of the marker has to render
    // in front of that separator and a caret behind it behind it: the two positions
    // are distinct in the input and must stay distinct in the text.
    let buf = buffer_with_caret_back("ni'hao", 4);
    assert_eq!(buf.caret(), 2);
    let preedit = build_preedit(&buf, &dag_for("ni'hao"));
    assert_eq!(preedit.text, "ni'hao");
    assert_eq!(preedit.caret, 2);
    assert_eq!(preedit.spans, spans("S0..2 C2..2 |2..3 S3..6"));
    assert_invariants(&preedit, "ni'hao");

    let buf = buffer_with_caret_back("ni'hao", 3);
    assert_eq!(buf.caret(), 3);
    let preedit = build_preedit(&buf, &dag_for("ni'hao"));
    assert_eq!(preedit.text, "ni'hao");
    assert_eq!(preedit.caret, 3);
    assert_eq!(preedit.spans, spans("S0..2 |2..3 C3..3 S3..6"));
    assert_invariants(&preedit, "ni'hao");
}

#[test]
fn test_build_preedit_maps_the_caret_into_the_text() {
    // At the start of the input: the marker precedes the first span and shares its
    // start with it, which is the one place two spans may start together.
    let buf = buffer_with_caret_back("nihao", 5);
    let preedit = build_preedit(&buf, &dag_for("nihao"));
    assert_eq!(preedit.caret, 0);
    assert_eq!(preedit.spans, spans("C0..0 S0..2 |2..3 S3..6"));
    assert_invariants(&preedit, "nihao");

    // On a syllable boundary: the marker follows the cut line in front of it.
    let buf = buffer_with_caret_back("nihao", 3);
    let preedit = build_preedit(&buf, &dag_for("nihao"));
    assert_eq!(preedit.caret, 3);
    assert_eq!(preedit.spans, spans("S0..2 |2..3 C3..3 S3..6"));

    // Inside a syllable, which is where an edit that removed a character in front
    // of the caret leaves it: the marker keeps its distance from the syllable
    // start, and the separator in front of it is not counted.
    let mut buf = buffer("nihaoa");
    assert!(buf.move_caret(-2));
    assert_eq!(buf.backspace(), BackspaceOutcome::RemovedChar);
    assert_eq!(buf.caret(), 3);
    let preedit = build_preedit(&buf, &dag_for("nihao"));
    assert_eq!(preedit.caret, 4);
    assert_invariants(&preedit, "nihao");

    // Across a syllable whose spelling grows: `lv` renders as the two-byte umlaut,
    // so the caret at the end of the input is not at its own byte offset.
    let preedit = build_preedit(&buffer("lv"), &dag_for("lv"));
    assert_eq!(preedit.text, "lü");
    assert_eq!(preedit.caret, 3);

    // And a caret in front of that umlaut stays in front of it.
    let buf = buffer_with_caret_back("lv", 1);
    assert_eq!(buf.caret(), 1);
    let preedit = build_preedit(&buf, &dag_for("lv"));
    assert_eq!(preedit.caret, 1);
    assert_invariants(&preedit, "lv");
}

#[test]
fn test_build_preedit_renders_the_graph_spelling_when_the_inputs_differ() {
    // The graph owns the spelling the text is made of, so a caller that hands over a
    // graph of another input gets that graph's spelling rather than a mixture of the
    // two. What matters here is the consequence: the caret still lands on a character
    // boundary of the text and the spans still tile it, so a stale graph renders a
    // frame that is merely old instead of one that is malformed.
    let buf = buffer_with_caret_back("nihaoa", 4);
    assert_eq!(buf.caret(), 2);
    let preedit = build_preedit(&buf, &dag_for("nihao"));
    assert_eq!(preedit.text, "ni'hao");
    assert_eq!(preedit.caret, 3);
    assert_eq!(preedit.spans, spans("S0..2 |2..3 C3..3 S3..6"));
    assert_invariants(&preedit, "nihaoa");
}

#[test]
fn test_build_preedit_stays_within_the_size_bound() {
    // The longest input the decoder accepts, cut into 26 syllables: 25 separators.
    let raw = "nihao".repeat(12) + "niha";
    assert_eq!(raw.len(), MAX_RAW_LEN);
    let preedit = build_preedit(&buffer(&raw), &dag_for(&raw));
    assert_eq!(preedit.text.len(), raw.len() + 25);
    assert!(
        preedit.text.len() <= 96,
        "the header budgets 96 bytes, got {}",
        preedit.text.len()
    );
    assert_invariants(&preedit, &raw);

    // The worst case of the documented bound: 64 one-byte syllables, so every byte
    // carries a separator and the text reaches `2 * 64 - 1` bytes.
    let raw = "a".repeat(MAX_RAW_LEN);
    let preedit = build_preedit(&buffer(&raw), &dag_for(&raw));
    assert_eq!(preedit.text.len(), 2 * raw.len() - 1);
    assert_invariants(&preedit, &raw);
}

#[test]
fn test_build_preedit_into_reuses_the_buffers_of_the_caller() {
    let mut buf = buffer("zhongguo");
    let mut dag = SyllableDag::new();
    let mut preedit = empty_preedit();

    assert_eq!(dag.build(buf.raw()), Ok(()));
    build_preedit_into(&buf, &dag, &mut preedit);
    assert_eq!(preedit, build_preedit(&buf, &dag));
    let capacity = preedit.text.capacity();

    // A second, shorter build clears both buffers instead of reallocating.
    buf.clear();
    for ch in "ni".chars() {
        assert!(buf.push_char(ch).is_ok());
    }
    assert_eq!(dag.build(buf.raw()), Ok(()));
    build_preedit_into(&buf, &dag, &mut preedit);
    assert_eq!(preedit.text, "ni");
    assert_eq!(preedit.caret, 2);
    assert_eq!(preedit.spans.len(), 2);
    assert_eq!(preedit.text.capacity(), capacity);
}

#[test]
fn test_build_preedit_into_clears_a_stale_preedit() {
    // The builder owns every field of the value it writes. A session that ends and an
    // empty one that follows must leave nothing of the previous session behind, which
    // is the case a caller that keeps one preedit across keystrokes hits on the first
    // key of the next session.
    let mut preedit = build_preedit(&buffer("zhongguo"), &dag_for("zhongguo"));
    assert_eq!(preedit.text, "zhong'guo");
    assert_eq!(preedit.spans.len(), 4);

    build_preedit_into(&InputBuffer::new(), &SyllableDag::new(), &mut preedit);
    assert_eq!(preedit, empty_preedit());
    assert!(preedit.spans.is_empty(), "no span survives the clear");
    assert_eq!(preedit.caret, 0, "the caret returns to the origin");
    assert_invariants(&preedit, "");
}

/// Asserts that every prefix of `raw` counts the characters the normalizer keeps
/// of it, for the prefixes the normalizer's trailing-marker rule cannot affect.
fn assert_prefix_count(raw: &str) {
    for caret in 0..=raw.len() {
        if !raw.is_char_boundary(caret) || raw[..caret].ends_with('\'') {
            continue;
        }
        let kept = normalize(&raw[..caret]).text.chars().count();
        assert_eq!(surviving_chars(raw, caret), kept, "prefix of {raw:?}");
    }
}

#[test]
fn test_surviving_chars_matches_the_normalizer_for_a_prefix() {
    for raw in ["ni", "nihao", "ni'hao", "'ni", "ni''hao", "a''b"] {
        assert_prefix_count(raw);
    }
    for raw in ["Nihao", "lv", "LVE", "zzz", "ê"] {
        assert_prefix_count(raw);
    }
    // Longer inputs, and one carrying a character outside the alphabet.
    assert_prefix_count("zhongguo");
    assert_prefix_count("ni3hao");
    // A character outside the alphabet is dropped by both.
    assert_eq!(surviving_chars("ni hao", 4), 3);
}

#[test]
fn test_caret_helpers_count_characters_rather_than_bytes() {
    assert_eq!(char_offset("", 0), 0);
    assert_eq!(char_offset("lü", 0), 0);
    assert_eq!(char_offset("lü", 1), 1);
    assert_eq!(char_offset("lü", 2), 3);
    // Past the last character, and past the end: both answer the end of the text.
    assert_eq!(char_offset("lü", 3), 3);
    assert_eq!(char_offset("lü", 99), 3);

    // The umlaut is two bytes wide, so a caret inside it is pulled back to the
    // character boundary in front of it.
    assert_eq!(clamp_to_char_boundary("lü", 2), 1);
    assert_eq!(clamp_to_char_boundary("lü", 3), 3);
    assert_eq!(clamp_to_char_boundary("lü", 99), 3);
    assert_eq!(clamp_to_char_boundary("", 7), 0);
}

#[test]
fn test_cursor_index_is_the_count_of_spans_that_start_before_the_caret() {
    let list = spans("S0..2 |2..3 S3..6");
    assert_eq!(cursor_index(&list, 0), 0, "at the start");
    assert_eq!(cursor_index(&list, 2), 1, "before the separator");
    assert_eq!(cursor_index(&list, 3), 2, "before the last syllable");
    assert_eq!(cursor_index(&list, 6), 3, "at the end");
}

/// One random input character, weighted toward the letters a pinyin input is made
/// of; the last branch feeds the buffer a character it must reject.
fn char_strategy() -> impl Strategy<Value = char> {
    prop_oneof![
        6 => (b'a'..=b'z').prop_map(char::from),
        2 => (b'A'..=b'Z').prop_map(char::from),
        2 => Just('\''),
        1 => any::<char>(),
    ]
}

/// A random composing session: random keystrokes followed by a random mix of caret
/// moves and deletions, which is what puts the caret inside a syllable as well as
/// on a boundary.
///
/// The buffer's syllable grid is deliberately left as the edits repair it. The
/// preedit does not read the grid, and the boundaries the segmentation reports
/// address the *normalized* string, so writing them back into a buffer holding the
/// raw input would describe the wrong offsets.
fn buffer_strategy() -> impl Strategy<Value = InputBuffer> {
    let parts = (
        prop::collection::vec(char_strategy(), 0..=MAX_RAW_LEN),
        prop::collection::vec(-3i8..=3, 0..16),
        prop::collection::vec(any::<bool>(), 0..16),
    );
    parts.prop_map(|(chars, deltas, deletes)| {
        let mut buf = InputBuffer::new();
        for ch in chars {
            let _ = buf.push_char(ch);
        }
        for (index, delta) in deltas.iter().enumerate() {
            if deletes.get(index).copied().unwrap_or(false) {
                let _ = buf.backspace();
            } else {
                let _ = buf.move_caret(*delta);
            }
        }
        buf
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn test_build_preedit_preserves_the_invariants_for_any_session(buf in buffer_strategy()) {
        // A failed build is the pass-through case, so both branches are covered.
        let mut dag = SyllableDag::new();
        let _ = dag.build(buf.raw());
        let preedit = build_preedit(&buf, &dag);
        assert_invariants(&preedit, buf.raw());
    }
}
