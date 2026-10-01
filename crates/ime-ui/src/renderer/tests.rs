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
//!
//! The opacity probes live here too: they drive a bound `opacity` and read the composited
//! alpha back off the surface, which is the measured behaviour every claim about what the
//! software renderer does with element opacity is held to.

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
use crate::adapter::Adapter;
use crate::adapter::tests::frame_with;
use crate::layout;
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

/// The text scene, used by the ignored font-backend test and by the text-opacity probe.
#[allow(dead_code)]
mod text_scene {
    slint::slint! {
        export component TextCard inherits Window {
            width: 96px;
            height: 32px;
            background: transparent;

            // The text-opacity probe drives this; the ignored font test leaves it at the
            // full-strength default.
            in-out property <float> text-opacity: 1.0;

            Text {
                x: 0px;
                y: 0px;
                text: "你好 abc 1";
                font-size: 16px;
                color: white;
                opacity: text-opacity;
            }
        }
    }
}

/// The opacity probe scene: one fill the window draws plain, and one the window draws
/// through a bound `opacity` the test drives.
///
/// Like [`scene`], it is private and `Text`-free, for the same two reasons: the candidate
/// window must never become a programmable Slint surface, and the assertions must hold in
/// every build. The two fills are the same red rectangle far enough apart not to share a
/// pixel, so the unbound one is each frame's built-in reference for what full strength
/// looks like.
#[allow(dead_code)]
mod opacity_scene {
    slint::slint! {
        export component OpacityCard inherits Window {
            width: 96px;
            height: 64px;
            background: transparent;

            in-out property <float> subject-opacity: 1.0;

            Rectangle {
                x: 8px;
                y: 8px;
                width: 40px;
                height: 24px;
                background: #ff0000;
            }

            Rectangle {
                x: 56px;
                y: 8px;
                width: 32px;
                height: 24px;
                background: #ff0000;
                opacity: subject-opacity;
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

/// Renders the opacity probe scene with the subject bound to `opacity` and returns what
/// reached the surface: the pixel of the unbound reference fill, and the strongest alpha
/// byte inside the subject's rectangle.
///
/// The property is set before the component is shown, so the reading comes from the first
/// frame -- a full repaint -- and depends on no later frame's damage tracking. The window
/// background is transparent, so the subject's own alpha is the composited alpha and the
/// measurement is exact.
fn probe_rect_opacity(opacity: f32) -> ([u8; 4], u8) {
    on_own_thread(|| {
        let (backend, state) = MockSurface::new(96, 64, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = opacity_scene::OpacityCard::new().expect("the component binds to the platform");
        card.set_subject_opacity(opacity);
        card.show().expect("the surface can be mapped");
        platform
            .render_if_dirty()
            .expect("the probe frame is committed");
        let state = state.lock().expect("the mock is not poisoned");
        let stride = 96 * BYTES_PER_PIXEL;
        let reference = state.pixel(stride, 28, 20);
        let mut strongest = 0u16;
        for y in 8..32 {
            for x in 56..88 {
                strongest = strongest.max(u16::from(state.pixel(stride, x, y)[3]));
            }
        }
        (reference, strongest as u8)
    })
}

#[test]
fn test_rect_opacity_scales_the_composited_alpha() {
    let (full_reference, full) = probe_rect_opacity(1.0);
    let (half_reference, half) = probe_rect_opacity(0.5);
    assert_eq!(
        full_reference,
        [0, 0, 255, 255],
        "the unbound fill is opaque red in the surface byte order"
    );
    assert_eq!(
        full, 255,
        "the subject at opacity 1.0 composites at full strength"
    );
    assert_eq!(
        half_reference,
        [0, 0, 255, 255],
        "the scene at 0.5 still draws its reference fill untouched"
    );
    assert!(
        (u16::from(half) * 2).abs_diff(u16::from(full)) <= 2,
        "opacity 0.5 composites the fill at half its alpha: {half} against {full}"
    );
}

#[test]
fn test_rect_opacity_zero_draws_no_pixels() {
    let (reference, subject) = probe_rect_opacity(0.0);
    assert_eq!(
        reference,
        [0, 0, 255, 255],
        "the scene around the culled subtree still draws"
    );
    assert_eq!(
        subject, 0,
        "opacity 0.0 is below the renderer's 0.01 cull line, so the subtree leaves no pixel"
    );
}

/// The mean alpha of the ink the text scene commits, with the run bound to `opacity`.
///
/// Returns the ink count and the alpha summed over it divided by it, so the caller can
/// assert both that glyphs exist and how strongly they composite.
fn probe_text_strength(opacity: f32) -> (u32, u32) {
    on_own_thread(|| {
        let (backend, state) = MockSurface::new(96, 32, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = text_scene::TextCard::new().expect("the component binds to the platform");
        card.set_text_opacity(opacity);
        card.show().expect("the surface can be mapped");
        platform
            .render_if_dirty()
            .expect("the text frame is committed");
        let state = state.lock().expect("the mock is not poisoned");
        let stride = 96 * BYTES_PER_PIXEL;
        let mut ink = 0u32;
        let mut total = 0u64;
        for y in 0..32 {
            for x in 0..96 {
                let alpha = u32::from(state.pixel(stride, x, y)[3]);
                if alpha > 0 {
                    ink += 1;
                    total += u64::from(alpha);
                }
            }
        }
        // An empty frame has no ink to divide by; its mean is then defined as zero.
        let mean = (total / u64::from(ink.max(1))) as u32;
        (ink, mean)
    })
}

#[test]
#[ignore = "needs a font backend: Slint's software renderer aborts when shaping text without the `software-renderer-systemfonts` feature or an embedded bitmap font"]
fn test_text_opacity_lowers_the_glyph_alpha() {
    let (ink_full, mean_full) = probe_text_strength(1.0);
    let (ink_half, mean_half) = probe_text_strength(0.5);
    assert!(
        ink_full > 0,
        "the glyphs draw once a font backend exists: {ink_full}"
    );
    assert!(ink_half > 0, "and the faded run still draws: {ink_half}");
    assert!(
        u64::from(mean_half) * 4 < u64::from(mean_full) * 3,
        "the faded glyphs composite noticeably below the full-strength ones: \
         mean {mean_half} against {mean_full}"
    );
}

/// The surface the real-panel scene draws into, in logical pixels at a scale of 1.0 -- the
/// same budget `adapter/tests.rs` reserves, so the widest fixture panel fits with its
/// shadow margin.
const PANEL_WIDTH_DP: u32 = 424;
const PANEL_HEIGHT_DP: u32 = 160;

/// One frame at the 144Hz rate the motion is designed against.
const PANEL_FRAME_S: f32 = 1.0 / 144.0;

/// Renders the real candidate panel once, after the appear motion has been advanced by
/// `step` and, when `run_to_rest` is on, out to its end, and returns the alpha of the
/// panel's own fill sampled in the container padding left of the first cell.
///
/// Each scene commits exactly one frame and that frame is the first one -- a full repaint
/// -- so the reading never depends on which regions a later frame's damage happens to
/// cover. The sample point sits inside the panel at both ends of the appear scale: at 0.96
/// the panel's left edge lands about 4.4dp right of the shadow margin while its content
/// keeps its own size, so the padding two pixels left of the first cell is panel fill at
/// full scale and still inside the shrunk panel, clear of its stroke, of the first cell and
/// of the highlight box.
fn panel_fill_alpha(step: f32, run_to_rest: bool) -> u8 {
    on_own_thread(|| {
        let (backend, state) = MockSurface::new(PANEL_WIDTH_DP, PANEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 2)));
        adapter.advance(step);
        if run_to_rest {
            let mut frames = 0u32;
            while adapter.advance(PANEL_FRAME_S) {
                frames += 1;
                assert!(frames < 1_000, "the appear motion must come to rest");
            }
        }
        let outcome = platform
            .render_if_dirty()
            .expect("the appear frame is committed");
        assert!(
            matches!(outcome, RenderOutcome::Rendered { .. }),
            "the scene commits a frame: {outcome:?}"
        );
        let metrics =
            layout::metrics().expect("ui/candidate.slint declares a readable metrics block");
        let stride = PANEL_WIDTH_DP as usize * BYTES_PER_PIXEL;
        let x = metrics.shadow_margin as usize + metrics.container_padding as usize - 2;
        let y = metrics.shadow_margin as usize
            + metrics.header_height as usize
            + metrics.separator_height as usize
            + metrics.container_padding as usize
            + metrics.cell_height as usize / 2;
        let state = state.lock().expect("the mock is not poisoned");
        state.pixel(stride, x, y)[3]
    })
}

#[test]
fn test_appear_motion_fades_the_panel_in() {
    let first = panel_fill_alpha(0.0, false);
    let mid = panel_fill_alpha(PANEL_FRAME_S, false);
    let settled = panel_fill_alpha(0.0, true);
    assert_eq!(first, 0, "the window's first frame is fully transparent");
    assert!(
        mid > 0,
        "one frame into the appear motion the fade has started: {mid}"
    );
    assert!(
        mid < settled,
        "and it is still short of the settled fill: {mid} against {settled}"
    );
    assert!(
        settled > 200,
        "the settled panel is the acrylic fill itself: {settled}"
    );
}
