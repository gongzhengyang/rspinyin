//! Unit tests for the candidate surface.
//!
//! Every test builds a real surface over the mock backend and drives it the way the UI loop
//! does, so the assertions are about what the surface did rather than about what its code says.
//! The surface is built on a thread of its own, because Slint installs one platform per thread
//! and the component it holds is not `Send`. No test needs a display server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ime_types::ui::{OverlayEntry, OverlayFrame, OverlayKind, OverlaySection};
use ime_types::{Anchor, HideReason, Placement, ScreenId, SelectTrigger, SurfaceEvent, UiEvent};

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

/// The anchor fixture at a device pixel ratio of the test's choosing: the shared fixture
/// pins the 1.0 case, and the scale-adoption scenes need the other supported ratios.
fn anchor_at(scale: f32) -> Anchor {
    Anchor {
        cursor: RectI {
            x: 100,
            y: 200,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale,
        placement: Placement::Auto,
    }
}

/// Shows the window at `scale`, draws one frame of `count` candidates and rasterizes it.
///
/// The frame's own anchor stays the 1.0 fixture on purpose: a `Show` overrides it, which
/// is the same rule the placement pass applies, and what keeps a re-scaled session from
/// being dragged back by the frames that follow it.
fn show_at(surface: &mut CandidateSurface, revision: u32, scale: f32, count: usize) {
    surface
        .apply(SurfaceUpdate::Show {
            revision,
            anchor: anchor_at(scale),
        })
        .expect("the window can be shown");
    surface
        .apply(SurfaceUpdate::Frame(Box::new(frame_with(
            revision, "ni'hao", count,
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
fn test_surface_hide_fades_while_mapped_then_unmaps_and_goes_idle() {
    let (committed, shown, mapped_mid_fade, fading, hidden, idle, idle_commits) =
        with_surface(|surface, state| {
            show_and_draw(surface, 9);
            let settled = settle(surface);
            let committed = surface.committed_frames();
            let shown = state.lock().expect("the mock is not poisoned").visible;
            surface
                .apply(SurfaceUpdate::Hide {
                    revision: 2,
                    reason: HideReason::Committed,
                })
                .expect("the window can be hidden");
            // One frame in: the window is still on screen and the exit fade reports a
            // deadline, which is the loop's signal to come back for the next frame.
            let due = surface
                .render(settled + Duration::from_millis(8))
                .expect("the first fade frame is drawn");
            let mapped_mid_fade = state.lock().expect("the mock is not poisoned").visible;
            // The fade runs to its own end: the frame that reports no deadline is the
            // one the window left the screen on, and past it the surface is idle.
            let mut now = settled + Duration::from_millis(8);
            for _ in 0..64 {
                now += Duration::from_millis(8);
                if surface
                    .render(now)
                    .expect("a fade frame is drawn")
                    .is_none()
                {
                    break;
                }
            }
            let hidden = !state.lock().expect("the mock is not poisoned").visible;
            let commits = state.lock().expect("the mock is not poisoned").commits;
            let first = surface
                .render(now + Duration::from_millis(8))
                .expect("an idle frame is handled");
            let second = surface
                .render(now + Duration::from_millis(58))
                .expect("an idle frame is handled");
            let idle = first.is_none() && second.is_none();
            let idle_commits = state.lock().expect("the mock is not poisoned").commits;
            (
                committed,
                shown,
                mapped_mid_fade,
                due.is_some(),
                hidden,
                idle,
                idle_commits - commits,
            )
        });
    assert!(committed > 0, "the frame reaches the surface");
    assert!(shown, "showing the window maps the surface");
    assert!(
        mapped_mid_fade,
        "the staged Hide keeps the window mapped while the exit fade runs"
    );
    assert!(
        fading,
        "the fade reports a deadline, so the loop comes back for its frames"
    );
    assert!(
        hidden,
        "the fade's last frame is the one that unmaps the surface"
    );
    assert!(
        idle,
        "past the fade the surface reports no deadline, which is what lets the loop block"
    );
    assert_eq!(
        idle_commits, 0,
        "an idle surface commits nothing: the fade's wakeups end with it"
    );
}

#[test]
fn test_surface_rapid_show_hide_alternations_leave_no_residual() {
    let (mapped_throughout, settled_hidden) = with_surface(|surface, state| {
        show_and_draw(surface, 9);
        let mut now = Instant::now();
        // A hundred Show/Hide pairs faster than any fade can run: every Show cancels the
        // staged unmap of the Hide before it, every Hide restages from wherever the fade
        // was, and neither may leave the window stuck half way or unmapped early.
        for step in 0..100u32 {
            surface
                .apply(SurfaceUpdate::Show {
                    revision: step * 2 + 2,
                    anchor: anchor(),
                })
                .expect("the window can be re-shown");
            now += Duration::from_millis(4);
            surface.render(now).expect("the show frame is drawn");
            surface
                .apply(SurfaceUpdate::Hide {
                    revision: step * 2 + 3,
                    reason: HideReason::Cancelled,
                })
                .expect("the window can be re-hidden");
            now += Duration::from_millis(4);
            surface.render(now).expect("the hide frame is drawn");
        }
        let mapped_throughout = state.lock().expect("the mock is not poisoned").visible;
        // The last word was a Hide, so its staged fade must still run out to the unmap.
        for _ in 0..64 {
            now += Duration::from_millis(8);
            if surface
                .render(now)
                .expect("a fade frame is drawn")
                .is_none()
            {
                break;
            }
        }
        let settled_hidden = !state.lock().expect("the mock is not poisoned").visible;
        (mapped_throughout, settled_hidden)
    });
    assert!(
        mapped_throughout,
        "the window stays on screen through the whole alternation: no Hide unmapped early"
    );
    assert!(
        settled_hidden,
        "and the last Hide wins: its staged fade runs out to the one unmap"
    );
}

#[test]
fn test_surface_close_unmaps_immediately_in_the_middle_of_a_fade() {
    let (mapped_mid_fade, hidden, second_clean) = with_surface(|surface, state| {
        show_and_draw(surface, 9);
        let settled = settle(surface);
        surface
            .apply(SurfaceUpdate::Hide {
                revision: 2,
                reason: HideReason::FocusLost,
            })
            .expect("the window can be hidden");
        surface
            .render(settled + Duration::from_millis(8))
            .expect("the first fade frame is drawn");
        let mapped_mid_fade = state.lock().expect("the mock is not poisoned").visible;
        // The shutdown lands while the fade is still running: the close unmaps at once
        // instead of waiting the fade out, which is the host's 200ms stop budget's due.
        surface.close().expect("the surface can be closed");
        let hidden = !state.lock().expect("the mock is not poisoned").visible;
        // A second close is the same no-op it has always been.
        surface.close().expect("a second close is a no-op");
        let second_clean = !state.lock().expect("the mock is not poisoned").visible;
        (mapped_mid_fade, hidden, second_clean)
    });
    assert!(
        mapped_mid_fade,
        "the fade was still running when the close landed"
    );
    assert!(
        hidden,
        "the close unmaps at once, without waiting the fade out"
    );
    assert!(second_clean, "and the surface stays unmapped");
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

/// One cheat-sheet frame, as the engine builds it from the bindings in force.
fn cheat_sheet() -> Box<OverlayFrame> {
    Box::new(OverlayFrame {
        kind: OverlayKind::CheatSheet,
        title: String::from("按键速查"),
        sections: vec![OverlaySection {
            title: String::from("编辑"),
            entries: vec![
                OverlayEntry {
                    keys: String::from("Esc"),
                    label: String::from("取消输入"),
                },
                OverlayEntry {
                    keys: String::from("Space"),
                    label: String::from("上屏"),
                },
            ],
        }],
        selected: None,
        query: String::new(),
    })
}

/// One command-palette frame, as a second panel that replaces the first.
fn command_palette() -> Box<OverlayFrame> {
    Box::new(OverlayFrame {
        kind: OverlayKind::CommandPalette,
        title: String::from("命令面板"),
        sections: Vec::new(),
        selected: Some(0),
        query: String::from("n"),
    })
}

#[test]
fn test_surface_overlay_frame_draws_over_the_panel_and_clearing_restores_it() {
    let (before, during, after, committed, retained, region) = with_surface(|surface, state| {
        show_and_draw(surface, 1);
        let settled = settle(surface);
        let before = state
            .lock()
            .expect("the mock is not poisoned")
            .pixels
            .clone();
        let region = surface.input_region().expect("the panel was placed");
        surface
            .apply(SurfaceUpdate::Overlay(Some(cheat_sheet())))
            .expect("the overlay opens");
        surface
            .render(settled + Duration::from_millis(16))
            .expect("the overlay frame is drawn");
        let committed = surface.committed_frames();
        let retained = surface.overlay().map(|frame| frame.title.to_string());
        let during = state
            .lock()
            .expect("the mock is not poisoned")
            .pixels
            .clone();
        surface
            .apply(SurfaceUpdate::Overlay(None))
            .expect("the overlay closes");
        surface
            .render(settled + Duration::from_millis(32))
            .expect("the closed frame is drawn");
        let after = state
            .lock()
            .expect("the mock is not poisoned")
            .pixels
            .clone();
        (before, during, after, committed, retained, region)
    });
    assert_eq!(
        retained.as_deref(),
        Some("按键速查"),
        "the drawn frame is retained beside the candidate frame"
    );
    assert!(committed > 0, "the overlay frame reached the surface");
    assert_ne!(
        before, during,
        "the overlay is a view of its own over the panel, not a copy of it"
    );
    // The change is inside the panel the region describes, not only somewhere in the
    // reserve: every pixel of the overlay's own background and text is panel-area.
    let mut differing = 0;
    for y in region.y as usize..(region.y + region.h as i32) as usize {
        for x in region.x as usize..(region.x + region.w as i32) as usize {
            let at = y * STRIDE + x * 4;
            if before[at..at + 4] != during[at..at + 4] {
                differing += 1;
            }
        }
    }
    assert!(
        differing > 0,
        "the overlay draws inside the panel at {region:?}"
    );
    assert_eq!(
        before, after,
        "clearing the overlay restores the candidate view byte for byte, with no \
         re-decode of the frame beneath it"
    );
}

#[test]
fn test_surface_overlay_retained_frames_follow_the_newest_command() {
    let retained = with_surface(|surface, _state| {
        let mut retained = Vec::new();
        surface
            .apply(SurfaceUpdate::Overlay(Some(cheat_sheet())))
            .expect("the panel opens");
        retained.push(surface.overlay().map(|frame| frame.title.to_string()));
        surface
            .apply(SurfaceUpdate::Overlay(Some(command_palette())))
            .expect("the panel is replaced");
        retained.push(surface.overlay().map(|frame| frame.title.to_string()));
        surface
            .apply(SurfaceUpdate::Overlay(None))
            .expect("the panel closes");
        retained.push(surface.overlay().map(|frame| frame.title.to_string()));
        retained
    });
    assert_eq!(
        retained,
        [
            Some(String::from("按键速查")),
            Some(String::from("命令面板")),
            None,
        ],
        "the surface retains the newest overlay state, and a close is a value of its own"
    );
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

/// The mock's buffer length at the birth size, and at the canvas re-expressed at 2.0 and
/// 1.25: the physical sizes every scale assertion below is written against.
const BIRTH_BYTES: usize = 320 * 160 * 4;
const TWICE_BYTES: usize = 640 * 320 * 4;
const QUARTER_AGAIN_BYTES: usize = 400 * 200 * 4;

#[test]
fn test_surface_show_at_anchor_scale_two_adopts_the_ratio_and_repaints_fully() {
    let (last_damage, pixels_len, resynthesized) = with_surface(|surface, state| {
        // A 1.0 session first: the first frame of a newborn window is always full, so it
        // would prove nothing about the adoption's own repaint.
        show_and_draw(surface, 3);
        let settled = settle(surface);
        let _ = surface.render(settled).expect("a settled frame is drawn");
        show_at(surface, 2, 2.0, 3);
        let locked = state.lock().expect("the mock is not poisoned");
        let last_damage = locked.damage.last().copied();
        let pixels_len = locked.pixels.len();
        drop(locked);
        // The synthesized events ride `pending` until they are drained; draining them
        // clears the queue the next assertion reads.
        let events = UiEventQueue::new(&ChannelConfig::default());
        surface
            .drain_events(&events, 8)
            .expect("the synthesized events are drained");
        surface
            .apply(SurfaceUpdate::Show {
                revision: 3,
                anchor: anchor_at(2.0),
            })
            .expect("the window can be re-shown");
        let resynthesized = !surface.pending.is_empty();
        (last_damage, pixels_len, resynthesized)
    });
    assert_eq!(
        pixels_len, TWICE_BYTES,
        "the surface runs at the anchor's ratio: the logical canvas at twice the device \
         pixel ratio"
    );
    assert_eq!(
        last_damage,
        Some(RectI {
            x: 0,
            y: 0,
            w: 640,
            h: 320
        }),
        "a scale change repaints the whole surface: the buffers the smaller ratio drew \
         describe a different grid"
    );
    assert!(
        !resynthesized,
        "a ratio the surface already runs at is adopted once, not once per keystroke"
    );
}

#[test]
fn test_surface_at_twice_the_ratio_clicks_land_on_the_cells_the_window_drew() {
    let (region, cell_alpha, reserve_alpha, selected) = with_surface(|surface, state| {
        show_at(surface, 1, 2.0, 1);
        settle(surface);
        let region = surface.input_region().expect("the panel was placed");
        let (cell, index) = surface.hit_map()[0];
        let centre = (
            (region.x + cell.x + cell.w as i32 / 2) as usize,
            (region.y + cell.y + cell.h as i32 / 2) as usize,
        );
        let stride = 640_usize * 4;
        let (cell_alpha, reserve_alpha) = {
            let state = state.lock().expect("the mock is not poisoned");
            (
                state.pixel(stride, centre.0, centre.1)[3],
                state.pixel(stride, 1, 1)[3],
            )
        };
        let events = UiEventQueue::new(&ChannelConfig::default());
        click(&state, centre.0 as i32, centre.1 as i32);
        surface
            .drain_events(&events, 8)
            .expect("the click is delivered");
        let selected = events.poll(Duration::ZERO);
        (region, cell_alpha, reserve_alpha, (selected, index))
    });
    assert_eq!(
        region,
        RectI {
            x: 64,
            y: 64,
            w: 440,
            h: 176
        },
        "the interactive region is the panel the placement computed at twice the ratio -- \
         the same fixture's 1.0 region, doubled"
    );
    assert!(
        cell_alpha > 0,
        "the cell the hit map names is painted where the map puts it, so hit rectangles \
         and drawn pixels are the same grid"
    );
    assert_eq!(
        reserve_alpha, 0,
        "the shadow reserve stays transparent at any ratio"
    );
    assert_eq!(
        selected.0,
        Some(UiEvent::Select {
            revision: 1,
            index: selected.1,
            trigger: SelectTrigger::Mouse
        }),
        "a click at twice the ratio selects the candidate the drawn cell names"
    );
}

#[test]
fn test_surface_anchor_scale_off_the_supported_set_adopts_the_nearest_ratio() {
    let (at_1_2, at_zero) = with_surface(|surface, state| {
        show_at(surface, 1, 1.2, 3);
        let at_1_2 = state.lock().expect("the mock is not poisoned").pixels.len();
        show_at(surface, 2, 0.0, 3);
        let at_zero = state.lock().expect("the mock is not poisoned").pixels.len();
        (at_1_2, at_zero)
    });
    assert_eq!(
        at_1_2, QUARTER_AGAIN_BYTES,
        "1.2 sits between two supported ratios and lands on the nearer one, 1.25"
    );
    assert_eq!(
        at_zero, BIRTH_BYTES,
        "a ratio no supported output can report falls back to 1.0, the birth size"
    );
}

#[test]
fn test_surface_scale_round_trip_returns_to_the_birth_size_and_settles_idle() {
    let (sizes, idle, commits_before, commits_after) = with_surface(|surface, state| {
        let mut sizes = Vec::new();
        for (revision, scale) in [(1, 2.0_f32), (2, 1.25), (3, 1.0)] {
            show_at(surface, revision, scale, 3);
            let pixels = state.lock().expect("the mock is not poisoned").pixels.len();
            sizes.push(pixels);
        }
        // The appear motion the first `Show` started is run out before the idle claim:
        // a window still animating reports a deadline, and would fail the assertion for
        // a reason the round trip has nothing to do with.
        let _ = settle(surface);
        let commits_before = state.lock().expect("the mock is not poisoned").commits;
        let first = surface
            .render(Instant::now())
            .expect("an idle frame is drawn");
        let second = surface
            .render(Instant::now())
            .expect("an idle frame is drawn");
        let commits_after = state.lock().expect("the mock is not poisoned").commits;
        (
            sizes,
            first.is_none() && second.is_none(),
            commits_before,
            commits_after,
        )
    });
    assert_eq!(
        sizes,
        [TWICE_BYTES, QUARTER_AGAIN_BYTES, BIRTH_BYTES],
        "every step of the round trip runs at the ratio it was sent, ending at the birth \
         size with no allocation left behind"
    );
    assert!(
        idle,
        "a round trip leaves no repaint owing: the window reports no deadline"
    );
    assert_eq!(
        commits_before, commits_after,
        "and a window at rest commits nothing further"
    );
}

#[test]
fn test_surface_resize_then_scale_keeps_one_canvas() {
    let pixels_len = with_surface(|surface, state| {
        let events = UiEventQueue::new(&ChannelConfig::default());
        // The compositor resized the window first: the mock adopts it into its own dp.
        state
            .lock()
            .expect("the mock is not poisoned")
            .pending
            .push(SurfaceEvent::Resize { w: 640, h: 320 });
        surface
            .drain_events(&events, 8)
            .expect("the configure is delivered");
        // The anchor then moves the surface to twice the ratio: the same canvas,
        // re-expressed.
        show_at(surface, 1, 2.0, 3);
        state.lock().expect("the mock is not poisoned").pixels.len()
    });
    assert_eq!(
        pixels_len,
        1280 * 640 * 4,
        "the adopted ratio re-expresses the canvas the resize created: 640x320 dp at \
         twice the ratio, which a stale scale would not have computed"
    );
}

#[test]
fn test_surface_scale_then_resize_keeps_one_canvas() {
    let pixels_len = with_surface(|surface, state| {
        show_at(surface, 1, 2.0, 3);
        // The compositor then reports the size the surface already runs at -- the echo
        // the adoption itself produces. Nothing may drift.
        let events = UiEventQueue::new(&ChannelConfig::default());
        state
            .lock()
            .expect("the mock is not poisoned")
            .pending
            .push(SurfaceEvent::Resize { w: 640, h: 320 });
        surface
            .drain_events(&events, 8)
            .expect("the configure is delivered");
        surface.render(Instant::now()).expect("the frame is drawn");
        state.lock().expect("the mock is not poisoned").pixels.len()
    });
    assert_eq!(
        pixels_len, TWICE_BYTES,
        "a configure that names the adopted size changes nothing: the canvas and the \
         ratio agree"
    );
}

#[test]
fn test_surface_scale_one_session_synthesizes_no_scale_event() {
    let (pending_empty, pixels_len) = with_surface(|surface, state| {
        show_and_draw(surface, 3);
        let pending_empty = surface.pending.is_empty();
        let pixels_len = state.lock().expect("the mock is not poisoned").pixels.len();
        (pending_empty, pixels_len)
    });
    assert!(
        pending_empty,
        "a session already at the anchor's ratio synthesizes no scale event"
    );
    assert_eq!(pixels_len, BIRTH_BYTES, "the birth size is kept");
}

/// The component's constants, for the pixel arithmetic the ring assertions do.
fn ring_metrics() -> &'static crate::layout::Metrics {
    crate::layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// The width of one cell of the shared fixture: two CJK glyphs and the chrome around them.
fn ring_cell_width() -> f32 {
    let metrics = ring_metrics();
    2.0 * metrics.font_size_cell + metrics.cell_chrome_width
}

/// The surface-relative top-left corner of the cell at `position`, at a ratio of 1.0.
///
/// The placement arithmetic restated for sampling: the panel is drawn at the shadow
/// margin, the header and its rule sit above the grid, and the grid pads by the
/// container padding. At a ratio of 1.0 a logical pixel is a physical one.
fn ring_cell_origin(position: usize) -> (usize, usize) {
    let metrics = ring_metrics();
    let gap = metrics.grid_gap as usize;
    let x = (metrics.shadow_margin + metrics.container_padding) as usize
        + position * (ring_cell_width() as usize + gap);
    let y = (metrics.shadow_margin
        + metrics.header_height
        + metrics.separator_height
        + metrics.container_padding) as usize;
    (x, y)
}

/// The strongest stroke blue along the top edge of the cell at `position`.
///
/// Only the focus ring strokes a cell -- a hovered cell's top edge is its own fill --
/// so the strongest edge sample is how the ringed cell is found without depending on
/// the exact row the stroke lands on.
fn strongest_edge_blue(state: &MockState, stride: usize, position: usize) -> u8 {
    let (x, y) = ring_cell_origin(position);
    let centre = x + ring_cell_width() as usize / 2;
    (y - 1..y + 3)
        .map(|row| state.pixel(stride, centre, row)[0])
        .max()
        .unwrap_or(0)
}

#[test]
fn test_surface_pointer_gesture_keeps_the_ring_on_the_frames_cell() {
    // The refresh a gesture triggers rebuilds the pointer state from the frame, so the
    // ring is the frame's word: a press and its release on another cell must not walk
    // it onto the cell they landed on. The frame puts the ring on the third cell; the
    // gesture lands on the first; the stroked cell afterwards is still the third.
    let (before, after) = with_surface(|surface, state| {
        let mut frame = frame_with(1, "ni'hao", 3);
        frame.highlight = Some(2);
        surface
            .apply(SurfaceUpdate::Show {
                revision: 1,
                anchor: anchor(),
            })
            .expect("the window can be shown");
        surface
            .apply(SurfaceUpdate::Frame(Box::new(frame)))
            .expect("the frame is applied");
        let settled = settle(surface);
        let _ = surface.render(settled).expect("the settled frame is drawn");

        let stride = SURFACE_WIDTH_DP as usize * 4;
        let read_strokes = |state: &Arc<Mutex<MockState>>| {
            let state = state.lock().expect("the mock is not poisoned");
            [
                strongest_edge_blue(&state, stride, 0),
                strongest_edge_blue(&state, stride, 1),
                strongest_edge_blue(&state, stride, 2),
            ]
        };
        let before = read_strokes(&state);

        let events = UiEventQueue::new(&ChannelConfig::default());
        let region = surface.input_region().expect("the panel was placed");
        let (cell, _) = surface.hit_map()[0];
        click(
            &state,
            region.x + cell.x + cell.w as i32 / 2,
            region.y + cell.y + cell.h as i32 / 2,
        );
        surface
            .drain_events(&events, 8)
            .expect("the gesture is delivered");
        surface
            .render(settled + Duration::from_millis(16))
            .expect("the repainted frame is drawn");
        (before, read_strokes(&state))
    });
    let stroked = |strokes: [u8; 3]| -> usize {
        let (position, _) = strokes
            .iter()
            .enumerate()
            .max_by_key(|(_, blue)| **blue)
            .expect("three cells were sampled");
        position
    };
    assert_eq!(
        stroked(before),
        2,
        "before the gesture the ring is where the frame put it"
    );
    assert!(
        before[2] > before[0] + 20,
        "the ring's stroke is distinguishable from an untouched edge: {:?}",
        before
    );
    assert_eq!(
        stroked(after),
        2,
        "a press and its release moved nothing: the stroked cell is still the frame's"
    );
    assert!(
        after[2] > after[0] + 20,
        "and the clicked cell draws no ring of its own: {:?}",
        after
    );
}
