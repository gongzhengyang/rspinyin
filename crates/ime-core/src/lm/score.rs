//! The fixed-point scoring layer: an integer `log2` and the weighted edge and
//! path score the decoder ranks candidates with.
//!
//! Responsibility: turn the signals the decoder already holds -- the language
//! model's log probabilities, the user's own frequency counts, a word's
//! character count and the number of words a path is cut into -- into one `i32`
//! score. Nothing here opens a file, reads a clock or touches global state, so
//! the same inputs always produce the same order.
//!
//! Boundaries: this module owns the arithmetic and the weights and nothing
//! else. It does not know what a lattice is, it never looks a word up, and it
//! does not decide how many candidates survive; the decoder above it does that.
//!
//! # Fixed-point discipline
//!
//! Every quantity is an integer. [`Q`] is the Q8.8 unit: `1.0` is `256`. A
//! weight times a log probability is a product of two Q8.8 values, so a whole
//! edge score is Q16.16, and it stays well inside `i32` for any input the
//! decoder can produce. Floating point is excluded here on purpose: a candidate
//! order that drifts with rounding is not reproducible, and reproducibility is
//! part of the decode contract.
//!
//! # Why the numbers are constants
//!
//! The weights, the bigram miss penalty and the user-frequency cap are fixed
//! values rather than anything measured at run time. [`Scorer`] is a pure
//! function of its arguments, so changing any of them has to be a change to this
//! file -- reviewed and re-measured with the offline tuner -- and never
//! something the environment can influence.

use ime_types::{ImeError, LanguageModel, UserFreqSource};

/// The Q8.8 unit: `1.0` is this value.
pub const Q: i32 = 256;

/// The value [`log2_q8`] answers with when the ratio it is asked about is
/// impossible: a zero numerator or a zero denominator.
///
/// It is `log2` of `2^-8` in Q8.8, which is far below any probability a real
/// language model assigns, so a word that reaches it is ranked last without
/// being removed from the lattice.
pub const LOG2_FLOOR: i32 = -2048;

/// Upper bound on the scaled user-frequency term, in Q8.8.
///
/// The term stops growing once the count passes about 65500, which is what keeps
/// a word the user has typed a hundred thousand times from holding the top of
/// the candidate list for good.
pub const USER_TERM_CAP: i32 = 2 * Q;

/// Returns `log2(prob_num / prob_den)` in Q8.8.
///
/// The ratio is normalized to `[1, 2)` and read off an eight-segment table with
/// linear interpolation, so the cost is one division, one table lookup and two
/// integer multiplies -- no floating point, and no branch that depends on the
/// data. The result is within 2 of the correctly rounded double-precision
/// reference over every sample the accuracy test sweeps, 0.008 in `log2` units.
///
/// A zero numerator or denominator has no logarithm, so it answers
/// [`LOG2_FLOOR`] instead of failing; the general result is not clamped, because
/// a caller that ranks candidates needs the real magnitude of a small
/// probability.
///
/// # Panics
///
/// Never panics: every shift is bounded by the width of its operand and every
/// arithmetic step saturates.
///
/// # Examples
///
/// ```
/// use ime_core::lm::log2_q8;
///
/// assert_eq!(log2_q8(2, 1), 256); // log2(2) == 1.0
/// assert_eq!(log2_q8(1, 4), -512); // log2(1/4) == -2.0
/// assert_eq!(log2_q8(0, 5), -2048); // no logarithm of zero
/// ```
pub fn log2_q8(prob_num: u32, prob_den: u32) -> i32 {
    if prob_num == 0 || prob_den == 0 {
        return LOG2_FLOOR;
    }
    let (exponent, mantissa_q16) = split_ratio(prob_num, prob_den);
    exponent
        .saturating_mul(Q)
        .saturating_add(log2_mantissa_q8(mantissa_q16))
}

/// Splits `num / den` into `(exponent, mantissa)`, where the ratio equals
/// `mantissa / 2^16` times `2^exponent` and the mantissa is in `[65536, 131072)`,
/// that is a Q16.16 value in `[1, 2)`.
///
/// Both operands are shifted to the top of the word first, which makes the
/// integer division below exact to the bit and folds the scaling into the
/// exponent; that is why no precision is lost before the table lookup.
fn split_ratio(num: u32, den: u32) -> (i32, u32) {
    let num_shift = num.leading_zeros();
    let den_shift = den.leading_zeros();
    let num_n = u64::from(num) << num_shift;
    let den_n = u64::from(den) << den_shift;
    // Both operands are now in `[2^31, 2^32)`, so the quotient is in
    // `(2^31, 2^33)` and the shift below cannot overflow a `u64`.
    let ratio_q32 = (num_n << 32) / den_n;
    let (top_exponent, normalized) = if ratio_q32 >= (1u64 << 32) {
        (32, ratio_q32 >> 1)
    } else {
        (31, ratio_q32)
    };
    // `normalized` is in `[2^31, 2^32)`, so dropping 15 bits lands the mantissa
    // in Q16.16 in `[2^16, 2^17)`.
    let mantissa_q16 = (normalized >> 15) as u32;
    let exponent = top_exponent - 32 + den_shift as i32 - num_shift as i32;
    (exponent, mantissa_q16)
}

/// Fractional bits the mantissa keeps while the segment table is interpolated.
const SEGMENT_SHIFT: u32 = 13;

/// Mask selecting the bits of the mantissa that interpolate within a segment.
const SEGMENT_MASK: u32 = (1 << SEGMENT_SHIFT) - 1;

/// Q16.16 representation of `1.0`.
const MANTISSA_Q16_ONE: u32 = 1 << 16;

/// `log2` at the nine boundaries of the eight equal segments of `[1, 2)`, in
/// Q8.8.
///
/// Eight segments are enough: over one eighth of an octave the curve is nearly
/// straight, and the largest gap between it and the straight line through the
/// boundaries is 0.0025, that is 0.64 in Q8.8. The boundaries themselves are
/// exact, so they cost nothing.
const LOG2_SEGMENTS: [i32; 9] = [0, 44, 82, 118, 150, 179, 207, 232, 256];

/// `log2` of a Q16.16 mantissa in `[1, 2)`, in Q8.8.
fn log2_mantissa_q8(mantissa_q16: u32) -> i32 {
    let offset = mantissa_q16 - MANTISSA_Q16_ONE;
    let index = (offset >> SEGMENT_SHIFT) as usize;
    let fraction = offset & SEGMENT_MASK;
    let low = LOG2_SEGMENTS[index];
    let high = LOG2_SEGMENTS[index + 1];
    // Rounded, not truncated: truncation alone would cost a whole Q8.8 unit on
    // the steep first segment, which is a third of the error budget.
    let step = high - low;
    low + (((step * fraction as i32) + (1 << (SEGMENT_SHIFT - 1))) >> SEGMENT_SHIFT)
}

/// The five weights that turn the raw signals into one score.
///
/// Every field is a Q8.8 weight and every one of them is non-negative; a
/// negative weight inverts the meaning of its signal, so [`Scorer::new`] refuses
/// it rather than producing a ranking nobody can explain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreWeights {
    /// Weight of the language model's unigram term.
    pub uni: i32,
    /// Weight of the language model's bigram term.
    pub bi: i32,
    /// Weight of the per-extra-character length bonus.
    pub len: i32,
    /// Weight of the per-segment path penalty.
    pub seg: i32,
    /// Weight of the user-frequency term.
    pub user: i32,
}

impl Default for ScoreWeights {
    /// The shipped tuple `(1.0, 0.6, 0.35, 0.5, 0.8)` in Q8.8.
    ///
    /// The values are the starting point the offline tuner walks outwards from,
    /// and they are what a `DecodeConfig` carries until a tuning run replaces
    /// them.
    fn default() -> Self {
        Self {
            uni: 256,
            bi: 154,
            len: 90,
            seg: 128,
            user: 205,
        }
    }
}

/// Combines the language model, the user's frequency and the shape of a path
/// into the integer score the decoder ranks with.
///
/// # Concurrency
///
/// `Send + Sync` and reentrant: a scorer is five integers and every method is a
/// pure function of its arguments. No method blocks, allocates or takes a lock,
/// so a scorer can be shared across threads and called from the decode hot path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scorer {
    w: ScoreWeights,
}

impl Scorer {
    /// Builds a scorer from an explicit weight tuple.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::ConfigInvalid`] naming the offending field when any
    /// weight is negative.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::lm::{ScoreWeights, Scorer};
    ///
    /// let scorer = Scorer::new(ScoreWeights::default()).unwrap_or_default();
    /// assert_eq!(scorer.weights().uni, 256);
    ///
    /// let negative = ScoreWeights {
    ///     seg: -1,
    ///     ..ScoreWeights::default()
    /// };
    /// assert!(Scorer::new(negative).is_err());
    /// ```
    pub fn new(w: ScoreWeights) -> Result<Self, ImeError> {
        for (key, value) in [
            ("lm.lambda_uni", w.uni),
            ("lm.lambda_bi", w.bi),
            ("lm.lambda_len", w.len),
            ("lm.lambda_seg", w.seg),
            ("lm.lambda_user", w.user),
        ] {
            if value < 0 {
                return Err(ImeError::ConfigInvalid {
                    key: String::from(key),
                    reason: String::from("weight must not be negative"),
                });
            }
        }
        Ok(Self { w })
    }

    /// Returns the weight tuple the scorer was built with.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn weights(&self) -> ScoreWeights {
        self.w
    }

    /// Scores one lattice edge: `word`, reached from `prev_word`.
    ///
    /// The five terms are, in Q16.16:
    ///
    /// ```text
    /// uni  * unigram(word)
    /// bi   * bigram(prev_word, word)
    /// len  * (char_count - 1) * 256
    /// user * min(log2(1 + freq) / 8, 2) * 256
    /// ```
    ///
    /// A word with no predecessor contributes no bigram term: the model carries
    /// no sentence-start symbol, and reusing the unigram there would weight the
    /// first word of every path twice, making the comparison between two first
    /// words depend on the unigram weight twice over.
    ///
    /// # Panics
    ///
    /// Never panics: every arithmetic step saturates instead of overflowing.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::lm::{Scorer, ScoreWeights};
    /// use ime_types::{LanguageModel, UserFreqSource};
    ///
    /// struct Constant;
    /// impl LanguageModel for Constant {
    ///     fn unigram(&self, _word: &str) -> i32 { -256 }
    ///     fn bigram(&self, _prev: &str, _word: &str) -> i32 { -128 }
    /// }
    /// impl UserFreqSource for Constant {
    ///     fn freq(&self, _key: &str) -> u32 { 0 }
    ///     fn record(&self, _key: &str, _weight_hint: u16) {}
    ///     fn is_user_word(&self, _key: &str) -> bool { false }
    /// }
    ///
    /// let scorer = Scorer::new(ScoreWeights::default()).unwrap_or_default();
    /// // One character, no predecessor, no user history: only the unigram term.
    /// assert_eq!(scorer.edge_score(&Constant, &Constant, None, "ni", 1), 256 * -256);
    /// ```
    pub fn edge_score(
        &self,
        lm: &dyn LanguageModel,
        uf: &dyn UserFreqSource,
        prev_word: Option<&str>,
        word: &str,
        char_count: u16,
    ) -> i32 {
        let bigram = match prev_word {
            Some(prev) => lm.bigram(prev, word),
            None => 0,
        };
        let extra_characters = i32::from(char_count.saturating_sub(1));
        self.w
            .uni
            .saturating_mul(lm.unigram(word))
            .saturating_add(self.w.bi.saturating_mul(bigram))
            .saturating_add(
                self.w
                    .len
                    .saturating_mul(extra_characters)
                    .saturating_mul(Q),
            )
            .saturating_add(user_term(uf.freq(word), self.w.user))
    }

    /// Penalizes a path cut into `seg_count` words.
    ///
    /// The penalty is linear in the segment count, which is what lets a decoder
    /// either apply it once at the end of a path or charge one segment per edge
    /// and reach the same total.
    ///
    /// # Panics
    ///
    /// Never panics: every arithmetic step saturates instead of overflowing.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::lm::{Scorer, ScoreWeights, Q};
    ///
    /// let scorer = Scorer::new(ScoreWeights::default()).unwrap_or_default();
    /// // Two segments at 0.5 each.
    /// assert_eq!(scorer.path_penalty(2), -2 * 128 * Q);
    /// assert_eq!(scorer.path_penalty(0), 0);
    /// ```
    pub fn path_penalty(&self, seg_count: u16) -> i32 {
        self.w
            .seg
            .saturating_mul(Q)
            .saturating_mul(i32::from(seg_count))
            .saturating_neg()
    }
}

/// The user-frequency contribution to an edge score, in Q16.16.
///
/// The term is `lambda_user` times `log2(1 + freq) / 8`: a word the user has
/// committed once earns `lambda_user * 1/8`, one committed eight times earns
/// `lambda_user * 3/8`. The logarithm is what keeps the signal useful -- a word
/// typed a hundred times should outrank one typed once, but not by a hundred --
/// and the division by eight brings it into the same order of magnitude as the
/// language model's log probabilities, so the two terms can be compared at all.
///
/// The scaled term is capped at `2`, which the count reaches at about 65500: a
/// word the user typed a hundred thousand times keeps a strong but finite
/// advantage instead of holding the top of the candidate list for good.
fn user_term(freq: u32, lambda_user: i32) -> i32 {
    let scaled = log2_q8(freq.saturating_add(1), 1) / 8;
    lambda_user.saturating_mul(scaled.min(USER_TERM_CAP))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// Number of sample points the accuracy test sweeps.
    const ACCURACY_SAMPLES: u32 = 4096;

    /// A language model whose scores come straight from tables.
    ///
    /// The bigrams are nested rather than keyed by a tuple, because a `BTreeMap`
    /// keyed by `(&str, &str)` cannot be looked up with two borrowed strings: the
    /// lookup needs the key type to borrow a tuple that only exists at the call
    /// site.
    struct MockLm {
        unigrams: BTreeMap<&'static str, i32>,
        bigrams: BTreeMap<&'static str, BTreeMap<&'static str, i32>>,
    }

    impl MockLm {
        /// A model with the given unigram scores and no bigram counts at all.
        fn unigrams(rows: &[(&'static str, i32)]) -> Self {
            Self {
                unigrams: rows.iter().copied().collect(),
                bigrams: BTreeMap::new(),
            }
        }
    }

    impl LanguageModel for MockLm {
        fn unigram(&self, word: &str) -> i32 {
            self.unigrams.get(word).copied().unwrap_or(0)
        }

        fn bigram(&self, prev: &str, word: &str) -> i32 {
            self.bigrams
                .get(prev)
                .and_then(|row| row.get(word))
                .copied()
                .unwrap_or(0)
        }
    }

    /// A user-frequency source with a fixed count per key.
    struct MockUserFreq {
        counts: BTreeMap<&'static str, u32>,
    }

    impl MockUserFreq {
        /// A source that has never recorded anything.
        fn empty() -> Self {
            Self {
                counts: BTreeMap::new(),
            }
        }
    }

    impl UserFreqSource for MockUserFreq {
        fn freq(&self, key: &str) -> u32 {
            self.counts.get(key).copied().unwrap_or(0)
        }

        fn record(&self, _key: &str, _weight_hint: u16) {}

        fn is_user_word(&self, key: &str) -> bool {
            self.counts.contains_key(key)
        }
    }

    /// One of the [`ACCURACY_SAMPLES`] `(numerator, denominator)` pairs.
    ///
    /// Two odd multipliers walk the pair across the whole `u32` range instead of
    /// along a lattice, and the `| 1` keeps both sides non-zero: a zero side is
    /// the documented floor case, which has its own test.
    fn sample_ratio(index: u32) -> (u32, u32) {
        let left = index.wrapping_mul(2_654_435_761) | 1;
        let right = index.wrapping_mul(40_503) | 1;
        if index % 2 == 0 {
            (left, right)
        } else {
            (right, left)
        }
    }

    /// A scorer built from the shipped defaults.
    fn default_scorer() -> Scorer {
        Scorer::new(ScoreWeights::default()).expect("the defaults are legal")
    }

    #[test]
    fn test_log2_q8_matches_the_float_reference_over_4096_samples() {
        let mut worst = 0i32;
        let mut worst_at = (0u32, 0u32);
        let mut below_one = 0u32;
        let mut above_one = 0u32;
        for index in 0..ACCURACY_SAMPLES {
            let (num, den) = sample_ratio(index);
            let reference = (f64::from(num) / f64::from(den)).log2() * 256.0;
            let error = (log2_q8(num, den) - reference.round() as i32).abs();
            if error > worst {
                worst = error;
                worst_at = (num, den);
            }
            if num < den {
                below_one += 1;
            } else if num > den {
                above_one += 1;
            }
        }
        assert!(
            worst <= 2,
            "log2_q8 is off by {worst} at {worst_at:?}, budget is 2"
        );
        // A sweep that only walked one direction of the ratio would pass the
        // assertion above without testing the negative exponents at all.
        assert!(
            below_one > 0 && above_one > 0,
            "the sweep missed a direction"
        );
    }

    #[test]
    fn test_log2_q8_is_exact_on_the_powers_of_two() {
        let cases = [
            (1u32, 1u32, 0i32),
            (2, 1, 256),
            (4, 1, 512),
            (1024, 1, 2560),
            (1, 2, -256),
            (1, 4, -512),
            (1, 1024, -2560),
            (3, 3, 0),
        ];
        for (num, den, expected) in cases {
            assert_eq!(log2_q8(num, den), expected, "log2_q8({num}, {den})");
        }
    }

    #[test]
    fn test_log2_q8_returns_the_floor_for_a_zero_operand() {
        assert_eq!(log2_q8(0, 1), LOG2_FLOOR);
        assert_eq!(log2_q8(0, u32::MAX), LOG2_FLOOR);
        assert_eq!(log2_q8(1, 0), LOG2_FLOOR);
        assert_eq!(log2_q8(u32::MAX, 0), LOG2_FLOOR);
    }

    #[test]
    fn test_log2_q8_grows_with_the_ratio_and_never_overflows() {
        assert!(log2_q8(2, 1) > log2_q8(1, 1));
        assert!(log2_q8(1, 1) > log2_q8(1, 2));
        // The extreme corners of the argument range must answer, not panic.
        let widest = log2_q8(u32::MAX, 1);
        let narrowest = log2_q8(1, u32::MAX);
        assert_eq!(widest, 32 * Q);
        assert_eq!(narrowest, -32 * Q);
        assert_eq!(log2_q8(u32::MAX, u32::MAX), 0);
    }

    #[test]
    fn test_split_ratio_keeps_the_mantissa_in_one_octave() {
        for index in 0..ACCURACY_SAMPLES {
            let (num, den) = sample_ratio(index);
            let (_, mantissa) = split_ratio(num, den);
            assert!(
                (65_536..131_072).contains(&mantissa),
                "mantissa {mantissa} out of [1, 2) for ({num}, {den})"
            );
        }
    }

    #[test]
    fn test_scorer_new_rejects_a_negative_weight() {
        let fields = [
            (
                "lm.lambda_uni",
                ScoreWeights {
                    uni: -1,
                    ..ScoreWeights::default()
                },
            ),
            (
                "lm.lambda_bi",
                ScoreWeights {
                    bi: -256,
                    ..ScoreWeights::default()
                },
            ),
            (
                "lm.lambda_len",
                ScoreWeights {
                    len: -1,
                    ..ScoreWeights::default()
                },
            ),
            (
                "lm.lambda_seg",
                ScoreWeights {
                    seg: -1,
                    ..ScoreWeights::default()
                },
            ),
            (
                "lm.lambda_user",
                ScoreWeights {
                    user: i32::MIN,
                    ..ScoreWeights::default()
                },
            ),
        ];
        for (key, weights) in fields {
            let error = Scorer::new(weights).expect_err("a negative weight must be refused");
            assert!(
                error.to_string().starts_with("config/invalid:"),
                "unexpected code: {error}"
            );
            assert!(error.to_string().contains(key), "unexpected key: {error}");
        }
    }

    #[test]
    fn test_scorer_new_accepts_zero_and_reports_the_weights() {
        let zero = ScoreWeights {
            uni: 0,
            bi: 0,
            len: 0,
            seg: 0,
            user: 0,
        };
        let scorer = Scorer::new(zero).expect("zero is not negative");
        assert_eq!(scorer.weights(), zero);
        assert_eq!(Scorer::default().weights(), ScoreWeights::default());
    }

    #[test]
    fn test_default_weights_are_the_documented_tuple() {
        let weights = ScoreWeights::default();
        assert_eq!(weights.uni, 256); // 1.0
        assert_eq!(weights.bi, 154); // 0.6
        assert_eq!(weights.len, 90); // 0.35
        assert_eq!(weights.seg, 128); // 0.5
        assert_eq!(weights.user, 205); // 0.8
    }

    #[test]
    fn test_edge_score_is_monotone_in_the_unigram_score() {
        let scorer = default_scorer();
        let user = MockUserFreq::empty();
        let common = MockLm::unigrams(&[("de", -512)]);
        let rare = MockLm::unigrams(&[("de", -2048)]);
        let common_score = scorer.edge_score(&common, &user, None, "de", 1);
        let rare_score = scorer.edge_score(&rare, &user, None, "de", 1);
        assert!(
            common_score > rare_score,
            "a likelier word must score higher: {common_score} vs {rare_score}"
        );
        // The gap is the unigram weight times the difference of the two scores.
        assert_eq!(common_score - rare_score, 256 * 1536);
    }

    #[test]
    fn test_edge_score_rewards_the_longer_word() {
        let scorer = default_scorer();
        let lm = MockLm::unigrams(&[]);
        let user = MockUserFreq::empty();
        let one = scorer.edge_score(&lm, &user, None, "hao", 1);
        let two = scorer.edge_score(&lm, &user, None, "hao", 2);
        let four = scorer.edge_score(&lm, &user, None, "hao", 4);
        assert_eq!(two - one, 90 * Q); // 0.35
        assert_eq!(four - one, 3 * 90 * Q); // 1.05
        // A zero character count cannot underflow into a bonus.
        assert_eq!(scorer.edge_score(&lm, &user, None, "", 0), one);
    }

    #[test]
    fn test_edge_score_uses_the_bigram_when_a_predecessor_is_given() {
        let scorer = default_scorer();
        let user = MockUserFreq::empty();
        let mut lm = MockLm::unigrams(&[("hao", -256)]);
        lm.bigrams.entry("ni").or_default().insert("hao", -64);
        let with_prev = scorer.edge_score(&lm, &user, Some("ni"), "hao", 1);
        let without_prev = scorer.edge_score(&lm, &user, None, "hao", 1);
        assert_eq!(with_prev - without_prev, 154 * -64);
    }

    #[test]
    fn test_edge_score_clamps_the_user_term_at_a_million_hits() {
        let scorer = default_scorer();
        let lm = MockLm::unigrams(&[]);
        let mut counts = BTreeMap::new();
        counts.insert("nihao", 1_000_000u32);
        let user = MockUserFreq { counts };

        let plain = scorer.edge_score(&lm, &MockUserFreq::empty(), None, "nihao", 2);
        let learned = scorer.edge_score(&lm, &user, None, "nihao", 2);
        // The cap is lambda_user times 2, in Q16.16.
        assert_eq!(learned - plain, 205 * 2 * Q);

        let mut saturating = BTreeMap::new();
        saturating.insert("nihao", u32::MAX);
        let extreme = MockUserFreq { counts: saturating };
        assert_eq!(
            scorer.edge_score(&lm, &extreme, None, "nihao", 2),
            learned,
            "the term must not grow past the cap"
        );
    }

    #[test]
    fn test_user_term_starts_at_zero_and_scales_with_the_count() {
        assert_eq!(user_term(0, 205), 0);
        // One commit earns lambda_user / 8, in Q16.16.
        assert_eq!(user_term(1, 205), 205 * 32);
        // Eight commits earn lambda_user * 3/8.
        assert_eq!(user_term(8, 205), 205 * 101);
        // A zero weight silences the signal without changing anything else.
        assert_eq!(user_term(1_000_000, 0), 0);
    }

    #[test]
    fn test_path_penalty_is_linear_in_the_segment_count() {
        let scorer = default_scorer();
        let per_segment = scorer.path_penalty(1);
        assert_eq!(per_segment, -128 * Q);
        for segments in 0..=64u16 {
            assert_eq!(
                scorer.path_penalty(segments),
                per_segment * i32::from(segments),
                "penalty of {segments} segments"
            );
        }
        assert!(scorer.path_penalty(3) < scorer.path_penalty(2));
    }

    #[test]
    fn test_path_penalty_of_a_zero_weight_is_zero() {
        let scorer = Scorer::new(ScoreWeights {
            seg: 0,
            ..ScoreWeights::default()
        })
        .expect("zero is not negative");
        assert_eq!(scorer.path_penalty(0), 0);
        assert_eq!(scorer.path_penalty(u16::MAX), 0);
    }
}
