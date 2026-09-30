//! The copy out of the scratch: what region a frame has to carry over, and what it costs.
//!
//! These cases are about the *copy*, not about the scene that produced it. Two of them run
//! a real component (the steady-state keystroke, and the merged copy against the
//! per-rectangle form it replaced); the rest drive a [`FrameState`] whose scratch holds a
//! pattern, so a copy that lands at the wrong offset, skips a row or stops one row early
//! shows up as a byte comparison rather than as a flat fill that hides it.

use ime_types::{PlatformError, RectI};

use super::super::raster::{Argb8888Pixel, BYTES_PER_PIXEL, union_pair, union_rect};
use super::super::{FrameState, RenderOutcome, copy_bounds, copy_frame, region_bytes};
use super::show_card;
use crate::renderer::mock::on_own_thread;

/// The byte every buffer in these tests starts out filled with.
const INITIAL_FILL: u8 = 0x5a;

/// Whether `(x, y)` is inside `rect`.
fn inside(rect: RectI, x: i32, y: i32) -> bool {
    let right = rect.x + rect.w as i32;
    let bottom = rect.y + rect.h as i32;
    x >= rect.x && y >= rect.y && x < right && y < bottom
}

/// A frame state whose scratch holds a pattern that makes a misplaced copy visible.
///
/// Every pixel is a function of its position, so a copy that lands at the wrong offset, that
/// skips a row or that stops one row early changes the byte comparison instead of hiding in
/// a flat fill.
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
    // The steady state the merge exists for: three rectangles of one frame's damage, all
    // of them overlapping the rectangle the surface is still showing. One copy covers their
    // bounding box, so the number of rectangles the damage arrived in does not multiply
    // the memory traffic -- which is what the per-rectangle form charged for the overlap.
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
