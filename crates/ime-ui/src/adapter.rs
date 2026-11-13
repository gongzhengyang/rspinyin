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
use ime_types::{ImeError, Placement, Rgba8, UiFrame};
use slint::{ComponentHandle as _, SharedString, VecModel};

use crate::layout::{self, Metrics};
use crate::spring::{AnimationSet, BlendTransition, FrameMotion, MotionConfig, TRANSPARENT};
use crate::theme::ThemeSink;
use crate::ui_generated::{CandidateData, CandidateWindow};

use self::theme::ThemeGlide;
use self::write::WindowTheme;

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
mod theme;
mod window;
mod write;

pub use self::arrow::arrow_in_container;
pub use self::cell::{CellState, Measure, PointerState, VisualState, local_position};
pub use self::frame::{DrawDelta, DrawState, RevisionGate, container_cap, draw_state};
pub use self::highlight::highlight_rect;
pub use self::preedit::{
    MIN_PREEDIT_WIDTH_DP, PREEDIT_MAX_CHARS, PreeditLayout, PreeditRun, RunKind,
    SECONDARY_STATUS_WIDTH_DP, layout_preedit,
};
pub use self::window::SURFACE_FAILED_CODE;

/// Recorded when the candidate window's component cannot be created.
///
/// The window is the whole point of the user-interface addon, so a component that cannot be
/// bound to the platform is reported where the platform's own install failure is reported:
/// there is no self-drawn candidate window, and the host's own user interface has to take
/// over.
pub const COMPONENT_FAILED_CODE: &str = "ui/slint/component";

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
    /// Whether a staged unmap is waiting for the exit fade to settle.
    ///
    /// The window stays mapped while it is set -- that is what makes the fade visible --
    /// and [`Adapter::advance`] clears it by performing the real unmap once the motion
    /// has settled; [`Adapter::unmap_now`] clears it the same way, without the wait.
    pending_hide: bool,
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
    /// The window's motions: the highlight slide, the appear and disappear motion, the
    /// page-content slide, and the pressed cell's sink of 3.4's Active row.
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
    /// The theme crossfade: the accent and the base alpha gliding toward the last
    /// applied theme, on 3.3.2's 120ms.
    theme_fade: BlendTransition<ThemeGlide>,
    /// The fade frame last written into the component: the global's accent and
    /// alpha, and the cluster's four marker colours. A fade frame equal to this
    /// one is not written, which is what keeps a settled window write-free.
    drawn_fade: (Rgba8, u8, [Rgba8; 4]),
    /// Whether a theme has been applied. The first one lands at once -- there is
    /// no previous palette for it to glide away from.
    themed: bool,
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
    /// Creates the component with the built-in motion; see [`Self::with_motion`].
    pub fn new() -> Result<Self, ImeError> {
        Self::with_motion(MotionConfig::default())
    }

    /// Creates the component against the Slint platform of the calling thread, with the
    /// motion settings `[ui.animation]` assembles.
    ///
    /// # Parameters
    ///
    /// * `motion` -- what `[ui.animation]` assembled; read once, here, never re-tuned.
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
    pub fn with_motion(motion: MotionConfig) -> Result<Self, ImeError> {
        let window = CandidateWindow::new().map_err(|_| ImeError::CompositorUnsupported {
            detail: String::from(COMPONENT_FAILED_CODE),
        })?;
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
            pending_hide: false,
            items,
            before,
            after,
            arrow: None,
            measure: Measure::default(),
            motion: AnimationSet::new(motion),
            drawn_motion: None,
            theme_fade: BlendTransition::new(ThemeGlide {
                accent: TRANSPARENT,
                base_alpha: 0,
            }),
            drawn_fade: (TRANSPARENT, 0, [TRANSPARENT; 4]),
            themed: false,
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
        if !self.motion.is_enabled() {
            // The disabled path lands everything at once: a flag that flipped in
            // this frame started its marker fade inside the mapping, and the snap
            // is what makes the frame instantaneous and the screenshot exact.
            self.state.snap_markers();
        }
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
        // 3.4's Active sink is the fifth motion: a press that starts or ends moves the
        // spring; one already held retargets it where it travels and arms nothing.
        self.motion.set_pressed(pointer.pressed.is_some());
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
    /// mid-flight lands everything at once -- the crossfades with the springs -- and
    /// switching it back on restarts nothing. While the switch is off a theme change lands
    /// directly too, which is what keeps the disabled path deterministic.
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
        if !enabled {
            let glide = self.theme_fade.target();
            self.theme_fade.snap_to(glide);
            self.state.snap_markers();
        }
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
}
