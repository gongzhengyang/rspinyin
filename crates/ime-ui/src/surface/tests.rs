//! Unit tests for the candidate surface.
//!
//! Every test builds a real surface over the mock backend and drives it the way the UI loop
//! does, so the assertions are about what the surface did rather than about what its code says.
//! The surface is built on a thread of its own, because Slint installs one platform per thread
//! and the component it holds is not `Send`. No test needs a display server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ime_types::{HideReason, SelectTrigger, SurfaceEvent, UiEvent};

use super::*;
use crate::adapter::tests::{anchor, frame_with};
use crate::channel::ChannelConfig;
use crate::renderer::mock::{MockState, MockSurface, on_own_thread};

/// The surface the tests draw into, in logical pixels at a scale of 1.0.
const SURFACE_WIDTH_DP: u32 = 320;
const SURFACE_HEIGHT_DP: u32 = 160;

/// Bytes per row of the mock's buffer at a scale of 1.0.
const STRIDE: usize = SURFACE_WIDTH_DP as usize * 4;

/// Builds a surface on a fresh thread and runs `scene` against it.
///
/// The scene runs on its own thread because Slint installs one platform per thread, and it
/// returns plain values because the component the surface holds is not `Send`.
fn with_surface<R: Send + 'static>(
    scene: impl FnOnce(&mut CandidateSurface, Arc<Mutex<MockState>>) -> R + Send + 'static,
) -> R {
    on_own_thread(move || {
        let (backend, state) = MockSurface::new(SURFACE_WIDTH_DP, SURFACE_HEIGHT_DP, 1.0);
        let mut surface = CandidateSurface::new(Box::new(backend)).expect("the surface starts");
        scene(&mut surface, state)
    })
}

/// Shows the window, draws one frame of `count` candidates and rasterizes it.
fn show_and_draw(surface: &mut CandidateSurface, count: usize) {
    surface
        .apply(SurfaceUpdate::Show {
            revision: 1,
            anchor: anchor(),
        })
        .expect("the window can be shown");
    surface
        .apply(SurfaceUpdate::Frame(Box::new(frame_with(
            1, "ni'hao", count,
        ))))
        .expect("the frame is applied");
    surface.render(Instant::now()).expect("the frame is drawn");
}

/// Renders until the scene stops changing, so a sampled frame is the settled one.
/// Runs frames until the appear motion has settled, and answers the instant it did.
///
/// The instant is stepped rather than read: the motion advances by the difference
/// between two frames, so rendering four times against a real clock a microsecond apart
/// would move it by nothing and leave the panel at its starting scale. The returned
/// instant is the one the caller's next frame has to follow.
fn settle(surface: &mut CandidateSurface) -> Instant {
    let mut now = Instant::now();
    let mut moved = false;
    for step in 0..64 {
        now += Duration::from_millis(8);
        let due = surface.render(now).expect("a settling frame is drawn");
        moved |= due.is_some();
        assert!(
            due.is_none() || step < 63,
            "the appear motion has not settled after 512ms"
        );
    }
    assert!(moved, "the appear motion never ran");
    now
}

#[test]
fn test_surface_show_frame_hide_commits_and_unmaps() {
    let (committed, starved, shown, hidden) = with_surface(|surface, state| {
        show_and_draw(surface, 9);
        let committed = surface.committed_frames();
        let starved = surface.starvation_streak();
        let shown = state.lock().expect("the mock is not poisoned").visible;
        surface
            .apply(SurfaceUpdate::Hide {
                revision: 2,
                reason: HideReason::Committed,
            })
            .expect("the window can be hidden");
        surface.render(Instant::now()).expect("the hide is handled");
        let hidden = state.lock().expect("the mock is not poisoned").visible;
        (committed, starved, shown, hidden)
    });
    assert!(committed > 0, "the frame reaches the surface");
    assert_eq!(starved, 0, "the mock always has a free buffer");
    assert!(shown, "showing the window maps the surface");
    assert!(!hidden, "hiding it unmaps the surface");
}

#[test]
fn test_surface_draws_a_non_empty_panel_and_leaves_the_reserve_transparent() {
    let (centre_alpha, reserve_alpha, region) = with_surface(|surface, state| {
        show_and_draw(surface, 1);
        settle(surface);
        let region = surface.input_region().expect("the panel was placed");
        let state = state.lock().expect("the mock is not poisoned");
        let centre = state.pixel(
            STRIDE,
            (region.x + region.w as i32 / 2) as usize,
            (region.y + region.h as i32 / 2) as usize,
        );
        let reserve = state.pixel(STRIDE, 1, 1);
        (centre[3], reserve[3], region)
    });
    assert!(
        centre_alpha > 0,
        "the panel is painted, and {region:?} is where it was placed"
    );
    assert_eq!(
        reserve_alpha, 0,
        "the shadow reserve is transparent, so nothing is drawn outside the panel"
    );
}

#[test]
fn test_surface_applies_the_panel_as_the_interactive_region() {
    let (region, recorded) = with_surface(|surface, state| {
        show_and_draw(surface, 1);
        let region = surface.input_region().expect("the panel was placed");
        let recorded = state
            .lock()
            .expect("the mock is not poisoned")
            .region
            .clone();
        (region, recorded)
    });
    assert_eq!(
        region,
        RectI {
            x: 32,
            y: 32,
            w: 220,
            h: 88
        },
        "the panel is inset by the 32dp shadow reserve on every side, and the placement \
         rounds its extent up to an even number"
    );
    assert_eq!(
        recorded,
        vec![region],
        "the reserve stays out of the interactive region, so a click there reaches the \
         application underneath"
    );
}

#[test]
fn test_surface_drops_a_frame_older_than_the_one_it_draws() {
    let (before, after) = with_surface(|surface, _state| {
        show_and_draw(surface, 1);
        let before = surface.input_region();
        surface
            .apply(SurfaceUpdate::Frame(Box::new(frame_with(0, "ni", 9))))
            .expect("the frame is handled");
        (before, surface.input_region())
    });
    assert_eq!(
        before, after,
        "an older frame does not resize the panel the window is drawing"
    );
}

#[test]
fn test_surface_idle_render_reports_no_deadline_and_commits_nothing() {
    let (idle, before, after) = with_surface(|surface, state| {
        show_and_draw(surface, 9);
        let settled = settle(surface);
        let before = state.lock().expect("the mock is not poisoned").commits;
        let first = surface.render(settled).expect("an idle frame is handled");
        // The budget's probe watches ten seconds of a still window; what a unit test can
        // assert is the same claim over a window it can afford to wait out. The instant
        // is stepped rather than slept through, so the claim is about the motion and not
        // about how long the test took.
        let second = surface
            .render(settled + Duration::from_millis(50))
            .expect("an idle frame is handled");
        let after = state.lock().expect("the mock is not poisoned").commits;
        (first.is_none() && second.is_none(), before, after)
    });
    assert!(
        idle,
        "an idle surface reports no deadline, which is what lets the loop block"
    );
    assert_eq!(after, before, "an idle surface commits nothing");
}

#[test]
fn test_surface_click_inside_a_cell_posts_a_select() {
    let (selected, from_header, index) = with_surface(|surface, state| {
        show_and_draw(surface, 3);
        let events = UiEventQueue::new(&ChannelConfig::default());
        let region = surface.input_region().expect("the panel was placed");
        let (cell, index) = surface.hit_map()[0];
        let centre_x = region.x + cell.x + cell.w as i32 / 2;
        let centre_y = region.y + cell.y + cell.h as i32 / 2;
        click(&state, centre_x, centre_y);
        surface
            .drain_events(&events, 8)
            .expect("the click is delivered");
        let selected = events.poll(Duration::ZERO);
        // The header carries the preedit, not a candidate.
        click(&state, region.x + 2, region.y + 2);
        surface
            .drain_events(&events, 8)
            .expect("the click is delivered");
        let from_header = events.poll(Duration::ZERO);
        (selected, from_header, index)
    });
    assert_eq!(
        selected,
        Some(UiEvent::Select {
            revision: 1,
            index,
            trigger: SelectTrigger::Mouse
        }),
        "a click on a cell selects the candidate the hit map names"
    );
    assert_eq!(from_header, None, "the header is not a candidate");
}

#[test]
fn test_surface_pointer_motion_posts_a_hover() {
    let hovered = with_surface(|surface, state| {
        show_and_draw(surface, 3);
        let events = UiEventQueue::new(&ChannelConfig::default());
        let region = surface.input_region().expect("the panel was placed");
        let (cell, _) = surface.hit_map()[0];
        state
            .lock()
            .expect("the mock is not poisoned")
            .pending
            .push(SurfaceEvent::PointerMotion {
                x: region.x + cell.x + 1,
                y: region.y + cell.y + 1,
            });
        surface
            .drain_events(&events, 8)
            .expect("the motion is delivered");
        events.poll(Duration::ZERO)
    });
    assert!(
        matches!(hovered, Some(UiEvent::Hover { index: Some(0), .. })),
        "the pointer over a cell hovers that candidate, got {hovered:?}"
    );
}

#[test]
fn test_surface_without_a_frame_has_no_region_and_no_hit_map() {
    let (region, hits, geometry_events) = with_surface(|surface, _state| {
        let events = UiEventQueue::new(&ChannelConfig::default());
        let geometry_events = surface
            .drain_events(&events, 8)
            .map(|()| events.poll(Duration::ZERO));
        (
            surface.input_region(),
            surface.hit_map().len(),
            geometry_events,
        )
    });
    assert_eq!(region, None, "nothing is placed before the first frame");
    assert_eq!(hits, 0, "and nothing can be hit");
    assert!(
        matches!(geometry_events, Ok(None)),
        "an empty backend posts nothing"
    );
}

/// Pushes a left button press and its release at a window-relative position.
///
/// The pair rather than the press alone: a click is a gesture, and the selection is the
/// release that completes it on the cell the press started on. A press on its own draws the
/// `Active` state and is deliberately not an event.
fn click(state: &Arc<Mutex<MockState>>, x: i32, y: i32) {
    let mut state = state.lock().expect("the mock is not poisoned");
    for pressed in [true, false] {
        state.pending.push(SurfaceEvent::PointerButton {
            x,
            y,
            button: 1,
            pressed,
        });
    }
}

/// Pushes a pointer motion to a window-relative position.
fn motion(state: &Arc<Mutex<MockState>>, x: i32, y: i32) {
    state
        .lock()
        .expect("the mock is not poisoned")
        .pending
        .push(SurfaceEvent::PointerMotion { x, y });
}

#[test]
fn test_surface_hover_repaints_the_window() {
    // The defect this pins is one only real rendering can catch: a hover that is written
    // into the grid's model but never drawn leaves the user with no feedback at all, and
    // "the frame was drawn" is not the same claim as "this frame differs from the last".
    let (before, after, hovered) = with_surface(|surface, state| {
        show_and_draw(surface, 3);
        // The appear motion is run out first: a panel still growing changes every pixel, and
        // the comparison would then be about the motion rather than about the hover.
        let settled = settle(surface);
        let before = state
            .lock()
            .expect("the mock is not poisoned")
            .pixels
            .clone();
        let events = UiEventQueue::new(&ChannelConfig::default());
        let region = surface.input_region().expect("the panel was placed");
        // The second cell, because the first carries the keyboard highlight: the design
        // ranks the focus ring above a hover, so hovering the highlighted cell would draw
        // nothing new.
        let (cell, index) = surface.hit_map()[1];
        let x = region.x + cell.x + cell.w as i32 / 2;
        let y = region.y + cell.y + cell.h as i32 / 2;
        motion(&state, x, y);
        surface
            .drain_events(&events, 8)
            .expect("the motion is delivered");
        let hovered = events.poll(Duration::ZERO);
        surface
            .render(settled + Duration::from_millis(16))
            .expect("the hovered frame is drawn");
        let after = state
            .lock()
            .expect("the mock is not poisoned")
            .pixels
            .clone();
        (before, after, (hovered, index))
    });
    assert_eq!(
        hovered.0,
        Some(UiEvent::Hover {
            revision: 1,
            index: Some(hovered.1)
        }),
        "the motion reaches the host as a hover on the cell it landed on"
    );
    assert!(
        !before.is_empty(),
        "the window drew a frame for the comparison to be about"
    );
    assert_eq!(before.len(), after.len(), "the panel did not change size");
    assert_ne!(
        before, after,
        "a hover changes the pixels the window draws, rather than only the model behind them"
    );
}
