//! The measurement's data shapes: a held-out row, the report it produces, the tallies.
//!
//! Split out of `quality.rs` to keep that file inside the line limit. Everything here is
//! data and the arithmetic that folds one into the next; nothing here reads a clock, a
//! hash seed or the filesystem, which is what keeps a rate measured twice comparable.

use super::*;

/// One row of the held-out set, resolved to the key the container is queried with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldoutCase {
    /// The `'`-separated key the row's pinyin column segments into.
    pub key: String,
    /// The word that has to rank first for the row to count as a first-choice hit.
    pub expected: String,
    /// The layer the row is attributed to, for the per-layer breakdown.
    pub source: ReadingSource,
}

/// Quality of one decode over the whole hold-out set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct QualityReport {
    /// Fraction of cases whose first candidate is the expected word.
    pub top1_rate: f64,
    /// Fraction of cases where the expected word appears in the first nine
    /// candidates. This is the reachability half: L3b fixes this one.
    pub top9_rate: f64,
    /// Cases the decoder could not spell at all, reported separately so a
    /// missing word is never confused with a badly ranked one.
    pub unreachable: u32,
    /// Fraction of the cases the key serves whose first candidate is the expected word.
    ///
    /// [`QualityReport::top1_rate`] divides by every case, so a word the dictionary does
    /// not hold counts against it exactly like a word it ranks second. This rate divides
    /// by the cases the key serves, which is what isolates the ordering from the coverage:
    /// a dictionary that gains a word moves this number while the other stays where it is,
    /// and a dictionary that reorders its candidates moves both.
    pub served_top1_rate: f64,
    /// Top-1 accuracy by the expected word's length, matching the normalization bands.
    pub length_bands: LengthBandAccuracy,
}

/// Top-1 accuracy attributed by word length.
///
/// The bands mirror the normalization bands so a regression can name its layer instead of
/// surfacing as an anonymous drop in the total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LengthBandAccuracy {
    /// Single-character cases.
    pub single: Top1Band,
    /// Two-character cases: the normalization pipeline's true-frequency core.
    pub two: Top1Band,
    /// Three-or-more-character cases: the flattened tail.
    pub long: Top1Band,
}

/// One word-length band's hit count and sample size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Top1Band {
    /// Cases in the band.
    pub cases: u64,
    /// Cases whose expected word came first.
    pub first: u64,
}

impl Top1Band {
    /// The band's top-1 rate; `None` when it is empty, so a report never presents a
    /// misleading 0% for a layer the hold-out set does not exercise.
    pub fn top1(self) -> Option<f64> {
        (self.cases > 0).then(|| self.first as f64 / self.cases as f64)
    }

    /// Records one case and whether it hit.
    pub(crate) fn note(&mut self, first: bool) {
        self.cases += 1;
        self.first += u64::from(first);
    }
}

/// What one pass counted, before the rates were divided out.
///
/// The counts are kept alongside the rates rather than only as rates so that a per-layer
/// line can state how many cases it stands on. A rate over a handful of cases and a rate
/// over five thousand print the same way; the count is what tells them apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tally {
    /// Cases the pass looked at.
    pub(super) cases: u64,
    /// Cases whose expected word came first.
    pub(super) first: u64,
    /// Cases whose expected word was inside the reported page.
    pub(super) within: u64,
    /// Cases whose expected word the key does not serve at all.
    pub(super) missing: u64,
    /// Cases grouped by the expected word's length.
    pub(super) length_bands: LengthBandAccuracy,
}

impl Tally {
    /// The rates the counts imply; an empty pass reports zero rather than dividing by
    /// zero.
    pub(super) fn report(self) -> QualityReport {
        if self.cases == 0 {
            return QualityReport::default();
        }
        let total = self.cases as f64;
        QualityReport {
            top1_rate: self.first as f64 / total,
            top9_rate: self.within as f64 / total,
            unreachable: u32::try_from(self.missing).unwrap_or(u32::MAX),
            served_top1_rate: self.served_first(),
            length_bands: self.length_bands,
        }
    }

    /// Cases the key serves at all, which is the subset an ordering can be read from.
    pub(super) fn served(self) -> u64 {
        self.cases.saturating_sub(self.missing)
    }

    /// Fraction of the cases the key serves whose expected word came first; a pass that
    /// served nothing answers zero rather than dividing by zero.
    pub(super) fn served_first(self) -> f64 {
        let served = self.served();
        if served == 0 {
            return 0.0;
        }
        self.first as f64 / served as f64
    }
}
