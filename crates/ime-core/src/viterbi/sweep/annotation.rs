//! The result: the ordered drafts written into the frozen `DecodeResult`, and the
//! annotation each candidate carries.
//!
//! Responsibility: turn the finished draft list into the candidate list the contract
//! defines -- numbered, text-owned, display-scored -- and compose the annotation, which
//! is where the wrong-sound and wrong-word hints of the hint channel live.
//!
//! Boundaries: this module decides nothing about the order. The list arrives ordered;
//! every field is written through the slot the last decode already owns, which is what
//! keeps a steady-state decode allocation-free apart from the hints themselves.
//!
//! # The annotation's composition order
//!
//! The hints come first and the dictionary labels last, joined with the separator:
//! `音:` with the standard reading of a widened edge, then `纠错:` with the spelling the
//! user typed, then `字:→` with the correct form of a confusable hit. An English
//! candidate carries only its own label -- its text cannot hit a confusable key and its
//! edge exists only where no widened edge did.

use ime_types::{Candidate, CandidateSource, DecodeResult};

use crate::confusable::{CONFUSABLE_ANNOTATION_PREFIX, ConfusableTable};
use crate::lm::Q;
use crate::segment::correct::CORRECTION_ANNOTATION_PREFIX;

use super::draft::Draft;

/// The annotation prefix a wrong-sound hint carries, ahead of the standard reading.
///
/// User-facing text is Chinese by the project's language convention. The hint says
/// what the word the candidate spells is read as in standard pinyin -- the reading the
/// widened edge looked its word up under, not the spelling the user typed.
pub(crate) const READING_ANNOTATION_PREFIX: &str = "音:";

/// The separator two parts of one annotation are joined with.
const ANNOTATION_SEPARATOR: &str = "｜";

/// The Q16.16 unit as a float, for the display-only candidate score.
const Q16_ONE: f32 = (Q * Q) as f32;

/// Turns the ordered drafts into the frozen result, reusing the buffers `out` already holds.
///
/// Every candidate is written through the slot it already occupies, which is what keeps the
/// text buffers: a decode that produced the same number of candidates as the last one
/// allocates nothing here, because each text is overwritten in place rather than replaced.
///
/// A candidate whose path used a widened edge is annotated: `音:` with the standard
/// reading, `纠错:` with the typed spelling when the edge was a correction, and `字:→`
/// with the correct form when `hints` is attached and the text is one of its wrong
/// forms. A candidate with no hint of its own is annotated with nothing, and borrows
/// nothing.
///
/// # Panics
///
/// Never: every slot is taken with a checked lookup and every arithmetic step
/// saturates.
pub(crate) fn result_of_into(
    out: &mut DecodeResult,
    drafts: &[Draft],
    degraded: bool,
    normalized: &str,
    hints: Option<&ConfusableTable>,
) {
    for (position, draft) in drafts.iter().enumerate() {
        // The display number is the position, 1-based: the window draws it next to the
        // number key that selects the candidate.
        let index = u16::try_from(position)
            .unwrap_or(u16::MAX)
            .saturating_add(1);
        // The one float in the pipeline, written last and read by nobody: a Q16.16
        // score turned into the display score the window may show.
        let score = draft.score as f32 / Q16_ONE;
        match out.candidates.get_mut(position) {
            Some(slot) => {
                slot.index = index;
                slot.text.clear();
                slot.text.push_str(&draft.text);
                slot.annotation = annotation_of(draft, normalized, hints);
                slot.source = draft.source;
                slot.score = score;
                slot.consumed_syllables = draft.syllables;
            }
            None => out.candidates.push(Candidate {
                index,
                text: draft.text.clone(),
                annotation: annotation_of(draft, normalized, hints),
                source: draft.source,
                score,
                consumed_syllables: draft.syllables,
            }),
        }
    }
    // The list is cut to what this decode produced. A longer previous list gives its surplus
    // candidates up, text buffers included: the contract says the list is exactly what the
    // decode found.
    out.candidates.truncate(drafts.len());
    out.degraded = degraded;
}

/// Composes the annotation one draft carries, or `None` when it carries no hint.
///
/// The buffer of an existing slot is reused, so a steady-state decode that keeps its
/// hints reallocates nothing after the first; the allocation happens once, the first
/// time a slot carries an annotation at all.
fn annotation_of(
    draft: &Draft,
    normalized: &str,
    hints: Option<&ConfusableTable>,
) -> Option<String> {
    if draft.source == CandidateSource::English {
        // The word came from the English lexicon, and the label is what tells the
        // user so: it is the slot the phrase table's label fills. An English
        // candidate is never widened -- its edge exists only where correction found
        // nothing to widen -- so the two labels never compete for the slot.
        return Some(String::from(crate::segment::english::ENGLISH_ANNOTATION));
    }
    if !draft.corrected && draft.readings.is_empty() {
        // A candidate with no engine hint of its own can still be one of the
        // table's wrong forms, so the lookup runs before anything is allocated:
        // a miss costs one binary search and no string, and a hit is the whole
        // annotation.
        return match hints.and_then(|table| table.lookup(&draft.text)) {
            None => None,
            Some(correct) => Some(format!("{CONFUSABLE_ANNOTATION_PREFIX}{correct}")),
        };
    }
    let mut text = String::new();
    write_hinted_annotation(&mut text, draft, normalized, hints);
    Some(text)
}

/// Fills `text` with the hints one candidate carries, cleared first.
///
/// The order is the channel's own: `音:` with the standard reading of the widened
/// edges, then `纠错:` with the spelling the user typed, then `字:→` with the correct
/// form the confusable table holds for the text -- parts joined with the separator.
/// The confusable lookup runs only when a table is attached, and a miss appends
/// nothing, so a decode whose candidates hit nothing pays one binary search per
/// candidate and no allocation.
fn write_hinted_annotation(
    text: &mut String,
    draft: &Draft,
    normalized: &str,
    hints: Option<&ConfusableTable>,
) {
    text.clear();
    if !draft.readings.is_empty() {
        text.push_str(READING_ANNOTATION_PREFIX);
        text.push_str(&draft.readings);
    }
    if draft.corrected {
        if !text.is_empty() {
            text.push_str(ANNOTATION_SEPARATOR);
        }
        text.push_str(CORRECTION_ANNOTATION_PREFIX);
        text.push_str(normalized);
    }
    if let Some(table) = hints {
        if let Some(correct) = table.lookup(&draft.text) {
            if !text.is_empty() {
                text.push_str(ANNOTATION_SEPARATOR);
            }
            text.push_str(CONFUSABLE_ANNOTATION_PREFIX);
            text.push_str(correct);
        }
    }
}

/// Appends the standard reading one widened edge's word carries to the hint's buffer.
///
/// The reading is the canonical spelling the lattice walk looked the word up under,
/// held in the build's arena as one whole slice and named by the range the lattice
/// recorded for the edge. A range the arena does not name, one that is not UTF-8 --
/// which the build cannot produce, since the arena is spelled from syllable text --
/// and an empty range all append nothing: a hint is best-effort annotation and never
/// a failure.
pub(crate) fn append_standard_reading(readings: &mut String, keys: &[u8], key: (u32, u16)) {
    let start = usize::try_from(key.0).unwrap_or(usize::MAX);
    let Some(bytes) = keys
        .get(start..)
        .and_then(|rest| rest.get(..usize::from(key.1)))
    else {
        return;
    };
    let Ok(spelling) = std::str::from_utf8(bytes) else {
        return;
    };
    if spelling.is_empty() {
        return;
    }
    // Two widened words in one path read as two readings, joined the way the keys
    // themselves are joined.
    if !readings.is_empty() {
        readings.push('\'');
    }
    readings.push_str(spelling);
}
