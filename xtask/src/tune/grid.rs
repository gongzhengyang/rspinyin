//! The weight search: walking the grid of scoring weights over one evaluation set.
//!
//! Responsibility: score every combination of the three weights the tuner's data can
//! identify, keep the best first-choice rate, and report it together with the weights that
//! produced it.
//!
//! Boundaries: this layer owns the grid and nothing else. It scores through [`super::eval`]
//! and never touches a file, a word list or the lattice itself; the weights it prints are
//! a recommendation a human writes into the shipped defaults, not a change this tool
//! makes.

use std::path::Path;

use anyhow::{Result, bail};
use ime_core::lm::{InMemoryLm, ScoreWeights, Scorer};

use super::eval::{Metrics, evaluate, report};
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
/// # Errors
///
/// Returns an error when a weight tuple is refused or when a row cannot be scored.
pub(super) fn grid_search(rows: &[Row], lexicon: &TsvLexicon, lm: &InMemoryLm) -> Result<()> {
    let defaults = ScoreWeights::default();
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
                // Strictly better only, and the grid starts at the defaults, so a
                // tie keeps the shipped weights rather than an arbitrary one of
                // the combinations that share the top score.
                let better = best
                    .as_ref()
                    .is_none_or(|(_, current)| metrics.first_choice > current.first_choice);
                if better {
                    best = Some((weights, metrics));
                }
            }
        }
    }
    let Some((weights, metrics)) = best else {
        bail!("tune: the weight grid is empty");
    };
    println!("tune: {tried} combinations scored, defaults first");
    report(Path::new("(grid)"), &weights, &metrics);
    if weights != defaults {
        println!("tune: write the winning weights into ScoreWeights::default and");
        println!("tune: docs/dev/lm-weights.md");
    }
    Ok(())
}
