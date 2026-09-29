//! Build-time tool host for rspinyin.
//!
//! Subcommands live here rather than in shell scripts so they can share the
//! workspace's dependency versions and error handling. Currently: dictionary
//! compilation, plugin installation, and budget validation.

use clap::Parser;

mod budget;
mod dictc;
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
    /// Validate the performance budget file against the architecture spec.
    Budget {
        /// Compare every threshold with the authoritative table in the spec.
        #[arg(long)]
        validate: bool,
    },
    /// Check that the addon descriptor advertises the workspace package version.
    CheckVersions,
    /// Install the plugin into the Fcitx5 addon directory.
    Install,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dictc(args) => dictc::run(*args),
        Command::Install => anyhow::bail!("xtask subcommand not implemented yet"),
        Command::Budget { validate } => budget::run(validate),
        Command::CheckVersions => versions::run(),
    }
}
