//! Unit tests for the preedit's layout: the cut, the caret split and the kinds.
//!
//! Pure functions of a `Preedit`, so they need no component, no platform and no fixtures.
//! The widths they assert on are the estimator's own -- one em per character outside ASCII
//! and half an em per ASCII one -- multiplied by the font size the case draws at, which is
//! what makes the expected cut arithmetic rather than a measurement.

use ime_types::{Preedit, PreeditSpan, SpanKind};

use crate::layout;

use super::{
    MIN_PREEDIT_WIDTH_DP, PREEDIT_MAX_CHARS, PreeditLayout, RunKind, SECONDARY_STATUS_WIDTH_DP,
    layout_preedit,
};

/// The width one ASCII character occupies at `font_size`, per the estimator.
fn ascii(font_size: f32) -> f32 {
    0.5 * font_size
}

/// The width one character outside ASCII occupies at `font_size`.
fn wide(font_size: f32) -> f32 {
    font_size
}

/// A preedit whose spans are `pieces`, in order, with a caret span at `caret`.
///
/// The spans tile the text exactly and the caret span is inserted in its sorted position,
/// which is what `ime_types::Preedit` guarantees and what the layout is written against.
fn preedit_of(pieces: &[(SpanKind, &str)], caret: usize) -> Preedit {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (kind, piece) in pieces {
        let start = text.len();
        text.push_str(piece);
        spans.push(PreeditSpan {
            start: start as u16,
            end: text.len() as u16,
            kind: *kind,
        });
    }
    let index = spans.partition_point(|span| usize::from(span.start) < caret);
    spans.insert(
        index,
        PreeditSpan {
            start: caret as u16,
            end: caret as u16,
            kind: SpanKind::Cursor,
        },
    );
    Preedit {
        text,
        caret: caret as u32,
        spans,
    }
}

/// The preedit of a reading typed to the end: syllables joined by separators, caret last.
fn typed(reading: &str) -> Preedit {
    let mut pieces: Vec<(SpanKind, &str)> = Vec::new();
    for (index, part) in reading.split('\'').enumerate() {
        if index > 0 {
            pieces.push((SpanKind::Separator, "'"));
        }
        pieces.push((SpanKind::Syllable, part));
    }
    preedit_of(&pieces, reading.len())
}

/// A reading of `syllables` three-letter syllables, which is long enough to be cut.
fn long_reading(syllables: usize) -> String {
    let mut reading = String::new();
    for index in 0..syllables {
        if index > 0 {
            reading.push('\'');
        }
        reading.push_str("hao");
    }
    reading
}

/// The text of every run of a list, in order.
fn texts(runs: &[super::PreeditRun]) -> Vec<&str> {
    runs.iter().map(|run| run.text.as_str()).collect()
}

/// The estimator's width of a run list at `font_size`.
fn runs_width(runs: &[super::PreeditRun], font_size: f32) -> f32 {
    runs.iter()
        .map(|run| {
            run.text
                .chars()
                .map(|character| {
                    if character.is_ascii() {
                        ascii(font_size)
                    } else {
                        wide(font_size)
                    }
                })
                .sum::<f32>()
        })
        .sum()
}

#[test]
fn test_layout_preedit_splits_the_runs_around_a_caret_at_the_end() {
    let layout = layout_preedit(
        &typed("ni'hao"),
        200.0,
        14.0,
        &mut super::Measure::default(),
    );

    assert_eq!(texts(&layout.before), ["ni", "'", "hao"]);
    assert!(layout.after.is_empty(), "nothing follows a caret at the end");
    assert!(layout.caret_visible);
    assert!(!layout.truncated, "the reading fits the budget it was given");
    assert!(layout.show_secondary_status);
    assert_eq!(layout.before[1].kind, RunKind::Separator);
    assert_eq!(layout.before[1].kind.code(), 1);
}

#[test]
fn test_layout_preedit_splits_the_runs_around_a_caret_in_the_middle() {
    let mut preedit = typed("ni'hao");
    preedit.caret = 3;

    let layout = layout_preedit(&preedit, 200.0, 14.0, &mut super::Measure::default());

    assert_eq!(
        texts(&layout.before),
        ["ni", "'"],
        "everything up to the caret is drawn before it"
    );
    assert_eq!(
        texts(&layout.after),
        ["hao"],
        "and everything after it is drawn behind it"
    );
    assert!(layout.caret_visible);
}

#[test]
fn test_layout_preedit_splits_the_runs_around_a_caret_at_the_start() {
    let mut preedit = typed("ni'hao");
    preedit.caret = 0;

    let layout = layout_preedit(&preedit, 200.0, 14.0, &mut super::Measure::default());

    assert!(layout.before.is_empty(), "nothing precedes a leading caret");
    assert_eq!(texts(&layout.after), ["ni", "'", "hao"]);
    assert!(layout.caret_visible, "a caret that fits is drawn");
}

#[test]
fn test_layout_preedit_cuts_the_head_and_keeps_the_newest_input() {
    let font_size = 14.0;
    let budget = 100.0;
    let reading = long_reading(40);
    let layout = layout_preedit(
        &typed(&reading),
        budget,
        font_size,
        &mut super::Measure::default(),
    );

    // The tail is what survives: 3 whole syllables of 21dp each plus the separators between
    // them are 84dp, and the remainder of 16dp pays for two of the three characters of the
    // syllable the cut lands in.
    assert_eq!(texts(&layout.before), ["ao", "'", "hao", "'", "hao", "'", "hao"]);
    assert!(layout.truncated, "a dropped prefix is reported");
    assert!(
        runs_width(&layout.before, font_size) <= budget,
        "what is drawn fits the budget it was given"
    );
    assert!(
        reading.ends_with("hao'hao'hao"),
        "the newest input is the part that survives"
    );
}

#[test]
fn test_layout_preedit_hides_a_caret_the_cut_dropped() {
    let mut preedit = typed(&long_reading(40));
    preedit.caret = 0;

    let layout = layout_preedit(&preedit, 100.0, 14.0, &mut super::Measure::default());

    assert!(layout.truncated);
    assert!(
        !layout.caret_visible,
        "a caret left of everything that survived is not on screen"
    );
    assert!(
        layout.before.is_empty(),
        "and the runs it precedes are drawn as the after list"
    );
    assert!(!layout.after.is_empty());
}

#[test]
fn test_layout_preedit_gives_the_secondary_markers_room_back_to_a_narrow_preedit() {
    let mut measure = super::Measure::default();
    // One dp under the threshold: the cluster drops its two secondary markers and the
    // preedit is laid out with the 48dp they were occupying. `ni'hao` is 42dp, so it fits
    // only because that room came back.
    let narrow = layout_preedit(&typed("ni'hao"), 40.0, 14.0, &mut measure);
    assert!(!narrow.show_secondary_status);
    assert!(
        !narrow.truncated,
        "the room the dropped markers freed is what the preedit is laid out with"
    );

    // The threshold itself is inclusive: at exactly the minimum the markers stay.
    let at_minimum = layout_preedit(
        &typed("ni'hao"),
        MIN_PREEDIT_WIDTH_DP,
        14.0,
        &mut measure,
    );
    assert!(at_minimum.show_secondary_status);
    assert_eq!(texts(&at_minimum.before), ["ni", "'", "hao"]);
}

#[test]
fn test_layout_preedit_of_an_empty_preedit_draws_nothing() {
    let layout = layout_preedit(
        &Preedit {
            text: String::new(),
            caret: 0,
            spans: Vec::new(),
        },
        200.0,
        14.0,
        &mut super::Measure::default(),
    );

    assert!(layout.before.is_empty());
    assert!(layout.after.is_empty());
    assert!(
        !layout.caret_visible,
        "there is no composing session to put a caret in"
    );
    assert!(!layout.truncated);
}

#[test]
fn test_layout_preedit_draws_a_text_with_no_spans_as_one_passthrough_run() {
    // The builder tiles every non-empty text, so this is a frame the contract cannot
    // produce; the header has always shown the text it is handed, and the fallback keeps
    // that rather than turning a malformed frame into what looks like an empty session.
    let preedit = Preedit {
        text: String::from("ni'hao"),
        caret: 6,
        spans: Vec::new(),
    };
    let layout = layout_preedit(&preedit, 200.0, 14.0, &mut super::Measure::default());

    assert_eq!(texts(&layout.before), ["ni'hao"]);
    assert_eq!(layout.before[0].kind, RunKind::Passthrough);
    assert!(layout.caret_visible);
    assert!(!layout.truncated);
}

#[test]
fn test_layout_preedit_writes_into_the_buffers_it_already_holds() {
    let mut layout = PreeditLayout::default();
    let mut measure = super::Measure::default();
    let preedit = typed("ni'hao");

    assert!(layout.update(&preedit, 200.0, 14.0, &mut measure));
    let address = layout.before[0].text.as_ptr();
    assert!(
        !layout.update(&preedit, 200.0, 14.0, &mut measure),
        "a preedit that draws what the layout already holds reports nothing"
    );
    assert_eq!(
        layout.before[0].text.as_ptr(),
        address,
        "and a redrawn run reuses the buffer rather than allocating a new string"
    );

    // A shorter preedit keeps the capacity it grew into and drops the runs it no longer
    // draws, which is what makes a keystroke that deletes a syllable free.
    assert!(layout.update(&typed("ni"), 200.0, 14.0, &mut measure));
    assert_eq!(texts(&layout.before), ["ni"]);
    assert!(layout.after.is_empty());
}

#[test]
fn test_layout_preedit_cuts_on_a_character_boundary() {
    let font_size = 14.0;
    let preedit = preedit_of(&[(SpanKind::Passthrough, "你好世界")], 12);
    let layout = layout_preedit(&preedit, 30.0, font_size, &mut super::Measure::default());

    // Two full-width characters are 28dp and three are 42dp, so the budget of 30dp keeps
    // exactly the last two -- whole characters, never half of one.
    assert_eq!(texts(&layout.before), ["世界"]);
    assert!(layout.truncated);
    assert_eq!(layout.before[0].kind, RunKind::Passthrough);
    assert_eq!(layout.before[0].kind.code(), 2);
}

#[test]
fn test_layout_preedit_bounds_the_drawn_text_by_the_character_ceiling() {
    let ceiling = PREEDIT_MAX_CHARS;
    let text: String = "a".repeat(ceiling + 6);
    let preedit = preedit_of(&[(SpanKind::Passthrough, text.as_str())], text.len());

    // A budget wide enough for every character, so the ceiling is the only thing that cuts.
    let layout = layout_preedit(&preedit, 10_000.0, 14.0, &mut super::Measure::default());

    assert!(layout.truncated);
    assert_eq!(layout.before.len(), 1);
    assert_eq!(
        layout.before[0].text.chars().count(),
        ceiling,
        "the drawn run is bounded by the character ceiling"
    );
}

#[test]
fn test_truncation_start_stays_on_a_character_boundary() {
    let text = "拼音ni'hao";
    for max_chars in 0..=text.chars().count() + 2 {
        let start = super::truncation_start(text, max_chars);
        assert!(text.is_char_boundary(start), "max_chars {max_chars}");
    }
    assert_eq!(
        super::truncation_start(text, 0),
        text.len(),
        "a ceiling of nothing keeps nothing"
    );
    assert_eq!(
        super::truncation_start(text, 2),
        text.len() - 2,
        "the last two characters, which are one byte each"
    );
    assert_eq!(
        super::truncation_start(text, 100),
        0,
        "a text inside the ceiling is kept whole"
    );
}

#[test]
fn test_run_kind_maps_the_contract_and_rejects_the_cursor() {
    assert_eq!(RunKind::of(SpanKind::Syllable), Some(RunKind::Syllable));
    assert_eq!(RunKind::of(SpanKind::Separator), Some(RunKind::Separator));
    assert_eq!(RunKind::of(SpanKind::Passthrough), Some(RunKind::Passthrough));
    assert_eq!(
        RunKind::of(SpanKind::Cursor),
        None,
        "the cursor is a marker, not a run"
    );
    assert_eq!(RunKind::Syllable.code(), 0);
    assert_eq!(RunKind::Separator.code(), 1);
    assert_eq!(RunKind::Passthrough.code(), 2);
}

#[test]
fn test_secondary_status_width_matches_the_cluster_the_component_draws() {
    let metrics = layout::metrics().expect("ui/candidate.slint declares its constants");
    let secondary = 2.0 * metrics.header_icon_size + metrics.header_icon_gap;
    let full = 4.0 * metrics.header_icon_size + 3.0 * metrics.header_icon_gap;

    assert_eq!(
        SECONDARY_STATUS_WIDTH_DP, secondary,
        "the room the preedit gains is the two markers' own width (3.1.1)"
    );
    assert_eq!(
        full - SECONDARY_STATUS_WIDTH_DP,
        secondary,
        "and it is exactly what the cluster's fixed width loses when they are dropped"
    );
    assert!(
        MIN_PREEDIT_WIDTH_DP <= SECONDARY_STATUS_WIDTH_DP,
        "the preedit is never given less room than the markers it displaces"
    );
}
