//! The mixed-input fallback: what a scheme session does with a keystroke the active
//! layout cannot read.
//!
//! Responsibility: resolve one position of a keystroke stream -- the scheme's own
//! reading of it, a full-pinyin syllable read inside a scheme session, or a single
//! keystroke passed through on its own -- and report which of the three it was.
//!
//! Boundaries: a pure function of the table, the keys and the policy; no file, no
//! clock, no environment, no global state (0.4 rule 4). Whatever it resolves to is a
//! full-pinyin spelling, so the segmentation layer never learns that schemes exist:
//! the DAG, the lattice and the Viterbi pass are untouched.
//!
//! # The ladder
//!
//! A user who switched to a scheme and then types a full-pinyin syllable gets nothing,
//! which reads as "the input method broke" rather than as "you are in the wrong mode".
//! Every mainstream implementation therefore keeps reading the occasional full-pinyin
//! syllable inside a scheme session. The rule is three steps, tried in this order:
//!
//! 1. the scheme's own reading ([`SyllableOrigin::Scheme`]) -- a pair the table can
//!    spell is never re-read as full pinyin, because the layout the user chose is the
//!    layout they expect;
//! 2. the full-pinyin reading ([`SyllableOrigin::FullPinyinFallback`]), taken only when
//!    the policy asks for it;
//! 3. the single keystroke carried through ([`SyllableOrigin::Literal`]).
//!
//! The second step is the only one that costs the decode path anything, and it runs
//! only where the scheme has already failed: a session whose keystrokes all map pays
//! nothing for it.
//!
//! # Nothing is swallowed
//!
//! Every position produces a resolution, and the origin says which step produced it.
//! The third step is a decision rather than a silent skip: a caller that writes only
//! text it can trust asks [`is_carriable`] first and reports the keystrokes that fail,
//! because a byte that disappears from the rewritten input moves every offset after it.

use ime_types::{ImeError, SyllableId};

use crate::segment::syllable::{DroppedChars, MAX_SYLLABLE_LEN, lookup, normalize_into};

use super::{MappedSyllable, SchemeTable, key_index, pair_syllable, standalone_syllable};

/// Which of the three readings resolved one position of the keystroke stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyllableOrigin {
    /// The active scheme's table mapped the keys.
    Scheme,
    /// The keys were read as a full-pinyin syllable instead.
    ///
    /// The candidate window shows a subtle hint for one of these, so that the user can
    /// tell why this syllable looks different from what the scheme would have produced
    /// for the same keys.
    FullPinyinFallback,
    /// Neither reading worked, and the keystroke passed through on its own.
    Literal,
}

/// Whether a keystroke the scheme cannot read may be read as full pinyin.
///
/// The value comes from the `scheme.keep_full_pinyin` configuration key, which this
/// crate cannot see: `ime-core` holds no configuration, so the switch arrives as a
/// value, the way every other decode switch does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FallbackPolicy {
    /// Whether an unmappable position is offered to the full-pinyin reading.
    keep_full_pinyin: bool,
}

impl FallbackPolicy {
    /// Reads an unmappable position as a full-pinyin syllable when one fits.
    pub const KEEP_FULL_PINYIN: Self = Self {
        keep_full_pinyin: true,
    };

    /// Resolves every position by the scheme alone, so an unmappable one becomes a
    /// literal keystroke.
    pub const SCHEME_ONLY: Self = Self {
        keep_full_pinyin: false,
    };

    /// Builds the policy from the `scheme.keep_full_pinyin` configuration key.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn from_keep_full_pinyin(keep_full_pinyin: bool) -> Self {
        Self { keep_full_pinyin }
    }

    /// Whether the policy reads an unmappable position as full pinyin.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn keeps_full_pinyin(self) -> bool {
        self.keep_full_pinyin
    }
}

/// One position of a keystroke stream, resolved.
///
/// The fields are private because the origin and the syllable have to agree: a
/// resolution that says [`SyllableOrigin::Literal`] never carries a syllable, and one
/// that says [`SyllableOrigin::Scheme`] always does. [`resolve`] is the only way to
/// build one, so that agreement holds by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSyllable {
    /// Which reading resolved the position.
    origin: SyllableOrigin,
    /// The syllable the position spells, or `None` for a literal keystroke.
    full: Option<SyllableId>,
    /// How many bytes of the stream the resolution consumed.
    consumed: u8,
}

impl ResolvedSyllable {
    /// The one outcome that carries no syllable: a single keystroke passed through.
    const fn literal() -> Self {
        Self {
            origin: SyllableOrigin::Literal,
            full: None,
            consumed: 1,
        }
    }

    /// The outcome of a table mapping, which is always a syllable.
    const fn mapped(origin: SyllableOrigin, mapped: MappedSyllable) -> Self {
        Self {
            origin,
            full: Some(mapped.full),
            consumed: mapped.consumed,
        }
    }

    /// Which reading resolved the position.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn origin(self) -> SyllableOrigin {
        self.origin
    }

    /// The syllable the position spells, or `None` for a literal keystroke.
    ///
    /// The identifier indexes the segmentation layer's syllable table, exactly as
    /// [`MappedSyllable::full`] does.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn full(self) -> Option<SyllableId> {
        self.full
    }

    /// How many bytes of the keystroke stream the resolution consumed, at least one.
    ///
    /// A scheme syllable is one or two keys; a full-pinyin fallback is as many keys as
    /// the syllable it found, which is what makes the caller's cursor advance past the
    /// whole syllable rather than one key of it.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn consumed(self) -> u8 {
        self.consumed
    }
}

/// Resolves one position of a double-pinyin keystroke stream.
///
/// # Parameters
///
/// - `table`: the active scheme's table.
/// - `keys`: the keystroke stream, read from its first byte; the tail after the
///   position is what lets a full-pinyin fallback span more than two keys, and the
///   caller keeps its own cursor. A second byte that is not a scheme key cannot form a
///   pair, so the position resolves on its first key alone and that byte is reported
///   when the caller's cursor reaches it.
/// - `policy`: whether an unmappable position may be read as full pinyin.
/// - `scratch`: a buffer the caller owns and reuses across positions, so that
///   resolving a syllable allocates nothing after the first call. It is cleared on
///   entry and its contents are never part of the result.
///
/// # Returns
///
/// `Ok(None)` when `keys` is empty: there is no position to resolve, and inventing one
/// would make the caller's cursor advance past the end of its own buffer.
/// `Ok(Some(resolved))` for every other input, including a keystroke that resolved to
/// nothing -- a keystroke is never dropped, so there is no fourth outcome.
///
/// An apostrophe is a forced syllable boundary rather than a scheme position: the
/// caller takes it out of the stream before calling, exactly as it does for the scheme
/// rewrite today. A byte that is not a scheme key at all still resolves, to
/// [`SyllableOrigin::Literal`], which [`is_carriable`] then refuses to carry.
///
/// # Errors
///
/// [`ImeError::SchemeUnsupported`] when the table claims a scheme this build does not
/// implement, which is a malformed table rather than a malformed keystroke.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::shuangpin::fallback::{FallbackPolicy, SyllableOrigin, resolve};
/// use ime_core::shuangpin::xiaohe;
///
/// let policy = FallbackPolicy::KEEP_FULL_PINYIN;
/// let mut scratch = String::new();
///
/// // `vs` is the scheme's own reading of zhong.
/// let scheme = resolve(&xiaohe::XIAOHE, b"vs", policy, &mut scratch);
/// assert_eq!(scheme.ok().flatten().map(|it| it.origin()), Some(SyllableOrigin::Scheme));
///
/// // `m` is not a Xiaohe syllable, but it is a full-pinyin one.
/// let fallback = resolve(&xiaohe::XIAOHE, b"m", policy, &mut scratch);
/// let fallback = fallback.ok().flatten().map(|it| it.origin());
/// assert_eq!(fallback, Some(SyllableOrigin::FullPinyinFallback));
///
/// // `b` is neither, and comes back as the keystroke the user typed.
/// let literal = resolve(&xiaohe::XIAOHE, b"b", policy, &mut scratch);
/// assert_eq!(literal.ok().flatten().map(|it| it.origin()), Some(SyllableOrigin::Literal));
/// ```
pub fn resolve(
    table: &SchemeTable,
    keys: &[u8],
    policy: FallbackPolicy,
    scratch: &mut String,
) -> Result<Option<ResolvedSyllable>, ImeError> {
    if !table.id.is_known() {
        return Err(ImeError::SchemeUnsupported {
            scheme: table.id.value(),
        });
    }
    let Some(&first) = keys.first() else {
        return Ok(None);
    };
    let Some(first_slot) = key_index(first) else {
        // Not a key of any scheme: there is nothing for either reading to say about it,
        // and reporting it as a literal is what keeps the caller from writing a byte it
        // cannot account for into the rewritten text.
        return Ok(Some(ResolvedSyllable::literal()));
    };
    // The pair is tried before the key on its own, so a scheme that spells a syllable
    // with two keys takes the pair even when its first key also stands alone.
    let second = keys.get(1).copied().and_then(key_index);
    let paired = second.and_then(|second| pair_syllable(table, first_slot, second, scratch));
    if let Some(mapped) = paired {
        return Ok(Some(ResolvedSyllable::mapped(
            SyllableOrigin::Scheme,
            mapped,
        )));
    }
    if let Some(mapped) = standalone_syllable(table, first_slot) {
        return Ok(Some(ResolvedSyllable::mapped(
            SyllableOrigin::Scheme,
            mapped,
        )));
    }
    if policy.keeps_full_pinyin() {
        // The second step: the keys read as full pinyin, which is the reading the user
        // falls back on when they type a syllable the layout cannot spell.
        let origin = SyllableOrigin::FullPinyinFallback;
        return match full_pinyin_at(keys, scratch) {
            Some(mapped) => Ok(Some(ResolvedSyllable::mapped(origin, mapped))),
            None => Ok(Some(ResolvedSyllable::literal())),
        };
    }
    Ok(Some(ResolvedSyllable::literal()))
}

/// Whether a literal keystroke can be carried into the rewritten text as it stands.
///
/// A carried keystroke has to survive the segmentation layer's normalizer byte for
/// byte, or it changes the length of the rewritten text behind the caller's back and
/// moves every offset after it. That rules out two families of keystroke: anything
/// outside `a`-`z`, which the normalizer discards (`;`, `'`, a digit), and the letter
/// `v`, which the normalizer folds onto `ü`.
///
/// The `v` test is case-insensitive on purpose. The keystroke is written in lower case
/// on its way into the text, so testing `ch != 'v'` here would let an upper-case `V`
/// through and then write a bare `v` -- precisely the byte the normalizer folds, which
/// is the thing this exclusion exists to prevent.
///
/// # Examples
///
/// ```
/// use ime_core::shuangpin::fallback::is_carriable;
///
/// assert!(is_carriable('b'));
/// assert!(!is_carriable('v'));
/// assert!(!is_carriable(';'));
/// ```
///
/// # Panics
///
/// Never.
pub fn is_carriable(keystroke: char) -> bool {
    let folded = keystroke.to_ascii_lowercase();
    folded.is_ascii_alphabetic() && folded != 'v'
}

/// Reads the longest full-pinyin syllable the keystroke stream starts with.
///
/// The reading is longest-first because a shorter one would split a syllable the user
/// typed whole: `ang` read as `an` leaves a `g` behind, and the `g` is then a syllable
/// of its own in some schemes, so the mistake would travel on as a wrong word rather
/// than as a leftover keystroke.
///
/// The prefix is put through the segmentation layer's own normalizer before the table
/// is consulted, so the `ü` spellings (`lv`, `nv`, `jv`) reach the entries the
/// segmentation layer would reach, and the spelling that comes back is the canonical
/// one the rewritten text has to hold. `consumed` counts the keystrokes, not the
/// normalized bytes, because the caller's cursor moves over the stream it was given.
///
/// Only the letters of the stream are considered: a keystroke that is not a letter
/// ends the search, so a fallback never reaches past a boundary marker.
fn full_pinyin_at(keys: &[u8], scratch: &mut String) -> Option<MappedSyllable> {
    let width = keys
        .iter()
        .take_while(|key| key.is_ascii_alphabetic())
        .take(MAX_SYLLABLE_LEN)
        .count();
    // The normalizer clears both buffers on entry, and only ASCII letters are fed to
    // it here, so nothing is ever recorded as dropped.
    let mut dropped = DroppedChars::default();
    for len in (1..=width).rev() {
        let Ok(prefix) = std::str::from_utf8(&keys[..len]) else {
            continue;
        };
        normalize_into(prefix, scratch, &mut dropped);
        if let Some(full) = lookup(scratch) {
            return Some(MappedSyllable {
                full,
                consumed: u8::try_from(len).ok()?,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests;
