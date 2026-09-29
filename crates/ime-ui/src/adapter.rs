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
//! # Concurrency
//!
//! The adapter is built on the UI thread and never leaves it: the component it holds is
//! reference counted and is not `Send`. Every method takes `&mut self` and is called from that
//! thread only, which is also the thread Slint's platform is installed on.

use std::rc::Rc;

use ime_types::{CandidateSource, ImeError, Rgba8, ThemeSpec, UiFrame};
use slint::{Color, ComponentHandle as _, SharedString, VecModel};

use crate::layout::{self, Metrics};
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
        }
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

    /// Maps or unmaps the surface.
    ///
    /// Idempotent: asking for the visibility the window already has writes nothing. Mapping
    /// the surface is what makes the component appear at all -- an unshown component is laid
    /// out but never drawn into a buffer.
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
        Ok(())
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
