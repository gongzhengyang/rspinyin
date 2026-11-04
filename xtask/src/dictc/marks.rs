//! The build-time ranking marks: the ranking disciplines the benchmark
//! transcribed from rime-ice's runtime filters, compiled into the weights.
//!
//! Responsibility: the two mark rules the mixed-English and long-word filters
//! of the benchmark express at runtime, stated as weight arithmetic the
//! compilers apply before a container is written. The English collision rule
//! scales the rank of an entry whose spelling a pinyin typist leaves
//! mid-syllable, and the long-word gradient lifts the within-key candidates
//! longer than their key's head word to the promotion position. The running
//! engine never re-applies either: ranking is compiled into the weights, and
//! the container format gains no mark channel.
//!
//! Boundaries: this module owns the judgment rules and the factor constants,
//! and nothing else. It reads no word list (the callers hand their rows and
//! key maps over), writes no container (that is `ime_dict::format::writer`),
//! and never rewrites a word list: the English reduction moves the rank an
//! entry carries into the unigram scaling, the gradient moves the per-key
//! candidate order the FST stores, and both are pure functions of their
//! inputs, so recompiling the same sources reproduces the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use ime_core::segment::lookup;

/// The default English reduce list, relative to the repository root.
pub(crate) const DEFAULT_REDUCE_LIST: &str = "data/raw/reduce-eng.tsv";

/// The denominator of the English collision factor: the rank an entry on the
/// reduce list carries is divided by this.
///
/// rime-ice demotes a colliding English candidate to a fixed display position;
/// this workspace's English container has no per-key order to demote in -- one
/// word per key -- so the reduction lands on the only rank channel an entry
/// has, the weight the unigram scaling reads. A quarter keeps the entry
/// visible (the scaling clamps every rank above zero) while sinking it below
/// the entries a pinyin typist is plausibly asking for.
pub(crate) const REDUCE_DENOMINATOR: u32 = 4;

/// The candidate position the long-word gradient promotes to, one-based.
///
/// The benchmark's long-word filter inserts the words longer than the list's
/// head word at this position, leaving the head of the list to the short
/// candidates the two-character hold-out measures. The container expresses
/// positions through weights: a promoted candidate is capped just below the
/// weight of the candidate at the position before this one, which reproduces
/// insert-at-position for a key of any size.
pub(crate) const LONG_WORD_POSITION: usize = 4;

/// The weight factors of the long-word gradient, one per extra character over
/// the key's head word, saturating at the last entry.
///
/// The gradient is multiplicative because the within-key weight is the only
/// channel the format gives a mark: a longer candidate starts lower (corpus
/// frequency falls with length) and needs a steeper factor to reach the same
/// band. Doubling per character keeps a promoted word below the heavy
/// two-character heads the hold-out gate protects while giving the long
/// candidates a length-ordered ladder among themselves.
const LONG_WORD_FACTORS: [u32; 3] = [2, 4, 8];

/// The pinyin initials a spelling can be left hanging on.
///
/// The twenty single letters a syllable can start with plus the three
/// digraphs `zh`, `ch` and `sh`. `y` and `w` stand in the table although
/// strict phonology calls them zero-initial markers: a spelling left hanging
/// on them is as unfinished a syllable as one left hanging on `b`, which is
/// the situation the collision rule looks for. The list is sorted, so the
/// tail check can binary-search it.
const INITIALS: [&str; 23] = [
    "b", "c", "ch", "d", "f", "g", "h", "j", "k", "l", "m", "n", "p", "q", "r", "s", "sh", "t",
    "w", "x", "y", "z", "zh",
];

/// Returns the byte offset the spelling cuts at, when it collides with pinyin.
///
/// The rule the reduce list is built on: a spelling collides when it cuts into
/// a complete syllable of the table followed by exactly an initial -- the
/// shape a pinyin typist leaves mid-syllable. `ranch` cuts `ran|ch`, `bail`
/// cuts `bai|l`, `push` cuts `pu|sh`. The head must be at least two letters:
/// the single-vowel syllables `a`/`o`/`e` would drag the one- and two-letter
/// English words in, which the benchmark's own list also skips. A
/// complete-pinyin spelling (`panda` cuts `pan|da`, but `da` is a syllable)
/// collides by no cut here: the input it shares with Chinese is fully typed,
/// not half-typed, and the reduce list does not own that case.
///
/// The scan takes the first valid cut, so the answer is a function of the
/// spelling alone. The input is lowercased first and must be ASCII for the
/// slicing to be byte-addressed; anything else answers `None`, because a
/// spelling the English container cannot key cannot collide either.
pub(crate) fn collision_cut(word: &str) -> Option<usize> {
    let word = word.to_ascii_lowercase();
    if !word.is_ascii() {
        return None;
    }
    for cut in 2..word.len() {
        let (head, tail) = (&word[..cut], &word[cut..]);
        if lookup(head).is_some() && is_initial(tail) {
            return Some(cut);
        }
    }
    None
}

/// Whether `spelling` is one of the initials a syllable can be left hanging on.
fn is_initial(spelling: &str) -> bool {
    INITIALS.binary_search(&spelling).is_ok()
}

/// Loads the English reduce list: one lowercase word per row, comments allowed.
///
/// The list is hand-curated (the machine judgment proposes, a human confirms),
/// so a malformed row fails the build instead of being skipped: a silently
/// ignored row is a demotion that never happens. The first column is the word;
/// a second column may document the judgment cut and is not read.
///
/// # Errors
///
/// Returns an error when the file cannot be read, when a row is not a single
/// ASCII-alphabetic word, or when a row repeats a word the list already holds.
pub(crate) fn load_reduce_list(path: &Path) -> Result<BTreeSet<String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut words = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let word = line.split('\t').next().unwrap_or("").trim();
        ensure!(
            !word.is_empty() && word.chars().all(|ch| ch.is_ascii_alphabetic()),
            "{}:{}: `{word}` is not one ASCII word",
            path.display(),
            number + 1
        );
        let word = word.to_lowercase();
        ensure!(
            words.insert(word.clone()),
            "{}:{}: `{word}` is listed twice",
            path.display(),
            number + 1
        );
    }
    Ok(words)
}

/// Applies the long-word gradient to one compiled key map, in place.
///
/// Every candidate longer than its key's head word -- the head being the
/// heaviest candidate before the mark runs -- has its within-key weight
/// multiplied by its length bucket's factor and capped just below the weight
/// of the candidate at the position before [`LONG_WORD_POSITION`]. The cap is
/// what turns a factor into an insertion: a promoted candidate ranks after
/// that anchor whatever its corpus frequency says, the way the benchmark's
/// filter inserts at the position rather than rescoring, and a key too short
/// to have that anchor degrades to appending, which is the same insertion at
/// the end. A long word already ahead of the anchor is pulled back behind it,
/// which is the relocation the filter performs.
///
/// The head candidate and every candidate no longer than it keep their weight:
/// the two-character core a hold-out set asks for is never displaced. The raw
/// weights the entries and the unigram table store are untouched too -- the
/// gradient is a display-order mark over the per-key candidate lists, not a
/// language-model change.
///
/// Deterministic and order-free: baseline, anchor and factors read the
/// pre-mark weights only, so the mark neither compounds under a recompile nor
/// depends on the order the pairs sit in.
pub(crate) fn apply_long_word_gradient(
    keys: &mut BTreeMap<String, Vec<(u32, u32)>>,
    char_counts: &[usize],
) -> u64 {
    let mut promoted = 0u64;
    for candidates in keys.values_mut() {
        // The pre-mark ranking, heaviest first -- the comparator the container's
        // own sort uses, so the anchor is the candidate the promotion position
        // names. Cloned rather than sorted in place: the mark reads this order
        // and writes the weights, and the two must not feed each other.
        let mut ranked = candidates.clone();
        ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let Some(&(_, head_id)) = ranked.first() else {
            continue;
        };
        let baseline = char_counts.get(head_id as usize).copied().unwrap_or(0);
        let anchor_index = (LONG_WORD_POSITION - 2).min(ranked.len() - 1);
        let cap = ranked[anchor_index].0.saturating_sub(1);
        for pair in candidates.iter_mut() {
            let Some(count) = char_counts.get(pair.1 as usize).copied() else {
                continue;
            };
            if count <= baseline {
                continue;
            }
            let bucket = (count - baseline - 1).min(LONG_WORD_FACTORS.len() - 1);
            let boosted = pair.0.saturating_mul(LONG_WORD_FACTORS[bucket]).min(cap);
            if boosted != pair.0 {
                pair.0 = boosted;
                promoted += 1;
            }
        }
    }
    promoted
}

#[cfg(test)]
mod tests;
