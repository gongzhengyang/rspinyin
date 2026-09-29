//! Double-pinyin schemes: the key tables, the syllable mapping, and the rewrite that
//! turns a scheme's keystrokes into full pinyin.
//!
//! Responsibility: own the five scheme tables this build ships, map a scheme's one-
//! or two-key syllable onto an entry of the segmentation layer's syllable table, and
//! rewrite a whole keystroke stream into the full-pinyin spelling the segmentation
//! layer reads.
//!
//! Boundaries: a rewrite is a pure function of its arguments -- no file, no clock, no
//! environment, no global state -- which is what keeps the decoder's determinism
//! guarantee intact (0.4 rule 4). The segmentation layer never learns that schemes
//! exist: it is defined over the 411-syllable table, and a scheme is a different
//! encoding of the same syllables, so rewriting *before* segmentation leaves the DAG,
//! the lattice, the Viterbi pass and the preedit builder untouched.
//!
//! # Where a scheme sits in the pipeline
//!
//! ```text
//! raw keystrokes -> rewrite_for_scheme -> SyllableDag::build -> lattice -> candidates
//! ```
//!
//! # Index semantics
//!
//! One scheme syllable produces exactly one full-pinyin syllable, so the syllable
//! *index* is the same on both sides of the rewrite; only the *byte offsets* differ,
//! because two keys expand into up to six letters. [`SchemeMap`] carries the
//! alignment, so the engine can report a [`Segment`](ime_types::Segment) span in
//! scheme syllables -- what the user actually typed, and what the window's preedit
//! highlights -- instead of in full pinyin the user never sees.
//!
//! # The key alphabet
//!
//! A scheme is indexed by `a`-`z` plus `;`. The semicolon is a key in its own right:
//! the published Microsoft, Sogou and Ziguang layouts all carry `ing` on it, and a
//! 26-letter table could not express those three schemes without either losing every
//! `-ing` syllable or losing `lü` and `nü`, which the same key would then have to
//! hold. This project's input buffer does not accept `;` today, so that column is
//! currently unreachable from the keyboard; the tables stay faithful to the published
//! layouts rather than dropping a key the schemes define.

pub mod microsoft;
pub mod sogou;
pub mod xiaohe;
pub mod ziguang;
pub mod ziranma;

#[cfg(test)]
mod tests;

use std::borrow::Cow;

use ime_types::{DecodeFlags, DecodeRequest, ImeError, SchemeId, SyllableId};

use crate::segment::syllable::{MAX_RAW_LEN, lookup, syllable_at};

/// Number of keys one scheme table is indexed by: `a`-`z` and `;`.
pub const SCHEME_KEYS: usize = 27;

/// Slot of the `;` key in a scheme table.
pub const SEMICOLON_INDEX: usize = 26;

/// Returns the table slot a scheme key occupies.
///
/// # Returns
///
/// `Some(slot)` for `a`-`z` and for `;`, and `None` for any other byte. The caller
/// decides whether the scheme actually defines the key; an undefined key is not part
/// of that scheme's alphabet.
///
/// # Panics
///
/// Never.
pub const fn key_index(key: u8) -> Option<usize> {
    match key {
        // `usize::from` is not callable in a `const fn` on this toolchain (`From` is not
        // yet a const trait), so the widening is written as a cast. It is total: the arm
        // matches only `a`..=`z`, so the difference is 0..=25.
        b'a'..=b'z' => Some((key - b'a') as usize),
        b';' => Some(SEMICOLON_INDEX),
        _ => None,
    }
}

/// One double-pinyin scheme: the letter that stands for each syllable initial and
/// each syllable final.
///
/// The tables are `const` because a scheme is a published standard, not a user
/// preference. The one scheme that *is* a user preference -- a custom table -- is
/// built at runtime from the configuration and has the same shape.
///
/// A syllable is one or two keys: either a key that stands alone
/// ([`standalone`](SchemeTable::standalone)), or an initial key followed by a final
/// key. The full-pinyin spelling is the initial joined to the final, except that a
/// final which already begins with its initial is used as it stands -- which is how
/// the vowel-initial schemes write `an` as the `a` key followed by the `an` key
/// without producing `aan`.
#[derive(Clone, Copy, Debug)]
pub struct SchemeTable {
    /// Identifies the scheme in diagnostics and configuration.
    pub id: SchemeId,
    /// Human-readable name, as the scheme's publisher writes it. This is user-facing
    /// copy, so it is Chinese like the rest of the window's text.
    pub name: &'static str,
    /// `initials[slot]` is the syllable initial the key stands for.
    ///
    /// `None` means the key cannot open a syllable. `Some("")` is the *zero initial*:
    /// the key opens a vowel-initial syllable and contributes nothing to its
    /// spelling, which is how the Microsoft, Sogou and Ziguang layouts write `an` as
    /// `o` + the `an` key rather than `a` + the `an` key.
    pub initials: [Option<&'static str>; SCHEME_KEYS],
    /// `finals[slot]` lists the finals the key stands for, most common first.
    ///
    /// A key carries more than one final because the syllable table is sparse enough
    /// to tell them apart: Xiaohe's `k` is `uai` before `g` and `ing` before `t`, and
    /// only one of the two joins into a syllable either way. The list is empty for a
    /// key that carries no final at all.
    pub finals: [&'static [&'static str]; SCHEME_KEYS],
    /// `standalone[slot]` is the syllable the key is on its own.
    ///
    /// Empty means the key alone is not a syllable. `a`, `e` and `o` carry one in
    /// every scheme this build ships; the vowel-initial syllables reach their spelling
    /// through the initial-and-final path instead.
    pub standalone: [&'static str; SCHEME_KEYS],
}

impl SchemeTable {
    /// Whether the scheme defines `slot` as a key at all.
    ///
    /// A key the scheme does not define is not part of its alphabet: a `;` typed into
    /// a scheme that has no use for it is discarded rather than mapped to an empty
    /// syllable.
    ///
    /// # Panics
    ///
    /// Never.
    fn defines(&self, slot: usize) -> bool {
        self.initials.get(slot).copied().flatten().is_some()
            || self.finals.get(slot).is_some_and(|list| !list.is_empty())
            || self
                .standalone
                .get(slot)
                .is_some_and(|text| !text.is_empty())
    }
}

/// One syllable produced by a scheme mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedSyllable {
    /// Index into the segmentation layer's 411-entry syllable table; the spelling
    /// itself is [`syllable_at`](crate::segment::syllable::syllable_at) of this.
    pub full: SyllableId,
    /// How many scheme keys this syllable consumed: 1 or 2.
    pub consumed: u8,
}

/// Maps the start of a double-pinyin key sequence onto one full-pinyin syllable.
///
/// At most the first two keys are read: a syllable is one or two keys long, and the
/// caller passes the rest of the stream so that it can keep its own cursor. The
/// two-key reading is tried first, so a scheme that spells a syllable with a pair
/// takes the pair even when its first key also stands alone.
///
/// The mapping is a pure function of the scheme and the input: it reads no file, no
/// clock and no global state, which is what keeps the decoder's determinism guarantee
/// intact (0.4 rule 4).
///
/// # Returns
///
/// `Ok(Some(syllable))` when the keys spell a syllable, and `Ok(None)` when they do
/// not -- an empty slice, a key the scheme does not define, or a pair that joins into
/// something the syllable table does not hold. `None` is not a failure: the caller
/// falls back to the raw keystroke and keeps going, so that a user is never unable to
/// type.
///
/// # Errors
///
/// - [`ImeError::DecodeInvalidChar`] for a byte outside the scheme key alphabet. `at`
///   is the byte's index within `keys`.
/// - [`ImeError::SchemeUnsupported`] when the table claims a scheme this build does
///   not implement, which is a malformed table rather than a malformed keystroke.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::segment::syllable_at;
/// use ime_core::shuangpin::{map_syllable, xiaohe};
///
/// // Xiaohe spells `zhong` as `v` (zh) followed by `s` (ong).
/// let mapped = map_syllable(&xiaohe::XIAOHE, b"vs").ok().flatten();
/// assert_eq!(mapped.map(|mapped| mapped.consumed), Some(2));
/// assert_eq!(mapped.and_then(|mapped| syllable_at(mapped.full)), Some("zhong"));
///
/// // A pair the scheme cannot spell is reported, not guessed at.
/// assert_eq!(map_syllable(&xiaohe::XIAOHE, b"bz").ok().flatten(), None);
/// ```
pub fn map_syllable(table: &SchemeTable, keys: &[u8]) -> Result<Option<MappedSyllable>, ImeError> {
    if !table.id.is_known() {
        return Err(ImeError::SchemeUnsupported {
            scheme: table.id.value(),
        });
    }
    let Some(&first) = keys.first() else {
        return Ok(None);
    };
    let first_slot = slot_of(first, 0)?;
    let mut text = String::new();
    if let Some(&second) = keys.get(1) {
        let second_slot = slot_of(second, 1)?;
        if let Some(mapped) = pair_syllable(table, first_slot, second_slot, &mut text) {
            return Ok(Some(mapped));
        }
    }
    Ok(standalone_syllable(table, first_slot))
}

/// The scheme tables this build ships, in scheme-number order.
///
/// This list is the one place a scheme is registered: [`table_for`] searches it, and a
/// test walks it to check that every table identifies itself by the number it is
/// registered under. [`SchemeId::FULL`] is deliberately absent -- full pinyin is the
/// identity and needs no table.
pub static TABLES: [(SchemeId, &SchemeTable); 5] = [
    (SchemeId::XIAOHE, &xiaohe::XIAOHE),
    (SchemeId::ZIRANMA, &ziranma::ZIRANMA),
    (SchemeId::MICROSOFT, &microsoft::MICROSOFT),
    (SchemeId::SOGOU, &sogou::SOGOU),
    (SchemeId::ZIGUANG, &ziguang::ZIGUANG),
];

/// Returns the table a scheme is implemented by.
///
/// # Returns
///
/// `Ok(None)` for [`SchemeId::FULL`], which needs no table: full pinyin *is* the
/// spelling the segmentation layer reads, so the rewrite is the identity. `Ok(Some)`
/// for every scheme this build ships.
///
/// # Errors
///
/// [`ImeError::SchemeUnsupported`] when the number names a scheme a newer build wrote
/// and this one cannot honour. The caller reports `decode/scheme-unsupported` and
/// falls back to full pinyin rather than refusing to start.
///
/// # Panics
///
/// Never.
pub fn table_for(scheme: SchemeId) -> Result<Option<&'static SchemeTable>, ImeError> {
    if scheme == SchemeId::FULL {
        return Ok(None);
    }
    match TABLES.iter().find(|(id, _)| *id == scheme) {
        Some((_, table)) => Ok(Some(*table)),
        None => Err(ImeError::SchemeUnsupported {
            scheme: scheme.value(),
        }),
    }
}

/// Whether `req` asks for the double-pinyin rewrite.
///
/// The scheme and the switch are two separate facts: `scheme` says which layout the
/// keystrokes follow, [`DecodeFlags::SHUANGPIN`] says whether the caller wants them
/// rewritten. A request may carry a scheme with the switch clear -- the engine does
/// that while a full-pinyin word is typed in a mixed session -- and then the input is
/// used exactly as typed.
///
/// # Panics
///
/// Never.
pub fn is_rewrite_requested(req: &DecodeRequest) -> bool {
    req.flags.contains(DecodeFlags::SHUANGPIN) && req.scheme != SchemeId::FULL
}

/// Rewrites a decode request's input when the request asks for a scheme.
///
/// This is the engine's entry point: it honours [`DecodeFlags::SHUANGPIN`] so that a
/// caller cannot forget the switch and rewrite a full-pinyin request, and it borrows
/// the request's own buffer when no rewrite is wanted.
///
/// # Errors
///
/// The errors of [`rewrite_for_scheme`].
///
/// # Panics
///
/// Never.
pub fn rewrite_request(req: &DecodeRequest) -> Result<Cow<'_, str>, ImeError> {
    if !is_rewrite_requested(req) {
        return Ok(Cow::Borrowed(req.raw.as_str()));
    }
    rewrite_for_scheme(&req.raw, req.scheme)
}

/// Rewrites a double-pinyin keystroke stream into the full-pinyin spelling the
/// segmentation layer reads.
///
/// The segmentation layer must never learn about schemes: it is defined over the
/// 411-syllable table, and a scheme is a different encoding of the same syllables.
/// Rewriting here keeps every downstream stage -- the DAG, the lattice, the Viterbi
/// pass, the preedit builder -- unchanged.
///
/// # Returns
///
/// A borrowed slice for [`SchemeId::FULL`], which is the identity and copies nothing;
/// an owned string for a scheme. The rewritten text is a fixed point of the
/// segmentation layer's normalizer, so the graph the caller builds from it reports
/// byte offsets into exactly this string.
///
/// # Errors
///
/// - [`ImeError::SchemeUnsupported`] when `scheme` names a layout this build does not
///   implement.
/// - [`ImeError::DecodeTooLong`] when `raw` is longer than the segmentation layer's
///   hard limit. The limit is judged on the keystrokes, exactly as the segmentation
///   layer judges it on a full-pinyin input; a scheme rewrites the input rather than
///   relaxing the limit.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::shuangpin::rewrite_for_scheme;
/// use ime_types::SchemeId;
///
/// // Xiaohe: `vs` is zhong, `nihc` is ni'hao.
/// let rewritten = rewrite_for_scheme("vs", SchemeId::XIAOHE);
/// assert_eq!(rewritten.as_deref().unwrap_or(""), "zhong");
/// let rewritten = rewrite_for_scheme("nihc", SchemeId::XIAOHE);
/// assert_eq!(rewritten.as_deref().unwrap_or(""), "nihao");
///
/// // Full pinyin is the identity, and borrows rather than copying.
/// let identity = rewrite_for_scheme("nihao", SchemeId::FULL);
/// assert!(matches!(identity, Ok(std::borrow::Cow::Borrowed("nihao"))));
/// ```
pub fn rewrite_for_scheme(raw: &str, scheme: SchemeId) -> Result<Cow<'_, str>, ImeError> {
    match SchemeMap::build(raw, scheme)? {
        Some(map) => Ok(Cow::Owned(map.into_text())),
        None => Ok(Cow::Borrowed(raw)),
    }
}

/// The rewritten input, plus the alignment between the scheme's syllables and the
/// text the segmentation layer sees.
///
/// Build one with [`SchemeMap::build`] before segmentation, and use it afterwards to
/// turn the byte span of a winning path back into the scheme syllables the user
/// typed. [`rewrite_for_scheme`] is the same rewrite without the alignment, for a
/// caller that does not need it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemeMap {
    /// The full-pinyin spelling the segmentation layer reads.
    text: String,
    /// `bounds[i]` is where scheme syllable `i` starts in `text`, and `bounds[n]` is
    /// the end. A syllable the scheme has no entry for is still one entry here: the
    /// keystroke it fell back to is one syllable as far as the window is concerned.
    bounds: Vec<u16>,
    /// One flag per syllable: whether the scheme table had no entry for its keys and
    /// the raw keystroke was carried through instead.
    unmapped: Vec<bool>,
}

impl SchemeMap {
    /// Rewrites `raw` for `scheme` and records the syllable alignment.
    ///
    /// # Returns
    ///
    /// `Ok(None)` for [`SchemeId::FULL`], which rewrites nothing: the input is already
    /// full pinyin and its syllables are whatever the segmentation layer makes of it.
    /// `Ok(Some(map))` for a scheme this build ships.
    ///
    /// # Errors
    ///
    /// - [`ImeError::SchemeUnsupported`] when `scheme` names a layout this build does
    ///   not implement.
    /// - [`ImeError::DecodeTooLong`] when `raw` is longer than the segmentation
    ///   layer's hard limit.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn build(raw: &str, scheme: SchemeId) -> Result<Option<Self>, ImeError> {
        let Some(table) = table_for(scheme)? else {
            return Ok(None);
        };
        if raw.len() > MAX_RAW_LEN {
            return Err(ImeError::DecodeTooLong {
                len: raw.len(),
                max: MAX_RAW_LEN,
            });
        }
        let mut map = Self::default();
        map.rewrite(raw, table);
        Ok(Some(map))
    }

    /// Returns the full-pinyin spelling the segmentation layer reads.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Gives up the rewritten text, dropping the alignment.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn into_text(self) -> String {
        self.text
    }

    /// Returns how many scheme syllables the keystroke stream held.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn syllables(&self) -> u16 {
        u16::try_from(self.bounds.len().saturating_sub(1)).unwrap_or(u16::MAX)
    }

    /// Returns whether scheme syllable `index` had no table entry and fell back to its
    /// raw keystroke.
    ///
    /// A fallback is carried through rather than dropped so that the user always has
    /// something to commit; the decode usually degrades to the pass-through candidate
    /// when one is present, which is the honest answer for a half-typed syllable.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_unmapped(&self, index: u16) -> bool {
        self.unmapped
            .get(usize::from(index))
            .copied()
            .unwrap_or(false)
    }

    /// Returns how many scheme syllables fell back to their raw keystroke.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn unmapped_count(&self) -> u16 {
        let count = self.unmapped.iter().filter(|&&flag| flag).count();
        u16::try_from(count).unwrap_or(u16::MAX)
    }

    /// Returns the byte span scheme syllable `index` occupies in
    /// [`text`](SchemeMap::text), or `None` for an index past the last syllable.
    ///
    /// The span of a syllable that a forced `'` precedes begins on the marker, which
    /// is how the segmentation graph reports a pinned boundary too.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn full_span(&self, index: u16) -> Option<(u16, u16)> {
        let slot = usize::from(index);
        let start = *self.bounds.get(slot)?;
        let end = *self.bounds.get(slot.checked_add(1)?)?;
        Some((start, end))
    }

    /// Turns a byte span of [`text`](SchemeMap::text) back into the scheme syllables
    /// it covers.
    ///
    /// The engine calls this with the byte range of one winning segment: a segment may
    /// cover several scheme syllables, and the span it reports to the window has to be
    /// in scheme syllables because those are the keys the user typed.
    ///
    /// # Returns
    ///
    /// `Some((start, end))` as a half-open range of scheme syllable indices, and
    /// `None` when the range is empty or falls outside the rewritten text.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn scheme_span(&self, full_start: u16, full_end: u16) -> Option<(u16, u16)> {
        if full_end <= full_start || self.bounds.len() < 2 {
            return None;
        }
        // The last boundary at or before the span's start is the syllable that holds
        // it, and the first boundary strictly before the span's end closes it.
        let start = self
            .bounds
            .partition_point(|&bound| bound <= full_start)
            .saturating_sub(1);
        let end = self.bounds.partition_point(|&bound| bound < full_end);
        let last = self.bounds.len() - 1;
        if start >= end || end > last {
            return None;
        }
        Some((
            u16::try_from(start).unwrap_or(u16::MAX),
            u16::try_from(end).unwrap_or(u16::MAX),
        ))
    }

    /// Walks `raw` and fills `text`, `bounds` and `unmapped` from it.
    ///
    /// The walk reads one or two keys at a time. A key the scheme does not define, and
    /// any character that is not a key at all, is discarded -- the segmentation
    /// layer's normalizer discards the same set, so discarding here keeps `text` a
    /// fixed point of it and this map's offsets the graph's own. An apostrophe is a
    /// forced boundary: it is kept when it can carry information and folded away when
    /// it cannot, exactly as the normalizer folds a leading, doubled or trailing one.
    fn rewrite(&mut self, raw: &str, table: &SchemeTable) {
        self.text.clear();
        self.bounds.clear();
        self.unmapped.clear();
        self.bounds.push(0);
        // One buffer for the whole walk: composing a syllable must not allocate once
        // per keystroke.
        let mut scratch = String::new();
        let mut at = 0usize;
        let mut pending_marker = false;
        while at < raw.len() {
            let rest = &raw[at..];
            let Some(ch) = rest.chars().next() else {
                break;
            };
            let width = ch.len_utf8();
            if ch == '\'' {
                pending_marker = true;
                at += width;
                continue;
            }
            let Some(slot) = scheme_slot(ch).filter(|&slot| table.defines(slot)) else {
                at += width;
                continue;
            };
            let tail = &rest[width..];
            let second_width = tail.chars().next().map_or(0, char::len_utf8);
            let second = tail
                .chars()
                .next()
                .and_then(scheme_slot)
                .filter(|&slot| table.defines(slot));
            let paired = second.and_then(|second| pair_syllable(table, slot, second, &mut scratch));
            let mapped = match paired {
                Some(mapped) => Some(mapped.full),
                None => standalone_syllable(table, slot).map(|mapped| mapped.full),
            };
            self.flush_marker(&mut pending_marker);
            let text = mapped.and_then(syllable_at);
            // The scheme had no entry for these keys. Carry the keystroke through
            // rather than dropping it, so that the input stays visible and
            // committable -- but only when the segmentation layer will read the byte
            // back unchanged. That rules out a non-letter like `;`, which the
            // normalizer discards, and the letter `v`, which it folds onto `ü`: either
            // one would change the text's length behind the caller's back and move
            // every offset after it.
            //
            // The `v` test is case-insensitive on purpose. The keystroke is pushed in
            // lower case below, so testing `ch != 'v'` here would let an upper-case `V`
            // through and then write a bare `v` into the text -- precisely the byte the
            // normalizer folds, which is the thing this exclusion exists to prevent.
            let carried =
                text.is_none() && ch.is_ascii_alphabetic() && !ch.eq_ignore_ascii_case(&'v');
            if let Some(spelling) = text {
                self.text.push_str(spelling);
            } else if carried {
                self.text.push(ch.to_ascii_lowercase());
            }
            if text.is_some() || carried {
                self.unmapped.push(carried);
                let end = u16::try_from(self.text.len()).unwrap_or(u16::MAX);
                self.bounds.push(end);
            }
            at += width;
            if paired.is_some() {
                at += second_width;
            }
        }
    }

    /// Emits a pending forced-boundary marker when the text can carry one.
    ///
    /// A leading marker, a doubled one and a trailing one carry no information, and
    /// the segmentation layer's normalizer folds them away. Folding them here too is
    /// what keeps `text` a fixed point of that normalizer.
    fn flush_marker(&mut self, pending: &mut bool) {
        if std::mem::take(pending) && !self.text.is_empty() && !self.text.ends_with('\'') {
            self.text.push('\'');
        }
    }
}

/// Returns the scheme slot a character stands for.
///
/// Letters are folded to lower case first, so an upper-case keystroke reaches the
/// same slot as the lower-case one. Returns `None` for anything that is not a key.
fn scheme_slot(ch: char) -> Option<usize> {
    u8::try_from(ch.to_ascii_lowercase())
        .ok()
        .and_then(key_index)
}

/// Maps an initial key and a final key onto a syllable, trying every final the key
/// stands for in the order the table lists them.
fn pair_syllable(
    table: &SchemeTable,
    initial_slot: usize,
    final_slot: usize,
    scratch: &mut String,
) -> Option<MappedSyllable> {
    let initial = table.initials.get(initial_slot).copied().flatten()?;
    let finals = table.finals.get(final_slot)?;
    for final_text in *finals {
        if let Some(full) = compose_into(initial, final_text, scratch) {
            return Some(MappedSyllable { full, consumed: 2 });
        }
    }
    None
}

/// Returns the syllable a key stands for on its own, if the scheme gives it one.
fn standalone_syllable(table: &SchemeTable, slot: usize) -> Option<MappedSyllable> {
    let text = table.standalone.get(slot).copied().unwrap_or("");
    if text.is_empty() {
        return None;
    }
    lookup(text).map(|full| MappedSyllable { full, consumed: 1 })
}

/// Joins one initial and one final into a spelling the syllable table holds.
///
/// The joining rule is what makes the vowel-initial schemes work. Xiaohe writes `an`
/// as the `a` key followed by the `an` key; joining naively would produce `aan`, so a
/// final that already begins with its initial is used as it stands. The same rule
/// covers the zero initial, where the initial is the empty string.
///
/// `scratch` is cleared first and then filled, so a caller that keeps one string
/// across syllables never reallocates.
fn compose_into(initial: &str, final_text: &str, scratch: &mut String) -> Option<SyllableId> {
    scratch.clear();
    if initial.is_empty() || final_text.starts_with(initial) {
        scratch.push_str(final_text);
    } else {
        scratch.push_str(initial);
        scratch.push_str(final_text);
    }
    lookup(scratch)
}

/// Returns the table slot `key` occupies, or the contract's invalid-character error.
fn slot_of(key: u8, at: usize) -> Result<usize, ImeError> {
    key_index(key).ok_or(ImeError::DecodeInvalidChar {
        ch: char::from(key),
        at,
    })
}
