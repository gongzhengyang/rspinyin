//! Candidate-window placement: the screen-avoidance pass and the hit map.
//!
//! [`compute`] answers two questions in one pass of integer arithmetic: where the
//! candidate window goes on the virtual desktop, and where each candidate cell sits
//! inside it. It is the only implementation of [`Placement::Auto`], and it is the
//! producer of the hit map the interaction state consumes.
//!
//! # What it is given, and what it decides
//!
//! The panel is already sized when the pass runs: [`crate::layout`] turns the candidate
//! list into a panel size, a shared cell width and a column count, from the constants the
//! component draws with. The pass takes that panel as it is -- it never re-derives a cell
//! width or a column count, because the hit map has to land on the cells the component
//! actually drew -- and decides only where that panel goes, and what has to give when the
//! output is too small for it.
//!
//! # Coordinate spaces
//!
//! Three spaces appear below, and every rectangle says which one it is in.
//!
//! | Space | Origin | Appears in |
//! |---|---|---|
//! | virtual desktop | the primary output's top-left corner | [`Geometry::window_pos`], [`Geometry::arrow`] |
//! | window | the window's top-left corner, shadow reserve included | [`Geometry::container_offset`] |
//! | container | the container's top-left corner, shadow reserve excluded | [`Geometry::hit_map`] |
//!
//! The container is inset from the window by [`Geometry::container_offset`], which is the
//! shadow reserve the component keeps on all four sides. A pointer coordinate arrives
//! relative to the window, so subtracting that offset -- the conversion the interaction
//! entry performs once, on the way in -- lands exactly in the space the hit map is
//! expressed in.
//!
//! # Units
//!
//! Every number produced here is a **physical pixel**, and the window position is absolute
//! on the virtual desktop, which is what the layer-shell tier needs before it subtracts
//! the output's origin. The panel size and the cell width come in as logical pixels,
//! because that is the space the layout pass works in; they go through the same dp-to-pixel
//! helper the platform layer uses, so every layer rounds alike.
//!
//! The hit map is the one thing accumulated *before* it is converted: the cell strides are
//! summed in logical pixels and rounded once, because that is the order the component's own
//! layout works in. Rounding each stride first would drift away from the cells it draws; see
//! `Grid::rect` for the arithmetic.
//!
//! # Determinism
//!
//! Every placement decision is an integer comparison, so the same request yields the same
//! geometry byte for byte. The only floating-point work is snapping the scale to a ratio
//! the design supports and the dp-to-pixel conversions that follow it; neither orders
//! anything, and neither is affected by the order the outputs were enumerated in.
//!
//! # Degenerate input
//!
//! A zero-sized panel, a caret outside every output, an empty output list, a scale of `0.0`
//! or `NaN`, a desktop with a single output and a candidate list taller than the screen all
//! produce a usable [`Geometry`]: the window keeps a non-zero even size, and it is pinned
//! to the base output whenever one is known. Nothing here allocates beyond the hit map,
//! nothing here can fail, and nothing here touches a display server.

mod placement;

#[cfg(test)]
mod conformance;

#[cfg(test)]
mod tests;

use ime_types::{Anchor, Placement, RectI, ScreenId, UiFrame};

use crate::layout::{self, ContainerSize, Metrics};
use crate::platform::normalize_scale;

use self::placement::{Bounds, Pass, Px, Window};

/// The device pixel ratios the design is drawn for, lowest first.
///
/// A ratio outside this set cannot come from a supported output, so one that arrives
/// anyway is moved to the nearest entry by [`snap_scale`].
pub const SUPPORTED_SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 3.0];

/// Clearance kept between the window and an output's edge, in logical pixels.
const EDGE_MARGIN_DP: f32 = 8.0;

/// Distance between the caret and the window, in logical pixels.
const CARET_GAP_DP: f32 = 6.0;

/// Smallest container this pass will place, in physical pixels.
///
/// Two rather than one: the window is the container plus twice the shadow reserve, so an
/// even container is what keeps the window even as well, and a zero-sized container would
/// make the window nothing but reserve.
const MIN_CONTAINER: i64 = 2;

/// One output of the virtual desktop, as the placement pass needs it.
///
/// This is the host's output description reduced to the fields the arithmetic reads, and
/// a caller builds it from that type field for field: the same id, the same origin and
/// size in physical pixels, the same device pixel ratio. Placement is driven by the ratio
/// in [`PlacementRequest::scale`], not by this one; the field is carried so a caller can
/// tell which output a geometry belongs to without a second lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    /// Output id, as the platform's enumeration reports it.
    pub id: ScreenId,
    /// Top-left corner in virtual-desktop physical pixels; negative on a desktop that
    /// extends left of or above the primary output.
    pub origin: (i32, i32),
    /// Size in physical pixels.
    pub size: (u32, u32),
    /// Device pixel ratio of this output.
    pub scale: f32,
}

impl Screen {
    /// Whether `point` lies inside this output.
    ///
    /// The bounds are half-open, which is the rule the host's enumeration uses: a point on
    /// the top or left edge is inside, one on the bottom or right edge is not, so two
    /// outputs that share an edge resolve to exactly one of them.
    pub fn contains(&self, point: (i32, i32)) -> bool {
        let bounds = Bounds::of(self);
        let x = i64::from(point.0);
        let y = i64::from(point.1);
        (bounds.left..bounds.right).contains(&x) && (bounds.top..bounds.bottom).contains(&y)
    }
}

/// The outputs of the virtual desktop, plus the one the platform calls primary.
#[derive(Clone, Copy, Debug)]
pub struct Desktop<'a> {
    /// The outputs, in enumeration order; that order breaks a tie when two overlap.
    pub screens: &'a [Screen],
    /// The output the platform calls primary.
    pub primary: ScreenId,
}

/// The panel the layout pass sized, in logical pixels.
///
/// The three numbers come straight from [`crate::layout`]: the size from
/// [`crate::layout::container_size`], the cell width from [`crate::layout::cell_width`]
/// and the column count from [`crate::layout::grid`]. Passing them on rather than
/// recomputing them is what keeps the hit map on the cells the component drew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    /// Panel size, shadow reserve excluded.
    pub size: ContainerSize,
    /// Shared width of one candidate cell; every cell in a row uses it.
    pub cell_width: f32,
    /// Candidates on a full row; at least one.
    pub columns: u8,
}

/// A device pixel ratio after snapping to [`SUPPORTED_SCALES`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleSnap {
    /// The ratio to rasterise with: the nearest supported one.
    pub value: f32,
    /// Whether the input was already exactly one of the supported ratios.
    pub is_exact: bool,
}

/// Snaps a device pixel ratio to the nearest one the design supports.
///
/// A ratio that cannot be used at all -- zero, negative, `NaN` or infinite -- is replaced
/// by `1.0` first, through the same helper the platform layer applies to a ratio that came
/// from configuration. A usable ratio between two supported ones moves to the nearer of
/// the pair, a tie going to the lower one so the choice does not depend on comparison
/// order. `is_exact` is what a caller records when it wants to report the substitution.
///
/// # Examples
///
/// ```
/// use ime_ui::geometry::snap_scale;
///
/// assert!(snap_scale(1.5).is_exact);
/// assert_eq!(snap_scale(1.2).value, 1.25);
/// assert_eq!(snap_scale(f32::NAN).value, 1.0);
/// assert!(!snap_scale(0.0).is_exact);
/// ```
pub fn snap_scale(scale: f32) -> ScaleSnap {
    let usable = normalize_scale(scale);
    let mut best = SUPPORTED_SCALES[0];
    for candidate in SUPPORTED_SCALES {
        if (candidate - usable).abs() < (best - usable).abs() {
            best = candidate;
        }
    }
    ScaleSnap {
        value: best,
        is_exact: usable == scale && best == usable,
    }
}

/// Everything one placement pass reads.
///
/// The bundle exists because the pass needs more inputs than a function signature is
/// allowed to carry, and because it gives the caller one place to see which ratio the pass
/// will actually use.
#[derive(Clone, Copy, Debug)]
pub struct PlacementRequest<'a> {
    /// Where the caret is, in virtual-desktop physical pixels.
    pub anchor: &'a Anchor,
    /// The outputs of the virtual desktop.
    pub desktop: Desktop<'a>,
    /// The panel the layout pass sized, in logical pixels.
    pub panel: Panel,
    /// The frame being placed: its candidate list bounds the hit map, and its page state
    /// fixes the first global index.
    pub frame: &'a UiFrame,
    /// The component's own constants, read back by [`crate::layout::metrics`]. They are
    /// what the hit map has to agree with.
    pub metrics: &'a Metrics,
    /// The ratio the window is rasterised with, normally `anchor.scale`.
    pub scale: f32,
}

impl<'a> PlacementRequest<'a> {
    /// A request that takes its ratio from the anchor, which is what every caller but a
    /// test wants.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_types::{
    ///     Anchor, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId, StatusStrip,
    ///     UiFrame,
    /// };
    /// use ime_ui::geometry::{compute, Desktop, Panel, PlacementRequest};
    /// use ime_ui::layout::{ContainerSize, metrics};
    ///
    /// let anchor = Anchor {
    ///     cursor: RectI { x: 0, y: 0, w: 2, h: 20 },
    ///     screen: ScreenId::new(0),
    ///     scale: 1.0,
    ///     placement: Placement::Auto,
    /// };
    /// let desktop = Desktop { screens: &[], primary: ScreenId::new(0) };
    /// let panel = Panel {
    ///     size: ContainerSize { width: 360.0, height: 87.0 },
    ///     cell_width: 64.0,
    ///     columns: 5,
    /// };
    /// let frame = UiFrame {
    ///     revision: 1,
    ///     preedit: Preedit { text: String::new(), caret: 0, spans: Vec::new() },
    ///     candidates: Vec::new(),
    ///     page: PageState { current: 1, total: 1, page_size: 9 },
    ///     status: StatusStrip::default(),
    ///     anchor,
    ///     layout: LayoutHint { max_per_row: 5, show_annotation: false, max_width_dp: 720 },
    /// };
    /// let metrics = metrics().expect("the component declares its constants");
    /// let request = PlacementRequest::new(&anchor, desktop, panel, &frame, metrics);
    /// let geometry = compute(&request);
    /// assert_eq!(geometry.placement, Placement::Below);
    /// assert_eq!(geometry.window_size, (424, 152));
    /// ```
    pub fn new(
        anchor: &'a Anchor,
        desktop: Desktop<'a>,
        panel: Panel,
        frame: &'a UiFrame,
        metrics: &'a Metrics,
    ) -> Self {
        Self {
            anchor,
            desktop,
            panel,
            frame,
            metrics,
            scale: anchor.scale,
        }
    }
}

/// Where the window goes and where the candidate cells are inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Window position in virtual-desktop physical pixels, shadow reserve included.
    pub window_pos: (i32, i32),
    /// Window size in physical pixels; both components are even.
    ///
    /// This is the size the surface is created with, and it can be a pixel larger than
    /// [`crate::layout::window_size`] reports for the same panel: the even rule below is
    /// applied here, and the surplus pixel lands in the transparent reserve.
    pub window_size: (u32, u32),
    /// The container's offset inside the window: the shadow reserve on each side.
    pub container_offset: (i32, i32),
    /// The container size in physical pixels, after any narrowing the output forced.
    pub container_size: (u32, u32),
    /// The side the window ended up on.
    pub placement: Placement,
    /// Whether the window had to be moved horizontally to stay on the output.
    pub clamped_x: bool,
    /// Whether the window had to be moved vertically to stay on the output.
    pub clamped_y: bool,
    /// One entry per visible candidate, ordered like `UiFrame::candidates`.
    ///
    /// Each rectangle is in **container-relative** physical pixels, and its index is the
    /// global candidate index the interaction layer reports -- one that runs across pages
    /// rather than restarting on each page. Add [`Geometry::container_offset`] to a
    /// rectangle to get it relative to the window instead.
    pub hit_map: Vec<(RectI, u16)>,
    /// The caret indicator arrow in virtual-desktop physical pixels, or `None` when the
    /// design says not to draw it.
    pub arrow: Option<RectI>,
    /// The ratio every physical number above was computed with.
    pub scale: ScaleSnap,
    /// The output the window was placed on.
    pub screen: ScreenId,
}

/// Places the candidate window and lays out its hit map.
///
/// The pass runs in the order the design fixes: the base output is chosen by hit testing
/// the caret's centre and falling back to the anchor's output and then to the primary one;
/// the window is narrowed to the output when it cannot fit; the row count comes down until
/// one side of the caret has room; the side is chosen -- flipped above the caret when the
/// space below it is short, unless the caller pinned a side; and the position is finally
/// clamped to the output's edges. The caret arrow and the hit map are derived from the
/// result.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`, because every input it cannot make
/// sense of has a documented degradation rather than a failure.
///
/// # Panics
///
/// Never panics. Every coordinate is widened to `i64` before arithmetic that could
/// overflow, every division is by a value the pixel conversion clamps to at least one, and
/// every cast back to `i32` or `u32` saturates.
pub fn compute(request: &PlacementRequest<'_>) -> Geometry {
    let scale = snap_scale(request.scale);
    let px = Px::for_scale(scale.value, request.panel.cell_width, request.metrics);
    let screen = base_screen(request);
    let bounds = screen.map(Bounds::of);
    let pass = Pass::of(px, request.anchor, bounds);

    let (window_w, window_h) = layout::window_size(
        request.panel.size.width,
        request.panel.size.height,
        scale.value,
        request.metrics,
    );
    let container_w = narrow(&px, bounds, in_container(even_up(i64::from(window_w)), &px));
    let container_h = in_container(even_up(i64::from(window_h)), &px);

    let columns = px
        .columns_in(container_w)
        .min(i64::from(request.panel.columns.max(1)));
    let (placement, rows, container_h) = pass.vertical(px.rows_in(container_h), container_h);

    let window_w = container_w + 2 * px.shadow;
    let window_h = container_h + 2 * px.shadow;
    let (x, clamped_x) = pass.horizontal(window_w);
    let (y, clamped_y) = pass.vertical_pos(placement, window_h);
    let window = Window::of(x, y);

    let grid = Grid {
        // Logical pixels rather than the physical forms above, so the cell strides are
        // accumulated in the space the component lays out in; see `Grid::rect`.
        x_dp: request.metrics.container_padding,
        y_dp: top_block_dp(request.metrics),
        columns,
        cell_dp: (request.panel.cell_width, request.metrics.cell_height),
        gap_dp: request.metrics.grid_gap,
        scale: scale.value,
    };
    let capacity = rows.saturating_mul(columns).max(0);
    let visible = usize::try_from(capacity)
        .unwrap_or(usize::MAX)
        .min(request.frame.candidates.len());

    Geometry {
        window_pos: (to_i32(x), to_i32(y)),
        window_size: (to_u32(window_w), to_u32(window_h)),
        container_offset: (to_i32(px.shadow), to_i32(px.shadow)),
        container_size: (to_u32(container_w), to_u32(container_h)),
        placement,
        clamped_x,
        clamped_y,
        hit_map: grid.hit_map(visible, first_index(request.frame)),
        arrow: pass.arrow(placement, window, container_w, clamped_x, clamped_y),
        scale,
        screen: screen.map_or(request.anchor.screen, |output| output.id),
    }
}

/// The container inside a window of this width, never smaller than [`MIN_CONTAINER`].
///
/// Deriving the container from the window rather than the other way round is what keeps
/// [`Geometry::window_size`] equal to the container plus the reserve on both sides.
fn in_container(window_w: i64, px: &Px) -> i64 {
    (window_w - 2 * px.shadow).max(MIN_CONTAINER)
}

/// The output the caret belongs to.
///
/// The caret's centre is hit tested against the outputs first, which is the rule the caret
/// ladder itself applies. A caret that lands in no output -- a gap between monitors, or a
/// layout that changed after the caret was resolved -- falls back to the output the anchor
/// names, and then to the primary one, so the window is always placed somewhere the user
/// is looking rather than nowhere at all.
fn base_screen<'a>(request: &'a PlacementRequest<'_>) -> Option<&'a Screen> {
    let screens = request.desktop.screens;
    let centre = centre_of(request.anchor.cursor);
    let hit = screens.iter().find(|screen| screen.contains(centre));
    let named = screens
        .iter()
        .find(|screen| screen.id == request.anchor.screen);
    let primary = screens
        .iter()
        .find(|screen| screen.id == request.desktop.primary)
        .or_else(|| screens.first());
    hit.or(named).or(primary)
}

/// The container width that fits the output, given the width the layout pass asked for.
///
/// A window wider than its output cannot be clamped into it, so the container is narrowed
/// first and the caller re-runs its grid at the new width. The shadow reserve is subtracted
/// from the narrowed window rather than from the output, which is what keeps the window's
/// own bounds -- shadow included -- inside the edge margin.
fn narrow(px: &Px, bounds: Option<Bounds>, container_w: i64) -> i64 {
    let Some(bounds) = bounds else {
        return container_w;
    };
    let limit = (bounds.width() - 2 * px.margin).max(2 * px.shadow + MIN_CONTAINER);
    if container_w + 2 * px.shadow <= limit {
        return container_w;
    }
    in_container(even_down(limit), px)
}

/// The global index of the first candidate on the page a frame carries.
///
/// The hit map pairs each cell with the index the interaction layer reports, and that index
/// runs across pages rather than restarting on each one, so the page the frame shows has to
/// be folded back in.
fn first_index(frame: &UiFrame) -> u32 {
    u32::from(frame.page.current.saturating_sub(1)) * u32::from(frame.page.page_size)
}

/// The centre of a rectangle, computed in `i64` so a very wide one cannot wrap.
fn centre_of(cursor: RectI) -> (i32, i32) {
    let x = i64::from(cursor.x) + i64::from(cursor.w) / 2;
    let y = i64::from(cursor.y) + i64::from(cursor.h) / 2;
    (to_i32(x), to_i32(y))
}

/// Rounds an extent up to the next even number, never below two.
///
/// Some compositors handle odd-sized ARGB buffers inconsistently. The window is the
/// container plus twice the shadow reserve -- always an even number -- so an even container
/// is what keeps the window even too, and the surplus pixel is absorbed by the container's
/// own padding rather than by the transparent reserve, where it would make the reserve
/// asymmetric.
fn even_up(value: i64) -> i64 {
    let value = value.max(MIN_CONTAINER);
    value + (value & 1)
}

/// Rounds an extent down to the previous even number, never below two.
fn even_down(value: i64) -> i64 {
    let value = value.max(MIN_CONTAINER);
    value - (value & 1)
}

/// Clamps a widened coordinate back into `i32`.
///
/// A plain cast truncates rather than saturates, so a coordinate computed in `i64` could
/// otherwise wrap into a plausible-looking position.
fn to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Clamps a widened extent into `u32`, saturating at zero.
fn to_u32(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

/// Converts a container-relative logical coordinate into physical pixels.
///
/// One rounding, on the accumulated coordinate, which is the order the component's own
/// layout works in: Slint places an element in logical pixels and maps that coordinate onto
/// the device grid afterwards. See [`Grid::rect`] for why the order matters.
///
/// A float-to-integer cast saturates in Rust, so a non-finite or absurd product lands on the
/// end of the range rather than wrapping.
fn to_px(dp: f32, scale: f32) -> i64 {
    (dp * scale).round() as i64
}

/// Height of the container block above the grid, in logical pixels.
///
/// These are the same three constants the placement pass sums in physical pixels for its row
/// capacity estimate, but summed before the conversion rather than after it. The hit map
/// needs this form because it is the one the component adds them in.
fn top_block_dp(metrics: &Metrics) -> f32 {
    metrics.container_padding + metrics.header_height + metrics.separator_height
}

/// Where the candidate cells sit inside the container.
///
/// Every field is a logical pixel except `scale`, and that is deliberate: the cells are laid
/// out in the space the component works in, and each coordinate is converted once, at the
/// end.
#[derive(Clone, Copy, Debug)]
struct Grid {
    /// Left edge of the grid area, from the container's left edge.
    x_dp: f32,
    /// Top edge of the grid area, from the container's top edge.
    y_dp: f32,
    /// Cells on a full row; at least one.
    columns: i64,
    /// Width and height of one cell.
    cell_dp: (f32, f32),
    /// Gap between two cells and between two rows.
    gap_dp: f32,
    /// The ratio the accumulated coordinates are converted with.
    scale: f32,
}

impl Grid {
    /// The rectangle of the `index`-th visible cell, container-relative physical pixels.
    ///
    /// The strides are accumulated in logical pixels and converted once, at the end, because
    /// that is the order the component's own layout works in: cell `k` sits at
    /// `padding + k * (cell + gap)` logical pixels, and the rasteriser maps that coordinate
    /// onto the device grid afterwards. Accumulating already-rounded physical strides instead
    /// drifts by half a pixel per column at a 1.25 ratio -- two pixels by the fifth column --
    /// and a hit rectangle that far from the cell the user sees is a click that lands on the
    /// neighbouring candidate or on nothing at all.
    fn rect(&self, index: i64) -> RectI {
        let column = index % self.columns;
        let row = index / self.columns;
        let x = self.x_dp + column as f32 * (self.cell_dp.0 + self.gap_dp);
        let y = self.y_dp + row as f32 * (self.cell_dp.1 + self.gap_dp);
        RectI {
            x: to_i32(to_px(x, self.scale)),
            y: to_i32(to_px(y, self.scale)),
            w: to_u32(to_px(self.cell_dp.0, self.scale)),
            h: to_u32(to_px(self.cell_dp.1, self.scale)),
        }
    }

    /// The hit map for `visible` cells, the first of them carrying global index `first`.
    fn hit_map(&self, visible: usize, first: u32) -> Vec<(RectI, u16)> {
        (0..visible)
            .map(|cell| {
                let offset = u32::try_from(cell).unwrap_or(u32::MAX);
                let index = first.saturating_add(offset);
                (
                    self.rect(cell as i64),
                    u16::try_from(index).unwrap_or(u16::MAX),
                )
            })
            .collect()
    }
}
