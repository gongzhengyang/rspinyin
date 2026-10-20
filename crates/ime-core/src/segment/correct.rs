//! Spelling correction: the keyboard-neighbour table, the one-step corrections of
//! one spelling, and the penalty a correction edge carries.
//!
//! Responsibility: answer, for one spelling of the input, which legal syllables a
//! single typing slip away from it spells -- a neighbour substitution (`eo` for
//! `wo`) or an adjacent transposition (`zhegn` for `zheng`) -- and carry the score
//! penalty that keeps a corrected reading far below anything the input spells
//! exactly.
//!
//! Boundaries: this is the correction algebra and nothing else. It does not read
//! the input buffer, does not walk the segmentation graph and never looks a word
//! up: a caller hands it a spelling and gets syllable identifiers back. It is a
//! pure function of its arguments -- no file, no clock, no environment, no global
//! state -- which is what keeps the decoder's determinism guarantee intact (0.4
//! rule 4). The full expansion is computed in a fixed stack buffer, so a caller
//! can ask on the decode path without touching the allocator.
//!
//! # Why the gate is the caller's
//!
//! A correction edge is only ever wanted where the exact query missed: the DAG
//! adds them when, and only when, its exact-and-fuzzy build has no complete path.
//! On an input that segments exactly this module is never called at all, which is
//! the structural guarantee that a correctly typed input decodes -- and ranks --
//! byte-identically with the switch on or off. The [`is_enabled`] test here is the
//! flag half of that gate; the miss half lives in [`SyllableDag::build_for`].
//!
//! [`SyllableDag::build_for`]: crate::segment::SyllableDag::build_for
//!
//! # Why one step, and why these two edits
//!
//! The corrections model where a finger lands wrong, not what a user misspells:
//! a neighbour substitution and an adjacent transposition are the two edits a
//! slipping finger produces, and both preserve length, which is why a span longer
//! than the longest syllable can never correct into one and is answered with an
//! empty set outright. Deletion and insertion edits model a different mistake --
//! doubling or dropping a letter -- and are librime's own edit-distance corrector,
//! which upstream disabled behind `#if 0`: its false-positive rate was not worth
//! the candidates it rescued. This module ships the two edits and not that one.
//!
//! # Determinism
//!
//! The order of the returned set is fixed by the left-to-right walk over the
//! spelling and by [`NEIGHBORS`]'s row order, never by a hash: a candidate order
//! that depended on a set's iteration order would not be reproducible across
//! processes. Distinct edits of one spelling always produce distinct results --
//! a substitution differs from the input at exactly one byte and a transposition
//! of two different letters at exactly two -- so the set holds no duplicates by
//! construction, and a property test keeps that argument honest.

use ime_types::{DecodeFlags, SyllableId};
use smallvec::SmallVec;

use crate::segment::syllable::{MAX_SYLLABLE_LEN, lookup};

/// Most corrections one spelling may reach.
///
/// The cap is [`librime`'s `max_corrections`]: a wider set costs every span of
/// every miss input and answers words the reader almost never meant. It is
/// reached by the early return in [`corrections_into`], in table order, so which
/// corrections are dropped is the same on every run.
///
/// [`librime`'s `max_corrections`]: https://github.com/rime/librime
pub const MAX_CORRECTIONS: usize = 4;

/// The score penalty one correction edge carries, in the Q16.16 unit the sweep
/// ranks with.
///
/// One step costs about 3.9 units of log probability -- librime's `-log(0.01)`
/// rounded onto the fixed-point grid, spelled `998 << 8` to keep the shifted
/// shape of its sibling penalties. That is roughly eighty times the fuzzy
/// penalty: a corrected reading loses to anything the input spells, including
/// every fuzzy and abbreviated reading of the same input, and only surfaces
/// where nothing spelled-correct competes -- which, because correction expands
/// only where the exact query missed, is every input it appears on at all.
pub const CORRECTION_PENALTY_Q8: i32 = 998 << 8;

// The penalties are a ladder -- an abbreviation outranks a fuzzy match, a fuzzy
// match outranks a correction -- and the ladder is what keeps a wider tolerance
// from displacing a narrower one at equal dictionary weight. Asserted here rather
// than in a test so that reordering the ladder is a compile error, like the
// other constant invariants of this workspace.
const _: () = assert!(CORRECTION_PENALTY_Q8 > crate::fuzzy::FUZZY_PENALTY_Q8);

/// The annotation a corrected candidate carries, ahead of the spelling the user
/// typed.
///
/// The annotation channel is how a candidate says why it is there: this prefix and
/// the input as normalized are what a corrected candidate is annotated with, so the
/// window can show the original spelling a candidate was reached past and a reader
/// of the channel can tell a corrected reading from an exact one. User-facing copy,
/// so it is Chinese like the rest of the window's text.
pub const CORRECTION_ANNOTATION_PREFIX: &str = "纠错：";

// QWERTY adjacency, one row per letter in alphabet order: the neighbours a
// finger can slip to from each key, up-row and down-row and along the row. The
// table is data, not control flow -- the same discipline as the key router's
// translation tables -- and it is kept symmetric (every link appears in both
// rows) with a property test standing guard.
// `#[rustfmt::skip]`: this is a data table, and one key per line keeps it
// reviewable against a keyboard picture.
#[rustfmt::skip]
pub(crate) static NEIGHBORS: [&[u8]; 26] = [
    b"qwsz", // a
    b"vghn", // b
    b"xdfv", // c
    b"serfcx", // d
    b"wsdr", // e
    b"drtgcv", // f
    b"ftyhbv", // g
    b"gyujbn", // h
    b"ujko", // i
    b"huiknm", // j
    b"jiolm", // k
    b"kop", // l
    b"njk", // m
    b"bhjm", // n
    b"iklp", // o
    b"ol", // p
    b"aw", // q
    b"edft", // r
    b"awedzx", // s
    b"rfgy", // t
    b"yhji", // u
    b"cfgb", // v
    b"qase", // w
    b"zscd", // x
    b"tghu", // y
    b"axs", // z
];

/// Returns the neighbour row of one ASCII letter, or `None` for a byte that is
/// not a lower-case ASCII letter.
///
/// The normalized input spells the non-ASCII syllables with two-byte characters
/// (`ü`, `ê`), whose bytes are never letters; the `None` here is what keeps a
/// substitution from editing inside a multi-byte character.
fn neighbor_row(byte: u8) -> Option<&'static [u8]> {
    if byte.is_ascii_lowercase() {
        Some(NEIGHBORS[usize::from(byte - b'a')])
    } else {
        None
    }
}

/// Whether spelling correction is switched on for `flags`.
///
/// The [`ime_types::DecodeFlags::CORRECTION`] bit is the whole switch: unlike
/// fuzzy matching there is no master bit beside it and no per-class bits under
/// it, so there is no second condition to keep in step.
///
/// # Errors
///
/// Never.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::segment::correct::is_enabled;
/// use ime_types::DecodeFlags;
///
/// assert!(!is_enabled(DecodeFlags::empty()));
/// assert!(!is_enabled(DecodeFlags::default()));
/// assert!(is_enabled(DecodeFlags::CORRECTION));
/// ```
pub fn is_enabled(flags: DecodeFlags) -> bool {
    flags.contains(DecodeFlags::CORRECTION)
}

/// The corrections one spelling reaches: legal syllables at most
/// [`MAX_CORRECTIONS`] wide, one edit away.
///
/// A [`SmallVec`] rather than a `Vec`: the set is bounded by
/// [`MAX_CORRECTIONS`] and every element is inline, so the whole expansion stays
/// in the caller's frame instead of going through the allocator.
pub type CorrectionSet = SmallVec<[SyllableId; MAX_CORRECTIONS]>;

/// Returns the legal syllables one edit away from `text`, table order.
///
/// `text` is a spelling of the input -- typically one a `lookup` of the input
/// itself has already refused, though the function is total and answers a legal
/// syllable too. Every element is a syllable the table holds; a spelling longer
/// than the longest syllable, or one that reaches no legal syllable, answers an
/// empty set.
///
/// # Errors
///
/// Never: the expansion is total, and a correction that lands on no syllable of
/// the table is skipped rather than reported.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::segment::correct::corrections_of;
/// use ime_core::segment::{lookup, syllable_at};
///
/// let spelled: Vec<&str> = corrections_of("zhegn")
///     .iter()
///     .filter_map(|id| syllable_at(*id))
///     .collect();
/// assert!(spelled.contains(&"zheng"), "the transposition is found: {spelled:?}");
/// // A spelling longer than the longest syllable cannot correct into one.
/// assert!(corrections_of("zhuangx").is_empty());
/// ```
pub fn corrections_of(text: &str) -> CorrectionSet {
    let mut out = CorrectionSet::new();
    corrections_into(&mut out, text);
    out
}

/// Fills `out` with the legal syllables one edit away from `text`, clearing it
/// first.
///
/// `out` is cleared before it is filled, so a caller that keeps one set across
/// spans never allocates: [`MAX_CORRECTIONS`] is the inline capacity and the
/// expansion stops there, so the set never spills to the heap.
///
/// # Errors
///
/// Never.
///
/// # Panics
///
/// Never.
pub fn corrections_into(out: &mut CorrectionSet, text: &str) {
    out.clear();
    let bytes = text.as_bytes();
    // Both edits preserve length, so a spelling longer than the longest syllable
    // the table holds can never correct into one; answered without touching the
    // buffer, which keeps a stray long span from paying for the walk.
    if bytes.is_empty() || bytes.len() > MAX_SYLLABLE_LEN {
        return;
    }
    let mut buffer = [0u8; MAX_SYLLABLE_LEN];
    // Neighbour substitutions, left to right, neighbours in table order.
    for (position, &byte) in bytes.iter().enumerate() {
        let Some(row) = neighbor_row(byte) else {
            continue;
        };
        for &neighbor in row {
            buffer[..bytes.len()].copy_from_slice(bytes);
            buffer[position] = neighbor;
            if push_hit(out, &buffer[..bytes.len()]) {
                return;
            }
        }
    }
    // Adjacent transpositions, left to right. A pair of equal letters transposes
    // to the spelling it started from, which the exact lookup already refused, so
    // the pair is skipped rather than looked up for nothing. Both bytes have to be
    // letters: a two-byte character beside an ASCII one must never be split.
    for (position, window) in bytes.windows(2).enumerate() {
        if window[0] == window[1]
            || neighbor_row(window[0]).is_none()
            || neighbor_row(window[1]).is_none()
        {
            continue;
        }
        buffer[..bytes.len()].copy_from_slice(bytes);
        buffer.swap(position, position + 1);
        if push_hit(out, &buffer[..bytes.len()]) {
            return;
        }
    }
}

/// Looks `spelled` up and pushes the syllable it names, answering whether
/// [`MAX_CORRECTIONS`] has cut the expansion short.
fn push_hit(out: &mut CorrectionSet, spelled: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(spelled) else {
        return false;
    };
    match lookup(text) {
        Some(syllable) if out.len() < MAX_CORRECTIONS => {
            out.push(syllable);
            out.len() >= MAX_CORRECTIONS
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
