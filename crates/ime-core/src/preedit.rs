//! Preedit generation: the input line the candidate window's header draws.
//!
//! Responsibility: turn the input buffer and the segmentation graph into the
//! [`Preedit`] the header renders -- the canonical spelling of what the user typed,
//! cut into syllables at the boundaries of the optimal segmentation, with a
//! zero-width marker where the caret sits.
//!
//! Boundaries: this layer renders, it does not decide. It never segments (the graph
//! arrives built), never scores, and never decides whether the text also reaches the
//! application through `ic->setPreedit` -- that is the engine's `client_preedit`
//! policy. It is a pure function of its two arguments: no file, no clock, no
//! environment, no global state, no display server.
//!
//! # The text
//!
//! `text` is the canonical spelling of the input with a `'` between syllables, not
//! the bytes the user pressed: `nihao` renders as `ni'hao`, and the separator the
//! user did not type is the cut line the header shows. The spelling comes from the
//! graph's normalized string, so the characters normalization discards -- a
//! redundant `'`, or anything outside the input alphabet -- never reach the text. A
//! `'` the user did type is kept as it is and never doubled: it pins the boundary the
//! segmentation would have drawn there anyway, so `ni'hao` renders exactly like
//! `nihao`.
//!
//! What normalization discarded is not lost, it is reported where it was found:
//! [`SyllableDag::dropped_chars`] carries those characters together with their offsets
//! in the raw input, and the engine raises `decode/invalid-char` from there. They are
//! deliberately absent from [`Preedit`], which is the value that crosses to the UI
//! thread: it holds what to draw, and a character that is drawn nowhere has no place in
//! it.
//!
//! When the graph has no path the input is not pinyin at all, and the text is the raw
//! input verbatim as a single [`SpanKind::Passthrough`] span. That is the string the
//! decoder's pass-through candidate commits, so the header and the candidate agree;
//! showing the normalized spelling there would advertise text the commit does not
//! produce.
//!
//! # Invariants
//!
//! Every preedit this module returns satisfies all four of:
//!
//! 1. `spans` is sorted by `start`, and the only spans that may share a start are a
//!    zero-width span and the span that begins at the same offset.
//! 2. The spans of non-zero width tile `[0, text.len())` exactly: no gap, no overlap.
//! 3. Exactly one [`SpanKind::Cursor`] span, of zero width, sits at `caret`. It may
//!    fall inside a syllable, because a caret an edit left inside one is still the
//!    caret.
//! 4. `caret` is a character boundary of `text` and never past its end.
//!
//! Empty input is the one case with no spans at all: there is nothing to cut and no
//! caret to mark.
//!
//! # Size
//!
//! `text` is at most `2 * raw.len() - 1` bytes: every syllable covers at least one
//! input byte and every syllable but the last contributes one separator, and the two
//! cannot both be maximal -- a syllable whose umlaut grows by a byte (`lv` into `lü`)
//! needs two input bytes, so it spends one of them on the growth instead of on a
//! separator. For the 64-byte input limit that is 127 bytes in the worst case, a run
//! of one-byte syllables such as `aaaa...`; a 64-byte input cut into syllables of two
//! bytes or more stays within the 96 bytes the header budgets for. Nothing is
//! truncated here: the header cuts a preedit that is too wide from the left, and
//! dropping input would hide what the user typed.

use ime_types::{Preedit, PreeditSpan, SpanKind};
use smallvec::SmallVec;

use crate::input::InputBuffer;
use crate::segment::{HINT_INLINE_BOUNDARIES, SyllableDag};

/// Builds the preedit of one composing session.
///
/// # Parameters
///
/// - `buf`: the session's input buffer, read for the raw input and the caret. Its
///   syllable grid is not consulted: the cut comes from `dag`.
/// - `dag`: the segmentation graph of the input the buffer holds. The caller builds
///   it from that same input; a graph of a different input renders *that* input,
///   because the graph owns the spelling the text is made of.
///
/// # Returns
///
/// The preedit the header draws: the canonical spelling with a separator at every
/// boundary of the optimal cut, one span per syllable and per separator, and the
/// caret. An empty input answers an empty preedit with no spans.
///
/// # Errors
///
/// None. Input that cannot be segmented is not an error here; it degrades to the
/// pass-through form documented at the top of the module.
///
/// # Panics
///
/// Never: the caret is clamped into the input and onto a character boundary, and
/// every offset is converted with `try_from` rather than assumed to fit.
///
/// # Examples
///
/// ```
/// use ime_core::input::InputBuffer;
/// use ime_core::preedit::build_preedit;
/// use ime_core::segment::SyllableDag;
/// use ime_types::SpanKind;
///
/// let mut buf = InputBuffer::new();
/// for ch in "nihao".chars() {
///     assert!(buf.push_char(ch).is_ok());
/// }
/// let mut dag = SyllableDag::new();
/// assert!(dag.build(buf.raw()).is_ok());
///
/// let preedit = build_preedit(&buf, &dag);
/// // The cut line was never typed, and the caret sits at the end of the text.
/// assert_eq!(preedit.text, "ni'hao");
/// assert_eq!(preedit.caret, 6);
/// assert_eq!(preedit.spans.len(), 4);
/// assert_eq!(preedit.spans[1].kind, SpanKind::Separator);
/// ```
pub fn build_preedit(buf: &InputBuffer, dag: &SyllableDag) -> Preedit {
    let mut preedit = empty_preedit();
    build_preedit_into(buf, dag, &mut preedit);
    preedit
}

/// Fills `out` with the preedit of one composing session, reusing its buffers.
///
/// Identical to [`build_preedit`] in what it produces, except that the text buffer
/// and the span list are cleared and refilled instead of allocated. The preedit is
/// rebuilt on every keystroke, so a caller that keeps one [`Preedit`] across the
/// session -- the session state machine does -- reallocates only when the input
/// outgrows the buffer.
///
/// # Parameters
///
/// - `buf`: the session's input buffer, as for [`build_preedit`].
/// - `dag`: the segmentation graph of the input the buffer holds.
/// - `out`: the preedit to overwrite. Its contents are cleared first, so a stale
///   preedit can never survive a build.
///
/// # Errors
///
/// None, as for [`build_preedit`].
///
/// # Panics
///
/// Never, as for [`build_preedit`].
pub fn build_preedit_into(buf: &InputBuffer, dag: &SyllableDag, out: &mut Preedit) {
    out.text.clear();
    out.spans.clear();
    out.caret = 0;

    let raw = buf.raw();
    if raw.is_empty() {
        return;
    }
    let caret = clamp_to_char_boundary(raw, usize::try_from(buf.caret()).unwrap_or(usize::MAX));

    let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !dag.has_path() || !dag.best_segmentation_hint(&mut boundaries) {
        // Not segmentable, so not pinyin: the header shows the input exactly as
        // typed, which is the text the pass-through candidate commits.
        out.text.push_str(raw);
        push_span(out, 0, raw.len(), SpanKind::Passthrough);
        out.caret = caret_offset(caret);
        insert_cursor_span(out, caret);
        return;
    }

    let target = if caret >= raw.len() {
        CaretTarget::End
    } else {
        CaretTarget::AfterChars(surviving_chars(raw, caret))
    };
    build_segmented(dag, &boundaries, target, out);
}

/// The preedit of an empty input: no text, no spans, the caret at the origin.
fn empty_preedit() -> Preedit {
    Preedit {
        text: String::new(),
        caret: 0,
        spans: Vec::new(),
    }
}

/// Where the composing caret sits, in the terms the builders work in.
///
/// The caret is a byte offset into the raw input, which is not an offset into the
/// text: the text is the normalized spelling, with separators the user never typed
/// and without the characters normalization discarded. Translating it therefore needs
/// the number of input characters that survived, which is what the segmented builder
/// consumes, while the pass-through builder renders the raw input and takes the
/// offset itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaretTarget {
    /// The caret is at the end of the input, so it renders at the end of the text.
    End,
    /// The caret sits in front of the normalized character that follows `count`
    /// surviving input characters.
    AfterChars(usize),
}

impl CaretTarget {
    /// The offset in `normalized` the caret points at, or `None` when the caret is
    /// at the end of the input and therefore at the end of the text.
    fn normalized_offset(self, normalized: &str) -> Option<usize> {
        match self {
            Self::End => None,
            Self::AfterChars(count) => Some(char_offset(normalized, count)),
        }
    }
}

/// Renders the optimal cut of `dag` as syllables joined by separators.
///
/// `boundaries` is the node list [`SyllableDag::best_segmentation_hint`] filled, so
/// adjacent entries name one syllable edge each, the first is node `0` and the last
/// is the terminal node.
fn build_segmented(dag: &SyllableDag, boundaries: &[u16], caret: CaretTarget, out: &mut Preedit) {
    let caret_at = caret.normalized_offset(dag.normalized());
    let syllables = boundaries.len().saturating_sub(1);
    let mut caret_text = None;

    for (index, pair) in boundaries.windows(2).enumerate() {
        let (start, end) = (pair[0], pair[1]);
        let syllable_start = out.text.len();
        out.text.push_str(syllable_text(dag, start, end));
        let syllable_end = out.text.len();
        push_span(out, syllable_start, syllable_end, SpanKind::Syllable);

        // The caret belongs to the syllable whose normalized range holds it, and a
        // caret on a boundary belongs to the syllable that starts there -- which puts
        // it after the separator in front of that syllable. The first syllable that
        // claims it keeps it, so a later one cannot move it.
        if caret_text.is_none() {
            caret_text = caret_in_syllable(caret_at, start, end, syllable_start);
        }

        // One separator per boundary, except after the last syllable. A boundary the
        // user pinned already carries its own marker, and the syllable text leaves
        // that marker out, so the separator is written here either way and never
        // doubled.
        if index + 1 < syllables {
            let at = out.text.len();
            out.text.push('\'');
            let separator_end = out.text.len();
            push_span(out, at, separator_end, SpanKind::Separator);
        }
    }

    // A caret nothing claimed sits at the end of the text: it was at the end of the
    // input, or past the text the graph describes.
    let text_offset = caret_text.unwrap_or(out.text.len());
    out.caret = caret_offset(text_offset);
    insert_cursor_span(out, text_offset);
}

/// The text of the syllable the edge from `start` to `end` covers.
///
/// The edge is looked up instead of the range being sliced, because a forced
/// boundary marker belongs to no syllable and [`SyllableDag::syllable_text`] is what
/// leaves it out. The boundaries come from the graph itself, so the edge is always
/// found; the plain slice is the defensive answer and still covers the range.
fn syllable_text(dag: &SyllableDag, start: u16, end: u16) -> &str {
    let edges = dag.edges_from(usize::from(start));
    let edge = edges.iter().find(|edge| edge.end == end);
    match edge {
        Some(edge) => dag.syllable_text(start, *edge),
        None => {
            let range = usize::from(start)..usize::from(end);
            dag.normalized().get(range).unwrap_or("")
        }
    }
}

/// The text offset of a caret sitting in the syllable that covers the normalized
/// range `start..end`, or `None` when the caret is outside that range.
///
/// `text_start` is where the syllable's own text begins. A syllable is copied
/// verbatim from the normalized string, so an offset inside it keeps its distance
/// from the syllable's start, and the answer lands on a character boundary because
/// the caret does.
fn caret_in_syllable(
    caret: Option<usize>,
    start: u16,
    end: u16,
    text_start: usize,
) -> Option<usize> {
    let caret = caret?;
    let (start, end) = (usize::from(start), usize::from(end));
    if caret < start || caret >= end {
        return None;
    }
    Some(text_start + (caret - start))
}

/// Counts the characters of `raw[..caret]` that survive normalization.
///
/// Normalization keeps one character per surviving input character, in order, so this
/// count is the index of the normalized character the caret sits in front of. The
/// rules applied here are the normalizer's: a `'` is folded away when it leads the
/// input or doubles the marker before it, and a character outside the input alphabet
/// is dropped. The normalizer's only context-dependent rule -- a marker that trails
/// the whole input is folded away -- cannot fire inside a prefix, so a prefix count is
/// what the full input produces for that prefix.
///
/// The input buffer only ever holds alphabet characters, so the second rule is a guard
/// rather than a live branch; it is kept because it makes this count agree with the
/// normalizer for *any* input, which is what the test that cross-checks the two
/// asserts.
fn surviving_chars(raw: &str, caret: usize) -> usize {
    // Nothing has survived yet, which is the state in which a `'` is leading.
    let mut previous_marker = true;
    let mut survivors = 0usize;
    for ch in raw[..caret].chars() {
        match ch {
            '\'' if previous_marker => continue,
            '\'' => {
                survivors += 1;
                previous_marker = true;
            }
            _ if ch.is_ascii_alphabetic() || ch == 'ê' => {
                survivors += 1;
                previous_marker = false;
            }
            _ => continue,
        }
    }
    survivors
}

/// The byte offset of the `index`-th character of `text`, or the end of `text` when
/// it holds fewer characters.
fn char_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(at, _)| at)
}

/// Appends a span covering `start..end` of the text.
fn push_span(out: &mut Preedit, start: usize, end: usize, kind: SpanKind) {
    out.spans.push(PreeditSpan {
        start: span_offset(start),
        end: span_offset(end),
        kind,
    });
}

/// Inserts the zero-width caret span in its sorted position.
///
/// The position is in front of the first span that starts at or after the caret, so
/// the marker sits before the syllable or separator that begins there and never inside
/// another span's range.
fn insert_cursor_span(out: &mut Preedit, caret: usize) {
    let index = cursor_index(&out.spans, caret);
    out.spans.insert(
        index,
        PreeditSpan {
            start: span_offset(caret),
            end: span_offset(caret),
            kind: SpanKind::Cursor,
        },
    );
}

/// The index the caret span takes in a span list sorted by `start`.
///
/// It is the number of spans that start before the caret, so the marker lands in front
/// of the span that begins there -- the one place two spans may share a start.
fn cursor_index(spans: &[PreeditSpan], caret: usize) -> usize {
    let before = |span: &PreeditSpan| usize::from(span.start) < caret;
    spans.partition_point(before)
}

/// Converts a text offset into the span coordinate the contract uses.
///
/// The text is at most `2 * 64 - 1` bytes, so the conversion is exact for every input
/// the engine accepts; the clamp only makes it total.
fn span_offset(offset: usize) -> u16 {
    u16::try_from(offset).unwrap_or(u16::MAX)
}

/// Clamps a byte offset into the preedit text to the wider type the caret field uses.
///
/// [`PreeditSpan`] addresses the text with `u16` offsets, but [`Preedit::caret`] is a
/// `u32`, so the caret cannot simply reuse [`span_offset`]. The two agree for any text
/// this builder can produce; the fallback keeps the conversion total.
fn caret_offset(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// Moves `at` inside `text` and back onto the nearest character boundary.
///
/// The input alphabet is ASCII, so this never moves the caret of a real session; it
/// keeps the build total for a buffer whose caret was set by something else.
fn clamp_to_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests;
