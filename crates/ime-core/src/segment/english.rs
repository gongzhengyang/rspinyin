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

/// The casing form the raw input was typed in.
///
/// Matching is case-folded -- the container's keys are lowercased spellings and the
/// segmentation normalizes the input before it is looked up -- so the shape is a
/// fact about the keystrokes alone, and it is read from the raw buffer rather than
/// from the normalized spelling. It is a display transform's input, never a matching
/// input: two spellings that differ only in case answer the same candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputShape {
    /// Every letter typed lowercase (the common composition). The matched entry
    /// goes out exactly as the lexicon authored it, which is what lets a brand's
    /// own casing -- `GitHub`, `GitLab` -- survive a lowercase spelling: the shell
    /// is the entry's, not the input's.
    Lower,
    /// The first letter typed uppercase and the rest lowercase. The entry goes out
    /// in that form: the first scalar value uppercased, the rest lowercased.
    Capitalized,
    /// Every letter typed uppercase. The entry goes out uppercased.
    Upper,
}

/// Classifies the casing form `raw` was typed in.
///
/// Only the two spellings a keyboard means something by name a shape: the
/// leading-uppercase one (`Github`) and the all-uppercase one (`GITHUB`). A mixed
/// form -- an uppercase past the first letter (`gitH`), a capital in the middle
/// (`GitH`) -- names neither, and answers [`InputShape::Lower`]: an unrecognizable
/// form is a request to leave the entry alone, which is the fail-safe answer and
/// the one every input answered before the transform existed. An empty raw, and a
/// raw with no ASCII letter in it, are [`InputShape::Lower`] by the same rule; a
/// single uppercase letter is [`InputShape::Upper`], which commits the same word
/// the capitalized reading would -- no entry exists that could tell the two apart.
///
/// # Panics
///
/// Never.
pub fn input_shape(raw: &str) -> InputShape {
    // The two counts the forms are defined by. A non-ASCII letter carries no
    // casing this classifier reads, so it counts as neither.
    let upper = raw.chars().filter(|ch| ch.is_ascii_uppercase()).count();
    if upper == 0 {
        return InputShape::Lower;
    }
    let lower = raw.chars().filter(|ch| ch.is_ascii_lowercase()).count();
    if lower == 0 {
        return InputShape::Upper;
    }
    // Both cases are present, so only the leading-uppercase spelling names a
    // shape, and one more uppercase past the head names none.
    let head_is_upper = raw.chars().next().is_some_and(|ch| ch.is_ascii_uppercase());
    let rest_upper = raw
        .chars()
        .skip(1)
        .filter(|ch| ch.is_ascii_uppercase())
        .count();
    if head_is_upper && rest_upper == 0 {
        InputShape::Capitalized
    } else {
        InputShape::Lower
    }
}

/// Applies the input's casing form to the matched entry.
///
/// The one output-side transform the English domain carries: the entry text the
/// lexicon authored is what goes to the application under [`InputShape::Lower`],
/// while the two typed forms the user can ask for reshape it -- [`InputShape::Capitalized`]
/// wraps the first scalar value in its uppercase and lowers the rest, and
/// [`InputShape::Upper`] uppercases every scalar value. The work is O(entry length)
/// and allocates the one string the commit carries, which is a per-selection cost
/// and not a per-keystroke one.
///
/// The transform never feeds back into matching or learning: the case-folded lookup
/// answers before this runs, and the frequency record is keyed by the entry as it
/// was authored, so an all-caps commit cannot teach the dictionary a second spelling
/// of the word it already holds.
///
/// # Panics
///
/// Never.
pub fn apply_shape(entry: &str, shape: InputShape) -> String {
    match shape {
        InputShape::Lower => String::from(entry),
        InputShape::Upper => entry.to_uppercase(),
        InputShape::Capitalized => {
            let mut shaped = String::with_capacity(entry.len());
            let mut first = true;
            for ch in entry.chars() {
                if first {
                    first = false;
                    shaped.extend(ch.to_uppercase());
                } else {
                    shaped.extend(ch.to_lowercase());
                }
            }
            shaped
        }
    }
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
        // Clippy reads the operands as constants and the assertion as constant-valued,
        // which is true today and is exactly the state the pin guards: a retuning that
        // broke the ladder must fail here, so the lint is silenced rather than the
        // relationship being restated as literals that could drift from the constants.
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(MIXED_ENGLISH_PENALTY_Q8 > 0);
            assert!(MIXED_ENGLISH_PENALTY_Q8 < crate::fuzzy::FUZZY_PENALTY_Q8);
            assert!(MIXED_ENGLISH_PENALTY_Q8 < crate::segment::correct::CORRECTION_PENALTY_Q8);
        }
    }

    #[test]
    fn test_input_shape_reads_the_three_spellings_a_keyboard_names() {
        assert_eq!(input_shape("github"), InputShape::Lower);
        assert_eq!(input_shape("gitlab"), InputShape::Lower);
        assert_eq!(input_shape("Github"), InputShape::Capitalized);
        assert_eq!(input_shape("Gitlab"), InputShape::Capitalized);
        assert_eq!(input_shape("GITHUB"), InputShape::Upper);
        assert_eq!(input_shape("GITLAB"), InputShape::Upper);
        // A single uppercase keystroke is the all-uppercase form; the capitalized
        // reading would commit the same word, which is why the ambiguity is free.
        assert_eq!(input_shape("G"), InputShape::Upper);
    }

    #[test]
    fn test_input_shape_answers_lower_for_the_forms_no_variant_names() {
        // A mixed form names no shape the enum holds, and the fail-safe answer is
        // the one every input gave before the transform existed: leave the entry
        // alone. The empty raw is the same answer by the same rule.
        assert_eq!(input_shape("gitHub"), InputShape::Lower);
        assert_eq!(input_shape("GitH"), InputShape::Lower);
        assert_eq!(input_shape(""), InputShape::Lower);
    }

    #[test]
    fn test_apply_shape_lower_leaves_the_entry_as_the_lexicon_authored_it() {
        // The brand casing the card's `github` -> `GitHub` example assumes lives in
        // the entry, not in the transform: matching is case-folded, so the lowercase
        // spelling reaches the entry the lexicon holds and the Lower shape hands it
        // on untouched. A lowercase entry stays lowercase for the same reason.
        assert_eq!(apply_shape("GitHub", InputShape::Lower), "GitHub");
        assert_eq!(apply_shape("GitLab", InputShape::Lower), "GitLab");
        assert_eq!(apply_shape("github", InputShape::Lower), "github");
    }

    #[test]
    fn test_apply_shape_capitalized_wraps_the_first_scalar_and_lowers_the_rest() {
        assert_eq!(apply_shape("github", InputShape::Capitalized), "Github");
        assert_eq!(apply_shape("GitHub", InputShape::Capitalized), "Github");
        assert_eq!(apply_shape("GITHUB", InputShape::Capitalized), "Github");
    }

    #[test]
    fn test_apply_shape_upper_uppercases_every_scalar() {
        assert_eq!(apply_shape("github", InputShape::Upper), "GITHUB");
        assert_eq!(apply_shape("GitHub", InputShape::Upper), "GITHUB");
        assert_eq!(apply_shape("GitLab", InputShape::Upper), "GITLAB");
    }

    #[test]
    fn test_apply_shape_is_total_over_a_one_letter_entry_and_empty_text() {
        // A one-letter entry answers the same word under the two uppercase shapes,
        // which is why the classifier can call a single uppercase keystroke either:
        // no entry exists that could tell the two apart.
        assert_eq!(apply_shape("g", InputShape::Capitalized), "G");
        assert_eq!(apply_shape("g", InputShape::Upper), "G");
        assert_eq!(apply_shape("", InputShape::Capitalized), "");
        assert_eq!(apply_shape("", InputShape::Upper), "");
        assert_eq!(apply_shape("", InputShape::Lower), "");
    }

    #[test]
    fn test_input_shape_and_apply_shape_agree_over_the_folded_spelling() {
        // The consistency the folded matching needs: whatever casing the user typed,
        // the shape applied to the spelling the container keys by is the word the
        // user asked for -- the same relation the commit path builds, where `raw`
        // is the buffer and the entry is what the folded lookup answered.
        for raw in ["github", "Github", "GITHUB"] {
            let shape = input_shape(raw);
            let folded = raw.to_ascii_lowercase();
            let shaped = apply_shape(&folded, shape);
            let expected = match shape {
                InputShape::Lower => String::from(&folded),
                InputShape::Capitalized => String::from("Github"),
                InputShape::Upper => String::from("GITHUB"),
            };
            assert_eq!(shaped, expected, "raw was {raw}");
        }
    }
}
