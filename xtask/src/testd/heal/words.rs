//! The words a name is spelled with, and the exact test a rename has to pass.
//!
//! Responsibility: decide whether one name is another with words added at either end, and nothing
//! else. This test is what makes a re-derivation exact rather than a similarity: the words of the
//! name a locator lost have to appear inside the candidate's, in order and adjacent, so a name
//! that merely resembles it is never a candidate and a heal can never wander towards one.
//!
//! # Why the words and not the characters
//!
//! The specification's constant column writes a name in kebab-case and a component declares one
//! in PascalCase; the two are the same constant, so a comparison on names has to look at the
//! words rather than at the characters. A hyphen, an underscore and a change from a lower-case
//! letter to an upper-case one all separate two words.

/// The words a name is spelled with, whatever spelling it is in.
///
/// The metrics block declares a constant in kebab-case and a component declares one in
/// PascalCase; the two are the same constant, so a comparison on names has to look at the words
/// rather than at the characters. A hyphen, an underscore and a change from a lower-case letter
/// to an upper-case one all separate two words, and every word is lower-cased.
///
/// # Panics
///
/// Never.
pub(super) fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in name.chars() {
        if ch == '-' || ch == '_' {
            push_word(&mut out, &mut word);
            continue;
        }
        if ch.is_ascii_uppercase() {
            push_word(&mut out, &mut word);
            word.push(ch.to_ascii_lowercase());
            continue;
        }
        word.push(ch.to_ascii_lowercase());
    }
    push_word(&mut out, &mut word);
    out
}

/// Whether one name is the other with words added at either end.
///
/// This is the rename the project's own vocabulary produces -- `grid-gap` declared again as
/// `candidate-grid-gap` -- and it is exact: the words of the shorter name have to appear inside
/// the longer one in order and adjacent, so a name that merely looks similar is not a candidate.
/// The relation is symmetric, because a name that lost words is the same rename seen the other
/// way round. A name is never a rename of itself.
///
/// # Panics
///
/// Never.
pub(super) fn is_rename_of(old: &str, new: &str) -> bool {
    if old == new {
        return false;
    }
    let old = words(old);
    let new = words(new);
    contains_run(&new, &old) || contains_run(&old, &new)
}

/// Ends the word being spelled, when there is one.
///
/// # Panics
///
/// Never.
fn push_word(out: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        out.push(std::mem::take(word));
    }
}

/// Whether `needle` appears inside `haystack` in order and adjacent.
///
/// # Panics
///
/// Never.
fn contains_run(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{is_rename_of, words};

    #[test]
    fn test_words_read_a_name_as_the_words_it_is_spelled_with() {
        assert_eq!(words("cell-height"), ["cell", "height"]);
        assert_eq!(
            words("CellHeight"),
            ["cell", "height"],
            "the spelling a component declares names the same constant"
        );
        assert_eq!(words("cell_height"), ["cell", "height"]);
        assert_eq!(words("grid-gap"), ["grid", "gap"]);
        assert_eq!(words("min-per-row"), ["min", "per", "row"]);
        assert_eq!(words("height"), ["height"]);
        assert!(
            words("").is_empty(),
            "a name with nothing in it spells no word"
        );
        assert!(
            words("--").is_empty(),
            "separators with no word between them spell nothing"
        );
    }

    #[test]
    fn test_is_rename_of_accepts_only_words_added_at_either_end() {
        assert!(is_rename_of("grid-gap", "candidate-grid-gap"));
        assert!(is_rename_of("grid-gap", "grid-gap-row"));
        assert!(is_rename_of("cell-height", "candidate-cell-height-dp"));
        assert!(
            is_rename_of("candidate-cell-height", "cell-height"),
            "a name that lost words is the same rename seen the other way round"
        );

        assert!(
            !is_rename_of("cell-height", "height-cell"),
            "the same words in another order are a different name"
        );
        assert!(
            !is_rename_of("cell-height", "cellheight"),
            "a spelling is not a word"
        );
        assert!(
            !is_rename_of("cell-height", "candidate-cell-padding"),
            "words that are not the locator's are not a rename of it"
        );
        assert!(
            !is_rename_of("cell-height", "cell-height"),
            "a name is not a rename of itself"
        );
        assert!(
            !is_rename_of("", "candidate"),
            "nothing has no words to carry"
        );
    }
}
