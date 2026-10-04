//! The adapter's motion: the staged hide, the appear/disappear springs and the
//! highlight's flight.
//!
//! Every test drives a real component on a real thread against the mock surface, and
//! reads the motion back through the properties the component holds — the same facts the
//! renderer draws from. The clock stays out: the tests step `FRAME_S` themselves.

use ime_types::Placement;

use crate::spring::{PAGE_SLIDE_DP, REST_POSITION_DP};

use super::PointerState;
use super::pixels::{cell_width, metrics};
use super::{Adapter, frame_with, with_adapter, with_mapped_adapter};

/// The frame rate the motion tests step at, in seconds per frame.
///
/// The design targets 144Hz and the integrator clamps a step to 1/60s, so stepping at the
/// target rate is what the durations the design quotes are measured against.
pub(super) const FRAME_S: f32 = 1.0 / 144.0;

/// Advances the adapter's motion until nothing is in flight and reports how many frames it
/// took.
///
/// The ceiling is what makes a motion that never settles fail the test rather than hang it.
pub(super) fn settle(adapter: &mut Adapter) -> u32 {
    let mut frames = 1u32;
    while adapter.advance(FRAME_S).expect("the motion advances") {
        frames += 1;
        assert!(frames < 1_000, "the window's motion must come to rest");
    }
    frames
}

/// The horizontal distance between two candidate cells, in logical pixels.
fn cell_step_x() -> f32 {
    cell_width() + metrics().grid_gap
}

/// The vertical distance between two rows of candidates, in logical pixels.
fn cell_step_y() -> f32 {
    metrics().cell_height + metrics().grid_gap
}

/// The highlight box's four components as the component holds them.
fn highlight_box(adapter: &Adapter) -> (f32, f32, f32, f32) {
    let window = adapter.window();
    (
        window.get_highlight_x(),
        window.get_highlight_y(),
        window.get_highlight_w(),
        window.get_highlight_h(),
    )
}

#[test]
fn test_adapter_appear_motion_settles_on_the_end_state_and_stops() {
    let (start, end, frames, idle) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 4)));
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        // The motion reaches the component on the first frame the loop advances, which is the
        // frame the window appears on.
        adapter.advance(0.0).expect("the motion advances");
        let window = adapter.window();
        let start = (window.get_window_scale(), window.get_window_opacity());
        let frames = settle(adapter);
        let idle = !adapter.advance(FRAME_S).expect("the motion advances")
            && !adapter.advance(FRAME_S).expect("the motion advances");
        let window = adapter.window();
        let end = (window.get_window_scale(), window.get_window_opacity());
        (start, end, frames, idle)
    });
    assert_eq!(start.0, 0.96, "the panel starts at 3.3.2's scale floor");
    assert_eq!(start.1, 0.0, "and fully transparent");
    assert_eq!(end, (1.0, 1.0), "and reaches full size and full opacity");
    // 110ms at 144Hz is 15.84 frames.
    assert!(
        (14..=18).contains(&frames),
        "the appear motion took {frames} frames, off the 110ms 3.3.2 allots it"
    );
    assert!(
        idle,
        "and a window at rest reports no deadline, which is what lets the loop block"
    );
}

#[test]
fn test_adapter_highlight_box_lands_on_the_highlighted_cell() {
    let (first, second) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let first = highlight_box(adapter);
        let visible = adapter.window().get_highlight_visible();
        // The seventh candidate sits on the second row, second column of a five-per-row page.
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(6),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        settle(adapter);
        let second = highlight_box(adapter);
        (first, (second, visible))
    });
    assert_eq!(
        first,
        (0.0, 0.0, cell_width(), metrics().cell_height),
        "the box rests on the first cell, at the grid's own origin"
    );
    assert_eq!(
        second.0,
        (
            cell_step_x(),
            cell_step_y(),
            cell_width(),
            metrics().cell_height
        ),
        "and one cell right and one row down for the seventh candidate"
    );
    assert!(
        second.1,
        "the box is drawn while a candidate carries the highlight"
    );
}

#[test]
fn test_adapter_highlight_slides_between_cells_instead_of_jumping() {
    let (redirect, middle, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(4),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        adapter.advance(0.0).expect("the motion advances");
        let redirect = highlight_box(adapter).0;
        adapter.advance(FRAME_S).expect("the motion advances");
        let middle = highlight_box(adapter).0;
        settle(adapter);
        (redirect, middle, highlight_box(adapter).0)
    });
    assert_eq!(redirect, 0.0, "a redirect keeps the box where it is");
    assert!(
        middle > 0.0 && middle < 4.0 * cell_step_x(),
        "one frame moves the box part of the way, got {middle} of {}",
        4.0 * cell_step_x()
    );
    assert_eq!(
        end,
        4.0 * cell_step_x(),
        "and it comes to rest on the cell it was sent to"
    );
}

#[test]
fn test_adapter_motion_disabled_lands_on_the_end_values_at_once() {
    let (scale, opacity, offset, highlight_box, animating) = with_adapter(|adapter| {
        adapter.set_motion_enabled(false);
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(3),
            hovered: None,
            pressed: None,
        }));
        let animating = adapter.advance(FRAME_S).expect("the motion advances");
        let window = adapter.window();
        (
            window.get_window_scale(),
            window.get_window_opacity(),
            window.get_page_offset_dp(),
            highlight_box(adapter),
            animating,
        )
    });
    assert_eq!(scale, 1.0, "the appear motion is instantaneous");
    assert_eq!(opacity, 1.0);
    assert_eq!(offset, 0.0, "the page content is home");
    assert_eq!(
        highlight_box,
        (
            3.0 * cell_step_x(),
            0.0,
            cell_width(),
            metrics().cell_height
        ),
        "the box is already on the cell the highlight moved to"
    );
    assert!(!animating, "a disabled motion has nothing in flight");
}

#[test]
fn test_adapter_page_turn_slides_the_grid_and_returns_it_home() {
    let (before, displaced, home) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let before = adapter.window().get_page_offset_dp();
        let mut second = frame_with(2, "ni'hao", 9);
        second.page.current = 2;
        assert!(adapter.apply_frame(&second));
        adapter.advance(0.0).expect("the motion advances");
        let displaced = adapter.window().get_page_offset_dp();
        settle(adapter);
        (before, displaced, adapter.window().get_page_offset_dp())
    });
    assert_eq!(before, 0.0, "the first page draws no displacement");
    assert_eq!(
        displaced, PAGE_SLIDE_DP,
        "the next page enters displaced from the side it came from"
    );
    assert_eq!(home, 0.0, "and the content springs home");
}

#[test]
fn test_adapter_scale_anchor_follows_the_placement() {
    let (below, above, automatic) = with_adapter(|adapter| {
        adapter.set_placement(Placement::Below);
        let below = adapter.window().get_grows_upward();
        adapter.set_placement(Placement::Above);
        let above = adapter.window().get_grows_upward();
        adapter.set_placement(Placement::Auto);
        (below, above, adapter.window().get_grows_upward())
    });
    assert!(!below, "a panel below the caret grows from its top edge");
    assert!(above, "and one above grows upward from its bottom edge");
    assert!(!automatic, "the automatic side resolves to the one below");
}

#[test]
fn test_adapter_highlight_is_hidden_for_a_page_with_no_candidate() {
    let (visible, highlight_box) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni", 0)));
        adapter.advance(0.0).expect("the motion advances");
        (
            adapter.window().get_highlight_visible(),
            highlight_box(adapter),
        )
    });
    assert!(
        !visible,
        "nothing is highlighted when there is no candidate"
    );
    assert_eq!(
        highlight_box,
        (0.0, 0.0, 0.0, 0.0),
        "and the box collapses rather than drawing at a stale size"
    );
}

#[test]
fn test_adapter_highlight_hides_when_the_page_empties() {
    // The box hiding without moving is the case a frame-to-frame comparison of the motion
    // alone cannot see: the springs are already at rest on the cell, so the frame the step
    // produces is the one before it and the component would keep drawing the box.
    let (before, after) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let before = adapter.window().get_highlight_visible();
        assert!(adapter.apply_frame(&frame_with(2, "ni", 0)));
        adapter.advance(FRAME_S).expect("the motion advances");
        (before, adapter.window().get_highlight_visible())
    });
    assert!(
        before,
        "the box is drawn while a candidate carries the highlight"
    );
    assert!(
        !after,
        "and it is gone once the page has no candidate left to highlight"
    );
}

#[test]
fn test_adapter_highlight_approaches_its_target_monotonically() {
    let (start, backwards, frames, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let start = adapter.window().get_highlight_x();
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(1),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        adapter.advance(0.0).expect("the motion advances");
        let mut previous = adapter.window().get_highlight_x();
        let mut backwards = 0.0f32;
        let mut frames = 0u32;
        while adapter.advance(FRAME_S).expect("the motion advances") {
            let x = adapter.window().get_highlight_x();
            backwards = backwards.max(previous - x);
            previous = x;
            frames += 1;
            assert!(frames < 1_000, "the highlight must come to rest");
        }
        (start, backwards, frames, adapter.window().get_highlight_x())
    });
    assert_eq!(start, 0.0, "the box starts on the first cell");
    assert!(
        frames >= 4,
        "the box was written over only {frames} frames, which is too few for the \
         convergence to mean anything"
    );
    // The approach is monotone up to the band the spring calls arrived: a movement larger
    // than that would be the box visibly passing the cell it was sent to and coming back.
    assert!(
        backwards <= REST_POSITION_DP,
        "the box moved {backwards}px back off its target, more than the {}px band the spring \
         settles within",
        REST_POSITION_DP
    );
    assert_eq!(
        end,
        cell_step_x(),
        "and it lands exactly on the cell it was sent to"
    );
}

#[test]
fn test_adapter_highlight_survives_twenty_rapid_retargets_without_jumping() {
    // Twenty presses four frames apart, which is 28ms at the target refresh rate: the box is
    // redirected long before it ever arrives, which is what a held arrow key produces.
    const FRAMES_PER_PRESS: u32 = 4;
    let (widest, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let mut previous = adapter.window().get_highlight_x();
        let mut widest = 0.0f32;
        for press in 0..20u32 {
            // The user jitters the key between two neighbouring cells rather than walking in
            // one direction, so every redirect has to absorb a velocity it did not choose.
            let highlighted = if press % 2 == 0 { 1 } else { 0 };
            assert!(
                adapter.apply_pointer(PointerState {
                    highlighted: Some(highlighted),
                    hovered: None,
                    pressed: None,
                }),
                "moving the highlight redraws the grid"
            );
            for _ in 0..FRAMES_PER_PRESS {
                adapter.advance(FRAME_S).expect("the motion advances");
                let x = adapter.window().get_highlight_x();
                widest = widest.max((x - previous).abs());
                previous = x;
            }
        }
        settle(adapter);
        (widest, adapter.window().get_highlight_x())
    });
    assert!(
        widest <= cell_step_x() * 0.2,
        "one frame moved the box {widest}px of the {}px the box had to cross, which is a jump \
         rather than a slide",
        cell_step_x()
    );
    // The twentieth press is odd, so the last target is the cell the box started on.
    assert_eq!(end, 0.0, "the box comes to rest on the last target");
}

#[test]
fn test_adapter_motion_disabled_mid_flight_writes_the_end_values_at_once() {
    let (in_flight, before, after, animating) = with_adapter(|adapter| {
        // The window is shown first, so the appear motion is running and the switch has
        // something to snap: an adapter nobody has shown sits at opacity zero, and
        // asserting the end state of a motion that never started proves nothing.
        adapter.set_visible(true).expect("the window shows");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        // The retarget is the frame's word now: the second frame turns the page and
        // names another cell, so both the page slide and the highlight box have
        // somewhere to go when the switch goes off.
        let mut second = frame_with(2, "ni'hao", 9);
        second.page.current = 2;
        second.highlight = Some(6);
        assert!(adapter.apply_frame(&second));
        let in_flight = adapter.advance(0.0).expect("the motion advances");
        let before = (
            adapter.window().get_page_offset_dp(),
            highlight_box(adapter).0,
        );
        adapter.set_motion_enabled(false);
        let animating = adapter.advance(FRAME_S).expect("the motion advances");
        let window = adapter.window();
        let after = (
            window.get_page_offset_dp(),
            window.get_window_opacity(),
            window.get_window_scale(),
            highlight_box(adapter),
        );
        (in_flight, before, after, animating)
    });
    assert!(
        in_flight,
        "the page slide and the highlight are both in flight when the switch goes off"
    );
    assert_eq!(
        before,
        (PAGE_SLIDE_DP, 0.0),
        "the content is displaced and the box has not left the first cell"
    );
    assert_eq!(after.0, 0.0, "the content is home");
    assert_eq!(after.1, 1.0, "the window is fully opaque");
    assert_eq!(after.2, 1.0, "and at its full size");
    assert_eq!(
        after.3,
        (
            cell_step_x(),
            cell_step_y(),
            cell_width(),
            metrics().cell_height
        ),
        "and the box is already on the cell the highlight moved to"
    );
    assert!(!animating, "a disabled motion has nothing left in flight");
}

#[test]
fn test_adapter_set_visible_hides_only_after_the_staged_fade_settles() {
    let (shown, mapped_mid_fade, frames, hidden, idle) = with_mapped_adapter(|adapter, state| {
        let mapped = || state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
        settle(adapter);
        let shown = mapped();
        adapter.set_visible(false).expect("the unmap is staged");
        // Staged, not executed: the window is still on screen here, which is the
        // whole point -- an exit fade needs a mapped window to be a fade at all.
        let mapped_mid_fade = mapped();
        let mut frames = 0u32;
        while adapter.advance(FRAME_S).expect("the motion advances") {
            frames += 1;
            assert!(frames < 1_000, "the exit fade must come to rest");
        }
        let hidden = mapped();
        let idle = !adapter.advance(FRAME_S).expect("the motion advances")
            && !adapter.advance(FRAME_S).expect("the motion advances");
        (shown, mapped_mid_fade, frames, hidden, idle)
    });
    assert!(shown, "showing the window maps the surface");
    assert!(
        mapped_mid_fade,
        "a staged Hide keeps the window mapped while the exit fade runs"
    );
    assert!(
        (10..=20).contains(&frames),
        "the fade took {frames} frames, off the 90ms 3.3.2 allots the disappear"
    );
    assert!(
        !hidden,
        "the advance that reports the motion over is the one that unmapped the surface"
    );
    assert!(
        idle,
        "and past it the adapter reports no deadline, which is what lets the loop block"
    );
}

#[test]
fn test_adapter_hide_fades_the_opacity_down_while_mapped() {
    let (full, mid, mapped_mid_fade, end, unmapped) = with_mapped_adapter(|adapter, state| {
        let mapped = || state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
        settle(adapter);
        let full = adapter.window().get_window_opacity();
        adapter.set_visible(false).expect("the unmap is staged");
        adapter.advance(FRAME_S).expect("the motion advances");
        adapter.advance(FRAME_S).expect("the motion advances");
        let mid = adapter.window().get_window_opacity();
        let mapped_mid_fade = mapped();
        settle(adapter);
        let end = adapter.window().get_window_opacity();
        (full, mid, mapped_mid_fade, end, !mapped())
    });
    assert_eq!(full, 1.0, "the window the fade starts from is fully opaque");
    assert!(
        mid > 0.0 && mid < 1.0,
        "two frames into the exit the fade is part way down, got {mid}"
    );
    assert!(
        mapped_mid_fade,
        "the fading window is still mapped: the fade runs on screen, not in the dark"
    );
    assert_eq!(
        end, 0.0,
        "the fade rests fully transparent before the unmap"
    );
    assert!(unmapped, "and the unmap follows it on the same advance");
}

#[test]
fn test_adapter_show_during_the_staged_fade_cancels_the_unmap_and_resumes() {
    let (mid, kept, rising, end_opacity, mapped) = with_mapped_adapter(|adapter, state| {
        let mapped = || state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
        settle(adapter);
        adapter.set_visible(false).expect("the unmap is staged");
        for _ in 0..4 {
            adapter.advance(FRAME_S).expect("the motion advances");
        }
        let mid = adapter.window().get_window_opacity();
        adapter
            .set_visible(true)
            .expect("the Show lands during the fade");
        let kept = adapter.window().get_window_opacity();
        adapter.advance(FRAME_S).expect("the motion advances");
        let rising = adapter.window().get_window_opacity();
        settle(adapter);
        (
            mid,
            kept,
            rising,
            adapter.window().get_window_opacity(),
            mapped(),
        )
    });
    assert!(
        mid > 0.0 && mid < 1.0,
        "the fade was part way down when the Show landed, got {mid}"
    );
    assert_eq!(kept, mid, "the reversal keeps the opacity it had reached");
    assert!(
        rising > mid,
        "and the fade is on its way back up, got {rising}"
    );
    assert_eq!(
        end_opacity, 1.0,
        "the resumed appear completes at full opacity"
    );
    assert!(
        mapped,
        "the window was never unmapped: the staged Hide was cancelled, not executed"
    );
}

#[test]
fn test_adapter_repeated_hide_while_fading_restarts_nothing() {
    let (before, after, frames, hidden) = with_mapped_adapter(|adapter, state| {
        let mapped = || state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
        settle(adapter);
        adapter.set_visible(false).expect("the unmap is staged");
        for _ in 0..3 {
            adapter.advance(FRAME_S).expect("the motion advances");
        }
        let before = adapter.window().get_window_opacity();
        // The second Hide asks for what is already happening, and must change nothing
        // about the fade in flight: no restart, no snap, one unmap at the end.
        adapter.set_visible(false).expect("the staged unmap stands");
        let after = adapter.window().get_window_opacity();
        let mut frames = 0u32;
        while adapter.advance(FRAME_S).expect("the motion advances") {
            frames += 1;
            assert!(frames < 1_000, "the exit fade must come to rest");
        }
        (before, after, frames, !mapped())
    });
    assert_eq!(
        before, after,
        "a repeated Hide neither snaps nor restarts the fade"
    );
    assert!(
        (1..1_000).contains(&frames),
        "the fade still ran to its own end, over {frames} frames"
    );
    assert!(hidden, "and it still ends in the one unmap");
}

#[test]
fn test_adapter_hide_with_motion_disabled_unmaps_on_the_first_advance() {
    let (shown, mapped_before, animating, unmapped, idle) =
        with_mapped_adapter(|adapter, state| {
            let mapped = || state.lock().expect("the mock is not poisoned").visible;
            adapter.set_motion_enabled(false);
            adapter
                .set_visible(true)
                .expect("the surface can be mapped");
            assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
            let shown = mapped();
            adapter.set_visible(false).expect("the unmap is staged");
            let mapped_before = mapped();
            // The disabled fade settles inside its first step, so the unmap completes
            // on that step too: the hide is direct, with no drawn frames spent on it.
            let animating = adapter.advance(FRAME_S).expect("the motion advances");
            let unmapped = !mapped();
            let idle = !adapter.advance(FRAME_S).expect("the motion advances");
            (shown, mapped_before, animating, unmapped, idle)
        });
    assert!(shown, "the window was mapped before the Hide");
    assert!(
        mapped_before,
        "staging keeps the window mapped even with the motion off"
    );
    assert!(!animating, "an instant fade settles inside its first step");
    assert!(unmapped, "the first advance carries the direct unmap");
    assert!(idle, "and the adapter is at rest right after it");
}

#[test]
fn test_adapter_unmap_now_unmaps_mid_fade_and_is_idempotent() {
    let (mapped_mid_fade, unmapped, second, idle) = with_mapped_adapter(|adapter, state| {
        let mapped = || state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 3)));
        settle(adapter);
        adapter.set_visible(false).expect("the unmap is staged");
        adapter.advance(FRAME_S).expect("the motion advances");
        let mapped_mid_fade = mapped();
        // The shutdown path: whatever the staged fade was doing, the window leaves now.
        adapter.unmap_now().expect("the surface can be unmapped");
        let unmapped = !mapped();
        // A close that lands twice is still one unmap.
        adapter.unmap_now().expect("a second close is a no-op");
        let second = !mapped();
        let idle = !adapter.advance(FRAME_S).expect("the motion advances");
        (mapped_mid_fade, unmapped, second, idle)
    });
    assert!(
        mapped_mid_fade,
        "the fade was still running when the close landed"
    );
    assert!(
        unmapped,
        "the close unmaps immediately, without waiting for the fade"
    );
    assert!(
        second,
        "and it stays unmapped: the second close unmapped nothing"
    );
    assert!(idle, "the adapter is at rest after the immediate unmap");
}
