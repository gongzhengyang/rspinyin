//! Script conversion: a committed word becomes its spelling in the other script.
//!
//! Responsibility: rewrite one string from simplified into traditional Chinese or
//! back, using a table of variant pairs -- `头发` is `頭髮`, `干净` is `乾淨`. The
//! rewrite is a longest-match replacement over a bounded window, and it is the only
//! thing this module does.
//!
//! Boundaries: a pure function of its two arguments. The table arrives as a
//! [`VariantSource`], never as a file handle: no file, no clock, no environment, no
//! global state, no display server (0.4 rule 4). The compiled table lives in the
//! dictionary layer, which implements [`VariantSource`] over its memory map; the
//! in-memory [`VariantTable`] beside this file implements the same trait, which is
//! what lets the algorithm be tested without a dictionary.
//!
//! # Why the conversion is not part of the decode
//!
//! The script a reader sees is not a property of how a word is pronounced, so the
//! rewrite runs on the text a session is about to commit, after the decoder has
//! ranked its candidates. Letting it into the scoring pass would make the candidate
//! order depend on a dimension the language model knows nothing about, and two
//! inputs that read alike would stop ordering alike. It follows that a conversion
//! never changes how many candidates a decode returns, nor the order they come in.
//!
//! # Longest match, and why one character is not enough
//!
//! A one-character mapping is wrong wherever a word-level mapping exists. `干` on
//! its own is ambiguous -- `乾淨`, `幹活` and `干涉` all begin with it -- but each of
//! those words is unambiguous once its second character is known. The rewrite
//! therefore tries the longest window first, [`MAX_MATCH_CHARS`] characters down to
//! one, and takes the first hit.
//!
//! # Lossless, never lossy
//!
//! A character the table cannot map is copied through unchanged. Nothing is ever
//! dropped and nothing is replaced by a placeholder. The text here is what the user
//! is about to send to their application: a character silently removed is data loss
//! they cannot see, and a conversion that cannot express a character still has to
//! commit *something*. The input is the only honest answer, and it is also what the
//! user typed. A table with no entry for an ambiguous single character (`发`, `干`,
//! `后`, `里`, `台`) is therefore refusing to guess rather than failing: the
//! character is left alone instead of being mapped to one of its several traditional
//! forms at random.
//!
//! # Idempotence
//!
//! Text already in the target script comes back unchanged, provided no pair's
//! traditional form is itself a simplified key. That is a property of the *table*,
//! not of the rewrite, and the tests beside the seed table assert it for the shipped
//! one; adding an entry is what could break it.
//!
//! # Determinism
//!
//! The rewrite walks the input left to right and, at each position, tries window
//! lengths in decreasing order. The result depends on the text and the table and on
//! nothing else -- no hash order, no clock, no environment.
//!
//! # Cost
//!
//! One conversion is at most `len * MAX_MATCH_CHARS` table lookups, each a binary
//! search, plus one `String` for the result. The commit path is the one place the
//! rewrite runs, and it runs there under the design's conversion budget.

use ime_types::ui::Script;

mod table;

pub use table::{SEED, VariantEntry, VariantTable};

#[cfg(test)]
mod tests;

/// The longest word the mapper looks up, in characters.
///
/// Eight covers every disambiguating word in the table with room to spare and
/// bounds the per-commit work at `len * 8` lookups.
pub const MAX_MATCH_CHARS: usize = 8;

/// A table of script variants, read one direction at a time.
///
/// The implementations are the compiled index in the dictionary layer -- which
/// answers from a memory map -- and [`VariantTable`] in memory. An implementation
/// must be `Send + Sync` and must not mutate: the table is shared by the thread that
/// decodes and the thread that draws, and a conversion on one must not be visible to
/// the other.
///
/// A lookup must be reentrant and must return in time bounded by the length of
/// `word` alone. It runs on the commit path, inside a host callback, which may not
/// block (0.4 rule 10).
pub trait VariantSource {
    /// Returns the spelling of `word` in `target`, or `None` when the table holds
    /// no distinct form for it.
    ///
    /// `None` is not a failure: the rewrite leaves the word exactly as it is, which
    /// is what makes a table with a gap lossless rather than lossy. An
    /// implementation that cannot serve one of the two directions at all answers
    /// `None` for it, and that direction degrades to the identity -- the text is
    /// still committed, and it is committed unchanged.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A word the table does
    /// not hold is an answer, not an error.
    ///
    /// # Panics
    ///
    /// Implementations must not panic. A lookup runs inside a host callback, where
    /// an unwind has nowhere to go.
    fn lookup(&self, word: &str, target: Script) -> Option<&str>;
}

/// Rewrites `text` into the script `target`.
///
/// Runs on the final committed string, after the decoder has chosen a candidate:
/// the script a user reads is not a property of how a word is pronounced, so letting
/// it into the scoring pass would make the candidate order depend on a dimension the
/// language model knows nothing about.
///
/// Matching is longest-first over a bounded window, because a one-character mapping
/// is wrong wherever a word-level mapping exists: `干` alone is ambiguous, but
/// `干净` is always `乾淨` and `干活` is always `幹活`. A character the table cannot
/// map is copied through unchanged; see the module documentation for why the rewrite
/// never drops one.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`. Text the table cannot map is
/// returned unchanged, which is the correct outcome rather than a failure.
///
/// # Examples
///
/// ```
/// use ime_core::script::{VariantTable, to_traditional};
///
/// let table = VariantTable::from_pairs(&[("干净", "乾淨")]);
/// assert_eq!(to_traditional("干净", &table), "乾淨");
/// ```
pub fn convert<S: VariantSource + ?Sized>(text: &str, source: &S, target: Script) -> String {
    convert_with_offsets(text, source, target).into_text()
}

/// Rewrites `text` into `target` and reports where the input's byte offsets landed.
///
/// The conversion can change a word's byte length, so a caller holding a byte offset
/// into the input -- a caret, a span boundary -- cannot reuse it on the output.
/// [`ConvertedText::map_offset`] answers with the offset in the converted text that a
/// given input offset maps to.
///
/// This is the same rewrite [`convert`] performs; it costs one entry per input
/// character for the map, so a caller with no offsets to carry should call
/// [`convert`] instead.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Examples
///
/// ```
/// use ime_core::script::{VariantTable, convert_with_offsets};
/// use ime_types::ui::Script;
///
/// // A fixture pair whose sides differ in length, which is what gives the map
/// // something to do: an entry generated from a variant field usually keeps the
/// // character count, and a caller must not rely on that.
/// let table = VariantTable::from_pairs(&[("甲", "甲乙")]);
/// let converted = convert_with_offsets("甲甲", &table, Script::Traditional);
///
/// assert_eq!(converted.text(), "甲乙甲乙");
/// assert_eq!(converted.map_offset(3), Some(6), "the second character moved");
/// assert_eq!(converted.map_offset(1), None, "not a character boundary");
/// ```
pub fn convert_with_offsets<S: VariantSource + ?Sized>(
    text: &str,
    source: &S,
    target: Script,
) -> ConvertedText {
    let mut converted = String::with_capacity(text.len());
    // At most one boundary per character plus the end of the text; sizing for the
    // worst case of one byte per character keeps the pushes below from reallocating.
    let mut boundaries = Vec::with_capacity(text.len().saturating_add(1));
    // Byte offset one past the last character a match already replaced. Every
    // character starting before it belongs to that match and is skipped, which is
    // what makes the walk below a single pass over the characters.
    let mut replaced_until = 0usize;
    for (at, ch) in text.char_indices() {
        if at < replaced_until {
            continue;
        }
        boundaries.push(Boundary {
            input: at,
            output: converted.len(),
        });
        match longest_match(text, at, source, target) {
            Some((len, replacement)) => {
                converted.push_str(replacement);
                replaced_until = at.saturating_add(len);
            }
            None => converted.push(ch),
        }
    }
    boundaries.push(Boundary {
        input: text.len(),
        output: converted.len(),
    });
    ConvertedText {
        text: converted,
        boundaries,
    }
}

/// Rewrites `text` into traditional Chinese.
///
/// The direction [`Script::Traditional`] names; see [`convert`] for the matching
/// rule and for what happens to a character the table cannot map.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Examples
///
/// ```
/// use ime_core::script::{VariantTable, to_traditional};
///
/// let table = VariantTable::from_pairs(&[("头发", "頭髮"), ("银行", "銀行")]);
/// assert_eq!(to_traditional("去银行理发", &table), "去銀行理发");
/// ```
pub fn to_traditional<S: VariantSource + ?Sized>(text: &str, source: &S) -> String {
    convert(text, source, Script::Traditional)
}

/// Rewrites `text` into simplified Chinese.
///
/// The reverse direction, for a user who typed in traditional and wants simplified
/// output. It reads the same table from the other side: a pair is a relation, so the
/// traditional spelling of a word is also the key that finds it.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
pub fn to_simplified<S: VariantSource + ?Sized>(text: &str, source: &S) -> String {
    convert(text, source, Script::Simplified)
}

/// Converted text together with the byte offsets the conversion moved.
///
/// Built by [`convert_with_offsets`]. The map holds one entry for every position of
/// the input at which the rewrite produced output -- the start of each word it
/// replaced and of each character it copied -- plus the end of the text, so a caller
/// can carry a caret or a span boundary across a conversion that changed a word's
/// length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvertedText {
    /// The text after the rewrite.
    text: String,
    /// Input offsets and the output offset each landed on, in increasing input
    /// order, ending with the end of the input.
    boundaries: Vec<Boundary>,
}

impl ConvertedText {
    /// The converted text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Consumes the value and returns the converted text.
    pub fn into_text(self) -> String {
        self.text
    }

    /// Returns the offset in [`Self::text`] that `offset` in the input maps to.
    ///
    /// Returns `None` when `offset` has no counterpart in the output. That covers
    /// three cases: an offset past the end of the input, an offset inside a
    /// character, and an offset inside a word the rewrite replaced as a unit. The
    /// third is deliberate rather than a gap -- a replaced word is one opaque
    /// string, so there is no position inside it to point at, and rounding the
    /// answer down to the start of the word would move a caret to a place the user
    /// did not ask for.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. An offset with no
    /// counterpart is an answer, not a failure.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::script::{VariantTable, convert_with_offsets};
    /// use ime_types::ui::Script;
    ///
    /// let table = VariantTable::from_pairs(&[("甲", "甲乙")]);
    /// let converted = convert_with_offsets("甲", &table, Script::Traditional);
    /// assert_eq!(converted.map_offset(0), Some(0));
    /// assert_eq!(converted.map_offset(3), Some(6), "the end of the input moved too");
    /// ```
    pub fn map_offset(&self, offset: usize) -> Option<usize> {
        match self
            .boundaries
            .binary_search_by_key(&offset, |held| held.input)
        {
            Ok(index) => self.boundaries.get(index).map(|held| held.output),
            Err(_) => None,
        }
    }
}

/// One input offset and the output offset it landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Boundary {
    /// Byte offset in the text the conversion was given.
    input: usize,
    /// Byte offset in the converted text, always a character boundary.
    output: usize,
}

/// Finds the longest table entry that starts at `at`, with its byte length.
///
/// The window shrinks from [`MAX_MATCH_CHARS`] characters down to one, so the first
/// hit is the longest entry the table holds there. `None` means the table knows
/// nothing that starts at `at`, which is the caller's cue to copy the character
/// through.
fn longest_match<'source, S: VariantSource + ?Sized>(
    text: &str,
    at: usize,
    source: &'source S,
    target: Script,
) -> Option<(usize, &'source str)> {
    let rest = text.get(at..)?;
    for count in (1..=MAX_MATCH_CHARS).rev() {
        let word = window(rest, count);
        // An empty replacement would delete the word it matched, so a source that
        // answers with one is treated as answering nothing: the rewrite never drops
        // text, whatever the table says.
        let replacement = source.lookup(word, target).filter(|held| !held.is_empty());
        if let Some(replacement) = replacement {
            return Some((word.len(), replacement));
        }
    }
    None
}

/// Returns the longest prefix of `rest` that is at most `count` characters long.
///
/// `rest` is never empty where this is called, so the window is never empty either:
/// the table is never asked about the empty string.
fn window(rest: &str, count: usize) -> &str {
    // The offset of the `count`-th character is the end of the window; fewer than
    // `count` characters left means the whole rest is the window. The `and_then` is
    // unreachable for a boundary `char_indices` produced, and falls back to the
    // whole rest rather than to a panic.
    rest.char_indices()
        .nth(count)
        .and_then(|(end, _)| rest.get(..end))
        .unwrap_or(rest)
}
