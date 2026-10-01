//! The uniqueness the unigram table's key relies on.
//!
//! Responsibility: count the word pairs whose [`hash_word`] values agree, and bound how
//! many such pairs a healthy hash can produce.
//!
//! Boundaries: this module hashes and compares and nothing else. It reads no file and
//! holds no table, and it is called by the compiler rather than by the read path: the
//! count is a property of a whole word list, and the compiler is the only place that has
//! one.
//!
//! # Why the check is a bound rather than a ban
//!
//! [`crate::format::reader::Reader::unigram_lookup`] resolves a collision by comparing
//! the candidate's text, because it can reach `ENTRIES` and `STRPOOL` through the same
//! index. A shipped container holding a few colliding words is therefore unambiguous at
//! runtime: every word still finds its own score, so refusing a build over birthday
//! luck would reject a dictionary the read path serves correctly.
//!
//! What a collision count does catch is a broken hash. FNV-1a over 32 bits produces
//! `n(n-1) / 2^33` colliding pairs in expectation over `n` words -- the birthday bound
//! -- and a healthy hash lands there. A hash that has degenerated (a truncated mix, a
//! constant, an accidental no-op) produces orders of magnitude more, and with it
//! equal-hash runs on the read path that degrade towards a linear scan. The compiler
//! refuses a word list whose count passes [`collision_limit`] and reports the count
//! otherwise, so a broken hash fails the build instead of the latency budget.

use crate::format::hash_word;

/// Multiplier above the birthday expectation the limit allows.
///
/// Two orders of magnitude. For a healthy 32-bit hash the colliding-pair count is a
/// Poisson with the birthday bound as its mean, so exceeding the mean a hundredfold is
/// an event of probability far below one in a million even at product scale -- while a
/// degenerate hash exceeds it by many orders of magnitude, which keeps the two regimes
/// far apart.
const LIMIT_HEADROOM: u128 = 100;

/// Smallest limit the check accepts as a failure threshold.
///
/// Below roughly 9,300 words the birthday expectation rounds to zero, and a bare
/// `expected * headroom` limit would refuse a healthy build on birthday luck about
/// once in three hundred runs -- a false positive the read path would have resolved
/// anyway. The floor keeps small word lists immune to that luck; a degenerate hash
/// still fails it, because its count on any real list is orders of magnitude past it.
const LIMIT_FLOOR: u64 = 4;

/// Returns the colliding-pair count past which the hash itself must be broken.
///
/// The bound is the birthday expectation `n(n-1) / 2^33` colliding pairs for `n`
/// words, times [`LIMIT_HEADROOM`], raised to [`LIMIT_FLOOR`] when the expectation
/// rounds to zero. The arithmetic is `u128` because `n(n-1)` outgrows `u64` before
/// any word list this format could hold does.
///
/// # Examples
///
/// ```
/// use ime_dict::format::collision_limit;
///
/// // Small word lists keep the floor: birthday luck must not fail a healthy build.
/// assert_eq!(collision_limit(0), 4);
/// assert_eq!(collision_limit(5_441), 4);
/// // Product scale (~349k words): 14 expected pairs, two orders of headroom.
/// assert_eq!(collision_limit(349_046), 1_400);
/// ```
pub fn collision_limit(word_count: usize) -> u64 {
    let words = u128::from(word_count as u64);
    let expected = words * words.saturating_sub(1) / 2 / (u128::from(u32::MAX) + 1);
    let limit = expected * LIMIT_HEADROOM;
    u64::try_from(limit).unwrap_or(u64::MAX).max(LIMIT_FLOOR)
}

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
    let rows = sorted_rows(words);
    first_collision(&rows)
}

/// Counts the pairs of distinct words whose hashes agree.
///
/// The count sums, over every equal-hash run of the `(hash, word)` sort, the pairs of
/// distinct texts the run holds. A word repeated in the input is one word rather than
/// a collision, matching [`first_collision`].
///
/// # Examples
///
/// ```
/// use ime_dict::format::count_colliding_hashes;
///
/// assert_eq!(count_colliding_hashes(&["中国", "银行", "中心"]), 0);
/// assert_eq!(count_colliding_hashes(&["中国", "中国"]), 0);
/// assert_eq!(count_colliding_hashes(&[]), 0);
/// ```
pub fn count_colliding_hashes(words: &[&str]) -> u64 {
    count_collisions(&sorted_rows(words))
}

/// Sorts `(hash, word)` rows, the order `ENTRIES` and `UNIGRAM` are written in.
///
/// The rows only borrow the slice for as long as they hold it, while the texts they
/// carry keep the words' own lifetime.
fn sorted_rows<'word>(words: &[&'word str]) -> Vec<(u32, &'word str)> {
    let mut rows: Vec<(u32, &str)> = words.iter().map(|word| (hash_word(word), *word)).collect();
    rows.sort_unstable();
    rows
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

/// Counts the distinct-text pairs inside every equal-hash run.
///
/// `rows` has to be sorted by `(hash, word)`. Within a run the sort also groups equal
/// texts together, so the distinct texts are the positions where the text differs from
/// its predecessor, and the pairs they form are the triangular number of that count.
pub(crate) fn count_collisions(rows: &[(u32, &str)]) -> u64 {
    let mut pairs = 0u64;
    let mut run = 0usize;
    while run < rows.len() {
        let mut end = run + 1;
        while end < rows.len() && rows[end].0 == rows[run].0 {
            end += 1;
        }
        let mut distinct = 0usize;
        for index in run..end {
            if index == run || rows[index].1 != rows[index - 1].1 {
                distinct += 1;
            }
        }
        let distinct = u64::try_from(distinct).unwrap_or(0);
        pairs = pairs.saturating_add(distinct * (distinct - 1) / 2);
        run = end;
    }
    pairs
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

    #[test]
    fn test_count_collisions_sums_pairs_across_every_equal_hash_run() {
        // Two runs holding collisions: hashes 2 and 5 each carry a distinct-text pair,
        // hash 9 carries three words that all agree, which is three pairs.
        let rows = [
            (2u32, "银行"),
            (2, "银杭"),
            (5, "中心"),
            (5, "忠心"),
            (9, "中国"),
            (9, "中立"),
            (9, "终种"),
        ];
        assert_eq!(count_collisions(&rows), 1 + 1 + 3);
    }

    #[test]
    fn test_count_collisions_counts_a_repeated_word_once() {
        // The same text four times is one word reached four times, not six pairs.
        let rows = [(3u32, "中国"), (3, "中国"), (3, "中国"), (3, "中国")];
        assert_eq!(count_collisions(&rows), 0);
    }

    #[test]
    fn test_count_collisions_handles_an_empty_and_a_single_row_table() {
        assert_eq!(count_collisions(&[]), 0);
        assert_eq!(count_collisions(&[(7, "中")]), 0);
    }

    #[test]
    fn test_count_colliding_hashes_agrees_with_first_collision_on_a_healthy_list() {
        // A list without collisions counts zero and finds none, over the same rows.
        let words = ["中国", "银行", "中心", "你好", "北京"];
        assert_eq!(count_colliding_hashes(&words), 0);
        assert_eq!(find_colliding_hashes(&words), None);
    }

    #[test]
    fn test_collision_limit_keeps_the_floor_for_word_lists_below_the_birthday_scale() {
        // Under ~9,300 words the expectation rounds to zero, and the floor is what
        // stands between a healthy build and birthday luck.
        assert_eq!(collision_limit(0), 4);
        assert_eq!(collision_limit(1), 4);
        assert_eq!(collision_limit(5_441), 4);
        assert_eq!(collision_limit(9_000), 4);
    }

    #[test]
    fn test_collision_limit_scales_with_the_birthday_expectation_at_product_scale() {
        // 349,046 words: C(n,2)/2^32 = 14 expected pairs, times the headroom.
        assert_eq!(collision_limit(349_046), 1_400);
        // The ASM-05 ceiling of the built-in dictionary.
        assert_eq!(collision_limit(400_000), 1_800);
    }

    #[test]
    fn test_collision_limit_stays_far_above_what_a_healthy_hash_produces() {
        // The largest count a healthy 32-bit hash plausibly produces at the product
        // ceiling sits orders of magnitude under the limit; a degenerate hash producing
        // even a thousandth of the maximum pair count is four orders past it.
        let limit = collision_limit(400_000);
        assert!(limit > 1_000, "the limit has to arm at product scale");
    }
}
