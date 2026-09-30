//! The scratch a window gives back: what happens on the frame that shrinks it.
//!
//! A candidate window's surface is not monotonic. A scale change, a second monitor, a
//! candidate list that grew to five rows and then shrank to one -- all of them move the
//! surface, and the scratch followed the largest one for the rest of the session until it
//! was allowed to shrink. That is a memory trade, but it has a correctness edge that is easy
//! to miss: the frame that shrinks the scratch is a frame whose buffer no longer holds what
//! the surface is showing, so it has to repaint all of it. A frame that copied only its own
//! damage would leave the rest of the window showing whatever the old buffer happened to
//! contain.
//!
//! The case here drives that frame through a real component on a mock surface, so what is
//! asserted is what the compositor would have been told. The threshold itself -- how many
//! small frames it takes, and that a jitter never triggers one -- is asserted directly on
//! the scratch below, where it does not need a scene to drive it.

use std::sync::{Arc, Mutex};

use ime_types::{RectI, SurfaceEvent};

use super::super::RenderOutcome;
use super::super::raster::{BYTES_PER_PIXEL, PixelScratch, SHRINK_AFTER_FRAMES};
use super::{on_own_thread, show_card};
use crate::renderer::mock::MockState;
use crate::slint_platform::RspinyinPlatform;

/// The surface the window starts at, which is the size the test scene is built for.
const SMALL: (u32, u32) = (160, 64);

/// The peak the window is grown to before it shrinks back.
///
/// Chosen so that the small surface needs well under a quarter of the peak allocation: a
/// quarter of `640 * 256` is `40_960` pixels and a small frame needs `162 * 65 = 10_530`, so
/// every small frame qualifies and the streak is what decides the shrink.
const PEAK: (u32, u32) = (640, 256);

/// The whole surface, as the damage of a full repaint is reported.
fn whole_surface() -> RectI {
    RectI {
        x: 0,
        y: 0,
        w: SMALL.0,
        h: SMALL.1,
    }
}

/// Reports a surface size to the platform, as a compositor's configure would.
///
/// The mock adopts the size in its own `poll_events` and the platform then applies it to the
/// window, which is the order the real backends work in.
fn resize(
    platform: &RspinyinPlatform,
    state: &Arc<Mutex<MockState>>,
    (width_px, height_px): (u32, u32),
) {
    let event = SurfaceEvent::Resize {
        w: width_px,
        h: height_px,
    };
    state
        .lock()
        .expect("the mock is not poisoned")
        .pending
        .push(event);
    platform
        .poll_events(&mut Vec::new())
        .expect("the resize is applied");
}

/// The fields of a committed frame, or `None` when the frame did not reach the surface.
fn rendered(outcome: RenderOutcome) -> Option<(RectI, u32, u32, u64)> {
    match outcome {
        RenderOutcome::Rendered {
            bounding,
            rectangles,
            copies,
            copy_bytes,
        } => Some((bounding, rectangles, copies, copy_bytes)),
        _ => None,
    }
}

#[test]
fn test_a_shrinking_scratch_repaints_the_whole_surface() {
    let (after_resize, partial, shrinking) = on_own_thread(|| {
        let (platform, card, state) = show_card();
        platform
            .render_if_dirty()
            .expect("the first frame is committed");

        // Grow the surface, so the scratch holds a peak the window no longer needs.
        resize(&platform, &state, PEAK);
        platform
            .render_if_dirty()
            .expect("the frame at the peak size is committed");
        // And shrink it back. The scratch keeps the peak allocation: one small frame is a
        // resize, not evidence that the window has settled at the smaller size.
        resize(&platform, &state, SMALL);
        let after_resize = platform
            .render_if_dirty()
            .expect("the frame after the resize is committed");

        // The frames that follow are partial ones, until the scratch is given back -- which
        // happens on the `SHRINK_AFTER_FRAMES`-th frame at the small size. This loop drives
        // exactly the rest of that streak, one dirty frame per step.
        let mut highlight = false;
        let mut partial = None;
        let mut shrinking = None;
        for frame in 2..=SHRINK_AFTER_FRAMES {
            highlight = !highlight;
            card.set_highlight(highlight);
            let outcome = platform
                .render_if_dirty()
                .expect("a frame of the shrink streak is committed");
            if frame == SHRINK_AFTER_FRAMES {
                shrinking = Some(outcome);
            } else {
                partial = Some(outcome);
            }
        }
        (after_resize, partial, shrinking)
    });

    // The resize itself is a full repaint: the window's geometry changed.
    assert!(
        rendered(after_resize).is_some_and(|(bounding, ..)| bounding == whole_surface()),
        "the frame after a resize repaints everything: {after_resize:?}"
    );
    let partial = partial.expect("the streak has frames before its last one");
    assert!(
        !rendered(partial).is_some_and(|(bounding, ..)| bounding == whole_surface()),
        "the frames of the streak are partial ones: {partial:?}"
    );
    let shrinking = shrinking.expect("the streak ends on the frame that shrinks the scratch");
    let (bounding, rectangles, copies, copy_bytes) =
        rendered(shrinking).expect("the shrinking frame reaches the surface");
    assert_eq!(bounding, whole_surface(), "the shrinking frame repaints it all");
    assert_eq!(rectangles, 1, "a full repaint is one rectangle");
    assert_eq!(copies, 1, "and it is carried over in one copy");
    assert_eq!(
        copy_bytes,
        u64::from(SMALL.0) * u64::from(SMALL.1) * BYTES_PER_PIXEL as u64,
        "the copy covers the whole surface, which is what makes the frame on screen valid"
    );
}

/// The number of pixels a fresh scratch allocates for `(width_px, height_px)`.
///
/// Asked of the type rather than worked out here, so that these cases do not have to know
/// the slack a scratch row and frame carry.
fn demand((width_px, height_px): (u32, u32)) -> usize {
    let mut scratch = PixelScratch::new();
    scratch.ensure(width_px, height_px);
    scratch.pixels.len()
}

#[test]
fn test_a_sustained_smaller_surface_gives_the_allocation_back() {
    let mut scratch = PixelScratch::new();
    assert!(scratch.ensure(PEAK.0, PEAK.1), "the peak surface allocates");
    let peak = scratch.pixels.capacity();
    // The frames before the threshold: the smaller surface fits, so nothing is reallocated
    // and the peak allocation is still held.
    for frame in 1..SHRINK_AFTER_FRAMES {
        assert!(
            !scratch.ensure(SMALL.0, SMALL.1),
            "frame {frame} of the streak reallocates nothing"
        );
    }
    assert_eq!(
        scratch.pixels.capacity(),
        peak,
        "the allocation is held until the streak is long enough"
    );
    let shrank = scratch.ensure(SMALL.0, SMALL.1);
    assert!(shrank, "the frame past the threshold reallocates");
    let needed = demand(SMALL);
    assert!(
        scratch.pixels.capacity() <= needed + needed / 2,
        "the capacity falls to the demand: {} against {needed}",
        scratch.pixels.capacity()
    );
    let covered = scratch.pixels.len() >= needed;
    assert!(covered, "and still covers the frame it was sized for");
}

#[test]
fn test_a_jittering_size_never_gives_the_allocation_back() {
    // A window whose size the user is dragging: the sizes asked for span a factor of four, so
    // the small frames would each qualify on their own. The run is long enough that a counter
    // which was never reset would fire -- twice over, in fact -- which is what makes the
    // reset, rather than the threshold, the property under test.
    let mut scratch = PixelScratch::new();
    assert!(scratch.ensure(400, 200));
    let held = scratch.pixels.capacity();
    for step in 0..SHRINK_AFTER_FRAMES * 2 {
        let small = step % 2 == 0;
        let (width_px, height_px) = if small { (100, 50) } else { (400, 200) };
        assert!(
            !scratch.ensure(width_px, height_px),
            "step {step} asks for a size the allocation already covers"
        );
    }
    assert_eq!(
        scratch.pixels.capacity(),
        held,
        "a jitter is not a shrink, however long it lasts"
    );
}
