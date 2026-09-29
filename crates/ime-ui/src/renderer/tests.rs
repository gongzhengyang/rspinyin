//! Scene-level tests: a real `.slint` component rasterized onto a mock surface.
//!
//! These are the tests that prove the whole chain -- generated component, software
//! renderer, scratch, surface buffer -- agrees on what a pixel is. They run on a thread of
//! their own (see [`on_own_thread`]) because Slint installs its platform per thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ime_types::{PlatformError, RectI, SurfaceBackend, SurfaceEvent};
use slint::platform::WindowAdapter as _;
use slint::platform::software_renderer::PhysicalRegion;
use slint::{ComponentHandle as _, PhysicalSize};

use super::mock::{MockState, MockSurface, on_own_thread};
use super::raster::{Argb8888Pixel, BYTES_PER_PIXEL, union_pair, union_rect};
use super::{FrameState, RenderOutcome, SlintWindowAdapter, copy_bounds, copy_frame, region_bytes};
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

/// A frame state whose scratch holds a pattern that makes a misplaced copy visible.
///
/// Every pixel is a function of its position, so a copy that lands at the wrong offset, that
/// skips a row or that stops one row early changes the byte comparison instead of hiding in
/// a flat fill.
/// The byte every buffer in these tests starts out filled with.
const INITIAL_FILL: u8 = 0x5a;

/// Whether `(x, y)` is inside `rect`.
fn inside(rect: RectI, x: i32, y: i32) -> bool {
    let right = rect.x + rect.w as i32;
    let bottom = rect.y + rect.h as i32;
    x >= rect.x && y >= rect.y && x < right && y < bottom
}

fn patterned_state(width_px: u32, height_px: u32) -> FrameState {
    let mut state = FrameState::new();
    state.scratch.ensure(width_px, height_px);
    let stride = state.scratch.stride;
    for (index, pixel) in state.scratch.pixels.iter_mut().enumerate() {
        let x = (index % stride) as u8;
        let y = (index / stride) as u8;
        *pixel = Argb8888Pixel::pack(x, y, x ^ y, 0xff);
    }
    state
}

#[test]
fn test_copy_bounds_folds_both_lists_into_one_region() {
    let (width_px, height_px) = (160u32, 64u32);
    let pending = [RectI {
        x: 8,
        y: 8,
        w: 8,
        h: 8,
    }];
    let shown = [RectI {
        x: 24,
        y: 8,
        w: 8,
        h: 8,
    }];
    assert_eq!(
        copy_bounds(&[], &[], width_px, height_px),
        None,
        "two empty lists have nothing to copy"
    );
    assert_eq!(
        copy_bounds(&pending, &[], width_px, height_px),
        Some(pending[0]),
        "one list copies as itself"
    );
    assert_eq!(
        copy_bounds(&pending, &shown, width_px, height_px),
        Some(union_pair(pending[0], shown[0])),
        "both lists are folded into one region, which is what makes a frame cost one copy"
    );
    // A rectangle outside the surface is dropped and one that straddles the edge is clipped
    // before it is folded in, so the region handed to the copy is always inside the surface.
    let outside = [RectI {
        x: 200,
        y: 0,
        w: 8,
        h: 8,
    }];
    assert_eq!(copy_bounds(&outside, &[], width_px, height_px), None);
    let straddling = [RectI {
        x: 156,
        y: 60,
        w: 32,
        h: 32,
    }];
    assert_eq!(
        copy_bounds(&straddling, &[], width_px, height_px),
        Some(RectI {
            x: 156,
            y: 60,
            w: 4,
            h: 4
        })
    );
    // `i32::MIN` must not overflow the fold.
    let extreme = [RectI {
        x: i32::MIN,
        y: i32::MIN,
        w: u32::MAX,
        h: u32::MAX,
    }];
    assert_eq!(
        copy_bounds(&extreme, &[], width_px, height_px),
        Some(RectI {
            x: 0,
            y: 0,
            w: 160,
            h: 64
        })
    );
}

#[test]
fn test_copy_frame_matches_the_per_rectangle_copy() {
    // The acceptance criterion for merging the copy: what reaches the surface is what the
    // per-rectangle form wrote. `reference` is that form -- the loop the frame path used to
    // run, over the same scratch and the same two lists -- and `actual` is the merged copy,
    // both starting from the same buffer contents.
    let (width_px, height_px) = (64u32, 32u32);
    let stride = width_px as usize * BYTES_PER_PIXEL;
    let mut state = patterned_state(width_px, height_px);
    let overlapping = RectI {
        x: 8,
        y: 4,
        w: 16,
        h: 12,
    };
    let other = RectI {
        x: 20,
        y: 10,
        w: 24,
        h: 16,
    };
    state.pending = vec![other];
    state.shown = vec![overlapping];
    let mut reference = vec![INITIAL_FILL; stride * height_px as usize];
    let mut actual = reference.clone();
    for rect in state.pending.iter().chain(state.shown.iter()) {
        state
            .scratch
            .blit_into(&mut reference, stride, *rect)
            .expect("the reference copy covers its rectangle");
    }
    let copied = copy_frame(&state, &mut actual, stride, width_px, height_px)
        .expect("the merged copy covers the union of both lists");
    let union = union_pair(overlapping, other);
    assert_eq!(copied, Some(union));
    // Compared pixel by pixel inside the two rectangles, not buffer against buffer. The
    // merged copy writes the *bounding box*, so it also touches the pixels inside the box
    // that neither rectangle names -- those it fills from the scratch, while the reference
    // leaves the initial fill there. Comparing whole buffers would therefore fail on
    // pixels the property under test says nothing about. What the property is about is
    // that every pixel the per-rectangle form wrote, the merged form wrote the same value.
    for y in union.y..union.y + union.h as i32 {
        for x in union.x..union.x + union.w as i32 {
            let named = inside(overlapping, x, y) || inside(other, x, y);
            if !named {
                continue;
            }
            for byte in 0..BYTES_PER_PIXEL {
                let at = y as usize * stride + x as usize * BYTES_PER_PIXEL + byte;
                assert_eq!(
                    actual[at], reference[at],
                    "pixel ({x}, {y}) byte {byte} differs between the two copies"
                );
            }
        }
    }
    // And the box's extra pixels really were written, rather than left as the initial fill:
    // that is what makes the merge a superset rather than a smaller copy in disguise.
    let mut extra_written = 0usize;
    for y in union.y..union.y + union.h as i32 {
        for x in union.x..union.x + union.w as i32 {
            if inside(overlapping, x, y) || inside(other, x, y) {
                continue;
            }
            let at = y as usize * stride + x as usize * BYTES_PER_PIXEL;
            if actual[at..at + BYTES_PER_PIXEL] != [INITIAL_FILL; BYTES_PER_PIXEL] {
                extra_written += 1;
            }
        }
    }
    assert!(
        extra_written > 0,
        "the merged copy writes the whole box, which is the price the merge pays"
    );
    // The union is strictly larger than either list, so the merged copy also writes pixels
    // neither list names. That is allowed -- those pixels are the same on both sides -- and
    // it is the price the merge pays for one pass instead of two.
    assert!(
        union.w > overlapping.w && union.h > overlapping.h,
        "the two lists must overlap for this case to say anything"
    );
    assert_eq!(
        actual.len(),
        reference.len(),
        "the copy never writes outside the buffer"
    );
}

#[test]
fn test_copy_frame_merges_overlapping_damage_into_one_region() {
    // The steady state the merge exists for: three rectangles of one frame's damage, all of
    // them overlapping the rectangle the surface is still showing. One copy covers their
    // bounding box, so the number of rectangles the damage arrived in does not multiply the
    // memory traffic -- which is what the per-rectangle form charged for the overlap.
    let (width_px, height_px) = (64u32, 32u32);
    let stride = width_px as usize * BYTES_PER_PIXEL;
    let mut state = patterned_state(width_px, height_px);
    let damage = [
        RectI {
            x: 8,
            y: 4,
            w: 16,
            h: 12,
        },
        RectI {
            x: 12,
            y: 8,
            w: 16,
            h: 12,
        },
        RectI {
            x: 16,
            y: 6,
            w: 8,
            h: 8,
        },
    ];
    let shown = RectI {
        x: 10,
        y: 5,
        w: 20,
        h: 16,
    };
    let all: Vec<RectI> = damage.iter().copied().chain([shown]).collect();
    state.pending = damage.to_vec();
    state.shown = vec![shown];
    // The box of the four rectangles, worked out by hand rather than by asking the code under
    // test what it produced.
    let bounds = RectI {
        x: 8,
        y: 4,
        w: 22,
        h: 17,
    };
    let mut destination = vec![INITIAL_FILL; stride * height_px as usize];
    let copied = copy_frame(&state, &mut destination, stride, width_px, height_px)
        .expect("the merged copy covers the bounding box of the four rectangles");
    assert_eq!(
        copied,
        Some(bounds),
        "three overlapping rectangles and the one on screen cost a single copy"
    );
    for rect in &all {
        let right = rect.x + rect.w as i32 - 1;
        let bottom = rect.y + rect.h as i32 - 1;
        assert!(
            inside(bounds, rect.x, rect.y) && inside(bounds, right, bottom),
            "{rect:?} is not inside the copy region {bounds:?}"
        );
    }
    // What the frame pays is the box, not the sum of the four rectangles: that is the merge,
    // and it is why a frame's copy cost does not grow with the number of rectangles its damage
    // is reported as.
    let separate: u64 = all.iter().map(|rect| region_bytes(*rect)).sum();
    assert!(
        region_bytes(bounds) < separate,
        "the merged copy writes {} bytes where the per-rectangle form wrote {separate}",
        region_bytes(bounds)
    );
    // And the box is written in full, so what the surface holds afterwards is the scratch's
    // pixel everywhere the copy covered -- the property that makes a superset safe to copy.
    let mut written = 0usize;
    for y in bounds.y..bounds.y + bounds.h as i32 {
        for x in bounds.x..bounds.x + bounds.w as i32 {
            let at = y as usize * stride + x as usize * BYTES_PER_PIXEL;
            if destination[at..at + BYTES_PER_PIXEL] != [INITIAL_FILL; BYTES_PER_PIXEL] {
                written += 1;
            }
        }
    }
    assert_eq!(
        written,
        bounds.w as usize * bounds.h as usize,
        "every pixel of the bounding box is written, which is the price the merge pays"
    );
}

#[test]
fn test_copy_frame_skips_an_empty_region_and_refuses_a_short_buffer() {
    let (width_px, height_px) = (32u32, 16u32);
    let stride = width_px as usize * BYTES_PER_PIXEL;
    let mut state = patterned_state(width_px, height_px);
    let mut destination = vec![0x11u8; stride * height_px as usize];
    assert_eq!(
        copy_frame(&state, &mut destination, stride, width_px, height_px),
        Ok(None),
        "a frame with nothing to carry over copies nothing"
    );
    assert!(
        destination.iter().all(|byte| *byte == 0x11),
        "and leaves the buffer exactly as it found it"
    );
    // A destination that cannot hold the region is refused rather than clamped: writing part
    // of a frame would put pixels in the wrong place.
    state.pending = vec![RectI {
        x: 0,
        y: 0,
        w: 8,
        h: 8,
    }];
    let mut short = vec![0u8; stride * 4];
    assert_eq!(
        copy_frame(&state, &mut short, stride, width_px, height_px),
        Err(PlatformError::Unavailable)
    );
}

#[test]
fn test_steady_state_keystroke_copies_the_damage_once() {
    // A committed frame has two regions to carry over -- what it just rendered and what the
    // surface is still showing -- and it copies their union in a single pass. The two frames
    // below damage different rectangles, so the copy covers strictly more than the frame
    // reports: that is exactly the case the per-rectangle form charged a second copy for.
    let (outcome, reported, previous) = on_own_thread(|| {
        let (platform, card, state) = show_card();
        for _ in 0..4 {
            platform
                .render_if_dirty()
                .expect("a settling frame is handled");
        }
        let settled = state.lock().expect("the mock is not poisoned").damage.len();
        card.set_highlight(true);
        platform
            .render_if_dirty()
            .expect("the changed frame is committed");
        let changed = state.lock().expect("the mock is not poisoned").damage.len();
        card.set_corner(true);
        let outcome = platform
            .render_if_dirty()
            .expect("the second frame is committed");
        let state = state.lock().expect("the mock is not poisoned");
        (
            outcome,
            state.damage[changed..].to_vec(),
            state.damage[settled..changed].to_vec(),
        )
    });
    let RenderOutcome::Rendered { .. } = outcome else {
        panic!("the changed scene produces a frame: {outcome:?}");
    };
    assert!(!reported.is_empty(), "the frame reports damage of its own");
    assert!(
        !previous.is_empty(),
        "and the frame before it reported damage the copy still has to carry over, which is \
         the second copy the merge removes"
    );
    assert_ne!(
        union_rect(&previous),
        union_rect(&reported),
        "the two frames damaged different rectangles, so the copy covered more than this \
         frame's damage"
    );
    // One copy over the union of both lists, reporting this frame's own damage and nothing
    // else: the per-rectangle form charged two copies for the same two lists.
    let mut both = previous.clone();
    both.extend_from_slice(&reported);
    let copy_region = union_rect(&both);
    assert_eq!(
        outcome,
        RenderOutcome::Rendered {
            bounding: union_rect(&reported),
            rectangles: reported.len() as u32,
            copies: 1,
            copy_bytes: region_bytes(copy_region),
        },
        "the frame reports its own damage and pays one copy for both lists"
    );
    assert!(
        copy_region != union_rect(&reported),
        "the copy covers more than the damage the compositor is told about, which is what \
         makes the byte count a merged one: {copy_region:?} against {reported:?}"
    );
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
