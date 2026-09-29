//! The weight search: walking the grid of scoring weights over one evaluation set.
//!
//! Responsibility: score the weights the project ships, score every combination of the
//! three weights the tuner's data can identify, keep the best first-choice rate, and print
//! the two side by side with the difference between them.
//!
//! Boundaries: this layer owns the grid and nothing else. It scores through [`super::eval`]
//! and never touches a file, a word list or the lattice itself; the weights it prints are
//! a recommendation a human writes into the shipped defaults, not a change this tool
//! makes. The comparison is printed as rates rather than as counts because that is the
//! form the decision is recorded in: the set's size is an accident of the corpus, while
//! the rate is the claim.

use anyhow::{Result, bail};
use ime_core::lm::{InMemoryLm, ScoreWeights, Scorer};

use super::eval::{Metrics, evaluate};
use super::io::Row;
use super::lexicon::TsvLexicon;
use super::{GRID_LEN, GRID_SEG, GRID_UNI};

/// Grid-searches the scoring weights on an evaluation set and prints the best.
///
/// `bi` and `user` are held at their defaults: the model the tuner can build from
/// a word list has no bigram counts, so the bigram weight cannot move the ranking,
/// and an evaluation set carries no user history, so the user term is zero on
/// every row. Tuning either one here would report a preference the data does not
/// support.
///
/// The shipped weights are scored first and win every tie, because the grid lists each
/// default first and only a strictly better rate displaces it: a tuple that merely ties
/// the shipped one is not evidence for changing a constant that also fixes the decoder's
/// determinism baseline.
///
/// # Errors
///
/// Returns an error when a weight tuple is refused or when a row cannot be scored.
pub(super) fn grid_search(rows: &[Row], lexicon: &TsvLexicon, lm: &InMemoryLm) -> Result<()> {
    let defaults = ScoreWeights::default();
    let before = evaluate(rows, lexicon, lm, &Scorer::new(defaults)?)?;
    let mut best: Option<(ScoreWeights, Metrics)> = None;
    let mut tried = 0u64;
    for uni in GRID_UNI {
        for len in GRID_LEN {
            for seg in GRID_SEG {
                let weights = ScoreWeights {
                    uni,
                    len,
                    seg,
                    ..defaults
                };
                let metrics = evaluate(rows, lexicon, lm, &Scorer::new(weights)?)?;
                tried += 1;
                let better = best
                    .as_ref()
                    .is_none_or(|(_, current)| metrics.first_choice > current.first_choice);
                if better {
                    best = Some((weights, metrics));
                }
            }
        }
    }
    let Some((weights, after)) = best else {
        bail!("tune: the weight grid is empty");
    };
    println!("tune: {tried} combinations scored, the shipped weights first");
    comparison(&defaults, &before, &weights, &after);
    Ok(())
}

/// Prints what the shipped weights scored, what the best tuple scored, and the difference
/// between the two, then names the tuple to write into the shipped defaults.
fn comparison(defaults: &ScoreWeights, before: &Metrics, weights: &ScoreWeights, after: &Metrics) {
    println!("tune: weights          rows     top1     top9  weighted");
    println!(
        "tune: shipped      {:>8} {:>7.1}% {:>7.1}% {:>7.1}%",
        before.rows,
        before.first_rate(),
        before.reach_rate(),
        before.weighted_rate()
    );
    println!(
        "tune: best         {:>8} {:>7.1}% {:>7.1}% {:>7.1}%",
        after.rows,
        after.first_rate(),
        after.reach_rate(),
        after.weighted_rate()
    );
    println!(
        "tune: delta                 {:>+7.1}pp {:>+6.1}pp {:>+6.1}pp",
        after.first_rate() - before.first_rate(),
        after.reach_rate() - before.reach_rate(),
        after.weighted_rate() - before.weighted_rate()
    );
    if weights == defaults {
        println!("tune: the shipped weights are the best this grid holds");
        return;
    }
    println!("tune: best weights {weights:?}");
    println!("tune: write them into ScoreWeights::default and docs/dev/lm-weights.md");
}
