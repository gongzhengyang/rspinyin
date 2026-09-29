//! Unit tests for the pointer and wheel translation.
//!
//! Every test drives the state with constructed `SurfaceEvent`s and asserts the
//! `UiEvent`s that come back, so the suite runs without a display server, a
//! compositor or a real clock: the instant is always handed in, and the one test
//! that crosses into the event channel uses the same instant for every post.
//!
//! The fixture is a page of 64x36 cells in one row, laid out in container
//! coordinates the way the placement pass produces them, so a window coordinate is
//! always the container coordinate plus the shadow reserve.

use std::time::{Duration, Instant};

use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId,
    StatusStrip, UiFrame,
};

use crate::channel::{ChannelConfig, UiEventQueue};
use crate::geometry::{Desktop, Geometry, Panel, PlacementRequest, ScaleSnap, Screen, compute};
use crate::layout::ContainerSize;

use super::*;

/// The revision every fixture frame carries.
const REV: u32 = 7;

/// The shadow reserve the placement pass keeps on all four sides, in physical pixels.
const SHADOW: i32 = 32;

/// Left edge of the first cell, in container coordinates.
const FIRST_X: i32 = 8;

/// Distance between the left edges of two neighbouring cells.
const PITCH: i32 = 70;

/// Top edge of the cell row, in container coordinates: header, rule and padding.
const CELL_Y: i32 = 43;

/// Width of one cell, in physical pixels.
const CELL_W: i32 = 64;

/// Height of one cell, in physical pixels.
const CELL_H: i32 = 36;

/// A page of `count` cells starting at global index `first`, in one row.
///
/// The container is sized to hold exactly those cells, so a point outside the cells
/// is either in the header strip or outside the panel altogether.
fn geometry(first: u16, count: u16) -> Geometry {
    let width = u32::try_from(i32::from(count) * PITCH + FIRST_X).expect("the fixture fits");
    Geometry {
        window_pos: (100, 200),
        window_size: (width + 64, 151),
        container_offset: (SHADOW, SHADOW),
        container_size: (width, 87),
        placement: Placement::Below,
        clamped_x: false,
        clamped_y: false,
        hit_map: (0..count)
            .map(|slot| {
                (
                    RectI {
                        x: FIRST_X + i32::from(slot) * PITCH,
                        y: CELL_Y,
                        w: CELL_W.unsigned_abs(),
                        h: CELL_H.unsigned_abs(),
                    },
                    first + slot,
                )
            })
            .collect(),
        arrow: None,
        scale: ScaleSnap {
            value: 1.0,
            is_exact: true,
        },
        screen: ScreenId::new(0),
    }
}

/// The centre of the `slot`-th cell, in window coordinates.
fn centre(slot: i32) -> (i32, i32) {
    let x = FIRST_X + slot * PITCH + CELL_W / 2 + SHADOW;
    let y = CELL_Y + CELL_H / 2 + SHADOW;
    (x, y)
}

/// A point in the panel's header strip, which holds no candidate.
const HEADER: (i32, i32) = (SHADOW + 100, SHADOW + 10);

/// A point in the transparent shadow reserve, outside the panel.
const RESERVE: (i32, i32) = (5, 5);

fn motion(x: i32, y: i32) -> SurfaceEvent {
    SurfaceEvent::PointerMotion { x, y }
}

fn click(x: i32, y: i32, pressed: bool) -> SurfaceEvent {
    SurfaceEvent::PointerButton {
        x,
        y,
        button: 1,
        pressed,
    }
}

fn right(x: i32, y: i32, pressed: bool) -> SurfaceEvent {
    SurfaceEvent::PointerButton {
        x,
        y,
        button: 3,
        pressed,
    }
}

fn wheel(x: i32, y: i32, delta: i32) -> SurfaceEvent {
    SurfaceEvent::Axis {
        x,
        y,
        delta,
        horizontal: false,
    }
}

fn hover(index: Option<u16>) -> UiEvent {
    UiEvent::Hover {
        revision: REV,
        index,
    }
}

fn select(index: u16) -> UiEvent {
    UiEvent::Select {
        revision: REV,
        index,
        trigger: SelectTrigger::Mouse,
    }
}

/// Presses and releases the primary button at `at`.
fn click_at(
    state: &mut InteractionState,
    geometry: &Geometry,
    at: (i32, i32),
    now: Instant,
) -> Option<UiEvent> {
    state.translate_at(&click(at.0, at.1, true), REV, geometry, now);
    state.translate_at(&click(at.0, at.1, false), REV, geometry, now)
}

#[test]
fn test_translate_motion_onto_a_cell_reports_its_global_index() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(1);
    let event = state.translate_at(&motion(x, y), REV, &geometry, Instant::now());
    assert_eq!(event, Some(hover(Some(1))));
    assert_eq!(state.hovered(), Some(1));
}

#[test]
fn test_translate_motion_on_a_later_page_reports_the_global_index() {
    // The index the host sees runs across pages rather than restarting on each one,
    // so the first cell of the second page is not candidate zero.
    let geometry = geometry(9, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(0);
    let event = state.translate_at(&motion(x, y), REV, &geometry, Instant::now());
    assert_eq!(event, Some(hover(Some(9))));
}

#[test]
fn test_translate_motion_into_the_gap_reports_no_hover() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(0);
    assert_eq!(
        state.translate_at(&motion(x, y), REV, &geometry, now),
        Some(hover(Some(0)))
    );
    // The six pixels between two cells are part of the panel but not of a candidate.
    let gap = (FIRST_X + CELL_W + SHADOW, y);
    assert_eq!(
        state.translate_at(&motion(gap.0, gap.1), REV, &geometry, now),
        Some(hover(None))
    );
    assert_eq!(state.hovered(), None);
}

#[test]
fn test_translate_motion_into_the_header_reports_no_hover() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(0);
    assert_eq!(
        state.translate_at(&motion(x, y), REV, &geometry, now),
        Some(hover(Some(0)))
    );
    assert_eq!(
        state.translate_at(&motion(HEADER.0, HEADER.1), REV, &geometry, now),
        Some(hover(None))
    );
}

#[test]
fn test_translate_motion_in_the_shadow_reserve_reports_no_hover() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(0);
    assert_eq!(
        state.translate_at(&motion(x, y), REV, &geometry, now),
        Some(hover(Some(0)))
    );
    assert_eq!(
        state.translate_at(&motion(RESERVE.0, RESERVE.1), REV, &geometry, now),
        Some(hover(None))
    );
}

#[test]
fn test_translate_motion_that_stays_on_the_same_cell_is_ignored() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(0);
    assert_eq!(
        state.translate_at(&motion(x, y), REV, &geometry, now),
        Some(hover(Some(0)))
    );
    assert_eq!(
        state.translate_at(&motion(x + 2, y + 1), REV, &geometry, now),
        None,
        "the host already knows about this cell"
    );
    assert_eq!(state.hovered(), Some(0));
}

#[test]
fn test_translate_ten_motions_on_one_cell_produce_one_hover() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(0);
    let mut produced = Vec::new();
    for step in 0..10 {
        produced.push(state.translate_at(&motion(x + step, y), REV, &geometry, now));
    }
    assert_eq!(
        produced.iter().filter(|event| event.is_some()).count(),
        1,
        "a stationary pointer produces one hover, not one per sample"
    );
    assert_eq!(produced.first(), Some(&Some(hover(Some(0)))));
}

#[test]
fn test_translate_sweeping_across_cells_delivers_only_the_final_hover() {
    // The two halves of the hover contract meet here. This layer reports every cell
    // the pointer crosses, because each one is new information; the channel's
    // latest-wins slot and its throttle turn the sweep into a single delivery that
    // names the cell the pointer stopped on.
    let queue = UiEventQueue::new(&ChannelConfig::default());
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    for slot in 0..5 {
        let (x, y) = centre(slot);
        if let Some(event) = state.translate_at(&motion(x, y), REV, &geometry, now) {
            queue.post_hover(event, now);
        }
    }
    assert_eq!(queue.poll(Duration::ZERO), Some(hover(Some(4))));
    assert_eq!(
        queue.poll(Duration::ZERO),
        None,
        "one sweep is one delivery, not five"
    );
}

#[test]
fn test_translate_press_then_release_selects_the_pressed_cell() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(2);
    assert_eq!(
        state.translate_at(&click(x, y, true), REV, &geometry, now),
        None,
        "the press draws the Active state rather than producing an event"
    );
    assert_eq!(state.pressed(), Some(2));
    assert_eq!(
        state.translate_at(&click(x, y, false), REV, &geometry, now),
        Some(select(2))
    );
    assert_eq!(state.pressed(), None);
}

#[test]
fn test_translate_click_on_a_later_page_selects_the_global_index() {
    let geometry = geometry(9, 5);
    let mut state = InteractionState::new();
    let event = click_at(&mut state, &geometry, centre(1), Instant::now());
    assert_eq!(event, Some(select(10)));
}

#[test]
fn test_translate_click_on_the_shadow_reserve_selects_nothing() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    assert_eq!(
        click_at(&mut state, &geometry, RESERVE, Instant::now()),
        None
    );
    assert_eq!(state.pressed(), None, "the reserve is not a cell to press");
}

#[test]
fn test_translate_click_on_the_header_selects_nothing() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    assert_eq!(
        click_at(&mut state, &geometry, HEADER, Instant::now()),
        None
    );
    assert_eq!(state.pressed(), None);
}

#[test]
fn test_translate_release_on_a_different_cell_selects_nothing() {
    // A press and a release on different cells are a drag: the user changed their
    // mind mid-gesture, and committing the cell they left would be a wrong commit.
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(1);
    state.translate_at(&click(x, y, true), REV, &geometry, now);
    let (other_x, other_y) = centre(3);
    assert_eq!(
        state.translate_at(&click(other_x, other_y, false), REV, &geometry, now),
        None
    );
    assert_eq!(
        state.pressed(),
        None,
        "the gesture ended without a selection"
    );
}

#[test]
fn test_translate_release_without_a_press_selects_nothing() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(1);
    assert_eq!(
        state.translate_at(&click(x, y, false), REV, &geometry, Instant::now()),
        None,
        "a release with no press in this window is not a click on it"
    );
}

#[test]
fn test_translate_release_against_a_newer_frame_selects_nothing() {
    // The same index in a newer frame names a different candidate, so a press that
    // outlived its frame must not commit anything.
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(1);
    state.translate_at(&click(x, y, true), REV, &geometry, now);
    assert_eq!(
        state.translate_at(&click(x, y, false), REV + 1, &geometry, now),
        None
    );
}

#[test]
fn test_translate_second_click_inside_the_debounce_window_is_ignored() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let start = Instant::now();
    let at = centre(2);
    assert_eq!(click_at(&mut state, &geometry, at, start), Some(select(2)));
    assert_eq!(
        click_at(&mut state, &geometry, at, start + Duration::from_millis(50)),
        None,
        "a double click is one intent"
    );
    assert_eq!(state.debounced_clicks(), 1);
    assert_eq!(
        click_at(
            &mut state,
            &geometry,
            at,
            start + Duration::from_millis(150)
        ),
        Some(select(2)),
        "the window has elapsed, so the next click counts"
    );
    assert_eq!(
        state.debounced_clicks(),
        1,
        "only the suppressed click is counted"
    );
}

#[test]
fn test_translate_click_on_another_cell_inside_the_window_is_not_debounced() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let start = Instant::now();
    assert_eq!(
        click_at(&mut state, &geometry, centre(2), start),
        Some(select(2))
    );
    assert_eq!(
        click_at(
            &mut state,
            &geometry,
            centre(3),
            start + Duration::from_millis(10)
        ),
        Some(select(3)),
        "the debounce is per candidate, not global"
    );
    assert_eq!(state.debounced_clicks(), 0);
}

#[test]
fn test_translate_press_and_release_raise_the_repaint_flag() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(2);
    state.translate_at(&click(x, y, true), REV, &geometry, now);
    assert!(state.take_repaint(), "the press changes what is drawn");
    assert!(!state.take_repaint(), "the flag is taken once");
    state.translate_at(&click(x, y, false), REV, &geometry, now);
    assert!(state.take_repaint(), "the release clears the Active state");
}

#[test]
fn test_translate_right_button_press_dismisses_and_its_release_is_ignored() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(2);
    let expected = UiEvent::Dismiss {
        revision: REV,
        reason: DismissReason::OutsideClick,
    };
    assert_eq!(
        state.translate_at(&right(x, y, true), REV, &geometry, now),
        Some(expected)
    );
    assert_eq!(
        state.translate_at(&right(x, y, false), REV, &geometry, now),
        None,
        "one right click is one dismissal"
    );
}

#[test]
fn test_translate_pointer_leave_clears_the_hover_and_cancels_a_press() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(1);
    state.translate_at(&motion(x, y), REV, &geometry, now);
    state.translate_at(&click(x, y, true), REV, &geometry, now);
    assert_eq!(
        state.translate_at(&SurfaceEvent::PointerLeave, REV, &geometry, now),
        Some(hover(None))
    );
    assert_eq!(state.hovered(), None);
    assert_eq!(state.pressed(), None, "the gesture ended with the pointer");
    assert_eq!(
        state.translate_at(&SurfaceEvent::PointerLeave, REV, &geometry, now),
        None,
        "leaving twice says nothing new"
    );
}

#[test]
fn test_translate_pointer_enter_is_handled_like_a_motion() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(4);
    let entered = SurfaceEvent::PointerEnter { x, y };
    assert_eq!(
        state.translate_at(&entered, REV, &geometry, Instant::now()),
        Some(hover(Some(4)))
    );
}

#[test]
fn test_translate_wheel_down_pages_next() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(2);
    assert_eq!(
        state.translate_at(&wheel(x, y, 1), REV, &geometry, Instant::now()),
        Some(UiEvent::Page {
            revision: REV,
            dir: PageDir::Next
        })
    );
}

#[test]
fn test_translate_wheel_up_away_from_the_first_page_pages_prev() {
    let geometry = geometry(9, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(0);
    assert_eq!(
        state.translate_at(&wheel(x, y, -1), REV, &geometry, Instant::now()),
        Some(UiEvent::Page {
            revision: REV,
            dir: PageDir::Prev
        })
    );
}

#[test]
fn test_translate_wheel_up_on_the_first_page_dismisses() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(2);
    assert_eq!(
        state.translate_at(&wheel(x, y, -1), REV, &geometry, Instant::now()),
        Some(UiEvent::Dismiss {
            revision: REV,
            reason: DismissReason::ScrollUpEmpty
        })
    );
}

#[test]
fn test_translate_wheel_up_with_no_cells_dismisses() {
    // Nothing on the page means nothing above it either, so the gesture that closes
    // the window is the only one left.
    let geometry = geometry(0, 0);
    let mut state = InteractionState::new();
    assert_eq!(
        state.translate_at(
            &wheel(SHADOW + 2, SHADOW + 40, -1),
            REV,
            &geometry,
            Instant::now()
        ),
        Some(UiEvent::Dismiss {
            revision: REV,
            reason: DismissReason::ScrollUpEmpty
        })
    );
}

#[test]
fn test_translate_horizontal_wheel_is_ignored() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(2);
    let sideways = SurfaceEvent::Axis {
        x,
        y,
        delta: 1,
        horizontal: true,
    };
    assert_eq!(
        state.translate_at(&sideways, REV, &geometry, Instant::now()),
        None
    );
}

#[test]
fn test_translate_wheel_in_the_shadow_reserve_is_left_to_the_host() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    assert_eq!(
        state.translate_at(
            &wheel(RESERVE.0, RESERVE.1, 1),
            REV,
            &geometry,
            Instant::now()
        ),
        None,
        "a step outside the panel is not ours to act on"
    );
}

#[test]
fn test_translate_compositor_events_produce_nothing() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    for event in [
        SurfaceEvent::Resize { w: 284, h: 151 },
        SurfaceEvent::Scale { factor: 2.0 },
        SurfaceEvent::CloseRequested,
    ] {
        assert_eq!(state.translate_at(&event, REV, &geometry, now), None);
    }
}

#[test]
fn test_adopt_frame_re_hit_tests_a_pointer_that_is_not_moving() {
    let first = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(4);
    state.translate_at(&motion(x, y), REV, &first, Instant::now());
    assert_eq!(state.hovered(), Some(4));
    // The new page holds two cells, so the pointer now sits in the panel's padding.
    // The event names the frame just adopted, not the one the pointer was last hit
    // against: the host has to drop a hover that refers to a frame it replaced.
    assert_eq!(
        state.adopt_frame(REV + 1, &geometry(9, 2)),
        Some(UiEvent::Hover {
            revision: REV + 1,
            index: None
        })
    );
    assert_eq!(state.hovered(), None);
}

#[test]
fn test_adopt_frame_reports_the_new_index_of_the_cell_under_the_pointer() {
    let first = geometry(0, 5);
    let mut state = InteractionState::new();
    let (x, y) = centre(0);
    state.translate_at(&motion(x, y), REV, &first, Instant::now());
    // The same cell on the next page is a different candidate, and the host has to
    // hear about it even though the pointer never moved.
    assert_eq!(
        state.adopt_frame(REV + 1, &geometry(9, 5)),
        Some(UiEvent::Hover {
            revision: REV + 1,
            index: Some(9)
        })
    );
}

#[test]
fn test_adopt_frame_cancels_a_press_taken_against_the_old_frame() {
    let geometry = geometry(0, 5);
    let mut state = InteractionState::new();
    let now = Instant::now();
    let (x, y) = centre(2);
    state.translate_at(&click(x, y, true), REV, &geometry, now);
    assert_eq!(state.pressed(), Some(2));
    state.adopt_frame(REV + 1, &geometry);
    assert_eq!(state.pressed(), None);
    assert_eq!(
        state.translate_at(&click(x, y, false), REV + 1, &geometry, now),
        None
    );
}

/// The geometry the placement pass produces for five candidates under a caret in the
/// middle of a full-HD output, which is what the real callers hit test against.
fn placed_geometry() -> Geometry {
    let anchor = Anchor {
        cursor: RectI {
            x: 640,
            y: 400,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Auto,
    };
    let output = Screen {
        id: ScreenId::new(0),
        origin: (0, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let desktop = Desktop {
        screens: std::slice::from_ref(&output),
        primary: ScreenId::new(0),
    };
    let panel = Panel {
        size: ContainerSize {
            width: 360.0,
            height: 87.0,
        },
        cell_width: 64.0,
        columns: 5,
    };
    let frame = UiFrame {
        revision: REV,
        preedit: Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: Vec::new(),
        },
        candidates: (0..5)
            .map(|position| Candidate {
                index: u16::try_from(position + 1).unwrap_or(u16::MAX),
                text: String::from("candidate"),
                annotation: None,
                source: CandidateSource::Dict,
                score: 1.0,
                consumed_syllables: 1,
            })
            .collect(),
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor,
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
    };
    let metrics =
        crate::layout::metrics().expect("the component declares a readable metrics block");
    compute(&PlacementRequest::new(
        &anchor, desktop, panel, &frame, metrics,
    ))
}

#[test]
fn test_translate_click_through_the_placement_pass_selects_the_cell_it_drew() {
    // The hit map is in container coordinates and the pointer arrives in window
    // coordinates; this is the test that the conversion between them is the one the
    // placement pass expects.
    let geometry = placed_geometry();
    let (rect, index) = geometry.hit_map[1];
    assert_eq!(
        index, 1,
        "the second cell of the first page is the second candidate"
    );
    let x = rect.x + i32::try_from(rect.w / 2).unwrap_or(0) + geometry.container_offset.0;
    let y = rect.y + i32::try_from(rect.h / 2).unwrap_or(0) + geometry.container_offset.1;
    let mut state = InteractionState::new();
    assert_eq!(
        click_at(&mut state, &geometry, (x, y), Instant::now()),
        Some(select(1))
    );
}

#[test]
fn test_translate_click_in_the_reserve_of_a_placed_window_selects_nothing() {
    let geometry = placed_geometry();
    let mut state = InteractionState::new();
    let inside_the_reserve = (
        geometry.container_offset.0 - 1,
        geometry.container_offset.1 - 1,
    );
    assert_eq!(
        click_at(&mut state, &geometry, inside_the_reserve, Instant::now()),
        None
    );
}
