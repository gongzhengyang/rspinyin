//! Adjacent-word co-occurrence statistics for the container's `BIGRAM` section.
//!
//! Responsibility: segment corpus lines by maximal forward match against the word
//! list being compiled, count adjacent pairs, and cut them to a deterministic,
//! size-bounded section. The compiler deliberately uses its own dictionary as its
//! segmenter: a tokenizer dependency would add a second segmentation vocabulary
//! that could disagree with the words the IME can actually rank.
//!
//! Boundaries: this module is offline build logic. It never opens files and never
//! influences runtime decoding; `DictLm` is the only consumer of the bytes it
//! produces, and it consumes them through [`ime_dict::format::Reader`].

use std::collections::{BTreeMap, BTreeSet};

use ime_dict::format::{BigramRecord, PROB_Q12_MAX, hash_word};

/// The knobs that bound and score the generated section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BigramConfig {
    /// The number of highest-scoring pairs kept.
    pub(crate) max_pairs: u32,
    /// The minimum `count * PMI` a pair needs to survive the cut.
    pub(crate) min_score: f32,
}

impl Default for BigramConfig {
    fn default() -> Self {
        Self {
            max_pairs: 200_000,
            min_score: 1.0,
        }
    }
}

/// The word vocabulary used both for segmentation and pair keys.
#[derive(Debug, Default)]
pub(crate) struct WordIndex {
    /// Complete words, indexed by their character vector for longest-match lookup.
    words: BTreeSet<Vec<char>>,
}

impl WordIndex {
    /// Indexes the compiled vocabulary.
    pub(crate) fn new<'word, Words>(words: Words) -> Self
    where
        Words: IntoIterator<Item = &'word str>,
    {
        Self {
            words: words
                .into_iter()
                .map(|word| word.chars().collect())
                .collect(),
        }
    }

    /// Segments one line by maximal forward match, returning known words in order.
    ///
    /// Unknown text is skipped rather than guessed: a pair that contains a word the
    /// engine cannot look up would be dead weight in the section. Words are emitted
    /// once per occurrence, which is what adjacency counting needs.
    fn segment(&self, line: &str) -> Vec<Vec<char>> {
        let characters: Vec<char> = line.chars().collect();
        let mut words = Vec::new();
        let mut start = 0;
        while start < characters.len() {
            let mut matched = None;
            for end in (start + 1..=characters.len()).rev() {
                let candidate = characters[start..end].to_vec();
                if self.words.contains(&candidate) {
                    matched = Some(candidate.len());
                    break;
                }
            }
            match matched {
                Some(length) => {
                    words.push(characters[start..start + length].to_vec());
                    start += length;
                }
                None => start += 1,
            }
        }
        words
    }
}

/// Counts adjacent word occurrences across newline-separated corpus lines.
///
/// The map is keyed by the same 64-bit pair key the container stores, and iteration
/// is ordered by that key. Marginal word frequencies are derived from the pair table
/// during scoring, so the public build pipeline remains the contract's single map.
pub(crate) fn count_cooccurrence(corpus: &str, words: &WordIndex) -> BTreeMap<u64, u32> {
    let mut counts = BTreeMap::new();
    for line in corpus.lines() {
        let segmented = words.segment(line);
        for pair in segmented.windows(2) {
            let previous = String::from_iter(&pair[0]);
            let next = String::from_iter(&pair[1]);
            let key = u64::from(hash_word(&previous)) << 32 | u64::from(hash_word(&next));
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
}

/// Scores, cuts and quantizes pair counts into a sorted section payload.
///
/// Scoring is `count * PMI`, with PMI represented by `count / independence` so the
/// ranking is monotonic without calling `log`: every candidate is multiplied by the
/// same raw count. Scores are then ordered by score and pair key, which gives the
/// cut deterministic handling even when floating-point scores tie. The output is
/// finally sorted by pair key, the order the reader binary-searches.
pub(crate) fn build_records(counts: BTreeMap<u64, u32>, config: BigramConfig) -> Vec<BigramRecord> {
    if counts.is_empty() || config.max_pairs == 0 {
        return Vec::new();
    }
    let marginals = Marginals::new(&counts);
    let total = counts
        .values()
        .copied()
        .fold(0u64, |total, count| total.saturating_add(u64::from(count)));
    let mut ranked: Vec<(u64, u32, f32)> = counts
        .into_iter()
        .filter_map(|(pair_key, count)| {
            let count = u64::from(count);
            let previous = u64::from(marginals.left(pair_key));
            let next = u64::from(marginals.right(pair_key));
            if previous == 0 || next == 0 {
                return None;
            }
            let independence = (previous.saturating_mul(next)) as f32 / total as f32;
            if independence == 0.0 {
                return None;
            }
            let score = count as f32 * (count as f32 / independence);
            (score >= config.min_score).then_some((pair_key, count as u32, score))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked.truncate(config.max_pairs as usize);

    let selected: BTreeMap<u64, u32> = ranked
        .into_iter()
        .map(|(pair_key, count, _)| (pair_key, count))
        .collect();
    selected
        .into_iter()
        .map(|(pair_key, count)| {
            let next = u64::from(marginals.right(pair_key));
            let probability = ((u64::from(count) * u64::from(PROB_Q12_MAX)) / next.max(1)) as u16;
            BigramRecord::new(pair_key, probability.clamp(1, PROB_Q12_MAX))
        })
        .collect()
}

/// The left and right marginal counts implied by the pair table.
#[derive(Debug)]
struct Marginals {
    previous: BTreeMap<u64, u32>,
    next: BTreeMap<u64, u32>,
}

impl Marginals {
    fn new(counts: &BTreeMap<u64, u32>) -> Self {
        let mut marginals = Self {
            previous: BTreeMap::new(),
            next: BTreeMap::new(),
        };
        for (&pair_key, &count) in counts {
            *marginals.previous.entry(pair_key >> 32).or_insert(0) += count;
            *marginals.next.entry(pair_key & 0xffff_ffff).or_insert(0) += count;
        }
        marginals
    }

    fn left(&self, pair_key: u64) -> u32 {
        self.previous.get(&(pair_key >> 32)).copied().unwrap_or(0)
    }

    fn right(&self, pair_key: u64) -> u32 {
        self.next
            .get(&(pair_key & 0xffff_ffff))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_cooccurrence_segments_known_adjacent_words_and_skips_unknown_text() {
        let index = WordIndex::new(["中国", "人民"]);
        let counts = count_cooccurrence("prefix中国人民中国人民suffix", &index);
        assert_eq!(counts.values().copied().collect::<Vec<u32>>(), vec![2, 1]);
    }

    #[test]
    fn test_build_records_cuts_quantizes_and_sorts_deterministically() {
        let index = WordIndex::new(["中国", "人民"]);
        let counts = count_cooccurrence("中国人民中国", &index);
        let records = build_records(counts, BigramConfig::default());
        assert_eq!(records.len(), 2);
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].pair_key < pair[1].pair_key)
        );
        assert!(records.iter().all(|record| record.prob_q12 == PROB_Q12_MAX));

        let limited = build_records(
            count_cooccurrence("中国人民中国", &index),
            BigramConfig {
                max_pairs: 1,
                min_score: 0.0,
            },
        );
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn test_build_records_honours_the_score_floor_and_empty_input() {
        let index = WordIndex::new(["中国", "人民"]);
        let counts = count_cooccurrence("中国人民", &index);
        assert!(
            build_records(
                counts,
                BigramConfig {
                    max_pairs: 8,
                    min_score: f32::INFINITY,
                },
            )
            .is_empty()
        );
        assert!(build_records(BTreeMap::new(), BigramConfig::default()).is_empty());
    }
}
