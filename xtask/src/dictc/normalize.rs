//! The weight-normalization pass of the compiler: the three frequency bands a
//! word's weight lands in, and the banded weight the unigram table consumes.
//!
//! Responsibility: map the single raw distribution of the word list (1 to tens of
//! millions, straight from the corpus statistics) onto the three-band shape the
//! ranking expects -- true frequency for single characters and for head
//! two-character words, one flat constant for the long tail and for two-character
//! noise -- and apply it where cross-length competition happens: the unigram
//! table the decoder scores with.
//!
//! # Why the bands feed the unigram channel only
//!
//! A container key's candidates always share the word's own character count (a
//! two-syllable key holds two-character words), so the within-key order the
//! `WORDLIST` section encodes never compares words of different lengths -- and
//! within one length, the corpus frequency is exactly the signal, which the
//! quality harness confirms: flattening the tail there *loses* half a point of
//! top1 because the order degrades to hash-order ties. Cross-length competition
//! lives in the `UNIGRAM` table the Viterbi scoring reads, where a long word's
//! corpus count used to outweigh a short word's: that is the channel the flat
//! tail constant and the core clamp are for. The raw weight therefore keeps
//! driving the `WORDLIST` and `ENTRIES` sections unchanged, while the banded
//! weight drives `UNIGRAM`.
//!
//! Boundaries: this pass rewrites no [`Word`] at all -- it is a pure mapping from
//! (character count, raw weight) to banded weight, consumed per word while the
//! sections are assembled. It is deterministic and order-independent, which is
//! what keeps the double-compile sha256 gate meaningful.

use anyhow::{Result, ensure};

/// The frequency band a word lands in after normalization. The band decides
/// whether the source frequency is kept verbatim or replaced by a constant,
/// mirroring rime-ice's base/ext split (true corpus frequency for short words,
/// presence-only for the long tail).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WeightBand {
    /// Single characters: real character frequency from the L1 source.
    CharFreq,
    /// Two-character words at or above the floor: keep the source frequency,
    /// clamped so the head cannot dominate the unigram mapping on its own.
    CoreTrueFreq,
    /// Three or more characters, and two-character words below the floor:
    /// flattened to the band constant.
    TailFlat,
}

/// Normalization rules; the defaults follow rime-ice's published practice
/// (tail flat = 100) and are overridable per run for tuning sweeps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NormalizeRules {
    /// The constant every tail-band word receives.
    pub(crate) tail_flat_weight: u32,
    /// Two-character words below this source frequency drop into the tail
    /// band: below it the frequency is noise, not signal. A word at the floor
    /// stays in the core band.
    pub(crate) core_min_freq: u32,
    /// Clamp that keeps any surviving core frequency from dominating the Q12
    /// mapping (the unigram table's ceiling).
    pub(crate) core_max_weight: u32,
}

impl Default for NormalizeRules {
    fn default() -> Self {
        Self {
            tail_flat_weight: 100,
            core_min_freq: 20,
            core_max_weight: 2_000_000,
        }
    }
}

impl NormalizeRules {
    /// The band a word of `characters` characters carrying `weight` lands in.
    ///
    /// Single characters always keep their frequency: the character table is the
    /// one place the corpus statistics are directly what a user types. Two
    /// characters are the core band only above the floor; everything longer is
    /// the tail, whatever its frequency, because a long word's corpus count says
    /// how often the *phrase* occurs in text -- which is the unigram channel's
    /// noise to flatten, not the within-key order's.
    pub(crate) fn band(&self, characters: usize, weight: u32) -> WeightBand {
        match characters {
            1 => WeightBand::CharFreq,
            2 if weight >= self.core_min_freq => WeightBand::CoreTrueFreq,
            _ => WeightBand::TailFlat,
        }
    }

    /// The banded weight for a word carrying `weight` in `band`: the value the
    /// unigram table consumes instead of the raw weight.
    pub(crate) fn weight_for(&self, band: WeightBand, weight: u32) -> u32 {
        match band {
            WeightBand::CharFreq => weight,
            WeightBand::CoreTrueFreq => weight.min(self.core_max_weight),
            WeightBand::TailFlat => self.tail_flat_weight,
        }
    }

    /// The banded replacement for one word's raw weight, from its character
    /// count. The only entry point [`super::build`] consumes per word.
    pub(crate) fn normalized_weight(&self, characters: usize, raw: u32) -> u32 {
        self.weight_for(self.band(characters, raw), raw)
    }

    /// Checks the rule combination the command line assembled.
    ///
    /// # Errors
    /// Returns an error naming the offending pair when the clamp sits under the
    /// floor (every core word would be flattened, which is not what any caller
    /// asking for a core band wants) or when the flat constant is zero (a
    /// zero-weight word would score below every real entry and silently vanish
    /// from the unigram ranking).
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.core_max_weight >= self.core_min_freq,
            "dictc: --core-min-freq ({}) exceeds the core clamp ({}); every core word \
             would be flattened",
            self.core_min_freq,
            self.core_max_weight
        );
        ensure!(
            self.tail_flat_weight > 0,
            "dictc: --tail-flat-weight must be at least 1; a zero tail weight would \
             rank flattened words below every real entry"
        );
        Ok(())
    }
}

/// The per-band audit the build log prints; deterministic order.
#[derive(Debug, Default)]
pub(crate) struct NormalizeReport {
    /// Words whose unigram weight kept their single-character frequency.
    pub(crate) char_freq: u32,
    /// Two-character words whose unigram weight kept (a clamped form of) their
    /// frequency.
    pub(crate) core_true_freq: u32,
    /// Words whose unigram weight was flattened to the band constant.
    pub(crate) tail_flat: u32,
}

impl NormalizeReport {
    /// Records one word's band.
    pub(crate) fn note(&mut self, band: WeightBand) {
        match band {
            WeightBand::CharFreq => self.char_freq += 1,
            WeightBand::CoreTrueFreq => self.core_true_freq += 1,
            WeightBand::TailFlat => self.tail_flat += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_two_char_true_freq_is_kept() {
        let rules = NormalizeRules::default();
        assert_eq!(rules.normalized_weight(2, 5_000), 5_000);
        assert_eq!(rules.band(2, 5_000), WeightBand::CoreTrueFreq);
    }

    #[test]
    fn test_normalize_core_clamp_caps_the_head() {
        let rules = NormalizeRules::default();
        assert_eq!(
            rules.normalized_weight(2, 19_260_000),
            rules.core_max_weight,
            "the clamp keeps the head from dominating the unigram mapping"
        );
    }

    #[test]
    fn test_normalize_long_words_are_flattened() {
        let rules = NormalizeRules::default();
        assert_eq!(rules.normalized_weight(3, 987_654), 100);
        assert_eq!(
            rules.normalized_weight(12, 1),
            100,
            "however rare or frequent"
        );
        assert_eq!(rules.band(3, 987_654), WeightBand::TailFlat);
    }

    #[test]
    fn test_normalize_low_freq_two_char_drops_into_the_tail() {
        let rules = NormalizeRules::default();
        assert_eq!(
            rules.normalized_weight(2, 19),
            100,
            "below the floor the frequency is noise"
        );
        assert_eq!(
            rules.normalized_weight(2, 20),
            20,
            "a word at the floor keeps its frequency"
        );
    }

    #[test]
    fn test_normalize_single_chars_keep_the_source_frequency() {
        let rules = NormalizeRules::default();
        assert_eq!(
            rules.normalized_weight(1, 19_260_000),
            19_260_000,
            "the character band keeps real frequency verbatim, unclamped"
        );
        assert_eq!(rules.normalized_weight(1, 10), 10);
    }

    #[test]
    fn test_normalize_report_counts_each_band() {
        let rules = NormalizeRules::default();
        let mut report = NormalizeReport::default();
        report.note(rules.band(1, 10));
        report.note(rules.band(2, 5_000));
        report.note(rules.band(3, 400));
        report.note(rules.band(2, 19));
        assert_eq!(report.char_freq, 1);
        assert_eq!(report.core_true_freq, 1);
        assert_eq!(report.tail_flat, 2);
    }

    #[test]
    fn test_normalize_is_order_independent() {
        let rules = NormalizeRules::default();
        let cases = [(1usize, 900_000u32), (2, 5_000), (3, 400), (2, 19)];
        let forward: Vec<u32> = cases
            .iter()
            .map(|&(characters, raw)| rules.normalized_weight(characters, raw))
            .collect();
        let backward: Vec<u32> = cases
            .iter()
            .rev()
            .map(|&(characters, raw)| rules.normalized_weight(characters, raw))
            .collect();
        assert_eq!(
            forward,
            backward.iter().rev().copied().collect::<Vec<u32>>(),
            "the same words in any order receive the same weights"
        );
    }

    #[test]
    fn test_normalize_rules_reject_an_inverted_window() {
        let rules = NormalizeRules {
            tail_flat_weight: 100,
            core_min_freq: 500,
            core_max_weight: 400,
        };
        let failure = rules.validate().expect_err("an inverted window is refused");
        assert!(failure.to_string().contains("core-min-freq"), "{failure}");
    }

    #[test]
    fn test_normalize_rules_reject_a_zero_tail_weight() {
        let rules = NormalizeRules {
            tail_flat_weight: 0,
            ..NormalizeRules::default()
        };
        let failure = rules.validate().expect_err("a zero flat weight is refused");
        assert!(
            failure.to_string().contains("tail-flat-weight"),
            "{failure}"
        );
    }

    #[test]
    fn test_normalize_default_rules_validate() {
        NormalizeRules::default()
            .validate()
            .expect("defaults are legal");
    }
}
