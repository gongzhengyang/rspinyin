//! The uniqueness the unigram table's key relies on.
//!
//! Responsibility: find the word pairs whose [`hash_word`] values agree, which is the
//! one thing that makes a hash-keyed table ambiguous.
//!
//! Boundaries: this module hashes and compares and nothing else. It reads no file and
//! holds no table, and it is called by the compiler rather than by the read path: the
//! check is a property of a whole word list, and the compiler is the only place that has
//! one.
//!
//! # Why the check exists at all
//!
//! [`crate::format::reader::Reader::unigram_lookup`] resolves a collision by comparing
//! the candidate's text, because it can reach `ENTRIES` and `STRPOOL` through the same
//! index. A language model built over the `UNIGRAM` section alone cannot: the section
//! stores hashes and no text, so a word is found by its hash and nothing else, and two
//! words sharing one would take each other's score. The probability of a collision at 32
//! bits is negligible for a dictionary of any realistic size, but "negligible" is not
//! "impossible", and a build that ships a dictionary holding one has shipped a wrong
//! score. Refusing the word list at compile time turns that from a silent wrong answer
//! into a failed build.

use crate::format::hash_word;

/// Returns the first pair of distinct words whose hashes agree, if the list holds one.
///
/// The words are hashed here, so the caller does not have to keep them in any particular
/// order: the search sorts by `(hash, word)` -- the order `ENTRIES` and `UNIGRAM` are
/// written in -- and then compares neighbours.
///
/// # Examples
///
/// ```
/// use ime_dict::format::find_colliding_hashes;
///
/// assert_eq!(find_colliding_hashes(&["中国", "银行", "中心"]), None);
/// assert_eq!(find_colliding_hashes(&[]), None);
/// ```
pub fn find_colliding_hashes<'a>(words: &[&'a str]) -> Option<(u32, &'a str, &'a str)> {
    let mut rows: Vec<(u32, &'a str)> = words.iter().map(|word| (hash_word(word), *word)).collect();
    rows.sort_unstable();
    first_collision(&rows)
}

/// Returns the first pair of rows that share a hash and not a text.
///
/// `rows` has to be sorted by `(hash, word)`; the search is then a single pass over
/// neighbours. Two rows with the same hash and the same text are one word reached twice
/// rather than a collision, and are skipped.
///
/// This is the half of [`find_colliding_hashes`] that decides, split out so that it can
/// be tested: a real FNV-1a collision cannot be written down by hand, while a pair of
/// rows that share a hash can.
pub(crate) fn first_collision<'a>(rows: &[(u32, &'a str)]) -> Option<(u32, &'a str, &'a str)> {
    for pair in rows.windows(2) {
        let (Some(left), Some(right)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        if left.0 == right.0 && left.1 != right.1 {
            return Some((left.0, left.1, right.1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_collision_reports_two_distinct_texts_that_share_a_hash() {
        // What a collision looks like to this check: two rows with one hash and two
        // texts, in the sorted order the compiler writes them in.
        let rows = [(1u32, "中国"), (2, "银行"), (2, "银杭")];
        assert_eq!(first_collision(&rows), Some((2, "银行", "银杭")));
    }

    #[test]
    fn test_first_collision_accepts_a_list_without_a_collision() {
        assert_eq!(first_collision(&[]), None, "an empty list cannot collide");
        assert_eq!(first_collision(&[(7, "中")]), None);
        assert_eq!(first_collision(&[(1, "中"), (2, "国"), (3, "心")]), None);
        // One word reached twice is one word: the hash repeats, the text does not differ.
        assert_eq!(first_collision(&[(7, "中"), (7, "中")]), None);
    }

    #[test]
    fn test_find_colliding_hashes_accepts_a_word_list_that_has_none() {
        assert_eq!(find_colliding_hashes(&[]), None);
        assert_eq!(
            find_colliding_hashes(&["中国", "银行", "中心", "你好"]),
            None
        );
        // The same word twice is not a collision either.
        assert_eq!(find_colliding_hashes(&["中国", "中国"]), None);
    }
}
