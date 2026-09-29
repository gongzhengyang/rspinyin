//! Build-time tool host for rspinyin.
//!
//! Subcommands live here rather than in shell scripts so they can share the
//! workspace's dependency versions and error handling. Currently: dictionary
//! compilation, plugin installation, and budget validation.

use clap::Parser;

mod budget;
mod dictc;
mod install;
mod tune;
mod versions;

// `testd` (the automated test platform's input and sandbox channels) is written but not
// yet wired in: it has compile errors against the x11rb 0.14 XTEST API that its author
// could not check without a compiler. Mounting it here is a one-line change once those
// are fixed; leaving it unmounted keeps the workspace compiling in the meantime.

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
    /// Validate the performance budget file against the architecture spec.
    Budget {
        /// Compare every threshold with the authoritative table in the spec.
        #[arg(long)]
        validate: bool,
    },
    /// Check that the addon descriptor advertises the workspace package version.
    CheckVersions,
    /// Install the plugin into the Fcitx5 addon directory, or remove it with `--uninstall`.
    ///
    /// Boxed for the same reason as `Dictc`: the argument struct is much larger than the
    /// other variants and the enum is built once at startup.
    Install(Box<install::InstallArgs>),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dictc(args) => dictc::run(*args),
        Command::Tune(args) => tune::run(*args),
        Command::Install(args) => install::run(*args),
        Command::Budget { validate } => budget::run(validate),
        Command::CheckVersions => versions::run(),
    }
}
