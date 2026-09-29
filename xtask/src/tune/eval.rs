//! The ranking and measurement layer of the tuner: lattice, candidates and rates.
//!
//! Responsibility: build the lattice of one key, score its edges, rank the words the key
//! can start, and turn an evaluation set into the rates two weight tuples are compared by.
//!
//! Boundaries: this layer owns no file and no word list; it reads rows from
//! [`super::io`] and words from [`super::lexicon`], and it writes nothing. Every
//! comparison ends in a tie-break on the text, so the same rows and the same weights
//! always produce the same numbers.

use std::path::Path;

use anyhow::Result;
use ime_core::lm::{InMemoryLm, ScoreWeights, Scorer};
use ime_types::{Lexicon, WordRef};

use super::REPORT_CANDIDATES;
use super::io::Row;
use super::lexicon::{NoUserFreq, TsvLexicon};

/// Longest key the ranking walk accepts, in syllables.
const MAX_KEY_SYLLABLES: usize = 16;

/// One lattice edge: a word covering a span of the input.
#[derive(Clone, Copy)]
struct Edge<'a> {
    /// Node the edge ends at.
    to: usize,
    /// The word the edge carries.
    word: WordRef<'a>,
}

/// Builds the lattice of `syllables`: for every span, the words stored under that
/// span's key.
fn build_edges<'a>(syllables: &[&str], lexicon: &'a TsvLexicon) -> Result<Vec<Vec<Edge<'a>>>> {
    let mut edges: Vec<Vec<Edge<'a>>> = vec![Vec::new(); syllables.len() + 1];
    let mut key = String::new();
    // `take(syllables.len())` drops the final slot: `edges` is one longer than
    // `syllables` so that the `to == syllables.len()` end position has somewhere to
    // land, and that slot is written by the spans that end there, not by a span
    // starting at it.
    for (from, slot) in edges.iter_mut().enumerate().take(syllables.len()) {
        for to in (from + 1)..=syllables.len() {
            if to > from + 1 {
                key.push('\'');
            }
            key.push_str(syllables[to - 1]);
            for word in lexicon.lookup(&key)? {
                slot.push(Edge { to, word });
            }
        }
        key.clear();
    }
    Ok(edges)
}

/// Scores one edge, including its share of the segment penalty. The tuner's model
/// has no bigram counts, so there is no predecessor to pass.
///
/// `Scorer::path_penalty` is linear in the segment count and every edge is one
/// segment, so charging one segment per edge adds up to the whole path exactly.
fn edge_score(scorer: &Scorer, lm: &InMemoryLm, word: WordRef<'_>) -> i32 {
    let characters = u16::try_from(word.text.chars().count()).unwrap_or(u16::MAX);
    let base = scorer.edge_score(lm, &NoUserFreq, None, word.text, characters);
    base.saturating_add(scorer.path_penalty(1))
}

/// Sweeps backwards so `best[node]` is the best score of any path to the end.
fn best_to_end(edges: &[Vec<Edge<'_>>], lm: &InMemoryLm, scorer: &Scorer) -> Vec<Option<i32>> {
    let end = edges.len().saturating_sub(1);
    let mut best: Vec<Option<i32>> = vec![None; end + 1];
    best[end] = Some(0);
    for node in (0..end).rev() {
        for edge in &edges[node] {
            if let Some(rest) = best[edge.to] {
                let score = edge_score(scorer, lm, edge.word).saturating_add(rest);
                if best[node].is_none_or(|current| score > current) {
                    best[node] = Some(score);
                }
            }
        }
    }
    best
}

/// One first word of a key, with the score of the best path that starts with it.
pub(super) struct Candidate {
    /// The word itself.
    pub(super) word: String,
    /// Score of the best path that starts with this word.
    pub(super) score: i32,
}

/// Ranks the candidate first words of `key`, best first.
///
/// # Errors
///
/// Returns an error when the lexicon refuses a lookup.
pub(super) fn rank_first_words(
    key: &str,
    lexicon: &TsvLexicon,
    lm: &InMemoryLm,
    scorer: &Scorer,
) -> Result<Vec<Candidate>> {
    let syllables: Vec<&str> = key.split('\'').filter(|piece| !piece.is_empty()).collect();
    if syllables.is_empty() || syllables.len() > MAX_KEY_SYLLABLES {
        return Ok(Vec::new());
    }
    let edges = build_edges(&syllables, lexicon)?;
    let rest = best_to_end(&edges, lm, scorer);
    let mut candidates: Vec<Candidate> = Vec::new();
    for edge in &edges[0] {
        if let Some(tail) = rest[edge.to] {
            let score = edge_score(scorer, lm, edge.word).saturating_add(tail);
            candidates.push(Candidate {
                word: edge.word.text.to_owned(),
                score,
            });
        }
    }
    // One word can start more than one path; the best-scoring one wins, and ties
    // break on the text so the ranking is reproducible.
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.word.cmp(&right.word))
    });
    candidates.dedup_by(|left, right| left.word == right.word);
    Ok(candidates)
}

/// What one scoring pass over an evaluation set measured. The rates are recorded
/// separately on purpose: the first-choice rate measures how good the ordering is,
/// and the reachability rate measures whether the expected word can be produced at
/// all, which is a different failure.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Metrics {
    /// Rows scored.
    pub(super) rows: u64,
    /// Rows whose expected word was ranked first.
    pub(super) first_choice: u64,
    /// Rows whose expected word is reachable within [`REPORT_CANDIDATES`].
    pub(super) in_top_nine: u64,
    /// Rows no candidate could be produced for.
    pub(super) empty: u64,
    /// Sum of the row weights.
    pub(super) weight_total: u64,
    /// Sum of the weights of the rows whose expected word was ranked first.
    pub(super) weight_hit: u64,
}

/// Scores one evaluation set.
///
/// # Errors
///
/// Returns an error when a lookup fails.
pub(super) fn evaluate(
    rows: &[Row],
    lex: &TsvLexicon,
    lm: &InMemoryLm,
    scorer: &Scorer,
) -> Result<Metrics> {
    let mut metrics = Metrics::default();
    for row in rows {
        metrics.rows += 1;
        metrics.weight_total += u64::from(row.weight);
        let candidates = rank_first_words(&row.key, lex, lm, scorer)?;
        let Some(first) = candidates.first() else {
            metrics.empty += 1;
            continue;
        };
        if first.word == row.word {
            metrics.first_choice += 1;
            metrics.weight_hit += u64::from(row.weight);
        }
        let reachable = candidates
            .iter()
            .take(REPORT_CANDIDATES)
            .any(|candidate| candidate.word == row.word);
        metrics.in_top_nine += u64::from(reachable);
    }
    Ok(metrics)
}

/// A count as a percentage of a total; zero of zero is zero.
pub(super) fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    part as f64 * 100.0 / total as f64
}

/// Prints the two rates and the weights that produced them.
pub(super) fn report(path: &Path, weights: &ScoreWeights, metrics: &Metrics) {
    let rows = metrics.rows;
    println!(
        "tune: {} {rows} rows, {} empty",
        path.display(),
        metrics.empty
    );
    let first = percent(metrics.first_choice, rows);
    println!("tune: first {first:.1}% ({}/{rows})", metrics.first_choice);
    let weighted = percent(metrics.weight_hit, metrics.weight_total);
    println!("tune: weighted {weighted:.1}%");
    let reachable = percent(metrics.in_top_nine, rows);
    println!(
        "tune: reachable {reachable:.1}% ({}/{rows})",
        metrics.in_top_nine
    );
    println!("tune: weights {weights:?}");
}
