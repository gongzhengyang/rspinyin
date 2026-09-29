//! `xtask tune` -- the offline language-model weight tuner and holdout generator.
//!
//! It derives `tests/fixtures/lm_holdout.tsv` from the corpus and grid-searches the
//! scoring weights on the golden fixture, so a change to the dictionary or to the
//! weights can be measured against a set the golden fixture is not part of. A
//! build-time tool: never part of the IME runtime, and never in CI.
//!
//! The ranking walks a lattice rather than one key, because a key such as `ke'yi`
//! also offers the words under `ke` followed by the words under `yi` -- a legal
//! segmentation of the same input. Ranking only the exact key would never see
//! those, and would never exercise the length bonus or the segment penalty, the
//! two weights a tuning run is mostly about.
//!
//! The module is split by responsibility: this root holds the command line and the
//! repository root, [`io`] reads the table formats, [`lexicon`] serves the word list and
//! the model built from it, [`eval`] ranks the candidates and measures them, [`grid`]
//! searches the weights, and [`holdout`] derives the held-out set.

mod eval;
mod grid;
mod holdout;
mod io;
mod lexicon;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_core::lm::Scorer;

use crate::tune::eval::{evaluate, report};
use crate::tune::grid::grid_search;
use crate::tune::holdout::generate_holdout;
use crate::tune::io::{load_l1, load_rows};
use crate::tune::lexicon::{build_lexicon, build_lm};

/// Default word list, relative to the repository root.
pub const DEFAULT_WORDS: &str = "data/raw/base.tsv";
/// Default L1 source of single-character readings.
pub const DEFAULT_L1: &str = "data/raw/pinyin-data.tsv";
/// Default corpus the holdout set is drawn from.
pub const DEFAULT_CORPUS: &str = "data/raw/jieba-dict.tsv";
/// Default evaluation set.
pub const DEFAULT_EVAL: &str = "crates/ime-core/tests/fixtures/lm_golden.tsv";
/// Default holdout set.
pub const DEFAULT_HOLDOUT: &str = "crates/ime-core/tests/fixtures/lm_holdout.tsv";
/// Candidate-list width the reachability metric looks at: one page of nine.
pub const REPORT_CANDIDATES: usize = 9;
/// Rows a generated holdout set must reach before it is written.
pub const MIN_HOLDOUT_ROWS: usize = 5000;

// The grid walks the three weights the tuner's data can identify; the default
// comes first in each list, so a tie keeps the weights the project ships.
/// Unigram weights the grid walks.
const GRID_UNI: [i32; 3] = [256, 128, 384];
/// Length weights the grid walks.
const GRID_LEN: [i32; 4] = [90, 0, 45, 180];
/// Segment weights the grid walks.
const GRID_SEG: [i32; 4] = [128, 0, 64, 256];

/// Command-line surface of `xtask tune`.
#[derive(Debug, Args)]
pub struct TuneArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// L1 source of single-character readings.
    #[arg(long, default_value = DEFAULT_L1)]
    l1: PathBuf,
    /// Corpus the holdout set is drawn from, in `word<TAB>count` columns.
    #[arg(long, default_value = DEFAULT_CORPUS)]
    corpus: PathBuf,
    /// Evaluation set, in `pinyin<TAB>word[<TAB>weight]` columns; the grid search
    /// runs on this set. The weight column is optional and defaults to one.
    #[arg(long, default_value = DEFAULT_EVAL)]
    eval: PathBuf,
    /// Holdout set; written by `--gen-holdout` and read by nothing else here.
    #[arg(long, default_value = DEFAULT_HOLDOUT)]
    holdout: PathBuf,
    /// Derive the holdout set from the corpus, write it out, and stop.
    #[arg(long)]
    gen_holdout: bool,
    /// Rows a generated holdout set keeps, most frequent first.
    #[arg(long, default_value_t = 6000)]
    holdout_rows: usize,
    /// Grid-search the weights on the evaluation set instead of scoring them.
    #[arg(long)]
    grid: bool,
}

/// Entry point for `xtask tune`.
///
/// # Errors
///
/// Returns an error when an input file cannot be read, when a row does not carry the
/// columns its format promises, or when the corpus cannot supply [`MIN_HOLDOUT_ROWS`].
pub fn run(args: TuneArgs) -> Result<()> {
    let root = resolve_root(args.root.as_deref())?;
    if args.gen_holdout {
        return generate_holdout(&root, &args);
    }
    let l1 = load_l1(&root.join(&args.l1))?;
    let lexicon = build_lexicon(&root.join(DEFAULT_WORDS), &l1)?;
    let lm = build_lm(&lexicon);
    let rows = load_rows(&root.join(&args.eval))?;
    ensure!(
        !rows.is_empty(),
        "{}: no rows to score",
        args.eval.display()
    );
    println!("tune: {} keys, {} rows", lexicon.key_count(), rows.len());
    if args.grid {
        return grid_search(&rows, &lexicon, &lm);
    }
    let scorer = Scorer::default();
    let metrics = evaluate(&rows, &lexicon, &lm, &scorer)?;
    report(&args.eval, &scorer.weights(), &metrics);
    Ok(())
}

/// Resolves the repository root: the explicit `--root`, or the parent of the xtask crate.
///
/// The default is derived from `CARGO_MANIFEST_DIR` rather than from the working
/// directory, because every default input path is relative to the repository root while a
/// test binary runs with the working directory set to the crate: resolving against the
/// working directory would read `xtask/data/raw/...` under `cargo nextest` and the real
/// files from the command line. Tests that name an input by its raw default constant call
/// this rather than joining the constant themselves.
///
/// # Errors
///
/// Returns an error when no root is given and the xtask crate has no parent directory.
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        return Ok(root.to_path_buf());
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask must live in a subdirectory of the repository root")
}
