//! Parse-fidelity tests: the wire shapes the engine builds, read back field for field.

use super::*;

/// Builds a frame-family wire over local storage, the way the engine's writer lays
/// it out. `annotation_ptr` distinguishes "no annotation" (null) from "empty" (a
/// live pointer with a zero length), which is the one distinction the parse has to
/// keep.
fn wire_of(kind: u32) -> RspinyinFrameWire {
    RspinyinFrameWire {
        kind,
        revision: 9,
        preedit: RspinyinStr {
            ptr: b"ni".as_ptr(),
            len: 2,
        },
        caret: 2,
        spans: std::ptr::null(),
        span_count: 0,
        candidates: std::ptr::null(),
        candidate_count: 0,
        page_current: 0,
        page_total: 0,
        page_size: 0,
        mode_label: RspinyinStr {
            ptr: b"py".as_ptr(),
            len: 2,
        },
        flags: FLAG_FULL_WIDTH | FLAG_READONLY,
        script: 1,
        cursor: RspinyinRectWire {
            x: 3,
            y: 4,
            w: 5,
            h: 6,
        },
        screen: 1,
        scale: 1.25,
        placement: 2,
        max_per_row: 5,
        show_annotation: 1,
        max_width_dp: 156,
        theme_accent: 0,
        theme_scheme: 0,
        theme_acrylic: 0,
        theme_base_alpha: 0,
        theme_corner_radius_dp: 0,
        theme_scale: 0.0,
        hide_reason: 0,
    }
}

#[test]
fn test_command_from_wire_rebuilds_a_frame_field_for_field() {
    let text = String::from("你好");
    let annotation = String::from("注");
    let empty = String::new();
    let candidates = [
        RspinyinCandidateWire {
            index: 1,
            text: RspinyinStr {
                ptr: text.as_ptr(),
                len: text.len() as u32,
            },
            annotation: RspinyinStr {
                ptr: annotation.as_ptr(),
                len: annotation.len() as u32,
            },
            source: 5,
            score: 0.75,
            consumed_syllables: 2,
        },
        RspinyinCandidateWire {
            index: 2,
            text: RspinyinStr {
                ptr: empty.as_ptr(),
                len: 0,
            },
            annotation: RspinyinStr {
                ptr: std::ptr::null(),
                len: 0,
            },
            source: 0,
            score: 0.5,
            consumed_syllables: 1,
        },
    ];
    let mut wire = wire_of(KIND_FRAME);
    wire.candidates = candidates.as_ptr();
    wire.candidate_count = candidates.len() as u32;

    let Some(UiCommand::Frame(frame)) = command_from_wire(&wire) else {
        panic!("a well-formed frame wire parses");
    };
    assert_eq!(frame.revision, 9);
    assert_eq!(frame.preedit.text, "ni");
    assert_eq!(frame.preedit.caret, 2);
    assert_eq!(frame.status.mode_label, "py");
    assert!(frame.status.full_width);
    assert!(frame.status.readonly);
    assert!(!frame.status.punctuation_full);
    assert_eq!(frame.status.script, Script::Traditional);
    assert_eq!(frame.anchor.cursor.x, 3);
    assert_eq!(frame.anchor.screen.value(), 1);
    assert_eq!(frame.anchor.scale, 1.25);
    assert_eq!(frame.anchor.placement, Placement::Auto);
    assert_eq!(frame.layout.max_per_row, 5);
    assert_eq!(frame.candidates.len(), 2);
    assert_eq!(frame.candidates[0].text, "你好");
    assert_eq!(frame.candidates[0].annotation.as_deref(), Some("注"));
    assert_eq!(frame.candidates[0].source, CandidateSource::Phrase);
    // A null-and-zero annotation is "no annotation", not an empty one.
    assert_eq!(frame.candidates[1].annotation, None);
    assert_eq!(frame.candidates[1].text, "");
}

#[test]
fn test_command_from_wire_maps_every_kind() {
    assert!(matches!(
        command_from_wire(&wire_of(KIND_SHOW)),
        Some(UiCommand::Show { revision: 9, .. })
    ));
    let mut hide = wire_of(KIND_HIDE);
    hide.revision = 4;
    hide.hide_reason = 2;
    assert!(matches!(
        command_from_wire(&hide),
        Some(UiCommand::Hide {
            revision: 4,
            reason: HideReason::FocusLost
        })
    ));
    let mut theme = wire_of(KIND_THEME);
    theme.theme_accent = 0x12345678;
    theme.theme_scheme = 1;
    theme.theme_acrylic = 1;
    theme.theme_base_alpha = 200;
    theme.theme_corner_radius_dp = 12;
    theme.theme_scale = 2.0;
    let Some(UiCommand::Theme(spec)) = command_from_wire(&theme) else {
        panic!("a theme wire parses");
    };
    assert_eq!(spec.scheme, ColorScheme::Dark);
    assert_eq!(spec.accent.r, 0x12);
    assert_eq!(spec.accent.a, 0x78);
    assert!(spec.acrylic);
    assert_eq!(spec.base_alpha, 200);
    assert!(matches!(
        command_from_wire(&wire_of(KIND_SHUTDOWN)),
        Some(UiCommand::Shutdown)
    ));
}

#[test]
fn test_command_from_wire_refuses_malformed_shapes() {
    assert!(command_from_wire(std::ptr::null()).is_none());
    // A kind this transcription does not know is refused, not guessed at.
    assert!(command_from_wire(&wire_of(99)).is_none());
    // A string whose pointer is null but whose length is not is invalid.
    let mut bad = wire_of(KIND_FRAME);
    bad.preedit = RspinyinStr {
        ptr: std::ptr::null(),
        len: 4,
    };
    assert!(command_from_wire(&bad).is_none());
    // A negative screen id has no ScreenId to become.
    let mut bad_screen = wire_of(KIND_SHOW);
    bad_screen.screen = -1;
    assert!(command_from_wire(&bad_screen).is_none());
    // An enum code past the table is refused.
    let mut bad_source = wire_of(KIND_FRAME);
    let candidates = [RspinyinCandidateWire {
        index: 1,
        text: RspinyinStr {
            ptr: b"x".as_ptr(),
            len: 1,
        },
        annotation: RspinyinStr {
            ptr: std::ptr::null(),
            len: 0,
        },
        source: 99,
        score: 0.0,
        consumed_syllables: 1,
    }];
    bad_source.candidates = candidates.as_ptr();
    bad_source.candidate_count = 1;
    assert!(command_from_wire(&bad_source).is_none());
}

#[test]
fn test_command_from_overlay_reads_both_slot_values() {
    let keys = String::from("Tab");
    let label = String::from("换词");
    let title = String::from("组字");
    let entries = [RspinyinOverlayEntryWire {
        keys: RspinyinStr {
            ptr: keys.as_ptr(),
            len: keys.len() as u32,
        },
        label: RspinyinStr {
            ptr: label.as_ptr(),
            len: label.len() as u32,
        },
    }];
    let sections = [RspinyinOverlaySectionWire {
        title: RspinyinStr {
            ptr: title.as_ptr(),
            len: title.len() as u32,
        },
        entries: entries.as_ptr(),
        entry_count: 1,
    }];
    let wire = RspinyinOverlayWire {
        kind: KIND_OVERLAY_OPEN,
        panel_kind: 0,
        title: RspinyinStr {
            ptr: title.as_ptr(),
            len: title.len() as u32,
        },
        selected: -1,
        query: RspinyinStr {
            ptr: std::ptr::null(),
            len: 0,
        },
        sections: sections.as_ptr(),
        section_count: 1,
    };
    let Some(UiCommand::Overlay(Some(frame))) = command_from_overlay(&wire) else {
        panic!("an open-overlay wire parses");
    };
    assert_eq!(frame.kind, OverlayKind::CheatSheet);
    assert_eq!(frame.title, "组字");
    assert_eq!(frame.sections.len(), 1);
    assert_eq!(frame.sections[0].entries[0].label, "换词");
    assert_eq!(frame.selected, None);
    assert_eq!(frame.query, "");

    let closed = RspinyinOverlayWire {
        kind: KIND_OVERLAY_CLOSED,
        ..wire
    };
    assert!(matches!(
        command_from_overlay(&closed),
        Some(UiCommand::Overlay(None))
    ));
}
