//! The `xtask testd-smoke` command-line surface: the arguments and the report print.
//!
//! Responsibility: turn the arguments into a run and turn the report into the lines an
//! operator reads. Nothing here decides a verdict -- the satisfaction check is the
//! module's, and this file only names what came out.
//!
//! Boundaries: this is the one smoke file allowed to print, and the only wall-clock
//! read of the whole module enters here: the CLI hands the run [`Clock::System`],
//! while a test hands the run a fixed reading.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;

use super::{Clock, DEFAULT_CASES, DEFAULT_DICT, DEFAULT_ENGLISH, derive, run_cases};

/// Command-line surface of `xtask testd-smoke`.
#[derive(Debug, Args)]
pub struct SmokeArgs {
    /// The case file to run; `-` reads it from stdin for ad-hoc runs.
    #[arg(long, default_value = DEFAULT_CASES, value_name = "FILE")]
    pub cases: PathBuf,
    /// The compiled dictionary the cases decode against.
    #[arg(long, default_value = DEFAULT_DICT, value_name = "FILE")]
    pub dict: PathBuf,
    /// The compiled English dictionary the `ENGLISH` rows decode against.
    ///
    /// Opened only when a row carries the switch, so a run without such rows never
    /// touches the file.
    #[arg(long, default_value = DEFAULT_ENGLISH, value_name = "FILE")]
    pub english: PathBuf,
    /// Print the first page of candidates for every failing case.
    #[arg(long, conflicts_with = "derive_from_holdout")]
    pub verbose: bool,
    /// Regenerate the core band from a holdout file instead of running.
    ///
    /// Every holdout row the engine already answers with its expected first choice
    /// becomes a `core` case, up to the module's [`super::CORE_QUOTA`]. The file named
    /// by `--cases` is rewritten: the cases that do not carry the `core` tag survive
    /// untouched (the handwritten bands), the cases that do are replaced by the
    /// derivation. A baseline that ranking work legitimately improved is refreshed by
    /// re-running this, never by editing the generated lines by hand.
    #[arg(long, value_name = "HOLDOUT_TSV")]
    pub derive_from_holdout: Option<PathBuf>,
}

/// Runs `xtask testd-smoke`.
///
/// # Errors
///
/// The derivation path and the check path each propagate their own failures: a
/// holdout or case file that cannot be read or parsed names the offending line, a
/// dictionary that cannot be mounted carries the loader's stable code, and a check with
/// at least one failing case ends in an error that quotes the count, which is what
/// turns the gate red.
///
/// # Panics
///
/// Never.
pub fn run(args: SmokeArgs) -> Result<()> {
    if let Some(holdout) = args.derive_from_holdout {
        let summary = derive(&args.cases, &holdout, &args.dict)?;
        println!(
            "smoke: derived {} core cases from {} ({} holdout rows scanned, {} non-core cases kept)",
            summary.derived,
            holdout.display(),
            summary.scanned,
            summary.kept
        );
        return Ok(());
    }
    let report = run_cases(&args.cases, &args.dict, Some(&args.english), Clock::System)?;
    for failure in &report.failures {
        println!(
            "FAIL  {}\tkeys={}\texpected={}\tgot={}",
            failure.id, failure.keys, failure.expected, failure.got
        );
        // The verbose line spells the page out, which is how a failed @top3 or
        // @contains row says where its text went; the doc on `--verbose` has promised
        // the page since the baseline existed.
        if args.verbose {
            println!("      page={}", failure.page);
        }
    }
    println!(
        "smoke: {}/{} cases passed against {}",
        report.passed,
        report.total,
        args.dict.display()
    );
    if report.failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "smoke: {} of {} cases diverge from the baseline",
            report.failures.len(),
            report.total
        )
    }
}
