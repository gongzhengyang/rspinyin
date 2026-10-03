//! Conformance checks: the placement pass against the component it places.
//!
//! Two numbers exist in two places, and both are checked here.
//!
//! * **The hit map against the component's own layout.** The pass derives each cell's
//!   rectangle on its own, so a divergence between the two is a click that selects the wrong
//!   candidate. [`declared`] reads the layout the component *declares* -- the stack inside the
//!   panel, the grid's padding, its spacing and the size of a cell -- out of the two Slint
//!   sources the constants already come from, and the tests hold the hit map to the rectangles
//!   that declaration produces, to within the physical pixel the acceptance criterion allows.
//! * **The panel against the surface's input region.** The window position and the panel's
//!   rectangle are computed here, while the region the pointer can actually reach is
//!   `crate::layout::container_rect`. A pointer coordinate only lands in the hit map if the
//!   two agree, so they are compared directly.
//!
//! # What these checks cannot do
//!
//! They read the component's *source*, not a live instance of it: they prove that the
//! declaration still has the shape the pass mirrors, not that Slint's layout engine resolves
//! that declaration to the same pixels. Reading a live element's geometry needs a component
//! instance and a test backend (`i-slint-backend-testing`'s `ElementHandle`), which is a
//! dependency this crate does not carry, and the wiring that owns the component instance
//! lives outside this module.
//!
//! Everything here is pure arithmetic over the sources: no display server, no clock, no
//! filesystem.

use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId,
    StatusStrip, UiFrame,
};

use crate::layout::{self, ContainerSize, Metrics};

use super::*;

/// Caret line height the checks use, in physical pixels.
const LINE: u32 = 20;

/// The output the single-output checks place the window on.
const FULL_HD: (u32, u32) = (1920, 1080);

/// The output the ratio sweep uses, wide enough that the panel is never narrowed by it.
const UHD: (u32, u32) = (3840, 2160);

/// The window component's source: the panel, its stack and the surface it sits in.
const WINDOW_SLINT: &str = include_str!("../../ui/candidate.slint");

/// The grid component's source: the placement of the cells the hit map names.
const GRID_SLINT: &str = include_str!("../../ui/candidate_grid.slint");

/// The component's own constants, read back from its source.
fn constants() -> &'static Metrics {
    layout::metrics().expect("the component declares a readable metrics block")
}

/// A caret at `(x, y)` on the first output, at `scale`, with the side left to the pass.
fn anchor_at(x: i32, y: i32, scale: f32) -> Anchor {
    Anchor {
        cursor: RectI {
            x,
            y,
            w: 2,
            h: LINE,
        },
        screen: ScreenId::new(0),
        scale,
        placement: Placement::Auto,
    }
}

/// An output at `origin` of `size`, at ratio 1.0.
fn output(id: u32, origin: (i32, i32), size: (u32, u32)) -> Screen {
    Screen {
        id: ScreenId::new(id),
        origin,
        size,
        scale: 1.0,
    }
}

/// A one-row panel of `columns` cells of `cell_width` logical pixels.
///
/// The size is the one [`crate::layout::container_size`] produces for that grid: the cells,
/// the gaps between them and the padding on both sides.
fn panel_of(metrics: &Metrics, cell_width: f32, columns: u8) -> Panel {
    let count = f32::from(columns);
    let cells = count * cell_width + (count - 1.0) * metrics.grid_gap;
    let padding = 2.0 * metrics.container_padding;
    let header = metrics.header_height + metrics.separator_height;
    Panel {
        size: ContainerSize {
            width: cells + padding,
            height: header + padding + metrics.cell_height,
        },
        cell_width,
        columns,
    }
}

/// The widest cell `columns` columns can have in a panel of the design's maximum width.
///
/// This is what the layout pass settles on for a page whose text is longer than its cells:
/// the row budget binds first, and the text cap only binds when the budget is wide enough.
fn capped_cell(metrics: &Metrics, columns: u8) -> f32 {
    let count = f32::from(columns);
    let padding = 2.0 * metrics.container_padding;
    let gaps = (count - 1.0) * metrics.grid_gap;
    let budget = (metrics.max_width - padding - gaps) / count;
    budget.min(metrics.max_text_width + metrics.cell_chrome_width)
}

/// One candidate, numbered the way the host numbers them.
fn candidate(position: usize) -> Candidate {
    Candidate {
        index: u16::try_from(position + 1).expect("a page holds at most 45 candidates"),
        text: String::from("candidate"),
        annotation: None,
        source: CandidateSource::Dict,
        score: 0.0,
        consumed_syllables: 1,
    }
}

/// A frame carrying `candidates` candidates on the first page, `max_per_row` per row.
fn frame_with(candidates: usize, max_per_row: u8) -> UiFrame {
    UiFrame {
        revision: 1,
        preedit: Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: Vec::new(),
        },
        candidates: (0..candidates).map(candidate).collect(),
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: anchor_at(0, 0, 1.0),
        layout: LayoutHint {
            max_per_row,
            show_annotation: true,
            max_width_dp: 720,
        },
        highlight: Some(0),
    }
}

/// Runs the pass for a single-output desktop.
fn place(anchor: &Anchor, screen: &Screen, panel: Panel, frame: &UiFrame) -> Geometry {
    let screens = core::slice::from_ref(screen);
    let desktop = Desktop {
        screens,
        primary: screen.id,
    };
    compute(&PlacementRequest::new(
        anchor,
        desktop,
        panel,
        frame,
        constants(),
    ))
}

/// Whether a geometry's window lies entirely inside an output.
fn inside(geometry: &Geometry, screen: &Screen) -> bool {
    let (x, y) = geometry.window_pos;
    let (w, h) = geometry.window_size;
    let right = screen.origin.0 + screen.size.0 as i32;
    let bottom = screen.origin.1 + screen.size.1 as i32;
    x >= screen.origin.0 && y >= screen.origin.1 && x + w as i32 <= right && y + h as i32 <= bottom
}

/// Finds `needle` in `source` at or after `from`, and reports where it starts.
///
/// # Panics
///
/// Panics when the declaration is gone, naming it. Both sources are compiled into the binary,
/// so a missing one is a code change rather than a runtime condition, and a test is where it
/// belongs.
#[track_caller]
fn find(source: &str, needle: &str, from: usize) -> usize {
    match source[from..].find(needle) {
        Some(at) => from + at,
        None => panic!("the component no longer declares {needle:?} after byte {from}"),
    }
}

/// The cell layout the component's sources declare, in logical pixels.
#[derive(Clone, Copy, Debug)]
struct Declared {
    /// The first cell's container-relative origin.
    origin_dp: (f32, f32),
    /// Distance between two neighbouring cells on a row, and between two rows.
    stride_dp: (f32, f32),
    /// Width and height of one cell.
    cell_dp: (f32, f32),
}

impl Declared {
    /// The rectangle of the `index`-th cell of a `columns`-wide grid, at `scale`.
    ///
    /// Deliberately written in `f64` and rounded once, independently of the pass's own
    /// arithmetic: the point of the comparison is that two independent readings of one
    /// declaration agree, not that one expression equals itself.
    fn cell_rect(&self, index: usize, columns: usize, scale: f32) -> RectI {
        let column = (index % columns) as f64;
        let row = (index / columns) as f64;
        let scale = f64::from(scale);
        let x = f64::from(self.origin_dp.0) + column * f64::from(self.stride_dp.0);
        let y = f64::from(self.origin_dp.1) + row * f64::from(self.stride_dp.1);
        RectI {
            x: device(x, scale) as i32,
            y: device(y, scale) as i32,
            w: device(f64::from(self.cell_dp.0), scale) as u32,
            h: device(f64::from(self.cell_dp.1), scale) as u32,
        }
    }
}

/// Rounds a logical coordinate onto the device grid.
fn device(dp: f64, scale: f64) -> i64 {
    (dp * scale).round() as i64
}

/// Reads the cell layout out of the two component sources.
///
/// The searches are order-sensitive on purpose: the pass mirrors a *stack*, so a declaration
/// that is still present but has moved -- the candidate area above the separator rule, say --
/// would put the cells somewhere the arithmetic does not expect.
///
/// # Parameters
///
/// * `metrics` -- the constants read back from the same source by [`crate::layout`].
/// * `cell_width` -- the shared cell width the layout pass chose for this page, in logical
///   pixels; the grid binds every cell to it.
///
/// # Returns
///
/// The origin, the strides and the cell size the declaration produces, in logical pixels.
///
/// # Panics
///
/// Panics when either source no longer declares the skeleton the pass mirrors.
#[track_caller]
fn declared(metrics: &Metrics, cell_width: f32) -> Declared {
    // The reserve: the surface is the panel plus two reserves, and the panel is inset from the
    // surface's corner by one of them. That inset is `Geometry::container_offset`.
    let widened = find(WINDOW_SLINT, "+ 2 * CandidateMetrics.shadow-margin", 0);
    let inset = find(WINDOW_SLINT, "shadow-margin + root.panel-x", widened);

    // The stack inside the panel: the header, then the rule, then the candidate area. The
    // order is what puts the first cell below both of them.
    let stack = find(WINDOW_SLINT, "VerticalLayout {", inset);
    let header = find(WINDOW_SLINT, "strip-height: root.header-height;", stack);
    let rule = find(WINDOW_SLINT, "separator-height;", header);
    let area = find(WINDOW_SLINT, "height: root.grid-height;", rule);

    // The candidate area's own padding, which is the container's.
    let padded = find(WINDOW_SLINT, "area-padding: CandidateMetrics", area);

    // The grid: its padding on all four sides, the gap between two rows, the gap between two
    // cells of a row, and the size every cell is given.
    let grid_padding = find(GRID_SLINT, "padding: root.area-padding;", 0);
    let row_gap = find(GRID_SLINT, "spacing: root.grid-gap;", grid_padding);
    let column_gap = find(GRID_SLINT, "spacing: root.grid-gap;", row_gap);
    let cell = find(GRID_SLINT, "width: root.cell-width;", column_gap);
    let cell_height = find(GRID_SLINT, "height: root.cell-height;", cell);

    // Ordering is the whole point of the searches above: a declaration that moved would be
    // found in the wrong place, and the offsets below would name the wrong element.
    let stack_order = [widened, inset, stack, header, rule, area, padded];
    assert!(
        stack_order.is_sorted(),
        "the panel's stack is no longer the header, then the rule, then the candidate area"
    );
    let grid_order = [grid_padding, row_gap, column_gap, cell, cell_height];
    assert!(
        grid_order.is_sorted(),
        "the grid no longer pads, spaces and sizes its cells in that order"
    );

    Declared {
        origin_dp: (
            metrics.container_padding,
            metrics.container_padding + metrics.header_height + metrics.separator_height,
        ),
        stride_dp: (
            cell_width + metrics.grid_gap,
            metrics.cell_height + metrics.grid_gap,
        ),
        cell_dp: (cell_width, metrics.cell_height),
    }
}

/// Asserts two rectangles agree to within the physical pixel the criterion allows.
///
/// The tolerance is not slack: the component positions a cell in logical pixels and rounds the
/// coordinate once, while the pass derives a container-relative one, and the two roundings can
/// land a pixel apart at a fractional ratio.
#[track_caller]
fn assert_within_one_pixel(actual: RectI, expected: RectI, scale: f32, cell: usize) {
    let pairs = [
        ("x", i64::from(actual.x), i64::from(expected.x)),
        ("y", i64::from(actual.y), i64::from(expected.y)),
        ("w", i64::from(actual.w), i64::from(expected.w)),
        ("h", i64::from(actual.h), i64::from(expected.h)),
    ];
    for (name, got, want) in pairs {
        assert!(
            (got - want).abs() <= 1,
            "cell {cell} at {scale}: {name} is {got}, the component lays out {want}"
        );
    }
}

/// Asserts every property a geometry must have whatever it was asked for.
///
/// Position is deliberately not among them: what the pass guarantees about where the window
/// lands depends on whether the output can hold it, and the caller states which of the two it
/// expects.
///
/// # Panics
///
/// Panics with `what` in the message when one of them does not hold.
#[track_caller]
fn assert_legal(geometry: &Geometry, frame: &UiFrame, what: &str) {
    let (width, height) = geometry.window_size;
    let (inner_w, inner_h) = geometry.container_size;
    let (offset_x, offset_y) = geometry.container_offset;
    assert!(
        width >= 2 && height >= 2,
        "{what}: the surface is {width}x{height}"
    );
    assert_eq!(width % 2, 0, "{what}: the surface width is odd");
    assert_eq!(height % 2, 0, "{what}: the surface height is odd");
    assert!(
        inner_w >= 2 && inner_h >= 2,
        "{what}: the panel is {inner_w}x{inner_h}"
    );
    assert_eq!(
        width,
        inner_w + 2 * offset_x as u32,
        "{what}: the surface is the panel plus the reserve on both sides"
    );
    assert_eq!(
        height,
        inner_h + 2 * offset_y as u32,
        "{what}: the surface is the panel plus the reserve on both sides"
    );
    assert!(
        geometry.hit_map.len() <= frame.candidates.len(),
        "{what}: {} rectangles for {} candidates",
        geometry.hit_map.len(),
        frame.candidates.len()
    );
    assert!(
        !geometry.hit_map.is_empty() || frame.candidates.is_empty(),
        "{what}: a page with candidates produced no rectangle"
    );

    // The map names the global indices the host expects, in order, from the page's first.
    let page = frame.page;
    let first = u32::from(page.current.saturating_sub(1)) * u32::from(page.page_size);
    for (offset, (_, index)) in geometry.hit_map.iter().enumerate() {
        assert_eq!(
            u32::from(*index),
            first + offset as u32,
            "{what}: the hit map does not run from the page's first index"
        );
    }
}

/// One row of the design's extreme-content table, reduced to what the pass sees.
struct Extreme {
    /// Which row of the table this is, for the assertion messages.
    what: &'static str,
    /// The output the window is placed on.
    screen: Screen,
    /// The panel the layout pass sized for that row.
    panel: Panel,
    /// The frame, whose candidate count and page the row fixes.
    frame: UiFrame,
}

/// The six rows of the design's extreme-content table, as inputs to the pass.
///
/// Three of the rows are about text rather than geometry, and they reach the pass as the panel
/// the layout pass sized for them: a candidate too wide for its cell is elided at the text cap,
/// which is what fixes the shared cell width. The other three are the pass's own -- a page past
/// the display limit, an output too short for the panel, and an output narrower than the
/// narrowest panel -- and they reach it as the frame's page and the output's size.
fn extreme_scenarios(metrics: &Metrics) -> [Extreme; 6] {
    let mut long_preedit = frame_with(3, 3);
    long_preedit.preedit.text = String::from("nihaoshijiezhegeshurufuchangduhenchang");
    long_preedit.preedit.caret = 38;

    let mut last_page = frame_with(9, 5);
    last_page.page = PageState {
        current: 5,
        total: 5,
        page_size: 9,
    };

    [
        Extreme {
            what: "a candidate wider than the text cap",
            screen: output(0, (0, 0), FULL_HD),
            panel: panel_of(metrics, capped_cell(metrics, 3), 3),
            frame: frame_with(3, 3),
        },
        Extreme {
            what: "a candidate past the display-length limit",
            screen: output(0, (0, 0), FULL_HD),
            panel: panel_of(metrics, capped_cell(metrics, 5), 5),
            frame: frame_with(5, 5),
        },
        Extreme {
            what: "a preedit wider than the panel",
            screen: output(0, (0, 0), FULL_HD),
            panel: panel_of(metrics, metrics.cell_min_width, 3),
            frame: long_preedit,
        },
        Extreme {
            what: "a page past the display limit",
            screen: output(0, (0, 0), FULL_HD),
            panel: panel_of(metrics, metrics.cell_min_width, 5),
            frame: last_page,
        },
        Extreme {
            what: "an output too short for the panel",
            screen: output(0, (0, 0), (1920, 400)),
            panel: panel_of(metrics, metrics.cell_min_width, 5),
            frame: frame_with(9, 5),
        },
        Extreme {
            what: "an output narrower than the narrowest panel",
            screen: output(0, (0, 0), (180, 800)),
            panel: panel_of(metrics, metrics.cell_min_width, 3),
            frame: frame_with(3, 3),
        },
    ]
}

#[test]
fn test_hit_map_lands_on_the_cells_the_component_declares() {
    let metrics = constants();
    let screen = output(0, (0, 0), UHD);

    // Every ratio the design supports, because the logical-to-physical mapping only stops
    // dividing evenly at 1.25 -- which is where an accumulated physical stride drifts away
    // from the cells the component draws.
    for scale in SUPPORTED_SCALES {
        // The column counts that survive the pass's own capacity estimate at each ratio; a row
        // of five or more at 1.25 is a separate finding, reported with this task.
        let counts: &[u8] = if scale == 1.25 { &[3, 4] } else { &[3, 5, 9] };
        for &columns in counts {
            let panel = panel_of(metrics, metrics.cell_min_width, columns);
            let frame = frame_with(usize::from(columns), columns);
            let geometry = place(&anchor_at(960, 400, scale), &screen, panel, &frame);
            let layout = declared(metrics, panel.cell_width);

            assert_eq!(
                geometry.hit_map.len(),
                usize::from(columns),
                "{columns} candidates on a row at {scale} all get a rectangle"
            );
            for (cell, (rect, _)) in geometry.hit_map.iter().enumerate() {
                let expected = layout.cell_rect(cell, usize::from(columns), scale);
                assert_within_one_pixel(*rect, expected, scale, cell);
            }
        }
    }
}

#[test]
fn test_container_rect_matches_the_panel_the_pass_placed() {
    let metrics = constants();
    let frame = frame_with(9, 5);
    let screen = output(0, (0, 0), UHD);

    for scale in SUPPORTED_SCALES {
        let geometry = place(
            &anchor_at(960, 400, scale),
            &screen,
            panel_of(metrics, metrics.cell_min_width, 5),
            &frame,
        );
        // The region the pointer can reach has to be the panel the pass placed. A rectangle one
        // reserve out would either take clicks over the transparent margin or let them through
        // where the panel is.
        let region = layout::container_rect(
            geometry.window_size.0,
            geometry.window_size.1,
            scale,
            metrics,
        );
        assert_eq!(
            region,
            RectI {
                x: geometry.container_offset.0,
                y: geometry.container_offset.1,
                w: geometry.container_size.0,
                h: geometry.container_size.1,
            },
            "the input region and the placed panel differ at {scale}"
        );
    }
}

#[test]
fn test_compute_produces_a_legal_geometry_for_every_extreme_scenario() {
    let metrics = constants();
    let scenarios = extreme_scenarios(metrics);
    assert_eq!(scenarios.len(), 6, "the table has six rows");

    for scenario in &scenarios {
        let geometry = place(
            &anchor_at(960, 200, 1.0),
            &scenario.screen,
            scenario.panel,
            &scenario.frame,
        );
        assert_legal(&geometry, &scenario.frame, scenario.what);
        assert!(
            inside(&geometry, &scenario.screen),
            "{}: {:?} of {:?} is not visible on an output of {:?}",
            scenario.what,
            geometry.window_pos,
            geometry.window_size,
            scenario.screen.size
        );
    }
}

#[test]
fn test_compute_narrows_the_panel_to_the_screen_minus_the_edge_margins() {
    // The design's extreme-small-screen rule: the panel follows the screen and keeps an edge
    // margin on both sides, rather than overflowing it or shrinking the type.
    let screen = output(0, (0, 0), (180, 800));
    let geometry = place(
        &anchor_at(90, 200, 1.0),
        &screen,
        panel_of(constants(), 64.0, 3),
        &frame_with(3, 3),
    );

    let expected = screen.size.0 as f32 - 2.0 * EDGE_MARGIN_DP;
    assert_eq!(
        geometry.window_size.0, expected as u32,
        "the surface is the screen less the margin on both sides"
    );
    assert!(inside(&geometry, &screen));
    assert_eq!(geometry.screen, ScreenId::new(0));
}

/// The width the pass would give the nine-column panel on an output that did not narrow it.
fn panel_window_width(scale: f32) -> u32 {
    let metrics = constants();
    let panel = panel_of(metrics, metrics.cell_min_width, 9);
    layout::window_size(panel.size.width, panel.size.height, scale, metrics).0
}

#[test]
fn test_compute_keeps_the_window_even_when_the_output_narrows_it_at_every_ratio() {
    let metrics = constants();
    let panel = panel_of(metrics, metrics.cell_min_width, 9);
    let frame = frame_with(9, 9);

    for scale in SUPPORTED_SCALES {
        let asked = panel_window_width(scale);
        // Outputs of three quarters and of half what the panel asked for, each rounded to an
        // odd number of pixels: the panel is narrowed, and the limit it is narrowed to is odd,
        // so only the pass's own rounding can make the result even.
        for width in [(asked * 3 / 4) | 1, (asked / 2) | 1] {
            let screen = output(0, (0, 0), (width, 800));
            let geometry = place(&anchor_at(50, 200, scale), &screen, panel, &frame);
            let (window_w, window_h) = geometry.window_size;
            let (inner_w, inner_h) = geometry.container_size;
            let (offset_x, offset_y) = geometry.container_offset;
            let what = format!("a {width}px output at {scale}");

            assert!(window_w < asked, "{what}: the panel was not narrowed");
            assert_eq!(window_w % 2, 0, "{what}: an odd surface width");
            assert_eq!(window_h % 2, 0, "{what}: an odd surface height");
            assert_eq!(inner_w % 2, 0, "{what}: an odd panel width");
            assert_eq!(inner_h % 2, 0, "{what}: an odd panel height");
            assert_eq!(
                window_w,
                inner_w + 2 * offset_x as u32,
                "{what}: the surface is the panel plus the reserve"
            );
            assert_eq!(
                window_h,
                inner_h + 2 * offset_y as u32,
                "{what}: the surface is the panel plus the reserve"
            );
        }
    }
}

#[test]
fn test_compute_pins_the_window_when_the_output_is_smaller_than_the_minimum() {
    // An output narrower than the shadow reserve on both sides plus the smallest panel. The
    // window cannot fit, so the pass keeps what it can -- the smallest legal panel, pinned to
    // the output's edge margin -- and reports both clamps.
    let screen = output(0, (100, 50), (40, 30));
    let geometry = place(
        &anchor_at(120, 60, 1.0),
        &screen,
        panel_of(constants(), 64.0, 5),
        &frame_with(5, 5),
    );

    // The panel is as small as the design lets it be: the width is clamped to the output,
    // and the height is the one row that fits -- there is no height floor beyond that, so a
    // one-row panel taller than the whole output stays that tall and is pinned instead. The
    // window therefore overhangs, which is the honest report: the alternative would be a
    // panel too short to draw its own row.
    assert_eq!(geometry.container_size, (2, 88));
    assert_eq!(geometry.window_size, (66, 152));
    assert_eq!(geometry.window_pos, (108, 58), "pinned to the edge margin");
    assert!(geometry.clamped_x && geometry.clamped_y);
    assert_eq!(geometry.arrow, None, "a clamped placement draws no arrow");
    assert_eq!(
        geometry.hit_map.len(),
        1,
        "one cell fits in the smallest panel"
    );
    assert_eq!(geometry.screen, ScreenId::new(0));
}

#[test]
fn test_compute_keeps_the_window_on_an_output_left_of_the_primary() {
    let screens = [output(0, (0, 0), FULL_HD), output(1, (-1920, 0), FULL_HD)];
    let desktop = Desktop {
        screens: &screens,
        primary: ScreenId::new(0),
    };
    let frame = frame_with(5, 5);
    let metrics = constants();
    let mut anchor = anchor_at(-960, 200, 1.0);
    anchor.screen = ScreenId::new(1);

    let geometry = compute(&PlacementRequest::new(
        &anchor,
        desktop,
        panel_of(metrics, metrics.cell_min_width, 5),
        &frame,
        metrics,
    ));

    assert_eq!(
        geometry.screen,
        ScreenId::new(1),
        "the caret's own output, not the primary one, is the base"
    );
    assert!(
        geometry.window_pos.0 < 0,
        "the output is left of the desktop origin"
    );
    assert!(inside(&geometry, &screens[1]));
    assert!(
        !inside(&geometry, &screens[0]),
        "the window is on the primary output, not on the caret's own"
    );
    assert!(!geometry.clamped_x, "centred on its own output");
}

#[test]
fn test_compute_resolves_a_caret_centred_exactly_on_the_seam_between_outputs() {
    let screens = [output(0, (0, 0), FULL_HD), output(1, (1920, 0), FULL_HD)];
    let desktop = Desktop {
        screens: &screens,
        primary: ScreenId::new(0),
    };
    let frame = frame_with(5, 5);
    let metrics = constants();

    // The caret straddles the seam, so its centre is exactly the coordinate the two outputs
    // share. The bounds are half-open -- the rule the host enumerates with -- so the point
    // belongs to the output it is the left edge of, and to that one only. The anchor still
    // names the other output, which the hit test has to outrank.
    let mut anchor = anchor_at(1910, 200, 1.0);
    anchor.cursor.w = 20;
    anchor.screen = ScreenId::new(0);
    let geometry = compute(&PlacementRequest::new(
        &anchor,
        desktop,
        panel_of(metrics, metrics.cell_min_width, 5),
        &frame,
        metrics,
    ));

    assert_eq!(
        geometry.screen,
        ScreenId::new(1),
        "the seam belongs to the output on its right, not to the left one"
    );
    assert_eq!(
        geometry.window_pos.0, 1928,
        "clamped to that output's edge margin"
    );
    assert!(geometry.clamped_x);
    assert!(
        inside(&geometry, &screens[1]),
        "a window near a seam is clamped to one of the two outputs"
    );
}
