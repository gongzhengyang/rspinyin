//! Tests for [`super::WaylandBackend`], driven by a scripted connection.
//!
//! This file is the body of the `tests` module declared in `backend.rs`; it lives beside that
//! file only because the two together exceed the project's file-length limit. Everything here
//! runs with no display server: the connection is [`ScriptedClient`], which answers from a
//! script the test writes and records what the backend asked it to do.

use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::platform::wayland::probe::{Global, LAYER_SHELL_INTERFACE, TierFailure};
use crate::platform::wayland::shm::SLOT_COUNT;

/// What the scripted connection was asked to do.
#[derive(Default, Debug)]
struct Log {
    created: Vec<Tier>,
    layer: Vec<LayerRequest>,
    popup: Vec<PopupRequest>,
    region: Vec<Vec<SurfaceRect>>,
    committed: Vec<(usize, (u32, u32))>,
    damage: Vec<Vec<RectI>>,
    frames: Vec<FrameToken>,
    pool_size: Vec<(u32, u32)>,
    visibility: Vec<bool>,
}

/// A connection that answers from a script and writes down what it was asked.
///
/// It holds the backend to the same call sequence the mock in the parent module holds the X11
/// backend to, and it makes the protocol's answers -- a configure, a buffer release, a focus
/// grab -- something a test can produce on demand.
struct ScriptedClient {
    globals: Vec<Global>,
    outputs: Vec<OutputInfo>,
    pixels: Vec<u8>,
    log: Arc<Mutex<Log>>,
    script: Arc<Mutex<Vec<WireEvent>>>,
    dispatch_fails: bool,
    hold_frames: bool,
}

impl ScriptedClient {
    fn log(&self) -> MutexGuard<'_, Log> {
        self.log.lock().expect("the log is not poisoned")
    }
}

impl ProtocolClient for ScriptedClient {
    fn globals(&self) -> Vec<Global> {
        self.globals.clone()
    }

    fn outputs(&self) -> Vec<OutputInfo> {
        self.outputs.clone()
    }

    fn connection_fd(&self) -> RawFd {
        // The tests never poll a real descriptor; any value will do.
        -1
    }

    fn create_surface(
        &mut self,
        tier: Tier,
        _output: Option<&OutputInfo>,
    ) -> Result<(), PlatformError> {
        self.log().created.push(tier);
        Ok(())
    }

    fn configure_layer(&mut self, request: &LayerRequest) -> Result<(), PlatformError> {
        self.log().layer.push(*request);
        Ok(())
    }

    fn configure_popup(&mut self, request: &PopupRequest) -> Result<(), PlatformError> {
        self.log().popup.push(*request);
        Ok(())
    }

    fn set_buffer_scale(&mut self, _scale: i32) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_input_region(&mut self, rects: &[SurfaceRect]) -> Result<(), PlatformError> {
        self.log().region.push(rects.to_vec());
        Ok(())
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        self.log().visibility.push(visible);
        Ok(())
    }

    fn attach_and_commit(&mut self, frame: FrameCommit<'_>) -> Result<(), PlatformError> {
        let mut log = self.log();
        log.committed.push((frame.slot, frame.size_px));
        log.damage.push(frame.damage.to_vec());
        Ok(())
    }

    fn resize_pool(&mut self, width_px: u32, height_px: u32) -> Result<(), PlatformError> {
        self.pixels = vec![0; width_px as usize * 4 * height_px as usize];
        self.log().pool_size.push((width_px, height_px));
        Ok(())
    }

    fn buffer_mut(&mut self, slot: usize) -> Option<&mut [u8]> {
        if slot < SLOT_COUNT {
            Some(&mut self.pixels)
        } else {
            None
        }
    }

    fn request_frame(&mut self, token: FrameToken) -> Result<(), PlatformError> {
        if self.hold_frames {
            return Err(PlatformError::Disconnected);
        }
        self.log().frames.push(token);
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<WireEvent>) -> Result<(), PlatformError> {
        let mut script = self.script.lock().expect("the script is not poisoned");
        out.append(&mut script);
        if self.dispatch_fails {
            return Err(PlatformError::Disconnected);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// A backend over a scripted connection, plus the handles a test drives it through.
struct Harness {
    backend: WaylandBackend,
    log: Arc<Mutex<Log>>,
    script: Arc<Mutex<Vec<WireEvent>>>,
}

impl Harness {
    fn new(globals: Vec<Global>) -> Self {
        Self::configured(globals, false, false)
    }

    fn configured(globals: Vec<Global>, dispatch_fails: bool, hold_frames: bool) -> Self {
        let log = Arc::new(Mutex::new(Log::default()));
        let script = Arc::new(Mutex::new(Vec::new()));
        let client = ScriptedClient {
            globals,
            outputs: vec![OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 2.0)],
            pixels: Vec::new(),
            log: Arc::clone(&log),
            script: Arc::clone(&script),
            dispatch_fails,
            hold_frames,
        };
        Self {
            backend: WaylandBackend::new(300, 70, 2.0, Box::new(client)),
            log,
            script,
        }
    }

    fn place(&mut self) {
        self.backend
            .place(placement())
            .expect("the placement is accepted");
    }

    /// Hands events to the connection the backend owns.
    fn deliver(&self, events: &[WireEvent]) {
        self.script
            .lock()
            .expect("the script is not poisoned")
            .extend_from_slice(events);
    }

    /// Polls the backend and returns what it reported.
    fn poll(&mut self) -> Vec<SurfaceEvent> {
        let mut events = Vec::new();
        self.backend
            .poll_events(&mut events)
            .expect("polling succeeds");
        events
    }

    fn log(&self) -> MutexGuard<'_, Log> {
        self.log.lock().expect("the log is not poisoned")
    }
}

/// A registry as a wlroots compositor would announce it.
fn wlroots_globals() -> Vec<Global> {
    vec![
        Global::new(1, "wl_compositor", 6),
        Global::new(2, "wl_shm", 1),
        Global::new(3, "xdg_wm_base", 6),
        Global::new(4, LAYER_SHELL_INTERFACE, 4),
    ]
}

/// A registry as a plain xdg-shell compositor would announce it.
fn xdg_globals() -> Vec<Global> {
    vec![
        Global::new(1, "wl_compositor", 4),
        Global::new(2, "wl_shm", 1),
        Global::new(3, "xdg_wm_base", 2),
    ]
}

/// The placement a test uses unless it is testing something else.
fn placement() -> WindowPlacement {
    WindowPlacement::new(
        RectI {
            x: 400,
            y: 800,
            w: 4,
            h: 40,
        },
        (400, 840),
        OutputInfo::new("eDP-1", (0, 0), (1920, 1080), 2.0),
    )
}

#[test]
fn test_new_starts_at_the_probed_tier() {
    let harness = Harness::new(wlroots_globals());
    assert_eq!(harness.backend.tier(), Tier::LayerShell);
    assert_eq!(harness.backend.backend_id(), "wlr-layer-shell");
    assert!(harness.backend.capabilities().layer_shell);
    assert!(
        harness.backend.deadline().is_none(),
        "no surface exists yet, so no budget is running"
    );
}

#[test]
fn test_new_without_a_display_protocol_reports_the_fallback() {
    let mut harness = Harness::new(vec![Global::new(1, "wl_shm", 1)]);
    assert_eq!(harness.backend.tier(), Tier::Fallback);
    assert_eq!(harness.backend.backend_id(), "wlr-fallback");
    assert!(matches!(
        harness.backend.set_visible(true),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        harness.backend.place(placement()),
        Err(PlatformError::Unavailable)
    ));
    assert_eq!(
        harness.backend.diagnostics().compositor,
        CompositorKind::Unsupported
    );
}

#[test]
fn test_place_creates_the_surface_and_applies_the_layer_margins() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    let log = harness.log();
    assert_eq!(log.created, vec![Tier::LayerShell]);
    assert_eq!(log.pool_size, vec![(600, 140)]);
    assert_eq!(
        log.region,
        vec![Vec::new()],
        "the region starts click-through"
    );
    let request = log.layer.last().expect("a layer request was sent");
    assert_eq!(
        (request.margin_top, request.margin_left),
        (420, 200),
        "the margin is the position in the output's own units"
    );
    assert_eq!(
        request.keyboard_interactivity,
        layer_shell::KEYBOARD_INTERACTIVITY_NONE,
        "the candidate window never asks for the keyboard"
    );
    assert_eq!(request.exclusive_zone, layer_shell::EXCLUSIVE_ZONE_NONE);
}

#[test]
fn test_a_layer_configure_is_reported_as_a_physical_resize() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    assert!(harness.backend.deadline().is_some(), "the budget is armed");
    // The compositor allows less than the whole window, as it does when the output cannot
    // fit it.
    harness.deliver(&[WireEvent::LayerConfigure {
        width: 400,
        height: 100,
    }]);
    let events = harness.poll();
    assert_eq!(
        events,
        vec![SurfaceEvent::Resize { w: 800, h: 200 }],
        "the resize is reported in physical pixels, as X11 reports a configure"
    );
    assert_eq!(harness.backend.geometry(), (400, 100, 2.0));
    assert!(
        harness.backend.deadline().is_none(),
        "a configure confirms the tier"
    );
    assert_eq!(harness.backend.diagnostics().tier, Tier::LayerShell);
}

#[test]
fn test_acquire_buffer_without_a_release_reports_no_free_buffer() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    for slot in 0..SLOT_COUNT {
        let buffer = harness
            .backend
            .acquire_buffer()
            .expect("a free slot at the start");
        assert_eq!(buffer.width, 600);
        assert_eq!(buffer.height, 140);
        assert_eq!(buffer.stride, 2400);
        assert_eq!(buffer.data.len(), 600 * 4 * 140, "slot {slot}");
    }
    assert!(
        matches!(
            harness.backend.acquire_buffer(),
            Err(PlatformError::NoFreeBuffer)
        ),
        "a buffer the compositor holds is never handed out again"
    );
}

#[test]
fn test_starved_frames_are_counted_once_per_run() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    {
        let _first = harness.backend.acquire_buffer().expect("slot one");
        let _second = harness.backend.acquire_buffer().expect("slot two");
    }
    for _ in 0..3 {
        assert!(harness.backend.acquire_buffer().is_err());
    }
    assert_eq!(harness.backend.diagnostics().starvation_episodes, 1);
    assert_eq!(
        harness.backend.pool.worst_run(),
        3,
        "the run is remembered for the diagnostics"
    );
    for _ in 0..4 {
        assert!(harness.backend.acquire_buffer().is_err());
    }
    assert_eq!(
        harness.backend.diagnostics().starvation_episodes,
        1,
        "a run that lasts longer is still one episode"
    );
}

#[test]
fn test_a_released_slot_becomes_available_again() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    {
        let _first = harness.backend.acquire_buffer().expect("slot one");
        let _second = harness.backend.acquire_buffer().expect("slot two");
    }
    harness.deliver(&[
        WireEvent::BufferReleased { slot: 0 },
        WireEvent::BufferReleased { slot: 1 },
    ]);
    harness.poll();
    assert!(harness.backend.acquire_buffer().is_ok());
}

#[test]
fn test_a_hundred_visibility_cycles_do_not_grow_the_pool() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    let baseline = harness.backend.pool.pool_bytes();
    for _ in 0..100 {
        harness
            .backend
            .set_visible(true)
            .expect("the window can be shown");
        harness
            .backend
            .set_visible(false)
            .expect("the window can be hidden");
    }
    assert_eq!(
        harness.backend.pool.pool_bytes(),
        baseline,
        "the pool is the same size after a hundred cycles"
    );
    assert!(!harness.backend.pool.has_pending());
    let log = harness.log();
    assert_eq!(log.pool_size.len(), 1, "the pool was created exactly once");
    assert_eq!(log.visibility.len(), 200);
}

#[test]
fn test_a_commit_attaches_the_acquired_slot_with_its_damage() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    {
        let buffer = harness.backend.acquire_buffer().expect("a free slot");
        buffer.data[..4].copy_from_slice(&[0x10, 0x20, 0x30, 0x40]);
    }
    let damage = RectI {
        x: 4,
        y: 4,
        w: 40,
        h: 12,
    };
    harness
        .backend
        .commit(&[damage])
        .expect("the frame is committed");
    let log = harness.log();
    assert_eq!(log.committed, vec![(0, (600, 140))]);
    assert_eq!(log.damage, vec![vec![damage]]);
}

#[test]
fn test_a_commit_before_the_pool_has_resized_skips_the_frame() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    {
        let _buffer = harness.backend.acquire_buffer().expect("a free slot");
    }
    // A configure arrives while the slot is out, so the pool cannot adopt the new size yet
    // and the buffer it would attach is the wrong one.
    harness.deliver(&[WireEvent::LayerConfigure {
        width: 400,
        height: 100,
    }]);
    harness.poll();
    assert!(matches!(
        harness.backend.commit(&[]),
        Err(PlatformError::NoFreeBuffer)
    ));
}

#[test]
fn test_a_popup_configure_moves_the_window_to_the_compositors_position() {
    let mut harness = Harness::new(xdg_globals());
    assert_eq!(harness.backend.tier(), Tier::Popup);
    harness.place();
    assert_eq!(harness.log().created, vec![Tier::Popup]);
    harness.deliver(&[WireEvent::PopupConfigure {
        x: 4,
        y: 6,
        width: 500,
        height: 120,
    }]);
    let events = harness.poll();
    assert_eq!(
        events,
        vec![SurfaceEvent::Resize { w: 1000, h: 240 }],
        "the configured size is reported in physical pixels"
    );
    assert_eq!(harness.backend.geometry(), (500, 120, 2.0));
    assert_eq!(
        harness.backend.position,
        (8, 12),
        "the compositor's position wins over the requested one"
    );
}

#[test]
fn test_the_canvas_tier_places_the_popup_where_the_backend_computed() {
    let mut harness = Harness::new(xdg_globals());
    harness.place();
    // The compositor dismisses the popup before configuring it, which is a refusal of the
    // tier, so the ladder moves to the canvas tier.
    harness.deliver(&[WireEvent::PopupDone]);
    harness.poll();
    assert_eq!(harness.backend.tier(), Tier::CanvasPopup);
    assert_eq!(harness.backend.backend_id(), "wlr-canvas");
    let log = harness.log();
    assert_eq!(log.created, vec![Tier::Popup, Tier::CanvasPopup]);
    let request = log.popup.last().expect("a popup request was sent");
    assert_eq!(
        request.constraint_adjustment,
        crate::platform::wayland::popup::CONSTRAINT_NONE,
        "tier 3 does its own avoidance"
    );
    assert_eq!(
        request.anchor_rect,
        SurfaceRect::new(200, 420, 1, 1),
        "the anchor rectangle carries the position the backend computed"
    );
}

#[test]
fn test_taking_the_keyboard_fails_the_tier_and_promotes() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    harness.deliver(&[WireEvent::KeyboardEnter]);
    harness.poll();
    assert_eq!(
        harness.backend.tier(),
        Tier::Popup,
        "the tier that grabbed the keyboard is dropped"
    );
    assert_eq!(harness.backend.diagnostics().focus_events, 1);
    assert_eq!(harness.log().created, vec![Tier::LayerShell, Tier::Popup]);
    assert_eq!(
        harness
            .backend
            .attempts()
            .first()
            .map(|attempt| attempt.failure),
        Some(TierFailure::FocusTaken)
    );
}

#[test]
fn test_a_dismissed_popup_hides_the_window_and_asks_the_caller_to_close() {
    let mut harness = Harness::new(xdg_globals());
    harness.place();
    harness
        .backend
        .set_visible(true)
        .expect("the window is shown");
    harness.deliver(&[
        WireEvent::PopupConfigure {
            x: 0,
            y: 0,
            width: 300,
            height: 70,
        },
        WireEvent::PopupDone,
    ]);
    let events = harness.poll();
    assert_eq!(events, vec![SurfaceEvent::CloseRequested]);
    assert_eq!(
        harness.backend.tier(),
        Tier::Popup,
        "a dismissal is not a failed tier"
    );
    assert_eq!(harness.log().visibility, vec![true, false]);
}

#[test]
fn test_pointer_events_reach_the_caller_in_physical_pixels() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    harness.deliver(&[
        WireEvent::PointerEnter { x: 1.0, y: 2.0 },
        WireEvent::PointerMotion { x: 3.0, y: 4.0 },
        WireEvent::PointerButton {
            x: 5.0,
            y: 6.0,
            button: 0x110,
            pressed: true,
        },
        WireEvent::PointerLeave,
    ]);
    let events = harness.poll();
    assert_eq!(
        events,
        vec![
            SurfaceEvent::PointerEnter { x: 2, y: 4 },
            SurfaceEvent::PointerMotion { x: 6, y: 8 },
            SurfaceEvent::PointerButton {
                x: 10,
                y: 12,
                button: 1,
                pressed: true
            },
            SurfaceEvent::PointerLeave,
        ]
    );
}

#[test]
fn test_the_input_region_is_clipped_and_converted_to_surface_units() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    let rects = [
        RectI {
            x: 0,
            y: 0,
            w: 100,
            h: 40,
        },
        RectI {
            x: 500,
            y: 100,
            w: 400,
            h: 400,
        },
        RectI {
            x: -10,
            y: -10,
            w: 10,
            h: 10,
        },
    ];
    harness
        .backend
        .set_input_region(&rects)
        .expect("the region applies");
    let log = harness.log();
    let sent = log.region.last().expect("a region was sent");
    assert_eq!(
        sent,
        &vec![
            SurfaceRect::new(0, 0, 50, 20),
            SurfaceRect::new(250, 50, 50, 20),
        ],
        "the third rectangle is outside the surface and the others are halved"
    );
}

#[test]
fn test_a_frame_callback_is_only_requested_once_until_it_fires() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    let token = harness
        .backend
        .request_frame()
        .expect("a callback can be requested");
    assert_eq!(token, FrameToken(0));
    assert_eq!(
        harness.backend.request_frame(),
        None,
        "the protocol allows one outstanding callback per surface"
    );
    harness.deliver(&[WireEvent::FrameDone { data: token.0 }]);
    harness.poll();
    assert_eq!(harness.backend.request_frame(), Some(FrameToken(1)));
    assert_eq!(
        harness.log().frames,
        vec![FrameToken(0), FrameToken(1)],
        "the compositor saw both requests"
    );
}

#[test]
fn test_a_frame_callback_that_cannot_be_requested_costs_no_frame() {
    let mut harness = Harness::configured(wlroots_globals(), false, true);
    harness.place();
    assert_eq!(harness.backend.request_frame(), None);
    assert_eq!(harness.backend.diagnostics().protocol_errors, 1);
}

#[test]
fn test_a_lost_connection_is_reported() {
    let mut harness = Harness::configured(wlroots_globals(), true, false);
    harness.place();
    let mut events = Vec::new();
    assert!(matches!(
        harness.backend.poll_events(&mut events),
        Err(PlatformError::Disconnected)
    ));
}

#[test]
fn test_showing_before_a_placement_is_refused() {
    let mut harness = Harness::new(wlroots_globals());
    assert!(matches!(
        harness.backend.set_visible(true),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        harness.backend.acquire_buffer(),
        Err(PlatformError::Unavailable)
    ));
    assert!(harness.backend.request_frame().is_none());
    assert!(
        harness.backend.set_visible(false).is_ok(),
        "hiding a window that was never shown is not an error"
    );
}

#[test]
fn test_the_scale_of_a_new_output_is_adopted() {
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    harness.deliver(&[WireEvent::SurfaceEnter { scale: 1.0 }]);
    let events = harness.poll();
    assert_eq!(events, vec![SurfaceEvent::Scale { factor: 1.0 }]);
    assert_eq!(harness.backend.geometry(), (300, 70, 1.0));
    assert_eq!(harness.backend.size_px, (300, 70));
    // The pool follows the scale at the next acquire, which is where it can safely recreate
    // its buffers.
    {
        let _buffer = harness.backend.acquire_buffer().expect("a free slot");
    }
    assert_eq!(harness.log().pool_size, vec![(600, 140), (300, 70)]);
}

#[test]
fn test_a_pointer_event_on_a_scaled_output_is_not_scaled_twice() {
    // The pointer arrives in surface-local units, which are already logical, so the conversion
    // happens exactly once even after the scale changed.
    let mut harness = Harness::new(wlroots_globals());
    harness.place();
    harness.deliver(&[
        WireEvent::SurfaceEnter { scale: 1.0 },
        WireEvent::PointerMotion { x: 6.0, y: 8.0 },
    ]);
    let events = harness.poll();
    assert_eq!(
        events,
        vec![
            SurfaceEvent::Scale { factor: 1.0 },
            SurfaceEvent::PointerMotion { x: 6, y: 8 },
        ]
    );
}
