//! `xtask install` -- put the built plugin on the system, and take it back off.
//!
//! Responsibility: turn the build outputs into an installed Fcitx5 addon, and reverse
//! that completely. This is the only place that knows the installed layout as a whole:
//! the destination directories come from [`layout`], the checks that decide whether a
//! build may be installed come from [`elf`] and from the existing `check-versions`
//! gate, the record that makes the install reversible comes from [`manifest`], and the
//! Fcitx5 setting the plugin pins to itself comes from [`takeover`].
//!
//! Boundaries: a build-time and packaging tool. It never runs at IME runtime, it does
//! not build anything -- the caller builds first, so a failed build is reported as a
//! failed build rather than as a failed install -- and it never touches
//! `~/.config/fcitx5/profile`, because the user's input method list is the user's.
//!
//! # What an install refuses
//!
//! Four conditions are checked before the first file on the system changes, because the
//! first three produce a plugin that is present and does not work, and the fourth produces
//! a system no uninstall can put back:
//!
//! 1. **A missing addon factory symbol.** Fcitx5 resolves
//!    [`FACTORY_SYMBOL`] with `dlsym`; a library stripped with `--strip-all` still
//!    loads and contains no addon. The installer strips what it installs, so it
//!    verifies the symbol in the file it is about to copy.
//! 2. **A descriptor whose version disagrees with the workspace.** Fcitx5 reports the
//!    descriptor's value, so the mismatch ships a plugin that mislabels itself. The
//!    `check-versions` gate is run rather than reimplemented.
//! 3. **A destination that cannot be written.** Probed before the first copy, so that
//!    an install which cannot finish does not start.
//! 4. **A destination that is not a regular file, or a backup this run cannot account
//!    for.** See [`place`]: a symbolic link, a directory or a device at a destination
//!    cannot be copied aside and put back, and a `.rspinyin-bak` no manifest mentions
//!    holds the only copy of what an earlier run displaced.
//!
//! # Modules
//!
//! This root holds the command line, the constants every other module names, and the
//! sequence an install is. The rest is split by responsibility: [`payload`] names the
//! files an install consists of and resolves them into a plan, [`size`] holds the budgets
//! they are measured against, [`stage`] produces a stripped copy of a library and confirms
//! the symbol survived it, [`verify`] runs the gates a whole plan passes, [`place`]
//! carries the plan out and records what it displaced, [`report`] is what the user is
//! told, and [`icon_cache`] refreshes the desktop's icon index once the icons have landed.
//!
//! # Adding the second library
//!
//! The plugin ships as two addons -- an `InputMethod` engine and a `UI` addon -- and the
//! build produces the engine today. Everything an install does is driven by
//! [`payload::PAYLOADS`], so the UI addon is two rows there and nothing else: its library
//! with [`Destination::AddonLibrary`](layout::Destination::AddonLibrary), which puts it
//! through the symbol check, and its descriptor with
//! [`Destination::AddonDescriptor`](layout::Destination::AddonDescriptor). Uninstall needs
//! no change at all: it works from the manifest, which is written from the same table.

// `elf` and `layout` are visible to the rest of the crate because `xtask package` reuses
// them: the packager resolves its payloads through the same artifact lookup and reads the
// same dynamic symbol table before it lets a library out of the repository. The remaining
// submodules stay private -- the manifest and the uninstall record describe an installed
// system, which is not what a release is.
pub(crate) mod elf;
pub(crate) mod layout;
mod manifest;
// The round-trip verification drives the real install and the real uninstall and compares
// the tree before and after. Nothing on the production path calls it, so it is compiled for
// the tests alone -- a `mod` here would be dead code, which the workspace denies.
#[cfg(test)]
mod reversible;
mod takeover;
mod uninstall;

mod icon_cache;
mod payload;
mod place;
mod report;
mod size;
mod stage;
mod verify;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;

// `xtask package` strips, checks and measures through the same items an install does: a
// release and an install have to agree on what a budget is and on what a stripped library
// exports, so the five names it needs are re-exported here rather than duplicated there.
pub(crate) use self::size::{Budget, SizeBudgets, check_size_in};
pub(crate) use self::stage::{StripPolicy, stage_library_file};

use self::icon_cache::update_icon_cache;
use self::layout::{Layout, Sources};
use self::manifest::Elevation;
use self::payload::{PAYLOADS, plan_install};
use self::place::apply;
use self::report::{report_installed, report_plan};
use self::stage::stage_library;
use self::verify::verify_sizes;

/// The symbol Fcitx5 resolves with `dlsym` after `dlopen`.
///
/// A library that does not export it loads and provides no addon at all, so an install
/// that cannot find it is refused. The name is fixed by Fcitx5's addon factory macro
/// and is the same one `ime-fcitx5`'s build script forces the linker to keep.
pub(crate) const FACTORY_SYMBOL: &str = "fcitx_addon_factory_instance";

/// Version this build installs.
///
/// Taken from the workspace package version, which every crate in the workspace shares.
/// [`crate::versions::run`] has already asserted that the addon descriptor advertises
/// the same value, so recording it in the manifest needs no second parse of either
/// file.
pub(crate) const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the compiled dictionary is looked for, in the order it is tried.
///
/// The specification names `target/dict/base.dict` and `xtask dictc` writes
/// `data/compiled/base.dict` by default. Both are build artifacts of the same file, so
/// both are accepted and whichever is present is the one installed.
pub(crate) const DICTIONARY: &[&str] = &["target/dict/base.dict", "data/compiled/base.dict"];

/// What the command line asked for, in the form both directions share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Print the plan and change nothing.
    pub dry_run: bool,
    /// Never elevate through `sudo`.
    pub no_sudo: bool,
}

/// Command-line surface of `xtask install`.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Installation prefix; defaults to the one `pkg-config` reports for Fcitx5.
    #[arg(long)]
    prefix: Option<PathBuf>,
    /// Staging directory prepended to every destination, for packaging.
    #[arg(long)]
    destdir: Option<PathBuf>,
    /// Print the plan and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Remove the plugin instead of installing it.
    #[arg(long)]
    uninstall: bool,
    /// Never elevate through `sudo`; every destination must be writable as it is.
    #[arg(long)]
    no_sudo: bool,
}

/// Entry point for `xtask install`.
///
/// # Errors
///
/// Returns an error when the Fcitx5 development package cannot be found, when a build
/// artifact is missing or fails its symbol or size check, when a destination cannot be
/// written, and when the manifest or the takeover record cannot be read or written.
pub fn run(args: InstallArgs) -> Result<()> {
    let root = resolve_root(args.root.as_deref())?;
    let layout = Layout::probe(args.prefix.as_deref(), args.destdir.as_deref())?;
    let options = Options {
        dry_run: args.dry_run,
        no_sudo: args.no_sudo,
    };
    if args.uninstall {
        return uninstall::uninstall(&layout, &options);
    }
    install(&Sources::new(root), &layout, &options)
}

/// Copies every payload into place and records what it did.
///
/// # Errors
///
/// As [`run`], plus a size budget that a built artifact passes.
fn install(sources: &Sources, layout: &Layout, options: &Options) -> Result<()> {
    // Fcitx5 reports the descriptor's version, not the library's, so a descriptor left
    // behind by a version bump installs a plugin that mislabels itself. The gate that
    // catches it already exists; running it here is what keeps the two from drifting.
    crate::versions::run()?;

    let budgets = SizeBudgets::load(sources)?;
    let mut plan = plan_install(PAYLOADS, sources, layout)?;
    let elevation = Elevation::detect(options.no_sudo)?;
    if options.dry_run {
        report_plan(&plan, layout, elevation, &budgets);
        return Ok(());
    }

    stage_library(sources, &mut plan)?;
    verify_sizes(&plan, &budgets)?;
    if !elevation.is_sudo() {
        manifest::check_writable(&layout.destination_directories())?;
    }

    let manifest = apply(&plan, layout, elevation)?;
    update_icon_cache(layout, elevation)?;
    report_installed(&manifest, layout);
    Ok(())
}

/// Resolves the repository root: the explicit `--root`, or the parent of the xtask crate.
///
/// The default is derived from `CARGO_MANIFEST_DIR` rather than from the working
/// directory, because every default artifact path is relative to the repository root
/// while a test binary runs with the working directory set to the crate.
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
