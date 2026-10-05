//! The pressed cell's sink: the fifth motion, driven by the pointer state.
//!
//! The scenes run a real component against the mock surface and read the sink back
//! through the `press-scale` property the grid draws from -- the same write path the
//! renderer's frames travel. The clock stays out: the tests step `FRAME_S` themselves.

use super::PointerState;
use super::motion::{FRAME_S, settle};
use super::{Adapter, frame_with, with_adapter};
use crate::spring::PRESS_SCALE_TO;

/// Whether the model row at `position` draws the pressed state.
fn pressed_at(adapter: &Adapter, position: usize) -> Option<bool> {
    adapter
        .window()
        .get_items()
        .row_data(position)
        .map(|cell| cell.is_pressed)
}

#[test]
fn test_adapter_press_sinks_the_pressed_cell_monotonically_to_its_scale() {
    let (scales, pressed, settled, frames) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(0),
                hovered: Some(1),
                pressed: Some(1),
            }),
            "a press changes what the grid draws"
        );
        let pressed = pressed_at(adapter, 1);
        let mut scales = Vec::new();
        let mut frames = 0u32;
        while adapter.advance(FRAME_S).expect("the motion advances") {
            frames += 1;
            assert!(frames < 1_000, "the sink must settle");
            scales.push(adapter.window().get_press_scale());
        }
        (scales, pressed, adapter.window().get_press_scale(), frames)
    });
    assert_eq!(
        pressed,
        Some(true),
        "the pressed cell carries the Active flag"
    );
    assert!(
        (settled - PRESS_SCALE_TO).abs() < 1.0e-6,
        "the sink lands on the exact pressed scale, got {settled}"
    );
    // The 60ms the Active row allots the sink, rounded up to one 144Hz frame.
    assert!(
        frames <= 9,
        "the sink took {frames} frames, past the 60ms it is allotted"
    );
    // Every frame in flight is strictly between the pressed floor and full size, and
    // each one is deeper than the last: no ring, no stall, no jump.
    let mut previous = 1.0;
    for scale in &scales {
        assert!(
            *scale > PRESS_SCALE_TO && *scale < previous,
            "the sink is monotone: {scale} after {previous}"
        );
        previous = *scale;
    }
}

#[test]
fn test_adapter_release_mid_sink_rebounds_and_unmarks_the_cell() {
    let (mid_sink, scales, settled, unmarked) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(0),
            hovered: Some(1),
            pressed: Some(1),
        }));
        for _ in 0..3 {
            adapter.advance(FRAME_S).expect("the motion advances");
        }
        let mid_sink = adapter.window().get_press_scale();
        assert!(
            mid_sink > PRESS_SCALE_TO && mid_sink < 1.0,
            "the release lands while the cell is still sinking, got {mid_sink}"
        );
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(0),
            hovered: Some(1),
            pressed: None,
        }));
        let mut scales = Vec::new();
        while adapter.advance(FRAME_S).expect("the motion advances") {
            scales.push(adapter.window().get_press_scale());
            assert!(scales.len() < 1_000, "the rebound must settle");
        }
        let settled = adapter.window().get_press_scale();
        let unmarked = pressed_at(adapter, 1);
        (mid_sink, scales, settled, unmarked)
    });
    assert_eq!(
        unmarked,
        Some(false),
        "the released cell loses the Active flag"
    );
    assert!(
        (settled - 1.0).abs() < 1.0e-6,
        "the rebound lands back at full size, got {settled}"
    );
    let mut previous = mid_sink;
    for scale in &scales {
        assert!(
            *scale > previous && *scale <= 1.0,
            "the rebound is monotone and never rings past full size: {scale}"
        );
        previous = *scale;
    }
}

#[test]
fn test_adapter_press_is_cancelled_by_a_page_turn_and_rebounds() {
    let (marks, settled) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(0),
            hovered: None,
            pressed: Some(1),
        }));
        for _ in 0..3 {
            adapter.advance(FRAME_S).expect("the motion advances");
        }
        // The page turn itself is the frame that shows the next page; the press dies
        // with the frame it started against, which reaches the adapter the way every
        // pointer fact does, through the re-anchored pointer state.
        let mut next = frame_with(2, "ni'hao", 9);
        next.page.current = 2;
        assert!(adapter.apply_frame(&next));
        settle(adapter);
        adapter.apply_pointer(PointerState {
            highlighted: Some(0),
            hovered: None,
            pressed: None,
        });
        settle(adapter);
        let marks: Vec<bool> = (0..9)
            .map(|position| pressed_at(adapter, position).unwrap_or(true))
            .collect();
        (marks, adapter.window().get_press_scale())
    });
    assert!(
        marks.iter().all(|pressed| !pressed),
        "no cell of the new page draws the Active state"
    );
    assert!(
        (settled - 1.0).abs() < 1.0e-6,
        "the invalidated press rebounded to full size, got {settled}"
    );
}

#[test]
fn test_adapter_press_with_motion_disabled_lands_on_the_first_frame() {
    let (first, second, pressed) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        adapter.set_motion_enabled(false);
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(0),
            hovered: Some(1),
            pressed: Some(1),
        }));
        let first = adapter.advance(0.0).expect("the motion advances");
        let scale = adapter.window().get_press_scale();
        let second = adapter.advance(0.0).expect("the motion advances");
        (first, second, scale, pressed_at(adapter, 1))
    });
    assert!(
        !first,
        "the disabled path leaves nothing in flight, so the loop may block"
    );
    assert!(!second, "and stays idle on the frames after it");
    assert!(
        (pressed.0 - PRESS_SCALE_TO).abs() < 1.0e-6,
        "the pressed cell draws its pressed scale at once, got {}",
        pressed.0
    );
    assert_eq!(pressed.1, Some(true), "the Active flag is drawn as pressed");
}
