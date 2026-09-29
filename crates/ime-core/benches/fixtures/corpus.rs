//! The generated corpus behind the synthetic dictionary.
//!
//! Responsibility: turn a list of keys into a dictionary of sixty thousand words
//! whose shape a decode cannot tell from a real one -- keys of one to four
//! syllables, several readings under the frequent ones, and a frequency
//! distribution that falls off the way a real one does.
//!
//! The generator draws nothing from the environment: the key enumeration is a
//! mixed-radix count over the syllable table and the one random quantity -- how
//! many readings a key carries -- comes from a generator whose algorithm is
//! written out below rather than taken from a crate, so that a dependency upgrade
//! cannot silently change the corpus a measurement is compared against.

use ime_core::segment::{SYLLABLE_COUNT, SYLLABLES};

use super::SyntheticDict;

/// Words in the generated corpus.
///
/// The design names sixty thousand, which is the order of magnitude at which a
/// decode's cost stops being dominated by the lattice and starts being dominated by
/// the dictionary lookups -- the regime the shipped dictionary runs in.
pub const CORPUS_WORDS: usize = 60_000;

/// Readings a seed key is given, so that the merge has something to rank.
const SEED_READINGS: usize = 3;

/// Readings a background key is given at most.
const BACKGROUND_READINGS: usize = 3;

/// Seed of the corpus generator.
///
/// Fixed, and part of the corpus's definition: changing it changes every weight and
/// every reading count, which is a change to what the benchmark measures.
const SEED: u64 = 0x7273_7069_6E79_696E;

/// Weight of the corpus's most frequent word, in Q16.16 log probability.
const TOP_WEIGHT: u32 = 1 << 20;

/// Weight one doubling of the rank costs, in Q16.16 log probability.
const RANK_FALLOFF: u32 = 1 << 14;

/// Largest number of rank doublings the fall-off is applied to.
///
/// Bounded so that the subtraction below cannot leave the range of a weight: the
/// corpus is nowhere near `2^24` words, and a weight that saturated at zero would
/// make the weakest words of a key indistinguishable.
const MAX_DOUBLINGS: u32 = 24;

/// First code point of the block the synthetic glyphs are drawn from.
const GLYPH_BASE: u32 = 0x4E00;

/// Number of code points the synthetic glyphs are drawn from.
///
/// U+4E00 + 0x1000 is still inside the CJK Unified Ideographs block, so every
/// offset in the range names a real ideograph and a valid `char`.
const GLYPH_SPAN: u32 = 0x1000;

/// The glyph a checked conversion falls back to.
const FALLBACK_GLYPH: char = '\u{4E00}';

/// Builds the corpus: one reading of every syllable, then `seed_keys`, then
/// background words until the dictionary holds `target` entries.
///
/// Every entry's weight is lower than the weight of the entry before it, so a key's
/// list stays ordered by descending weight as it is appended to, which is the order
/// [`Lexicon`](ime_types::Lexicon) promises its caller.
pub fn generate(seed_keys: &[String], target: usize) -> SyntheticDict {
    let mut rng = Rng::new(SEED);
    let mut dict = SyntheticDict::default();

    // A real dictionary answers for every syllable of the table, and the lattice
    // reads those single-character readings wherever no word covers a span.
    for syllable in SYLLABLES.iter().copied() {
        dict.push(syllable, 1);
    }

    for key in seed_keys {
        let syllables = syllable_count(key);
        for _ in 0..SEED_READINGS {
            dict.push(key, syllables);
        }
    }

    let mut index = 0usize;
    while dict.entries() < target {
        // Two-syllable words dominate a real dictionary; three and four make up the
        // tail, and the schedule is fixed so that the corpus is a function of its
        // own constants alone.
        let syllables = match index % 10 {
            0..=6 => 2u8,
            7..=8 => 3,
            _ => 4,
        };
        let key = key_at(usize::from(syllables), index);
        let readings = 1 + rng.below(BACKGROUND_READINGS);
        for _ in 0..readings {
            if dict.entries() >= target {
                break;
            }
            dict.push(&key, syllables);
        }
        index += 1;
    }

    dict
}

/// The `n`th key of `syllables` syllables, in a fixed enumeration of the key space.
///
/// Enumerating rather than drawing keys at random is what keeps the corpus's size
/// exact: every key is generated once, so the loop that fills the corpus reaches
/// its target in a bounded number of steps instead of retrying on collisions.
fn key_at(syllables: usize, n: usize) -> String {
    let mut rest = n;
    let mut parts = Vec::with_capacity(syllables);
    for _ in 0..syllables {
        // A remainder modulo the table's length, so the fallback is unreachable; it
        // is spelled out because `get` is the checked accessor and this file does not
        // index a slice directly.
        parts.push(SYLLABLES.get(rest % SYLLABLE_COUNT).copied().unwrap_or(""));
        rest /= SYLLABLE_COUNT;
    }
    parts.join("'")
}

/// Syllables a key spells, which is the number of its `'`-separated parts.
fn syllable_count(key: &str) -> u8 {
    // A word covers at most sixteen syllables, so the count always fits the field;
    // the conversion saturates rather than failing.
    u8::try_from(key.split('\'').count()).unwrap_or(u8::MAX)
}

/// The text one reading of `key` is spelled with.
///
/// One ideograph per syllable, offset by the reading so that two readings of one
/// key never spell the same word. The glyphs are not real words, and they do not
/// need to be: what a decode spends on a word depends on its key, its length and
/// its weight, never on which characters it draws.
pub(super) fn text_of(key: &str, reading: u32) -> String {
    key.split('\'')
        .enumerate()
        .map(|(position, syllable)| glyph(syllable, position, reading))
        .collect()
}

/// One ideograph of a synthetic word.
fn glyph(syllable: &str, position: usize, reading: u32) -> char {
    let hash = syllable.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let mixed = hash ^ (position as u32).wrapping_mul(97) ^ reading.wrapping_mul(7919);
    char::from_u32(GLYPH_BASE + mixed % GLYPH_SPAN).unwrap_or(FALLBACK_GLYPH)
}

/// The weight of the entry inserted at `rank`, in Q16.16 log probability.
///
/// A frequency list is Zipf-shaped: the most frequent word carries the top weight
/// and every doubling of the rank costs the same amount. The corpus is generated in
/// rank order, so the rank is the number of entries already in it.
pub(super) fn weight_at(rank: usize) -> u32 {
    let doublings = (usize::BITS - rank.leading_zeros()).min(MAX_DOUBLINGS);
    TOP_WEIGHT.saturating_sub(RANK_FALLOFF.saturating_mul(doublings))
}

/// A splitmix64 generator.
///
/// Twelve lines of integer arithmetic with a fixed definition, rather than a seeded
/// `rand` generator whose algorithm is free to change between releases: the corpus
/// a measurement is compared against has to be identical on every machine and at
/// every dependency version.
struct Rng(u64);

impl Rng {
    /// Seeds the generator.
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Returns the next 64 bits.
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value below `bound`.
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % bound as u64).unwrap_or(0)
    }
}
