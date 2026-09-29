//! Build-time tool host for rspinyin.
//!
//! Subcommands live here rather than in shell scripts so they can share the
//! workspace's dependency versions and error handling. Currently: dictionary
//! compilation, plugin installation, budget validation, and the automated test
//! platform's input-injection channel.

use std::path::PathBuf;

use clap::Parser;

mod budget;
mod dictc;
mod install;
mod package;
mod report;
mod testd;
mod tune;
mod versions;

/// Command-line entry point.
#[derive(Debug, Parser)]
#[command(name = "xtask", about = "rspinyin build-time tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Available build-time tools.
#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Compile raw dictionary sources into the binary dictionary format.
    ///
    /// Boxed because the argument struct is far larger than the other variants, and the
    /// enum is built once at startup — the indirection costs nothing and keeps the
    /// unboxed size from being paid by every other subcommand.
    Dictc(Box<dictc::DictcArgs>),
    /// Tune the language-model weights, or derive the held-out evaluation set.
    ///
    /// Boxed for the same reason as `Dictc`: the argument struct is much larger than the
    /// other variants and the enum is built once at startup.
    Tune(Box<tune::TuneArgs>),
    /// Validate the performance budget file against the architecture spec, assert the
    /// criterion measurements, or measure the release artifacts.
    Budget {
        /// Compare every threshold with the authoritative table in the spec.
        #[arg(long)]
        validate: bool,
        /// Compare the criterion measurements under `target/criterion` against the
        /// thresholds, exiting non-zero on the first violation. Run `cargo bench` first.
        #[arg(long)]
        check: bool,
        /// Measure the release artifacts under `--dist` against the size thresholds,
        /// exiting non-zero on the first one past its budget. Run `just package` first.
        #[arg(long)]
        measure: bool,
        /// Narrow `--check` to one criterion group, e.g. `decode`.
        #[arg(long, value_name = "GROUP", requires = "check")]
        bench: Option<String>,
        /// Directory the release artifacts `--measure` reads.
        #[arg(long, value_name = "DIR", default_value = "dist")]
        dist: PathBuf,
    },
    /// Check that the addon descriptor advertises the workspace package version.
    CheckVersions,
    /// Print the performance-budget dashboard: the probe snapshot judged against the
    /// thresholds in the budget document.
    Report(report::ReportArgs),
    /// Install the plugin into the Fcitx5 addon directory, or remove it with `--uninstall`.
    ///
    /// Boxed for the same reason as `Dictc`: the argument struct is much larger than the
    /// other variants and the enum is built once at startup.
    Install(Box<install::InstallArgs>),
    /// Produce a releasable archive plus its release manifest and checksums.
    ///
    /// Boxed for the same reason as `Dictc`.
    Package(Box<package::PackageArgs>),
    /// Drive the candidate window from outside: inject keys, probe the environment,
    /// read the focus back.
    ///
    /// Boxed for the same reason as `Dictc`.
    Testd(Box<testd::TestdArgs>),
    /// Replay decode scenarios against the in-memory engine, without fcitx5.
    ///
    /// Boxed for the same reason as `Dictc`.
    TestdEngine(Box<testd::engine::EngineArgs>),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dictc(args) => dictc::run(*args),
        Command::Tune(args) => tune::run(*args),
        Command::Install(args) => install::run(*args),
        Command::Package(args) => package::run(*args),
        Command::Budget {
            validate,
            check,
            measure,
            bench,
            dist,
        } => budget::run(budget::Actions {
            validate,
            check,
            measure,
            bench_group: bench.as_deref(),
            dist: &dist,
        }),
        Command::CheckVersions => versions::run(),
        Command::Report(args) => report::run(args),
        Command::Testd(args) => testd::run(*args),
        Command::TestdEngine(args) => testd::engine::run(*args),
    }
}
