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

use ime_types::ui::OverlayFrame;
use ime_types::{ImeError, PageDir, Placement, Rgba8, ThemeSpec, UiFrame};
use slint::{Color, ComponentHandle as _, SharedString, VecModel};

use crate::layout::{self, Metrics};
use crate::spring::{AnimationSet, FrameMotion, MotionConfig};
use crate::theme::{self, ThemeSink, ThemeTokens};
use crate::ui_generated::{CandidateData, CandidateWindow, Theme};

use self::cell::cell_data;

mod arrow;
mod cell;
mod frame;
mod highlight;
mod overlay;
mod preedit;

#[cfg(test)]
// Crate-visible rather than private: `surface`'s tests drive the same frames, and the
// fixtures they share (`frame_with`, `anchor`) live here rather than being written twice.
// The `#[cfg(test)]` keeps the module out of the shipped library entirely.
pub(crate) mod tests;

pub use self::arrow::arrow_in_container;
pub use self::cell::{CellState, Measure, PointerState, VisualState, local_position};
pub use self::frame::{DrawDelta, DrawState, RevisionGate, container_cap, draw_state};
pub use self::highlight::highlight_rect;
pub use self::preedit::{
    MIN_PREEDIT_WIDTH_DP, PREEDIT_MAX_CHARS, PreeditLayout, PreeditRun, RunKind,
    SECONDARY_STATUS_WIDTH_DP, layout_preedit,
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
    /// The preedit's runs left of the caret, as the header's model.
    ///
    /// Two models rather than one with a split index: the component draws them on either
    /// side of the caret, and a single model would make it slice on every frame.
    before: Rc<VecModel<crate::ui_generated::PreeditRun>>,
    /// The preedit's runs right of the caret.
    after: Rc<VecModel<crate::ui_generated::PreeditRun>>,
    /// The caret indicator's top-left corner in container-relative logical pixels, or `None`
    /// when the placement pass drew no arrow. See [`Adapter::set_arrow`].
    arrow: Option<(f32, f32)>,
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
        let before: Rc<VecModel<crate::ui_generated::PreeditRun>> = Rc::new(VecModel::default());
        let after: Rc<VecModel<crate::ui_generated::PreeditRun>> = Rc::new(VecModel::default());
        window.set_preedit_before(before.clone().into());
        window.set_preedit_after(after.clone().into());
        Ok(Self {
            window,
            metrics: layout::metrics()?,
            state: DrawState::default(),
            gate: RevisionGate::default(),
            visible: false,
            items,
            before,
            after,
            arrow: None,
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
        // The highlight is the frame's (ADR-0005's `UiFrame::highlight`): adopted before
        // the cells resolve, in the page-local coordinates the frame carries. A position
        // with no cell -- a drifted wire, never a well-formed frame -- hides the ring.
        self.state.pointer.highlighted = frame
            .highlight
            .filter(|&position| usize::from(position) < frame.candidates.len());
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
    /// The highlight is the frame's ([`UiFrame::highlight`], page-local, adopted by
    /// [`Adapter::apply_frame`]); the hover and the press are the router's. The caller
    /// re-anchors them to the page of the frame being drawn with
    /// [`PointerState::for_page`], passing the frame's highlight through untouched.
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
        // A model change does not reach the window's dirty flag on this platform, for the
        // same reason `apply_frame` and `advance` request the repaint themselves.
        self.request_repaint();
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

    /// Draws the overlay mode, or returns the window to the candidate view.
    ///
    /// `Some(frame)` covers the panel with the overlay component and fills it from the
    /// engine's frame; `None` hides the overlay and uncovers the candidate panel the
    /// window was drawing, which has been untouched since the overlay went up -- that is
    /// what makes closing a panel restore the candidates without a re-decode.
    ///
    /// Every call writes, and the caller repaints: the overlay channel is a mode rather
    /// than a frame, so the call runs when the host opens or closes a panel and never per
    /// keystroke, and the cost is a handful of property stores beside one repaint
    /// request.
    ///
    /// # Parameters
    ///
    /// * `frame` -- the overlay frame to draw, or `None` when no panel is open.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn apply_overlay(&mut self, frame: Option<&OverlayFrame>) {
        self.window.set_overlay_visible(frame.is_some());
        if let Some(frame) = frame {
            self.window
                .set_overlay_title(SharedString::from(frame.title.as_str()));
            self.window.set_overlay_sections(overlay::sections(frame));
        }
        self.request_repaint();
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

    /// Records where the caret indicator goes, in container-relative logical pixels.
    ///
    /// Called by the placement pass with [`arrow_in_container`]'s answer. The pass is the
    /// only layer that knows whether an arrow may be drawn at all -- a window flipped above
    /// the caret or pushed sideways has no straight line back to it, and the pass answers
    /// `None` for those -- but the component's vocabulary stays in this module, so the pass
    /// hands over a pair of numbers and this writes them.
    ///
    /// # Parameters
    ///
    /// * `arrow` -- the arrow's top-left corner in container-relative logical pixels, or
    ///   `None` when no arrow is drawn. The two coordinates are meaningless while it is
    ///   `None`.
    ///
    /// # Returns
    ///
    /// Whether what the component draws changed. A placement that lands the arrow where the
    /// previous one did writes nothing.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn set_arrow(&mut self, arrow: Option<(f32, f32)>) -> bool {
        if self.arrow == arrow {
            return false;
        }
        self.arrow = arrow;
        self.window.set_arrow_visible(arrow.is_some());
        if let Some((x, y)) = arrow {
            self.window.set_arrow_x(x);
            self.window.set_arrow_y(y);
        }
        self.request_repaint();
        true
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
        if delta.preedit {
            self.write_preedit();
        }
        if delta.status {
            self.write_status();
        }
        if delta.mode_label {
            self.window
                .set_mode_label(SharedString::from(self.state.mode_label.as_str()));
        }
        if delta.input_full {
            self.window.set_input_full(self.state.input_full);
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

    /// Writes the preedit's runs and the three facts the header draws them with.
    ///
    /// The two models are replaced rather than patched, for the reason the cells' model is:
    /// a frame that reaches here has already changed what the preedit draws, and one
    /// `set_vec` is a single notification instead of one per run.
    fn write_preedit(&self) {
        let before: Vec<crate::ui_generated::PreeditRun> =
            self.state.preedit.before.iter().map(run_data).collect();
        let after: Vec<crate::ui_generated::PreeditRun> =
            self.state.preedit.after.iter().map(run_data).collect();
        self.before.set_vec(before);
        self.after.set_vec(after);
        self.window
            .set_caret_visible(self.state.preedit.caret_visible);
        self.window
            .set_preedit_truncated(self.state.preedit.truncated);
        self.window
            .set_show_secondary_status(self.state.preedit.show_secondary_status);
    }

    /// Writes the four status facts the header's marker cluster draws.
    ///
    /// `chinese` is the strip's own bit (ADR-0005), not an inference from the label's
    /// text: a notice occupying the label slot must not light the mode dot.
    fn write_status(&self) {
        self.window.set_chinese(self.state.chinese);
        self.window.set_full_width(self.state.full_width);
        self.window
            .set_punctuation_full(self.state.punctuation_full);
        self.window.set_readonly(self.state.readonly);
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
        // The fade half of the motion (features.md 3.3.2): the panel rectangle binds
        // `window-opacity` to its own `opacity`, so this write is what fades the panel in
        // while it grows and out while it shrinks. The renderer side of that path is
        // pinned by the pixel probes in `renderer/tests.rs`.
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
    /// The value is the frame's ([`UiFrame::highlight`], adopted into the pointer state
    /// when the frame was applied), so `None` reaches the hidden branch below unchanged.
    /// A page with no candidate, and a position past the page's cells -- which a
    /// well-formed frame never carries -- hide the ring rather than draw off the grid.
    fn highlight_position(&self) -> Option<u16> {
        if self.state.cells.is_empty() {
            return None;
        }
        self.state
            .pointer
            .highlighted
            .filter(|&position| usize::from(position) < self.state.cells.len())
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

/// One preedit run, as the header's model holds it.
///
/// The kind travels as its wire value, because a `.slint` source has no enum of its own;
/// [`RunKind::code`] is the one place the mapping is written out.
fn run_data(run: &PreeditRun) -> crate::ui_generated::PreeditRun {
    crate::ui_generated::PreeditRun {
        text: SharedString::from(run.text.as_str()),
        kind: run.kind.code(),
    }
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
