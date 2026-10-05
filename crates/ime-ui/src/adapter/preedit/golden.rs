//! The preedit layout's golden set: twenty mixed-script rows laid out at a fixed font.
//!
//! Each row pins the exact runs the layout produces for an input mixing CJK, Latin, digits,
//! full-width punctuation, a wide ASCII run or Greek -- the scripts the estimator prices in
//! four different tiers -- against expectations worked out by hand from those tiers. A
//! change to the packing or to a tier has to argue with every row at once.
//!
//! Every row also re-checks the three promises the cut makes on its own: what is drawn fits
//! the budget with the mark paid for, at most one run carries the ellipsis and it is the
//! drawn sequence's first, and the drawn text is a suffix of the input -- the newest key is
//! the last thing the cut can take.
//!
//! The font is 10dp throughout, which makes the arithmetic readable: a narrow ASCII
//! character is 3dp, an ordinary one 5dp, a wide one or a digit 6dp, and anything outside
//! ASCII -- the ellipsis included -- is 10dp.

use ime_types::{Preedit, PreeditSpan, SpanKind};

use super::{
    ELLIPSIS, MIN_PREEDIT_WIDTH_DP, Measure, SECONDARY_STATUS_WIDTH_DP, character_em,
    layout_preedit,
};

/// The font size every row is laid out at, in logical pixels.
const FONT_SIZE: f32 = 10.0;

/// One worked example: an input, a budget, and the exact layout it must produce.
struct Golden {
    /// The spans of the input, tiling its text in order.
    pieces: &'static [(SpanKind, &'static str)],
    /// The caret's byte offset in the input.
    caret: usize,
    /// The width the header has for the preedit, in logical pixels. Values below the
    /// secondary markers' floor are laid out with the room those markers give back.
    available: f32,
    /// The runs drawn left of the caret, as their exact texts.
    before: &'static [&'static str],
    /// The runs drawn right of the caret, as their exact texts.
    after: &'static [&'static str],
    /// Whether a prefix was dropped to make the preedit fit.
    truncated: bool,
    /// Whether the caret is drawn.
    caret_visible: bool,
    /// The drawn run the head mark was baked into.
    head_cut_run: Option<usize>,
}

/// A preedit whose spans are `pieces`, in order, with a cursor span at `caret`.
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

/// The twenty rows: fits, marked cuts, unmarked cuts, boundary cuts, caret edges.
#[rustfmt::skip]
const GOLDEN: [Golden; 20] = [
    // 1. Pure CJK, room to spare: the whole reading, untouched.
    Golden {
        pieces: &[(SpanKind::Passthrough, "你好世界")],
        caret: 12, available: 200.0,
        before: &["你好世界"], after: &[],
        truncated: false, caret_visible: true, head_cut_run: None,
    },
    // 2. Pure CJK, 55dp of 60: the mark is paid first and 45dp keep the last four
    //    characters whole.
    Golden {
        pieces: &[(SpanKind::Passthrough, "你好世界你好")],
        caret: 18, available: 55.0,
        before: &["…世界你好"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 3. Latin, a CJK glyph and digits mixed, room to spare: four runs, none cut.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "ni"),
            (SpanKind::Passthrough, "好"),
            (SpanKind::Passthrough, "hao"),
            (SpanKind::Passthrough, "123"),
        ],
        caret: 11, available: 200.0,
        before: &["ni", "好", "hao", "123"], after: &[],
        truncated: false, caret_visible: true, head_cut_run: None,
    },
    // 4. The same line against 50dp: 7dp survive of `ni` -- short of the mark, so the cut
    //    goes out unmarked rather than past the budget.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "ni"),
            (SpanKind::Passthrough, "好"),
            (SpanKind::Passthrough, "hao"),
            (SpanKind::Passthrough, "123"),
        ],
        caret: 11, available: 50.0,
        before: &["i", "好", "hao", "123"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: None,
    },
    // 5. A cut that kept no character of the run it landed in: the mark rides the whole
    //    run beside the cut. `nihao` is 23dp, `12345678` is 48dp at the digits' wide
    //    0.60em, and the 11dp left of a 59dp budget pay the mark without reaching the
    //    narrowest character of `nihao`.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "nihao"),
            (SpanKind::Passthrough, "12345678"),
        ],
        caret: 13, available: 59.0,
        before: &["…12345678"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 6. A mid-run cut against 48dp: the mark plus the nine tail characters the 38dp
    //    remaining pay for -- the tail keeps `d e` because it picks up the narrow
    //    `f i j l` on the way.
    Golden {
        pieces: &[(SpanKind::Passthrough, "abcdefghijkl")],
        caret: 12, available: 48.0,
        before: &["…defghijkl"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 7. The caret sitting exactly on the cut: no run is left of it, so the mark rides the
    //    after list's first run. Digits price at the wide 0.60em, so 48dp less the mark
    //    keeps the six tail characters and the cut lands on the caret.
    Golden {
        pieces: &[(SpanKind::Passthrough, "0123456789")],
        caret: 4, available: 48.0,
        before: &[], after: &["…456789"],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 8. The caret left of the cut: the one thing that cannot be drawn is the caret, and
    //    the marked tail is drawn as the after list.
    Golden {
        pieces: &[(SpanKind::Passthrough, "0123456789")],
        caret: 1, available: 48.0,
        before: &[], after: &["…456789"],
        truncated: true, caret_visible: false, head_cut_run: Some(0),
    },
    // 9. The caret right of the cut: the fragment left of it carries the mark, the tail
    //    behind the caret stays plain.
    Golden {
        pieces: &[(SpanKind::Passthrough, "0123456789")],
        caret: 5, available: 48.0,
        before: &["…4"], after: &["56789"],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 10. Full-width punctuation against 50dp: the mark rides the comma run it opens, and
    //     the drawn line lands exactly on the budget.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "你好"),
            (SpanKind::Passthrough, "，"),
            (SpanKind::Passthrough, "世界"),
            (SpanKind::Passthrough, "！"),
        ],
        caret: 18, available: 50.0,
        before: &["…，", "世界", "！"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 11. The same line against 48dp: the 8dp remainder cannot pay for the mark, so the
    //     comma run opens the line unmarked.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "你好"),
            (SpanKind::Passthrough, "，"),
            (SpanKind::Passthrough, "世界"),
            (SpanKind::Passthrough, "！"),
        ],
        caret: 18, available: 48.0,
        before: &["，", "世界", "！"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: None,
    },
    // 12. Greek prices at the same one em as CJK: ten characters against 55dp keep the
    //     last four, mark first.
    Golden {
        pieces: &[(SpanKind::Passthrough, "αβγδεαβγδε")],
        caret: 20, available: 55.0,
        before: &["…βγδε"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 13. Wide ASCII against 48dp: `W @ %` characters price at six tenths of an em, so
    //     the 23dp the mark leaves keep the last three of them where a half-em table
    //     would have kept five.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "W@%W@%"),
            (SpanKind::Passthrough, "123"),
        ],
        caret: 9, available: 48.0,
        before: &["…W@%", "123"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 14. Narrow ASCII against 48dp: twenty `i` at three tenths of an em keep twelve of
    //     their own where ordinary letters would keep seven.
    Golden {
        pieces: &[(SpanKind::Passthrough, "iiiiiiiiiiiiiiiiiiii")],
        caret: 20, available: 48.0,
        before: &["…iiiiiiiiiiii"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 15. Syllables and narrow separators against 60dp: the cut lands inside a syllable,
    //     the 6dp remainder keeps its last character, and the mark is one dp short of
    //     affordable -- the fragment opens the line unmarked.
    Golden {
        pieces: &[
            (SpanKind::Syllable, "hao"),
            (SpanKind::Separator, "'"),
            (SpanKind::Syllable, "hao"),
            (SpanKind::Separator, "'"),
            (SpanKind::Syllable, "hao"),
            (SpanKind::Separator, "'"),
            (SpanKind::Syllable, "hao"),
            (SpanKind::Separator, "'"),
            (SpanKind::Syllable, "hao"),
        ],
        caret: 19, available: 60.0,
        before: &["o", "'", "hao", "'", "hao", "'", "hao"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: None,
    },
    // 16. A reading with a full-width comma, room to spare: the kinds survive the fit.
    Golden {
        pieces: &[
            (SpanKind::Syllable, "ni"),
            (SpanKind::Separator, "'"),
            (SpanKind::Passthrough, "，"),
            (SpanKind::Syllable, "hao"),
        ],
        caret: 9, available: 48.0,
        before: &["ni", "'", "，", "hao"], after: &[],
        truncated: false, caret_visible: true, head_cut_run: None,
    },
    // 17. A narrow header drops the two secondary markers and gives their room back: 20dp
    //     of own room means a 60dp budget, and the whole reading fits.
    Golden {
        pieces: &[(SpanKind::Passthrough, "你好世界")],
        caret: 12, available: 20.0,
        before: &["你好世界"], after: &[],
        truncated: false, caret_visible: true, head_cut_run: None,
    },
    // 18. The same narrow header with a line too long even for the returned room: the mark
    //     and five characters land exactly on the 60dp budget.
    Golden {
        pieces: &[(SpanKind::Passthrough, "你好世界你好世界")],
        caret: 24, available: 20.0,
        before: &["…界你好世界"], after: &[],
        truncated: true, caret_visible: true, head_cut_run: Some(0),
    },
    // 19. Digits beside a CJK glyph, room to spare: the wide tier does not cut a line that
    //     fits.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "v2"),
            (SpanKind::Passthrough, "日"),
            (SpanKind::Passthrough, "v3"),
        ],
        caret: 7, available: 48.0,
        before: &["v2", "日", "v3"], after: &[],
        truncated: false, caret_visible: true, head_cut_run: None,
    },
    // 20. A cut that swallows the caret and keeps one character: the drawn line opens
    //     with that character alone, marked, and the kept tail follows it.
    Golden {
        pieces: &[
            (SpanKind::Passthrough, "012345678"),
            (SpanKind::Passthrough, "12345"),
        ],
        caret: 4, available: 48.0,
        before: &[], after: &["…8", "12345"],
        truncated: true, caret_visible: false, head_cut_run: Some(0),
    },
];

#[test]
fn test_layout_preedit_golden_set_draws_the_expected_mixed_script_runs() {
    for (index, row) in GOLDEN.iter().enumerate() {
        let preedit = preedit_of(row.pieces, row.caret);
        let layout = layout_preedit(&preedit, row.available, FONT_SIZE, &mut Measure::default());

        let before: Vec<&str> = layout.before.iter().map(|run| run.text.as_str()).collect();
        let after: Vec<&str> = layout.after.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(
            before,
            row.before,
            "row {}: the runs left of the caret",
            index + 1
        );
        assert_eq!(
            after,
            row.after,
            "row {}: the runs right of the caret",
            index + 1
        );
        assert_eq!(layout.truncated, row.truncated, "row {}", index + 1);
        assert_eq!(layout.caret_visible, row.caret_visible, "row {}", index + 1);
        assert_eq!(layout.head_cut_run, row.head_cut_run, "row {}", index + 1);

        let input: String = row.pieces.iter().map(|(_, piece)| *piece).collect();
        let runs = layout.before.iter().chain(layout.after.iter());
        let drawn: String = runs.clone().map(|run| run.text.as_str()).collect();

        let marks = drawn.matches(ELLIPSIS).count();
        assert!(
            marks <= 1,
            "row {}: at most one ellipsis on the whole line, got {marks}",
            index + 1
        );
        assert_eq!(
            layout.head_cut_run.is_some(),
            marks == 1,
            "row {}: the record and the drawn mark agree",
            index + 1
        );
        if let Some(position) = layout.head_cut_run {
            assert_eq!(
                position,
                0,
                "row {}: the mark rides the first drawn run",
                index + 1
            );
        }

        let width: f32 = runs
            .map(|run| {
                run.text
                    .chars()
                    .map(|c| character_em(c) * FONT_SIZE)
                    .sum::<f32>()
            })
            .sum();
        // A narrow header hands the secondary markers' room back, so the budget the
        // line is cut to is the returned one, not the raw width the caller gave.
        let budget = if row.available >= MIN_PREEDIT_WIDTH_DP {
            row.available
        } else {
            row.available + SECONDARY_STATUS_WIDTH_DP
        };
        assert!(
            width <= budget + 1.0e-3,
            "row {}: drew {width}dp against a {budget}dp budget",
            index + 1
        );
        let body = drawn.strip_prefix(ELLIPSIS).unwrap_or(&drawn);
        assert!(
            input.ends_with(body),
            "row {}: drew {drawn:?}, which is not the tail of {input:?}",
            index + 1
        );
    }
}
