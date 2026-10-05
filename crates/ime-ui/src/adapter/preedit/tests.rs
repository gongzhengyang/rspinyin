//! Unit tests for the preedit's layout: the cut, the caret split and the kinds.
//!
//! Pure functions of a `Preedit`, so they need no component, no platform and no fixtures.
//! The widths they assert on are the estimator's own -- the four tiers of
//! [`crate::adapter::cell::character_em`] multiplied by the font size the case draws at --
//! which is what makes the expected cut arithmetic rather than a measurement.

use ime_types::{Preedit, PreeditSpan, SpanKind};

use crate::layout;

use super::{
    MIN_PREEDIT_WIDTH_DP, PREEDIT_MAX_CHARS, PreeditLayout, RunKind, SECONDARY_STATUS_WIDTH_DP,
    layout_preedit,
};

/// The width one character occupies at `font_size`, per the estimator's tiers.
fn character_width(character: char, font_size: f32) -> f32 {
    super::character_em(character) * font_size
}

/// The estimator's width of a text at `font_size`.
fn text_width(text: &str, font_size: f32) -> f32 {
    text.chars()
        .map(|character| character_width(character, font_size))
        .sum()
}

/// The width the head ellipsis occupies at `font_size`.
fn marker_width(font_size: f32) -> f32 {
    super::ellipsis_width(font_size)
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
        .map(|run| text_width(&run.text, font_size))
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
    assert!(
        layout.after.is_empty(),
        "nothing follows a caret at the end"
    );
    assert!(layout.caret_visible);
    assert!(
        !layout.truncated,
        "the reading fits the budget it was given"
    );
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

    // The tail is what survives, whole runs first: four syllables of 21dp each plus the
    // three separators between them -- separators are narrow ASCII -- are 96.6dp, and the
    // 3.4dp of remainder neither pays for another syllable nor for the mark, so the cut
    // stays where the last whole run begins and goes out unmarked.
    assert_eq!(
        texts(&layout.before),
        ["hao", "'", "hao", "'", "hao", "'", "hao"]
    );
    assert!(layout.truncated, "a dropped prefix is reported");
    assert_eq!(
        layout.head_cut_run, None,
        "a remainder that cannot pay for the mark marks nothing"
    );
    assert!(
        runs_width(&layout.before, font_size) <= budget,
        "what is drawn fits the budget it was given"
    );
    assert!(
        reading.ends_with("hao'hao'hao'hao"),
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
    // preedit is laid out with the 40dp they were occupying. `ni'hao` is 42dp, so it fits
    // only because that room came back.
    let narrow = layout_preedit(&typed("ni'hao"), 40.0, 14.0, &mut measure);
    assert!(!narrow.show_secondary_status);
    assert!(
        !narrow.truncated,
        "the room the dropped markers freed is what the preedit is laid out with"
    );

    // The threshold itself is inclusive: at exactly the minimum the markers stay.
    let at_minimum = layout_preedit(&typed("ni'hao"), MIN_PREEDIT_WIDTH_DP, 14.0, &mut measure);
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
    assert_eq!(layout.head_cut_run, None, "nothing drawn, nothing marked");
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
    let preedit = preedit_of(&[(SpanKind::Passthrough, "你好世界你好")], 24);
    let layout = layout_preedit(&preedit, 30.0, font_size, &mut super::Measure::default());

    // A line this narrow is below the markers' minimum, so the markers give their room
    // back and the preedit's budget is the line plus `SECONDARY_STATUS_WIDTH_DP`: 70dp.
    // The mark is paid for first (14dp), and the 56dp it leaves keep exactly the last four
    // whole characters -- never half of one.
    assert_eq!(texts(&layout.before), ["…世界你好"]);
    assert!(layout.truncated);
    assert_eq!(
        layout.head_cut_run,
        Some(0),
        "the mark rides the first drawn run"
    );
    assert_eq!(layout.before[0].kind, RunKind::Passthrough);
    assert_eq!(layout.before[0].kind.code(), 2);
    assert!(
        runs_width(&layout.before, font_size) <= 30.0 + SECONDARY_STATUS_WIDTH_DP,
        "the mark was paid for before the characters were placed"
    );
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
    assert!(
        layout.before[0].text.starts_with(super::ELLIPSIS),
        "a ceiling cut is a head cut, and it is marked like one"
    );
    let body = &layout.before[0].text[super::ELLIPSIS.len_utf8()..];
    assert_eq!(
        body.chars().count(),
        ceiling,
        "the drawn run is bounded by the character ceiling"
    );
    assert_eq!(layout.head_cut_run, Some(0));
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
    assert_eq!(
        RunKind::of(SpanKind::Passthrough),
        Some(RunKind::Passthrough)
    );
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
    // Dropping the markers folds their two icons and the two gaps that separated them
    // from what stays, so the cluster's fixed width loses that much and the strip keeps
    // one gap of slack between it and the preedit.
    assert_eq!(
        full - SECONDARY_STATUS_WIDTH_DP,
        2.0 * metrics.header_icon_size + 2.0 * metrics.header_icon_gap,
        "and it is exactly what the cluster's fixed width loses when they are dropped"
    );
    // A relation between two constants is a compile-time invariant, which is what the
    // const block asserts; a runtime `assert!` on it is the one clippy calls a constant
    // assertion, and it would also run once per test process for no information.
    const {
        assert!(
            MIN_PREEDIT_WIDTH_DP >= SECONDARY_STATUS_WIDTH_DP,
            "the preedit is never given less room than the markers it displaces"
        );
    }
}

#[test]
fn test_layout_preedit_bakes_the_head_ellipsis_into_the_leftmost_drawn_run() {
    let font_size = 10.0;
    // `abcdefghijkl` is 52dp against a 48dp budget -- `f i j l` price at the narrow
    // 0.30em, the rest at 0.50em -- so the mark is paid for first and the 38dp it
    // leaves keep the last nine characters (37dp, the tail's own narrow letters
    // included). The mark is baked into the run's own text, so a redrawn frame writes
    // nothing at all.
    let preedit = preedit_of(&[(SpanKind::Passthrough, "abcdefghijkl")], 12);
    let mut layout = PreeditLayout::default();
    let mut measure = super::Measure::default();

    assert!(layout.update(&preedit, 48.0, font_size, &mut measure));
    assert_eq!(texts(&layout.before), ["…defghijkl"]);
    assert!(layout.after.is_empty());
    assert!(layout.truncated);
    assert_eq!(layout.head_cut_run, Some(0));
    assert!(
        runs_width(&layout.before, font_size) <= 48.0,
        "the marked line fits the budget it was cut to"
    );
    assert!(
        !layout.update(&preedit, 48.0, font_size, &mut measure),
        "a redrawn marked line is change-detected like any other"
    );
}

#[test]
fn test_layout_preedit_marks_a_whole_run_when_the_cut_lands_on_a_boundary() {
    let font_size = 10.0;
    // `nihao` (23dp) then `12345678` (48dp: digits price at the wide 0.60em) against
    // 59dp: `12345678` is kept whole and the 11dp remainder pays for the mark but not
    // for a character of `nihao` -- three a narrow character -- so the mark rides the
    // whole run beside the cut instead.
    let preedit = preedit_of(
        &[
            (SpanKind::Passthrough, "nihao"),
            (SpanKind::Passthrough, "12345678"),
        ],
        13,
    );
    let layout = layout_preedit(&preedit, 59.0, font_size, &mut super::Measure::default());

    assert_eq!(texts(&layout.before), ["…12345678"]);
    assert!(layout.after.is_empty());
    assert!(layout.truncated);
    assert_eq!(layout.head_cut_run, Some(0));
    assert!(runs_width(&layout.before, font_size) <= 59.0);
}

#[test]
fn test_layout_preedit_leaves_the_cut_unmarked_when_the_remainder_cannot_pay_for_the_mark() {
    let font_size = 10.0;
    // `ni` (8dp) `好` (10) `hao` (15) `123` (18) against 50dp: the last three runs are kept
    // whole and 7dp of `ni` survive, but 7dp is short of the mark's em. Drawing the mark
    // anyway would push the line past its budget, onto the newest input, so the cut goes
    // out unmarked.
    let preedit = preedit_of(
        &[
            (SpanKind::Passthrough, "ni"),
            (SpanKind::Passthrough, "好"),
            (SpanKind::Passthrough, "hao"),
            (SpanKind::Passthrough, "123"),
        ],
        11,
    );
    let layout = layout_preedit(&preedit, 50.0, font_size, &mut super::Measure::default());

    assert_eq!(texts(&layout.before), ["i", "好", "hao", "123"]);
    assert!(layout.after.is_empty());
    assert!(layout.truncated);
    assert_eq!(layout.head_cut_run, None);
    assert!(
        !layout
            .before
            .iter()
            .any(|run| run.text.contains(super::ELLIPSIS)),
        "no run carries a mark the budget did not pay for"
    );
    assert!(runs_width(&layout.before, font_size) <= 50.0);
}

#[test]
fn test_layout_preedit_marks_the_after_list_when_the_caret_sits_on_the_cut() {
    let font_size = 10.0;
    // `0123456789` is 60dp (digits at the wide 0.60em) against 48dp: the mark is paid
    // first and the 38dp it leaves keep the six tail characters. The caret sits exactly
    // on the cut, so no run is left of it and the mark rides the after list's first run
    // instead.
    let preedit = preedit_of(&[(SpanKind::Passthrough, "0123456789")], 4);
    let layout = layout_preedit(&preedit, 48.0, font_size, &mut super::Measure::default());

    assert!(
        layout.before.is_empty(),
        "the cut starts where the caret is"
    );
    assert_eq!(texts(&layout.after), ["…456789"]);
    assert!(
        layout.caret_visible,
        "a caret on the cut is still on screen"
    );
    assert_eq!(layout.head_cut_run, Some(0));
}

#[test]
fn test_layout_preedit_marks_the_surviving_tail_when_the_cut_drops_the_caret() {
    let font_size = 10.0;
    // The same cut as a caret on it, only with the caret left of the cut: the caret is the
    // one thing that cannot be drawn, and the marked tail is drawn as the after list.
    let preedit = preedit_of(&[(SpanKind::Passthrough, "0123456789")], 1);
    let layout = layout_preedit(&preedit, 48.0, font_size, &mut super::Measure::default());

    assert!(!layout.caret_visible);
    assert!(layout.before.is_empty());
    assert_eq!(texts(&layout.after), ["…456789"]);
    assert_eq!(layout.head_cut_run, Some(0));
}

#[test]
fn test_layout_preedit_keeps_the_cut_run_kind_when_the_mark_is_baked() {
    let font_size = 10.0;
    // `nihao` (23dp: `i` is narrow) then `12345678` (48dp: digits price at the wide
    // 0.60em) against 65dp: the passthrough is kept whole, the mark is paid, and the
    // 7dp that remain keep exactly the syllable's last character. The fragment is
    // still a syllable run -- the mark changes what the run opens with, not what it is.
    let preedit = preedit_of(
        &[
            (SpanKind::Syllable, "nihao"),
            (SpanKind::Passthrough, "12345678"),
        ],
        13,
    );
    let layout = layout_preedit(&preedit, 65.0, font_size, &mut super::Measure::default());

    assert_eq!(texts(&layout.before), ["…o", "12345678"]);
    assert_eq!(
        layout.before[0].kind,
        RunKind::Syllable,
        "the cut run keeps the kind it was cut from"
    );
    assert_eq!(layout.head_cut_run, Some(0));
    assert!(runs_width(&layout.before, font_size) <= 65.0);
}

#[test]
fn test_layout_preedit_drawn_tail_always_matches_the_input_tail() {
    // One text of every script class the estimator prices, from a fit to a deep cut: what
    // is drawn must stay a suffix of the input wherever the caret is on screen.
    let cases = [
        ("ni'hao", 6),
        ("你好世界", 12),
        ("ni好hao世界123", 17),
        ("W@%123", 6),
        ("你好，世界！", 18),
    ];
    for (text, caret) in cases {
        let preedit = preedit_of(&[(SpanKind::Passthrough, text)], caret);
        let mut budget = 48.0;
        while budget <= 60.0 {
            let layout = layout_preedit(&preedit, budget, 10.0, &mut super::Measure::default());
            if layout.caret_visible {
                let drawn: String = layout
                    .before
                    .iter()
                    .chain(layout.after.iter())
                    .map(|run| run.text.as_str())
                    .collect();
                let body = drawn.strip_prefix(super::ELLIPSIS).unwrap_or(&drawn);
                assert!(
                    text.ends_with(body),
                    "{text:?} at {budget}dp drew {drawn:?}, which is not its tail"
                );
            }
            budget += 1.0;
        }
    }
}

#[test]
fn test_layout_preedit_budget_sweep_shrinks_the_line_continuously_and_keeps_the_tail() {
    let font_size = 10.0;
    // CJK, a narrow separator, ordinary and wide ASCII in one line of 82dp; the caret
    // stays at the end, so the tail promise holds at every step of the sweep.
    let input = "ni'haoW@%123世界";
    let preedit = preedit_of(
        &[
            (SpanKind::Syllable, "ni"),
            (SpanKind::Separator, "'"),
            (SpanKind::Syllable, "hao"),
            (SpanKind::Passthrough, "W@%123"),
            (SpanKind::Passthrough, "世界"),
        ],
        18,
    );

    let mut previous_width = 0.0;
    let mut budget = 48.0;
    while budget <= 90.0 {
        let layout = layout_preedit(&preedit, budget, font_size, &mut super::Measure::default());
        let runs: Vec<&super::PreeditRun> =
            layout.before.iter().chain(layout.after.iter()).collect();
        let width: f32 = runs
            .iter()
            .map(|run| text_width(&run.text, font_size))
            .sum();

        assert!(
            width <= budget + 1.0e-3,
            "{budget}dp: the drawn line is {width}dp wide, past the room it was given"
        );
        assert!(
            width > budget - 10.0 - 1.0e-3,
            "{budget}dp: the packing fills the budget to within one character, drew {width}dp"
        );
        assert!(
            width + 1.0e-3 >= previous_width,
            "{budget}dp: more room never draws less -- {width}dp after {previous_width}dp"
        );

        let drawn: String = runs.iter().map(|run| run.text.as_str()).collect();
        let marks = drawn.matches(super::ELLIPSIS).count();
        assert!(
            marks <= 1,
            "{budget}dp: at most one mark on the line, got {marks}"
        );
        assert_eq!(
            layout.head_cut_run.is_some(),
            marks == 1,
            "{budget}dp: the record and the drawn mark agree"
        );
        if let Some(index) = layout.head_cut_run {
            assert_eq!(index, 0, "{budget}dp: the mark rides the first drawn run");
        }
        let body = drawn.strip_prefix(super::ELLIPSIS).unwrap_or(&drawn);
        assert!(
            input.ends_with(body),
            "{budget}dp: drew {drawn:?}, which is not the tail of {input:?}"
        );

        previous_width = width;
        budget += 1.0;
    }
}

#[test]
fn test_kept_start_leaves_the_cut_unmarked_when_the_budget_cannot_pay_for_the_mark() {
    // One run `abc` of 15dp against a 9dp budget: even with nothing kept beside it, the
    // mark's 10dp do not fit, so the walk keeps what the 9dp buy and reports no mark.
    let preedit = preedit_of(&[(SpanKind::Passthrough, "abc")], 3);
    let kept = super::kept_start(
        &preedit.text,
        &preedit.spans,
        9.0,
        10.0,
        &mut super::Measure::default(),
    );

    assert_eq!(kept.start, 2, "one tail character of 5dp fits, two do not");
    assert!(!kept.marked);
}

#[test]
fn test_kept_start_marks_the_ceiling_cut_when_the_room_pays_for_it() {
    // A budget wide enough for everything, so the character ceiling is the only cut -- and
    // it is marked like any other head cut.
    let text: String = "a".repeat(PREEDIT_MAX_CHARS + 6);
    let preedit = preedit_of(&[(SpanKind::Passthrough, text.as_str())], text.len());
    let kept = super::kept_start(
        &text,
        &preedit.spans,
        10_000.0,
        10.0,
        &mut super::Measure::default(),
    );

    assert_eq!(
        kept.start,
        super::truncation_start(&text, PREEDIT_MAX_CHARS)
    );
    assert!(kept.marked, "10_000dp of room pays for a 10dp mark");
}

#[test]
fn test_kept_start_deepens_the_ceiling_cut_when_the_mark_is_short_of_room() {
    // 64 kept characters of 5dp are 320dp against a budget of 326dp: the 6dp of slack are
    // short of the mark, so the walk gives a character back and marks the deeper cut
    // instead of drawing the marked line past its budget.
    let text: String = "a".repeat(PREEDIT_MAX_CHARS + 6);
    let preedit = preedit_of(&[(SpanKind::Passthrough, text.as_str())], text.len());
    let kept = super::kept_start(
        &text,
        &preedit.spans,
        326.0,
        10.0,
        &mut super::Measure::default(),
    );

    assert_eq!(
        kept.start,
        super::truncation_start(&text, PREEDIT_MAX_CHARS) + 1
    );
    assert!(kept.marked);

    let layout = layout_preedit(&preedit, 326.0, 10.0, &mut super::Measure::default());
    assert_eq!(layout.head_cut_run, Some(0));
    assert!(runs_width(&layout.before, 10.0) <= 326.0);
}
