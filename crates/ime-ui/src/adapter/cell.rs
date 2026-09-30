//! The candidate grid's cell model: what one candidate draws, the five states of 3.4 and
//! the width estimator the truncation of 3.1.3 is computed with.
//!
//! `ui/candidate_grid.slint` draws one `CandidateData` per cell and decides nothing: the
//! number label, the text that fits, the annotation and the state all arrive from here, and
//! [`cell_data`] is the last step of that -- the model entry the component is handed. That
//! split is what lets every rule below be covered by a test that never opens a window, and
//! it is what keeps a `.slint` source free of business logic.
//!
//! # Where the state priority lives
//!
//! 3.4 ranks the five states `Disabled > Active > Focus Ring > Hover > Default` and allows
//! exactly one of them at a time. The ranking is resolved here, into a single
//! [`VisualState`], rather than in the component: the component then draws three booleans
//! that can never contradict each other.
//!
//! # Where the pointer state comes from
//!
//! `UiFrame` carries the candidates and the page, not the highlighted or the hovered one:
//! the engine's paging state holds those, and the frame is the UI thread's only view of the
//! session. [`PointerState`] is therefore an input of the adapter rather than a field of the
//! frame, and [`PointerState::for_page`] is the one place where the contract's global
//! numbering becomes a position within the page on show.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use ime_types::{Candidate, CandidateSource, PageState};
use slint::SharedString;

use crate::layout::Metrics;
use crate::ui_generated::CandidateData;

/// The character a cut text ends with (3.1.3).
const ELLIPSIS: char = '…';

/// How many distinct texts the width cache holds.
///
/// The candidate set of one reading repeats heavily across keystrokes and the window draws
/// at most a page of nine, so a cache this size covers a whole composing session: an entry
/// only leaves it once the user has typed past a few hundred distinct words.
pub const MEASURE_CACHE_CAPACITY: usize = 512;

/// The width of one ASCII character in ems.
///
/// Latin, digits and punctuation advance by roughly half an em at any font size.
const ASCII_EM: f32 = 0.5;

/// The width of one character outside ASCII, in ems.
///
/// A CJK glyph fills its em box, and rounding anything else up to one em over-estimates
/// rather than cuts: the estimate decides how much text a cell shows, and showing slightly
/// less than fits is recoverable where showing more than fits is not.
const WIDE_EM: f32 = 1.0;

/// The number keys of 3.5, in the order they name a candidate on the page.
const NUMBER_LABELS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Writes `text` into a buffer the caller already holds, reporting whether it changed.
///
/// Every string the window draws goes through here: a frame whose text did not change
/// writes nothing and allocates nothing, which is what keeps a repeated keystroke free.
pub(crate) fn write_text(target: &mut String, text: &str) -> bool {
    if target == text {
        return false;
    }
    target.clear();
    target.push_str(text);
    true
}

/// Stores `value` in `slot`, reporting whether the slot changed.
pub(crate) fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// The five states of 3.4, as one value.
///
/// Exactly one state applies at a time, which is what the component's three booleans
/// express: the ranking is resolved here and the component only draws the result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualState {
    /// Nothing is over the cell and it is not highlighted.
    #[default]
    Default,
    /// The pointer is over the cell.
    Hover,
    /// The pointer is pressing the cell.
    Active,
    /// The keyboard highlight, which in this phase is also the first candidate's ring.
    FocusRing,
    /// Reserved: 3.4's `Disabled` row. Nothing in this phase disables a candidate, and the
    /// variant exists so that the ranking below is complete and testable.
    Disabled,
}

impl VisualState {
    /// Resolves the ranking of 3.4: `Disabled > Active > Focus Ring > Hover > Default`.
    ///
    /// # Parameters
    ///
    /// * `highlighted` -- the cell carries the keyboard highlight.
    /// * `hovered` -- the pointer is over the cell.
    /// * `pressed` -- the pointer is pressing the cell.
    /// * `disabled` -- the candidate cannot be selected.
    ///
    /// # Returns
    ///
    /// The one state the cell is in.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn resolve(highlighted: bool, hovered: bool, pressed: bool, disabled: bool) -> Self {
        if disabled {
            Self::Disabled
        } else if pressed {
            Self::Active
        } else if highlighted {
            Self::FocusRing
        } else if hovered {
            Self::Hover
        } else {
            Self::Default
        }
    }
}

/// The pointer and highlight state the grid draws from, in page coordinates.
///
/// The three fields are positions within `UiFrame::candidates`, zero-based, because that is
/// what the grid compares against a cell. The contract numbers candidates globally across
/// pages (`Paging::highlight`, `UiEvent::Hover` and `UiEvent::Select` all carry the global
/// index); [`PointerState::for_page`] is the conversion between the two, and the only place
/// it happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerState {
    /// The candidate the keyboard has highlighted, if any.
    pub highlighted: Option<u16>,
    /// The candidate the pointer is over, if any.
    pub hovered: Option<u16>,
    /// The candidate the pointer is pressing, if any.
    pub pressed: Option<u16>,
}

impl Default for PointerState {
    /// The state of a session that has just decoded: the first candidate of the page is
    /// highlighted, which is where `Paging` starts and what `Space` commits.
    fn default() -> Self {
        Self {
            highlighted: Some(0),
            hovered: None,
            pressed: None,
        }
    }
}

impl PointerState {
    /// Re-anchors indices the contract numbers globally to the page `page` shows.
    ///
    /// The frame the pointer state belongs to is the one whose page this is; an index on
    /// another page names no cell of this one and becomes `None`, so a stale hover from the
    /// page the user just left cannot draw on the page they are on.
    ///
    /// # Parameters
    ///
    /// * `page` -- the paging state of the frame being drawn.
    ///
    /// # Returns
    ///
    /// The same state, in positions within the page.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn for_page(self, page: &PageState) -> Self {
        Self {
            highlighted: self
                .highlighted
                .and_then(|index| local_position(index, page)),
            hovered: self.hovered.and_then(|index| local_position(index, page)),
            pressed: self.pressed.and_then(|index| local_position(index, page)),
        }
    }
}

/// The position of the globally numbered candidate `index` within the page `page` shows.
///
/// The frame describes its page as a [`PageState`], whose `current` is one-based and whose
/// `page_size` is the configured candidates per row, so the page starts at
/// `(current - 1) * page_size`. That is the same arithmetic as the engine's
/// `Paging::page_start`, restated because the view layer does not depend on the engine; an
/// index before the page, and an empty frame, answer `None` rather than a wrapped position.
///
/// # Parameters
///
/// * `index` -- a global candidate index, as `UiEvent` and `Paging` number them.
/// * `page` -- the paging state of the frame the index belongs to.
///
/// # Returns
///
/// The zero-based position within the page, or `None` when the index is not on it.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn local_position(index: u16, page: &PageState) -> Option<u16> {
    index.checked_sub(page_start(page))
}

/// The global index of the first candidate of the page on show.
///
/// A one-based `current` of zero -- an empty frame -- has no page, and starts at zero.
fn page_start(page: &PageState) -> u16 {
    u16::from(page.current.saturating_sub(1)).saturating_mul(u16::from(page.page_size))
}

/// The geometry and type scale one cell is drawn with.
///
/// One value per cell rather than a parameter list: the width and the metrics are the same
/// for every cell of a page and only the position and the annotation's width differ, so the
/// caller builds one and adjusts two fields per cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellGeometry {
    /// Position of the candidate within the page, zero-based.
    ///
    /// It decides the number label and which cell the pointer state names, which is why it
    /// is a position rather than the candidate's own index.
    pub position: u16,
    /// Shared width of every cell in the grid, in logical pixels.
    pub width: f32,
    /// What the annotation costs beside the text, in logical pixels; zero when none is
    /// drawn, which includes the gap before it.
    ///
    /// The zero carries the whole decision -- the layout's own switch and this cell's room
    /// for a reading hint -- so nothing else has to know why an annotation is absent.
    pub annotation_width: f32,
    /// The component's constants, from [`crate::layout::metrics`].
    pub metrics: Metrics,
}

impl CellGeometry {
    /// The width the cell's text may occupy, in logical pixels.
    ///
    /// A cell spends [`Metrics::cell_chrome_width`] on its padding, its number label and
    /// the gap after it; the annotation takes what it needs; and 3.1.3 stops the text at
    /// [`Metrics::max_text_width`] however much room is left over.
    ///
    /// # Returns
    ///
    /// The budget in logical pixels, never negative: a cell narrower than its own chrome
    /// leaves nothing for the text rather than a negative budget.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn text_budget(&self) -> f32 {
        let chrome = self.metrics.cell_chrome_width + self.annotation_width;
        (self.width - chrome)
            .min(self.metrics.max_text_width)
            .max(0.0)
    }
}

/// One candidate cell, as `ui/candidate_grid.slint` draws it.
///
/// A plain value, so the grid can be asserted on without a Slint platform, and the strings
/// are kept across frames so a redrawn page reuses the buffers it already holds.
#[derive(Clone, Debug, PartialEq)]
pub struct CellState {
    /// Global candidate index, the numbering `UiEvent::Select` and `Paging` use.
    ///
    /// It is what a click names, and it is deliberately not what the cell prints: the
    /// label below is the key that selects the candidate on the page on show.
    pub index: u16,
    /// The number key of 3.5 that selects this candidate, empty past the ninth.
    ///
    /// A tenth candidate has no key of its own, so it draws no label; it stays in the grid
    /// and stays clickable.
    pub label: String,
    /// The full text. Never cut: this is what a selection commits.
    pub text: String,
    /// The text as drawn, cut with `…` when it does not fit the cell (3.1.3).
    pub display_text: String,
    /// The grey right-hand annotation, empty when there is none to draw.
    pub annotation: String,
    /// Where the candidate came from.
    ///
    /// Carried for the tasks that draw a source label; 3.4 defines no per-source style, so
    /// every source is drawn alike today.
    pub source: CandidateSource,
    /// The state of 3.4 this cell is in.
    pub state: VisualState,
}

impl Default for CellState {
    /// An empty cell: no candidate, the default state.
    fn default() -> Self {
        Self {
            index: 0,
            label: String::new(),
            text: String::new(),
            display_text: String::new(),
            annotation: String::new(),
            source: CandidateSource::Dict,
            state: VisualState::Default,
        }
    }
}

impl CellState {
    /// Writes one candidate into this cell, reusing the buffers the cell already holds.
    ///
    /// # Parameters
    ///
    /// * `candidate` -- the candidate to draw.
    /// * `geometry` -- the cell's position, width and type scale.
    /// * `measure` -- the width estimator, which the truncation of 3.1.3 is computed with.
    ///
    /// # Returns
    ///
    /// Whether anything the cell draws changed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(crate) fn write(
        &mut self,
        candidate: &Candidate,
        geometry: CellGeometry,
        measure: &mut Measure,
    ) -> bool {
        let mut changed = false;
        changed |= replace(&mut self.index, candidate.index);
        changed |= write_label(&mut self.label, geometry.position);
        changed |= write_text(&mut self.text, &candidate.text);
        changed |= measure.write_elided(&mut self.display_text, &candidate.text, geometry);
        changed |= write_text(&mut self.annotation, annotation_of(candidate, geometry));
        changed |= replace(&mut self.source, candidate.source);
        changed
    }

    /// Re-resolves the state of this cell from `pointer`, at `position` within the page.
    ///
    /// # Parameters
    ///
    /// * `position` -- the cell's zero-based position within the page.
    /// * `pointer` -- the pointer and highlight state, in the same coordinates.
    ///
    /// # Returns
    ///
    /// Whether the state changed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(crate) fn resolve(&mut self, position: u16, pointer: PointerState) -> bool {
        let state = VisualState::resolve(
            pointer.highlighted == Some(position),
            pointer.hovered == Some(position),
            pointer.pressed == Some(position),
            // 3.4's `Disabled` row is reserved: nothing in this phase produces a disabled
            // candidate, and the ranking stays complete without one.
            false,
        );
        replace(&mut self.state, state)
    }
}

/// The number label of the candidate at `position` within the page.
///
/// The label is the key the user presses, so it counts within the page rather than across
/// the whole decoded list: the digits start over on every page (`Paging::digit_target`).
fn write_label(target: &mut String, position: u16) -> bool {
    let label = NUMBER_LABELS
        .get(usize::from(position))
        .copied()
        .unwrap_or("");
    write_text(target, label)
}

/// The annotation a cell draws: the candidate's own, or nothing when the cell has no room
/// for it.
///
/// The room is the whole test, because [`CellGeometry::annotation_width`] already carries
/// both halves of the decision -- the layout's switch and this cell's width.
fn annotation_of(candidate: &Candidate, geometry: CellGeometry) -> &str {
    if geometry.annotation_width > 0.0 {
        candidate.annotation.as_deref().unwrap_or("")
    } else {
        ""
    }
}

/// The width estimator of the candidate grid.
///
/// One em per character outside ASCII and half an em per ASCII one: the window draws CJK
/// candidates, where a glyph fills its em box, and Latin text advances by roughly half of
/// one. Estimating a Latin character at a full em would cut text that fits, and a CJK one
/// at half an em would let it overflow the cell.
///
/// This is an estimate, not a font measurement: the view layer owns no font metrics
/// (`ASM-09`), and the component's `overflow: elide` stays the last-resort guard it is
/// meant to be.
///
/// # The cache
///
/// A width is linear in the font size, so what is cached is the em width of a text rather
/// than a pixel width: one entry serves both the cell font and the smaller annotation font.
/// The candidate set of one reading repeats heavily across keystrokes, so the cache is what
/// keeps a keystroke from walking every candidate's text again. It holds
/// `MEASURE_CACHE_CAPACITY` entries; a text that arrives at a full cache evicts the least
/// recently used one.
///
/// # Concurrency
///
/// Owned by the adapter and used on the UI thread only. No method blocks, takes a lock or
/// allocates on the hit path.
#[derive(Debug, Default)]
pub struct Measure {
    /// The cached em widths, keyed by the text they were measured from.
    entries: HashMap<String, Entry>,
    /// Counts every use, so eviction follows real use rather than insertion order.
    clock: u64,
    /// Texts measured for the first time. The cache test asserts on this instead of on a
    /// wall-clock budget, which a shared machine cannot make reproducible.
    misses: u64,
}

/// One cached measurement.
#[derive(Clone, Copy, Debug)]
struct Entry {
    /// The width of the text in ems.
    ems: f32,
    /// The clock reading of its last use.
    used: u64,
}

impl Measure {
    /// The width of `text` at `font_size`, from the cache when it is already known.
    ///
    /// # Parameters
    ///
    /// * `text` -- the text to measure.
    /// * `font_size` -- the size it is drawn at, in logical pixels.
    ///
    /// # Returns
    ///
    /// The estimated width in logical pixels.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics: the width is a sum of `f32` terms, so a text long enough to lose
    /// precision loses it in the estimate rather than in the arithmetic.
    pub fn width(&mut self, text: &str, font_size: f32) -> f32 {
        self.em_width(text) * font_size
    }

    /// How many texts this measure has computed for the first time.
    ///
    /// # Returns
    ///
    /// The miss count since the measure was created.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// How many texts the cache holds.
    ///
    /// # Returns
    ///
    /// The number of cached measurements.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds nothing.
    ///
    /// # Returns
    ///
    /// `true` before the first measurement.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Writes the text a cell draws into `target`, reusing the buffer it holds.
    ///
    /// `text` itself when it fits [`CellGeometry::text_budget`], and otherwise its longest
    /// prefix that fits with an ellipsis appended (3.1.3). The cut is on a character
    /// boundary by construction -- the walk is over `char`s -- so what is written is always
    /// valid UTF-8, and a text that is cut always ends with `…`, which is what tells the
    /// user the cell is showing less than it holds.
    ///
    /// # Parameters
    ///
    /// * `target` -- the buffer to write into.
    /// * `text` -- the full text of the candidate.
    /// * `geometry` -- the cell the text is drawn in.
    ///
    /// # Returns
    ///
    /// Whether `target` changed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(crate) fn write_elided(
        &mut self,
        target: &mut String,
        text: &str,
        geometry: CellGeometry,
    ) -> bool {
        let size = geometry.metrics.font_size_cell;
        let budget = geometry.text_budget();
        if self.width(text, size) <= budget {
            return write_text(target, text);
        }
        // The ellipsis is reserved before the first character is placed: a text cut without
        // one would read as a complete word, which is the misreading 3.1.3 exists to
        // prevent. A budget too small even for the ellipsis therefore shows the ellipsis
        // alone rather than an empty cell.
        let room = (budget - ellipsis_width(size)).max(0.0);
        let mut used = 0.0;
        let mut cut = 0;
        for (offset, character) in text.char_indices() {
            let width = character_em(character) * size;
            if used + width > room {
                break;
            }
            used += width;
            cut = offset + character.len_utf8();
        }
        write_cut(target, text, cut)
    }

    /// The em width of `text`, computing and caching it when it is not known yet.
    fn em_width(&mut self, text: &str) -> f32 {
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(text) {
            entry.used = self.clock;
            return entry.ems;
        }
        let ems = text_ems(text);
        self.misses = self.misses.saturating_add(1);
        self.insert(text, ems);
        ems
    }

    /// Caches one measurement, evicting the least recently used entry when the cache is
    /// full.
    ///
    /// The eviction scans the cache, which is 512 comparisons and only happens when a
    /// genuinely new text arrives at a full cache: the hit path stays a single lookup.
    fn insert(&mut self, text: &str, ems: f32) {
        if self.entries.len() >= MEASURE_CACHE_CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            String::from(text),
            Entry {
                ems,
                used: self.clock,
            },
        );
    }
}

/// The width of `text` in ems.
fn text_ems(text: &str) -> f32 {
    text.chars().map(character_em).sum()
}

/// The width of one character in ems.
///
/// Crate-visible rather than private because the preedit's cut walks a span a character at a
/// time and must not spend a cache entry per character doing it; see
/// [`super::preedit`].
pub(crate) fn character_em(character: char) -> f32 {
    if character.is_ascii() {
        ASCII_EM
    } else {
        WIDE_EM
    }
}

/// The width of the ellipsis in logical pixels.
fn ellipsis_width(font_size: f32) -> f32 {
    character_em(ELLIPSIS) * font_size
}

/// Writes the first `cut` bytes of `text` followed by the ellipsis into `target`.
///
/// The comparison comes first so a cell whose text did not change keeps its buffer, which
/// is what makes an unchanged frame free.
fn write_cut(target: &mut String, text: &str, cut: usize) -> bool {
    let unchanged = target.len() == cut + ELLIPSIS.len_utf8()
        && target.starts_with(&text[..cut])
        && target.ends_with(ELLIPSIS);
    if unchanged {
        return false;
    }
    target.clear();
    target.push_str(&text[..cut]);
    target.push(ELLIPSIS);
    true
}

/// One cell, as the grid's model holds it.
///
/// The three state booleans are derived from the single state the ranking of 3.4 resolved, so
/// they can never contradict each other, and the full text travels beside the text that is
/// drawn: a cell cut with `…` still carries what a selection commits.
pub(crate) fn cell_data(cell: &CellState) -> CandidateData {
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
