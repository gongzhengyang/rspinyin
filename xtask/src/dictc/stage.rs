//! Staging a compiled dictionary for a hot swap.
//!
//! Responsibility: copy a compiled container into the data directory's staged location with
//! the layout's own private modes, and print the two steps that follow — the configuration
//! key that names the staged file, and the reload that triggers the swap. The plugin does
//! the rest at the reload: full validation, the atomic swap, and the write-back that clears
//! the key.
//!
//! Boundaries: this is a helper, not a trigger. It never edits the configuration file and
//! never restarts the plugin — both are the user's hand, which is what keeps a staged file
//! an explicit act rather than a side effect of a build.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use anyhow::Context;

/// Arguments of `xtask dict stage`.
#[derive(Debug, clap::Parser)]
pub struct StageArgs {
    /// The compiled container to stage, as `dictc` produced it.
    pub file: PathBuf,
}

/// The file name the staged container sits at inside the data directory.
const STAGED_FILE: &str = "staged.dict";

/// Copies `args.file` into the staged location and prints the follow-up steps.
///
/// # Errors
///
/// The source is not a file, the data directory cannot be prepared, or the copy fails.
pub fn run(args: StageArgs) -> anyhow::Result<()> {
    let source = &args.file;
    anyhow::ensure!(source.is_file(), "not a file: {}", source.display());
    let paths = ime_dict::paths::ensure_dirs().context("preparing the data directories")?;
    let staged = paths.data_dir.join(STAGED_FILE);
    fs::copy(source, &staged)
        .with_context(|| format!("copying {} to {}", source.display(), staged.display()))?;
    fs::set_permissions(
        &staged,
        fs::Permissions::from_mode(ime_dict::paths::FILE_MODE),
    )
    .with_context(|| format!("narrowing the mode of {}", staged.display()))?;

    println!("staged {} as {}", source.display(), staged.display());
    println!("next steps:");
    println!("  1. set the staged path in the configuration file, e.g.:");
    println!();
    println!("      [data]");
    println!("      dict_staged_path = \"{}\"", staged.display());
    println!();
    println!(
        "     the configuration file is at {}",
        ime_config::default_path()
            .unwrap_or_else(|| PathBuf::from("<no configuration directory>"))
            .display()
    );
    println!(
        "  2. reload the fcitx5 configuration (`fcitx5-remote -r`, or the config tool's reload)"
    );
    println!(
        "     the plugin validates the staged container, swaps it in atomically, and clears the key;"
    );
    println!(
        "     a container that fails validation is rejected, the old dictionary keeps serving, and the"
    );
    println!("     reason is reported as dict/swap/rejected on the diagnostic channel");
    Ok(())
}
