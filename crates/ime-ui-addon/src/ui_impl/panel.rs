//! The host's input panel, captured as the validation record and the fallback source.
//!
//! Responsibility: validate one input-panel snapshot, store it in the latest-wins slot,
//! and hand it back to a caller that needs it. The slot keeps its two `String`s, so a
//! steady-state update allocates nothing.
//!
//! Boundaries: the host's candidate list is never this plugin's source of candidates, so
//! nothing here derives a frame or decides what to draw. Normalising a caret rectangle is
//! `crate::cursor`'s job, and the answer the update callback returns is always `false` --
//! a panel update is a state change to record, not a drawing instruction.

use std::sync::{Mutex, MutexGuard};

use crate::ffi::emit_diagnostic;

/// One input-panel snapshot, borrowed from the host buffer for the length of the call.
///
/// A plain data carrier: [`crate::ffi::abi`] reads the C struct and validates the two
/// buffers before constructing it, so everything here is already a safe Rust value.
#[derive(Clone, Copy, Debug)]
pub struct PanelUpdate<'a> {
    /// The input context the panel belongs to.
    pub ic: u64,
    /// Preedit text as the host holds it, in UTF-8.
    pub preedit: &'a str,
    /// Caret position inside `preedit`, in bytes, as the host reported it. The preedit
    /// builder clamps it to a character boundary; this layer does not touch it.
    pub caret: u32,
    /// Candidate texts as the host holds them, `'\n'`-separated.
    pub candidates: &'a str,
    /// Number of candidates in `candidates`.
    pub candidate_count: u32,
    /// Highlighted candidate, or -1 when nothing is highlighted.
    pub cursor_index: i32,
    /// Current page, 0 when the host's list is not pageable.
    pub page: u8,
    /// Number of pages, 0 when the host's list is not pageable.
    pub total_pages: u8,
    /// Number of candidates on the current page.
    pub page_size: u8,
}

/// The host panel as the last update described it.
///
/// The host's candidate list is never this plugin's source of candidates, so the mirror
/// is the validation record and the fallback for a frame the decoder could not build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelMirror {
    /// The input context the panel belongs to.
    pub ic: u64,
    /// Preedit text as the host holds it.
    pub preedit: String,
    /// Caret position inside `preedit`, in bytes.
    pub caret: u32,
    /// Candidate texts as the host holds them, `'\n'`-separated.
    pub candidates: String,
    /// Number of candidates in `candidates`.
    pub candidate_count: u32,
    /// Highlighted candidate, or -1 when nothing is highlighted.
    pub cursor_index: i32,
    /// Current page, 0 when the host's list is not pageable.
    pub page: u8,
    /// Number of pages, 0 when the host's list is not pageable.
    pub total_pages: u8,
    /// Number of candidates on the current page.
    pub page_size: u8,
}

impl PanelMirror {
    /// Replaces the mirror's contents with `panel`, reusing both buffers.
    ///
    /// The reuse is what keeps a steady-state update allocation-free: `clear` keeps the
    /// capacity, so the two `push_str` calls only allocate while the panel grows.
    pub(super) fn refill(&mut self, panel: PanelUpdate<'_>) {
        self.ic = panel.ic;
        self.caret = panel.caret;
        self.candidate_count = panel.candidate_count;
        self.cursor_index = panel.cursor_index;
        self.page = panel.page;
        self.total_pages = panel.total_pages;
        self.page_size = panel.page_size;
        self.preedit.clear();
        self.preedit.push_str(panel.preedit);
        self.candidates.clear();
        self.candidates.push_str(panel.candidates);
    }
}

/// Handles `UserInterface::update` for the input panel.
///
/// Captures the panel into [`panel_mirror`] and answers whether a frame must be drawn.
/// The answer is always `false`, and deliberately so: this plugin's frames are driven
/// by the decoder posting [`ime_types::UiCommand::Frame`], so a host panel update is a
/// state change to record, not a drawing instruction. Returning `true` would make the
/// host expect a frame this callback never produced.
///
/// A snapshot whose candidate count disagrees with its buffer, or whose highlighted
/// index is outside the list, is a broken host rather than a panel: it is diagnosed as
/// `ffi/invalid-panel-snapshot` and dropped, leaving the previous mirror in place. The
/// host sends a panel update per keystroke, so that diagnostic goes through the throttle
/// in `crate::ffi`, which writes it once per second and counts the rest.
pub fn on_input_panel_update(panel: PanelUpdate<'_>) -> bool {
    if !is_consistent(&panel) {
        emit_diagnostic("ffi/invalid-panel-snapshot");
        return false;
    }
    store_panel(panel);
    false
}

/// Whether a snapshot's numbers agree with its buffers.
///
/// The host builds both together, so a disagreement means one of them was misread or
/// the caller is not the host this ABI describes. Everything the host can legitimately
/// send — an empty panel, a panel with no candidates, a list without a highlight — is
/// consistent and passes.
fn is_consistent(panel: &PanelUpdate<'_>) -> bool {
    if count_fields(panel.candidates) != panel.candidate_count as usize {
        return false;
    }
    match u32::try_from(panel.cursor_index) {
        Ok(index) => index < panel.candidate_count,
        // -1 is the host's "nothing is highlighted"; anything else negative is not.
        Err(_) => panel.cursor_index == -1,
    }
}

/// Number of `'\n'`-separated fields in `candidates`, which is 0 for an empty buffer.
pub(super) fn count_fields(candidates: &str) -> usize {
    if candidates.is_empty() {
        return 0;
    }
    candidates.bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// Stores a validated panel as the new mirror, reusing the slot's buffers.
fn store_panel(panel: PanelUpdate<'_>) {
    let mut slot = lock_panel();
    slot.get_or_insert_with(PanelMirror::default).refill(panel);
}

/// The panel mirror, or `None` before the host has described a panel.
pub fn panel_mirror() -> Option<PanelMirror> {
    lock_panel().as_ref().cloned()
}

/// The panel the host last described.
static PANEL_MIRROR: Mutex<Option<PanelMirror>> = Mutex::new(None);

/// Borrows the panel slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The mirror it guards is independent of whatever
/// that was, and the alternative — treating the panel as permanently absent — would
/// silently disable the fallback for the rest of the session.
fn lock_panel() -> MutexGuard<'static, Option<PanelMirror>> {
    match PANEL_MIRROR.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}
