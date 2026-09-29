//! The phrase table: the shortcuts a user defines for their own text.
//!
//! Responsibility: hold the phrases a user defined -- `rq` for a date, `dz` for an
//! email address -- and turn the ones that match an input into candidates. A phrase
//! is not a word: it has no reading, so it is not a lattice edge and takes no part in
//! the scoring pass. It enters the candidate list after the decoder has ranked its
//! words, ahead of them, because a user who defined `rq` means `rq`.
//!
//! Boundaries: everything here is pure (0.4 rule 4). The text of a phrase document
//! arrives as a `&str` and the loader that owns the bytes lives in the host layer;
//! no file, no clock, no environment and no global state is read. That is also why
//! the time escapes a body may carry (`%Y`, `%m`, ...) are expanded by the host
//! before a body reaches this table: a table that read the clock would make a decode
//! depend on when it ran.
//!
//! # The document
//!
//! One entry per line, two tab-separated columns; a blank line and a line whose
//! first non-blank character is `#` carry no entry:
//!
//! ```text
//! # key <TAB> the text the key commits
//! rq	2026-09-29
//! dz	user@example.com
//! ```
//!
//! The key is folded to lower case and must then be 1..=[`MAX_PHRASE_KEY_LEN`] bytes
//! of `a`..=`z` or the `'` that pins a syllable boundary; the body must be non-empty
//! and at most [`MAX_PHRASE_TEXT_LEN`] bytes. A key that is defined twice keeps its
//! last definition, which is what lets a user's own document override a built-in one
//! when the two are read one after the other.
//!
//! # The table
//!
//! A table is an immutable snapshot: it is built once and swapped behind an `Arc`, so
//! a decode that is already running keeps the table it started with and never sees a
//! half-loaded one. Entries are held sorted by key with their bodies concatenated
//! into a single pool, which makes a lookup a binary search per candidate key length
//! and a hit a pair of byte ranges rather than a copy.
//!
//! # Matching
//!
//! The longest key that is a prefix of the input wins: a user who defined both `rq`
//! and `rqq` means the longer one when they type it. The input a phrase is matched
//! against is the normalized one -- lower case, `'` kept -- and a phrase is matched
//! even when that input has no reading at all, because a shorthand such as `rq` is
//! not a syllable sequence, and that is exactly the input a phrase exists for.
//!
//! # Concurrency
//!
//! A built table is never mutated, so it is `Send + Sync` and any number of decodes
//! may read one at once. Rebuilding it is a load-time job: a reload parses the
//! document into a fresh table and replaces the pointer the decode path holds, which
//! is what keeps a reload from disturbing an input session in progress.

use ime_types::{Candidate, CandidateSource, ImeError};

use crate::segment::{MAX_SYLLABLE_LEN, lookup};

/// Longest key a phrase may have, in bytes.
///
/// A key is typed, not read: it is a handful of letters, and a key long enough to be
/// a sentence is a word the dictionary should hold instead. The bound is what lets a
/// match walk its candidate key lengths without a second guard.
pub const MAX_PHRASE_KEY_LEN: usize = 32;

/// Longest phrase body, in bytes.
///
/// An identity number, an address or a bank account fits with room to spare, and the
/// bound keeps a file that is not a phrase table from being loaded as one.
pub const MAX_PHRASE_TEXT_LEN: usize = 256;

/// The diagnostic a loader reports when the phrase table is missing or unreadable.
///
/// A degradation, not a failure: without a phrase table the input method is fully
/// usable, so this is an `info`-level diagnostic rather than an error a caller has to
/// handle. The spelling is registered in the design's runtime-diagnostic table and
/// must not be reworded; it has no `ImeError` variant for that reason.
pub const PHRASE_TABLE_UNAVAILABLE: &str = "phrase/table-unavailable";

/// The annotation a phrase candidate carries.
///
/// The window draws it with the token it draws every other annotation with (C-6): a
/// phrase is a candidate, not another kind of thing, so it introduces no colour and
/// changes no cell size.
pub const PHRASE_ANNOTATION: &str = "短语";

/// The configuration key a malformed phrase document is reported against.
///
/// The document is the file `phrases.file` names, so that is the key a diagnostic
/// points at; a document that cannot be read is never reported as a failure of the
/// phrase engine itself.
const PHRASES_FILE_KEY: &str = "phrases.file";

/// How far above the candidate that follows it a phrase is scored.
///
/// The float in a candidate is display-only and decides no order; it is raised by a
/// point so that what a window shows does not disagree with the list it is showing.
const PHRASE_SCORE_MARGIN: f32 = 1.0;

/// Every phrase a user defined, sorted by key and ready to match.
///
/// Built once per load and read by every decode, so it is immutable in use. See the
/// module documentation for the document it is built from.
#[derive(Clone, Debug, Default)]
pub struct PhraseTable {
    /// Entries in ascending key order, which is what makes a lookup a binary search.
    entries: Vec<Entry>,
    /// Every body, concatenated in entry order. A body is read as a slice of this pool
    /// rather than owned by its entry, so a table of five thousand phrases is one
    /// allocation for the text and not five thousand.
    pool: String,
}

/// One entry of the table.
#[derive(Clone, Debug)]
struct Entry {
    /// The key, folded to lower case.
    key: Box<str>,
    /// Start of the body in [`PhraseTable::pool`].
    start: u32,
    /// End of the body in [`PhraseTable::pool`].
    end: u32,
    /// Syllables the key spells, for the candidate's syllable accounting.
    syllables: u16,
}

/// One phrase that matched an input.
///
/// The two ranges name the match without copying anything: the key is a byte range of
/// the input that was matched, and the body is a byte range of the table's own pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhraseHit {
    /// Byte range of the matched key inside the raw input.
    pub key_start: u16,
    /// One past the last byte of the matched key.
    pub key_end: u16,
    /// Byte range of the body inside the table's pool.
    pub text_start: u32,
    /// One past the last byte of the body.
    pub text_end: u32,
}

/// What reading a phrase document produced.
///
/// The counts a loader reports: how much of the user's document became a table, and
/// what was left behind. A document that is refused outright has no report -- the
/// refusal names the line it stopped on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhraseReport {
    /// Entries the resulting table holds.
    pub loaded: usize,
    /// Rows that could not be used and were skipped.
    pub skipped: usize,
    /// Rows that replaced an earlier definition of the same key.
    pub overridden: usize,
    /// Rows dropped because the table had reached its entry limit.
    pub over_limit: usize,
}

impl PhraseTable {
    /// Builds a table from the text of a phrase document, refusing a row it cannot use.
    ///
    /// This is the form a caller validating a file a user just wrote wants: the answer
    /// names the line that is wrong instead of quietly leaving it out. A loader that
    /// must keep the input method working under any document uses [`PhraseTable::load`]
    /// instead, which skips what it cannot use and counts it.
    ///
    /// # Parameters
    ///
    /// - `text`: the contents of a phrase document, as the module documentation
    ///   describes it.
    /// - `max_entries`: the most entries the table may hold. Rows past it are refused,
    ///   so a document that is not a phrase table at all cannot fill memory.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming `phrases.file` for the first row that cannot
    /// be used -- a line that is not two tab-separated fields, a key outside the key
    /// alphabet or past [`MAX_PHRASE_KEY_LEN`], an empty body or one past
    /// [`MAX_PHRASE_TEXT_LEN`] -- and for the first row past `max_entries`. The reason
    /// carries the line number and never the line's text, which may be the user's own.
    ///
    /// # Panics
    ///
    /// Never: every row is read with a checked split and a checked slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::phrase::PhraseTable;
    ///
    /// let table = PhraseTable::parse("rq\t2026-09-29\n", 100);
    /// assert!(table.is_ok());
    /// let refused = PhraseTable::parse("rq 2026-09-29\n", 100);
    /// assert!(refused.is_err());
    /// ```
    pub fn parse(text: &str, max_entries: usize) -> Result<Self, ImeError> {
        let (rows, report, fault) = read_rows(text, max_entries, true);
        match fault {
            Some(error) => Err(error),
            None => Ok(assemble(rows, report).0),
        }
    }

    /// Builds a table from the text of a phrase document, skipping rows it cannot use.
    ///
    /// This is the degradation path: a document with a broken line, a key outside the
    /// alphabet or a body that is too long still yields the entries that were usable,
    /// and the report says how many were not. It never fails, because a phrase table
    /// the user can live without must not be able to stop the input method from
    /// starting.
    ///
    /// # Parameters
    ///
    /// - `text`: the contents of a phrase document.
    /// - `max_entries`: the most entries the table may hold; rows past it are dropped
    ///   and counted in [`PhraseReport::over_limit`].
    ///
    /// # Returns
    ///
    /// The table and the counts of what was read. A missing file is an empty document,
    /// which answers an empty table and a zero report; the loader reports
    /// [`PHRASE_TABLE_UNAVAILABLE`] for it, once, at the point where it knows the file
    /// is absent.
    ///
    /// # Errors
    ///
    /// None: every row that cannot be used is counted rather than propagated.
    ///
    /// # Panics
    ///
    /// Never: every row is read with a checked split and a checked slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::phrase::PhraseTable;
    ///
    /// let (table, report) = PhraseTable::load("# a comment\nrq\t2026-09-29\nbroken\n", 100);
    /// assert_eq!(report.loaded, 1);
    /// assert_eq!(report.skipped, 1);
    /// let hit = table.longest_match("rq", 0);
    /// assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29"));
    /// ```
    pub fn load(text: &str, max_entries: usize) -> (Self, PhraseReport) {
        let (rows, report, _) = read_rows(text, max_entries, false);
        assemble(rows, report)
    }

    /// Builds an empty table, which matches nothing.
    ///
    /// The state a table is in when there is no phrase document: the phrase engine
    /// degrades to it rather than to an error, so an input method with no phrases
    /// behaves exactly like one whose user defined none.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Looks up the longest key that is a prefix of `raw` starting at `at`.
    ///
    /// Longest-match rather than shortest: a user who defines both `rq` and `rqq`
    /// expects the longer one to win when they type it.
    ///
    /// # Parameters
    ///
    /// - `raw`: the normalized input, spelled the way the segmentation graph spells
    ///   it: lower case, with any `'` the user typed kept.
    /// - `at`: the byte offset to match from. A key is matched at the offset it is
    ///   asked about, so a caller that has already consumed part of the input asks
    ///   about the rest.
    ///
    /// # Returns
    ///
    /// The match, or `None` when no key of the table is a prefix of `raw` at `at`.
    ///
    /// # Errors
    ///
    /// None. A table that could not be read is an empty table, which answers `None`
    /// rather than an error no caller could act on; the degradation is reported once,
    /// by the loader that knows the file is absent, as [`PHRASE_TABLE_UNAVAILABLE`].
    ///
    /// # Panics
    ///
    /// Never: an offset past the end of `raw`, and a candidate length that is not a
    /// character boundary, are both answered with `None`.
    pub fn longest_match(&self, raw: &str, at: usize) -> Option<PhraseHit> {
        let rest = raw.get(at..)?;
        let mut len = rest.len().min(MAX_PHRASE_KEY_LEN);
        while len > 0 {
            if rest.is_char_boundary(len) {
                let key = rest.get(..len)?;
                if let Ok(index) = self.entries.binary_search_by(|entry| (*entry.key).cmp(key)) {
                    let entry = self.entries.get(index)?;
                    return Some(PhraseHit {
                        key_start: u16::try_from(at).ok()?,
                        key_end: u16::try_from(at.saturating_add(len)).ok()?,
                        text_start: entry.start,
                        text_end: entry.end,
                    });
                }
            }
            len -= 1;
        }
        None
    }

    /// Returns the body a hit matched, as a slice of the table's own pool.
    ///
    /// # Returns
    ///
    /// The text the key commits, borrowed from the table: nothing is copied here, and
    /// the caller owns the copy it makes when it builds a candidate.
    ///
    /// # Errors
    ///
    /// None. A hit that does not name an entry of this table -- one taken from another
    /// table, or built by hand -- answers an empty string.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn text(&self, hit: PhraseHit) -> &str {
        match self.entry_of(hit) {
            Some(entry) => self
                .pool
                .get(usize::from(entry.start)..usize::from(entry.end))
                .unwrap_or(""),
            None => "",
        }
    }

    /// Returns how many entries the table holds.
    ///
    /// Reported by the diagnostics surface, so that a user can see whether their
    /// document was read at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn len(&self) -> u64 {
        u64::try_from(self.entries.len()).unwrap_or(u64::MAX)
    }

    /// Returns whether the table holds no phrase at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns how many syllables the key of `hit` spells.
    ///
    /// A phrase consumes the syllables of its own key, which is what a commit of it
    /// leaves of the input behind. A key no reading spells -- the usual case, since a
    /// shorthand is not a syllable sequence -- counts its characters instead, which is
    /// the number of units the preedit shows for an input the decoder cannot segment.
    ///
    /// # Panics
    ///
    /// Never: a hit that does not name an entry answers zero.
    fn syllables(&self, hit: PhraseHit) -> u16 {
        self.entry_of(hit).map_or(0, |entry| entry.syllables)
    }

    /// Returns the entry a hit names, or `None` when the hit came from another table.
    ///
    /// The pool holds the bodies in entry order, so a body's start offset is ascending
    /// and names exactly one entry. Both ends of the range are checked, so a hit built
    /// against a different table answers `None` rather than a body of the wrong length.
    fn entry_of(&self, hit: PhraseHit) -> Option<&Entry> {
        self.entries
            .binary_search_by_key(&hit.text_start, |entry| entry.start)
            .ok()
            .and_then(|index| self.entries.get(index))
            .filter(|entry| entry.end == hit.text_end)
    }
}

/// Appends the phrase candidate that matches the raw input, ahead of the rest.
///
/// Phrases are injected after the decoder has produced its word candidates and never
/// as lattice edges: a phrase has no reading and therefore no place in a scoring pass
/// that is defined over readings. The candidate goes first because a user who defined
/// `rq` means it -- an explicit definition outranks anything statistics learned.
///
/// A candidate the dictionary produced with the very text of the phrase gives way to
/// it rather than standing beside it, so the window never shows the same text twice.
///
/// # Parameters
///
/// - `table`: the phrase table to match against.
/// - `raw`: the normalized input, as [`PhraseTable::longest_match`] documents it.
/// - `candidates`: the candidate list, best first. A match is inserted at its head and
///   every candidate is renumbered, because the display number is the position.
///
/// # Returns
///
/// How many candidates were injected: `1` on a match and `0` otherwise. A caller that
/// also bounds the list must cut it after this call, since the phrase is at the head
/// and the list is one longer than it was.
///
/// # Errors
///
/// None. An empty table -- what a missing or unusable document leaves behind -- injects
/// nothing, which is the degradation the phrase engine promises rather than a failure
/// the caller has to handle.
///
/// # Panics
///
/// Never: the match is taken with a checked lookup and every byte range comes from the
/// table itself.
///
/// # Examples
///
/// ```
/// use ime_core::phrase::{PhraseTable, inject_phrase_candidates};
/// use ime_types::{Candidate, CandidateSource};
///
/// let (table, _) = PhraseTable::load("rq\t2026-09-29\n", 100);
/// let mut candidates = vec![Candidate {
///     index: 1,
///     text: String::from("renqi"),
///     annotation: None,
///     source: CandidateSource::Dict,
///     score: 0.0,
///     consumed_syllables: 2,
/// }];
/// assert_eq!(inject_phrase_candidates(&table, "rq", &mut candidates), 1);
/// assert_eq!(candidates[0].text, "2026-09-29");
/// assert_eq!(candidates[0].source, CandidateSource::Phrase);
/// assert_eq!(candidates[1].index, 2);
/// ```
pub fn inject_phrase_candidates(
    table: &PhraseTable,
    raw: &str,
    candidates: &mut Vec<Candidate>,
) -> usize {
    let Some(hit) = table.longest_match(raw, 0) else {
        return 0;
    };
    let text = table.text(hit);
    if text.is_empty() {
        return 0;
    }
    candidates.retain(|held| held.text != text);
    let score = candidates.first().map_or(0.0, |held| held.score) + PHRASE_SCORE_MARGIN;
    candidates.insert(
        0,
        Candidate {
            index: 1,
            text: text.to_owned(),
            annotation: Some(String::from(PHRASE_ANNOTATION)),
            source: CandidateSource::Phrase,
            score,
            consumed_syllables: table.syllables(hit),
        },
    );
    for (position, held) in candidates.iter_mut().enumerate() {
        held.index = u16::try_from(position).unwrap_or(u16::MAX).saturating_add(1);
    }
    1
}

/// One row of a phrase document, as the reader left it.
#[derive(Clone, Debug)]
struct RawRow {
    /// The key, folded to lower case.
    key: Box<str>,
    /// The text the key commits.
    text: Box<str>,
    /// Syllables the key spells.
    syllables: u16,
}

/// Why one row of a phrase document cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowFault {
    /// The line is not two tab-separated fields.
    Shape,
    /// The key is empty, or holds a character outside the key alphabet.
    KeyAlphabet,
    /// The key is longer than [`MAX_PHRASE_KEY_LEN`].
    KeyTooLong,
    /// The body is empty.
    EmptyText,
    /// The body is longer than [`MAX_PHRASE_TEXT_LEN`].
    TextTooLong,
    /// The table already holds `max_entries` rows.
    OverLimit,
}

impl RowFault {
    /// The reason a diagnostic carries for this fault, with the line it was on.
    ///
    /// The reason states what is wrong with the row and never quotes it: a phrase
    /// document is the user's own text, and a diagnostic is not a place to copy it to.
    fn reason(self, line: usize, max_entries: usize) -> String {
        match self {
            Self::Shape => format!("line {line}: not two tab-separated fields"),
            Self::KeyAlphabet => {
                format!("line {line}: the key is empty or holds a character outside a-z and '")
            }
            Self::KeyTooLong => {
                format!("line {line}: the key is longer than {MAX_PHRASE_KEY_LEN} bytes")
            }
            Self::EmptyText => format!("line {line}: the phrase is empty"),
            Self::TextTooLong => {
                format!("line {line}: the phrase is longer than {MAX_PHRASE_TEXT_LEN} bytes")
            }
            Self::OverLimit => {
                format!("line {line}: the table is already at its limit of {max_entries} entries")
            }
        }
    }
}

/// Reads the rows of a phrase document.
///
/// One walk serves both entry points because they differ in a single branch: what an
/// unusable row costs. In `strict` mode the first one ends the walk with its
/// diagnostic; otherwise it is counted and the document is read to its end, which is
/// what a loader that must not fail does with a file a user edited by hand.
///
/// # Returns
///
/// The rows that were usable, what reading them produced, and -- in `strict` mode --
/// the diagnostic of the first row that was not.
fn read_rows(
    text: &str,
    max_entries: usize,
    strict: bool,
) -> (Vec<RawRow>, PhraseReport, Option<ImeError>) {
    let mut rows: Vec<RawRow> = Vec::new();
    let mut report = PhraseReport::default();
    for (index, line) in text.lines().enumerate() {
        let number = index.saturating_add(1);
        let fault = match row_of(line) {
            Ok(None) => continue,
            Ok(Some(row)) => {
                if rows.len() < max_entries {
                    rows.push(row);
                    continue;
                }
                RowFault::OverLimit
            }
            Err(fault) => fault,
        };
        if strict {
            let reason = fault.reason(number, max_entries);
            let error = ImeError::ConfigInvalid {
                key: String::from(PHRASES_FILE_KEY),
                reason,
            };
            return (rows, report, Some(error));
        }
        match fault {
            RowFault::OverLimit => report.over_limit = report.over_limit.saturating_add(1),
            _ => report.skipped = report.skipped.saturating_add(1),
        }
    }
    (rows, report, None)
}

/// Splits one line of a phrase document.
///
/// # Returns
///
/// `Ok(None)` for a blank line and for a comment, `Ok(Some(row))` for a usable row,
/// and the fault of the first row that is not.
fn row_of(line: &str) -> Result<Option<RawRow>, RowFault> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut fields = line.split('\t');
    let Some(key) = fields.next() else {
        return Err(RowFault::Shape);
    };
    let Some(text) = fields.next() else {
        return Err(RowFault::Shape);
    };
    if fields.next().is_some() {
        return Err(RowFault::Shape);
    }
    // The key is folded rather than rejected: a document a user typed by hand may
    // spell a key in upper case, and the canonical form is the lower-case one the
    // match folds the input to.
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() || !key.bytes().all(is_key_byte) {
        return Err(RowFault::KeyAlphabet);
    }
    if key.len() > MAX_PHRASE_KEY_LEN {
        return Err(RowFault::KeyTooLong);
    }
    if text.is_empty() {
        return Err(RowFault::EmptyText);
    }
    if text.len() > MAX_PHRASE_TEXT_LEN {
        return Err(RowFault::TextTooLong);
    }
    Ok(Some(RawRow {
        syllables: count_syllables(&key),
        key: key.into_boxed_str(),
        text: text.into(),
    }))
}

/// Answers whether one byte may appear in a phrase key.
///
/// The alphabet is the input alphabet minus its upper case: the letters a user types
/// and the `'` that pins a syllable boundary. A digit, a space or a non-ASCII byte is
/// not a shortcut and is refused while the document is read, so it can never reach a
/// lookup that would compare against it forever without matching.
fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte == b'\''
}

/// Sorts the rows, resolves repeated keys and builds the table.
///
/// The sort is stable, so the definitions of one key stay in the order the document
/// gave them and the last of them is the one that survives. That single rule is what
/// lets a user's own table override a built-in one: the loader reads the built-in
/// document and then the user's, and the later definition wins.
fn assemble(mut rows: Vec<RawRow>, mut report: PhraseReport) -> (PhraseTable, PhraseReport) {
    rows.sort_by(|left, right| (*left.key).cmp(&*right.key));
    let mut unique: Vec<RawRow> = Vec::with_capacity(rows.len());
    for row in rows {
        match unique.last_mut() {
            Some(previous) if previous.key == row.key => {
                *previous = row;
                report.overridden = report.overridden.saturating_add(1);
            }
            _ => unique.push(row),
        }
    }
    let mut pool = String::new();
    let mut entries = Vec::with_capacity(unique.len());
    for row in unique {
        // The pool is bounded by `max_entries * MAX_PHRASE_TEXT_LEN`, so the offsets
        // of a table that was read at all fit a `u32`; the saturation is what a
        // document past that bound would need, and it cannot be built.
        let start = u32::try_from(pool.len()).unwrap_or(u32::MAX);
        pool.push_str(&row.text);
        let end = u32::try_from(pool.len()).unwrap_or(u32::MAX);
        entries.push(Entry {
            key: row.key,
            start,
            end,
            syllables: row.syllables,
        });
    }
    report.loaded = entries.len();
    (PhraseTable { entries, pool }, report)
}

/// Counts the syllables a key spells.
///
/// A phrase key is a shortcut, and a shortcut is usually not a reading: `rq` is not a
/// sequence any segmentation accepts. The count is therefore the greedy longest-match
/// over the syllable alphabet, and a key no reading spells falls back to its own
/// character count -- the number of units the preedit shows for an input the decoder
/// cannot segment at all.
fn count_syllables(key: &str) -> u16 {
    let mut at = 0usize;
    let mut syllables = 0u16;
    while at < key.len() {
        // The apostrophe pins a boundary; it is not a sound of its own.
        if key.as_bytes().get(at) == Some(&b'\'') {
            at = at.saturating_add(1);
            continue;
        }
        match longest_syllable_at(key, at) {
            Some(len) => {
                at = at.saturating_add(len);
                syllables = syllables.saturating_add(1);
            }
            None => return u16::try_from(key.chars().count()).unwrap_or(u16::MAX),
        }
    }
    syllables
}

/// Returns the length of the longest syllable the alphabet spells at `at`.
///
/// # Returns
///
/// The length in bytes, or `None` when no syllable starts there.
fn longest_syllable_at(key: &str, at: usize) -> Option<usize> {
    for len in (1..=MAX_SYLLABLE_LEN).rev() {
        let Some(text) = key.get(at..at.saturating_add(len)) else {
            continue;
        };
        if lookup(text).is_some() {
            return Some(len);
        }
    }
    None
}

#[cfg(test)]
mod tests;
