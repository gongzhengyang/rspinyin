//! One frame's worth of component state, and the mapping that produces it.
//!
//! This is the whole of `UiFrame -> .slint`: it turns the engine's snapshot into the values
//! the component draws from. The mapping is a pure function of the frame, the panel budget
//! and the component's constants -- it reads no clock, no display server and no global state
//! -- which is what lets every case below be covered by a test that never opens a window.
//!
//! # Steady state allocates nothing
//!
//! A frame arrives on every keystroke, so the state keeps its `String` buffers and reports
//! *which* of them changed rather than building a new value: a frame that draws the same
//! state as the one before it produces an empty [`DrawDelta`] and no property write at all,
//! and a frame whose text changed reuses the buffer already held. That holds for the
//! candidate cells as well: the vector is grown and truncated rather than rebuilt, so a
//! page that did not change keeps every string it draws.
//!
//! # Geometry
//!
//! The panel is sized by [`crate::layout`], from the same constants the component draws with,
//! so the rectangle the window occupies and the cells the pointer hits cannot drift apart.
//! The mapping decides no placement and owns no font metrics: the width a cell asks for is
//! estimated from its character count, which is a deterministic upper bound for the CJK text
//! a candidate holds. The estimate is cached across frames by [`Measure`], which the adapter
//! owns and hands in.
//!
//! # The candidate cells
//!
//! [`DrawState::cells`] is the grid's whole input: the label, the text that fits, the
//! annotation and the state of 3.4, one entry per candidate of the page. The state is
//! resolved from [`DrawState::pointer`], which `UiFrame` does not carry -- the engine's
//! paging state holds the highlight, not the frame -- so it is an adapter input rather than
//! a field of the snapshot.

use ime_types::{Candidate, StatusStrip, UiFrame};

use super::cell::{CellGeometry, CellState, Measure, PointerState, replace, write_text};
use super::preedit::PreeditLayout;
use crate::layout::{self, GridLayout, Metrics};

/// The window's drawable state, as the component's properties hold it.
///
/// A plain value, so the mapping can be tested without a Slint platform: a test builds one
/// from a frame and asserts on it, and only the final write into the component needs a live
/// instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawState {
    /// The preedit line: its runs, already cut on the left so the newest input stays visible,
    /// split around the caret and carrying the two facts about the cut the header draws with.
    pub preedit: PreeditLayout,
    /// The mode strip's label, in the language the scheme uses.
    pub mode_label: String,
    /// Candidates on the page being drawn.
    pub item_count: i32,
    /// Candidates on a full row, clamped into the range the component draws.
    pub max_per_row: i32,
    /// Rows the page occupies.
    pub grid_rows: i32,
    /// Width of one candidate cell, in logical pixels; every cell in a row shares it.
    pub cell_width: f32,
    /// Panel width in logical pixels, shadow reserve excluded.
    pub container_width: f32,
    /// Panel height in logical pixels, shadow reserve excluded.
    pub container_height: f32,
    /// Height of the header strip in logical pixels.
    ///
    /// Written because the panel's height is: a window with no candidate draws the compressed
    /// header, and leaving the component on its own default would draw a strip taller than
    /// the panel the layout sized.
    pub header_height: f32,
    /// Whether a candidate's annotation is drawn beside it, from `layout.show_annotation`.
    pub show_annotation: bool,
    /// Whether the status strip reports full-width input (3.1.1's second marker).
    pub full_width: bool,
    /// Whether the status strip reports Chinese punctuation (3.1.1's third marker).
    pub punctuation_full: bool,
    /// Whether the status strip reports read-only mode (3.6's lock).
    pub readonly: bool,
    /// The cells of the page, in the order the frame holds them.
    ///
    /// Empty for a frame with no candidate, which is what makes the grid take no height and
    /// leaves the window drawing its header alone.
    pub cells: Vec<CellState>,
    /// The pointer and highlight state the cells' five-state is resolved from.
    ///
    /// Part of the drawn state rather than of the frame: the engine's paging state holds the
    /// highlight, and `UiFrame` carries no hover or press at all.
    pub pointer: PointerState,
}

/// Which of the component's properties a frame changed.
///
/// The adapter writes only the properties this marks, which is what keeps a frame that draws
/// what is already on screen -- a replay, a redelivery, a keystroke that changed nothing
/// visible -- from spending a `SharedString` per candidate text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawDelta {
    /// What the preedit line draws changed.
    pub preedit: bool,
    /// The mode strip's label changed.
    pub mode_label: bool,
    /// The number of candidates on the page changed.
    pub item_count: bool,
    /// The number of candidates per row changed.
    pub max_per_row: bool,
    /// The number of rows changed.
    pub grid_rows: bool,
    /// The shared cell width changed.
    pub cell_width: bool,
    /// The panel width changed.
    pub container_width: bool,
    /// The panel height changed.
    pub container_height: bool,
    /// The header height changed.
    pub header_height: bool,
    /// Whether an annotation is drawn changed.
    pub show_annotation: bool,
    /// One of the status strip's three flags changed.
    pub status: bool,
    /// What a cell draws changed: a candidate, its text or its state.
    pub cells: bool,
}

impl DrawDelta {
    /// Whether the frame changed nothing the component draws.
    ///
    /// # Returns
    ///
    /// `true` when every field is `false`.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_empty(self) -> bool {
        !(self.preedit
            || self.mode_label
            || self.item_count
            || self.max_per_row
            || self.grid_rows
            || self.cell_width
            || self.container_width
            || self.container_height
            || self.header_height
            || self.show_annotation
            || self.status
            || self.cells)
    }
}

impl DrawState {
    /// Recomputes this state from `frame`, reusing the buffers it already holds.
    ///
    /// The convenience form of [`DrawState::update_cached`] for a caller that has no width
    /// cache to hand in: it measures every distinct text once per call, which is what a test
    /// and a one-off mapping want. The adapter keeps a [`Measure`] across frames and calls
    /// the cached form.
    ///
    /// # Parameters
    ///
    /// * `frame` -- the engine's snapshot of the window.
    /// * `max_container_width` -- widest panel the screen allows, in logical pixels.
    /// * `metrics` -- the component's constants, from [`crate::layout::metrics`]; passed in
    ///   rather than read here so that the mapping stays a pure function of its arguments.
    ///
    /// # Returns
    ///
    /// Which of the component's properties the frame changed. Every field is `false` when the
    /// frame draws exactly what the state already holds.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn update(
        &mut self,
        frame: &UiFrame,
        max_container_width: f32,
        metrics: &Metrics,
    ) -> DrawDelta {
        self.update_cached(frame, max_container_width, metrics, &mut Measure::default())
    }

    /// Recomputes this state from `frame`, measuring through `measure`.
    ///
    /// # Parameters
    ///
    /// * `frame` -- the engine's snapshot of the window.
    /// * `max_container_width` -- widest panel the screen allows, in logical pixels.
    /// * `metrics` -- the component's constants, from [`crate::layout::metrics`].
    /// * `measure` -- the width estimator, kept across frames so a candidate text is
    ///   measured once rather than once per keystroke.
    ///
    /// # Returns
    ///
    /// Which of the component's properties the frame changed. Every field is `false` when the
    /// frame draws exactly what the state already holds.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn update_cached(
        &mut self,
        frame: &UiFrame,
        max_container_width: f32,
        metrics: &Metrics,
        measure: &mut Measure,
    ) -> DrawDelta {
        let columns = per_row(frame.layout.max_per_row, metrics);
        let grid = layout::grid(
            frame.candidates.len(),
            usize::from(frame.page.total),
            columns,
            metrics,
        );
        let show_annotation = frame.layout.show_annotation;
        // The width budget protects one full row, so what owes it is the cells that row
        // actually holds: a page shorter than a row spends the room its empty slots leave.
        // Budgeting the configured row length instead would cut a lone candidate's text to a
        // fraction of the 3.1.3 limit it fits in, on account of a row it is not a member of.
        let on_row = u8::try_from(frame.candidates.len())
            .unwrap_or(u8::MAX)
            .min(columns)
            .max(1);
        let cell = layout::cell_width(
            &[widest_cell(frame, show_annotation, metrics, measure)],
            on_row,
            max_container_width,
            metrics,
        );
        let container = layout::container_size(&grid, cell.width, max_container_width, metrics);
        let geometry = CellGeometry {
            position: 0,
            width: cell.width,
            annotation_width: 0.0,
            metrics: *metrics,
        };
        let cells = self.write_cells(frame, geometry, show_annotation, measure);
        // The header's measurements -- the mode label here, the preedit's spans below -- go
        // through a scratch estimator rather than the caller's. Those strings are fragments
        // of the reading being typed and one fixed label, not the candidate texts that
        // repeat across keystrokes, and the caller's cache exists for the latter alone: its
        // miss count is the number the adapter's measurement budget is asserted with, and a
        // header string measured into it would spend a candidate's slot on a string that
        // never pays it back.
        let mut header_measure = Measure::default();
        // Laid out after the panel is sized, because the preedit's budget is what the panel's
        // width leaves once the strip's own chrome has taken its share of it.
        let available =
            preedit_budget(container.width, &frame.status, metrics, &mut header_measure);
        let preedit = self.preedit.update(
            &frame.preedit,
            available,
            metrics.font_size_header,
            &mut header_measure,
        );
        DrawDelta {
            preedit,
            mode_label: write_text(&mut self.mode_label, &frame.status.mode_label),
            item_count: replace(&mut self.item_count, count(frame.candidates.len())),
            max_per_row: replace(&mut self.max_per_row, i32::from(columns)),
            grid_rows: replace(&mut self.grid_rows, i32::from(grid.rows)),
            cell_width: replace(&mut self.cell_width, cell.width),
            container_width: replace(&mut self.container_width, container.width),
            container_height: replace(&mut self.container_height, container.height),
            header_height: replace(&mut self.header_height, header_height(&grid, metrics)),
            show_annotation: replace(&mut self.show_annotation, show_annotation),
            status: self.write_status(&frame.status),
            cells,
        }
    }

    /// Re-resolves the five-state of every cell from [`DrawState::pointer`].
    ///
    /// A pointer that moves, a press and a release change what the grid draws without any
    /// frame arriving, so this is callable on its own rather than only from
    /// [`DrawState::update_cached`].
    ///
    /// # Returns
    ///
    /// Whether any cell's state changed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn resolve_states(&mut self) -> bool {
        let pointer = self.pointer;
        let mut changed = false;
        for (position, cell) in self.cells.iter_mut().enumerate() {
            let Ok(position) = u16::try_from(position) else {
                break;
            };
            changed |= cell.resolve(position, pointer);
        }
        changed
    }

    /// Writes the status strip's three flags into this state.
    ///
    /// The strip carries a mode label, four booleans and a script. The header draws the label
    /// as text and three of the four booleans as markers -- the mode dot reads the label, so
    /// it needs no boolean of its own. `has_user_dict_hit` is the fourth and is deliberately
    /// not drawn: 3.1.1 gives the cluster four fixed slots and names them mode, full-width,
    /// punctuation and read-only, so the fifth has no slot to take. It travels in the frame
    /// and is drawn nowhere rather than inventing a marker the fixed-width contract has no
    /// room for.
    ///
    /// # Returns
    ///
    /// Whether any of the three changed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn write_status(&mut self, status: &StatusStrip) -> bool {
        let mut changed = replace(&mut self.full_width, status.full_width);
        changed |= replace(&mut self.punctuation_full, status.punctuation_full);
        changed |= replace(&mut self.readonly, status.readonly);
        changed
    }

    /// Rebuilds the cells of the page from `frame`, reusing the buffers already held.
    ///
    /// The vector is grown and truncated rather than rebuilt, so a frame that redraws the
    /// same page writes into the strings it already has and reports no change.
    ///
    /// # Returns
    ///
    /// Whether anything a cell draws changed.
    fn write_cells(
        &mut self,
        frame: &UiFrame,
        base: CellGeometry,
        show_annotation: bool,
        measure: &mut Measure,
    ) -> bool {
        let count = frame.candidates.len();
        let mut changed = false;
        if self.cells.len() < count {
            self.cells.resize_with(count, CellState::default);
            changed = true;
        } else if self.cells.len() > count {
            self.cells.truncate(count);
            changed = true;
        }
        for (position, candidate) in frame.candidates.iter().enumerate() {
            let Ok(position) = u16::try_from(position) else {
                break;
            };
            let Some(cell) = self.cells.get_mut(usize::from(position)) else {
                continue;
            };
            let mut geometry = base;
            geometry.position = position;
            geometry.annotation_width =
                annotation_width(candidate, show_annotation, &base.metrics, measure);
            changed |= cell.write(candidate, geometry, measure);
            changed |= cell.resolve(position, self.pointer);
        }
        changed
    }
}

/// Turns one frame into the state the component draws.
///
/// The convenience form of [`DrawState::update`] for a caller that has no state to reuse;
/// the adapter itself keeps one and updates it in place.
///
/// # Parameters
///
/// * `frame` -- the engine's snapshot of the window.
/// * `max_container_width` -- widest panel the screen allows, in logical pixels.
/// * `metrics` -- the component's constants, from [`crate::layout::metrics`].
///
/// # Returns
///
/// The state the component should be drawn with.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn draw_state(frame: &UiFrame, max_container_width: f32, metrics: &Metrics) -> DrawState {
    let mut state = DrawState::default();
    state.update(frame, max_container_width, metrics);
    state
}

/// The widest panel a frame allows, in logical pixels.
///
/// The true cap belongs to the screen: a panel wider than the output it appears on cannot be
/// drawn, and only the placement pass knows the outputs. What is left on this side is the
/// ceiling the configuration sets; a value that cannot describe a panel falls back to the
/// component's own maximum rather than collapsing the window to a pixel.
///
/// # Parameters
///
/// * `frame` -- the frame whose layout constraints carry the configured ceiling.
/// * `metrics` -- the component's constants, from [`crate::layout::metrics`].
///
/// # Returns
///
/// The widest panel in logical pixels, never zero.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn container_cap(frame: &UiFrame, metrics: &Metrics) -> f32 {
    let configured = f32::from(frame.layout.max_width_dp);
    if configured > 0.0 {
        configured
    } else {
        metrics.max_width
    }
}

/// The newest frame revision the window has drawn.
///
/// A frame is a full snapshot and the engine numbers them monotonically, so a frame older than
/// the one already drawn is a replay or an out-of-order delivery: drawing it would put the
/// window back on a state the user has left, and the selection that followed from the newer
/// frame would be rejected by the engine as stale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RevisionGate {
    current: Option<u32>,
}

impl RevisionGate {
    /// Whether `revision` is at least as new as the one already drawn, recording it if so.
    ///
    /// The first frame is always accepted. A revision equal to the current one is accepted as
    /// well: it carries the same state, and the writes it would produce are skipped by the
    /// change detection anyway.
    ///
    /// # Parameters
    ///
    /// * `revision` -- the frame's revision, from [`UiFrame::revision`].
    ///
    /// # Returns
    ///
    /// `true` when the frame may be drawn.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn accept(&mut self, revision: u32) -> bool {
        match self.current {
            Some(current) if revision < current => false,
            _ => {
                self.current = Some(revision);
                true
            }
        }
    }

    /// The revision the window is drawing.
    ///
    /// # Returns
    ///
    /// The newest accepted revision, or `None` before the first frame.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn current(&self) -> Option<u32> {
        self.current
    }
}

/// Candidates per row, clamped into the range the component draws and never zero.
///
/// The same rule [`crate::layout::grid`] applies internally, restated because the component
/// needs the value itself: it derives its last row from it. The two are pinned together by the
/// test below, which asserts the clamp agrees with the column count the layout reports for a
/// page that fills a row.
fn per_row(max_per_row: u8, metrics: &Metrics) -> u8 {
    max_per_row
        .max(metrics.min_per_row)
        .min(metrics.max_per_row_limit)
        .max(1)
}

/// The header height a page needs, in logical pixels.
///
/// A page with no candidate draws the compressed strip, which is the height
/// [`crate::layout::container_size`] already used for the panel.
fn header_height(grid: &GridLayout, metrics: &Metrics) -> f32 {
    if grid.rows == 0 {
        metrics.header_height_compact
    } else {
        metrics.header_height
    }
}

/// The widest cell the page needs, in logical pixels.
///
/// [`crate::layout::cell_width`] takes the maximum of the widths it is given, so the page's
/// widest candidate is the whole of what it reads: passing the maximum alone is the same
/// answer without a buffer per candidate.
fn widest_cell(
    frame: &UiFrame,
    show_annotation: bool,
    metrics: &Metrics,
    measure: &mut Measure,
) -> f32 {
    frame
        .candidates
        .iter()
        .map(|candidate| natural_cell_width(candidate, show_annotation, metrics, measure))
        .fold(0.0, f32::max)
}

/// The width one cell's content needs, chrome included, in logical pixels.
///
/// An estimate rather than a font measurement, which this layer owns no metrics for: the
/// window draws CJK candidates, where one glyph fills its em box, and Latin text at roughly
/// half of one. Rounding a Latin character up to a full em leaves a cell wider than it needs
/// rather than cutting it off, and the component's elide stays the last-resort guard it is
/// meant to be.
fn natural_cell_width(
    candidate: &Candidate,
    show_annotation: bool,
    metrics: &Metrics,
    measure: &mut Measure,
) -> f32 {
    let text = measure.width(&candidate.text, metrics.font_size_cell);
    let annotation = annotation_width(candidate, show_annotation, metrics, measure);
    text + annotation + metrics.cell_chrome_width
}

/// What a candidate's annotation costs beside its text, in logical pixels.
///
/// Zero when there is nothing to draw, when the layout turned the annotation off, or when
/// this cell cannot afford it -- which is what keeps a cell without one from reserving room
/// for it. The gap before the annotation is part of the cost, so the caller can subtract the
/// whole value from the text's budget.
///
/// 3.1.3 caps the text at `max_text_width`, and an annotation is dropped whole rather than
/// allowed to squeeze the text: a candidate whose text has been elided to make room for a
/// reading hint is worse than one with no hint at all. The test is what the layout
/// arithmetic makes it -- a cell is never wider than `max_text_width + cell_chrome_width`,
/// so text, gap and hint have to fit inside `max_text_width` together, and a cell that keeps
/// its hint is then never one the cap has to cut. The decision is made here, once, so the
/// component never has to branch on a measurement.
fn annotation_width(
    candidate: &Candidate,
    show_annotation: bool,
    metrics: &Metrics,
    measure: &mut Measure,
) -> f32 {
    if !show_annotation {
        return 0.0;
    }
    let Some(annotation) = candidate.annotation.as_deref() else {
        return 0.0;
    };
    if annotation.is_empty() {
        return 0.0;
    }
    let text = measure.width(&candidate.text, metrics.font_size_cell);
    let reading = measure.width(annotation, metrics.font_size_small);
    if text + metrics.annotation_gap + reading > metrics.max_text_width {
        return 0.0;
    }
    metrics.annotation_gap + reading
}

/// The width the preedit has in the header, in logical pixels.
///
/// Everything the strip draws beside the preedit comes off the panel's width: the two
/// insets, the gap either side of the status text, the text's own estimated width and the
/// status cluster's fixed width. The cluster's width is the *full* one, because this is
/// asked before the preedit's layout decides whether the two secondary markers are worth
/// their room -- and that layout adds the room back when it drops them, which is what makes
/// the two halves one decision rather than two.
fn preedit_budget(
    container_width: f32,
    status: &StatusStrip,
    metrics: &Metrics,
    measure: &mut Measure,
) -> f32 {
    let chrome = metrics.container_padding
        + metrics.header_padding_h
        + 2.0 * metrics.header_text_gap
        + measure.width(&status.mode_label, metrics.font_size_small)
        + status_cluster_width(metrics);
    (container_width - chrome).max(0.0)
}

/// The width of the header's status cluster, in logical pixels.
///
/// `4 * icon + 3 * gap`, the expression `ui/candidate.slint` declares for the component
/// itself. The cluster is a fixed-size block so that toggling a mode cannot reflow the
/// preedit, which is what makes its width a constant of the layout rather than a function of
/// the frame.
fn status_cluster_width(metrics: &Metrics) -> f32 {
    4.0 * metrics.header_icon_size + 3.0 * metrics.header_icon_gap
}

/// A candidate count as the component's integer property type.
fn count(candidates: usize) -> i32 {
    candidates.min(i32::MAX as usize) as i32
}

#[cfg(test)]
mod tests;
