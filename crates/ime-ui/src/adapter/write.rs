//! The half of the adapter that writes the component's properties.
//!
//! A frame reaches the adapter as a [`DrawDelta`] of changed facts; this module is the
//! one place those facts become property writes on the window and its models. It also
//! holds the [`ThemeSink`] bridge the theme resolution drives, which is a write like any
//! other: the global's properties are the window's, only reached through a different
//! door.

use slint::{Color, ComponentHandle as _, SharedString};

use ime_types::{PageDir, Rgba8, UiFrame};

use crate::spring::FrameMotion;
use crate::theme::ThemeSink;
use crate::ui_generated::{CandidateData, CandidateWindow, Theme};

use super::cell::cell_data;
use super::{Adapter, DrawDelta, PreeditRun, highlight_rect};

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

impl Adapter {
    /// Writes the properties a frame changed.
    pub(super) fn write(&mut self, delta: &DrawDelta) {
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
    pub(super) fn write_items(&self) {
        let items: Vec<CandidateData> = self.state.cells.iter().map(cell_data).collect();
        self.items.set_vec(items);
    }

    /// Writes one frame of the window's motion into the component.
    ///
    /// Every value is in logical pixels: the highlight box's four components are in the
    /// candidate grid's own space, which is the space the springs integrate in, and the window
    /// adds the container padding and the header block when it draws the box.
    pub(super) fn write_motion(&self, motion: &FrameMotion) {
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
    pub(super) fn retarget_highlight(&mut self) {
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
    pub(super) fn turn_page(&mut self, frame: &UiFrame) {
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

/// The component's theme global, as [`ThemeSink`] sees it.
///
/// A borrow rather than a stored handle: the global is reached through the component, so
/// holding one would tie the adapter's lifetime to the component it is stored beside.
pub(super) struct WindowTheme<'a> {
    /// The component whose theme global every write lands on.
    pub(super) window: &'a CandidateWindow,
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
