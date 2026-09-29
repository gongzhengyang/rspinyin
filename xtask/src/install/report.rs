//! What the user is told: what an install would do, and what it did.
//!
//! Responsibility: render the plan, the outcome and the two steps that make an installed
//! plugin usable. Every line is prefixed with the command it belongs to, so a run that
//! prints both directions is still readable, and none of them carries a path the user did
//! not ask for.
//!
//! Boundaries: this module prints and decides nothing. [`report_plan`] is handed the
//! plan the run really built, never a second derivation of it, so a dry run describes
//! exactly what the same command without `--dry-run` would do; and the reload hint is a
//! hint: the installer never restarts Fcitx5 itself, and it never edits the user's input
//! method list.

use super::layout::Layout;
use super::manifest::{Elevation, Manifest};
use super::payload::PlannedFile;
use super::size::SizeBudgets;

/// Prints what an install would do, without doing any of it.
pub(super) fn report_plan(
    plan: &[PlannedFile],
    layout: &Layout,
    elevation: Elevation,
    budgets: &SizeBudgets,
) {
    println!("install: dry run -- nothing will be written");
    for file in plan {
        println!(
            "install:   {} -> {}",
            file.source.display(),
            file.destination.display()
        );
    }
    println!("install: manifest {}", layout.manifest_path().display());
    println!(
        "install: privileged operations: {}",
        if elevation.is_sudo() { "sudo" } else { "none" }
    );
    println!(
        "install: budgets BUDGET-SIZE-01 <= {:.2}MiB, BUDGET-SIZE-02 <= {:.2}MiB",
        budgets.library_mb, budgets.dictionary_mb
    );
    print_reload_hint();
}

/// Prints what was installed and what the user has to do next.
pub(super) fn report_installed(manifest: &Manifest, layout: &Layout) {
    println!(
        "install: {} files installed, version {}",
        manifest.entries.len(),
        manifest.package_version
    );
    println!("install: manifest {}", layout.manifest_path().display());
    print_reload_hint();
}

/// Prints the two steps that make an installed plugin usable.
fn print_reload_hint() {
    println!("install: run `fcitx5 -r` to reload Fcitx5");
    println!(
        "install: add Rust Pinyin in `fcitx5-configtool` (the input method list is yours; \
              this installer does not edit it)"
    );
}
