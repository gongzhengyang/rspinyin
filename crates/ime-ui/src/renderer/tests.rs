//! Scene-level tests: a real `.slint` component rasterized onto a mock surface.
//!
//! These are the tests that prove the whole chain -- generated component, software
//! renderer, scratch, surface buffer -- agrees on what a pixel is. They run on a thread of
//! their own (see [`on_own_thread`]) because Slint installs its platform per thread.
//!
//! The cases that need a component of their own, or that are about one part of the frame
//! path rather than about the chain, live in submodules: [`animation`] drives a moving
//! highlight, [`copy`] covers the copy out of the scratch, [`damage`] the damage list, and
//! [`shrink`] the scratch a window gives back after it has stayed small.

mod animation;
mod copy;
mod damage;
mod shrink;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ime_types::{RectI, SurfaceBackend, SurfaceEvent};
use slint::platform::WindowAdapter as _;
use slint::platform::software_renderer::PhysicalRegion;
use slint::{ComponentHandle as _, PhysicalSize};

use super::mock::{MockState, MockSurface, on_own_thread};
use super::raster::BYTES_PER_PIXEL;
use super::{FrameState, RenderOutcome, SlintWindowAdapter};
use crate::slint_platform::RspinyinPlatform;

/// The scene the pixel assertions are made on.
///
/// It is private and stays private: the candidate window must never become a Slint surface
/// a third party can program against (`OB-4`). The allow covers the property accessors the
/// macro generates and these tests never call.
///
/// There is deliberately no `Text` in this scene: Slint's software renderer aborts when it
/// is asked to shape text and the build has no font backend, and the pixel assertions must
/// hold in every build. The text path is covered by the ignored
/// `test_text_scene_renders_with_a_font_backend`, which runs once the build has one.
#[allow(dead_code)]
mod scene {
    slint::slint! {
        export component RenderCard inherits Window {
            width: 160px;
            height: 64px;
            background: transparent;

            in-out property <bool> highlight: false;

            // Drawn only while `corner` is set, far from the highlight, so two frames in a
            // row can damage two rectangles that do not overlap. That is what tells the
            // damage a frame reports apart from the region its copy has to cover.
            in-out property <bool> corner: false;

            // A rounded rectangle whose corners must stay transparent.
            Rectangle {
                x: 4px;
                y: 4px;
                width: 40px;
                height: 32px;
                border-radius: 8px;
                background: #ff0000;
            }

            // A gradient, so the sampling has a ramp whose ends are known.
            Rectangle {
                x: 52px;
                y: 4px;
                width: 20px;
                height: 32px;
                background: @linear-gradient(180deg, #000000, #ffffff);
            }

            // Drawn over the gradient only while `highlight` is set, so toggling the
            // property changes a rectangle that is smaller than the window.
            Rectangle {
                x: 52px;
                y: 4px;
                width: 20px;
                height: 32px;
                visible: highlight;
                background: #00ff00;
            }

            // Far from the highlight and drawn only while `corner` is set.
            Rectangle {
                x: 120px;
                y: 44px;
                width: 24px;
                height: 16px;
                visible: corner;
                background: #0000ff;
            }
        }
    }
}

/// The text scene, used only by the ignored font-backend test.
#[allow(dead_code)]
mod text_scene {
    slint::slint! {
        export component TextCard inherits Window {
            width: 96px;
            height: 32px;
            background: transparent;

            Text {
                x: 0px;
                y: 0px;
                text: "你好 abc 1";
                font-size: 16px;
                color: white;
            }
        }
    }
}

/// The row length of the test surface, in bytes.
const STRIDE: usize = 160 * BYTES_PER_PIXEL;

/// Starts a platform on a mock surface and shows the test card on it.
///
/// Returns the platform, the component and the observation state.
fn show_card() -> (RspinyinPlatform, scene::RenderCard, Arc<Mutex<MockState>>) {
    let (backend, state) = MockSurface::new(160, 64, 1.0);
    let platform = RspinyinPlatform::new(Box::new(backend));
    platform
        .install()
        .expect("a fresh thread has no Slint platform yet");
    let card = scene::RenderCard::new().expect("the component binds to the platform");
    card.show().expect("the surface can be mapped");
    (platform, card, state)
}

#[test]
fn test_render_card_paints_the_rounded_rectangle_and_the_gradient() {
    let (transparent, corner, centre, gradient_top, gradient_bottom, commits) =
        on_own_thread(|| {
            let (platform, _card, state) = show_card();
            platform
                .render_if_dirty()
                .expect("the first frame is committed");
            let state = state.lock().expect("the mock is not poisoned");
            (
                state.pixel(STRIDE, 0, 0),
                state.pixel(STRIDE, 4, 4),
                state.pixel(STRIDE, 24, 20),
                state.pixel(STRIDE, 62, 6),
                state.pixel(STRIDE, 62, 34),
                state.commits,
            )
        });
    assert_eq!(commits, 1, "the first frame reaches the surface once");
    assert_eq!(transparent[3], 0, "the window background stays transparent");
    assert_eq!(
        corner[3], 0,
        "the rounded corner is outside the shape, so nothing is painted there"
    );
    assert_eq!(
        centre,
        [0, 0, 255, 255],
        "the centre of the rectangle is opaque red in the surface byte order"
    );
    assert_eq!(gradient_top[3], 255, "the gradient is opaque");
    assert_eq!(gradient_bottom[3], 255, "the gradient is opaque");
    let dark = gradient_top[2].min(gradient_bottom[2]);
    let light = gradient_top[2].max(gradient_bottom[2]);
    assert!(dark < 40, "one end of the gradient is nearly black: {dark}");
    assert!(light > 215, "the other end is nearly white: {light}");
    assert_ne!(
        gradient_top, gradient_bottom,
        "the gradient is a ramp, not a flat fill"
    );
}

#[test]
fn test_partial_repaint_keeps_the_pixels_the_frame_did_not_touch() {
    // The changed frame goes into the *other* surface buffer, which still holds what was
    // there before it. Everything the frame does not redraw therefore has to come from the
    // copy of the previous damage, not from the buffer's old contents: this is the
    // assertion that the copy covers two regions rather than one.
    let (changed, untouched, rendered) = on_own_thread(|| {
        let (platform, card, state) = show_card();
        // Settle the first layout, then change exactly one rectangle.
        for _ in 0..4 {
            platform
                .render_if_dirty()
                .expect("a settling frame is handled");
        }
        card.set_highlight(true);
        let outcome = platform
            .render_if_dirty()
            .expect("the changed frame is committed");
        let state = state.lock().expect("the mock is not poisoned");
        (
            state.pixel(STRIDE, 62, 20),
            state.pixel(STRIDE, 24, 20),
            matches!(outcome, RenderOutcome::Rendered { .. }),
        )
    });
    assert!(rendered, "a changed property produces a frame");
    assert_eq!(
        changed,
        [0, 255, 0, 255],
        "the highlighted rectangle is green"
    );
    assert_eq!(
        untouched,
        [0, 0, 255, 255],
        "the rectangle the frame did not touch still holds the previous frame's pixels"
    );
}

#[test]
fn test_render_if_dirty_commits_nothing_once_the_scene_settles() {
    // `BUDGET-CPU-01`: a window nothing is happening to must not touch the compositor. The
    // first frames are allowed to settle the renderer's caches; after that, nothing.
    let (before, after, outcomes) = on_own_thread(|| {
        let (platform, _card, state) = show_card();
        let mut outcomes = Vec::new();
        for _ in 0..3 {
            outcomes.push(
                platform
                    .render_if_dirty()
                    .expect("a settling frame is handled"),
            );
        }
        let before = state.lock().expect("the mock is not poisoned").commits;
        for _ in 0..3 {
            outcomes.push(
                platform
                    .render_if_dirty()
                    .expect("an idle frame is handled"),
            );
        }
        let after = state.lock().expect("the mock is not poisoned").commits;
        (before, after, outcomes)
    });
    assert!(
        outcomes[3..]
            .iter()
            .all(|outcome| *outcome == RenderOutcome::Idle),
        "the window goes idle once its layout has settled: {outcomes:?}"
    );
    assert_eq!(
        after, before,
        "an idle window commits nothing: {outcomes:?}"
    );
}

#[test]
fn test_render_if_dirty_skips_a_frame_the_backend_cannot_take() {
    let (skipped, streak, retried, commits, acquired) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(160, 64, 1.0);
        state.lock().expect("the mock is not poisoned").starve = 1;
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = scene::RenderCard::new().expect("the component binds to the platform");
        card.show().expect("the surface can be mapped");
        let skipped = platform
            .render_if_dirty()
            .expect("a starved frame is not an error");
        let streak = platform.starvation_streak();
        let retried = platform
            .render_if_dirty()
            .expect("the frame is retried on the next call");
        let state = state.lock().expect("the mock is not poisoned");
        (skipped, streak, retried, state.commits, state.acquired)
    });
    assert_eq!(skipped, RenderOutcome::Skipped);
    assert_eq!(streak, 1, "the streak counts consecutive starved frames");
    assert!(
        matches!(retried, RenderOutcome::Rendered { .. }),
        "the skipped frame is still dirty and is drawn on the retry"
    );
    assert_eq!(commits, 1, "a skipped frame commits nothing");
    assert_eq!(
        acquired, 1,
        "only the retry is handed a buffer; the starved attempt is not"
    );
}

#[test]
fn test_showing_the_surface_again_repaints_all_of_it() {
    // Unmapping the surface leaves its buffers holding undefined pixels, so the frame that
    // follows a show has to damage the whole surface rather than a dirty rectangle.
    let damage = on_own_thread(|| {
        let (platform, card, state) = show_card();
        platform
            .render_if_dirty()
            .expect("the first frame is committed");
        card.hide().expect("the surface can be unmapped");
        card.show().expect("the surface can be mapped again");
        platform
            .render_if_dirty()
            .expect("the frame after the show is committed");
        let state = state.lock().expect("the mock is not poisoned");
        state.damage.clone()
    });
    assert_eq!(
        damage.last(),
        Some(&RectI {
            x: 0,
            y: 0,
            w: 160,
            h: 64
        }),
        "the whole surface is damaged: {damage:?}"
    );
}

#[test]
fn test_apply_resize_follows_the_surface() {
    let backend: Rc<RefCell<Box<dyn SurfaceBackend>>> =
        Rc::new(RefCell::new(Box::new(MockSurface::new(160, 64, 2.0).0)));
    let adapter = SlintWindowAdapter::new(backend).expect("the adapter is created");
    assert_eq!(adapter.size(), PhysicalSize::new(320, 128));
    let applied = adapter
        .apply_geometry_event(SurfaceEvent::Resize { w: 400, h: 200 })
        .expect("the resize is applied");
    assert!(applied, "a resize is a geometry event");
    assert_eq!(adapter.size(), PhysicalSize::new(400, 200));
    assert!(
        !adapter
            .apply_geometry_event(SurfaceEvent::PointerLeave)
            .expect("a pointer event is not a geometry event"),
        "pointer events are left to the UI thread"
    );
    assert_eq!(
        adapter.size(),
        PhysicalSize::new(400, 200),
        "a pointer event leaves the geometry alone"
    );
}

#[test]
fn test_apply_scale_rescales_the_physical_size() {
    let backend: Rc<RefCell<Box<dyn SurfaceBackend>>> =
        Rc::new(RefCell::new(Box::new(MockSurface::new(160, 64, 1.0).0)));
    let adapter = SlintWindowAdapter::new(backend).expect("the adapter is created");
    assert_eq!(adapter.size(), PhysicalSize::new(160, 64));
    adapter
        .apply_geometry_event(SurfaceEvent::Scale { factor: 2.0 })
        .expect("the scale is applied");
    assert_eq!(adapter.size(), PhysicalSize::new(320, 128));
    // A scale that cannot be used is replaced rather than propagated.
    adapter
        .apply_geometry_event(SurfaceEvent::Scale { factor: 0.0 })
        .expect("a degenerate scale is replaced, not rejected");
    assert_eq!(adapter.size(), PhysicalSize::new(160, 64));
}

#[test]
fn test_frame_state_starts_with_a_full_repaint_and_adds_nothing_for_an_empty_region() {
    let mut state = FrameState::new();
    assert!(
        state.full,
        "the first frame has no previous contents to keep"
    );
    state.record_damage(&PhysicalRegion::default(), 160, 64);
    assert_eq!(
        state.pending,
        vec![RectI {
            x: 0,
            y: 0,
            w: 160,
            h: 64
        }],
        "a full repaint damages the whole surface"
    );
    state.full = false;
    state.pending.clear();
    state.record_damage(&PhysicalRegion::default(), 160, 64);
    assert!(state.pending.is_empty(), "an empty region damages nothing");
}

#[test]
#[ignore = "needs a font backend: Slint's software renderer aborts when shaping text without the `software-renderer-systemfonts` feature or an embedded bitmap font"]
fn test_text_scene_renders_with_a_font_backend() {
    let ink = on_own_thread(|| {
        let (backend, state) = MockSurface::new(96, 32, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = text_scene::TextCard::new().expect("the component binds to the platform");
        card.show().expect("the surface can be mapped");
        platform
            .render_if_dirty()
            .expect("the text frame is committed");
        let state = state.lock().expect("the mock is not poisoned");
        let stride = 96 * BYTES_PER_PIXEL;
        let mut ink = 0;
        for y in 0..32 {
            for x in 0..96 {
                if state.pixel(stride, x, y)[3] > 0 {
                    ink += 1;
                }
            }
        }
        ink
    });
    assert!(
        ink > 0,
        "the text scene paints glyphs once a font backend exists"
    );
}
