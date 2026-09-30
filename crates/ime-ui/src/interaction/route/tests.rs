//! Unit tests for the event routing, and the surface-level wiring that consumes it.
//!
//! Every scene here runs without a display server. The tests build a placement with the real
//! placement pass and drive the real event queue, so what they assert is which channel an
//! event came out of rather than which method was called. The scenes that need to see pixels
//! live beside the surface, in `crate::surface`'s tests, where the mock backend's fixtures
//! already are.
//!
//! # The placement the fixtures use
//!
//! The panel is sized by the same layout pass the surface uses, from the component's own
//! constants, and placed on a full-HD output with the caret in the middle of it. That is
//! what makes the round-trip tests meaningful at every ratio: the grid the hit map is built
//! from is the grid the component draws, at the ratio the surface was created with.

use std::time::{Duration, Instant};

use ime_types::{
    Anchor, Candidate, CandidateSource, DismissReason, LayoutHint, PageDir, PageState, Placement,
    Preedit, RectI, ScreenId, SelectTrigger, StatusStrip, SurfaceEvent, UiEvent, UiFrame,
};

use crate::channel::{ChannelConfig, UiEventQueue};
use crate::geometry::{Desktop, Geometry, Panel, PlacementRequest, Screen, compute};
use crate::layout::{self, Metrics};

use super::{PointerRouter, RouteRequest};

/// The revision every fixture frame carries.
const REV: u32 = 11;

/// Candidates per row in the fixtures.
const COLUMNS: u8 = 5;

/// Candidates on the fixture page.
const CANDIDATES: usize = 9;

/// The ratios the round-trip tests cover.
///
/// The list is the design's supported set minus the 3.0 tier: the point of covering more
/// than one ratio is that the hit map is built from rounded physical strides, and a grid
/// that fits exactly at one ratio is the case that goes wrong at a fractional one.
const SCALES: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

/// The component's constants, parsed once.
fn metrics() -> &'static Metrics {
    layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// The panel a page of `count` candidates needs, sized the way the surface sizes it.
fn panel(count: usize) -> Panel {
    let metrics = metrics();
    let grid = layout::grid(count, 1, COLUMNS, metrics);
    // A candidate two glyphs wide, which is what the fixture frame holds.
    let measured = [2.0 * metrics.font_size_cell + metrics.cell_chrome_width];
    let cell = layout::cell_width(&measured, COLUMNS, metrics.max_width, metrics);
    Panel {
        size: layout::container_size(&grid, cell.width, metrics.max_width, metrics),
        cell_width: cell.width,
        columns: grid.cols.max(1),
    }
}

/// The caret the fixtures anchor to.
fn anchor(scale: f32) -> Anchor {
    Anchor {
        cursor: RectI {
            x: 640,
            y: 400,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale,
        placement: Placement::Auto,
    }
}

/// A frame of `count` candidates showing `page`.
fn frame(count: usize, page: PageState) -> UiFrame {
    let candidates = (0..count)
        .map(|position| Candidate {
            index: u16::try_from(position + 1).unwrap_or(u16::MAX),
            text: String::from("你好"),
            annotation: None,
            source: CandidateSource::Dict,
            score: 0.0,
            consumed_syllables: 1,
        })
        .collect();
    UiFrame {
        revision: REV,
        preedit: Preedit {
            text: String::from("ni'hao"),
            caret: 6,
            spans: Vec::new(),
        },
        candidates,
        page,
        status: StatusStrip::default(),
        anchor: anchor(1.0),
        layout: LayoutHint {
            max_per_row: COLUMNS,
            show_annotation: false,
            max_width_dp: 720,
        },
    }
}

/// The paging state of a one-based page of the fixture list.
fn page(current: u8) -> PageState {
    PageState {
        current,
        total: 2,
        page_size: COLUMNS,
    }
}

/// The placement of a page of candidates at `scale`, through the real pass.
fn placed(scale: f32, current: u8) -> Geometry {
    let anchor = anchor(scale);
    let output = Screen {
        id: ScreenId::new(0),
        origin: (0, 0),
        size: (1920, 1080),
        scale,
    };
    let desktop = Desktop {
        screens: std::slice::from_ref(&output),
        primary: ScreenId::new(0),
    };
    let frame = frame(CANDIDATES, page(current));
    compute(&PlacementRequest::new(
        &anchor,
        desktop,
        panel(CANDIDATES),
        &frame,
        metrics(),
    ))
}

/// The centre of the `position`-th cell, in window coordinates.
fn centre(geometry: &Geometry, position: usize) -> (i32, i32) {
    let (rect, _) = geometry.hit_map[position];
    let x = rect.x + geometry.container_offset.0 + i32::try_from(rect.w / 2).unwrap_or(0);
    let y = rect.y + geometry.container_offset.1 + i32::try_from(rect.h / 2).unwrap_or(0);
    (x, y)
}

fn press(at: (i32, i32)) -> SurfaceEvent {
    SurfaceEvent::PointerButton {
        x: at.0,
        y: at.1,
        button: 1,
        pressed: true,
    }
}

fn release(at: (i32, i32)) -> SurfaceEvent {
    SurfaceEvent::PointerButton {
        x: at.0,
        y: at.1,
        button: 1,
        pressed: false,
    }
}

fn motion(at: (i32, i32)) -> SurfaceEvent {
    SurfaceEvent::PointerMotion { x: at.0, y: at.1 }
}

fn wheel(at: (i32, i32), delta: i32) -> SurfaceEvent {
    SurfaceEvent::Axis {
        x: at.0,
        y: at.1,
        delta,
        horizontal: false,
    }
}

fn queue() -> UiEventQueue {
    UiEventQueue::new(&ChannelConfig::default())
}

/// Routes one event against `geometry`.
fn route(
    router: &mut PointerRouter,
    geometry: &Geometry,
    event: &SurfaceEvent,
    events: &UiEventQueue,
    now: Instant,
) -> Option<UiEvent> {
    router.route(
        RouteRequest {
            event,
            revision: REV,
            geometry: Some(geometry),
            now,
        },
        events,
    )
}

/// Presses and releases the primary button on the `position`-th cell.
fn click(
    router: &mut PointerRouter,
    geometry: &Geometry,
    position: usize,
    events: &UiEventQueue,
    now: Instant,
) -> Option<UiEvent> {
    let at = centre(geometry, position);
    assert_eq!(
        route(router, geometry, &press(at), events, now),
        None,
        "a press draws the Active state rather than producing an event"
    );
    route(router, geometry, &release(at), events, now)
}

/// Clicks the `position`-th cell and answers the global index the click named.
fn clicked_index(
    router: &mut PointerRouter,
    geometry: &Geometry,
    position: usize,
    events: &UiEventQueue,
    now: Instant,
) -> u16 {
    match click(router, geometry, position, events, now) {
        Some(UiEvent::Select { index, .. }) => index,
        other => panic!("a click on cell {position} selects nothing, got {other:?}"),
    }
}

#[test]
fn test_route_click_selects_the_cell_under_the_pointer() {
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let selected = click(&mut router, &geometry, 1, &events, Instant::now());
    assert_eq!(
        selected,
        Some(UiEvent::Select {
            revision: REV,
            index: 1,
            trigger: SelectTrigger::Mouse,
        }),
        "the click names the candidate the hit map pairs with the cell it landed on"
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        selected,
        "and it reached the host through the queue rather than only being returned"
    );
}

#[test]
fn test_route_click_and_the_number_key_name_the_same_candidate() {
    // The same-source rule: a click and the digit that labels the cell must name one
    // candidate. The two indices are computed from different inputs -- the hit map for the
    // click, the candidate's own number for the key -- so an agreement is a fact about the
    // numbering rather than about a conversion. The trigger is the one field the two paths
    // are meant to differ in.
    let geometry = placed(1.0, 1);
    let frame = frame(CANDIDATES, page(1));
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    // The first cell of the page carries the global index the page starts at.
    let page_start = geometry.hit_map[0].1;
    for (position, candidate) in frame.candidates.iter().enumerate() {
        let by_click = clicked_index(&mut router, &geometry, position, &events, now);
        // What the engine's `Paging::digit_target` answers for the key the cell labels:
        // the page's first index plus the digit, one-based.
        let by_key = page_start + candidate.index - 1;
        assert_eq!(
            by_click, by_key,
            "cell {position} is labelled {} and the click named {by_click}",
            candidate.index
        );
        let from_mouse = UiEvent::Select {
            revision: REV,
            index: by_click,
            trigger: SelectTrigger::Mouse,
        };
        let from_key = UiEvent::Select {
            revision: REV,
            index: by_key,
            trigger: SelectTrigger::NumberKey,
        };
        assert_eq!(
            normalise(from_mouse.clone()),
            normalise(from_key.clone()),
            "the two paths produce the same event once the trigger is set aside"
        );
        assert_ne!(
            from_mouse, from_key,
            "and the trigger is the only field they differ in"
        );
    }
}

/// An event with the trigger field removed, for comparing two paths to one candidate.
fn normalise(event: UiEvent) -> UiEvent {
    match event {
        UiEvent::Select {
            revision, index, ..
        } => UiEvent::Select {
            revision,
            index,
            trigger: SelectTrigger::Mouse,
        },
        other => other,
    }
}

#[test]
fn test_route_round_trips_every_cell_at_every_supported_scale() {
    // A hit map built from rounded physical strides can land on the neighbouring cell at a
    // fractional ratio, so every cell of a full page is walked at each one.
    for scale in SCALES {
        let geometry = placed(scale, 1);
        assert_eq!(
            geometry.hit_map.len(),
            CANDIDATES,
            "the page shows every candidate at {scale}"
        );
        let mut router = PointerRouter::new();
        let events = queue();
        let now = Instant::now();
        for position in 0..CANDIDATES {
            let expected = geometry.hit_map[position].1;
            let clicked = clicked_index(&mut router, &geometry, position, &events, now);
            assert_eq!(
                clicked, expected,
                "at {scale} the centre of cell {position} selects {clicked}, not {expected}"
            );
        }
    }
}

#[test]
fn test_route_pointer_in_the_shadow_band_selects_nothing() {
    for scale in SCALES {
        let geometry = placed(scale, 1);
        let mut router = PointerRouter::new();
        let events = queue();
        let now = Instant::now();
        // One pixel inside the window and outside the panel: the reserve is transparent and
        // a click there belongs to the application underneath.
        let reserve = (
            geometry.container_offset.0 - 1,
            geometry.container_offset.1 - 1,
        );
        let _ = route(&mut router, &geometry, &press(reserve), &events, now);
        assert_eq!(
            route(&mut router, &geometry, &release(reserve), &events, now),
            None,
            "a press in the reserve is not a press on a cell at {scale}"
        );
        assert_eq!(
            route(&mut router, &geometry, &motion(reserve), &events, now),
            None,
            "and it produces no hover, so nothing is highlighted"
        );
        assert_eq!(router.hovered(), None);
    }
}

#[test]
fn test_route_hover_goes_to_the_latest_wins_slot_and_a_page_turn_to_the_ordered_one() {
    // The two channels have different overflow rules, and the only way to tell which one an
    // event reached is which of them the host sees first: the ordered queue is drained
    // before the hover slot. A page turn posted into the hover slot would be refused there
    // (the slot takes only hovers) and the hover would come out first instead.
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let (x, y) = centre(&geometry, 2);
    assert_eq!(
        route(&mut router, &geometry, &motion((x, y)), &events, now),
        Some(UiEvent::Hover {
            revision: REV,
            index: Some(2)
        })
    );
    assert_eq!(
        route(&mut router, &geometry, &wheel((x, y), 1), &events, now),
        Some(UiEvent::Page {
            revision: REV,
            dir: PageDir::Next
        })
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        Some(UiEvent::Page {
            revision: REV,
            dir: PageDir::Next
        }),
        "the page turn is on the ordered queue, which the host drains first"
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        Some(UiEvent::Hover {
            revision: REV,
            index: Some(2)
        }),
        "and the hover is in the latest-wins slot"
    );
    assert_eq!(events.poll(Duration::ZERO), None);
}

#[test]
fn test_route_click_is_never_dropped_behind_a_busy_hover_and_page_queue() {
    // The contract's hardest rule: a click is never dropped. It holds because a click goes
    // to its own bounded queue, which the host drains before either of the other two.
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    for position in 0..CANDIDATES {
        let (x, y) = centre(&geometry, position);
        let _ = route(&mut router, &geometry, &motion((x, y)), &events, now);
        let _ = route(&mut router, &geometry, &wheel((x, y), 1), &events, now);
    }
    let selected = click(&mut router, &geometry, 4, &events, now);
    assert_eq!(
        selected,
        Some(UiEvent::Select {
            revision: REV,
            index: 4,
            trigger: SelectTrigger::Mouse
        })
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        selected,
        "the click comes out ahead of everything else that is queued"
    );
    assert_eq!(
        events.select_timeouts(),
        0,
        "and nothing had to be abandoned"
    );
}

#[test]
fn test_route_click_the_select_channel_refuses_is_counted_and_not_retried() {
    // A budget of zero turns the bounded wait into a single attempt, so the overflow path
    // is reachable without waiting half a millisecond. The click is abandoned loudly: the
    // counter behind `ui/select/timeout` records it, and the caller is not failed, because
    // taking the UI thread down over a click the user can repeat is the worse trade.
    let config = ChannelConfig {
        select_capacity: 1,
        select_spin: Duration::ZERO,
        ..ChannelConfig::default()
    };
    let events = UiEventQueue::new(&config);
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let now = Instant::now();
    assert!(
        click(&mut router, &geometry, 0, &events, now).is_some(),
        "the first click fits"
    );
    assert_eq!(
        click(&mut router, &geometry, 1, &events, now),
        None,
        "the second is refused rather than queued behind a host that is not draining"
    );
    assert_eq!(events.select_timeouts(), 1, "and the refusal is reported");
}

#[test]
fn test_route_wheel_up_on_the_first_page_dismisses_and_on_a_later_page_turns_back() {
    let first = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let (x, y) = centre(&first, 0);
    assert_eq!(
        route(&mut router, &first, &wheel((x, y), -1), &events, now),
        Some(UiEvent::Dismiss {
            revision: REV,
            reason: DismissReason::ScrollUpEmpty
        }),
        "there is nothing above the first page, so the gesture closes the window"
    );

    let later = placed(1.0, 2);
    assert_eq!(
        route(&mut router, &later, &wheel((x, y), -1), &events, now),
        Some(UiEvent::Page {
            revision: REV,
            dir: PageDir::Prev
        }),
        "on a later page the same gesture turns back"
    );
}

#[test]
fn test_route_without_a_placement_posts_nothing() {
    // Before the first frame there is no hit map, so a pointer event is meaningless rather
    // than "outside": nothing may be posted and nothing may be remembered.
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    for event in [motion((10, 10)), press((10, 10)), wheel((10, 10), 1)] {
        assert_eq!(
            router.route(
                RouteRequest {
                    event: &event,
                    revision: REV,
                    geometry: None,
                    now
                },
                &events
            ),
            None
        );
    }
    assert_eq!(events.poll(Duration::ZERO), None);
    assert_eq!(router.hovered(), None);
    assert_eq!(
        router.adopt_frame(REV, None, now, &events),
        None,
        "and there is nothing to adopt either"
    );
}

#[test]
fn test_route_click_on_a_later_page_names_the_global_index() {
    // The index runs across pages rather than restarting on each one, which is what makes a
    // click on the second page name a candidate the engine knows about.
    let geometry = placed(1.0, 2);
    let mut router = PointerRouter::new();
    let events = queue();
    let selected = click(&mut router, &geometry, 1, &events, Instant::now());
    assert_eq!(
        selected,
        Some(UiEvent::Select {
            revision: REV,
            index: 6,
            trigger: SelectTrigger::Mouse
        }),
        "the second cell of the second page of five is candidate six"
    );
}

#[test]
fn test_route_adopt_frame_re_hit_tests_a_stationary_pointer() {
    let first = placed(1.0, 1);
    let second = placed(1.0, 2);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let (x, y) = centre(&first, 0);
    assert_eq!(
        route(&mut router, &first, &motion((x, y)), &events, now),
        Some(UiEvent::Hover {
            revision: REV,
            index: Some(0)
        })
    );
    assert_eq!(
        router.adopt_frame(REV + 1, Some(&second), now, &events),
        Some(UiEvent::Hover {
            revision: REV + 1,
            index: Some(5)
        }),
        "the same cell on the next page is a different candidate, and the host has to hear \
         about it even though the pointer never moved"
    );
    assert_eq!(router.hovered(), Some(5));
    assert_eq!(
        router.adopt_frame(REV + 1, Some(&second), now, &events),
        None,
        "adopting the same frame twice says nothing new"
    );
}

#[test]
fn test_route_press_and_release_raise_the_repaint_flag() {
    // A press produces no event, so the flag is the only thing that tells the caller the
    // window has something new to draw. Without it a press would be invisible until the
    // next frame arrived.
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let at = centre(&geometry, 2);
    assert!(!router.take_repaint(), "a fresh router has drawn nothing");
    let _ = route(&mut router, &geometry, &press(at), &events, now);
    assert_eq!(router.pressed(), Some(2));
    assert!(router.take_repaint(), "the press changes what is drawn");
    assert!(!router.take_repaint(), "and the flag is taken once");
    let _ = route(&mut router, &geometry, &release(at), &events, now);
    assert_eq!(router.pressed(), None);
    assert!(router.take_repaint(), "the release clears the Active state");
}

#[test]
fn test_route_right_button_press_dismisses_through_the_ordered_queue() {
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let (x, y) = centre(&geometry, 1);
    let right = SurfaceEvent::PointerButton {
        x,
        y,
        button: 3,
        pressed: true,
    };
    assert_eq!(
        route(&mut router, &geometry, &right, &events, now),
        Some(UiEvent::Dismiss {
            revision: REV,
            reason: DismissReason::OutsideClick
        })
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        Some(UiEvent::Dismiss {
            revision: REV,
            reason: DismissReason::OutsideClick
        }),
        "a dismissal is ordered, like a page turn"
    );
}

#[test]
fn test_route_hover_inside_the_throttle_window_keeps_the_newest_index() {
    // The router reports every cell the pointer crosses, because each one is new
    // information; the slot is what turns a sweep into one delivery naming where the
    // pointer stopped. Suppressing the change here as well would lose that final position.
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let now = Instant::now();
    let first = centre(&geometry, 0);
    let second = centre(&geometry, 1);
    let _ = route(&mut router, &geometry, &motion(first), &events, now);
    let _ = route(
        &mut router,
        &geometry,
        &motion(second),
        &events,
        now + Duration::from_millis(1),
    );
    assert_eq!(
        events.poll(Duration::ZERO),
        Some(UiEvent::Hover {
            revision: REV,
            index: Some(1)
        }),
        "one sweep is one delivery, naming the cell the pointer stopped on"
    );
    assert_eq!(events.poll(Duration::ZERO), None);
    assert_eq!(router.hovered(), Some(1));
}

#[test]
fn test_route_second_click_inside_the_debounce_window_is_counted() {
    let geometry = placed(1.0, 1);
    let mut router = PointerRouter::new();
    let events = queue();
    let start = Instant::now();
    assert!(
        click(&mut router, &geometry, 3, &events, start).is_some(),
        "the first click counts"
    );
    assert_eq!(
        click(
            &mut router,
            &geometry,
            3,
            &events,
            start + Duration::from_millis(20)
        ),
        None,
        "a double click is one intent, so the second is not a second commit"
    );
    assert_eq!(router.debounced_clicks(), 1);
}
