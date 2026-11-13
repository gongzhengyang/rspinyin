//! Reverse lookup by decomposition: the `u`-led domain that finds a character
//! whose shape the user can name but whose reading they cannot.
//!
//! Responsibility: describe the dictionary-side reverse-index contract
//! ([`ReverseSource`]), the whole-buffer domain decision ([`recognize`]) a
//! router consults before it may hand the buffer to this domain, and the
//! candidate rendering ([`candidates`]) the panel displays.
//!
//! The domain is the query-direction mirror of [`crate::aux`]: an aux session
//! filters candidates the decoder already produced by their recorded codes,
//! while a reverse query knows no reading at all and asks the dictionary which
//! characters decompose into the components the user spelled. Both consume the
//! same decomposition table, and both match codes in a fixed order: the typed
//! payload must be a subsequence of a character's decomposition code, so the
//! order the user spelled the components in is preserved and a payload can
//! never match a character whose components appear in a different order.
//!
//! # Case split with the codepoint domain
//!
//! Lowercase `u` followed by one to four letters selects this domain;
//! uppercase `U` followed by hex digits selects the codepoint domain
//! ([`crate::quick`]). The split is by the trigger's case, and the two
//! recognizers are deliberately not disjoint on their own: a payload of pure
//! hex letters (such as `uab`) matches both the codepoint spelling and this
//! domain, which is why the router must consult [`recognize`] *before* the
//! quick trigger table for buffers that start with a lowercase `u`. A payload
//! containing a digit (such as `u4f60`) never selects this domain, so the
//! lowercase codepoint spelling keeps working for numeric payloads.
//!
//! Boundary: this module performs no dictionary access and no decoding. The
//! router resolves hits through [`ReverseSource`] once per keystroke; the
//! rendering below is pure string work over borrowed rows.

use ime_types::DictError;

use crate::quick::QuickCandidate;

/// The stable diagnostic code emitted once per process when a `u`-payload is
/// typed but the active container carries no reverse index
/// (`domain/action/reason` form; the router owns the emission, this module owns
/// the code).
pub const ABSENT_INDEX_CODE: &str = "reverse/absent";

/// The trigger character that opens the domain: a lowercase `u`.
///
/// The uppercase spelling belongs to the codepoint domain; see the module
/// documentation for the case split.
pub const TRIGGER: char = 'u';

/// The longest payload the domain accepts, in letters.
///
/// A decomposition code is at most four letters, and a payload longer than
/// that cannot be a subsequence of any code. Past this bound the buffer is
/// ordinary input again, mirroring the codepoint domain's echo bound.
pub const MAX_PAYLOAD_LEN: usize = 4;

/// The most candidates one reverse query renders.
///
/// A query on a broad fragment (a single letter that occurs in most
/// decompositions) can match hundreds of characters. The bound keeps the
/// panel's memory bounded at the cost of hiding the tail of such a list; with
/// the table at its planned scale a full page-through of a broad query stays
/// far inside the quick-domain budget, so the bound is a safety rail rather
/// than a working limit.
pub const MAX_CANDIDATES: usize = 256;

/// A whole-buffer domain decision: the buffer names a reverse query.
///
/// `trigger_len` is always one (`TRIGGER` is a single character) and
/// `payload_len` counts the payload letters after it, so a caller can name the
/// typed fragment and its byte range without re-parsing the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReverseMatch {
    /// The number of bytes in the trigger itself.
    pub trigger_len: usize,
    /// The number of bytes in the payload letters after the trigger.
    pub payload_len: usize,
}

/// Decides whether the raw buffer names a reverse-decomposition query.
///
/// The domain is `u` followed by one to [`MAX_PAYLOAD_LEN`] ASCII lowercase
/// letters. A bare `u` is not a match — it is a pinyin syllable and must keep
/// composing as ordinary input — and neither is a payload holding a digit or
/// any non-letter, which leaves the lowercase codepoint spelling (`u4f60`)
/// working. An uppercase `U` never selects this domain: that spelling belongs
/// to the codepoint domain, and the case split is the contract that keeps the
/// two recognizers composable.
///
/// # Examples
///
/// ```
/// use ime_core::reverse::{MAX_PAYLOAD_LEN, recognize};
///
/// // Letters after the lowercase trigger open the domain.
/// assert!(recognize("uhf").is_some());
/// // A bare `u` keeps composing as pinyin.
/// assert!(recognize("u").is_none());
/// // Digits stay with the codepoint domain.
/// assert!(recognize("u4f60").is_none());
/// // The uppercase spelling is the codepoint domain's.
/// assert!(recognize("U4f60").is_none());
/// // Past the payload bound the buffer is ordinary input again.
/// assert!(recognize("uhfxyz").is_none());
/// assert_eq!(MAX_PAYLOAD_LEN, 4);
/// ```
///
/// # Panics
///
/// Never panics.
pub fn recognize(buffer: &str) -> Option<ReverseMatch> {
    let payload = buffer.strip_prefix(TRIGGER)?;
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_LEN {
        return None;
    }
    if !payload.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    Some(ReverseMatch {
        trigger_len: TRIGGER.len_utf8(),
        payload_len: payload.len(),
    })
}

/// One reverse-lookup hit, borrowed from the dictionary image.
///
/// `character` is what a selection commits; `code` is the character's full
/// decomposition code, which doubles as the annotation: it shows the user
/// which component sequence matched and what they could have typed more
/// precisely. The code is built from component readings' first letters, so it
/// describes the decomposition rather than the character's own reading — for a
/// polyphonic character the annotation stays the decomposition code, which is
/// what keeps it stable across the character's readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReverseHit<'a> {
    /// The character a selection commits.
    pub character: char,
    /// The character's decomposition code, borrowed from the image.
    pub code: &'a str,
}

/// The dictionary-side view of the compiled reverse index.
///
/// The index is a build-time materialization of the decomposition table: every
/// non-empty subsequence of every code is a key that points at the characters
/// whose code contains it in order. A query is therefore one binary search and
/// one posting slice, and a container without the index block is exactly the
/// pre-reverse layout.
///
/// # Concurrency
///
/// Implementations are `Send + Sync`, as the `Sync` supertrait requires. The
/// methods are bounded reads over an immutable mapped image: they never block,
/// never mutate, and never log the payload or the hit text.
pub trait ReverseSource: Sync {
    /// Returns `true` when the backing store carries a reverse index at all.
    ///
    /// This is the gate the router checks before it claims a `u`-payload;
    /// `false` means the buffer must fall through to the ordinary decode and
    /// the [`ABSENT_INDEX_CODE`] diagnostic is owed (once per process).
    fn has_reverse_index(&self) -> bool;

    /// Returns the characters whose decomposition code contains `payload` as a
    /// subsequence, in container order (ascending codepoint), each with its
    /// full code.
    ///
    /// A payload the domain recognizer would not accept — empty, longer than
    /// four letters, or holding anything but lowercase letters — is answered
    /// with an empty list rather than an error: it names no key, and the
    /// caller's own recognizer is the boundary that should have stopped it.
    ///
    /// # Errors
    /// Returns [`DictError`] when the index is absent or the backing store is
    /// internally inconsistent, which a container that passed structural
    /// verification cannot be; a router surfaces the error rather than
    /// treating it as a miss.
    fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError>;
}

/// Renders the reverse candidates a query's hits answer with.
///
/// Each hit becomes one panel candidate: the character as the commit text and
/// its decomposition code as the annotation. The list is truncated at
/// [`MAX_CANDIDATES`]; the hits come from [`ReverseSource::reverse_lookup`] in
/// container order, so the truncation is deterministic.
///
/// # Panics
///
/// Never panics.
pub fn candidates(hits: &[ReverseHit<'_>]) -> Vec<QuickCandidate> {
    hits.iter()
        .take(MAX_CANDIDATES)
        .map(|hit| QuickCandidate {
            text: String::from(hit.character),
            annotation: Some(String::from(hit.code)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
