//! The small edits a session makes to a decode result and to the input buffer.
//!
//! Responsibility: emptying a result in place without losing its buffers, renumbering a
//! candidate list after one of its entries is gone, and writing a syllable grid back onto
//! the raw input.
//!
//! Boundaries: free functions over values the caller owns. They hold no session and read
//! nothing outside their parameters.

use smallvec::SmallVec;

use ime_types::{Candidate, DecodeResult, Preedit};

use crate::input::InputBuffer;
use crate::segment::{HINT_INLINE_BOUNDARIES, SyllableDag};
use crate::state::boundaries::map_to_raw;

/// Empties a result in place, keeping the buffers it holds.
///
/// The same value [`empty_result`] builds from nothing, for a caller that already has a
/// result: the lists lose their entries, and the capacity they were sized to stays for
/// the next decode to write into.
pub(super) fn clear_result(result: &mut DecodeResult) {
    result.candidates.clear();
    result.segments.clear();
    result.degraded = false;
}

/// Rewrites the display numbers of a candidate list so that each one is its position plus
/// one.
///
/// That is the numbering a decode writes and the numbering the window draws against: the
/// number key that selects a candidate and the index a click names are both read from it, so
/// a list that lost a candidate in the middle must not go on handing out the number the
/// removed word had.
pub(super) fn renumber(candidates: &mut [Candidate]) {
    for (position, candidate) in candidates.iter_mut().enumerate() {
        candidate.index = u16::try_from(position)
            .unwrap_or(u16::MAX)
            .saturating_add(1);
    }
}

/// Writes the syllable grid of `dag` into `buf`, so that a Backspace removes a syllable.
///
/// The grid is written back only when the graph has a segmentation at all. A graph
/// without one reports the whole input as a single pass-through range, and adopting that
/// would make one Backspace delete everything the user typed; the buffer keeps the grid
/// it already had instead.
///
/// The buffer and the graph are separate arguments rather than two fields of a session, so
/// that the caller decides which graph the grid is derived from.
pub(super) fn write_boundaries(buf: &mut InputBuffer, dag: &SyllableDag) {
    let mut hint = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !dag.best_segmentation_hint(&mut hint) {
        return;
    }
    let raw = buf.raw();
    let mut grid = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !map_to_raw(raw, &hint, &mut grid) {
        return;
    }
    buf.set_boundaries(&grid);
}

/// Returns the preedit of a session that has composed nothing.
pub(super) fn empty_preedit() -> Preedit {
    Preedit {
        text: String::new(),
        caret: 0,
        spans: Vec::new(),
    }
}
