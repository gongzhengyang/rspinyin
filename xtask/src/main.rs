//! Build-time tool host for rspinyin.
//!
//! Subcommands live here rather than in shell scripts so they can share the
//! workspace's dependency versions and error handling. Currently: dictionary
//! compilation, plugin installation, and budget validation.

use clap::Parser;

mod budget;

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
    Dictc,
    /// Validate the performance budget file against the architecture spec.
    Budget {
        /// Compare every threshold with the authoritative table in the spec.
        #[arg(long)]
        validate: bool,
    },
    /// Install the plugin into the Fcitx5 addon directory.
    Install,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dictc | Command::Install => {
            anyhow::bail!("xtask subcommand not implemented yet")
        }
        Command::Budget { validate } => budget::run(validate),
    }
}
