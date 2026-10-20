//! The truncation audit: what the per-key word ceiling costs, one key at a time.
//!
//! Responsibility: measure, before the compiler cuts a key's word list down to the
//! ceiling, exactly what the cut would drop -- how many words wanted in, how many
//! got in, and the weight of the best word left behind. A ceiling is only a problem
//! when the word it dropped would have been a reasonable candidate, so the report
//! ranks keys by the quality of what they lost, and the number at the top of it is
//! the number a cap decision reads.
//!
//! Boundaries: this module reads a word map and answers a report. It does not
//! truncate anything -- the compiler keeps doing that -- and it does not know where
//! the map came from. It is a pure function of its arguments, which is what makes
//! the report reproducible: the same map and the same ceiling answer the same rows
//! on every run, in the same order.

use std::collections::BTreeMap;

/// The pre-truncation word map: one key to the `(weight, word id)` pairs that want
/// in under it, in whatever order they arrived.
///
/// The compiler builds exactly this shape on its way to the word list, so the audit
/// takes it as-is rather than asking for a copy in a particular order; the audit
/// sorts each key's rows itself, the same way the writer will, so what it measures
/// is what the cut would keep.
pub type WordMap = BTreeMap<String, Vec<(u32, u32)>>;

/// One key that hit the ceiling: how many words wanted in, how many got in, and the
/// weight of the best word left behind.
///
/// A cap is only a problem when the word it dropped would have been a reasonable
/// candidate, which is why the row carries a weight and not just a count: a key
/// that lost a tail of near-zero weights has lost nothing a reader would have
/// reached, and a key that lost a head-weight word has lost a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruncatedKey {
    /// The key, as the container stores it.
    pub key: String,
    /// Words that wanted in under the key.
    pub wanted: u32,
    /// Words the ceiling let in.
    pub kept: u32,
    /// Weight of the strongest word the ceiling dropped; the writer's own ranking
    /// order decides which one that is.
    pub best_dropped_weight: u32,
}

/// Audits a word map against `cap`: every key whose list runs past the ceiling,
/// ranked by the quality of what the cut cost.
///
/// Each key's rows are sorted the way the compiler sorts them before it writes --
/// strongest weight first, ties by the lower word id, so the order is total -- and
/// the row measures that order: `kept` is what the ceiling holds, and
/// `best_dropped_weight` is the weight of the first word past it. The rows come
/// back best-dropped-weight first, ties by key, so a report reads top-down from
/// the most expensive cut.
///
/// # Panics
///
/// Never.
pub fn audit(entries: &WordMap, cap: u32) -> Vec<TruncatedKey> {
    let cap = cap as usize;
    let mut rows: Vec<TruncatedKey> = entries
        .iter()
        .filter(|(_, candidates)| candidates.len() > cap)
        .map(|(key, candidates)| {
            let mut ranked = candidates.clone();
            ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            TruncatedKey {
                key: key.clone(),
                wanted: ranked.len() as u32,
                kept: cap as u32,
                best_dropped_weight: ranked.get(cap).map_or(0, |(weight, _)| *weight),
            }
        })
        .collect();
    // The report's order is the decision's order: the key whose cut cost the most
    // sits at the top, and ties fall by key so the report never depends on the
    // map's own iteration order.
    rows.sort_by(|left, right| {
        right
            .best_dropped_weight
            .cmp(&left.best_dropped_weight)
            .then_with(|| left.key.cmp(&right.key))
    });
    rows
}

/// Tests for the audit, over maps small enough to read whole.
#[cfg(test)]
mod tests;
