//! Initial-letter abbreviations: the readings one input has when a syllable may be
//! typed as its initial alone, and the dictionary query each reading spells.
//!
//! Responsibility: own the algebra of abbreviation -- `nh` reading as `ni'hao`,
//! `nih` as the same word with its first syllable spelled out and its second one
//! left as an initial, `bjdx` as `bei'jing'da'xue` -- and answer, for one reading,
//! the prefix string a dictionary is queried with and the penalty an abbreviated
//! edge carries.
//!
//! Boundaries: this is the abbreviation algebra and nothing else. It does not walk
//! the segmentation graph, does not read the dictionary and scores nothing; a
//! caller hands it a string and gets readings back. It is a pure function of its
//! argument -- no file, no clock, no environment, no global state -- which is what
//! keeps the decoder's determinism guarantee intact.
//!
//! # The two ways to abbreviate
//!
//! An input is read as a sequence of units, each of which consumes at least one
//! letter: a fully spelled syllable the table holds, or a single letter standing
//! for every syllable whose spelling begins with it. An input whose letters are all
//! initials (`bjdx`) therefore has one reading per prefix of itself, and an input
//! that spells syllables out (`nih`, `beijingd`) has one reading per way of cutting
//! it. The two are not exclusive: the interjection syllables (`n`, `m`, `a`) are a
//! letter long, so `nh` reads both as the initial `n` followed by the initial `h`
//! and as the syllable `n` followed by the initial `h`.
//!
//! The enumeration deliberately does not decide which reading the user meant. `nh`
//! is `ni'hao` and `na'he` at once, and only the dictionary and the language model
//! can tell those apart, so the enumeration hands back every reading the syllable
//! table allows and lets the lattice rank them.
//!
//! # Why the input is not rewritten
//!
//! A reading keeps the byte offsets of the input it came from, exactly as the
//! segmentation graph does, and a unit never crosses a boundary marker: the marker
//! is consumed by the unit before it, which is the rule the graph's forced edge
//! follows. An edge built from a reading therefore ends on a node the graph already
//! has, and the preedit and the window keep pointing at the bytes they pointed at
//! before. This is also why the input is required to be normalized: a byte outside
//! the alphabet simply starts no unit, so an input that was not normalized has fewer
//! readings rather than being misread.
//!
//! # The dictionary query
//!
//! [`spell_into`] writes the string a reading is looked up under, which is the
//! reading's units joined by `'` -- the separator the dictionary keys are built
//! with. A reading that spells its first syllable out produces a query that is a
//! literal prefix of the full-pinyin key (`ni'h` for the word keyed `ni'hao`), and
//! a reading made of initials produces the abbreviation key itself (`n'h`). The
//! second form is the one a dictionary has to index: a dictionary that stores
//! full-pinyin keys alone answers the first query and not the second.
//!
//! One letter can stand for more than one spelling: `z` is both `z` and `zh`, and
//! the same holds for `c` and `s`. A reading therefore spells one query string per
//! combination of those choices, and [`spelling_count`] and [`spell_into`] walk the
//! combinations in a fixed order -- the choices of the first initial vary slowest --
//! so the queries a caller makes are the same on every run.
//!
//! # Order and the cap
//!
//! Readings are produced longest-syllable-first, and a reading is emitted only
//! after everything it can be extended into, so the most specific reading of the
//! input comes first and the vaguest -- every letter a bare initial -- comes last.
//! [`MAX_READINGS`] caps one call, and because the enumeration is ordered that way
//! the cap drops the vaguest readings rather than the most specific ones. The cap is
//! what keeps the lattice bounded: without it, the readings of a long input multiply
//! with every letter that can both start a syllable and be one. A caller that
//! enumerates the readings of a long input should bound the slice it hands over --
//! a word covers at most six syllables, so the letters of more than six are of no
//! use to the lattice -- or the specific readings will fill the cap by themselves.
//!
//! # Where it plugs in
//!
//! Abbreviation widens which words the lattice holds, not which syllables the input
//! has, so it sits beside the segmentation layer rather than inside the decoder,
//! exactly as fuzzy matching does. The caller gates the whole feature on
//! [`DecodeFlags::ABBREV`] through [`is_enabled`], asks for the readings of the
//! input from the node it is walking, and turns every reading whose
//! [`AbbrevReading::has_initial`] answers `true` into edges through the dictionary's
//! prefix query, bounded by [`PREFIX_LIMIT`] and scored through [`penalize`] so that
//! a full spelling outranks an abbreviation at equal dictionary weight. With the
//! switch clear nothing here is called at all, which is what makes the feature free
//! when it is off.

use ime_types::{DecodeFlags, SyllableId};
use smallvec::SmallVec;

use crate::segment::syllable::{MAX_SYLLABLE_LEN, lookup, syllable_at};

/// Most readings one call may produce.
///
/// Abbreviation multiplies the lattice: without a bound, `bjdx` would reach every
/// four-initial word in the dictionary. The cap is applied to an enumeration ordered
/// most-specific-first, so what it drops is the vaguest readings -- the ones a
/// reader is least likely to have meant -- and it is what keeps the work one input
/// costs independent of how ambiguous that input is.
pub const MAX_READINGS: usize = 64;

/// The stable diagnostic code an input raises when [`MAX_READINGS`] cut its
/// enumeration short.
///
/// The outcome is informational rather than a failure: the decode still answers,
/// with the most specific readings the cap left room for. The string is the code a
/// diagnostic is matched on, so it is never reworded, and the answer of
/// [`readings_into`] is the signal it reports.
pub const ABBREV_TRUNCATED_CODE: &str = "decode/abbrev-truncated";

/// Fewest letters an input must hold for abbreviation to apply at all.
///
/// A one-letter input stands for every syllable of the dictionary that begins with
/// it, which is a whole first-letter block of candidates: the list would be useless
/// and the lattice would be flooded. A one-letter input is answered by the
/// single-character fallback the graph already produces.
pub const MIN_LETTERS: usize = 2;

/// Most words one prefix query may return.
///
/// A single initial such as `z` prefixes tens of thousands of words. The dictionary
/// returns them in descending weight order, so the head of that list is where the
/// words a user means live, and the lattice takes no more than this many.
pub const PREFIX_LIMIT: usize = 64;

/// The score penalty one abbreviated edge carries, in the unit the sweep ranks with.
///
/// The sweep's edge score is Q16.16 and this constant is `6` in Q8.8 shifted into
/// that unit, about 2.3% of one unit of log probability. It is half the fuzzy
/// penalty on purpose: a fuzzy match means the user misspelled a syllable, while an
/// abbreviation means the user deliberately typed less, so an abbreviation has to
/// lose to a full spelling at equal dictionary weight, but only just.
pub const ABBREV_PENALTY_Q8: i32 = 6 << 8;

// The two relations the ranking depends on, checked at build time rather than in a test:
// a shortening has to cost something, and it has to cost less than a misspelling, or the
// order between an abbreviation and a fuzzy match flips.
const _: () = assert!(ABBREV_PENALTY_Q8 > 0);
const _: () = assert!(ABBREV_PENALTY_Q8 < crate::fuzzy::FUZZY_PENALTY_Q8);

/// Most query strings one reading may spell.
///
/// A letter stands for one spelling except `z`, `c` and `s`, which stand for two
/// each, so a reading spells `2^k` queries for its `k` ambiguous initials. Six
/// syllables is the longest word the lattice considers, which is where the cap comes
/// from: it is a guard against a reading longer than a word rather than a limit the
/// lattice ever reaches.
pub const MAX_SPELLINGS: usize = 64;

/// One syllable of an abbreviated reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbbrevSyllable {
    /// A fully spelled syllable.
    Full(SyllableId),
    /// A bare initial: the dictionary is asked for every word whose reading starts
    /// with this letter.
    ///
    /// The byte is the lower-case ASCII letter of the normalized input, which is
    /// also the first byte of every spelling the unit stands for.
    Initial(u8),
}

/// One way to read an input as a mix of full syllables and bare initials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbbrevReading {
    /// One entry per consumed unit, in order.
    pub syllables: Vec<AbbrevSyllable>,
    /// Bytes of the input the reading consumes.
    ///
    /// A reading covers a prefix of the input rather than all of it, so a caller
    /// walking the input node by node reads this to find the node the reading
    /// reaches, and can ask again from there for a longer one.
    pub consumed: u16,
}

impl AbbrevReading {
    /// Returns `true` when at least one unit of the reading is a bare initial.
    ///
    /// A reading with no initial is the input's ordinary full-pinyin cut, which the
    /// segmentation graph already covers through an exact lookup. A caller that wants
    /// abbreviation edges only -- the ones a prefix query and the penalty are for --
    /// asks this and skips the rest.
    pub fn has_initial(&self) -> bool {
        self.syllables
            .iter()
            .any(|unit| matches!(unit, AbbrevSyllable::Initial(_)))
    }
}

/// A buffer of readings that a caller keeps across keystrokes.
///
/// The enumeration holds one syllable vector per reading, and a decode runs it once
/// per node of the input, so a caller that keeps this buffer hands those vectors
/// back on the next call instead of building them again: after the first call of a
/// given shape, an enumeration of the same shape allocates nothing at all. The
/// readings of the last call are read through [`Readings::as_slice`].
#[derive(Debug, Default)]
pub struct Readings {
    /// The readings of the last call, in enumeration order.
    entries: Vec<AbbrevReading>,
    /// Syllable vectors of the readings of the previous call, kept for reuse.
    spare: Vec<Vec<AbbrevSyllable>>,
    /// The path the enumeration walks, kept so that a call allocates no buffer of
    /// its own. Empty between calls.
    path: Vec<AbbrevSyllable>,
    /// The prefix query the abbreviation walk spells, kept beside the readings it is
    /// spelled from for the same reason the syllable vectors are pooled: a buffer a
    /// caller keeps should never be asked for twice. The walk clears it before every
    /// spelling, so what it holds between calls is stale and never read.
    query: String,
}

impl Readings {
    /// Creates an empty buffer; the first call allocates.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the readings of the last call, in enumeration order.
    pub fn as_slice(&self) -> &[AbbrevReading] {
        &self.entries
    }

    /// Returns how many readings the last call produced.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the last call produced no reading at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The buffer one prefix query is spelled into.
    ///
    /// Crate-internal because spelling a query is the abbreviation walk's business; a
    /// reader of this buffer wants [`Readings::as_slice`]. The walk takes the `String`
    /// out, spells into it while it pushes edges, and puts it back, so the accessor is
    /// the only door and the allocation travels with the buffer's owner.
    pub(crate) fn query_buf(&mut self) -> &mut String {
        &mut self.query
    }

    /// Drops the readings of the last call, keeping every allocation for the next.
    ///
    /// The vectors are pooled in the reverse of the order they are taken out of it,
    /// so the reading built at position `i` of the next call gets the vector of the
    /// reading at position `i` of this one. Without that the two would swap places on
    /// every call and the buffer one reading had grown would be handed to another.
    fn clear(&mut self) {
        for reading in self.entries.drain(..).rev() {
            let mut syllables = reading.syllables;
            syllables.clear();
            // The pool is capped at the number of readings one call can produce, so a
            // caller that decodes one input at a time never grows it without bound.
            if self.spare.len() < MAX_READINGS {
                self.spare.push(syllables);
            }
        }
    }

    /// Appends one reading, reusing a syllable vector from the pool when there is one.
    fn push(&mut self, syllables: &[AbbrevSyllable], consumed: usize) {
        let mut buffer = self.spare.pop().unwrap_or_default();
        buffer.clear();
        buffer.extend_from_slice(syllables);
        self.entries.push(AbbrevReading {
            syllables: buffer,
            consumed: u16::try_from(consumed).unwrap_or(u16::MAX),
        });
    }
}

/// Whether abbreviation is switched on for a decode.
///
/// The frozen `DecodeFlags::ABBREV` bit is the whole switch: unlike fuzzy matching
/// there is no class namespace beside it, because abbreviation has no variants to
/// select. A caller asks this before enumerating anything, which is what makes the
/// feature free when it is off.
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
/// use ime_core::segment::abbrev::is_enabled;
/// use ime_types::DecodeFlags;
///
/// assert!(is_enabled(DecodeFlags::ABBREV));
/// assert!(is_enabled(DecodeFlags::ABBREV | DecodeFlags::USER_DICT));
/// assert!(!is_enabled(DecodeFlags::FUZZY));
/// assert!(!is_enabled(DecodeFlags::empty()));
/// ```
pub fn is_enabled(flags: DecodeFlags) -> bool {
    flags.contains(DecodeFlags::ABBREV)
}

/// Enumerates the readings of `raw`, allocating a buffer for the call.
///
/// This is the convenience form of [`readings_into`], which is the one a decode path
/// uses: it builds a [`Readings`] for the call and throws it away.
///
/// # Errors
///
/// Never: an input with no reading at all yields an empty vector, which the caller
/// treats as "abbreviation does not apply" rather than as a failure.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::segment::abbrev::readings;
///
/// // `bjdx` is four initials and nothing else, so its longest reading consumes the
/// // whole input, and its own prefixes give the shorter ones.
/// let found = readings("bjdx");
/// assert_eq!(found[0].consumed, 4);
/// assert!(found[0].has_initial());
///
/// // A one-letter input has no reading: the single-character fallback answers it.
/// assert!(readings("b").is_empty());
/// ```
pub fn readings(raw: &str) -> Vec<AbbrevReading> {
    let mut buffer = Readings::new();
    readings_into(&mut buffer, raw);
    buffer.entries
}

/// Fills `out` with every reading of `raw`, and answers whether [`MAX_READINGS`] cut
/// the enumeration short.
///
/// `raw` is the normalized input, as the segmentation graph holds it: lower-case
/// ASCII letters and `'` boundary markers. A byte outside that alphabet starts no
/// unit, so an input that was not normalized has fewer readings rather than being
/// misread.
///
/// The readings are ordered most-specific-first, and the answer is `true` only when
/// a reading was found that the cap left no room for. It is the signal behind the
/// [`ABBREV_TRUNCATED_CODE`] diagnostic, and it is a fact about this input rather
/// than about the flag set, so a caller that wants to record the diagnostic once has
/// to remember that it already did.
///
/// `out` is cleared first and then filled, and the syllable vectors of the readings
/// it held are reused, so a caller that keeps one buffer across keystrokes stops
/// allocating after the first call of a given shape.
///
/// # Errors
///
/// Never.
///
/// # Panics
///
/// Never.
pub fn readings_into(out: &mut Readings, raw: &str) -> bool {
    out.clear();
    if letter_count(raw) < MIN_LETTERS {
        return false;
    }
    // The path is taken out of the buffer so that it can be pushed into while the
    // readings are pushed into the buffer itself.
    let mut path = std::mem::take(&mut out.path);
    let mut stack: SmallVec<[Frame; INLINE_FRAMES]> = SmallVec::new();
    let mut current = Frame::root(raw);
    let mut truncated = false;
    loop {
        if let Some(unit) = current.units.at(current.next) {
            current.next += 1;
            let depth = path.len();
            stack.push(current);
            path.push(unit.syllable);
            current = Frame::new(raw, unit.end, depth);
            continue;
        }
        // Every unit of this position is spent, so the path is one complete reading.
        // It is emitted here, on the way back up, which is what puts the longest and
        // most specific readings in front of the vaguest ones. The cap is checked
        // before the push rather than after it, so the answer means "a reading was
        // dropped" and not merely "the cap was reached": an input whose readings
        // number exactly `MAX_READINGS` is enumerated whole and reports nothing.
        if !path.is_empty() {
            if out.len() >= MAX_READINGS {
                truncated = true;
                break;
            }
            out.push(&path, current.at);
        }
        path.truncate(current.depth);
        match stack.pop() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    path.clear();
    out.path = path;
    truncated
}

/// Returns the initials of the syllable table that begin with `letter`.
///
/// The answer holds one spelling per way a syllable starting with that letter can
/// begin: `z` reaches both `z` and `zh`, while `h` reaches only `h`. A letter no
/// syllable starts with -- `i`, `u` and `v`, the last of which normalization folds
/// into `ü` -- reaches nothing, which is what keeps a reading from being built out
/// of a letter the dictionary could never match.
///
/// The order is the syllable table's own, so the spellings of one letter are the
/// same on every run. A byte outside `a` through `z` answers with nothing: the input
/// alphabet is ASCII, and the one syllable outside it (`ê`) is always spelled out in
/// full because it has no initial to stand for.
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
/// use ime_core::segment::abbrev::initials_of;
///
/// assert_eq!(initials_of(b'z'), &["z", "zh"]);
/// assert_eq!(initials_of(b'h'), &["h"]);
/// // No syllable starts with `i`, so it can never be a bare initial.
/// assert!(initials_of(b'i').is_empty());
/// ```
pub fn initials_of(letter: u8) -> &'static [&'static str] {
    letter
        .checked_sub(b'a')
        .and_then(|index| INITIALS.get(usize::from(index)))
        .copied()
        .unwrap_or(&[])
}

/// How many query strings `reading` spells, capped at [`MAX_SPELLINGS`].
///
/// A reading with no initial spells one query, the reading itself. A reading whose
/// true count is past the cap answers with the cap; such a reading is refused by
/// [`spell_into`] rather than being silently cut to its first combinations, because
/// a caller that cannot see the queries it is not making cannot rank them.
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
/// use ime_core::segment::abbrev::{AbbrevReading, AbbrevSyllable, spelling_count};
///
/// let reading = AbbrevReading {
///     syllables: vec![AbbrevSyllable::Initial(b'z'), AbbrevSyllable::Initial(b'g')],
///     consumed: 2,
/// };
/// assert_eq!(spelling_count(&reading), 2);
/// ```
pub fn spelling_count(reading: &AbbrevReading) -> usize {
    combination_count(reading).unwrap_or(MAX_SPELLINGS)
}

/// Writes query string number `combination` of `reading` into `out`.
///
/// The query is the reading's units joined by `'`, a full syllable contributing its
/// table spelling and an initial contributing the spelling its digit selects. The
/// combinations are numbered in mixed-radix order over the initials, with the first
/// initial varying slowest, so combination `0` is the one that spells every initial
/// at its shortest.
///
/// Returns `false` -- and leaves `out` empty -- when the combination is past
/// [`spelling_count`], when the reading spells more combinations than
/// [`MAX_SPELLINGS`] holds, when an initial has no spelling at all, or when a unit
/// names a syllable the table does not hold.
///
/// # Errors
///
/// Never.
///
/// # Panics
///
/// Never: every division is exact because the combination count is the product of
/// exactly the choices the loop walks, so no weight is zero and no digit leaves its
/// list.
///
/// # Examples
///
/// ```
/// use ime_core::segment::abbrev::{readings, spell_into};
///
/// // `nih` spells `ni` out and leaves `hao` as its initial, so the query is a
/// // literal prefix of the word's own key.
/// let found = readings("nih");
/// let mut query = String::new();
/// assert!(spell_into(&mut query, &found[0], 0));
/// assert_eq!(query, "ni'h");
/// assert!("ni'hao".starts_with(&query));
/// ```
pub fn spell_into(out: &mut String, reading: &AbbrevReading, combination: usize) -> bool {
    out.clear();
    let Some(total) = combination_count(reading) else {
        return false;
    };
    if combination >= total {
        return false;
    }
    // The digits are read off by dividing the remaining combination by the weight of
    // the choices after this one, which the running prefix product gives exactly.
    let mut prefix = 1usize;
    let mut remaining = combination;
    for (index, unit) in reading.syllables.iter().enumerate() {
        if index > 0 {
            out.push('\'');
        }
        match *unit {
            AbbrevSyllable::Full(id) => {
                let Some(text) = syllable_at(id) else {
                    out.clear();
                    return false;
                };
                out.push_str(text);
            }
            AbbrevSyllable::Initial(letter) => {
                let choices = initials_of(letter);
                prefix = prefix.saturating_mul(choices.len());
                let weight = total / prefix;
                let digit = (remaining / weight) % choices.len();
                remaining %= weight;
                let Some(text) = choices.get(digit) else {
                    out.clear();
                    return false;
                };
                out.push_str(text);
            }
        }
    }
    true
}

/// Returns `score` with the penalty of an abbreviated edge applied.
///
/// `score` is one edge score in the Q16.16 unit the sweep ranks with, and the answer
/// is that score minus [`ABBREV_PENALTY_Q8`] in the same unit. The subtraction
/// saturates, so a score at the floor of the `i32` range stays where it is instead of
/// wrapping to the top.
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
/// use ime_core::segment::abbrev::{ABBREV_PENALTY_Q8, penalize};
///
/// // Two edges at equal dictionary weight: the fully spelled one wins.
/// let spelled_out = -3 << 16;
/// assert!(penalize(spelled_out) < spelled_out);
/// assert_eq!(spelled_out - penalize(spelled_out), ABBREV_PENALTY_Q8);
/// ```
pub fn penalize(score_q16: i32) -> i32 {
    score_q16.saturating_sub(ABBREV_PENALTY_Q8)
}

/// The exact number of query strings `reading` spells, or `None` when it is past
/// [`MAX_SPELLINGS`].
fn combination_count(reading: &AbbrevReading) -> Option<usize> {
    let mut total = 1usize;
    for unit in &reading.syllables {
        if let AbbrevSyllable::Initial(letter) = *unit {
            total = total.checked_mul(initials_of(letter).len())?;
            if total > MAX_SPELLINGS {
                return None;
            }
        }
    }
    Some(total)
}

/// Counts the letters of `raw`, leaving the boundary markers out of it.
fn letter_count(raw: &str) -> usize {
    raw.chars().filter(|ch| *ch != '\'').count()
}

/// Frames the walk's stack holds without touching the allocator.
///
/// A frame consumes at least one byte, so an input of `n` letters walks at most `n`
/// frames deep; eight covers the words a user abbreviates, and a longer input spills
/// to the heap rather than being refused.
const INLINE_FRAMES: usize = 8;

/// Most units one position of the input can start.
///
/// A position starts at most one syllable per length the table allows, plus the bare
/// initial, so the widest position is a `zhuang`-shaped one: `zhu`, `zhua`, `zhuan`,
/// `zhuang` and the initial `z`.
const MAX_UNITS: usize = MAX_SYLLABLE_LEN + 1;

/// One position of the walk: where it is, how far its units have been tried, and
/// what it can be extended by.
///
/// The units are held in the frame rather than looked up again on every step, so a
/// position is read from the table once no matter how many readings pass through it.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// Byte offset the partial reading has reached.
    at: usize,
    /// Length of the partial reading before this frame's own unit was pushed.
    depth: usize,
    /// Index of the next unit of `units` to try.
    next: usize,
    /// The units the input starts at `at`.
    units: Units,
}

impl Frame {
    /// Opens the walk at the first byte of `raw`.
    fn root(raw: &str) -> Self {
        Self::new(raw, 0, 0)
    }

    /// Opens a frame at `at`, remembering the depth to come back to.
    fn new(raw: &str, at: usize, depth: usize) -> Self {
        Self {
            at,
            depth,
            next: 0,
            units: units_at(raw, at),
        }
    }
}

/// One unit a reading can be built from: what it stands for and where it ends.
#[derive(Clone, Copy, Debug)]
struct Unit {
    /// The syllable the unit spells.
    syllable: AbbrevSyllable,
    /// Byte offset just past the unit, the boundary marker it carries included.
    end: usize,
}

/// The unit a slot holds before anything is written into it.
///
/// `AbbrevSyllable` has no neutral value -- every syllable is a real one -- but the
/// inline storage needs something to start from. Only the slots below the live
/// length are ever read, so the placeholder is never handed out.
const EMPTY_UNIT: Unit = Unit {
    syllable: AbbrevSyllable::Initial(0),
    end: 0,
};

/// The units one position of the input starts, the longest syllable first.
#[derive(Clone, Copy, Debug)]
struct Units {
    /// The units, live up to `len`.
    items: [Unit; MAX_UNITS],
    /// How many of the slots are live.
    len: usize,
}

impl Units {
    /// Creates an empty list.
    fn new() -> Self {
        Self {
            items: [EMPTY_UNIT; MAX_UNITS],
            len: 0,
        }
    }

    /// Appends one unit, dropping it when the list is already full.
    ///
    /// A position can start at most [`MAX_UNITS`] units, so the drop is unreachable
    /// rather than lossy; it is written as a checked store because the array is what
    /// the list is and an unchecked one would be a panic path.
    fn push(&mut self, syllable: AbbrevSyllable, end: usize) {
        if let Some(slot) = self.items.get_mut(self.len) {
            *slot = Unit { syllable, end };
            self.len += 1;
        }
    }

    /// Returns the unit at `index`, or `None` when the list is shorter.
    fn at(&self, index: usize) -> Option<Unit> {
        if index < self.len {
            self.items.get(index).copied()
        } else {
            None
        }
    }
}

/// Returns the units the input starts at `at`, the longest syllable first.
///
/// The order is the enumeration's ordering rule: a longer syllable is the more
/// specific reading of the letters it covers, and the bare initial -- the vaguest
/// reading of a position -- is tried last, after every syllable that starts there.
/// The initial is skipped for a letter no syllable begins with, which is what keeps
/// a reading from being built out of a letter the dictionary could never match.
fn units_at(raw: &str, at: usize) -> Units {
    let mut units = Units::new();
    let Some(&first) = raw.as_bytes().get(at) else {
        return units;
    };
    // A boundary marker starts no unit: it belongs to the unit before it, which is
    // the rule the graph's forced edge follows.
    if first == b'\'' {
        return units;
    }
    for length in (1..=MAX_SYLLABLE_LEN).rev() {
        let Some(end) = at.checked_add(length) else {
            continue;
        };
        // A slice that is not a character boundary, or that runs past the end, is no
        // syllable; `get` answers for both without the walk having to know which it
        // was.
        let Some(text) = raw.get(at..end) else {
            continue;
        };
        if let Some(syllable) = lookup(text) {
            units.push(AbbrevSyllable::Full(syllable), end + marker_len(raw, end));
        }
    }
    if !initials_of(first).is_empty() {
        units.push(
            AbbrevSyllable::Initial(first),
            at + 1 + marker_len(raw, at + 1),
        );
    }
    units
}

/// Returns one when a boundary marker sits at `at`, and zero otherwise.
fn marker_len(raw: &str, at: usize) -> usize {
    usize::from(raw.as_bytes().get(at) == Some(&b'\''))
}

/// The spellings a syllable beginning with one letter can start with, in the order
/// the syllable table holds them.
///
/// A `static` rather than a `const` because [`initials_of`] hands out references into
/// it, and a reference into a `const` item is a reference into a temporary.
// `#[rustfmt::skip]`: this is a data table, and one letter per line keeps it
// reviewable and inside the project's line limit.
#[rustfmt::skip]
static INITIALS: [&[&str]; 26] = [
    &["a"],        // a: `a`, `ai`, `an`, `ang`, `ao` all begin with the letter itself
    &["b"],        // b
    &["c", "ch"],  // c
    &["d"],        // d
    &["e"],        // e: `e`, `ei`, `en`, `eng`, `er`
    &["f"],        // f
    &["g"],        // g
    &["h"],        // h
    &[],           // i: no syllable begins with it
    &["j"],        // j
    &["k"],        // k
    &["l"],        // l
    &["m"],        // m
    &["n"],        // n
    &["o"],        // o: `o` and `ou`
    &["p"],        // p
    &["q"],        // q
    &["r"],        // r
    &["s", "sh"],  // s
    &["t"],        // t
    &[],           // u: no syllable begins with it
    &[],           // v: normalization folds it into `ü`, which no syllable begins with
    &["w"],        // w
    &["x"],        // x
    &["y"],        // y
    &["z", "zh"],  // z
];

#[cfg(test)]
mod tests;
