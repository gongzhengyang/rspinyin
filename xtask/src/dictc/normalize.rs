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

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};

use super::source::Word;

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
    /// What the trim pass did, when the build ran one.
    pub(crate) trim: TrimReport,
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

/// What the trim pass did, recorded in [`NormalizeReport::trim`] and printed by
/// the build log.
///
/// The two protected-band counts are witnesses of the pass's central invariant
/// -- single characters and head two-character words never enter the candidate
/// set -- and are therefore always zero; a nonzero value would mean the band
/// partition and the pass had come apart, which is exactly what the witness
/// fields exist to surface.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct TrimReport {
    /// The target the pass planned against, when one was requested.
    pub(crate) requested_target_mib: Option<f64>,
    /// Entries the pass dropped.
    pub(crate) dropped: u64,
    /// Floor bytes the drops provably free (`ENTRIES` + `UNIGRAM` + `STRPOOL`);
    /// the unprovable `FST` and `WORDLIST` savings are the margin, not the
    /// record. See the audit module's floor accounting.
    pub(crate) freed_floor_bytes: u64,
    /// Tail-band entries dropped; the only band the pass may touch.
    pub(crate) dropped_tail_flat: u64,
    /// Core-band entries dropped; a witness field, always zero.
    pub(crate) dropped_core_true_freq: u64,
    /// Character-band entries dropped; a witness field, always zero.
    pub(crate) dropped_char_freq: u64,
}

/// Drops the least valuable tail-band words until the projected container fits
/// `target_mib`, in place.
///
/// The band partition is the normalization's own: single characters and
/// two-character words at or above the core floor are protected and never enter
/// the candidate set, so what the pass may drop is exactly the flattened tail.
/// The drop set is planned by [`super::audit::plan_trim_drops`] over the audit
/// module's value ordering, which is total and input-order-free, and the floor
/// accounting makes the post-trim projection an upper bound of the real size --
/// see the audit module's documentation.
///
/// `projected_bytes` is the exact size the current word list compiles to (the
/// caller measures it on the first build) and `band_threshold` is the L3b
/// expansion floor the value model reads. A projection already inside the
/// target drops nothing.
///
/// # Errors
/// Returns an error when the target is unreachable: dropping every tail-band
/// candidate still leaves the projection past the target, which means the
/// target sits below the floor the protected bands set and no trim can honor
/// it.
pub(crate) fn trim_words(
    words: &mut Vec<Word>,
    rules: NormalizeRules,
    band_threshold: u32,
    target_mib: f64,
    projected_bytes: u64,
) -> Result<TrimReport> {
    let target_bytes = super::audit::target_bytes(target_mib)?;
    // The projection only has to be beaten when it is past the target; the
    // padding slack keeps the alignment jitter of six shrinking sections from
    // eating the fit promise.
    let savings_needed = if projected_bytes > target_bytes {
        projected_bytes
            .saturating_sub(target_bytes)
            .saturating_add(super::audit::TRIM_PADDING_SAFETY)
    } else {
        0
    };
    // Neighbors on the canonical key: the more words share a key, the deeper
    // each of them sits in that key's candidate list and the less likely a
    // user ever reaches them, which is what makes them the weaker keep.
    let mut key_counts: BTreeMap<&str, u32> = BTreeMap::new();
    for word in words.iter() {
        *key_counts.entry(word.key.as_str()).or_insert(0) += 1;
    }
    let candidates: Vec<(usize, super::audit::TrimValue)> = words
        .iter()
        .enumerate()
        .filter(|(_, word)| {
            rules.band(word.text.chars().count(), word.weight) == WeightBand::TailFlat
        })
        .map(|(index, word)| {
            let neighbors =
                key_counts.get(word.key.as_str()).copied().unwrap_or(1).saturating_sub(1);
            (index, super::audit::TrimValue {
                text: word.text.clone(),
                key: word.key.clone(),
                weight: word.weight,
                characters: word.text.chars().count(),
                in_expansion_band: word.weight >= band_threshold,
                neighbors,
            })
        })
        .collect();
    let plan = super::audit::plan_trim_drops(candidates, savings_needed);
    if plan.exhausted(savings_needed) {
        bail!(
            "dictc: trim target {target_mib:.2} MiB is below the floor the protected bands set: \
             dropping every one of the {} tail-flat candidates frees at most {} bytes and {} more \
             are needed; raise --trim-target-mib",
            plan.available,
            plan.freed_bytes,
            savings_needed.saturating_sub(plan.freed_bytes)
        );
    }
    let drop: BTreeSet<usize> = plan.drop_indices.iter().copied().collect();
    // The witness counts walk the same band function the candidate filter did,
    // so a nonzero protected-band count here would mean the two had disagreed.
    let mut dropped_char_freq = 0u64;
    let mut dropped_core_true_freq = 0u64;
    let mut dropped_tail_flat = 0u64;
    for index in &drop {
        let word = &words[*index];
        match rules.band(word.text.chars().count(), word.weight) {
            WeightBand::CharFreq => dropped_char_freq += 1,
            WeightBand::CoreTrueFreq => dropped_core_true_freq += 1,
            WeightBand::TailFlat => dropped_tail_flat += 1,
        }
    }
    // Removal preserves the relative order of the survivors, so the ids the
    // assembler assigns afterwards compact monotonically: within-key orders
    // that tie on weight keep their relative order, which is what keeps the
    // compiled ranking of every kept word bit-identical.
    let mut index = 0usize;
    words.retain(|_| {
        let keep = !drop.contains(&index);
        index += 1;
        keep
    });
    Ok(TrimReport {
        requested_target_mib: Some(target_mib),
        dropped: drop.len() as u64,
        freed_floor_bytes: plan.freed_bytes,
        dropped_tail_flat,
        dropped_core_true_freq,
        dropped_char_freq,
    })
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

    /// One parsed word for the trim tests; readings and flags are unused there.
    fn word(text: &str, key: &str, weight: u32) -> Word {
        Word {
            text: text.to_owned(),
            key: key.to_owned(),
            readings: Vec::new(),
            weight,
            flags: 0,
            polyphone: false,
        }
    }

    #[test]
    fn test_trim_words_drops_only_tail_band_candidates() {
        let rules = NormalizeRules::default();
        let mut words = vec![
            word("的", "de", 900_000),
            word("中国", "zhong'guo", 5_000),
            word("东西", "dong'xi", 19),
            word("不像话", "bu'xiang'hua", 10),
            word("稀有词", "xi'you'ci", 5),
        ];
        // The three tail candidates free 30 + 33 + 33 = 96 floor bytes; a
        // target 30 bytes past the projection asks for 94 with the slack, so
        // the whole tail goes and the protected bands stay.
        let projected = 1_000_000;
        let target = (projected - 30) as f64 / (1024.0 * 1024.0);
        let report = trim_words(&mut words, rules, 0, target, projected)
            .expect("the tail covers the requested savings");
        assert_eq!(report.dropped, 3);
        assert_eq!(report.dropped_tail_flat, 3);
        assert_eq!(report.dropped_char_freq, 0, "the character band is protected");
        assert_eq!(report.dropped_core_true_freq, 0, "the core band is protected");
        assert_eq!(
            report.freed_floor_bytes, 96,
            "the floor record is the linear rows of the dropped words"
        );
        let kept: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(kept, vec!["的", "中国"], "survivors keep their order");
    }

    #[test]
    fn test_trim_words_breaks_weight_ties_by_redundancy_then_text() {
        let rules = NormalizeRules::default();
        // Two tail words share a crowded key; one sits on a quiet one. Equal
        // weights, equal lengths, all out of band: the crowded pair goes first.
        let mut words = vec![
            word("乙词", "yi'ci", 7),
            word("拥挤甲", "yong'ji", 7),
            word("拥挤乙", "yong'ji", 7),
            word("安静词", "an'jing", 7),
        ];
        // Two drops free 2 x 33 = 66; a one-byte ask plus 64 slack = 65 needs
        // exactly two drops, so the ranking must stop at the crowded pair and
        // leave the quiet word -- and the shorter quiet word -- standing.
        let projected = 1_000_000;
        let target = (projected - 1) as f64 / (1024.0 * 1024.0);
        trim_words(&mut words, rules, 0, target, projected)
            .expect("the tail covers the requested savings");
        let kept: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(
            kept,
            vec!["乙词", "安静词"],
            "the crowded pair drops before the quiet word at equal weight"
        );
    }

    #[test]
    fn test_trim_words_is_independent_of_input_order() {
        let rules = NormalizeRules::default();
        let mut forward = vec![
            word("不像话", "bu'xiang'hua", 10),
            word("稀有词", "xi'you'ci", 5),
            word("东西", "dong'xi", 19),
        ];
        let mut backward = vec![
            word("东西", "dong'xi", 19),
            word("稀有词", "xi'you'ci", 5),
            word("不像话", "bu'xiang'hua", 10),
        ];
        let target = (1_000_000 - 1) as f64 / (1024.0 * 1024.0);
        trim_words(&mut forward, rules, 0, target, 1_000_000).expect("forward trims");
        trim_words(&mut backward, rules, 0, target, 1_000_000).expect("backward trims");
        let kept: Vec<String> = forward
            .iter()
            .map(|word| word.text.clone())
            .collect();
        let kept_back: Vec<String> = backward
            .iter()
            .map(|word| word.text.clone())
            .collect();
        assert_eq!(
            kept, kept_back,
            "the same words survive whatever order the rows arrived in"
        );
        assert_eq!(kept, vec!["东西"], "the lightest tail word goes first");
    }

    #[test]
    fn test_trim_words_refuses_a_target_below_the_protected_floor() {
        let rules = NormalizeRules::default();
        let mut words = vec![
            word("的", "de", 900_000),
            word("中国", "zhong'guo", 5_000),
            word("东西", "dong'xi", 19),
        ];
        // One tail candidate frees 30 bytes; an ask of 4 KiB past the
        // projection cannot be met even by dropping it.
        let target = (1_000_000.0 - 4_100.0) / (1024.0 * 1024.0);
        let failure =
            trim_words(&mut words, rules, 0, target, 1_000_000).expect_err("the target is unreachable");
        let message = failure.to_string();
        assert!(
            message.contains("below the floor the protected bands set"),
            "the refusal names the floor: {message}"
        );
        assert_eq!(words.len(), 3, "a refused trim drops nothing");
    }

    #[test]
    fn test_trim_words_is_a_no_op_when_the_projection_already_fits() {
        let rules = NormalizeRules::default();
        let mut words = vec![
            word("的", "de", 900_000),
            word("中国", "zhong'guo", 5_000),
            word("东西", "dong'xi", 19),
        ];
        let target = (1_000_000.0 + 64.0) / (1024.0 * 1024.0);
        let report = trim_words(&mut words, rules, 0, target, 1_000_000)
            .expect("a projection inside the target needs no trim");
        assert_eq!(report.dropped, 0);
        assert_eq!(report.freed_floor_bytes, 0);
        assert_eq!(words.len(), 3);
    }

    #[test]
    fn test_trim_words_witness_counts_stay_zero_for_protected_bands() {
        let rules = NormalizeRules::default();
        // Two tail candidates free 33 + 33 = 66; a one-byte ask plus the slack
        // needs exactly those two drops, and the witness fields record that
        // every one of them came from the tail.
        let mut words = vec![
            word("不像话", "bu'xiang'hua", 10),
            word("稀有词", "xi'you'ci", 5),
        ];
        let target = (1_000_000 - 1) as f64 / (1024.0 * 1024.0);
        let report = trim_words(&mut words, rules, 0, target, 1_000_000)
            .expect("the two tail candidates cover the small ask");
        assert_eq!(report.dropped, 2);
        assert_eq!(report.requested_target_mib, Some(target));
        assert_eq!(report.dropped_tail_flat, 2);
        assert_eq!(report.dropped_char_freq, 0);
        assert_eq!(report.dropped_core_true_freq, 0);
    }
}
