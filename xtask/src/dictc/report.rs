//! The `dictc` build log: what went in, what came out, and what was dropped.
//!
//! One formatter the pipeline calls once per build, plus the byte/percent/hex
//! renderers the rest of the tool shares.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use ime_dict::format::{MAX_WORDS_PER_KEY, collision_limit};
use sha2::{Digest, Sha256};

use super::build::{Band, Stats};
use super::marks;
use super::normalize;
use super::source::Skips;

/// Prints the build log: what went in, what came out, and what was dropped.
pub(crate) fn report(
    output: &Path,
    stats: &Stats,
    skips: &Skips,
    band: Band,
    cap: usize,
    normalize_rules: normalize::NormalizeRules,
    times: [Duration; 4],
) -> Result<()> {
    let size = fs::metadata(output)
        .with_context(|| format!("reading back {}", output.display()))?
        .len();
    let digest = Sha256::digest(fs::read(output)?);
    println!(
        "dictc: {} words, {} keys, {} (key growth {:+.1}%)",
        stats.words,
        stats.keys,
        human_size(size),
        growth_percent(stats.keys, stats.baseline_keys())
    );
    println!(
        "dictc: shape 1/2/3/4/5+ = {}/{}/{}/{}/{}",
        stats.shape[0], stats.shape[1], stats.shape[2], stats.shape[3], stats.shape[4]
    );
    println!(
        "dictc: L3b band weight >= {} (top {} of the frequency source): {} words in band, \
         cap {cap}, {} words expanded, {} keys generated",
        band.threshold, band.requested, stats.band_words, stats.expanded_words, stats.expanded_keys
    );
    println!(
        "dictc: L3c polyphone corrections: {} keys added or reweighted, {} rows unmatched, \
         {} (key, word) pairs dropped at the 32-word ceiling",
        stats.polyphone_keys, stats.polyphone_unmatched, stats.truncated_pairs
    );
    println!(
        "dictc: long-word marks: {} within-key pairs promoted below position {}",
        stats.long_word_promoted,
        marks::LONG_WORD_POSITION
    );
    println!(
        "dictc: weight bands (unigram channel): {} char-freq, {} core-true-freq, {} tail-flat \
         (flat {}, core floor {})",
        stats.weight_bands.char_freq,
        stats.weight_bands.core_true_freq,
        stats.weight_bands.tail_flat,
        normalize_rules.tail_flat_weight,
        normalize_rules.core_min_freq
    );
    // The trim record, when the build ran one: the target, what dropped, and
    // the floor bytes the drops provably freed. The two protected-band counts
    // are printed as the invariant witnesses they are.
    if let Some(target) = stats.weight_bands.trim.requested_target_mib {
        let trim = &stats.weight_bands.trim;
        println!(
            "dictc: trim pass: target {target:.2} MiB, {} entries dropped \
             (char-freq {}, 2char-core {}, tail-flat {}), floor freed {}",
            trim.dropped,
            trim.dropped_char_freq,
            trim.dropped_core_true_freq,
            trim.dropped_tail_flat,
            human_size(trim.freed_floor_bytes)
        );
    }
    println!(
        "dictc: {} polyphone-risk words (a character of the word has several readings)",
        stats.polyphone_risk
    );
    // The read path resolves these by text; the count is the hash-health signal the
    // format layer documents, and the build has already refused to get here if it
    // passed the bound.
    println!(
        "dictc: {} hash collisions (the bound for {} words is {})",
        stats.hash_collisions,
        stats.words,
        collision_limit(stats.words as usize)
    );
    if !stats.expanded_examples.is_empty() {
        let sample: Vec<String> = stats
            .expanded_examples
            .iter()
            .map(|(word, keys)| format!("{word}={keys}"))
            .collect();
        println!("dictc: L3b sample, word=keys: {}", sample.join(" "));
    }
    // The truncation audit: what the per-key word ceiling cost this build. The
    // total and the heaviest loss are the two numbers a cap decision reads; the
    // quoted keys are the head of the ranked report, heaviest loss first.
    if stats.truncated_keys > 0 {
        println!(
            "dictc: truncation audit: {} key(s) cut at the {}-word ceiling, worst dropped weight {}",
            stats.truncated_keys, MAX_WORDS_PER_KEY, stats.worst_dropped_weight
        );
        for (key, wanted, weight) in &stats.truncated_worst {
            println!("dictc:   {key}: wanted {wanted}, best dropped weight {weight}");
        }
    } else {
        println!(
            "dictc: truncation audit: no key reached the {}-word ceiling",
            MAX_WORDS_PER_KEY
        );
    }
    if skips.total() > 0 {
        println!(
            "dictc: skipped {} rows (non-han {}, too-long {}, bad-weight {}, bad-flags {}, \
             bad-reading {}, unknown-char {}, duplicate {})",
            skips.total(),
            skips.non_han,
            skips.too_long,
            skips.bad_weight,
            skips.bad_flags,
            skips.bad_reading,
            skips.unknown_char,
            skips.duplicate
        );
        for example in &skips.examples {
            println!("dictc:   {example}");
        }
    }
    println!(
        "dictc: {} written, sha256 {}, parse {:.0}ms build {:.0}ms write {:.0}ms total {:.0}ms",
        output.display(),
        hex(&digest),
        times[0].as_secs_f64() * 1e3,
        times[1].as_secs_f64() * 1e3,
        times[2].as_secs_f64() * 1e3,
        times[3].as_secs_f64() * 1e3
    );
    Ok(())
}

/// Key growth in percent: how many keys the expansion added over the L3a baseline.
///
/// The denominator is the baseline *key* count, not the word count. Words collapse onto
/// shared keys — 5441 words can yield fewer distinct keys than there are words — so a
/// keys-per-word ratio mostly measures that collapse and can report zero growth while
/// the expansion is in fact adding hundreds of keys. The recorded figure for the full
/// dictionary (`+23%`, 337k → 414k) is a key-to-key comparison of the same shape.
pub(crate) fn growth_percent(keys: u64, baseline_keys: u64) -> f64 {
    if baseline_keys == 0 {
        return 0.0;
    }
    (keys.saturating_sub(baseline_keys)) as f64 * 100.0 / baseline_keys as f64
}

/// Renders a byte count for the build log.
pub(crate) fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2}MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}KiB", bytes as f64 / 1024.0)
    }
}

/// Renders a digest as lower-case hexadecimal.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
