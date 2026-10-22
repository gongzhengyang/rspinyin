//! The mixed-English edge: the one candidate an input no pinyin reading cuts
//! earns from the English lexicon.
//!
//! Responsibility: own the price of the English edge and the rule that decides
//! when it exists. The edge itself is built by the lattice's walk
//! ([`crate::viterbi`]), which is where the lexicon handles and the covered-node
//! map live; this module is the policy the walk reads, kept beside the segment
//! layer because it is a statement about the *input* -- what counts as an
//! English span -- rather than about the graph.
//!
//! # When an English edge exists
//!
//! Only where the exact, fuzzy and correction walks all left the input uncut.
//! An input that reads as pinyin never asks the English lexicon a question,
//! which is what makes the switch free for every correctly typed composition
//! and what makes an English candidate unable to displace a pinyin reading:
//! they are never offered for the same input.
//!
//! # Why the price is positive here
//!
//! The design's quality ladder (custom phrase above pinyin above English) is
//! written as a negative offset against the English entry. This workspace's
//! penalties are positive integers the sweep subtracts from the edge score --
//! [`CORRECTION_PENALTY_Q8`](crate::fuzzy) and the abbreviation penalty are the
//! precedent -- so the same ladder lands here as a positive constant, and the
//! subtraction produces the intended discount (ADR-0016).

use ime_types::DecodeFlags;

/// The grey annotation an English candidate carries.
///
/// User-facing text is Chinese by the project's language convention; this is the
/// source label the candidate window draws beside the word, the same slot the
/// phrase table's `短语` label fills.
pub const ENGLISH_ANNOTATION: &str = "英文";

/// The price an English edge carries, in the Q8.8 unit the lattice's other
/// penalties use.
///
/// About 0.18 log2 units: the same gap melt_eng's `initial_quality` 1.1-versus-1.2
/// ratio expresses. It prices an English edge against the other paths of a mixed
/// composition; on an input with no pinyin reading at all it has nothing to lose
/// against, because the English edge is the only way the input can be covered.
pub const MIXED_ENGLISH_PENALTY_Q8: i32 = 46;

const _: () = assert!(MIXED_ENGLISH_PENALTY_Q8 > 0);

/// Whether the request asks for English candidates at all.
///
/// The gate the sweep's callers and the lattice's options both read: with the bit
/// clear the English lexicon is never consulted, which is the whole cost of the
/// feature being switched off.
pub fn is_enabled(flags: DecodeFlags) -> bool {
    flags.contains(DecodeFlags::ENGLISH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_enabled_follows_the_english_bit() {
        assert!(!is_enabled(DecodeFlags::empty()));
        assert!(!is_enabled(DecodeFlags::USER_DICT));
        assert!(is_enabled(DecodeFlags::ENGLISH));
        assert!(is_enabled(DecodeFlags::USER_DICT | DecodeFlags::ENGLISH));
    }

    #[test]
    fn test_mixed_english_penalty_is_positive_and_below_the_correction_ladder() {
        // The sign convention is the lattice's own (subtracted from the score), and
        // the ladder holds: an English edge is priced far below a corrected one.
        assert!(MIXED_ENGLISH_PENALTY_Q8 > 0);
        assert!(MIXED_ENGLISH_PENALTY_Q8 < crate::fuzzy::FUZZY_PENALTY_Q8);
        assert!(MIXED_ENGLISH_PENALTY_Q8 < crate::segment::correct::CORRECTION_PENALTY_Q8);
    }
}
