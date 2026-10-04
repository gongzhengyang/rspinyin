//! Wire-fidelity tests: every field the writer builds, read back from a captured sink.

use super::*;
use ime_types::{LayoutHint, OverlaySection, PageState, Placement, Preedit, PreeditSpan, RectI, StatusStrip};

/// What one sink call captured, copied out of the borrowed wire before returning.
#[derive(Default)]
struct Captured {
    kind: u32,
    revision: u32,
    preedit: String,
    caret: u32,
    span_kinds: Vec<u32>,
    candidate_texts: Vec<String>,
    candidate_sources: Vec<u32>,
    page: [u8; 3],
    mode_label: String,
    flags: u32,
    script: u32,
    cursor: [i32; 4],
    scale: f32,
    placement: u32,
    layout: [u32; 3],
    theme: [u64; 6],
    hide_reason: u32,
    highlight: u16,
    has_highlight: u8,
    overlay_kind: u32,
    overlay_sections: Vec<(String, Vec<(String, String)>)>,
    calls: u32,
}

/// SAFETY: `ctx` points at the `Captured` the active test owns; the wire is valid
/// for this call only, and everything kept is copied here.
extern "C" fn capture_frame(ctx: *mut c_void, wire: *const RspinyinFrameWire) {
    // SAFETY: the test owns the context outliving this call, and the engine passes
    // a wire valid for its duration.
    let wire = unsafe { &*wire };
    let captured = unsafe { &mut *(ctx.cast::<Captured>()) };
    captured.calls += 1;
    captured.kind = wire.kind;
    captured.revision = wire.revision;
    captured.caret = wire.caret;
    captured.flags = wire.flags;
    captured.script = wire.script;
    captured.scale = wire.scale;
    captured.placement = wire.placement;
    captured.hide_reason = wire.hide_reason;
    captured.highlight = wire.highlight;
    captured.has_highlight = wire.has_highlight;
    captured.page = [wire.page_current, wire.page_total, wire.page_size];
    captured.cursor = [
        wire.cursor.x,
        wire.cursor.y,
        wire.cursor.w as i32,
        wire.cursor.h as i32,
    ];
    captured.layout = [
        u32::from(wire.max_per_row),
        u32::from(wire.show_annotation),
        u32::from(wire.max_width_dp),
    ];
    captured.theme = [
        u64::from(wire.theme_accent),
        u64::from(wire.theme_scheme),
        u64::from(wire.theme_acrylic),
        u64::from(wire.theme_base_alpha),
        u64::from(wire.theme_corner_radius_dp),
        f64::from(wire.theme_scale).round() as u64,
    ];
    // The reader treats null-and-zero as the empty shape; the capture does the same
    // so a zeroed field reads back as empty rather than as a null dereference.
    let read_bytes = |raw: &RspinyinStr| -> String {
        if raw.len == 0 {
            return String::new();
        }
        // SAFETY: the writer's string points into the command it sent, valid here.
        let bytes = unsafe { std::slice::from_raw_parts(raw.ptr, raw.len as usize) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    captured.preedit = read_bytes(&wire.preedit);
    captured.mode_label = read_bytes(&wire.mode_label);
    fn read_slice<'a, R>(ptr: *const R, count: u32) -> &'a [R] {
        if count == 0 {
            return &[];
        }
        // SAFETY: the writer's array points into its scratch, valid here.
        unsafe { std::slice::from_raw_parts(ptr, count as usize) }
    }
    captured.span_kinds = read_slice(wire.spans, wire.span_count)
        .iter()
        .map(|span| span.kind)
        .collect();
    captured.candidate_texts = read_slice(wire.candidates, wire.candidate_count)
        .iter()
        .map(|row| read_bytes(&row.text))
        .collect();
    captured.candidate_sources = read_slice(wire.candidates, wire.candidate_count)
        .iter()
        .map(|row| row.source)
        .collect();
}

/// SAFETY: as `capture_frame`, for the overlay family.
extern "C" fn capture_overlay(ctx: *mut c_void, wire: *const RspinyinOverlayWire) {
    // SAFETY: as `capture_frame`.
    let wire = unsafe { &*wire };
    let captured = unsafe { &mut *(ctx.cast::<Captured>()) };
    captured.calls += 1;
    captured.overlay_kind = wire.kind;
    // Null-and-zero is the writer's empty shape; reading a closed overlay's zeroed
    // wire must skip the arrays, not dereference null.
    let read = |raw: &RspinyinStr| -> String {
        if raw.len == 0 {
            return String::new();
        }
        // SAFETY: the writer's string points into the command it sent, valid here.
        let bytes = unsafe { std::slice::from_raw_parts(raw.ptr, raw.len as usize) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    fn rows_of<'a>(
        ptr: *const RspinyinOverlayEntryWire,
        count: u32,
    ) -> &'a [RspinyinOverlayEntryWire] {
        if count == 0 {
            return &[];
        }
        // SAFETY: the writer's array points into its scratch, valid here.
        unsafe { std::slice::from_raw_parts(ptr, count as usize) }
    }
    let sections = if wire.section_count == 0 {
        &[][..]
    } else {
        // SAFETY: the writer's array points into its scratch, valid here.
        unsafe { std::slice::from_raw_parts(wire.sections, wire.section_count as usize) }
    };
    captured.overlay_sections = sections
        .iter()
        .map(|section| {
            let rows = rows_of(section.entries, section.entry_count);
            (
                read(&section.title),
                rows.iter()
                    .map(|row| (read(&row.keys), read(&row.label)))
                    .collect(),
            )
        })
        .collect();
}

/// Registers a test sink and answers the guard restoring the slot afterwards.
fn with_captured(test: impl FnOnce(&mut Captured)) {
    static mut CAPTURED: Option<Captured> = None;
    // SAFETY: the test bodies run one at a time on the test thread; nextest gives
    // every test its own process, so the static cannot be contended.
    let captured = unsafe {
        let taken = std::mem::take(&mut *std::ptr::addr_of_mut!(CAPTURED));
        *std::ptr::addr_of_mut!(CAPTURED) = Some(taken.unwrap_or_default());
        (*std::ptr::addr_of_mut!(CAPTURED)).as_mut().unwrap()
    };
    let sink = RspinyinUiSink {
        ctx: captured as *mut Captured as *mut c_void,
        frame: capture_frame,
        overlay: capture_overlay,
    };
    assert!(rspinyin_engine_register_ui_sinks(&sink));
    test(captured);
    rspinyin_engine_clear_ui_sinks();
    assert!(
        !dispatch(&UiCommand::Shutdown),
        "a cleared slot posts nothing"
    );
}

fn sample_frame() -> UiFrame {
    UiFrame {
        revision: 7,
        preedit: Preedit {
            text: "ni'hao".into(),
            caret: 3,
            spans: vec![
                PreeditSpan {
                    start: 0,
                    end: 2,
                    kind: SpanKind::Syllable,
                },
                PreeditSpan {
                    start: 3,
                    end: 6,
                    kind: SpanKind::Passthrough,
                },
            ],
        },
        candidates: vec![
            Candidate {
                index: 1,
                text: "你好".into(),
                annotation: Some("annotation".into()),
                source: CandidateSource::UserDict,
                score: 0.5,
                consumed_syllables: 2,
            },
            Candidate {
                index: 2,
                text: "拟好".into(),
                annotation: None,
                source: CandidateSource::Dict,
                score: 0.25,
                consumed_syllables: 2,
            },
        ],
        page: PageState {
            current: 1,
            total: 3,
            page_size: 5,
        },
        status: StatusStrip {
            mode_label: "拼音".into(),
            full_width: true,
            punctuation_full: false,
            has_user_dict_hit: true,
            readonly: true,
            chinese: true,
            script: Script::Traditional,
        },
        anchor: Anchor {
            cursor: RectI {
                x: 10,
                y: 20,
                w: 30,
                h: 40,
            },
            screen: ime_types::ScreenId::new(2),
            scale: 2.0,
            placement: Placement::Above,
        },
        layout: LayoutHint {
            max_per_row: 9,
            show_annotation: true,
            max_width_dp: 156,
        },
        highlight: Some(1),
    }
}

#[test]
fn test_frame_wire_round_trips_every_field() {
    with_captured(|captured| {
        assert!(dispatch(&UiCommand::Frame(Box::new(sample_frame()))));
        assert_eq!(captured.calls, 1);
        assert_eq!(captured.kind, KIND_FRAME);
        assert_eq!(captured.revision, 7);
        assert_eq!(captured.preedit, "ni'hao");
        assert_eq!(captured.caret, 3);
        assert_eq!(captured.span_kinds, vec![0, 2]);
        assert_eq!(captured.candidate_texts, vec!["你好", "拟好"]);
        assert_eq!(captured.candidate_sources, vec![1, 0]);
        assert_eq!(captured.page, [1, 3, 5]);
        assert_eq!(captured.mode_label, "拼音");
        assert_eq!(
            captured.flags,
            FLAG_FULL_WIDTH | FLAG_READONLY | FLAG_HAS_USER_DICT_HIT
        );
        assert_eq!(captured.script, 1);
        assert_eq!(captured.cursor, [10, 20, 30, 40]);
        assert_eq!(captured.scale, 2.0);
        assert_eq!(captured.placement, 1);
        assert_eq!(captured.layout, [9, 1, 156]);
        assert_eq!(captured.highlight, 1);
        assert_eq!(captured.has_highlight, 1);
    });
}

#[test]
fn test_frame_wire_carries_the_highlight_or_its_absence() {
    with_captured(|captured| {
        // `None` travels as the flag alone: the position slot is zeroed rather than
        // left holding a value the reader must know to ignore.
        let mut hidden = sample_frame();
        hidden.highlight = None;
        assert!(dispatch(&UiCommand::Frame(Box::new(hidden))));
        assert_eq!(captured.has_highlight, 0);
        assert_eq!(captured.highlight, 0);

        let mut shown = sample_frame();
        shown.highlight = Some(4);
        assert!(dispatch(&UiCommand::Frame(Box::new(shown))));
        assert_eq!(captured.has_highlight, 1);
        assert_eq!(captured.highlight, 4);
    });
}

#[test]
fn test_each_frame_family_kind_fills_its_own_fields() {
    with_captured(|captured| {
        assert!(dispatch(&UiCommand::Show {
            revision: 11,
            anchor: Anchor {
                cursor: RectI {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4
                },
                screen: ime_types::ScreenId::new(5),
                scale: 1.5,
                placement: Placement::Below,
            },
        }));
        assert_eq!(captured.kind, KIND_SHOW);
        assert_eq!(captured.revision, 11);
        assert_eq!(captured.cursor, [1, 2, 3, 4]);
        assert_eq!(captured.scale, 1.5);
        assert_eq!(captured.placement, 0);
        // The highlight pair is a `Frame` field: the kind-gated reader must find it
        // zeroed on every other kind of the family.
        assert_eq!((captured.highlight, captured.has_highlight), (0, 0));

        assert!(dispatch(&UiCommand::Hide {
            revision: 12,
            reason: HideReason::FocusLost,
        }));
        assert_eq!(captured.kind, KIND_HIDE);
        assert_eq!(captured.revision, 12);
        assert_eq!(captured.hide_reason, 2);

        assert!(dispatch(&UiCommand::Theme(ThemeSpec {
            scheme: ColorScheme::Dark,
            accent: Rgba8 {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 0x78
            },
            acrylic: true,
            base_alpha: 217,
            corner_radius_dp: 12,
            scale: 3.0,
        })));
        assert_eq!(captured.kind, KIND_THEME);
        assert_eq!(captured.theme, [0x12345678, 1, 1, 217, 12, 3]);

        assert!(dispatch(&UiCommand::Shutdown));
        assert_eq!(captured.kind, KIND_SHUTDOWN);
    });
}

#[test]
fn test_overlay_wire_round_trips_the_three_layers() {
    with_captured(|captured| {
        let overlay = OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: "按键速查".into(),
            sections: vec![
                OverlaySection {
                    title: "组字".into(),
                    entries: vec![
                        ime_types::OverlayEntry {
                            keys: "Tab".into(),
                            label: "换词".into(),
                        },
                        ime_types::OverlayEntry {
                            keys: "↓".into(),
                            label: "下一项".into(),
                        },
                    ],
                },
                OverlaySection {
                    title: "模式".into(),
                    entries: vec![ime_types::OverlayEntry {
                        keys: "Shift".into(),
                        label: "中英".into(),
                    }],
                },
            ],
            selected: None,
            query: String::new(),
        };
        assert!(dispatch(&UiCommand::Overlay(Some(Box::new(overlay)))));
        assert_eq!(captured.calls, 1);
        assert_eq!(captured.overlay_kind, KIND_OVERLAY_OPEN);
        assert_eq!(
            captured.overlay_sections,
            vec![
                (
                    "组字".to_string(),
                    vec![
                        ("Tab".to_string(), "换词".to_string()),
                        ("↓".to_string(), "下一项".to_string())
                    ]
                ),
                (
                    "模式".to_string(),
                    vec![("Shift".to_string(), "中英".to_string())]
                ),
            ]
        );

        assert!(dispatch(&UiCommand::Overlay(None)));
        assert_eq!(captured.overlay_kind, KIND_OVERLAY_CLOSED);
    });
}

#[test]
fn test_dispatch_reports_a_missing_channel_and_registration_refuses_null() {
    // No sink registered: the dispatch answers false, which is what drives the
    // caller's `ui/not-ready` diagnostic.
    assert!(!dispatch(&UiCommand::Shutdown));
    assert!(!rspinyin_engine_register_ui_sinks(std::ptr::null()));
    // A null sink is refused rather than stored, so the slot stays empty.
    assert!(!dispatch(&UiCommand::Shutdown));
}

/// An `Anchor`-kind wire carrying a caret the reader can be asserted against.
fn anchor_wire() -> RspinyinEventWire {
    RspinyinEventWire {
        kind: EVENT_KIND_ANCHOR,
        revision: 0,
        index: 0,
        reason: 0,
        anchor_x: -40,
        anchor_y: 300,
        anchor_w: 12,
        anchor_h: 30,
        anchor_screen: 1,
        anchor_scale: 1.25,
        anchor_placement: 1,
    }
}

#[test]
fn test_anchor_wire_reads_back_every_anchor_field() {
    let anchor = anchor_from_wire(&anchor_wire()).expect("a well-formed anchor parses");
    assert_eq!(
        anchor.cursor,
        RectI {
            x: -40,
            y: 300,
            w: 12,
            h: 30
        },
        "the caret rectangle travels field for field"
    );
    assert_eq!(anchor.screen, ime_types::ScreenId::new(1));
    assert_eq!(anchor.scale, 1.25);
    assert_eq!(anchor.placement, Placement::Above);
}

#[test]
fn test_anchor_wire_is_not_a_window_event() {
    // The anchor is engine state, so the event reader must refuse it: a caret report
    // that could be mistaken for a click would step a session that never saw one.
    assert!(event_from_wire(&anchor_wire()).is_none());
}

#[test]
fn test_anchor_wire_rejects_a_negative_screen_or_unknown_placement() {
    let mut wire = anchor_wire();
    wire.anchor_screen = -1;
    assert!(
        anchor_from_wire(&wire).is_none(),
        "a screen id cannot be negative, so the wire is malformed"
    );
    wire.anchor_screen = 1;
    wire.anchor_placement = 3;
    assert!(
        anchor_from_wire(&wire).is_none(),
        "a placement this version does not know is a drifted wire"
    );
}

#[test]
fn test_event_ingest_refuses_a_null_or_unreadable_wire() {
    assert!(!rspinyin_event_ingest(1, std::ptr::null()));
    let mut unknown = anchor_wire();
    unknown.kind = 99;
    assert!(
        !rspinyin_event_ingest(1, &unknown),
        "a kind this transcription cannot read is refused, not guessed"
    );
}

#[test]
fn test_event_ingest_accepts_a_well_formed_anchor() {
    // With no session host installed the anchor still parses and is handed to the
    // session layer, which records the missing host on its own; acceptance here is
    // about the wire. The anchor's effect on the frames a session builds is asserted
    // at the session-host level, where the routing table is in place.
    assert!(rspinyin_event_ingest(1, &anchor_wire()));
}
