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
mod release;
mod report;
mod soak;
mod testd;
mod tune;
mod verify;
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
    /// Measure candidate quality on the held-out set and audit the correction table.
    ///
    /// Boxed for the same reason as `Dictc`: the argument struct is much larger than the
    /// other variants and the enum is built once at startup.
    Quality(Box<dictc::quality::QualityArgs>),
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
        /// Judge a soak report against the robustness thresholds and exit non-zero on a
        /// violation. Exclusive with the three actions above.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["validate", "check", "measure"])]
        soak_report: Option<PathBuf>,
        /// Narrow `--check` to one criterion group, e.g. `decode`.
        #[arg(long, value_name = "GROUP", requires = "check")]
        bench: Option<String>,
        /// Judge the memory budgets against the readings a running plugin recorded.
        #[arg(long)]
        memory: bool,
        /// The snapshot file `--memory` reads; the plugin's own file by default.
        #[arg(long, value_name = "PATH", requires = "memory")]
        memory_input: Option<PathBuf>,
        /// Directory the release artifacts `--measure` reads.
        #[arg(long, value_name = "DIR", default_value = "dist")]
        dist: PathBuf,
    },
    /// Check that the addon descriptor advertises the workspace package version.
    CheckVersions,
    /// Print the performance-budget dashboard: the probe snapshot judged against the
    /// thresholds in the budget document.
    Report(report::ReportArgs),
    /// Drive continuous input against a live session for a fixed duration, or judge a
    /// report one left behind, against the robustness thresholds.
    ///
    /// `SoakArgs` is under clippy's `large_enum_variant` threshold, so it is not boxed.
    Soak(soak::SoakArgs),
    /// Install the plugin into the Fcitx5 addon directory, or remove it with `--uninstall`.
    ///
    /// Boxed for the same reason as `Dictc`: the argument struct is much larger than the
    /// other variants and the enum is built once at startup.
    Install(Box<install::InstallArgs>),
    /// Produce a releasable archive plus its release manifest and checksums.
    ///
    /// Boxed for the same reason as `Dictc`.
    Package(Box<package::PackageArgs>),
    /// Verify a downloaded release against its manifest, checksum list and signature.
    ///
    /// Read-only: it installs nothing, replaces nothing, and looks for no newer release.
    ///
    /// Not boxed: the argument struct is two paths, far smaller than the other variants.
    Verify(verify::VerifyArgs),
    /// Assert a tag names the version this tree builds, and write the release manifest for
    /// an assembled release directory.
    ///
    /// Not boxed: the argument structs are a path, a version and a list, far smaller than
    /// the other variants.
    Release(release::ReleaseArgs),
    /// Drive the candidate window from outside: inject keys, probe the environment,
    /// read the focus back.
    ///
    /// Boxed for the same reason as `Dictc`.
    Testd(Box<testd::TestdArgs>),
    /// Start the client under test and read back the text the host committed into it.
    ///
    /// Boxed for the same reason as `Dictc`.
    TestdClient(Box<testd::commit_readback::ClientArgs>),
    /// Replay decode scenarios against the in-memory engine, without fcitx5.
    ///
    /// Boxed for the same reason as `Dictc`.
    TestdEngine(Box<testd::engine::EngineArgs>),
    /// Run the P0 baseline case suite and archive one evidence bundle per case.
    ///
    /// Not boxed: the argument struct is a path, a list of identifiers and a module code,
    /// far smaller than the boxed variants above.
    TestdSuite(testd::suite::SuiteArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dictc(args) => dictc::run(*args),
        Command::Quality(args) => dictc::quality::run(*args),
        Command::Tune(args) => tune::run(*args),
        Command::Install(args) => install::run(*args),
        Command::Package(args) => package::run(*args),
        Command::Verify(args) => verify::run(args),
        Command::Release(args) => release::run(args),
        Command::Budget {
            validate,
            check,
            measure,
            soak_report,
            bench,
            memory,
            memory_input,
            dist,
        } => {
            if let Some(path) = soak_report {
                // The soak's verdict is the one `xtask soak --assert` prints; routing it
                // through this subcommand is what lets the soak job reach it without a
                // second implementation of the judgement.
                return soak::assert_report(&path);
            }
            if memory {
                return budget::run_memory(memory_input.as_deref());
            }
            budget::run(budget::Actions {
                validate,
                check,
                measure,
                bench_group: bench.as_deref(),
                dist: &dist,
            })
        }
        Command::CheckVersions => versions::run(),
        Command::Report(args) => report::run(args),
        Command::Soak(args) => soak::run(args),
        Command::Testd(args) => testd::run(*args),
        Command::TestdClient(args) => testd::commit_readback::run(*args),
        Command::TestdEngine(args) => testd::engine::run(*args),
        Command::TestdSuite(args) => testd::suite::run(args),
    }
}
