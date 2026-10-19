//! The container-building half of the compiler: keys, sections and the
//! measurements taken while assembling them.
//!
//! Responsibility: turn parsed words into the five payloads of the container --
//! the L3b expansion, the key -> word mapping, the entry table, the string pool
//! and the unigram table -- and resolve the frequency band that decides which
//! words are expanded.
//!
//! Boundaries: this layer never reads a source file's text (that is [`super::source`])
//! and never writes the container (that is `ime_dict::format::writer`). It is
//! deterministic by construction: every map it iterates is ordered, so the same
//! words and thresholds always produce the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Result;
use ime_dict::format::{
    DictEntry, ENTRY_SIZE, MAX_SYL_COUNT, MAX_WORDS_PER_KEY, PROB_Q12_MAX, UNIGRAM_ENTRY_SIZE,
    collision_limit, count_colliding_hashes, find_colliding_hashes, hash_word, pack_fst_value,
};

use super::normalize::{NormalizeReport, NormalizeRules};
use super::source::{Word, baseline_key, compose_readings};

/// Weight multiplier applied to the key a polyphone correction names.
///
/// The correction table is a ranking aid, not a correctness source: the key it
/// names is ranked above the baseline key for the same word, so the reading the
/// table states wins the tie inside that word's own candidate list.
const POLYPHONE_WEIGHT_FACTOR: u32 = 4;

/// Weight floor used when the frequency source is unavailable, so the band still
/// selects the head of the list instead of silently expanding everything.
const FALLBACK_BAND_THRESHOLD: u32 = 100;

/// The reading-frequency floor a stated reading must clear to survive.
///
/// Rime-ice's discipline, in percent: a word whose reading for a character
/// disagrees with that character's top reading keeps the reading only when the
/// reading carries at least `freq_ratio_pct` percent of the top reading's
/// weighted usage across the word list itself. Below the line the word is
/// flattened onto the top reading -- the data-driven counterpart of the manual
/// L3c table, which stays the authority for the readings the floor rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolyphoneRule {
    /// Minimum percent of the character's top-reading usage a divergent reading
    /// needs; `5` is rime-ice's 5:100 line.
    pub(crate) freq_ratio_pct: u8,
}

/// What the automatic polyphone check decided, for the manifest and the gate.
///
/// The three buckets partition the words the pass examined -- every word whose
/// stated reading disagrees with some character's top reading -- and
/// `assigned + flattened + manual == total` is the no-silent-loss invariant the
/// quality gate asserts over the manifest.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PolyphoneAutoCounts {
    /// Divergent words whose readings cleared the floor and were kept.
    pub(crate) assigned: u64,
    /// Divergent words flattened onto their characters' top readings.
    pub(crate) flattened: u64,
    /// Divergent words the L3c table states a reading for, left to it.
    pub(crate) manual: u64,
    /// Every word the pass examined; the buckets' sum.
    pub(crate) total: u64,
}

/// Applies the automatic polyphone check to one word list, in place.
///
/// The evidence is the list itself: every word votes its weight for the
/// `(character, reading)` pairs its key names, and a stated reading survives
/// against its character's top reading by [`PolyphoneRule`]'s share of that
/// vote. A word the L3c table names is exempt whole -- a row that took the
/// trouble to state a reading has already been hand-verified -- and a word
/// whose key already uses every character's top reading has nothing to check.
/// Flattening replaces the key with the L3a baseline; the per-character reading
/// lists are untouched, so the L3b expansion still reaches the readings the
/// word lost from its baseline.
///
/// Deterministic and order-free: the vote is a sum over an ordered map, and
/// each word's verdict depends on the votes alone, never on where the word
/// sits in the list.
pub(crate) fn apply_polyphone_rule(
    words: &mut [Word],
    l1: &BTreeMap<char, Vec<String>>,
    manual: &BTreeSet<String>,
    rule: PolyphoneRule,
) -> PolyphoneAutoCounts {
    // One vote per word: each `(character, reading)` pair a key names gains the
    // word's weight. A `BTreeMap` keeps the sum independent of iteration order.
    let mut votes: BTreeMap<(char, &str), u64> = BTreeMap::new();
    for word in words.iter() {
        let weight = u64::from(word.weight);
        for (character, syllable) in word.text.chars().zip(word.key.split('\'')) {
            *votes.entry((character, syllable)).or_default() += weight;
        }
    }
    // The verdicts are decided against the immutable list and applied after the
    // borrow ends: a flatten replaces the key with the L3a baseline, and every
    // examined word's characters are in the L1 table -- a divergence is only
    // detectable against a top reading -- so the recomposition cannot fail.
    let mut counts = PolyphoneAutoCounts::default();
    let mut flattened_keys: Vec<Option<String>> = Vec::with_capacity(words.len());
    for word in words.iter() {
        let divergences: Vec<(char, &str)> = word
            .text
            .chars()
            .zip(word.key.split('\''))
            .filter(|(character, syllable)| {
                l1.get(character)
                    .and_then(|readings| readings.first())
                    .is_some_and(|top| {
                        // The top reading is spelled tonelessly; the stated one went
                        // through the same fold, so the strings compare directly.
                        top != *syllable
                    })
            })
            .collect();
        if divergences.is_empty() {
            flattened_keys.push(None);
            continue;
        }
        counts.total += 1;
        if manual.contains(&word.text) {
            counts.manual += 1;
            flattened_keys.push(None);
            continue;
        }
        let survives = divergences.iter().all(|(character, syllable)| {
            let stated = votes.get(&(*character, *syllable)).copied().unwrap_or(0);
            let Some(top) = l1.get(character).and_then(|readings| readings.first()) else {
                return false;
            };
            let top_votes = votes.get(&(*character, top.as_str())).copied().unwrap_or(0);
            if top_votes == 0 {
                // Nothing reads the character at the top reading, so there is
                // no majority to lose to; the stated reading stands.
                return true;
            }
            stated.saturating_mul(100) / top_votes >= u64::from(rule.freq_ratio_pct)
        });
        if survives {
            counts.assigned += 1;
            flattened_keys.push(None);
            continue;
        }
        counts.flattened += 1;
        flattened_keys.push(
            compose_readings(&word.text, l1)
                .and_then(|readings| baseline_key(&readings)),
        );
    }
    for (word, key) in words.iter_mut().zip(flattened_keys) {
        if let Some(baseline) = key {
            word.key = baseline;
        }
    }
    counts
}

/// How many expanded words the build log shows as a sample.
const EXPANDED_EXAMPLES: usize = 12;

/// What the two-character layer merge changed, for the build log.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TwoCharMergeStats {
    /// Words both sources hold, whose reading the huayu layer contributed.
    pub(crate) overridden: u64,
    /// Words only the huayu layer holds, added to the list.
    pub(crate) added: u64,
}

/// Merges the huayu two-character layer into the jieba word list.
///
/// The two sources measure frequency on different scales -- jieba counts corpus
/// occurrences, huayu counts input-method usage with a saturating field: its
/// heaviest two thousand entries all sit between 62,000 and 64,288, hundreds of
/// them on identical values, so the head of the huayu ranking is a tie cluster
/// whose order would scramble the candidates a hold-out set actually asks for.
/// The merge therefore takes each source where it is strong:
///
/// 1. a word both sources hold keeps its jieba weight -- the corpus ranking is
///    the backbone the hold-out numbers were measured against -- and takes the
///    huayu reading, which is the pinyin the IME-scene frequency was measured
///    against and what the automatic polyphone check then judges;
/// 2. a word only huayu holds is added at the tail of the ranking: one
///    weight, the lightest jieba two-character weight carries. The corpus
///    gives no evidence to rank a segmentation gap against, and a head
///    placement is exactly what let colloquial bigrams the segmenter splits
///    (`这是`, `的人`) displace the corpus-backed candidates a hold-out set
///    asks for; entering at the tail keeps the word reachable and leaves the
///    ordering to the user's own accumulation, which is the channel the
///    learning design trusts with per-user truth anyway.
///
/// Words of any other length pass through untouched. Entries below `min_freq`
/// are not admitted at all -- the remedy when the merged build outgrows its
/// budget is a tighter huayu floor, not a bigger ceiling.
///
/// The rule is deterministic and order-free: a word's outcome is decided by
/// whether the huayu source names it and by the total orders of the two
/// rankings, never by the order either list happens to hold its rows.
pub(crate) fn merge_two_char_layer(
    jieba: &mut Vec<Word>,
    huayu: Vec<Word>,
    min_freq: u32,
) -> TwoCharMergeStats {
    let mut stats = TwoCharMergeStats::default();
    // The huayu layer the floor admits, strongest first; the text breaks ties,
    // which makes the order total whatever order the rows arrived in.
    let mut admitted: Vec<Word> = huayu
        .into_iter()
        .filter(|word| word.text.chars().count() == 2 && word.weight >= min_freq)
        .collect();
    admitted.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.text.cmp(&right.text))
    });
    // The two-character words jieba already holds: a huayu entry with a twin
    // here is a reading upgrade, and one without is an addition.
    let jieba_texts: BTreeSet<String> = jieba
        .iter()
        .filter(|word| word.text.chars().count() == 2)
        .map(|word| word.text.clone())
        .collect();

    // The weight the added words enter at: the lightest two-character weight
    // jieba carries, so they rank after everything the corpus backs.
    let tail_weight = jieba
        .iter()
        .filter(|word| word.text.chars().count() == 2)
        .map(|word| word.weight)
        .min();

    // Split the merge: a jieba word the admitted layer names stays at its
    // jieba weight and takes the huayu reading in place, everything else
    // stays put. The admitted layer is indexed once: a linear find per jieba
    // word would be a quadratic walk over a quarter-million entries.
    let admitted_by_text: BTreeMap<&str, &Word> =
        admitted.iter().map(|word| (word.text.as_str(), word)).collect();
    for word in jieba.iter_mut() {
        if let Some(&huayu_word) = admitted_by_text.get(word.text.as_str()) {
            word.key = huayu_word.key.clone();
            word.readings = huayu_word.readings.clone();
            word.polyphone = huayu_word.polyphone;
            stats.overridden += 1;
        }
    }

    // The words only huayu holds, entering at the ranking's tail.
    let mut added: Vec<Word> = admitted
        .iter()
        .filter(|word| !jieba_texts.contains(word.text.as_str()))
        .filter_map(|word| {
            let weight = tail_weight?;
            let mut added = word.clone();
            added.weight = weight;
            Some(added)
        })
        .collect();
    stats.added = added.len() as u64;

    jieba.append(&mut added);
    stats
}


/// What the compiler measured, for the build log.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// Words that made it into the container.
    pub(crate) words: u64,
    /// Words inside the expansion band.
    pub(crate) band_words: u64,
    /// Words of each character length, indices 1 to 4, the rest in index 5.
    pub(crate) shape: [u64; 5],
    /// Words with at least one character that carries several readings.
    pub(crate) polyphone_risk: u64,
    /// Words inside the band that produced more than one key.
    pub(crate) expanded_words: u64,
    /// The first few expanded words with their key counts, so the expansion can
    /// be checked by hand without a second tool.
    pub(crate) expanded_examples: Vec<(String, u32)>,
    /// Keys produced by the L3b expansion beyond each word's baseline key.
    pub(crate) expanded_keys: u64,
    /// Keys the L3c correction table added or reweighted.
    pub(crate) polyphone_keys: u64,
    /// Polyphone rows that named a word outside the compiled list.
    pub(crate) polyphone_unmatched: u64,
    /// (key, word) pairs dropped because the key was already at its word ceiling.
    pub(crate) truncated_pairs: u64,
    /// Pairs of distinct words whose hashes agree. The read path resolves them by
    /// text; the count is reported because a count far past the birthday bound is
    /// what a broken hash looks like.
    pub(crate) hash_collisions: u64,
    /// Words per normalization band, from the unigram channel's re-weighting.
    pub(crate) weight_bands: NormalizeReport,
    /// Total FST keys.
    pub(crate) keys: u64,
}

impl Stats {
    /// Keys the L3a baseline alone would have produced.
    ///
    /// The L3b expansion and the L3c corrections are the only things that add keys on
    /// top of the baseline, so subtracting them recovers it. This is the denominator
    /// the growth figure has to use: comparing keys to *words* instead measures how much
    /// words collapse onto shared keys, which is unrelated to the expansion.
    pub(crate) fn baseline_keys(&self) -> u64 {
        self.keys
            .saturating_sub(self.expanded_keys)
            .saturating_sub(self.polyphone_keys)
    }
}

/// The expansion band: which words receive L3b multi-key expansion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Band {
    /// Weight floor; a word at or above it is expanded.
    pub(crate) threshold: u32,
    /// The rank the caller asked for, for the build log.
    pub(crate) requested: u64,
}

/// The built sections and the measurements taken while building them.
pub(crate) struct Compiled {
    /// `ENTRIES` payload.
    pub(crate) entries: Vec<u8>,
    /// `STRPOOL` payload.
    pub(crate) strpool: Vec<u8>,
    /// `UNIGRAM` payload.
    pub(crate) unigram: Vec<u8>,
    /// `WORDLIST` payload.
    pub(crate) wordlist: Vec<u8>,
    /// `FST` payload.
    pub(crate) fst: Vec<u8>,
    /// `BIGRAM` payload.
    pub(crate) bigram: Vec<u8>,
    /// Measurements for the build log.
    pub(crate) stats: Stats,
}

/// Builds every section of the container.
///
/// Word ids follow hash order, which is what makes `ENTRIES` and `UNIGRAM`
/// addressable by the same index and lets the unigram table be searched by hash.
///
/// The `WORDLIST` and `ENTRIES` sections are assembled from the words' raw
/// weights -- within a key every candidate shares the word's character count, so
/// the corpus frequency is the within-key signal and must not be re-banded. The
/// `UNIGRAM` section is assembled from the banded weights [`NormalizeRules`]
/// produces: that is the channel where words of different lengths compete, and
/// the channel the long tail's corpus noise is flattened out of.
pub(crate) fn compile(
    words: &[Word],
    band: Band,
    cap: usize,
    corrections: &[(String, String)],
    normalize: NormalizeRules,
) -> Result<Compiled> {
    let mut ordered: Vec<&Word> = words.iter().collect();
    ordered.sort_by(|left, right| {
        hash_word(&left.text)
            .cmp(&hash_word(&right.text))
            .then_with(|| left.text.cmp(&right.text))
    });
    // The unigram table is searched by hash and resolved by text on the read path
    // (`Reader::unigram_lookup` compares the candidate's word through the same
    // index), so a healthy-hash collision costs a read nothing. A count far past the
    // birthday bound, though, is what a degenerate hash looks like -- and with it a
    // read path whose equal-hash runs degrade towards a linear scan. The format layer
    // owns both the count and the bound because it owns the hash.
    let texts: Vec<&str> = ordered.iter().map(|word| word.text.as_str()).collect();
    let collisions = count_colliding_hashes(&texts);
    let limit = collision_limit(texts.len());
    if collisions > limit
        && let Some((hash, left, right)) = find_colliding_hashes(&texts)
    {
        anyhow::bail!(
            "dict/unigram/collision: {collisions} colliding pairs past the {limit} the \
             hash allows; {left:?} and {right:?} share hash {hash:#010x}"
        );
    }

    let mut stats = Stats {
        words: ordered.len() as u64,
        hash_collisions: collisions,
        ..Stats::default()
    };
    let bigram = Vec::new();
    let mut corrections_by_word: BTreeMap<&str, &str> = BTreeMap::new();
    for (word, reading) in corrections {
        corrections_by_word.insert(word.as_str(), reading.as_str());
    }

    // One bucket per FST key. Iterating a `BTreeMap` yields the byte order the FST
    // builder requires, which is also why no hash map appears in this pipeline.
    let mut keys: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for (id, word) in ordered.iter().enumerate() {
        let id = id as u32;
        let characters = word.text.chars().count();
        stats.shape[characters.clamp(1, 5) - 1] += 1;
        stats
            .weight_bands
            .note(normalize.band(characters, word.weight));
        if word.polyphone {
            stats.polyphone_risk += 1;
        }
        let in_band = word.weight >= band.threshold;
        if in_band {
            stats.band_words += 1;
        }
        let mut generated = if in_band && !word.readings.is_empty() {
            expand_keys(&word.readings, cap)
        } else {
            Vec::new()
        };
        if generated.len() > 1 {
            stats.expanded_words += 1;
            stats.expanded_keys += (generated.len() - 1) as u64;
            if stats.expanded_examples.len() < EXPANDED_EXAMPLES {
                stats
                    .expanded_examples
                    .push((word.text.clone(), generated.len() as u32));
            }
        }
        if generated.is_empty() {
            generated.push(word.key.clone());
        } else if !generated.contains(&word.key) {
            // The cap may have dropped the baseline combination; the word must
            // still be reachable under its own reading.
            generated.pop();
            generated.push(word.key.clone());
        }
        for key in generated {
            keys.entry(key).or_default().push((word.weight, id));
        }
        if let Some(reading) = corrections_by_word.get(word.text.as_str()) {
            let weight = word.weight.saturating_mul(POLYPHONE_WEIGHT_FACTOR);
            let candidates = keys.entry((*reading).to_owned()).or_default();
            match candidates
                .iter_mut()
                .find(|(_, candidate)| *candidate == id)
            {
                Some(pair) => pair.0 = weight,
                None => candidates.push((weight, id)),
            }
            stats.polyphone_keys += 1;
        }
    }

    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, mut candidates) in keys {
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        if candidates.len() > MAX_WORDS_PER_KEY as usize {
            stats.truncated_pairs += (candidates.len() - MAX_WORDS_PER_KEY as usize) as u64;
            candidates.truncate(MAX_WORDS_PER_KEY as usize);
        }
        let start = (wordlist.len() / 4) as u64;
        for (_, id) in &candidates {
            wordlist.extend_from_slice(&id.to_le_bytes());
        }
        fst_builder
            .insert(
                key.as_bytes(),
                pack_fst_value(start, candidates.len() as u32)?,
            )
            .map_err(|err| anyhow::anyhow!("building the FST: {err}"))?;
        stats.keys += 1;
    }

    // The unigram table is scaled to its own heaviest entry -- the banded one, not
    // the raw one -- so the bands, not a single outlier, define the mapping's span.
    let max_weight = ordered
        .iter()
        .map(|word| normalize.normalized_weight(word.text.chars().count(), word.weight))
        .max()
        .unwrap_or(0);
    let mut entries = Vec::with_capacity(ordered.len() * ENTRY_SIZE);
    let mut strpool = Vec::new();
    let mut unigram = Vec::with_capacity(ordered.len() * UNIGRAM_ENTRY_SIZE);
    for word in &ordered {
        let offset = strpool.len() as u32;
        strpool.extend_from_slice(word.text.as_bytes());
        let syllables = word.key.matches('\'').count() + 1;
        // The count travels with the record: deriving it costs a full UTF-8 scan, and the
        // decode path asks for the same word's count once per span the word covers. A word
        // in contract is at most `MAX_WORD_LEN` bytes, so the count always fits the field.
        let characters = u8::try_from(word.text.chars().count()).unwrap_or(u8::MAX);
        let banded = normalize.normalized_weight(word.text.chars().count(), word.weight);
        entries.extend_from_slice(
            &DictEntry::new(
                offset,
                word.text.len() as u16,
                syllables.min(MAX_SYL_COUNT as usize) as u8,
                word.flags,
                word.weight,
            )
            .with_characters(characters)
            .encode(),
        );
        unigram.extend_from_slice(&hash_word(&word.text).to_le_bytes());
        unigram.extend_from_slice(&prob_q12(banded, max_weight).to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }

    Ok(Compiled {
        entries,
        strpool,
        unigram,
        wordlist,
        fst: fst_builder.into_inner()?,
        bigram,
        stats,
    })
}

/// Generates the keys of one word: the cartesian product of its characters'
/// readings, most likely first.
///
/// Combinations are ordered by the sum of the reading ranks and then
/// lexicographically, so the first entry takes every character's most frequent
/// reading and the last ones are the least likely. The product is truncated to
/// `cap` after every character, which keeps a long word from materialising
/// millions of candidates while still producing the prefix of the fully sorted
/// product: a dropped prefix can only tie with a kept one, and ties are broken
/// lexicographically in favour of the kept one.
fn expand_keys(readings: &[Vec<String>], cap: usize) -> Vec<String> {
    if cap == 0 || readings.is_empty() {
        return Vec::new();
    }
    let mut combinations: Vec<(u32, Vec<usize>)> = vec![(0, Vec::new())];
    for choices in readings {
        let mut extended = Vec::with_capacity(combinations.len() * choices.len().max(1));
        for (rank, indices) in &combinations {
            for (index, _) in choices.iter().enumerate() {
                let mut next = indices.clone();
                next.push(index);
                extended.push((rank + index as u32, next));
            }
        }
        extended.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        extended.truncate(cap);
        combinations = extended;
    }
    combinations
        .into_iter()
        .filter_map(|(_, indices)| {
            let mut syllables = Vec::with_capacity(indices.len());
            for (position, index) in indices.iter().enumerate() {
                syllables.push(readings.get(position)?.get(*index)?.clone());
            }
            Some(syllables.join("'"))
        })
        .collect()
}

/// Scales a word's weight into the Q12 unigram score.
fn prob_q12(weight: u32, max_weight: u32) -> u16 {
    if max_weight == 0 {
        return 1;
    }
    let scaled = u64::from(weight) * u64::from(PROB_Q12_MAX) / u64::from(max_weight);
    scaled.clamp(1, u64::from(PROB_Q12_MAX)) as u16
}

/// Resolves the expansion band from the L4 source's frequency ranking.
///
/// The threshold is the weight of the `expand_top`-th most frequent entry of the
/// source, which is the frequency cut ADR-0000 measured as "the top 50k words". A
/// subset compiled from the head of that ranking therefore reproduces the
/// production expansion ratio instead of expanding every word it holds.
pub(crate) fn band_threshold(
    path: &Path,
    expand_top: u64,
    override_threshold: Option<u32>,
) -> Result<Band> {
    if expand_top == 0 {
        return Ok(Band {
            threshold: u32::MAX,
            requested: 0,
        });
    }
    let threshold = match override_threshold {
        Some(weight) => weight,
        None => match fs::read_to_string(path) {
            Ok(text) => {
                let mut weights: Vec<u32> = text
                    .lines()
                    .filter_map(|line| line.rsplit('\t').next())
                    .filter_map(|column| column.trim().parse::<u32>().ok())
                    .collect();
                if weights.is_empty() {
                    println!(
                        "dictc: frequency source {} has no frequencies; using the built-in band floor {FALLBACK_BAND_THRESHOLD}",
                        path.display()
                    );
                    FALLBACK_BAND_THRESHOLD
                } else {
                    weights.sort_unstable_by(|left, right| right.cmp(left));
                    let rank = usize::try_from(expand_top).unwrap_or(usize::MAX);
                    weights[rank.saturating_sub(1).min(weights.len() - 1)]
                }
            }
            Err(_) => {
                println!(
                    "dictc: frequency source {} is unavailable; using the built-in band floor {FALLBACK_BAND_THRESHOLD}",
                    path.display()
                );
                FALLBACK_BAND_THRESHOLD
            }
        },
    };
    Ok(Band {
        threshold,
        requested: expand_top,
    })
}

#[cfg(test)]
mod tests;
