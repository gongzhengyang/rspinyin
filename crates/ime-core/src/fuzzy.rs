//! Fuzzy syllable matching: the confusable-spelling classes, the syllables one
//! spelling reaches, and the penalty a fuzzy match carries.
//!
//! Responsibility: own the eight confusions the frozen `DecodeFlags` namespace
//! defines -- `zh`/`z`, `ch`/`c`, `sh`/`s`, `n`/`l`, `an`/`ang`, `en`/`eng`,
//! `in`/`ing`, `f`/`h` -- and answer, for one syllable and one flag set, every
//! syllable a speaker who does not distinguish them could have meant.
//!
//! Boundaries: this is the syllable algebra of fuzzy matching and nothing else. It
//! does not read the input, does not walk the segmentation graph and never looks a
//! word up: a caller hands it a syllable identifier and gets identifiers back. It
//! is a pure function of its arguments -- no file, no clock, no environment, no
//! global state -- which is what keeps the decoder's determinism guarantee intact
//! (0.4 rule 4).
//!
//! # Why a class is applied to a whole syllable
//!
//! A class is a pair of spellings, and it is applied to the syllable as a whole,
//! never to a letter inside it. Rewriting at letter level would turn `zhang` into
//! `zang`, which is the intent, but it would also turn `zha` into `za`, which is a
//! different syllable with a different meaning. A class is therefore tried at the
//! start of a spelling and at its end, and the syllable table decides which of the
//! two applies: `zh` only ever opens a syllable and `ang` only ever closes one, so
//! the spelling itself carries the distinction. A substitution that lands on
//! something the table does not hold is not a variant at all and is skipped --
//! which is what keeps `zhuang` from reaching a `zuang` that is no syllable.
//!
//! # Where the widening happens
//!
//! Fuzzy matching changes which syllables a spelling reaches, not the spelling
//! itself, so it sits beside the segmentation layer rather than in the decoder.
//! The widening is applied while the word lattice is built: the lattice spells one
//! dictionary key per span of the graph, and for every syllable of that span it
//! asks [`variants`] which other syllables to spell as well -- spelling them from
//! [`syllable_at`](crate::segment::syllable_at) rather than from the input text --
//! and scores each extra edge through [`penalize`]. The Viterbi pass is left alone
//! on purpose: it is defined over a fixed lattice, and handing it a growing one
//! would make the k-best guarantee depend on the order the extra edges were
//! discovered in.
//!
//! The input is never rewritten. A variant changes the key a span is looked up
//! under while the byte span stays the one the graph produced, so every offset the
//! preedit and the window use keeps pointing at the byte it pointed at before.
//!
//! # The master switch
//!
//! [`DecodeFlags::FUZZY`] gates every class bit: a class bit set without it does
//! nothing at all. A master switch set with no class bit is the same as fuzzy
//! matching being off, and is deliberately not reported as a misconfiguration --
//! there is nothing to report, and a diagnostic per keystroke would be noise. With
//! the switch clear [`variants`] answers with the syllable it was given and
//! touches neither the table nor the allocator.
//!
//! # Determinism
//!
//! The order of the returned set is fixed by [`CLASSES`] and by the order the two
//! ends of a spelling are tried in, never by a hash: a candidate order that
//! depended on a `HashSet`'s iteration order would not be reproducible across
//! processes. [`MAX_VARIANTS`] caps the set and the classes are applied in table
//! order, so the cap drops the same variants on every run.

use ime_types::{DecodeFlags, SyllableId};
use smallvec::SmallVec;

use crate::segment::syllable::{MAX_SYLLABLE_LEN, lookup, syllable_at};

/// Most syllables one spelling may reach, including the spelling itself.
///
/// Eight classes can in principle combine into `2^8` spellings, far more than any
/// dictionary key is worth spelling; the cap keeps the work one syllable costs
/// bounded. It is applied in [`CLASSES`] order, so which variants are dropped is
/// the same on every run.
pub const MAX_VARIANTS: usize = 8;

/// The score penalty one fuzzy-variant edge carries, in the Q16.16 unit the sweep
/// ranks with.
///
/// Without it a fuzzy match at equal dictionary weight would tie with an exact
/// match, and the tie would be broken by whichever edge the lattice happened to
/// hold first -- an order-dependent result, which the decoder's determinism
/// guarantee forbids. The value is `12.0` in Q8.8, which is about 4.7% of one unit
/// of log probability: enough to lose every tie, small enough that a fuzzy reading
/// only a few per cent likelier than the exact one still wins.
pub const FUZZY_PENALTY_Q8: i32 = 12 << 8;

/// Raw bits of every class in [`CLASSES`], `1 << 1` through `1 << 8`.
///
/// Written as bit positions rather than folded over the table because a `const`
/// cannot iterate; `test_class_table_matches_the_frozen_flag_namespace` keeps the
/// two in step, so a class added to the table without a bit here is a test failure.
const CLASS_BITS: u16 =
    (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8);

/// Longest spelling a substitution can build.
///
/// Twice the longest syllable in the table, which is more than a substitution can
/// need -- a class replaces one spelling with another of comparable length -- and
/// which leaves room for a class added later to lengthen a spelling without
/// silently overflowing the buffer and dropping a variant.
const MAX_SPELLING: usize = MAX_SYLLABLE_LEN * 2;

// A substitution can lengthen a spelling, so the buffer a join builds in has to be
// wider than the longest syllable the table holds. Asserted here rather than in a
// test so that the invariant is checked at compile time, like the other constant
// invariants of this workspace.
const _: () = assert!(MAX_SPELLING > MAX_SYLLABLE_LEN);

/// One fuzzy class: a pair of spellings treated as equivalent.
///
/// The class is applied at the syllable level, not the letter level: `zh` and `z`
/// are whole initials, and `an` / `ang` are whole finals. Letter-level rewriting
/// would turn `zhang` into `zang` correctly but also `zha` into `za`, which is a
/// different syllable with a different meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzyClass {
    /// The bit in `DecodeFlags` that enables this class.
    pub flag: DecodeFlags,
    /// The name the configuration file writes for this class.
    ///
    /// It is the bit's own name in lower case, so that the value in a
    /// configuration file, the diagnostic and the source all read the same.
    pub name: &'static str,
    /// The spelling as written.
    pub canonical: &'static str,
    /// The spelling it also matches.
    pub variant: &'static str,
}

/// Every class the frozen `DecodeFlags` namespace defines, in bit order.
///
/// A `static` rather than a `const` because [`class_by_name`] hands out references
/// into it, and a reference into a `const` item is a reference into a temporary.
// `#[rustfmt::skip]`: this is a data table, and one class per line keeps the bit
// order, the configuration name and the spelling pair reviewable side by side.
#[rustfmt::skip]
pub static CLASSES: [FuzzyClass; 8] = [
    FuzzyClass { flag: DecodeFlags::FUZZY_ZH_Z,   name: "zh_z",   canonical: "zh", variant: "z"   },
    FuzzyClass { flag: DecodeFlags::FUZZY_CH_C,   name: "ch_c",   canonical: "ch", variant: "c"   },
    FuzzyClass { flag: DecodeFlags::FUZZY_SH_S,   name: "sh_s",   canonical: "sh", variant: "s"   },
    FuzzyClass { flag: DecodeFlags::FUZZY_N_L,    name: "n_l",    canonical: "n",  variant: "l"   },
    FuzzyClass { flag: DecodeFlags::FUZZY_AN_ANG, name: "an_ang", canonical: "an", variant: "ang" },
    FuzzyClass { flag: DecodeFlags::FUZZY_EN_ENG, name: "en_eng", canonical: "en", variant: "eng" },
    FuzzyClass { flag: DecodeFlags::FUZZY_IN_ING, name: "in_ing", canonical: "in", variant: "ing" },
    FuzzyClass { flag: DecodeFlags::FUZZY_F_H,    name: "f_h",    canonical: "f",  variant: "h"   },
];

/// The syllables one spelling reaches, the exact spelling first.
///
/// A [`SmallVec`] rather than a `Vec`: the set is bounded by [`MAX_VARIANTS`] and a
/// decode widens one syllable per keystroke, so the whole expansion stays in the
/// caller's frame instead of going through the allocator.
pub type VariantSet = SmallVec<[SyllableId; MAX_VARIANTS]>;

/// Whether fuzzy matching is switched on at all.
///
/// Both halves of the switch are required: the master [`DecodeFlags::FUZZY`] bit
/// and at least one class bit. Either one alone is the same as fuzzy matching
/// being off, and neither is reported as a misconfiguration.
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
/// use ime_core::fuzzy::is_enabled;
/// use ime_types::DecodeFlags;
///
/// assert!(!is_enabled(DecodeFlags::FUZZY_ZH_Z));
/// assert!(!is_enabled(DecodeFlags::FUZZY));
/// assert!(is_enabled(DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z));
/// ```
pub fn is_enabled(flags: DecodeFlags) -> bool {
    flags.contains(DecodeFlags::FUZZY) && flags.bits() & CLASS_BITS != 0
}

/// How many classes `flags` switches on.
///
/// Answers zero when the master switch is clear, whatever the class bits say.
///
/// # Errors
///
/// Never.
///
/// # Panics
///
/// Never.
pub fn enabled_class_count(flags: DecodeFlags) -> usize {
    if !flags.contains(DecodeFlags::FUZZY) {
        return 0;
    }
    CLASSES
        .iter()
        .filter(|class| flags.contains(class.flag))
        .count()
}

/// Whether the enabled classes can produce more variants than [`MAX_VARIANTS`]
/// holds.
///
/// One class adds at most one spelling to each spelling already in the set -- a
/// syllable is substituted at one end or the other, never at both -- so three
/// classes reach exactly the cap and only a fourth can pass it. This is the cheap
/// test a caller uses to decide whether a `decode/fuzzy-truncated` diagnostic is
/// even possible, without widening anything.
///
/// It is a necessary condition, not a sufficient one, and with the eight classes
/// this build ships no syllable reaches the cap at all: a syllable has one initial
/// and one final, each belongs to at most one class, so at most two classes can
/// apply and the widest set holds four spellings. The cap is therefore a guard for
/// a class table that grows rather than a live limit today.
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
/// use ime_core::fuzzy::may_truncate;
/// use ime_types::DecodeFlags;
///
/// let three = DecodeFlags::FUZZY
///     | DecodeFlags::FUZZY_ZH_Z
///     | DecodeFlags::FUZZY_AN_ANG
///     | DecodeFlags::FUZZY_N_L;
/// assert!(!may_truncate(three));
/// assert!(may_truncate(three | DecodeFlags::FUZZY_F_H));
/// ```
pub fn may_truncate(flags: DecodeFlags) -> bool {
    enabled_class_count(flags) > 3
}

/// Returns the class a configuration name names.
///
/// The names are the class bits' own names in lower case (`zh_z`, `an_ang`), and
/// they are compared ASCII case-insensitively, so a file that spells one `ZH_Z`
/// still resolves. A name no class answers to returns `None`; the configuration
/// layer reports `config/invalid` and ignores it rather than refusing to start.
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
/// use ime_core::fuzzy::class_by_name;
/// use ime_types::DecodeFlags;
///
/// let zh_z = class_by_name("ZH_Z");
/// assert_eq!(zh_z.map(|class| class.flag), Some(DecodeFlags::FUZZY_ZH_Z));
/// assert_eq!(zh_z.map(|class| class.name), Some("zh_z"));
/// assert!(class_by_name("zhz").is_none());
/// ```
pub fn class_by_name(name: &str) -> Option<&'static FuzzyClass> {
    CLASSES
        .iter()
        .find(|class| class.name.eq_ignore_ascii_case(name))
}

/// Every syllable `syllable` reaches under `flags`, the exact spelling first.
///
/// The exact spelling is included, so a caller can treat the result as the
/// complete set of syllables to try without special-casing the exact match; a
/// caller that wants only the widened ones skips the first element.
///
/// With [`DecodeFlags::FUZZY`] clear, or with no class bit set, the set holds the
/// syllable alone and no table lookup is made at all.
///
/// # Errors
///
/// Never: the expansion is total, and a substitution that reaches no syllable of
/// the table is skipped rather than reported.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::fuzzy::variants;
/// use ime_core::segment::{lookup, syllable_at};
/// use ime_types::DecodeFlags;
///
/// let zhang = lookup("zhang").expect("zhang is in the syllable table");
/// let spelled = |flags| -> Vec<&'static str> {
///     variants(zhang, flags)
///         .iter()
///         .filter_map(|id| syllable_at(*id))
///         .collect()
/// };
///
/// // `zh`/`z` reaches the one spelling of `zhang` that is also a syllable.
/// let zh_z = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z;
/// assert_eq!(spelled(zh_z), ["zhang", "zang"]);
///
/// // The master switch alone changes nothing.
/// assert_eq!(spelled(DecodeFlags::FUZZY), ["zhang"]);
/// ```
pub fn variants(syllable: SyllableId, flags: DecodeFlags) -> VariantSet {
    let mut out = VariantSet::new();
    variants_into(&mut out, syllable, flags);
    out
}

/// Fills `out` with every syllable `syllable` reaches, and answers whether
/// [`MAX_VARIANTS`] cut the expansion short.
///
/// `out` is cleared first and then filled, so a caller that keeps one set across
/// keystrokes never allocates: [`MAX_VARIANTS`] is the inline capacity and the
/// expansion is bounded by the cap, so the set never spills to the heap.
///
/// The answer is `true` only when a variant was found that the cap left no room
/// for. It is the signal behind the `decode/fuzzy-truncated` diagnostic, and it is
/// a fact about this syllable rather than about the flag set, so a caller that
/// wants to record the diagnostic once has to remember that it already did. See
/// [`may_truncate`] for whether it can fire at all.
///
/// # Errors
///
/// Never: the expansion is total, and every table lookup answers an `Option`.
///
/// # Panics
///
/// Never.
pub fn variants_into(out: &mut VariantSet, syllable: SyllableId, flags: DecodeFlags) -> bool {
    out.clear();
    out.push(syllable);
    if !is_enabled(flags) {
        return false;
    }
    expand_into(out, flags, MAX_VARIANTS)
}

/// Applies every enabled class to the set, in table order, and answers whether
/// `cap` cut the expansion short.
///
/// The classes are applied one after another, each over the set the classes before
/// it produced, which is what lets two confusions compose: `zhang` with `zh_z` and
/// `an_ang` on reaches `zan` as well, and a reader who confuses both is exactly
/// the reader who types it.
///
/// `cap` is a parameter rather than [`MAX_VARIANTS`] so that the truncation branch
/// is reachable from a test: no syllable of the shipped table has more than four
/// spellings, so the shipped cap is never reached.
fn expand_into(out: &mut VariantSet, flags: DecodeFlags, cap: usize) -> bool {
    for class in &CLASSES {
        if !flags.contains(class.flag) {
            continue;
        }
        if widen(out, class, cap) {
            return true;
        }
    }
    false
}

/// Applies one class to every syllable the set held when it started, and answers
/// whether `cap` cut the widening short.
///
/// The spellings to widen are snapshotted first, because the set grows while it is
/// walked: a class applies once, and its own results are widened by the classes
/// after it rather than by itself again, so `zang` cannot turn into `zhang` and
/// back into `zang`.
fn widen(out: &mut VariantSet, class: &FuzzyClass, cap: usize) -> bool {
    let mut base = VariantSet::new();
    base.extend_from_slice(&out[..]);
    for syllable in base {
        let Some(text) = syllable_at(syllable) else {
            continue;
        };
        let reached = [
            substitute_prefix(text, class.canonical, class.variant),
            substitute_prefix(text, class.variant, class.canonical),
            substitute_suffix(text, class.canonical, class.variant),
            substitute_suffix(text, class.variant, class.canonical),
        ];
        for candidate in reached.into_iter().flatten() {
            if out.contains(&candidate) {
                continue;
            }
            if out.len() >= cap {
                return true;
            }
            out.push(candidate);
        }
    }
    false
}

/// Returns `score` with the penalty of a fuzzy-variant edge applied.
///
/// `score` is one edge score in the Q16.16 unit the sweep ranks with, and the
/// answer is that score minus [`FUZZY_PENALTY_Q8`] in the same unit. The
/// subtraction saturates, so a score at the floor of the `i32` range stays where
/// it is instead of wrapping to the top.
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
/// use ime_core::fuzzy::{FUZZY_PENALTY_Q8, penalize};
///
/// // An exact edge and a fuzzy one at equal weight: the exact one wins.
/// assert!(penalize(-65536) < -65536);
/// assert_eq!(-65536 - penalize(-65536), FUZZY_PENALTY_Q8);
/// ```
pub fn penalize(score_q16: i32) -> i32 {
    score_q16.saturating_sub(FUZZY_PENALTY_Q8)
}

/// Returns the spelling a class reaches when it replaces `from` at the start of
/// `syllable`, or `None` when the result is no syllable of the table.
fn substitute_prefix(syllable: &str, from: &str, to: &str) -> Option<SyllableId> {
    let rest = syllable.strip_prefix(from)?;
    join(to, rest)
}

/// Returns the spelling a class reaches when it replaces `from` at the end of
/// `syllable`, or `None` when the result is no syllable of the table.
fn substitute_suffix(syllable: &str, from: &str, to: &str) -> Option<SyllableId> {
    let rest = syllable.strip_suffix(from)?;
    join(rest, to)
}

/// Joins two spellings in a fixed-size buffer and looks the result up.
///
/// The buffer is on the stack and the join never allocates, which is what lets the
/// decode path widen a syllable without touching the allocator. Both slices are in
/// range because the length is checked against the buffer first, and a result
/// longer than any syllable could be is refused rather than truncated.
fn join(head: &str, tail: &str) -> Option<SyllableId> {
    let len = head.len().checked_add(tail.len())?;
    if len > MAX_SPELLING {
        return None;
    }
    let mut buffer = [0u8; MAX_SPELLING];
    buffer[..head.len()].copy_from_slice(head.as_bytes());
    buffer[head.len()..len].copy_from_slice(tail.as_bytes());
    let spelled = std::str::from_utf8(&buffer[..len]).ok()?;
    lookup(spelled)
}

#[cfg(test)]
mod tests;
