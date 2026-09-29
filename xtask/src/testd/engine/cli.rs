//! The command-line surface of `xtask testd engine`.
//!
//! This is the one file of the harness that touches the filesystem: it reads the
//! scenario files a caller names and writes the built-in set out. The drive path itself
//! opens no file, which is what lets the unit tests assert that property over its source.
//!
//! Nothing here reads a clock either. A scenario is replayed as fast as the engine
//! answers, and the only number the output carries is a count of runs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;

use crate::testd::engine::REPEAT_RUNS;
use crate::testd::engine::assert_repeatable;
use crate::testd::engine::run_scenario;
use crate::testd::engine::scenarios;
use crate::testd::engine::scenario::{EngineFixture, parse_scenario};

/// Command-line surface of `xtask testd engine`.
#[derive(Debug, Args)]
pub struct EngineArgs {
    /// Directory of TOML scenario files; every `*.toml` in it is loaded.
    ///
    /// Without it the built-in scenario set runs, which is what makes the channel usable
    /// with no fixture files on disk at all.
    #[arg(long, value_name = "DIR")]
    pub scenarios: Option<PathBuf>,
    /// Replay every scenario this many times and compare the candidate sequences.
    ///
    /// One is the scenario check without a comparison; the default is the repeat count
    /// the determinism contract is stated at.
    #[arg(long, default_value_t = REPEAT_RUNS)]
    pub repeat: usize,
    /// Write the built-in scenario set to stdout as TOML and exit.
    ///
    /// The built-in set is what a `tests/fixtures/scenarios/` directory should hold, so
    /// this is how the fixture files are materialised without anyone transcribing them.
    #[arg(long)]
    pub dump: bool,
}

/// Entry point for `xtask testd engine`.
///
/// # Returns
///
/// `Ok(())` when every scenario ran, every step observed what it expected and every
/// replay produced the same candidate sequence.
///
/// # Errors
///
/// Returns an error when a scenario file cannot be read or parsed, when the built-in set
/// describes a dictionary the doubles cannot build, and when any scenario diverged --
/// the divergences are printed as they are found and the run ends with a non-zero status.
///
/// # Panics
///
/// Never panics.
pub fn run(args: EngineArgs) -> Result<()> {
    if args.dump {
        return dump_builtin();
    }
    let fixtures = load(args.scenarios.as_deref())?;
    let mut failed = 0usize;
    for fixture in &fixtures {
        // The single run comes first, so that a scenario which diverges is reported as a
        // scenario failure rather than as a replay that drifted: the two are different
        // defects and a reader has to be able to tell them apart.
        let outcome = run_scenario(
            &fixture.scenario,
            &fixture.lexicon,
            &fixture.user_freq,
            &fixture.lm,
        )
        .and_then(|()| {
            assert_repeatable(&fixture.scenario, &fixture.sources(), args.repeat.max(1))
        });
        match outcome {
            Ok(()) => println!(
                "{}: ok ({} steps x {} runs)",
                fixture.scenario.name,
                fixture.scenario.steps.len(),
                args.repeat.max(1)
            ),
            Err(divergence) => {
                failed += 1;
                eprintln!(
                    "{}: DIVERGED\n{}",
                    fixture.scenario.name,
                    divergence.describe()
                );
            }
        }
    }
    println!(
        "testd engine: {} scenarios, {failed} diverged",
        fixtures.len()
    );
    if failed > 0 {
        bail!("{failed} scenario(s) diverged");
    }
    Ok(())
}

/// Loads the fixtures the arguments name.
///
/// # Errors
///
/// As [`load_directory`] and [`load_builtin`].
///
/// # Panics
///
/// Never panics.
fn load(directory: Option<&Path>) -> Result<Vec<EngineFixture>> {
    match directory {
        Some(directory) => load_directory(directory),
        None => load_builtin(),
    }
}

/// Builds the fixtures of the built-in scenario set.
///
/// # Errors
///
/// Returns an error when a built-in scenario describes a dictionary the doubles cannot
/// build, which is a defect in this file rather than a scenario outcome.
///
/// # Panics
///
/// Never panics.
fn load_builtin() -> Result<Vec<EngineFixture>> {
    scenarios()
        .into_iter()
        .map(|scenario| {
            let name = scenario.name.clone();
            EngineFixture::new(scenario).with_context(|| format!("building {name}"))
        })
        .collect()
}

/// Loads every `*.toml` scenario file a directory holds, in name order.
///
/// # Errors
///
/// Returns an error when the directory cannot be read, when it holds no scenario file,
/// when a file cannot be read or parsed, or when a file describes a dictionary the
/// doubles cannot build.
///
/// # Panics
///
/// Never panics.
fn load_directory(directory: &Path) -> Result<Vec<EngineFixture>> {
    let shown = directory.display();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("reading {shown}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<PathBuf>>>()
        .with_context(|| format!("reading {shown}"))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    // Sorted so that a directory runs in the same order on every machine, whatever the
    // filesystem hands back.
    paths.sort();
    ensure!(!paths.is_empty(), "{shown}: no *.toml scenario files");
    paths.into_iter().map(|path| fixture_at(&path)).collect()
}

/// Builds the fixture one scenario file describes.
///
/// # Errors
///
/// Returns an error when the file cannot be read or parsed, or when its dictionary
/// cannot be built.
///
/// # Panics
///
/// Never panics.
fn fixture_at(path: &Path) -> Result<EngineFixture> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {shown}"))?;
    let scenario = parse_scenario(&text).with_context(|| format!("parsing {shown}"))?;
    EngineFixture::new(scenario).with_context(|| format!("building {shown}"))
}

/// Writes the built-in scenario set to stdout as TOML.
///
/// # Errors
///
/// Returns an error when a scenario cannot be rendered.
///
/// # Panics
///
/// Never panics.
fn dump_builtin() -> Result<()> {
    for scenario in scenarios() {
        let text = toml::to_string_pretty(&scenario)
            .with_context(|| format!("rendering {}", scenario.name))?;
        println!("# --- scenarios/{}.toml ---\n{text}", scenario.name);
    }
    Ok(())
}
