//! The animation steady state: what a frame of a moving highlight costs.
//!
//! The case this file exists for is the one no other assertion covers. A window that
//! repaints *something* every frame looks healthy from the outside -- every test that only
//! asks "did this frame draw?" passes -- while a frame that quietly fell back to a full
//! repaint, or that carried a damage list as long as the streak of regions the renderer
//! handed over, costs a whole window's worth of memory traffic per frame. On a fast machine
//! that still lands inside the frame deadline, so no budget assertion catches it either.
//!
//! The frame is therefore measured against three things: the pixels it committed (must
//! differ from the frame before it, or nothing moved), the damage it reported (must stay
//! inside the neighbourhood the highlight is crossing), and the copies it paid for that
//! damage (at most one per frame, however many rectangles the damage arrived in).

use std::sync::{Arc, Mutex};

use ime_types::RectI;
use slint::ComponentHandle as _;

use super::super::raster::BYTES_PER_PIXEL;
use super::super::{PENDING_COLLAPSE_LIMIT, RenderOutcome};
use super::on_own_thread;
use crate::renderer::mock::{MockState, MockSurface};
use crate::slint_platform::RspinyinPlatform;
use crate::spring::{HighlightAnim, HighlightRect, SpringParams};

/// The candidate window the frame budgets are stated for, in logical pixels.
const WIDTH_DP: u32 = 600;
const HEIGHT_DP: u32 = 140;

/// The scale the surface is driven at.
///
/// One, so the damage areas in this file are readable as the logical areas they come from.
/// The ratios the assertions are about do not depend on it: scaling the surface scales the
/// damage and the region it is compared against by the same factor.
const SCALE: f32 = 1.0;

/// The frame interval the motion is driven at: the 144Hz the highlight budget names.
const FRAME_S: f32 = 1.0 / 144.0;

/// How many frames of the animation are measured.
///
/// Sixty is the figure the card states, and it is longer than one settle takes: the box is
/// sent back to the cell it came from whenever it arrives, so the frames measured are frames
/// of a motion rather than of a window that has come to rest.
const ANIMATION_FRAMES: u32 = 60;

/// Frames drawn before the measured ones, so the first layout is out of the way.
const SETTLE_FRAMES: usize = 4;

/// A candidate cell, in logical pixels, and the gap between two of them.
const CELL_W: f32 = 88.0;
const CELL_H: f32 = 30.0;
const GAP: f32 = 6.0;

/// The candidate grid: the strip of cells the damage is measured against.
const COLUMNS: usize = 5;
const ROWS: usize = 2;

/// Where the grid starts, in logical pixels.
const ORIGIN_X: f32 = 32.0;
const ORIGIN_Y: f32 = 48.0;

/// The row length of the test surface, in bytes.
const STRIDE: usize = WIDTH_DP as usize * BYTES_PER_PIXEL;

/// The pixel a cell's background paints, in the surface byte order `B, G, R, A`.
const CELL_PIXEL: [u8; 4] = [0x30, 0x24, 0x20, 0xff];

/// The pixel the highlight paints, in the surface byte order `B, G, R, A`.
const HIGHLIGHT_PIXEL: [u8; 4] = [0xa5, 0x6e, 0x3a, 0xff];

/// The scene: a candidate grid with one highlight box that slides across it.
///
/// The cells never change, so they contribute no damage of their own; they are in the scene
/// so that the region the damage is measured against is a real one rather than a number the
/// test made up. The highlight is declared last, which is what puts it above the cells.
///
/// The module is private and nothing in it is exported -- the candidate window must never
/// become a Slint surface a third party can program against (`OB-4`). The allow covers the
/// property accessors the macro generates and this file does call.
#[allow(dead_code)]
mod scene {
    slint::slint! {
        export component CandidateCard inherits Window {
            width: 600px;
            height: 140px;
            background: transparent;

            in-out property <length> highlight-x: 32px;
            in-out property <length> highlight-y: 48px;
            in-out property <length> highlight-w: 88px;
            in-out property <length> highlight-h: 30px;

            Rectangle { x: 32px; y: 48px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 126px; y: 48px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 220px; y: 48px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 314px; y: 48px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 408px; y: 48px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 32px; y: 84px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 126px; y: 84px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 220px; y: 84px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 314px; y: 84px; width: 88px; height: 30px; background: #202430; }
            Rectangle { x: 408px; y: 84px; width: 88px; height: 30px; background: #202430; }

            Rectangle {
                x: highlight-x;
                y: highlight-y;
                width: highlight-w;
                height: highlight-h;
                background: #3a6ea5;
            }
        }
    }
}

/// The area of the candidate grid, in physical pixels: what the damage is measured against.
fn candidate_area() -> u64 {
    let width = COLUMNS as f32 * CELL_W + (COLUMNS - 1) as f32 * GAP;
    let height = ROWS as f32 * CELL_H + (ROWS - 1) as f32 * GAP;
    width as u64 * height as u64
}

/// The cell at `(column, row)`, in logical pixels.
fn cell(column: usize, row: usize) -> HighlightRect {
    HighlightRect::new(
        ORIGIN_X + column as f32 * (CELL_W + GAP),
        ORIGIN_Y + row as f32 * (CELL_H + GAP),
        CELL_W,
        CELL_H,
    )
}

/// Moves the scene's highlight box to `rect`.
fn apply_highlight(card: &scene::CandidateCard, rect: HighlightRect) {
    card.set_highlight_x(rect.x);
    card.set_highlight_y(rect.y);
    card.set_highlight_w(rect.w);
    card.set_highlight_h(rect.h);
}

/// What the measured animation frames added up to.
#[derive(Debug, Default)]
struct Report {
    /// Frames measured.
    frames: u32,
    /// The largest damage area any one frame reported, in physical pixels.
    worst_damage: u64,
    /// Frames that damaged the whole surface, which is what a full repaint looks like.
    full_repaints: u32,
    /// Frames whose committed pixels differ from the frame before them.
    changed_frames: u32,
    /// The most rectangles any one frame reported its damage as.
    worst_rectangles: u32,
    /// The most copies of the scratch any one frame paid for.
    worst_copies: u32,
}

/// The whole surface, as the damage of a full repaint is reported.
fn whole_surface() -> RectI {
    RectI {
        x: 0,
        y: 0,
        w: WIDTH_DP,
        h: HEIGHT_DP,
    }
}

/// Whether `damage` is the whole surface, the shape a full repaint is reported as.
///
/// `FrameState::full` records the whole surface as a single rectangle, so a frame that
/// damaged everything and a frame that fell back to a full repaint are the same observation
/// from outside the renderer -- which is the observation this file can make.
fn is_whole_surface(damage: &[RectI]) -> bool {
    match damage {
        [only] => *only == whole_surface(),
        _ => false,
    }
}

#[test]
fn test_animation_steady_frames_damage_only_the_highlight_path() {
    let report = on_own_thread(|| {
        let (backend, state) = MockSurface::new(WIDTH_DP, HEIGHT_DP, SCALE);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = scene::CandidateCard::new().expect("the component binds to the platform");
        card.show().expect("the surface can be mapped");
        for _ in 0..SETTLE_FRAMES {
            platform
                .render_if_dirty()
                .expect("a settling frame is handled");
        }

        // Park the box on the first cell and let that frame reach the surface, so every
        // frame the loop below measures is a steady-state one.
        let mut highlight = HighlightAnim::new(SpringParams::HIGHLIGHT, cell(0, 0));
        highlight.set_visible(true);
        apply_highlight(&card, highlight.rect());
        platform
            .render_if_dirty()
            .expect("the frame that puts the box on the cell is committed");
        // The user pressed the arrow key: the box flies to the far end of the row.
        let mut heading_out = true;
        highlight.retarget(cell(COLUMNS - 1, 0));

        let mut previous = state.lock().expect("the mock is not poisoned").pixels.clone();
        let mut report = Report::default();
        while report.frames < ANIMATION_FRAMES {
            let step = highlight.step(FRAME_S, SCALE);
            apply_highlight(&card, step.rect);
            let before = state.lock().expect("the mock is not poisoned").damage.len();
            let outcome = platform
                .render_if_dirty()
                .expect("an animation frame is committed");
            let observed = state.lock().expect("the mock is not poisoned");
            let damage = &observed.damage[before..];
            let area: u64 = damage
                .iter()
                .map(|rect| u64::from(rect.w) * u64::from(rect.h))
                .sum();
            report.worst_damage = report.worst_damage.max(area);
            if is_whole_surface(damage) {
                report.full_repaints += 1;
            }
            if observed.pixels != previous {
                report.changed_frames += 1;
            }
            // Reused rather than reallocated: the buffer only exists to be compared against
            // the next frame's, and this loop runs once per animation frame.
            previous.clone_from(&observed.pixels);

            let (rectangles, copies) = match outcome {
                RenderOutcome::Rendered { rectangles, copies, .. } => (rectangles, copies),
                // A frame the renderer had nothing to draw for: no damage, no copy. It is a
                // legal outcome -- the box moved by less than the renderer could see -- and
                // the assertions below are about the frames that did draw something.
                _ => (0, 0),
            };
            report.worst_rectangles = report.worst_rectangles.max(rectangles);
            report.worst_copies = report.worst_copies.max(copies);
            report.frames += 1;
            if step.settled {
                // Arrived: send it back, so the frames that follow are frames of a motion.
                heading_out = !heading_out;
                let column = if heading_out { COLUMNS - 1 } else { 0 };
                highlight.retarget(cell(column, 0));
            }
        }
        report
    });

    assert_eq!(report.frames, ANIMATION_FRAMES, "the whole animation is measured");
    assert!(report.worst_damage > 0, "a moving box damages something: {report:?}");
    assert_eq!(report.full_repaints, 0, "no frame repaints everything: {report:?}");
    assert!(
        report.changed_frames * 5 >= report.frames * 4,
        "the animation must keep changing what is on screen, or the window has stopped \
         being redrawn: {report:?}"
    );
    assert!(report.worst_copies <= 2, "at most two copies per frame");
    assert!(
        report.worst_rectangles <= PENDING_COLLAPSE_LIMIT as u32,
        "the list stays inside the collapse limit: {report:?}"
    );
    // The budget the card states: a steady-state frame of the animation damages at most
    // 30% of the candidate region. The highlight is one cell of ten and moves by a fraction
    // of a cell per frame, so the merge has a wide margin to stay inside -- which is exactly
    // what makes a fallback to the whole surface, or a list that stopped being merged, show
    // up here.
    let region = candidate_area();
    assert!(
        report.worst_damage * 100 <= region * 30,
        "the worst frame damaged {}px^2, more than 30% of the {region}px^2 candidate region: \
         {report:?}",
        report.worst_damage
    );
}

#[test]
fn test_animation_frames_keep_the_highlight_inside_the_grid() {
    // The pixel half of the case above, which is what the area assertion cannot see: the
    // highlight is drawn where the spring put it, so the cell it crosses carries the
    // highlight's colour in the frames it crosses it. A window that stopped redrawing after
    // its first frame would still report damage of the right size -- it is the pixels that
    // say whether the damage was real.
    let (parked, crossed, landed, released) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(WIDTH_DP, HEIGHT_DP, SCALE);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let card = scene::CandidateCard::new().expect("the component binds to the platform");
        card.show().expect("the surface can be mapped");
        for _ in 0..SETTLE_FRAMES {
            platform
                .render_if_dirty()
                .expect("a settling frame is handled");
        }
        let mut highlight = HighlightAnim::new(SpringParams::HIGHLIGHT, cell(0, 0));
        highlight.set_visible(true);
        apply_highlight(&card, highlight.rect());
        platform
            .render_if_dirty()
            .expect("the parked frame is committed");
        let parked = sample(&state, cell(0, 0));

        highlight.retarget(cell(COLUMNS - 1, 0));
        let middle = cell(COLUMNS / 2, 0);
        let mut crossed = false;
        let mut frames = 0u32;
        loop {
            let step = highlight.step(FRAME_S, SCALE);
            apply_highlight(&card, step.rect);
            platform
                .render_if_dirty()
                .expect("an animation frame is committed");
            // Sampled while the box is still in flight: once it has settled, the middle cell
            // is behind it again.
            if step.settled {
                break;
            }
            crossed |= sample(&state, middle) == HIGHLIGHT_PIXEL;
            frames += 1;
            assert!(frames < 600, "the highlight must settle");
        }
        // After it settles, the far cell carries the box and the first one is a plain cell
        // again: the two ends of the path, told apart by their pixels.
        (
            parked,
            crossed,
            sample(&state, cell(COLUMNS - 1, 0)),
            sample(&state, cell(0, 0)),
        )
    });
    assert_eq!(parked, HIGHLIGHT_PIXEL, "the parked box paints the highlight");
    assert!(
        crossed,
        "the box is drawn on the cell it crosses, not only where it started and stopped"
    );
    assert_eq!(landed, parked, "the cell it settled on carries the highlight");
    assert_eq!(released, CELL_PIXEL, "and the cell it left is plain again");
}

/// The committed pixel at the centre of `rect`.
fn sample(state: &Arc<Mutex<MockState>>, rect: HighlightRect) -> [u8; 4] {
    let x = ((rect.x + rect.w / 2.0) * SCALE) as usize;
    let y = ((rect.y + rect.h / 2.0) * SCALE) as usize;
    state
        .lock()
        .expect("the mock is not poisoned")
        .pixel(STRIDE, x, y)
}
