//! The frame adapter: the one place a `UiFrame` becomes Slint properties.
//!
//! Responsibility: own the `CandidateWindow` component, turn one immutable frame into the
//! property writes the component draws from, and nothing else. It does not decode, does not
//! place the window, does not measure text with font metrics and does not read the clock.
//!
//! Boundaries: no `slint::*` type may appear in a public signature or field (Slint
//! Royalty-free 2.0 `OB-4`, enforced by `scripts/check-slint-leak.sh`). The component handle
//! therefore lives in a private field of a type whose public API speaks only in `ime_types`
//! vocabulary.
//!
//! # Why the mapping lives here and not in the component
//!
//! `ui/candidate.slint` is declaration only: no branch over candidate data, no string
//! formatting, no measurement. Keeping the mapping on this side is what makes the render path
//! a pure function of the frame, and what lets the whole mapping be covered by a test that
//! never opens a window.
//!
//! # The window's motion
//!
//! The adapter owns the [`AnimationSet`] as well as the properties it writes, because the
//! highlight box's resting place is a function of the cell geometry and the pointer state --
//! both of which this type owns -- and because the component's motion properties are this
//! type's own vocabulary. What it does not own is the clock: `dt` arrives from the UI thread,
//! which is the only layer that has an instant, and nothing here reads one. A window at rest
//! reports that nothing is in flight and writes no property at all, which is what lets the
//! loop block indefinitely.
//!
//! # Concurrency
//!
//! The adapter is built on the UI thread and never leaves it: the component it holds is
//! reference counted and is not `Send`. Every method takes `&mut self` and is called from that
//! thread only, which is also the thread Slint's platform is installed on.

use std::rc::Rc;

use ime_types::{CandidateSource, ImeError, PageDir, Placement, Rgba8, ThemeSpec, UiFrame};
use slint::{Color, ComponentHandle as _, SharedString, VecModel};

use crate::layout::{self, Metrics};
use crate::spring::{AnimationSet, FrameMotion, HighlightRect, MotionConfig};
use crate::theme::{self, ThemeSink, ThemeTokens};
use crate::ui_generated::{CandidateData, CandidateWindow, Theme};

mod cell;
mod frame;

#[cfg(test)]
// Crate-visible rather than private: `surface`'s tests drive the same frames, and the
// fixtures they share (`frame_with`, `anchor`) live here rather than being written twice.
// The `#[cfg(test)]` keeps the module out of the shipped library entirely.
pub(crate) mod tests;

pub use self::cell::{CellState, Measure, PointerState, VisualState, local_position};
pub use self::frame::{
    DrawDelta, DrawState, PREEDIT_MAX_CHARS, RevisionGate, container_cap, draw_state,
};

/// Recorded when the candidate window's component cannot be created.
///
/// The window is the whole point of the user-interface addon, so a component that cannot be
/// bound to the platform is reported where the platform's own install failure is reported:
/// there is no self-drawn candidate window, and the host's own user interface has to take
/// over.
pub const COMPONENT_FAILED_CODE: &str = "ui/slint/component";

/// Recorded when the window cannot be mapped or unmapped.
pub const SURFACE_FAILED_CODE: &str = "ui/slint/surface";

/// The candidate window's binding to one frame.
///
/// The component is created against the Slint platform the calling thread installed, and every
/// write into it goes through [`Adapter::apply_frame`] or [`Adapter::apply_theme`].
pub struct Adapter {
    /// The component the frame is drawn with.
    ///
    /// Private on purpose: a `pub` field of a Slint type would put the Slint API into this
    /// crate's public surface, which the royalty-free licence forbids (`OB-4`).
    window: CandidateWindow,
    /// The component's constants, parsed once from `ui/candidate.slint`.
    metrics: &'static Metrics,
    /// The state the component currently draws.
    state: DrawState,
    /// The newest frame revision that was drawn.
    gate: RevisionGate,
    /// Whether the surface is mapped.
    visible: bool,
    /// The candidate cells, as the grid's model.
    ///
    /// Created once and written in place: the property is what the grid draws from, and
    /// replacing it on every frame would rebuild the model and re-notify the view for a page
    /// that did not change.
    items: Rc<VecModel<CandidateData>>,
    /// The width estimator, kept across frames so a candidate text is measured once rather
    /// than once per keystroke.
    measure: Measure,
    /// The window's motions: the highlight slide, the appear and disappear motion, and the
    /// page-content slide.
    ///
    /// Owned here rather than by the surface because the highlight's resting place is a
    /// function of the cell geometry and the pointer state, both of which this type owns. The
    /// clock stays with the surface, which is the only layer that has an instant.
    motion: AnimationSet,
    /// The last frame of motion written into the component.
    ///
    /// Kept so a settled window writes nothing at all: the loop may block indefinitely only
    /// while nothing is in flight, and a property write per wake-up would be work the idle
    /// budget does not have.
    drawn_motion: Option<FrameMotion>,
    /// The page the component is drawing, one-based like the frame's, or zero before the
    /// first frame. A frame that shows a different page is a page turn.
    page: u8,
    /// The scale factor of the surface the window is drawn on, from the last frame's anchor.
    ///
    /// The motion draws in logical pixels and writes only logical properties, but the damage
    /// a highlight step reports is measured in physical ones.
    scale: f32,
}

impl Adapter {
    /// Creates the component against the Slint platform of the calling thread.
    ///
    /// The platform has to be installed before this call. A component that cannot be built is
    /// reported here rather than at the first frame, because a window that will never appear
    /// is something the host decides about while it is still starting up.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] carrying [`COMPONENT_FAILED_CODE`] when the
    /// component cannot be bound to the platform, and [`ImeError::ConfigInvalid`] when
    /// `ui/candidate.slint` no longer declares the metrics block the layout computes with.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Result<Self, ImeError> {
        let window = CandidateWindow::new().map_err(|_| component_error())?;
        let items: Rc<VecModel<CandidateData>> = Rc::new(VecModel::default());
        window.set_items(items.clone().into());
        Ok(Self {
            window,
            metrics: layout::metrics()?,
            state: DrawState::default(),
            gate: RevisionGate::default(),
            visible: false,
            items,
            measure: Measure::default(),
            motion: AnimationSet::new(MotionConfig::default()),
            drawn_motion: None,
            page: 0,
            scale: 1.0,
        })
    }

    /// Draws one frame, dropping it when it is older than the one already drawn.
    ///
    /// Only the properties the frame changed are written, so a frame that draws what is
    /// already on screen costs nothing.
    ///
    /// # Parameters
    ///
    /// * `frame` -- the engine's snapshot of the window.
    ///
    /// # Returns
    ///
    /// Whether the frame was drawn. `false` means it was older than the frame on screen and
    /// was dropped.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A property write cannot fail, and
    /// a frame that cannot be understood is answered by the layout's own clamps.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn apply_frame(&mut self, frame: &UiFrame) -> bool {
        if !self.gate.accept(frame.revision) {
            return false;
        }
        let delta = self.state.update_cached(
            frame,
            container_cap(frame, self.metrics),
            self.metrics,
            &mut self.measure,
        );
        if !delta.is_empty() {
            self.write(&delta);
            self.request_repaint();
        }
        self.scale = frame.anchor.scale;
        self.turn_page(frame);
        self.retarget_highlight();
        true
    }

    /// Re-draws the grid for a new pointer and highlight state.
    ///
    /// `UiFrame` carries the candidates and the page, not the highlighted or the hovered
    /// one: the engine's paging state holds those, and the frame is the UI thread's only view
    /// of the session. The pointer state is therefore an input of the adapter rather than a
    /// field of the snapshot, and the caller re-anchors it to the page of the frame it is
    /// drawing with [`PointerState::for_page`].
    ///
    /// # Parameters
    ///
    /// * `pointer` -- the state to draw, in positions within the page on show.
    ///
    /// # Returns
    ///
    /// Whether what the grid draws changed. `false` means the window already draws it, so a
    /// pointer that did not move costs nothing.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn apply_pointer(&mut self, pointer: PointerState) -> bool {
        if self.state.pointer == pointer {
            return false;
        }
        self.state.pointer = pointer;
        // Before the early return below: a pointer that changed nothing the grid draws can
        // still have moved the highlight, and the box must be redirected either way.
        self.retarget_highlight();
        if !self.state.resolve_states() {
            return false;
        }
        self.write_items();
        true
    }

    /// Writes a resolved theme into the component.
    ///
    /// Three properties of the theme global are written and nothing else: every other token
    /// derives from them, so a theme switch -- including the dark-to-light one -- is a
    /// property update rather than a rebuild, and cannot flash. The container radius is the
    /// one value that lives outside the global, because the component owns its geometry.
    ///
    /// # Parameters
    ///
    /// * `spec` -- the host's theme request, which carries the configured corner radius.
    /// * `tokens` -- the resolution to apply. Apply the tokens rather than the raw request, so
    ///   the alpha written is the one the contrast gate approved.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn apply_theme(&mut self, spec: &ThemeSpec, tokens: &ThemeTokens) {
        let mut sink = WindowTheme {
            window: &self.window,
        };
        theme::apply(&mut sink, tokens);
        self.window
            .set_container_radius(f32::from(spec.corner_radius_dp));
    }

    /// Writes the font family the window draws with.
    ///
    /// Called once, from the surface's construction, with the family the startup probe
    /// chose. Kept out of [`Adapter::apply_theme`] on purpose: the family does not change
    /// with the theme, and rewriting it on every light/dark switch would be work the probe
    /// exists to avoid repeating.
    ///
    /// An empty name leaves the view's own default in place, which is what a machine with
    /// no CJK family at all gets.
    ///
    /// # Parameters
    ///
    /// - `family`: a family name, or the empty string.
    ///
    /// # Returns
    ///
    /// `true` when a name was written.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn apply_font_family(&mut self, family: &str) -> bool {
        if family.is_empty() {
            return false;
        }
        let mut sink = WindowTheme {
            window: &self.window,
        };
        sink.set_font_family(family);
        true
    }

    /// Maps or unmaps the surface, and starts the motion that goes with it.
    ///
    /// Idempotent: asking for the visibility the window already has writes nothing. Mapping
    /// the surface is what makes the component appear at all -- an unshown component is laid
    /// out but never drawn into a buffer -- and it is also what starts 3.3.2's appear motion,
    /// so the panel grows out of the caret instead of arriving at full size. Unmapping starts
    /// the disappear motion; a window unmapped on the same call draws none of it, which is the
    /// host's shutdown path and the reason the motion costs nothing there.
    ///
    /// # Parameters
    ///
    /// * `visible` -- `true` to map the surface, `false` to unmap it.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] carrying [`SURFACE_FAILED_CODE`] when the
    /// platform refuses to map or unmap the window, which means the candidate window cannot be
    /// shown at all.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn set_visible(&mut self, visible: bool) -> Result<(), ImeError> {
        if self.visible == visible {
            return Ok(());
        }
        let outcome = if visible {
            self.window.show()
        } else {
            self.window.hide()
        };
        outcome.map_err(|_| surface_error())?;
        self.visible = visible;
        if visible {
            self.motion.appear();
        } else {
            self.motion.disappear();
        }
        Ok(())
    }

    /// Advances the window's motion by `dt` and writes the frame it produced.
    ///
    /// The delta comes from the UI thread's clock, which is the only place an instant is read:
    /// a session's first call passes zero, and a late frame passes a delta large enough that
    /// the integrator clamps it, so a stalled compositor slows the motion down rather than
    /// making it jump.
    ///
    /// A step whose result is the one already on screen writes nothing, so a window at rest
    /// costs no property write per wake-up.
    ///
    /// # Parameters
    ///
    /// * `dt` -- seconds since the previous call. Negative, zero and unusable values carry no
    ///   time rather than running the motion backwards.
    ///
    /// # Returns
    ///
    /// Whether anything is still in flight. `false` is what lets the caller block
    /// indefinitely, which is the whole of the idle CPU budget: a window at rest must not keep
    /// a timer armed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn advance(&mut self, dt: f32) -> bool {
        let motion = self.motion.step(dt, self.scale);
        // The visibility of the box is not part of `FrameMotion`, so it is checked against the
        // component's own value rather than against the previous frame. A box that hid without
        // moving -- the page emptied while the highlight was already at rest on a cell -- would
        // otherwise produce a frame equal to the one before it and never reach the component,
        // leaving it drawing a box the engine no longer has a candidate for.
        let visible = self.motion.highlight().is_visible();
        if self.drawn_motion != Some(motion) || self.window.get_highlight_visible() != visible {
            self.write_motion(&motion);
            self.drawn_motion = Some(motion);
            self.request_repaint();
        }
        motion.animating
    }

    /// Asks the platform for a repaint.
    ///
    /// Slint's own dirty tracking does not reach this window: the platform is a custom one, and
    /// a component whose properties have just been written is not marked for redraw by anything
    /// in Slint's property machinery. The write is therefore followed by an explicit request --
    /// without it the surface draws the frame `show()` left behind and every later one is
    /// dropped, because `render_if_dirty` has nothing to see.
    fn request_repaint(&self) {
        self.window.window().request_redraw();
    }

    /// Switches the window's motion on or off.
    ///
    /// `[ui.animation] enabled = false` arrives here: every motion jumps to its end state,
    /// which is the low-end-device path and the path a visual regression screenshot needs,
    /// where a frame must not depend on how many frames came before it. Switching it off
    /// mid-flight lands everything at once, and switching it back on restarts nothing.
    ///
    /// The spring's frequency and damping are fixed when the adapter is built; a
    /// configuration that changes them needs a new [`AnimationSet`], not this call.
    ///
    /// # Parameters
    ///
    /// * `enabled` -- whether the window animates at all.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn set_motion_enabled(&mut self, enabled: bool) {
        self.motion.set_enabled(enabled);
    }

    /// Records where the panel was placed, which is what anchors the appear motion.
    ///
    /// 3.3.2 grows the panel out of its cursor-side edge: a panel below the caret grows
    /// downward from its top edge, one above grows upward from its bottom edge. The geometry
    /// pass is the only layer that knows which side won -- it may flip the panel the anchor
    /// asked for -- so the caller passes its answer rather than the request.
    ///
    /// # Parameters
    ///
    /// * `placement` -- the side the panel ended up on. [`Placement::Auto`] resolves to the
    ///   side below the caret, which is what the pass does with it.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn set_placement(&mut self, placement: Placement) {
        self.window
            .set_grows_upward(matches!(placement, Placement::Above));
    }

    /// The state the component is drawing.
    ///
    /// The placement pass sizes the panel from these numbers rather than recomputing them, so
    /// the rectangle the pointer is tested against is the one that was drawn.
    ///
    /// # Returns
    ///
    /// The state of the last accepted frame.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn state(&self) -> &DrawState {
        &self.state
    }

    /// The component itself, for the tests that read the properties back.
    ///
    /// The properties are what the binding actually produced, so a test that asserts on them
    /// covers the write path and not only the mapping that feeds it.
    #[cfg(test)]
    pub(crate) fn window(&self) -> &CandidateWindow {
        &self.window
    }

    /// Writes the properties a frame changed.
    fn write(&mut self, delta: &DrawDelta) {
        if delta.preedit_text {
            self.window
                .set_preedit_text(SharedString::from(self.state.preedit_text.as_str()));
        }
        if delta.mode_label {
            self.window
                .set_mode_label(SharedString::from(self.state.mode_label.as_str()));
        }
        if delta.item_count {
            self.window.set_item_count(self.state.item_count);
        }
        if delta.max_per_row {
            self.window.set_max_per_row(self.state.max_per_row);
        }
        if delta.grid_rows {
            self.window.set_grid_rows(self.state.grid_rows);
        }
        if delta.cell_width {
            self.window.set_cell_width(self.state.cell_width);
        }
        if delta.container_width {
            self.window.set_container_width(self.state.container_width);
        }
        if delta.container_height {
            self.window
                .set_container_height(self.state.container_height);
        }
        if delta.header_height {
            self.window.set_header_height(self.state.header_height);
        }
        if delta.show_annotation {
            self.window.set_show_annotation(self.state.show_annotation);
        }
        if delta.cells {
            self.write_items();
        }
    }

    /// Writes the page's cells into the grid's model.
    ///
    /// The model is replaced rather than patched row by row: a frame that reaches here has
    /// already changed what the page draws, and one `set_vec` is a single notification
    /// instead of one per row. A frame that changes nothing never reaches this call, which is
    /// what keeps a repeated keystroke free.
    fn write_items(&self) {
        let items: Vec<CandidateData> = self.state.cells.iter().map(cell_data).collect();
        self.items.set_vec(items);
    }

    /// Writes one frame of the window's motion into the component.
    ///
    /// Every value is in logical pixels: the highlight box's four components are in the
    /// candidate grid's own space, which is the space the springs integrate in, and the window
    /// adds the container padding and the header block when it draws the box.
    fn write_motion(&self, motion: &FrameMotion) {
        let rect = motion.highlight.rect;
        self.window.set_highlight_x(rect.x);
        self.window.set_highlight_y(rect.y);
        self.window.set_highlight_w(rect.w);
        self.window.set_highlight_h(rect.h);
        self.window
            .set_highlight_visible(self.motion.highlight().is_visible());
        // `window-opacity` is still written, and still declared by the component, but
        // nothing is bound to it: see the note in `candidate.slint` for why a binding is
        // worse than no binding on this renderer. A renderer that gains support for a
        // bound opacity will need that decision revisited, not just this line.
        self.window.set_window_opacity(motion.opacity);
        self.window.set_window_scale(motion.scale);
        self.window.set_page_offset_dp(motion.page_offset_dp);
    }

    /// Re-anchors the highlight box on the cell the pointer state names.
    ///
    /// Called whenever what the grid draws changes: a frame that brings a new page or a new
    /// cell width moves the cell the box belongs on, and a pointer that moves the highlight
    /// redirects the springs, which keep the velocity of a box already in flight (3.3.1).
    fn retarget_highlight(&mut self) {
        let Some(position) = self.highlight_position() else {
            self.motion.set_highlight_visible(false);
            return;
        };
        // Retargeted before the box is made visible, and the order is load bearing: a box that
        // is not on screen has no flight to continue, so the redirect places it on the cell
        // rather than flying it in from wherever the previous appearance left it. An already
        // visible box keeps its velocity instead.
        self.motion
            .retarget_highlight(highlight_rect(&self.state, position, self.metrics));
        self.motion.set_highlight_visible(true);
    }

    /// The candidate the highlight box belongs on, or `None` when the page has none.
    ///
    /// A page with no candidate has nothing to highlight, and the box is hidden rather than
    /// left on the cell the page the user came from had.
    fn highlight_position(&self) -> Option<u16> {
        if self.state.cells.is_empty() {
            return None;
        }
        self.state.pointer.highlighted
    }

    /// Starts the page-content slide when the frame shows a different page.
    ///
    /// `UiFrame` carries the page the engine is on, so the direction is the sign of the change.
    /// A frame that redraws the same page slides nothing, and neither does a session's first
    /// frame: there is no previous page for the content to have come from.
    fn turn_page(&mut self, frame: &UiFrame) {
        let current = frame.page.current;
        let previous = core::mem::replace(&mut self.page, current);
        if previous == 0 || previous == current {
            return;
        }
        let dir = if current > previous {
            PageDir::Next
        } else {
            PageDir::Prev
        };
        self.motion.turn_page(dir);
    }
}

/// The rectangle the highlight box rests on, in the candidate grid's own coordinates.
///
/// The grid's origin is the candidate area's top-left corner, inside the container padding and
/// below the header: the window adds both when it draws the box, which is the only place the
/// two spaces meet. The arithmetic is the hit map's (`crate::geometry`), restated in logical
/// pixels, so the box the user sees and the cell the pointer hits cannot drift apart.
///
/// # Parameters
///
/// * `state` -- the state the component is drawing, which fixes the cell width, the columns
///   per row and the gaps the grid draws with.
/// * `position` -- the candidate's zero-based position within the page, which is what
///   [`PointerState`] holds and what the grid numbers its cells by.
/// * `metrics` -- the component's constants, from [`crate::layout::metrics`].
///
/// # Returns
///
/// The cell's rectangle in logical pixels, relative to the candidate grid's origin. A position
/// past the last cell of the page still lands on the grid, which is what keeps a stale
/// highlight visible instead of collapsing it onto the origin.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics: the column count is kept away from zero and the arithmetic is `f32`.
pub fn highlight_rect(state: &DrawState, position: u16, metrics: &Metrics) -> HighlightRect {
    // A page with no column -- an empty state -- still has one, which is what the grid itself
    // clamps to when it lays its rows out.
    let columns = state.max_per_row.max(1);
    let position = i32::from(position);
    let column = position % columns;
    let row = position / columns;
    HighlightRect::new(
        column as f32 * (state.cell_width + metrics.grid_gap),
        row as f32 * (metrics.cell_height + metrics.grid_gap),
        state.cell_width.max(0.0),
        metrics.cell_height.max(0.0),
    )
}

/// The component's theme global, as [`ThemeSink`] sees it.
///
/// A borrow rather than a stored handle: the global is reached through the component, so
/// holding one would tie the adapter's lifetime to the component it is stored beside.
struct WindowTheme<'a> {
    window: &'a CandidateWindow,
}

impl ThemeSink for WindowTheme<'_> {
    fn set_dark(&mut self, dark: bool) {
        self.window.global::<Theme>().set_dark(dark);
    }

    fn set_accent(&mut self, accent: Rgba8) {
        self.window
            .global::<Theme>()
            .set_accent(Color::from_argb_u8(accent.a, accent.r, accent.g, accent.b));
    }

    fn set_font_family(&mut self, family: &str) {
        if family.is_empty() {
            return;
        }
        self.window
            .global::<Theme>()
            .set_font_family(SharedString::from(family));
    }

    fn set_base_alpha(&mut self, alpha: f32) {
        self.window.global::<Theme>().set_base_alpha(alpha);
    }
}

/// One cell, as the grid's model holds it.
///
/// The three state booleans are derived from the single state the ranking of 3.4 resolved, so
/// they can never contradict each other, and the full text travels beside the text that is
/// drawn: a cell cut with `…` still carries what a selection commits.
fn cell_data(cell: &CellState) -> CandidateData {
    CandidateData {
        index: i32::from(cell.index),
        label: SharedString::from(cell.label.as_str()),
        text: SharedString::from(cell.text.as_str()),
        display_text: SharedString::from(cell.display_text.as_str()),
        annotation: SharedString::from(cell.annotation.as_str()),
        source: source_code(cell.source),
        is_highlighted: cell.state == VisualState::FocusRing,
        is_hovered: cell.state == VisualState::Hover,
        is_pressed: cell.state == VisualState::Active,
        is_disabled: cell.state == VisualState::Disabled,
    }
}

/// The number the grid carries a candidate's source as.
///
/// A `.slint` source has no enum of its own and `CandidateSource` is a frozen contract type,
/// so the variant travels as its own number: the mapping is written out rather than derived
/// from the discriminant, which makes a variant added later a compile error here instead of
/// a silently renumbered source.
fn source_code(source: CandidateSource) -> i32 {
    match source {
        CandidateSource::Dict => 0,
        CandidateSource::UserDict => 1,
        CandidateSource::Learned => 2,
        CandidateSource::Passthrough => 3,
        CandidateSource::Symbol => 4,
        CandidateSource::Phrase => 5,
        CandidateSource::Script => 6,
    }
}

/// The error a component that cannot be created reports.
fn component_error() -> ImeError {
    ImeError::CompositorUnsupported {
        detail: String::from(COMPONENT_FAILED_CODE),
    }
}

/// The error a window that cannot be mapped or unmapped reports.
fn surface_error() -> ImeError {
    ImeError::CompositorUnsupported {
        detail: String::from(SURFACE_FAILED_CODE),
    }
}

#[cfg(test)]
// Inline rather than in `adapter/tests.rs`, and the split is by what a test needs: this one
// covers a pure function of the drawn state, so it needs no component, no platform and no
// fixtures, and a unit test belongs beside the code it covers. The tests that drive a live
// window stay in `adapter/tests.rs`, where the fixtures they share live.
mod highlight_tests {
    use crate::layout::{self, Metrics};

    use super::{DrawState, highlight_rect};

    /// The width of one candidate cell: two CJK glyphs and the chrome around them, which is
    /// what the frames the adapter maps carry.
    fn cell_width(metrics: &Metrics) -> f32 {
        2.0 * metrics.font_size_cell + metrics.cell_chrome_width
    }

    #[test]
    fn test_highlight_rect_of_a_degenerate_state_stays_on_the_grid() {
        let metrics = layout::metrics().expect("ui/candidate.slint declares its constants");
        // An empty state has no column count and no cell width, which is what a page drawn
        // before its first frame holds. The column count is kept away from zero and the
        // position is still placed on the grid rather than dropped.
        let empty = highlight_rect(&DrawState::default(), 3, metrics);
        assert_eq!(
            empty.x, 0.0,
            "a page with no column must not divide by zero"
        );
        assert_eq!(empty.w, 0.0, "and a cell with no width draws nothing");
        assert_eq!(
            empty.y,
            3.0 * (metrics.cell_height + metrics.grid_gap),
            "the position is placed on the grid, one row step per row"
        );

        // A position past the last row still lands on the grid rather than off it.
        let state = DrawState {
            max_per_row: 5,
            cell_width: cell_width(metrics),
            ..DrawState::default()
        };
        let past = highlight_rect(&state, 12, metrics);
        let step_x = cell_width(metrics) + metrics.grid_gap;
        let step_y = metrics.cell_height + metrics.grid_gap;
        assert_eq!(
            (past.x, past.y),
            (2.0 * step_x, 2.0 * step_y),
            "the thirteenth candidate is two columns right and two rows down"
        );
        assert_eq!(past.w, cell_width(metrics), "and it is one cell wide");
        assert_eq!(past.h, metrics.cell_height, "and one cell tall");
    }
}
