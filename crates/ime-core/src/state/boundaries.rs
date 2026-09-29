//! Translating the segmentation's syllable grid onto the raw input.
//!
//! Responsibility: turn the node list `SyllableDag::best_segmentation_hint` reports
//! into byte offsets of the input the user typed, which is the form
//! `InputBuffer::set_boundaries` speaks.
//!
//! Boundaries: pure string arithmetic. It reaches the segmentation layer's own
//! normalizer -- the same function the graph itself was built with -- and nothing
//! else: no dictionary, no clock, no environment, no global state.
//!
//! # Why a translation is needed at all
//!
//! The graph is built over the *normalized* input and reports node offsets in it,
//! while the buffer speaks offsets into the bytes the user pressed. The two agree for
//! most inputs and differ wherever normalization is not length preserving:
//!
//! - `v` becomes the two-byte `ü`, so `lv` is two bytes of input and three of
//!   normalized text.
//! - An apostrophe that carries no information -- leading, trailing, or doubled -- is
//!   folded away, so `ni''hao` is eight bytes of input and six of normalized text.
//!
//! Handing a normalized offset to the buffer would either be rejected outright (it
//! does not describe the input) or, worse, describe a grid in which one Backspace
//! removes text the user did not point at.
//!
//! # How the map is built
//!
//! Every prefix of the raw input is re-normalized with the segmentation layer's own
//! `normalize_into`, which gives the normalized length of each prefix. That length is
//! non-decreasing in the prefix, so the raw offset a node names is the first prefix
//! whose normalized form reaches the node's offset. Reusing the normalizer rather
//! than restating its rules is what keeps the map from drifting away from them.
//!
//! The cost is quadratic in the length of the input, which is bounded at 64 bytes and
//! amounts to a few kilobytes of copying per keystroke -- far inside the step budget,
//! and the buffers are reused across the prefixes of one call.

use smallvec::SmallVec;

use crate::segment::{DroppedChars, HINT_INLINE_BOUNDARIES};

/// Translates the segmentation's node list into offsets into the raw input.
///
/// # Parameters
///
/// - `raw`: the input as the user typed it, the buffer's own string.
/// - `hint`: the node list the segmentation reported, in offsets of the *normalized*
///   input. It must start at `0`, which every hint does.
/// - `out`: cleared and filled with the grid, in offsets of `raw`.
///
/// # Returns
///
/// `true` when `out` holds a usable grid for `raw`: it starts at `0`, ends at the end
/// of the input, and is strictly increasing. `false` when it does not, which is what
/// a trailing folded apostrophe produces -- the last node then maps to a position
/// short of the end of the input. The caller leaves the buffer's previous grid in
/// place in that case, which costs syllable-wise Backspace for the rest of the
/// session and is still far better than a grid that makes one Backspace delete
/// everything.
///
/// # Panics
///
/// Never panics.
pub(super) fn map_to_raw(
    raw: &str,
    hint: &[u16],
    out: &mut SmallVec<[u16; HINT_INLINE_BOUNDARIES]>,
) -> bool {
    out.clear();
    if hint.first().copied() != Some(0) {
        return false;
    }
    let mut scratch = String::new();
    let mut dropped = DroppedChars::default();
    let mut next = 0usize;
    let boundaries = raw
        .char_indices()
        .map(|(at, _)| at)
        .chain(core::iter::once(raw.len()));
    for at in boundaries {
        let Ok(reached) = u16::try_from(normalize_prefix(raw, at, &mut scratch, &mut dropped))
        else {
            return false;
        };
        while hint.get(next).is_some_and(|node| *node <= reached) {
            out.push(u16::try_from(at).unwrap_or(u16::MAX));
            next += 1;
        }
    }
    if next != hint.len() {
        return false;
    }
    let last = u16::try_from(raw.len()).unwrap_or(u16::MAX);
    let describes_the_input = out.first().copied() == Some(0) && out.last().copied() == Some(last);
    let strictly_increasing = out.windows(2).all(|pair| pair[0] < pair[1]);
    describes_the_input && strictly_increasing
}

/// Returns the length of the normalized form of `raw[..at]`.
///
/// `scratch` and `dropped` are reused across the calls of one mapping, so building
/// the map allocates nothing after the first prefix.
fn normalize_prefix(
    raw: &str,
    at: usize,
    scratch: &mut String,
    dropped: &mut DroppedChars,
) -> usize {
    let prefix = raw.get(..at).unwrap_or(raw);
    crate::segment::syllable::normalize_into(prefix, scratch, dropped);
    scratch.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::SyllableDag;

    /// Returns the grid the hint of `raw` maps to, or `None` when the map refuses it.
    fn grid_for(raw: &str) -> Option<Vec<u16>> {
        let mut dag = SyllableDag::new();
        assert!(dag.build(raw).is_ok(), "building {raw:?}");
        let mut hint = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(dag.best_segmentation_hint(&mut hint), "hinting {raw:?}");
        let mut out = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        map_to_raw(raw, &hint, &mut out).then(|| out.to_vec())
    }

    #[test]
    fn test_map_to_raw_is_the_identity_when_normalization_keeps_the_length() {
        let cases: [(&str, &[u16]); 3] = [
            ("nihao", &[0, 2, 5]),
            ("nihaoa", &[0, 2, 5, 6]),
            ("zhongguo", &[0, 5, 8]),
        ];
        for (raw, expected) in cases {
            assert_eq!(grid_for(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn test_map_to_raw_shortens_the_grid_when_a_letter_grows() {
        // `lv` is two bytes of input and three of normalized text, so the node at
        // three names the end of the input and not a position past it.
        assert_eq!(grid_for("lv").as_deref(), Some(&[0, 2][..]));
        assert_eq!(grid_for("nvn").as_deref(), Some(&[0, 2, 3][..]));
        // After `j`, `q`, `x` and `y` the umlaut is written `u`, so nothing grows.
        assert_eq!(grid_for("ju").as_deref(), Some(&[0, 2][..]));
    }

    #[test]
    fn test_map_to_raw_keeps_the_boundary_after_a_marker() {
        // The marker belongs to the syllable before it, so the node after it lands
        // just past the marker in the raw input as well.
        assert_eq!(grid_for("ni'hao").as_deref(), Some(&[0, 4, 6][..]));
    }

    #[test]
    fn test_map_to_raw_refuses_a_grid_that_does_not_reach_the_end() {
        // A trailing marker is folded away by normalization, so the last node maps to
        // a position short of the end of the input. The grid is refused rather than
        // adopted, because adopting it would make one Backspace delete everything.
        assert_eq!(grid_for("ni'"), None);
    }

    #[test]
    fn test_map_to_raw_refuses_a_hint_that_does_not_start_at_zero() {
        let mut out = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(!map_to_raw("ni", &[1, 2], &mut out));
    }

    #[test]
    fn test_map_to_raw_accepts_the_empty_hint_of_an_empty_input() {
        let mut out = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(map_to_raw("", &[0], &mut out));
        assert_eq!(out.as_slice(), &[0]);
    }
}
