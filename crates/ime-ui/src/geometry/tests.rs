//! Unit tests for the placement pass and the hit map.
//!
//! Everything here is pure arithmetic, so the whole suite runs unconditionally: no test
//! needs a display server, a compositor or a font, and none of them touch a clock.

use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId,
    StatusStrip, UiFrame,
};

use crate::layout::{ContainerSize, Metrics};

use super::*;

/// Caret line height the tests use, in physical pixels.
const LINE: u32 = 20;

/// The output every test but the multi-monitor ones uses.
const FULL_HD: (u32, u32) = (1920, 1080);

/// The component's own constants, read back from its source.
fn constants() -> &'static Metrics {
    crate::layout::metrics().expect("the component declares a readable metrics block")
}

/// A panel of `size` logical pixels holding `columns` cells of `cell_width` each.
fn panel(size: (f32, f32), cell_width: f32, columns: u8) -> Panel {
    Panel {
        size: ContainerSize {
            width: size.0,
            height: size.1,
        },
        cell_width,
        columns,
    }
}

/// The panel a five-candidate single row of 64dp cells asks for.
fn row_panel() -> Panel {
    panel((360.0, 87.0), 64.0, 5)
}

/// A caret at `(x, y)` on the primary output, with the placement left to the pass.
fn anchor_at(x: i32, y: i32) -> Anchor {
    Anchor {
        cursor: RectI {
            x,
            y,
            w: 2,
            h: LINE,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Auto,
    }
}

/// An output at `origin` of `size`, at ratio 1.0.
fn output(id: u32, origin: (i32, i32), size: (u32, u32)) -> Screen {
    Screen {
        id: ScreenId::new(id),
        origin,
        size,
        scale: 1.0,
    }
}

/// A frame carrying `candidates` candidates and a column cap.
fn frame_with(candidates: usize, max_per_row: u8) -> UiFrame {
    UiFrame {
        revision: 1,
        preedit: Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: Vec::new(),
        },
        candidates: (0..candidates)
            .map(|position| Candidate {
                index: u16::try_from(position + 1).unwrap_or(u16::MAX),
                text: String::from("candidate"),
                annotation: None,
                source: CandidateSource::Dict,
                score: 0.0,
                consumed_syllables: 1,
            })
            .collect(),
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: anchor_at(0, 0),
        layout: LayoutHint {
            max_per_row,
            show_annotation: true,
            max_width_dp: 720,
        },
    }
}

/// Runs the pass for a single-output desktop.
fn place(anchor: &Anchor, screen: &Screen, panel: Panel, frame: &UiFrame) -> Geometry {
    let screens = core::slice::from_ref(screen);
    let desktop = Desktop {
        screens,
        primary: screen.id,
    };
    compute(&PlacementRequest::new(
        anchor,
        desktop,
        panel,
        frame,
        constants(),
    ))
}

/// Whether a geometry's window lies entirely inside an output.
fn inside(geometry: &Geometry, screen: &Screen) -> bool {
    let (x, y) = geometry.window_pos;
    let (w, h) = geometry.window_size;
    let right = screen.origin.0 + screen.size.0 as i32;
    let bottom = screen.origin.1 + screen.size.1 as i32;
    x >= screen.origin.0 && y >= screen.origin.1 && x + w as i32 <= right && y + h as i32 <= bottom
}

/// The four corners of an output, inset far enough to be inside it.
fn corners(screen: &Screen) -> [(i32, i32); 4] {
    let left = screen.origin.0 + 5;
    let top = screen.origin.1 + 5;
    let right = screen.origin.0 + screen.size.0 as i32 - 5;
    let bottom = screen.origin.1 + screen.size.1 as i32 - 5;
    [(left, top), (right, top), (left, bottom), (right, bottom)]
}

#[test]
fn test_compute_window_that_fits_sits_below_the_caret() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.placement, Placement::Below);
    assert_eq!(geometry.window_pos, (749, 226));
    assert_eq!(geometry.window_size, (424, 152));
    assert_eq!(geometry.container_offset, (32, 32));
    assert_eq!(geometry.container_size, (360, 88));
    assert!(!geometry.clamped_x);
    assert!(!geometry.clamped_y);
    assert_eq!(geometry.screen, ScreenId::new(0));
    assert!(geometry.scale.is_exact);
    assert_eq!(geometry.scale.value, 1.0);
    assert!(geometry.arrow.is_some(), "the placement is clean");
}

#[test]
fn test_compute_caret_at_the_bottom_flips_above_and_drops_the_arrow() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 1000);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.placement, Placement::Above);
    assert_eq!(geometry.window_pos, (749, 842));
    assert!(!geometry.clamped_y, "the flip is a decision, not a clamp");
    assert_eq!(geometry.arrow, None, "an arrow would point the wrong way");
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_clamps_at_the_left_edge_and_hides_the_arrow() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(5, 200);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.window_pos.0, 8, "the edge margin is kept");
    assert!(geometry.clamped_x);
    assert!(!geometry.clamped_y);
    assert_eq!(geometry.arrow, None, "the arrow would leave the window");
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_clamps_at_the_right_edge_and_hides_the_arrow() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(1915, 200);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.window_pos.0, 1920 - 424 - 8);
    assert!(geometry.clamped_x);
    assert_eq!(geometry.arrow, None);
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_keeps_the_window_inside_the_output_at_every_corner() {
    let screen = output(0, (0, 0), FULL_HD);
    let frame = frame_with(5, 5);
    for (x, y) in corners(&screen) {
        let geometry = place(&anchor_at(x, y), &screen, row_panel(), &frame);
        let window = geometry.window_pos;
        assert!(
            inside(&geometry, &screen),
            "caret at ({x}, {y}) produced {window:?} of {:?}",
            geometry.window_size
        );
        assert!(geometry.clamped_x || geometry.clamped_y);
    }
}

#[test]
fn test_compute_pins_a_window_taller_than_the_screen_to_the_output() {
    // An output too short for one row plus the shadow reserve: the window cannot fit, so
    // the pass keeps the legal part -- pinned inside the margin, and it says so.
    let screen = output(0, (0, 0), (200, 100));
    let anchor = anchor_at(50, 50);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.window_pos, (8, 8));
    assert!(geometry.clamped_x);
    assert!(geometry.clamped_y);
    assert_eq!(geometry.arrow, None);
    assert!(geometry.window_size.0 > 0 && geometry.window_size.1 > 0);
}

#[test]
fn test_compute_keeps_the_window_on_the_output_the_caret_is_on() {
    let screens = [output(0, (0, 0), FULL_HD), output(1, (1920, 0), FULL_HD)];
    let desktop = Desktop {
        screens: &screens,
        primary: ScreenId::new(0),
    };
    let frame = frame_with(5, 5);
    let constants = constants();

    let mut anchor = anchor_at(2880, 200);
    anchor.screen = ScreenId::new(1);
    let request = PlacementRequest::new(&anchor, desktop, row_panel(), &frame, constants);
    let geometry = compute(&request);
    assert_eq!(geometry.screen, ScreenId::new(1));
    assert_eq!(geometry.window_pos, (2669, 226));
    assert!(!geometry.clamped_x, "centred on its own output");
    assert!(inside(&geometry, &screens[1]));

    // Hard against the left edge of the second output: the window is clamped there rather
    // than allowed to straddle two outputs.
    let mut edge = anchor_at(1925, 200);
    edge.screen = ScreenId::new(1);
    let request = PlacementRequest::new(&edge, desktop, row_panel(), &frame, constants);
    let clamped = compute(&request);
    assert_eq!(clamped.screen, ScreenId::new(1));
    assert_eq!(clamped.window_pos.0, 1928);
    assert!(clamped.clamped_x);
    assert!(inside(&clamped, &screens[1]));
}

#[test]
fn test_compute_falls_back_to_the_anchor_output_when_the_caret_is_in_a_gap() {
    let screens = [output(0, (0, 0), FULL_HD), output(1, (1920, 0), FULL_HD)];
    let desktop = Desktop {
        screens: &screens,
        primary: ScreenId::new(0),
    };
    let frame = frame_with(5, 5);
    let constants = constants();

    // A caret below both outputs, still claiming the second one.
    let mut anchor = anchor_at(5000, 5000);
    anchor.screen = ScreenId::new(1);
    let request = PlacementRequest::new(&anchor, desktop, row_panel(), &frame, constants);
    let geometry = compute(&request);
    assert_eq!(geometry.screen, ScreenId::new(1));
    assert!(inside(&geometry, &screens[1]));

    // A caret naming an output the enumeration does not have falls through to the primary
    // one rather than to nowhere.
    let mut stale = anchor_at(5000, 5000);
    stale.screen = ScreenId::new(9);
    let request = PlacementRequest::new(&stale, desktop, row_panel(), &frame, constants);
    let geometry = compute(&request);
    assert_eq!(geometry.screen, ScreenId::new(0));
    assert!(inside(&geometry, &screens[0]));
}

#[test]
fn test_compute_without_any_output_places_below_and_clamps_nothing() {
    let desktop = Desktop {
        screens: &[],
        primary: ScreenId::new(0),
    };
    let frame = frame_with(5, 5);
    let anchor = anchor_at(960, 200);
    let request = PlacementRequest::new(&anchor, desktop, row_panel(), &frame, constants());
    let geometry = compute(&request);

    assert_eq!(geometry.placement, Placement::Below);
    assert_eq!(geometry.window_pos, (749, 226));
    assert!(!geometry.clamped_x);
    assert!(!geometry.clamped_y);
    assert_eq!(geometry.screen, ScreenId::new(0));
    assert!(geometry.arrow.is_some());
}

#[test]
fn test_compute_reduces_the_rows_until_the_window_fits_the_output() {
    let screen = output(0, (0, 0), (1920, 400));
    let anchor = anchor_at(960, 200);
    let frame = frame_with(9, 5);
    let tall = panel((360.0, 500.0), 64.0, 5);
    let geometry = place(&anchor, &screen, tall, &frame);

    assert_eq!(geometry.placement, Placement::Below);
    assert_eq!(geometry.container_size.1, 88, "one row is all it holds");
    assert_eq!(geometry.window_size, (424, 152));
    assert_eq!(geometry.hit_map.len(), 5, "one row of five candidates");
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_narrows_a_container_wider_than_the_output() {
    let screen = output(0, (0, 0), (600, 800));
    let anchor = anchor_at(300, 200);
    let frame = frame_with(9, 5);
    let wide = panel((720.0, 87.0), 64.0, 5);
    let geometry = place(&anchor, &screen, wide, &frame);

    assert_eq!(geometry.container_size.0, 520, "narrowed to the output");
    assert_eq!(geometry.window_size.0, 584);
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_hit_map_matches_the_grid_geometry() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    let expected: Vec<RectI> = (0..5)
        .map(|column| RectI {
            x: 8 + column * 70,
            y: 43,
            w: 64,
            h: 36,
        })
        .collect();
    let rects: Vec<RectI> = geometry.hit_map.iter().map(|(rect, _)| *rect).collect();
    assert_eq!(rects, expected);
    let indices: Vec<u16> = geometry.hit_map.iter().map(|(_, index)| *index).collect();
    assert_eq!(indices, [0u16, 1, 2, 3, 4]);

    let last = geometry.hit_map[4].0;
    let inner_edge = geometry.container_size.0 as i32 - 8;
    assert_eq!(last.x + last.w as i32, inner_edge, "cells end inside");
}

#[test]
fn test_compute_hit_map_wraps_onto_the_second_row() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(9, 5);
    let two_rows = panel((360.0, 130.0), 64.0, 5);
    let geometry = place(&anchor, &screen, two_rows, &frame);

    let second_row = RectI {
        x: 8,
        y: 85,
        w: 64,
        h: 36,
    };
    let last = RectI {
        x: 218,
        y: 85,
        w: 64,
        h: 36,
    };
    assert_eq!(geometry.hit_map.len(), 9);
    assert_eq!(geometry.hit_map[5].0, second_row);
    assert_eq!(geometry.hit_map[8].0, last);
    assert_eq!(geometry.hit_map[8].1, 8);
}

#[test]
fn test_compute_numbers_candidates_from_the_global_page_offset() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let mut frame = frame_with(5, 5);
    frame.page = PageState {
        current: 3,
        total: 5,
        page_size: 9,
    };
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    let first = geometry.hit_map.first().map(|(_, index)| *index);
    let last = geometry.hit_map.last().map(|(_, index)| *index);
    assert_eq!(first, Some(18));
    assert_eq!(last, Some(22));
}

#[test]
fn test_compute_hit_map_is_empty_without_candidates() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(0, 5);
    let empty = panel((220.0, 45.0), 64.0, 3);
    let geometry = place(&anchor, &screen, empty, &frame);

    assert!(geometry.hit_map.is_empty());
    assert!(geometry.window_size.0 > 0 && geometry.window_size.1 > 0);
}

#[test]
fn test_compute_hit_map_is_bounded_by_what_the_container_shows() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(45, 5);
    let two_rows = panel((360.0, 130.0), 64.0, 5);
    let geometry = place(&anchor, &screen, two_rows, &frame);

    assert_eq!(geometry.hit_map.len(), 10, "two rows of five, not 45");
    assert!(geometry.hit_map.len() <= frame.candidates.len());
}

#[test]
fn test_compute_keeps_the_hit_map_on_the_cells_the_layout_chose() {
    // A cell wider than the minimum moves every cell after the first: the hit map has to
    // follow the layout pass's width, not the component's minimum.
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(3, 5);
    let wide = panel((328.0, 87.0), 100.0, 3);
    let geometry = place(&anchor, &screen, wide, &frame);

    let second = geometry.hit_map[1].0;
    let third = geometry.hit_map[2].0;
    assert_eq!((second.x, second.w), (8 + 106, 100));
    assert_eq!(third.x, 8 + 212);
}

#[test]
fn test_compute_honours_a_pinned_placement() {
    let screen = output(0, (0, 0), FULL_HD);
    let frame = frame_with(5, 5);

    let mut pinned_below = anchor_at(960, 1000);
    pinned_below.placement = Placement::Below;
    let below = place(&pinned_below, &screen, row_panel(), &frame);
    assert_eq!(below.placement, Placement::Below, "not flipped");
    assert!(below.clamped_y, "clamped into the output instead");
    assert_eq!(below.arrow, None);
    assert!(inside(&below, &screen));

    let mut pinned_above = anchor_at(960, 200);
    pinned_above.placement = Placement::Above;
    let above = place(&pinned_above, &screen, row_panel(), &frame);
    assert_eq!(above.placement, Placement::Above);
    assert_eq!(above.window_pos.1, 42);
    assert!(!above.clamped_y);
    assert_eq!(above.arrow, None, "the arrow belongs below the caret");
}

#[test]
fn test_compute_scales_every_physical_number() {
    let screen = output(0, (0, 0), FULL_HD);
    let frame = frame_with(5, 5);
    let mut anchor = anchor_at(960, 200);
    anchor.scale = 2.0;
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert_eq!(geometry.window_size, (848, 302));
    assert_eq!(geometry.container_offset, (64, 64));
    assert_eq!(geometry.window_pos, (537, 232));
    let arrow = geometry.arrow.expect("a clean placement draws the arrow");
    assert_eq!((arrow.x, arrow.y, arrow.w, arrow.h), (949, 232, 24, 12));
    let first = geometry.hit_map[0].0;
    assert_eq!((first.x, first.y, first.w, first.h), (16, 86, 128, 72));
}

#[test]
fn test_compute_window_size_is_always_even_and_non_zero() {
    let screen = output(0, (0, 0), FULL_HD);
    let frame = frame_with(9, 5);
    let limits = u32::MAX as f32;
    let sizes = [
        (0.0, 0.0),
        (1.0, 1.0),
        (359.0, 89.0),
        (361.0, 87.0),
        (720.0, 500.0),
        (limits, limits),
    ];
    let scales = [1.0_f32, 1.25, 1.5, 2.0, 3.0, 0.0, f32::NAN];
    for size in sizes {
        for scale in scales {
            let mut anchor = anchor_at(960, 200);
            anchor.scale = scale;
            let screens = core::slice::from_ref(&screen);
            let desktop = Desktop {
                screens,
                primary: ScreenId::new(0),
            };
            let request = PlacementRequest {
                anchor: &anchor,
                desktop,
                panel: panel(size, 64.0, 5),
                frame: &frame,
                metrics: constants(),
                scale,
            };
            let geometry = compute(&request);
            let (width, height) = geometry.window_size;
            let (inner_w, inner_h) = geometry.container_size;
            assert_eq!(width % 2, 0, "panel {size:?} at {scale}");
            assert_eq!(height % 2, 0, "panel {size:?} at {scale}");
            assert!(width >= 2 && height >= 2);
            assert!(inner_w >= 2 && inner_h >= 2);
        }
    }
}

#[test]
fn test_compute_survives_a_zero_sized_container() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(3, 5);
    let geometry = place(&anchor, &screen, panel((0.0, 0.0), 64.0, 5), &frame);

    assert_eq!(geometry.container_size, (2, 2));
    assert_eq!(geometry.window_size, (66, 66));
    assert_eq!(geometry.hit_map.len(), 1, "one cell fits in two pixels");
    assert!(inside(&geometry, &screen));
}

#[test]
fn test_compute_replaces_a_scale_it_cannot_use() {
    let screen = output(0, (0, 0), FULL_HD);
    let frame = frame_with(5, 5);
    let baseline = place(&anchor_at(960, 200), &screen, row_panel(), &frame);

    for unusable in [0.0_f32, -2.0, f32::NAN, f32::INFINITY] {
        let mut anchor = anchor_at(960, 200);
        anchor.scale = unusable;
        let geometry = place(&anchor, &screen, row_panel(), &frame);
        assert_eq!(geometry.scale.value, 1.0, "{unusable} falls back");
        assert!(!geometry.scale.is_exact, "{unusable} is not exact");
        assert_eq!(geometry.window_size, baseline.window_size);
        assert_eq!(geometry.window_pos, baseline.window_pos);
    }
}

#[test]
fn test_snap_scale_moves_to_the_nearest_supported_ratio() {
    for supported in SUPPORTED_SCALES {
        let snap = snap_scale(supported);
        assert_eq!(snap.value, supported);
        assert!(snap.is_exact, "{supported} is supported as it stands");
    }
    assert_eq!(snap_scale(1.2).value, 1.25);
    assert!(!snap_scale(1.2).is_exact);
    assert_eq!(snap_scale(1.4).value, 1.5);
    assert_eq!(snap_scale(2.9).value, 3.0);
    assert_eq!(snap_scale(0.4).value, 1.0, "below the lowest ratio");
    assert!(!snap_scale(0.4).is_exact);
    // A tie goes to the lower ratio, so the choice does not depend on comparison order.
    assert_eq!(snap_scale(1.125).value, 1.0);
    assert_eq!(snap_scale(1.375).value, 1.25);
}

#[test]
fn test_compute_is_deterministic() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(9, 5);
    let two_rows = panel((360.0, 130.0), 64.0, 5);
    let first = place(&anchor, &screen, two_rows, &frame);
    let second = place(&anchor, &screen, two_rows, &frame);

    assert_eq!(first, second, "the same request, the same geometry");
    assert_eq!(first.hit_map, second.hit_map);
}

#[test]
fn test_screen_contains_uses_half_open_bounds() {
    let screen = output(7, (100, 200), (300, 400));
    assert!(screen.contains((100, 200)));
    assert!(screen.contains((399, 599)));
    assert!(!screen.contains((400, 600)), "the far edge is next");
    assert!(!screen.contains((99, 200)));

    let empty = output(0, (0, 0), (0, 0));
    assert!(!empty.contains((0, 0)), "an empty output holds nothing");
}

#[test]
fn test_screen_contains_survives_a_size_that_overflows_i32() {
    let screen = output(0, (i32::MAX - 10, 0), (100, 100));
    assert!(screen.contains((i32::MAX, 50)));
    assert!(!screen.contains((i32::MIN, 50)));
}

#[test]
fn test_compute_survives_an_output_that_overflows_i32() {
    // A backend that reported nonsense must not be able to wrap the arithmetic: the window
    // still lands somewhere legal for the output it was given.
    let screen = output(0, (i32::MIN + 1000, 0), (u32::MAX, u32::MAX));
    let anchor = anchor_at(0, 0);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    assert!(geometry.window_size.0 > 0 && geometry.window_size.1 > 0);
    assert!(geometry.window_pos.0 >= i32::MIN + 1000 + 8);
}

#[test]
fn test_compute_keeps_the_window_clear_of_the_caret_gap() {
    let screen = output(0, (0, 0), FULL_HD);
    let anchor = anchor_at(960, 200);
    let frame = frame_with(5, 5);
    let geometry = place(&anchor, &screen, row_panel(), &frame);

    let caret_bottom = anchor.cursor.y + anchor.cursor.h as i32;
    assert_eq!(geometry.window_pos.1, caret_bottom + 6);

    let arrow = geometry.arrow.expect("a clean placement draws the arrow");
    assert_eq!(arrow.y, geometry.window_pos.1, "the arrow fills the gap");
    assert_eq!(arrow.h, 6);
    let caret_centre = anchor.cursor.x + anchor.cursor.w as i32 / 2;
    assert_eq!(arrow.x + arrow.w as i32 / 2, caret_centre);
}
