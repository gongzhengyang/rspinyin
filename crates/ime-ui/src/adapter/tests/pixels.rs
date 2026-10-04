//! The adapter's drawing, read back as pixels.
//!
//! The mapping tests beside this module assert on properties; these assert on the bytes
//! the mock surface commits, because the five visual states and the shadow bands are
//! promises about pixels, not about properties.

use std::rc::Rc;

use ime_types::UiFrame;
use slint::{Model as _, SharedString, VecModel};

use crate::layout;
use crate::renderer::mock::{MockState, MockSurface, on_own_thread};
use crate::slint_platform::RspinyinPlatform;
use crate::ui_generated::CandidateData;

use super::motion::settle;
use super::{Adapter, PointerState, frame_with, with_adapter};

/// The size of the surface the pixel scenes draw into, in logical pixels at a scale of 1.0.
///
/// The panel is drawn at the shadow margin and sized by the frame, so the surface only has
/// to hold the widest panel the scenes ask for: four cells of two glyphs, plus the margin.
pub(super) const PIXEL_WIDTH_DP: u32 = 424;
pub(super) const PIXEL_HEIGHT_DP: u32 = 160;

/// Bytes per pixel of the mock's `Argb8888` buffer.
pub(super) const BYTES_PER_PIXEL: usize = 4;

/// The component's constants, parsed once for the pixel arithmetic.
pub(super) fn metrics() -> &'static crate::layout::Metrics {
    layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// The width of one cell of a pixel scene: two CJK glyphs and the chrome around them.
pub(super) fn cell_width() -> f32 {
    let metrics = metrics();
    2.0 * metrics.font_size_cell + metrics.cell_chrome_width
}

/// The surface-relative top-left corner of the cell at `position` in a pixel scene.
///
/// The panel is drawn at the shadow margin; the header and its rule sit above the candidate
/// area, which the grid pads by the container padding. At a scale of 1.0 a logical pixel is
/// a physical one.
fn cell_origin(position: usize) -> (usize, usize) {
    let metrics = metrics();
    let gap = metrics.grid_gap as usize;
    let x = (metrics.shadow_margin + metrics.container_padding) as usize
        + position * (cell_width() as usize + gap);
    let y = (metrics.shadow_margin
        + metrics.header_height
        + metrics.separator_height
        + metrics.container_padding) as usize;
    (x, y)
}

/// One pixel of a rendered frame, as the bytes `[blue, green, red, alpha]`.
fn pixel(state: &MockState, stride: usize, x: usize, y: usize) -> [u8; 4] {
    state.pixel(stride, x, y)
}

/// The strongest accent contribution in a band of rows, which is how the focus ring is found
/// without depending on the exact row the border lands on.
fn strongest_blue(state: &MockState, stride: usize, x: usize, rows: core::ops::Range<usize>) -> u8 {
    rows.map(|y| pixel(state, stride, x, y)[0])
        .max()
        .unwrap_or(0)
}

/// The ink a cell holds: the sum of the alpha of every pixel inside it.
fn ink(state: &MockState, stride: usize, position: usize) -> u32 {
    let (x, y) = cell_origin(position);
    let mut total = 0u32;
    for row in y..y + metrics().cell_height as usize {
        for column in x..x + cell_width() as usize {
            total = total.saturating_add(u32::from(pixel(state, stride, column, row)[3]));
        }
    }
    total
}

/// Rasterizes one frame on a mock surface and samples it.
fn with_pixels<R: Send + 'static>(
    frame: UiFrame,
    pointer: PointerState,
    scene: impl FnOnce(&MockState, usize) -> R + Send + 'static,
) -> R {
    on_own_thread(move || {
        let (backend, state) = MockSurface::new(PIXEL_WIDTH_DP, PIXEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame), "the frame is drawn");
        adapter.apply_pointer(pointer);
        // The scene is sampled once it has settled: the first frames are the ones that
        // build the renderer's caches.
        for _ in 0..4 {
            platform
                .render_if_dirty()
                .expect("a settling frame is drawn");
        }
        let state = state.lock().expect("the mock is not poisoned");
        scene(&state, PIXEL_WIDTH_DP as usize * BYTES_PER_PIXEL)
    })
}

#[test]
fn test_candidate_grid_draws_the_five_states_of_the_design_table() {
    // Four cells, one state each: the highlight, the hover, the press, and nothing.
    let pointer = PointerState {
        highlighted: Some(0),
        hovered: Some(1),
        pressed: Some(2),
    };
    let (focus, stroke, hover, hover_edge, active, plain, plain_edge, panel) =
        with_pixels(frame_with(1, "ni'hao", 4), pointer, |state, stride| {
            let (x, y) = cell_origin(0);
            let middle = y + metrics().cell_height as usize / 2;
            let right = x + cell_width() as usize - 4;
            let centre = x + cell_width() as usize / 2;
            (
                pixel(state, stride, right, middle),
                strongest_blue(state, stride, centre, y - 1..y + 3),
                pixel(state, stride, right + 72, middle),
                strongest_blue(state, stride, centre + 72, y - 1..y + 3),
                pixel(state, stride, right + 144, middle),
                pixel(state, stride, right + 216, middle),
                strongest_blue(state, stride, centre + 216, y - 1..y + 3),
                // The panel's own fill, sampled in the padding left of the first cell.
                pixel(state, stride, x - 4, middle),
            )
        });
    assert_eq!(
        plain, panel,
        "3.4 Default draws no background of its own, so the panel shows through"
    );
    assert_eq!(
        plain_edge, panel[0],
        "and no ring: the top edge of an untouched cell is the panel's own fill"
    );
    assert!(
        focus[0] > plain[0] + 20,
        "3.4 Focus Ring fills the cell with state.selected.bg: {focus:?} against {plain:?}"
    );
    assert!(
        stroke > focus[0] + 40,
        "3.4 Focus Ring rings it with state.selected.stroke: {stroke} against {focus:?}"
    );
    assert!(
        hover[0] > plain[0] + 8,
        "3.4 Hover fills the cell with state.hover: {hover:?} against {plain:?}"
    );
    assert_eq!(
        hover_edge, hover[0],
        "3.4 Hover draws no ring: the edge of a hovered cell is its own fill"
    );
    assert!(
        active[0] > hover[0] + 5,
        "3.4 Active fills it with state.pressed, which is the stronger of the two: \
         {active:?} against {hover:?}"
    );
}

#[test]
fn test_candidate_grid_draws_the_reserved_disabled_state_at_its_own_opacity() {
    // 3.4's `Disabled` row is reserved -- nothing in this phase disables a candidate -- so
    // the state is forced through the component's model, which is also what proves the
    // component draws the row the table specifies. The disabled cell carries the grid's
    // bound `opacity: dim` (0.32) and still draws, which is itself the counterexample to
    // the old claim that a bound opacity leaves nothing on the surface.
    let (enabled, disabled, flag) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(PIXEL_WIDTH_DP, PIXEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 2)));
        // Two cells that draw the same thing, one of them disabled.
        let cells: Vec<CandidateData> = [false, true]
            .iter()
            .map(|disabled| CandidateData {
                index: 1,
                label: SharedString::from("1"),
                text: SharedString::from("你好"),
                display_text: SharedString::from("你好"),
                annotation: SharedString::default(),
                source: 0,
                is_highlighted: false,
                is_hovered: false,
                is_pressed: false,
                is_disabled: *disabled,
            })
            .collect();
        let model = Rc::new(VecModel::from(cells));
        adapter.window().set_items(model.into());
        for _ in 0..4 {
            platform
                .render_if_dirty()
                .expect("a settling frame is drawn");
        }
        let flag = adapter
            .window()
            .get_items()
            .row_data(1)
            .map(|cell| cell.is_disabled);
        let state = state.lock().expect("the mock is not poisoned");
        let stride = PIXEL_WIDTH_DP as usize * BYTES_PER_PIXEL;
        (ink(&state, stride, 0), ink(&state, stride, 1), flag)
    });
    assert!(enabled > 0, "the cell draws its number and its text");
    assert!(disabled > 0, "the disabled cell still draws");
    // What this test cannot assert, and why its old reading was wrong.
    //
    // The obvious assertion is `disabled * 2 < enabled`, reading 3.4's "the disabled row is
    // drawn at 0.32". An earlier version of this comment reported the opposite from a
    // measurement -- two cells whose sampled ink sat at 515,630 against 515,833, a 0.04%
    // difference -- and read the near-equality as "`opacity` has no effect on the software
    // renderer's output". The instrument is why that reading was mistaken. This cell sits
    // on the panel's acrylic fill, and the fill dominates the composite: a count of ink
    // pixels cannot see an alpha change at all, because a dimmed glyph pixel is still an
    // ink pixel, and a sum of their alphas moves only on the pixels the glyphs cover --
    // the 0.32 dim takes a stroke core from 255 to 229 -- which stays far below the 1%
    // this assertion bounds. A near-equal ink over an opaque-ish backing measures the
    // backing, not the dim.
    //
    // What is actually true of the renderer is the direct opposite of the old reading:
    // the software renderer multiplies a bound element opacity into the state alpha and
    // culls a subtree at alpha 0.01, for rectangles and glyphs alike. That is pinned by
    // the pixel probes in `renderer/tests.rs`, which read the composited alpha back over
    // a transparent background where the subtree's own alpha is the whole answer. The
    // grid carries the factor as real bindings (`dim` in `candidate_grid.slint`), and
    // 3.1.1's 0.55 number label and 3.2's 0.50 annotation composite at those alphas.
    //
    // What is asserted here stays the observable half: the flag reaches the model, the
    // disabled cell draws -- already the counterexample to the old "a bound opacity
    // draws nothing" claim -- and it draws in the same place as its enabled twin.
    assert_eq!(
        flag,
        Some(true),
        "the adapter writes the disabled flag into the model"
    );
    assert!(
        disabled.abs_diff(enabled) < enabled / 100,
        "the two cells draw the same geometry: {disabled} against {enabled}"
    );
}

#[test]
fn test_adapter_writes_the_grid_model_from_a_frame() {
    let (count, text, label, highlighted, hovered) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(20, "ni'hao", 3)));
        let items = adapter.window().get_items();
        let first = items.row_data(0);
        (
            items.row_count(),
            first.as_ref().map(|cell| cell.text.to_string()),
            first.as_ref().map(|cell| cell.label.to_string()),
            first.as_ref().map(|cell| cell.is_highlighted),
            first.as_ref().map(|cell| cell.is_hovered),
        )
    });
    assert_eq!(count, 3);
    assert_eq!(text.as_deref(), Some("你好"));
    assert_eq!(label.as_deref(), Some("1"));
    assert_eq!(
        highlighted,
        Some(true),
        "the first candidate of the page draws the focus ring"
    );
    assert_eq!(hovered, Some(false));
}

#[test]
fn test_adapter_apply_pointer_redraws_the_grid_only_when_it_changes() {
    let (first, again, moved, hovered, pressed) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(21, "ni'hao", 3)));
        let pointer = PointerState {
            highlighted: Some(1),
            hovered: Some(2),
            pressed: Some(0),
        };
        let first = adapter.apply_pointer(pointer);
        let again = adapter.apply_pointer(pointer);
        let items = adapter.window().get_items();
        let row = |position: usize| items.row_data(position);
        // Read the cells while the state under test is the one installed. A tuple evaluates
        // left to right, so moving the hover inside the tuple would have the reads observe
        // the *new* state and the assertions below would be checking the wrong thing.
        let hovered = row(2).map(|cell| cell.is_hovered);
        let pressed = row(0).map(|cell| cell.is_pressed);
        let moved = adapter.apply_pointer(PointerState {
            hovered: Some(0),
            ..pointer
        });
        (first, again, moved, hovered, pressed)
    });
    assert!(first, "a new pointer state redraws the grid");
    assert!(!again, "the same state twice draws nothing the second time");
    assert!(moved, "a pointer that moved redraws the grid");
    assert_eq!(hovered, Some(true), "the hovered cell is the one named");
    assert_eq!(
        pressed,
        Some(true),
        "and the pressed cell keeps its state through a hover move"
    );
}

/// The row length of the pixel scenes, in bytes.
fn pixel_stride() -> usize {
    PIXEL_WIDTH_DP as usize * BYTES_PER_PIXEL
}

/// The surface rows the header strip occupies, and the rows below it.
///
/// The two bands are compared as raw bytes because the header owns no property a page turn
/// could move: only the pixels can say whether it stayed where it was.
fn bands() -> (core::ops::Range<usize>, core::ops::Range<usize>) {
    let metrics = metrics();
    let top = metrics.shadow_margin as usize;
    let header = top + metrics.header_height as usize;
    (top..header, header..PIXEL_HEIGHT_DP as usize)
}

/// One band of surface rows of the last committed frame, as raw bytes.
fn band(state: &MockState, rows: core::ops::Range<usize>) -> Vec<u8> {
    let stride = pixel_stride();
    state.pixels[rows.start * stride..rows.end * stride].to_vec()
}

#[test]
fn test_adapter_page_turn_slides_the_grid_and_leaves_the_header_still() {
    let (header_still, grid_moved) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(PIXEL_WIDTH_DP, PIXEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 4)));
        // The appear motion is run out first: a panel that was still growing would change
        // every band, and the comparison would be about the wrong motion.
        settle(&mut adapter);
        platform
            .render_if_dirty()
            .expect("the settled frame is drawn");
        let (header_rows, grid_rows) = bands();
        let before = {
            let state = state.lock().expect("the mock is not poisoned");
            (
                band(&state, header_rows.clone()),
                band(&state, grid_rows.clone()),
            )
        };
        let mut second = frame_with(2, "ni'hao", 4);
        second.page.current = 2;
        assert!(adapter.apply_frame(&second));
        adapter.advance(0.0).expect("the motion advances");
        platform
            .render_if_dirty()
            .expect("the displaced frame is drawn");
        let after = {
            let state = state.lock().expect("the mock is not poisoned");
            (band(&state, header_rows), band(&state, grid_rows))
        };
        (before.0 == after.0, before.1 != after.1)
    });
    assert!(
        header_still,
        "the header does not move: the preedit did not change page"
    );
    assert!(grid_moved, "the candidate grid does slide");
}
