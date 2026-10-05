//! The preedit line, as the header draws it.
//!
//! Responsibility: turn one [`Preedit`] into the runs `ui/candidate.slint` draws -- cut to
//! the room the header has left, split around the caret and stripped of the cursor marker.
//! It is the whole of the preedit's layout: the component draws what it is handed and
//! decides nothing, which is what keeps the render path a pure function of the frame.
//!
//! # The cut
//!
//! 3.1.3 cuts a preedit that does not fit from the *left*, keeping the newest input: the
//! user is typing the tail, so the head is the half that can be spared. The packing is
//! right-aligned -- runs are kept from the tail until the budget is spent, the one run the
//! budget runs out in is cut at a character, and everything left of it is dropped whole --
//! so what survives is always a suffix of the input and the newest key is the last thing
//! the cut can take. The cut run carries the ellipsis as a *prefix*, baked into its text
//! here: the component draws finished runs and branches on nothing.
//!
//! The cut is computed rather than measured, because this layer has to run with no display
//! server and no live component -- it uses the same deterministic estimate the candidate
//! cells use ([`Measure`]) and the component's `overflow: elide` stays the last-resort
//! guard.
//!
//! # The caret
//!
//! `ime_types::Preedit::spans` carries exactly one zero-width [`SpanKind::Cursor`] span, in
//! its sorted position. It is deliberately *not* a run: a zero-width run would take a slot
//! in the layout that the caret has to share with the text on either side of it. The run
//! list is split at the caret's byte offset instead, which is what lets a caret in the
//! middle of the string be placed without measuring anything on the drawing side.
//!
//! # Steady state
//!
//! The runs are written into the buffers the layout already holds, so a keystroke that
//! changes one syllable writes one string rather than rebuilding the list, and a frame that
//! changes nothing reports nothing.

#[cfg(test)]
mod golden;
#[cfg(test)]
mod tests;

use ime_types::{Preedit, PreeditSpan, SpanKind};

use super::cell::{ELLIPSIS, Measure, character_em, ellipsis_width, replace, write_text};

/// The longest preedit the adapter hands to the component, in characters.
///
/// A hard bound rather than a measurement, and a second one beside the width: the preedit
/// is one line of the header and the widest panel holds roughly fifty glyphs at the header
/// font size, so sixty-four characters is more than any container can draw and the string
/// the component is handed stays bounded however long a composing session runs. The width
/// cut below is what normally binds; this one only answers input the estimator would let
/// through.
pub const PREEDIT_MAX_CHARS: usize = 64;

/// The narrowest preedit the header keeps before it drops the status cluster's two
/// secondary markers.
///
/// Below this the preedit is too small to read a syllable in, so the markers that carry
/// "occasionally interesting" -- full-width and Chinese punctuation -- give their room back
/// to it and the cluster keeps only what the user has to know.
pub const MIN_PREEDIT_WIDTH_DP: f32 = 48.0;

/// What the status cluster's two secondary markers cost when they are drawn.
///
/// `2 * header-icon-size + header-icon-gap` (3.1.1), which is exactly the room the preedit
/// gains when they are dropped: the cluster's width and this number are two halves of one
/// decision, and the test below pins this one against the component's own constants. The
/// value is the two markers' own width rather than the cluster's whole reflow -- the strip
/// also folds one gap away when they go, and that gap is slack between the preedit and the
/// cluster, not room the preedit claims.
pub const SECONDARY_STATUS_WIDTH_DP: f32 = 40.0;

/// What one run of the preedit represents.
///
/// The cursor is deliberately absent; see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// A syllable of the reading.
    Syllable,
    /// The `'` between two syllables.
    Separator,
    /// Input that is not pinyin, shown as it was typed.
    Passthrough,
}

impl RunKind {
    /// The run's kind, or `None` for the cursor marker, which is not a run.
    ///
    /// # Parameters
    ///
    /// * `kind` -- the contract's span kind.
    ///
    /// # Returns
    ///
    /// The run kind, or `None` when the span carries no text.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn of(kind: SpanKind) -> Option<Self> {
        match kind {
            SpanKind::Syllable => Some(Self::Syllable),
            SpanKind::Separator => Some(Self::Separator),
            SpanKind::Passthrough => Some(Self::Passthrough),
            SpanKind::Cursor => None,
        }
    }

    /// The wire value `ui/candidate.slint` reads this kind as.
    ///
    /// A `.slint` source has no enum of its own, so the variant travels as its own number:
    /// the mapping is written out rather than derived from the discriminant, which makes a
    /// variant added to the contract a compile error here instead of a silently renumbered
    /// run.
    ///
    /// # Returns
    ///
    /// `0` for a syllable, `1` for a separator, `2` for a passthrough run.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn code(self) -> i32 {
        match self {
            Self::Syllable => 0,
            Self::Separator => 1,
            Self::Passthrough => 2,
        }
    }
}

/// One run of the preedit, as the header draws it.
///
/// A plain value with no Slint in it, so the layout can be asserted on without a Slint
/// platform, and the text is kept across frames so a redrawn preedit reuses the buffers it
/// already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreeditRun {
    /// The run's text: a slice of `Preedit::text`, never a reformatted one.
    pub text: String,
    /// What the run represents, which is what styles it.
    pub kind: RunKind,
}

/// The preedit cut to a budget and split around the caret.
///
/// The two run lists are the header's whole input: it draws `before`, the caret and then
/// `after`, and branches on nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct PreeditLayout {
    /// The runs left of the caret, in reading order.
    pub before: Vec<PreeditRun>,
    /// The runs right of the caret, in reading order.
    pub after: Vec<PreeditRun>,
    /// Whether the caret is drawn at all.
    pub caret_visible: bool,
    /// Whether a prefix was dropped to make the preedit fit.
    pub truncated: bool,
    /// The drawn run the head ellipsis was baked into: its index in the drawn run
    /// sequence -- `before` followed by `after`, the caret slot taking no index.
    ///
    /// Right-aligned packing only ever cuts the leftmost run it keeps, so the value is
    /// `Some(0)` whenever a mark is drawn -- the mark is always the drawn preedit's first
    /// run -- and `None` when the preedit fits whole, or was cut so hard that nothing is
    /// left to carry it. The mark itself travels inside the run's text; this field is the
    /// layout's own record that it did.
    pub head_cut_run: Option<usize>,
    /// Whether the status cluster's two secondary markers are drawn.
    pub show_secondary_status: bool,
}

impl Default for PreeditLayout {
    /// The layout of a session that has not composed anything.
    ///
    /// `show_secondary_status` starts true rather than false, and it has to start at the
    /// value `ui/candidate.slint` declares: the change detection compares a frame against
    /// the state the component is already drawing, so a frame that leaves the markers on
    /// writes nothing -- and a component whose own default was the other way round would
    /// then draw them dropped until a frame happened to turn them off and on again.
    fn default() -> Self {
        Self {
            before: Vec::new(),
            after: Vec::new(),
            caret_visible: false,
            truncated: false,
            head_cut_run: None,
            show_secondary_status: true,
        }
    }
}

impl PreeditLayout {
    /// Recomputes this layout from `preedit`, reusing the buffers it already holds.
    ///
    /// # Parameters
    ///
    /// * `preedit` -- the frame's preedit line.
    /// * `available_dp` -- the width the preedit has in the header, with the status
    ///   cluster's two secondary markers still drawn. The layout decides whether they are
    ///   worth their room, so the caller does not have to.
    /// * `font_size_dp` -- the size the preedit is drawn at, from
    ///   [`crate::layout::Metrics::font_size_header`].
    /// * `measure` -- the width estimator, kept across frames so a span is measured once
    ///   rather than once per keystroke.
    ///
    /// # Returns
    ///
    /// Whether anything the header draws changed. Every field of a frame that draws what
    /// the layout already holds reports `false`.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A span list that does not add
    /// up to the text, a caret that is not on a character boundary and a budget too small
    /// for anything all degrade to "draw what fits".
    ///
    /// # Panics
    ///
    /// Never panics: every offset is converted with `try_from` and clamped, and every slice
    /// is taken through `str::get`.
    pub fn update(
        &mut self,
        preedit: &Preedit,
        available_dp: f32,
        font_size_dp: f32,
        measure: &mut Measure,
    ) -> bool {
        let text = preedit.text.as_str();
        // A preedit with text but no spans is one the builder cannot produce -- it tiles every
        // non-empty text -- and it is drawn as a single passthrough run rather than dropped.
        // The header has always shown the text it is handed, and a fallback that drew nothing
        // would turn a malformed frame into what looks like an empty session.
        let fallback = [PreeditSpan {
            start: 0,
            end: u16::try_from(text.len()).unwrap_or(u16::MAX),
            kind: SpanKind::Passthrough,
        }];
        let spans: &[PreeditSpan] = if preedit.spans.is_empty() && !text.is_empty() {
            &fallback[..]
        } else {
            preedit.spans.as_slice()
        };
        let show_secondary = available_dp >= MIN_PREEDIT_WIDTH_DP;
        let budget = if show_secondary {
            available_dp
        } else {
            available_dp + SECONDARY_STATUS_WIDTH_DP
        };
        let kept = kept_start(text, spans, budget, font_size_dp, measure);
        let keep_from = kept.start;
        let caret = usize::try_from(preedit.caret)
            .unwrap_or(usize::MAX)
            .min(text.len());
        // A caret the cut dropped is not on screen, and it is the one case where its exact
        // position cannot be drawn: 3.1.3 spares the tail, and the caret was in the head.
        let caret_visible = !text.is_empty() && caret >= keep_from;
        let split = if caret_visible { caret } else { keep_from };
        // The head mark rides the leftmost drawn run: the runs left of the caret when the
        // cut is left of the caret, the runs right of it otherwise -- which covers both the
        // caret sitting exactly on the cut and the caret the cut dropped.
        let mark_before = kept.marked && split > keep_from;
        let mark_after = kept.marked && split <= keep_from;

        let mut changed = false;
        changed |= write_runs(&mut self.before, text, spans, keep_from, split, mark_before);
        changed |= write_runs(&mut self.after, text, spans, split, text.len(), mark_after);
        changed |= replace(&mut self.caret_visible, caret_visible);
        changed |= replace(&mut self.truncated, keep_from > 0);
        // The record follows what was actually drawn: a mark with no run left to carry it
        // -- a budget that dropped everything -- is recorded as no mark at all.
        let head_marked = if mark_before {
            !self.before.is_empty()
        } else {
            mark_after && !self.after.is_empty()
        };
        changed |= replace(&mut self.head_cut_run, head_marked.then_some(0));
        changed |= replace(&mut self.show_secondary_status, show_secondary);
        changed
    }
}

/// Lays the preedit out for a header with `available_dp` left for it.
///
/// The convenience form of [`PreeditLayout::update`] for a caller that has no layout to
/// reuse; the adapter keeps one and updates it in place.
///
/// # Parameters
///
/// * `preedit` -- the frame's preedit line.
/// * `available_dp` -- the width the preedit has in the header.
/// * `font_size_dp` -- the size it is drawn at.
/// * `measure` -- the width estimator.
///
/// # Returns
///
/// The runs the header draws, split around the caret.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn layout_preedit(
    preedit: &Preedit,
    available_dp: f32,
    font_size_dp: f32,
    measure: &mut Measure,
) -> PreeditLayout {
    let mut layout = PreeditLayout::default();
    layout.update(preedit, available_dp, font_size_dp, measure);
    layout
}

/// Where the drawn preedit starts, and whether its head is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kept {
    /// The byte offset the drawn preedit starts at.
    start: usize,
    /// Whether the leftmost drawn run carries the head ellipsis.
    marked: bool,
}

/// Where the drawn preedit starts, and whether its head is marked.
///
/// The spans tile the text, so walking them from the tail accumulates the newest input
/// first: the walk keeps whole runs while they fit, stops in the first run that does not,
/// and keeps as much of that run's tail as the remainder pays for -- with the ellipsis
/// paid for out of the same remainder before a single character is placed, so a marked cut
/// never draws wider than the budget. Everything left of that run is dropped whole. The
/// character bound of [`PREEDIT_MAX_CHARS`] is applied on top, and when it is the only cut
/// that binds it pays for the mark the same way, so either way the answer is the later of
/// the two cuts.
///
/// One pass over the spans plus one over the characters of the run the cut lands in, so
/// the whole walk is linear in the length of the text.
fn kept_start(
    text: &str,
    spans: &[PreeditSpan],
    budget: f32,
    font_size_dp: f32,
    measure: &mut Measure,
) -> Kept {
    let floor = truncation_start(text, PREEDIT_MAX_CHARS);
    let mark = ellipsis_width(font_size_dp);
    let mut start = text.len();
    let mut used = 0.0f32;
    for span in spans.iter().rev() {
        if RunKind::of(span.kind).is_none() {
            continue;
        }
        let (from, to) = (usize::from(span.start), usize::from(span.end));
        let Some(slice) = text.get(from..to) else {
            continue;
        };
        if slice.is_empty() {
            continue;
        }
        let width = measure.width(slice, font_size_dp);
        if used + width <= budget {
            used += width;
            start = from;
            continue;
        }
        // The budget runs out inside this run: it is the one run that is cut, and the mark
        // is paid for before any character of its tail is placed. A remainder too small
        // for the mark leaves the cut unmarked rather than drawing past the budget -- an
        // unpaid mark would push the line's own newest characters out of the room the
        // header gave it, which is the miscount the cut exists to prevent.
        let room = (budget - used).max(0.0);
        let marked = room >= mark;
        let chars_room = if marked { room - mark } else { room };
        let cut = from + suffix_start(slice, chars_room, font_size_dp);
        return Kept {
            start: cut.max(floor),
            marked,
        };
    }
    // Every run fits the width budget, so the character ceiling is the only cut that can
    // bind. It cuts the same left edge and pays for the mark the same way: out of the room
    // the kept text leaves, deepening the cut when that room is short of one mark.
    let start = start.max(floor);
    if start == 0 {
        return Kept {
            start: 0,
            marked: false,
        };
    }
    let kept = text.get(start..).unwrap_or("");
    if budget - measure.width(kept, font_size_dp) >= mark {
        return Kept {
            start,
            marked: true,
        };
    }
    if budget >= mark {
        return Kept {
            start: start + suffix_start(kept, budget - mark, font_size_dp),
            marked: true,
        };
    }
    Kept {
        start,
        marked: false,
    }
}

/// The offset inside `slice` at which the suffix that fits `room` logical pixels begins.
///
/// A character at a time from the tail, so the answer is always on a character boundary and
/// what is kept is a whole number of characters. The per-character widths come from the
/// same estimator the rest of the layer uses, but are not cached: a cache keyed by a single
/// character would fill up with one-character entries and evict the candidate texts it
/// exists for.
fn suffix_start(slice: &str, room: f32, font_size_dp: f32) -> usize {
    let mut used = 0.0f32;
    let mut start = slice.len();
    for (offset, character) in slice.char_indices().rev() {
        let width = character_em(character) * font_size_dp;
        if used + width > room {
            break;
        }
        used += width;
        start = offset;
    }
    start
}

/// Writes the runs `spans` describes between `from` and `to` into `target`.
///
/// A span that only partly falls inside the range is written as the part that does, which
/// is how the two ends of a cut preedit are drawn as the fragments they are. The first run
/// written carries the head ellipsis when `head_marker` says so: the caller has already
/// reserved the mark's room in the budget, and the mark is what tells the user the drawn
/// line opens a cut. Slots the target already holds are written into rather than replaced,
/// and the target is truncated to what was written, so a shorter preedit keeps its
/// capacity and a longer one grows into it.
///
/// # Returns
///
/// Whether anything the runs draw changed.
fn write_runs(
    target: &mut Vec<PreeditRun>,
    text: &str,
    spans: &[PreeditSpan],
    from: usize,
    to: usize,
    head_marker: bool,
) -> bool {
    let mut written = 0usize;
    let mut changed = false;
    for span in spans {
        let Some(kind) = RunKind::of(span.kind) else {
            continue;
        };
        let start = usize::from(span.start).max(from);
        let end = usize::from(span.end).min(to);
        if start >= end {
            continue;
        }
        let Some(slice) = text.get(start..end) else {
            continue;
        };
        // The mark lands on the first run that ends up drawn, whatever span produced it:
        // a cut that kept no character of the run it landed in marks the whole run beside
        // it, which is still the drawn preedit's left edge.
        let head = head_marker && written == 0;
        match target.get_mut(written) {
            Some(run) => {
                changed |= if head {
                    write_head_cut(&mut run.text, slice)
                } else {
                    write_text(&mut run.text, slice)
                };
                changed |= replace(&mut run.kind, kind);
            }
            None => {
                let mut line = String::new();
                if head {
                    line.push(ELLIPSIS);
                }
                line.push_str(slice);
                target.push(PreeditRun { text: line, kind });
                changed = true;
            }
        }
        written += 1;
    }
    if target.len() > written {
        target.truncate(written);
        changed = true;
    }
    changed
}

/// Writes the head-cut form of `slice` into `target`: the ellipsis of 3.1.3, then the kept
/// text.
///
/// The comparison comes first so a run whose marked text did not change keeps its buffer,
/// the same bargain [`write_text`] strikes for an unmarked one.
fn write_head_cut(target: &mut String, slice: &str) -> bool {
    if target.starts_with(ELLIPSIS) && &target[ELLIPSIS.len_utf8()..] == slice {
        return false;
    }
    target.clear();
    target.push(ELLIPSIS);
    target.push_str(slice);
    true
}

/// The byte offset at which the tail of `text` that fits `max_chars` characters begins.
///
/// Always a character boundary, so the slice it indexes is a valid `str` however the input
/// mixes scripts. A text that already fits keeps all of it.
fn truncation_start(text: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return text.len();
    }
    let characters = text.chars().count();
    if characters <= max_chars {
        return 0;
    }
    // `char_indices` is the only source of a byte offset guaranteed to sit on a character
    // boundary, which is what makes the slice this indexes a valid `str`.
    text.char_indices()
        .nth(characters - max_chars)
        .map_or(0, |(index, _)| index)
}
