//! The damage list: what a frame records, and how the list is kept short.
//!
//! A frame's damage is a list of rectangles, and the list is what the copy out of the
//! scratch and the report to the compositor are both built from. These cases pin the two
//! properties that keep it usable: regions that share a pixel are merged as they are
//! recorded, and regions that do not are kept apart until the list is long enough that
//! folding it is cheaper than carrying it.

use ime_types::RectI;

use super::super::raster::overlaps;
use super::super::{FrameState, PENDING_COLLAPSE_LIMIT, union_pair, union_rect};

/// The rectangles a frame's damage arrives in, in the order the renderer reported them.
///
/// A [`PhysicalRegion`] itself cannot be built here -- its fields are private and the only
/// constructor is `SoftwareRenderer::render` -- so these cases drive
/// [`FrameState::record_damage_rects`], which is the half that owns the merging rules. What
/// they cannot cover is where the rectangles come from; that is the renderer's, and it is
/// covered by the cases that rasterise for real.
fn region(rects: &[RectI]) -> Vec<RectI> {
    rects.to_vec()
}

/// A frame state that is past its first frame, so damage is recorded rather than forced.
fn recording_state() -> FrameState {
    let mut state = FrameState::new();
    state.full = false;
    state
}

#[test]
fn test_record_damage_merges_overlapping_regions_as_it_records_them() {
    // What a highlight that moved inside one frame looks like: where the box was, where it
    // is now, and the corner it covers on the way. The three share pixels, so one entry
    // covers all of them -- which is the whole point of merging on insert rather than
    // waiting for the list to grow past the collapse limit.
    let mut state = recording_state();
    let was = RectI {
        x: 40,
        y: 20,
        w: 60,
        h: 40,
    };
    let now = RectI {
        x: 46,
        y: 20,
        w: 60,
        h: 40,
    };
    let corner = RectI {
        x: 96,
        y: 56,
        w: 8,
        h: 4,
    };
    state.record_damage_rects(&region(&[was, now, corner]), 400, 200);
    assert_eq!(
        state.pending.len(),
        1,
        "three overlapping regions are one entry: {:?}",
        state.pending
    );
    assert_eq!(
        state.pending[0],
        union_pair(union_pair(was, now), corner),
        "and the entry is their bounding box"
    );
    assert!(
        state.pending[0].w > was.w && state.pending[0].x < now.x,
        "the box reaches from where the box was to where it is"
    );
}

#[test]
fn test_record_damage_keeps_regions_that_share_no_pixel_apart() {
    let mut state = recording_state();
    let left = RectI {
        x: 8,
        y: 8,
        w: 16,
        h: 16,
    };
    // Touching the first one along its right edge: no pixel is shared, so merging the two
    // would add the whole area of both to the copy without removing an entry.
    let touching = RectI {
        x: 24,
        y: 8,
        w: 16,
        h: 16,
    };
    let far = RectI {
        x: 96,
        y: 8,
        w: 16,
        h: 16,
    };
    state.record_damage_rects(&region(&[left, touching, far]), 400, 200);
    assert_eq!(
        state.pending,
        vec![left, touching, far],
        "regions that share no pixel stay separate, in the order they arrived"
    );
}

#[test]
fn test_record_damage_clips_before_it_merges() {
    let mut state = recording_state();
    // A region that runs off the surface: the part outside is dropped and what is left is
    // merged with the rectangle it overlaps, so the entry handed to the copy is inside the
    // surface and the copy can refuse rather than clamp.
    let inside = RectI {
        x: 0,
        y: 0,
        w: 20,
        h: 20,
    };
    let straddling = RectI {
        x: 10,
        y: 10,
        w: 40,
        h: 40,
    };
    state.record_damage_rects(&region(&[inside, straddling]), 20, 20);
    assert_eq!(
        state.pending,
        vec![RectI {
            x: 0,
            y: 0,
            w: 20,
            h: 20
        }],
        "the clipped part merges with the region it overlaps"
    );
    // A region entirely outside the surface damages nothing at all.
    state.pending.clear();
    let outside = RectI {
        x: 40,
        y: 40,
        w: 4,
        h: 4,
    };
    state.record_damage_rects(&region(&[outside]), 20, 20);
    assert!(state.pending.is_empty());
}

#[test]
fn test_record_damage_folds_a_list_of_disjoint_regions_past_the_limit() {
    // A starvation streak, or a scene that changed all over: regions that share no pixel
    // cannot be merged, so the list grows until the collapse bounds it.
    let mut state = recording_state();
    let rects: Vec<RectI> = (0..PENDING_COLLAPSE_LIMIT + 1)
        .map(|step| RectI {
            x: step as i32 * 8,
            y: 0,
            w: 4,
            h: 4,
        })
        .collect();
    state.record_damage_rects(&region(&rects), 400, 200);
    assert_eq!(
        state.pending.len(),
        1,
        "one region past the limit folds the list into its bounding box"
    );
    assert_eq!(
        state.pending[0],
        union_rect(&rects),
        "and the box covers every region the frame damaged"
    );
}

#[test]
fn test_record_damage_of_a_full_repaint_reports_the_whole_surface_once() {
    // The frame that follows a reallocation, a visibility flip or a geometry change: one
    // entry covering the surface, whatever the renderer's region says.
    let mut state = FrameState::new();
    assert!(state.full, "a fresh state repaints everything");
    let changed = RectI {
        x: 8,
        y: 8,
        w: 4,
        h: 4,
    };
    state.record_damage_rects(&region(&[changed]), 400, 200);
    assert_eq!(
        state.pending,
        vec![RectI {
            x: 0,
            y: 0,
            w: 400,
            h: 200
        }],
        "a full repaint is the whole surface as a single rectangle"
    );
    assert_eq!(
        state.pending.len(),
        1,
        "which makes the frame report one rectangle"
    );
}

#[test]
fn test_overlaps_requires_a_shared_pixel() {
    // The predicate the merge is built on: two regions that share a pixel are folded
    // together, and two that do not are left apart. Touching along an edge is the case worth
    // pinning -- it looks like an overlap and shares nothing, so merging it would add the
    // whole area of both to the copy without removing an entry from the list.
    let base = RectI {
        x: 10,
        y: 10,
        w: 10,
        h: 10,
    };
    let partial = RectI {
        x: 15,
        y: 15,
        w: 10,
        h: 10,
    };
    let one_pixel = RectI {
        x: 19,
        y: 19,
        w: 10,
        h: 10,
    };
    assert!(overlaps(base, base), "a rectangle overlaps itself");
    assert!(overlaps(base, partial), "a partial overlap counts");
    assert!(overlaps(base, one_pixel), "so does a one-pixel overlap");
    let right_edge = RectI {
        x: 20,
        y: 10,
        w: 10,
        h: 10,
    };
    let bottom_edge = RectI {
        x: 10,
        y: 20,
        w: 10,
        h: 10,
    };
    let far = RectI {
        x: 30,
        y: 30,
        w: 4,
        h: 4,
    };
    assert!(!overlaps(base, right_edge));
    assert!(!overlaps(base, bottom_edge));
    assert!(!overlaps(base, far));
    // A degenerate rectangle covers nothing, even when its origin is inside the other.
    let flat = RectI {
        x: 12,
        y: 12,
        w: 0,
        h: 4,
    };
    let thin = RectI {
        x: 12,
        y: 12,
        w: 4,
        h: 0,
    };
    let empty = RectI {
        x: i32::MIN,
        y: i32::MIN,
        w: 0,
        h: 0,
    };
    assert!(!overlaps(base, flat));
    assert!(!overlaps(base, thin));
    assert!(!overlaps(empty, base));
    // Symmetric, and `i32::MIN` must not overflow the edge arithmetic.
    assert_eq!(overlaps(base, one_pixel), overlaps(one_pixel, base));
    let everywhere = RectI {
        x: i32::MIN,
        y: i32::MIN,
        w: u32::MAX,
        h: u32::MAX,
    };
    assert!(overlaps(everywhere, base));
}
