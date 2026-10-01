//! Unit tests for the frame adapter, and the frame fixtures its neighbours share.
//!
//! Every scene here runs on a thread of its own: Slint installs its platform per thread and
//! refuses a second one on the same thread, so a test that needs a component gets a fresh
//! thread. That is also why the assertions are collected inside the scene and returned as
//! plain values -- the component is reference counted and cannot cross the thread boundary.

use std::rc::Rc;

use ime_types::ui::{OverlayEntry, OverlayFrame, OverlayKind, OverlaySection};
use ime_types::{
    Anchor, Candidate, CandidateSource, ColorScheme, LayoutHint, PageState, Placement, Preedit,
    PreeditSpan, RectI, Rgba8, ScreenId, SpanKind, StatusStrip, ThemeSpec, UiFrame,
};
use slint::{ComponentHandle as _, Model as _, SharedString, VecModel};

use super::{Adapter, COMPONENT_FAILED_CODE, PointerState};
use crate::layout;
use crate::renderer::mock::{MockState, MockSurface, on_own_thread};
use crate::slint_platform::RspinyinPlatform;
use crate::spring::{PAGE_SLIDE_DP, REST_POSITION_DP};
use crate::theme::{BlurNegotiation, ThemeResolution};
use crate::ui_generated::{CandidateData, PreeditRun, Theme};

mod header;

/// The caret position every fixture anchors to.
pub(crate) fn anchor() -> Anchor {
    Anchor {
        cursor: RectI {
            x: 100,
            y: 200,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Below,
    }
}

/// The preedit of a reading typed to the end, as the preedit builder produces it.
///
/// The text is the canonical spelling -- syllables joined by the separator the user did not
/// have to type -- and the span list tiles it exactly with one zero-width cursor span at the
/// caret, which is what `ime_types::Preedit` guarantees and what the header is written
/// against.
pub(crate) fn preedit_fixture(reading: &str) -> Preedit {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (index, part) in reading.split('\'').enumerate() {
        if index > 0 {
            let start = text.len();
            text.push('\'');
            spans.push(PreeditSpan {
                start: start as u16,
                end: text.len() as u16,
                kind: SpanKind::Separator,
            });
        }
        let start = text.len();
        text.push_str(part);
        spans.push(PreeditSpan {
            start: start as u16,
            end: text.len() as u16,
            kind: SpanKind::Syllable,
        });
    }
    let caret = text.len() as u16;
    spans.push(PreeditSpan {
        start: caret,
        end: caret,
        kind: SpanKind::Cursor,
    });
    Preedit {
        text,
        caret: u32::from(caret),
        spans,
    }
}

/// A frame with `count` candidates, each drawing two glyphs, and a preedit.
///
/// The candidate text is deliberately two characters wide: it pins the cell width the layout
/// derives, which is what the mapping tests assert on.
pub(crate) fn frame_with(revision: u32, preedit: &str, count: usize) -> UiFrame {
    let candidates = (0..count)
        .map(|n| Candidate {
            index: (n + 1) as u16,
            text: String::from("你好"),
            annotation: None,
            source: CandidateSource::Dict,
            score: 0.0,
            consumed_syllables: 1,
        })
        .collect();
    UiFrame {
        revision,
        preedit: preedit_fixture(preedit),
        candidates,
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: anchor(),
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
    }
}

/// The text of every run of a preedit model, in order.
fn run_texts(model: &slint::ModelRc<PreeditRun>) -> Vec<String> {
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|run| run.text.to_string())
        .collect()
}

/// The kind of every run of a preedit model, in order.
fn run_kinds(model: &slint::ModelRc<PreeditRun>) -> Vec<i32> {
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|run| run.kind)
        .collect()
}

/// Builds an adapter on a fresh thread and runs `scene` against it.
fn with_adapter<R: Send + 'static>(scene: impl FnOnce(&mut Adapter) -> R + Send + 'static) -> R {
    on_own_thread(move || {
        let (backend, _state) = MockSurface::new(320, 160, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        scene(&mut adapter)
    })
}

#[test]
fn test_adapter_writes_the_panel_properties_from_a_frame() {
    let (count, rows, width, height, header, per_row, before, after) = with_adapter(|adapter| {
        let frame = frame_with(1, "ni'hao", 9);
        assert!(adapter.apply_frame(&frame), "the first frame is drawn");
        let window = adapter.window();
        (
            window.get_item_count(),
            window.get_grid_rows(),
            window.get_container_width(),
            window.get_container_height(),
            window.get_header_height(),
            window.get_max_per_row(),
            run_texts(&window.get_preedit_before()),
            run_texts(&window.get_preedit_after()),
        )
    });
    assert_eq!(count, 9);
    assert_eq!(rows, 2, "nine candidates at five per row");
    assert_eq!(width, 370.0);
    assert_eq!(height, 129.0);
    assert_eq!(
        header, 34.0,
        "the full strip is drawn when there is a candidate"
    );
    assert_eq!(per_row, 5);
    assert_eq!(
        before,
        ["ni", "'", "hao"],
        "the whole reading is drawn before a caret at its end"
    );
    assert!(after.is_empty());
}

#[test]
fn test_adapter_draws_the_compressed_header_when_there_is_no_candidate() {
    let (rows, count, header, height) = with_adapter(|adapter| {
        let frame = frame_with(1, "ni", 0);
        assert!(adapter.apply_frame(&frame));
        let window = adapter.window();
        (
            window.get_grid_rows(),
            window.get_item_count(),
            window.get_header_height(),
            window.get_container_height(),
        )
    });
    assert_eq!(count, 0);
    assert_eq!(rows, 0);
    assert_eq!(header, 28.0, "the compact strip of a preedit-only window");
    assert_eq!(height, 45.0, "the panel the layout sized for it");
}

#[test]
fn test_adapter_drops_a_frame_older_than_the_one_it_draws() {
    let (stale, drawn) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(5, "ni", 9)));
        let stale = adapter.apply_frame(&frame_with(3, "ni", 1));
        (stale, adapter.window().get_item_count())
    });
    assert!(!stale, "a frame older than the drawn one is dropped");
    assert_eq!(drawn, 9, "the window keeps drawing the newest frame");
}

#[test]
fn test_adapter_theme_reaches_the_component_global() {
    let (dark, approved, written, radius, width) = with_adapter(|adapter| {
        let spec = ThemeSpec {
            scheme: ColorScheme::Light,
            accent: Rgba8 {
                r: 0x0A,
                g: 0x6C,
                b: 0xFF,
                a: 255,
            },
            acrylic: false,
            base_alpha: 217,
            corner_radius_dp: 14,
            scale: 1.0,
        };
        let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Disabled);
        adapter.apply_theme(&spec, &resolution.tokens);
        let theme = adapter.window().global::<Theme>();
        (
            theme.get_dark(),
            f32::from(resolution.tokens.base_alpha) / 255.0,
            theme.get_base_alpha(),
            adapter.window().get_container_radius(),
            adapter.window().get_container_width(),
        )
    });
    assert!(
        !dark,
        "the light scheme reaches the component's theme global"
    );
    assert!(
        approved > 0.0 && approved <= 1.0,
        "the resolution approved an alpha the component can paint with, got {approved}"
    );
    assert!(
        (written - approved).abs() < 1.0e-6,
        "the alpha the contrast gate approved is the one written, got {written}"
    );
    assert_eq!(radius, 14.0, "the configured corner radius is drawn");
    assert_eq!(
        width, 220.0,
        "a theme switch writes no geometry: the panel keeps the width it was given"
    );
}

#[test]
fn test_adapter_visibility_maps_and_unmaps_the_surface() {
    let (shown, hidden) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(160, 64, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        let shown = state.lock().expect("the mock is not poisoned").visible;
        adapter
            .set_visible(false)
            .expect("the surface can be unmapped");
        let hidden = state.lock().expect("the mock is not poisoned").visible;
        (shown, hidden)
    });
    assert!(shown, "showing the window maps the surface");
    assert!(!hidden, "hiding it unmaps the surface again");
}

#[test]
fn test_adapter_second_component_on_the_same_thread_is_refused() {
    let (first_ok, second_error) = on_own_thread(|| {
        let (backend, _state) = MockSurface::new(160, 64, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        // The first component has to stay alive across the second call: the platform keeps
        // only a weak reference to the window it handed out, so a dropped component looks
        // like no window at all and a second one would be served.
        let first = Adapter::new();
        let second = Adapter::new();
        (first.is_ok(), second.err().map(|error| error.to_string()))
    });
    assert!(first_ok, "the first component binds to the platform");
    assert_eq!(
        second_error.as_deref(),
        Some("platform/compositor/unsupported: ui/slint/component"),
        "one surface, one window: the second component is refused with the stable code"
    );
    assert_eq!(COMPONENT_FAILED_CODE, "ui/slint/component");
}

/// The size of the surface the pixel scenes draw into, in logical pixels at a scale of 1.0.
///
/// The panel is drawn at the shadow margin and sized by the frame, so the surface only has
/// to hold the widest panel the scenes ask for: four cells of two glyphs, plus the margin.
const PIXEL_WIDTH_DP: u32 = 424;
const PIXEL_HEIGHT_DP: u32 = 160;

/// Bytes per pixel of the mock's `Argb8888` buffer.
const BYTES_PER_PIXEL: usize = 4;

/// The component's constants, parsed once for the pixel arithmetic.
fn metrics() -> &'static crate::layout::Metrics {
    layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// The width of one cell of a pixel scene: two CJK glyphs and the chrome around them.
fn cell_width() -> f32 {
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
    // component draws the row the table specifies.
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
    // What this test cannot assert, and why.
    //
    // The obvious assertion is `disabled * 2 < enabled`, reading 3.4's "the disabled row is
    // drawn at 0.32". It does not hold, and the reason is not the grid: **`opacity` has no
    // effect on the software renderer's output here**. Measured directly -- setting a text
    // element's opacity to 0.1, and then to a colour with alpha 0.1, both leave the sampled
    // ink at 515,630 against 515,833 for the enabled cell, a 0.04% difference. That is also
    // why 3.1.1's number label at 0.55 and 3.2's annotation at 0.50 are drawn at full
    // strength today.
    //
    // The grid still carries the factor (`dim` in `candidate_grid.slint`, folded into every
    // child that draws), because the specification says 0.32 and the source is where the
    // specification is checked. What is asserted here is the half that is observable: the
    // flag reaches the model, the disabled cell renders, and it renders in the same place
    // as its enabled twin. The renderer limitation is recorded rather than papered over.
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

#[test]
fn test_adapter_keeps_the_full_text_of_a_truncated_candidate() {
    let text: String = "你好世界".chars().cycle().take(32).collect();
    let expected = text.clone();
    let (drawn, full, index) = with_adapter(move |adapter| {
        let mut frame = frame_with(22, "ni", 1);
        frame.candidates[0].text = text;
        assert!(adapter.apply_frame(&frame));
        let items = adapter.window().get_items();
        let cell = items.row_data(0).expect("the page holds one candidate");
        (
            cell.display_text.to_string(),
            cell.text.to_string(),
            cell.index,
        )
    });
    assert!(drawn.ends_with('…'), "the cell shows a cut text: {drawn:?}");
    assert_ne!(drawn, full);
    assert_eq!(
        full, expected,
        "the text a selection commits travels beside the text that is drawn"
    );
    assert_eq!(
        index, 1,
        "and the selection names the candidate by index, never by its text"
    );
}

/// The frame rate the motion tests step at, in seconds per frame.
///
/// The design targets 144Hz and the integrator clamps a step to 1/60s, so stepping at the
/// target rate is what the durations the design quotes are measured against.
const FRAME_S: f32 = 1.0 / 144.0;

/// Advances the adapter's motion until nothing is in flight and reports how many frames it
/// took.
///
/// The ceiling is what makes a motion that never settles fail the test rather than hang it.
fn settle(adapter: &mut Adapter) -> u32 {
    let mut frames = 1u32;
    while adapter.advance(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the window's motion must come to rest");
    }
    frames
}

/// The horizontal distance between two candidate cells, in logical pixels.
fn cell_step_x() -> f32 {
    cell_width() + metrics().grid_gap
}

/// The vertical distance between two rows of candidates, in logical pixels.
fn cell_step_y() -> f32 {
    metrics().cell_height + metrics().grid_gap
}

/// The highlight box's four components as the component holds them.
fn highlight_box(adapter: &Adapter) -> (f32, f32, f32, f32) {
    let window = adapter.window();
    (
        window.get_highlight_x(),
        window.get_highlight_y(),
        window.get_highlight_w(),
        window.get_highlight_h(),
    )
}

#[test]
fn test_adapter_appear_motion_settles_on_the_end_state_and_stops() {
    let (start, end, frames, idle) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 4)));
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        // The motion reaches the component on the first frame the loop advances, which is the
        // frame the window appears on.
        adapter.advance(0.0);
        let window = adapter.window();
        let start = (window.get_window_scale(), window.get_window_opacity());
        let frames = settle(adapter);
        let idle = !adapter.advance(FRAME_S) && !adapter.advance(FRAME_S);
        let window = adapter.window();
        let end = (window.get_window_scale(), window.get_window_opacity());
        (start, end, frames, idle)
    });
    assert_eq!(start.0, 0.96, "the panel starts at 3.3.2's scale floor");
    assert_eq!(start.1, 0.0, "and fully transparent");
    assert_eq!(end, (1.0, 1.0), "and reaches full size and full opacity");
    // 110ms at 144Hz is 15.84 frames.
    assert!(
        (14..=18).contains(&frames),
        "the appear motion took {frames} frames, off the 110ms 3.3.2 allots it"
    );
    assert!(
        idle,
        "and a window at rest reports no deadline, which is what lets the loop block"
    );
}

#[test]
fn test_adapter_highlight_box_lands_on_the_highlighted_cell() {
    let (first, second) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let first = highlight_box(adapter);
        let visible = adapter.window().get_highlight_visible();
        // The seventh candidate sits on the second row, second column of a five-per-row page.
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(6),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        settle(adapter);
        let second = highlight_box(adapter);
        (first, (second, visible))
    });
    assert_eq!(
        first,
        (0.0, 0.0, cell_width(), metrics().cell_height),
        "the box rests on the first cell, at the grid's own origin"
    );
    assert_eq!(
        second.0,
        (
            cell_step_x(),
            cell_step_y(),
            cell_width(),
            metrics().cell_height
        ),
        "and one cell right and one row down for the seventh candidate"
    );
    assert!(
        second.1,
        "the box is drawn while a candidate carries the highlight"
    );
}

#[test]
fn test_adapter_highlight_slides_between_cells_instead_of_jumping() {
    let (redirect, middle, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(4),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        adapter.advance(0.0);
        let redirect = highlight_box(adapter).0;
        adapter.advance(FRAME_S);
        let middle = highlight_box(adapter).0;
        settle(adapter);
        (redirect, middle, highlight_box(adapter).0)
    });
    assert_eq!(redirect, 0.0, "a redirect keeps the box where it is");
    assert!(
        middle > 0.0 && middle < 4.0 * cell_step_x(),
        "one frame moves the box part of the way, got {middle} of {}",
        4.0 * cell_step_x()
    );
    assert_eq!(
        end,
        4.0 * cell_step_x(),
        "and it comes to rest on the cell it was sent to"
    );
}

#[test]
fn test_adapter_motion_disabled_lands_on_the_end_values_at_once() {
    let (scale, opacity, offset, highlight_box, animating) = with_adapter(|adapter| {
        adapter.set_motion_enabled(false);
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_pointer(PointerState {
            highlighted: Some(3),
            hovered: None,
            pressed: None,
        }));
        let animating = adapter.advance(FRAME_S);
        let window = adapter.window();
        (
            window.get_window_scale(),
            window.get_window_opacity(),
            window.get_page_offset_dp(),
            highlight_box(adapter),
            animating,
        )
    });
    assert_eq!(scale, 1.0, "the appear motion is instantaneous");
    assert_eq!(opacity, 1.0);
    assert_eq!(offset, 0.0, "the page content is home");
    assert_eq!(
        highlight_box,
        (
            3.0 * cell_step_x(),
            0.0,
            cell_width(),
            metrics().cell_height
        ),
        "the box is already on the cell the highlight moved to"
    );
    assert!(!animating, "a disabled motion has nothing in flight");
}

#[test]
fn test_adapter_page_turn_slides_the_grid_and_returns_it_home() {
    let (before, displaced, home) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let before = adapter.window().get_page_offset_dp();
        let mut second = frame_with(2, "ni'hao", 9);
        second.page.current = 2;
        assert!(adapter.apply_frame(&second));
        adapter.advance(0.0);
        let displaced = adapter.window().get_page_offset_dp();
        settle(adapter);
        (before, displaced, adapter.window().get_page_offset_dp())
    });
    assert_eq!(before, 0.0, "the first page draws no displacement");
    assert_eq!(
        displaced, PAGE_SLIDE_DP,
        "the next page enters displaced from the side it came from"
    );
    assert_eq!(home, 0.0, "and the content springs home");
}

#[test]
fn test_adapter_scale_anchor_follows_the_placement() {
    let (below, above, automatic) = with_adapter(|adapter| {
        adapter.set_placement(Placement::Below);
        let below = adapter.window().get_grows_upward();
        adapter.set_placement(Placement::Above);
        let above = adapter.window().get_grows_upward();
        adapter.set_placement(Placement::Auto);
        (below, above, adapter.window().get_grows_upward())
    });
    assert!(!below, "a panel below the caret grows from its top edge");
    assert!(above, "and one above grows upward from its bottom edge");
    assert!(!automatic, "the automatic side resolves to the one below");
}

#[test]
fn test_adapter_highlight_is_hidden_for_a_page_with_no_candidate() {
    let (visible, highlight_box) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni", 0)));
        adapter.advance(0.0);
        (
            adapter.window().get_highlight_visible(),
            highlight_box(adapter),
        )
    });
    assert!(
        !visible,
        "nothing is highlighted when there is no candidate"
    );
    assert_eq!(
        highlight_box,
        (0.0, 0.0, 0.0, 0.0),
        "and the box collapses rather than drawing at a stale size"
    );
}

#[test]
fn test_adapter_highlight_hides_when_the_page_empties() {
    // The box hiding without moving is the case a frame-to-frame comparison of the motion
    // alone cannot see: the springs are already at rest on the cell, so the frame the step
    // produces is the one before it and the component would keep drawing the box.
    let (before, after) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let before = adapter.window().get_highlight_visible();
        assert!(adapter.apply_frame(&frame_with(2, "ni", 0)));
        adapter.advance(FRAME_S);
        (before, adapter.window().get_highlight_visible())
    });
    assert!(
        before,
        "the box is drawn while a candidate carries the highlight"
    );
    assert!(
        !after,
        "and it is gone once the page has no candidate left to highlight"
    );
}

#[test]
fn test_adapter_highlight_approaches_its_target_monotonically() {
    let (start, backwards, frames, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let start = adapter.window().get_highlight_x();
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(1),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        adapter.advance(0.0);
        let mut previous = adapter.window().get_highlight_x();
        let mut backwards = 0.0f32;
        let mut frames = 0u32;
        while adapter.advance(FRAME_S) {
            let x = adapter.window().get_highlight_x();
            backwards = backwards.max(previous - x);
            previous = x;
            frames += 1;
            assert!(frames < 1_000, "the highlight must come to rest");
        }
        (start, backwards, frames, adapter.window().get_highlight_x())
    });
    assert_eq!(start, 0.0, "the box starts on the first cell");
    assert!(
        frames >= 4,
        "the box was written over only {frames} frames, which is too few for the \
         convergence to mean anything"
    );
    // The approach is monotone up to the band the spring calls arrived: a movement larger
    // than that would be the box visibly passing the cell it was sent to and coming back.
    assert!(
        backwards <= REST_POSITION_DP,
        "the box moved {backwards}px back off its target, more than the {}px band the spring \
         settles within",
        REST_POSITION_DP
    );
    assert_eq!(
        end,
        cell_step_x(),
        "and it lands exactly on the cell it was sent to"
    );
}

#[test]
fn test_adapter_highlight_survives_twenty_rapid_retargets_without_jumping() {
    // Twenty presses four frames apart, which is 28ms at the target refresh rate: the box is
    // redirected long before it ever arrives, which is what a held arrow key produces.
    const FRAMES_PER_PRESS: u32 = 4;
    let (widest, end) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        let mut previous = adapter.window().get_highlight_x();
        let mut widest = 0.0f32;
        for press in 0..20u32 {
            // The user jitters the key between two neighbouring cells rather than walking in
            // one direction, so every redirect has to absorb a velocity it did not choose.
            let highlighted = if press % 2 == 0 { 1 } else { 0 };
            assert!(
                adapter.apply_pointer(PointerState {
                    highlighted: Some(highlighted),
                    hovered: None,
                    pressed: None,
                }),
                "moving the highlight redraws the grid"
            );
            for _ in 0..FRAMES_PER_PRESS {
                adapter.advance(FRAME_S);
                let x = adapter.window().get_highlight_x();
                widest = widest.max((x - previous).abs());
                previous = x;
            }
        }
        settle(adapter);
        (widest, adapter.window().get_highlight_x())
    });
    assert!(
        widest <= cell_step_x() * 0.2,
        "one frame moved the box {widest}px of the {}px the box had to cross, which is a jump \
         rather than a slide",
        cell_step_x()
    );
    // The twentieth press is odd, so the last target is the cell the box started on.
    assert_eq!(end, 0.0, "the box comes to rest on the last target");
}

#[test]
fn test_adapter_motion_disabled_mid_flight_writes_the_end_values_at_once() {
    let (in_flight, before, after, animating) = with_adapter(|adapter| {
        // The window is shown first, so the appear motion is running and the switch has
        // something to snap: an adapter nobody has shown sits at opacity zero, and
        // asserting the end state of a motion that never started proves nothing.
        adapter.set_visible(true).expect("the window shows");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 9)));
        settle(adapter);
        assert!(
            adapter.apply_pointer(PointerState {
                highlighted: Some(6),
                hovered: None,
                pressed: None,
            }),
            "moving the highlight redraws the grid"
        );
        let mut second = frame_with(2, "ni'hao", 9);
        second.page.current = 2;
        assert!(adapter.apply_frame(&second));
        let in_flight = adapter.advance(0.0);
        let before = (
            adapter.window().get_page_offset_dp(),
            highlight_box(adapter).0,
        );
        adapter.set_motion_enabled(false);
        let animating = adapter.advance(FRAME_S);
        let window = adapter.window();
        let after = (
            window.get_page_offset_dp(),
            window.get_window_opacity(),
            window.get_window_scale(),
            highlight_box(adapter),
        );
        (in_flight, before, after, animating)
    });
    assert!(
        in_flight,
        "the page slide and the highlight are both in flight when the switch goes off"
    );
    assert_eq!(
        before,
        (PAGE_SLIDE_DP, 0.0),
        "the content is displaced and the box has not left the first cell"
    );
    assert_eq!(after.0, 0.0, "the content is home");
    assert_eq!(after.1, 1.0, "the window is fully opaque");
    assert_eq!(after.2, 1.0, "and at its full size");
    assert_eq!(
        after.3,
        (
            cell_step_x(),
            cell_step_y(),
            cell_width(),
            metrics().cell_height
        ),
        "and the box is already on the cell the highlight moved to"
    );
    assert!(!animating, "a disabled motion has nothing left in flight");
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
        adapter.advance(0.0);
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

/// One cheat-sheet frame, as the engine builds it from the bindings in force.
fn overlay_frame() -> OverlayFrame {
    OverlayFrame {
        kind: OverlayKind::CheatSheet,
        title: String::from("按键速查"),
        sections: vec![OverlaySection {
            title: String::from("组字"),
            entries: vec![OverlayEntry {
                keys: String::from("tab"),
                label: String::from("移动候选高亮"),
            }],
        }],
        selected: None,
        query: String::new(),
    }
}

#[test]
fn test_adapter_apply_overlay_writes_the_overlay_properties() {
    // The properties are what the binding actually produced, so reading them back covers
    // the write path -- the component-side half of "the overlay draws" -- and not only
    // the mapping that feeds it.
    let (visible, title, sections, entries, hidden_after) = with_adapter(|adapter| {
        let frame = overlay_frame();
        adapter.apply_overlay(Some(&frame));
        let window = adapter.window();
        let open = (
            window.get_overlay_visible(),
            window.get_overlay_title().to_string(),
            window.get_overlay_sections().row_count(),
            window
                .get_overlay_sections()
                .row_data(0)
                .map(|section| section.entries.row_count()),
        );
        adapter.apply_overlay(None);
        let hidden_after = adapter.window().get_overlay_visible();
        (open.0, open.1, open.2, open.3, hidden_after)
    });
    assert!(visible, "the overlay mode is on while a frame is drawn");
    assert_eq!(title, "按键速查", "the title arrives from the frame");
    assert_eq!(sections, 1, "one model row per frame section");
    assert_eq!(entries, Some(1), "the group's rows travel with it");
    assert!(
        !hidden_after,
        "closing the overlay uncovers the candidate panel the adapter still holds"
    );
}
