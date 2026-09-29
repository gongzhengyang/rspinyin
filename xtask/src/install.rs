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
//! Three conditions are checked before the first file is copied, because all three
//! produce a plugin that is present and does not work:
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
//!
//! # Adding the second library
//!
//! The plugin ships as two addons -- an `InputMethod` engine and a `UI` addon -- and
//! the build produces the engine today. Everything an install does is driven by
//! [`PAYLOADS`], so the UI addon is two rows there and nothing else: its library with
//! [`Destination::AddonLibrary`], which puts it through the symbol check, and its
//! descriptor with [`Destination::AddonDescriptor`]. Uninstall needs no change at all:
//! it works from the manifest, which is written from the same table.

// `elf` and `layout` are visible to the rest of the crate because `xtask package` reuses
// them: the packager resolves its payloads through the same artifact lookup and reads the
// same dynamic symbol table before it lets a library out of the repository. The remaining
// submodules stay private -- the manifest and the uninstall record describe an installed
// system, which is not what a release is.
pub(crate) mod elf;
pub(crate) mod layout;
mod manifest;
mod takeover;
mod uninstall;

#[cfg(test)]
mod tests;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use clap::Args;

use self::layout::{Destination, Layout, Sources};
use self::manifest::{Elevation, Entry, EntryState, FILE_MODE, MANIFEST_VERSION, Manifest};

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

/// Which size budget a payload is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Budget {
    /// `BUDGET-SIZE-01`: the release addon library, measured after stripping.
    StrippedLibrary,
    /// `BUDGET-SIZE-02`: the built-in base dictionary.
    BaseDictionary,
}

impl Budget {
    /// The budget identifier as the specification writes it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::StrippedLibrary => "BUDGET-SIZE-01",
            Self::BaseDictionary => "BUDGET-SIZE-02",
        }
    }
}

/// Size thresholds, read from the machine-readable budget document.
pub(crate) struct SizeBudgets {
    /// `BUDGET-SIZE-01`, in mebibytes.
    pub(crate) library_mb: f64,
    /// `BUDGET-SIZE-02`, in mebibytes.
    pub(crate) dictionary_mb: f64,
}

impl SizeBudgets {
    /// Reads the thresholds from `docs/dev/budgets.json`.
    ///
    /// Read rather than written into this file so that a budget change reaches the
    /// installer, the packager, the benchmarks and the specification's table together.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be read or parsed.
    pub(crate) fn load(sources: &Sources) -> Result<Self> {
        let path = sources.root.join(crate::budget::BUDGETS_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let budgets = crate::budget::Budgets::from_json(&text)
            .with_context(|| format!("{}: invalid budget document", path.display()))?;
        Ok(Self {
            library_mb: budgets.size_mb.so_stripped,
            dictionary_mb: budgets.size_mb.base_dict,
        })
    }

    /// The threshold one budget names, in mebibytes.
    pub(crate) fn limit(&self, budget: Budget) -> f64 {
        match budget {
            Budget::StrippedLibrary => self.library_mb,
            Budget::BaseDictionary => self.dictionary_mb,
        }
    }
}

/// One file an install puts on the system.
struct Payload {
    /// File name at the destination.
    name: &'static str,
    /// Paths of the built file, relative to the repository root; the first one present
    /// is the one installed.
    artifact: &'static [&'static str],
    /// Directory the file belongs in.
    destination: Destination,
    /// Whether a missing artifact is skipped rather than reported.
    ///
    /// Only the icons are optional: they belong to a release's artwork, and a build
    /// that has not produced them yet must still be installable.
    optional: bool,
    /// The size budget the file is measured against, if it has one.
    budget: Option<Budget>,
}

/// Everything an install puts on the system, in the order it is copied.
///
/// The two libraries come first, and they are the reason the table exists rather than a
/// list of literal paths: the installer has to treat them differently from the rest —
/// each is staged, stripped and checked for [`FACTORY_SYMBOL`] — and the second library
/// goes through exactly the same steps as the first. That is also why the destination
/// model did not have to change when ADR-0003 split the plugin in two.
const PAYLOADS: &[Payload] = &[
    Payload {
        name: "librspinyin.so",
        artifact: &["target/release/librspinyin.so"],
        destination: Destination::AddonLibrary,
        optional: false,
        budget: Some(Budget::StrippedLibrary),
    },
    Payload {
        name: "librspinyin_ui.so",
        artifact: &["target/release/librspinyin_ui.so"],
        destination: Destination::AddonLibrary,
        optional: false,
        budget: Some(Budget::StrippedLibrary),
    },
    Payload {
        name: "rspinyin.conf",
        artifact: &["packaging/fcitx5/rspinyin.conf"],
        destination: Destination::AddonDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "rspinyin-ui.conf",
        artifact: &["packaging/fcitx5/rspinyin-ui.conf"],
        destination: Destination::AddonDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "rspinyin.conf",
        artifact: &["packaging/fcitx5/rspinyin-im.conf"],
        destination: Destination::InputMethodDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "base.dict",
        artifact: DICTIONARY,
        destination: Destination::Data,
        optional: false,
        budget: Some(Budget::BaseDictionary),
    },
    Payload {
        name: "fcitx-rspinyin.png",
        artifact: &["assets/icon-48.png"],
        destination: Destination::Icon { size: "48x48" },
        optional: true,
        budget: None,
    },
    Payload {
        name: "fcitx-rspinyin.svg",
        artifact: &["assets/icon.svg"],
        destination: Destination::Icon { size: "scalable" },
        optional: true,
        budget: None,
    },
];

/// One file an install will copy, once both of its paths are known.
#[derive(Debug)]
struct PlannedFile {
    /// File name at the destination.
    name: &'static str,
    /// Path of the built file to copy; the staged copy for a library.
    source: PathBuf,
    /// Absolute destination path, with `DESTDIR` applied.
    destination: PathBuf,
    /// Whether the file is a shared library Fcitx5 `dlopen`s, and therefore has to
    /// export [`FACTORY_SYMBOL`].
    is_addon_library: bool,
    /// The size budget this file is measured against, if it has one.
    budget: Option<Budget>,
}

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
    update_icon_cache(layout)?;
    report_installed(&manifest, layout);
    Ok(())
}

/// Records a plan and copies it into place: the reversible half of an install.
///
/// Split out from [`install`] so that the sequence that makes an install reversible --
/// classify, record, copy -- can be exercised against a scratch tree, with no build, no
/// target system and no `sudo`.
///
/// # Errors
///
/// Returns an error when a destination cannot be classified, when the manifest cannot
/// be written, and when a copy fails. The manifest is on disk before the first copy, so
/// an uninstall can still undo a run that failed part way.
fn apply(plan: &[PlannedFile], layout: &Layout, elevation: Elevation) -> Result<Manifest> {
    // The manifest goes down before the first copy, so an install interrupted half way
    // is still fully reversible: an uninstall can see which files are ours, and which
    // of them displaced something that has to come back.
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        package_version: PACKAGE_VERSION.to_owned(),
        entries: classify(plan, layout, elevation)?,
    };
    manifest.write(&layout.manifest_path())?;
    copy_into_place(plan, elevation)?;
    Ok(manifest)
}

/// Resolves every payload's built file and destination path.
///
/// The payload table is a parameter rather than a constant read here, so that the plan
/// a caller builds is the plan that gets applied.
///
/// # Errors
///
/// Returns an error when a payload that is not optional has no built file.
fn plan_install(
    payloads: &[Payload],
    sources: &Sources,
    layout: &Layout,
) -> Result<Vec<PlannedFile>> {
    let mut plan = Vec::new();
    for payload in payloads {
        let source = match sources.artifact(payload.artifact) {
            Ok(source) => source,
            Err(error) if payload.optional => {
                println!("install: skipping {}: {error}", payload.name);
                continue;
            }
            Err(error) => return Err(error),
        };
        plan.push(PlannedFile {
            name: payload.name,
            source,
            destination: layout.destination(payload.destination, payload.name),
            is_addon_library: payload.destination == Destination::AddonLibrary,
            budget: payload.budget,
        });
    }
    Ok(plan)
}

/// What a caller wants done when `strip` is not installed on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StripPolicy {
    /// Keep going with the unstripped copy and let the size check decide.
    ///
    /// `xtask install` writes into a running system, where an unstripped library still
    /// works: a machine without binutils must not be unable to install the plugin, and
    /// the size budget still fails the install if the library is too large.
    Tolerate,
    /// Refuse to produce the copy.
    ///
    /// `xtask package` produces the files that leave the repository, and "every shipped
    /// library has been stripped" is the one promise the packaging step makes. It is
    /// also the only chance to make it: `strip = true` in the release profile would pass
    /// `--strip-all` to the linker and take [`FACTORY_SYMBOL`] out of the dynamic symbol
    /// table, which produces a library that loads and contains no addon.
    Require,
}

/// Copies one addon library into the staging directory, strips the copy and verifies it.
///
/// Shared by `xtask install` and `xtask package`: both hand the next step a stripped,
/// symbol-checked copy of each addon library rather than the build tree's own file, and
/// neither may ship a library Fcitx5 would load and find no addon in. The build tree is
/// left as cargo produced it -- `strip` rewrites its argument in place, and an artifact
/// a later step can no longer inspect is a worse trade than a copy.
///
/// # Errors
///
/// Returns an error when the copy cannot be written, when `strip` runs and fails, when
/// `strip` is missing under [`StripPolicy::Require`], and when the staged library does
/// not export [`FACTORY_SYMBOL`].
pub(crate) fn stage_library_file(source: &Path, staged: &Path, policy: StripPolicy) -> Result<()> {
    fs::copy(source, staged)
        .with_context(|| format!("staging {} at {}", source.display(), staged.display()))?;
    strip(staged, policy)?;
    verify_factory_symbol(staged)
}

/// Strips every addon library into the staging directory and verifies it.
///
/// # Errors
///
/// Returns an error when the staging directory cannot be created, when `strip` fails,
/// or when a staged library does not export [`FACTORY_SYMBOL`].
fn stage_library(sources: &Sources, plan: &mut [PlannedFile]) -> Result<()> {
    if !plan.iter().any(|file| file.is_addon_library) {
        return Ok(());
    }
    fs::create_dir_all(&sources.staging_dir)
        .with_context(|| format!("creating {}", sources.staging_dir.display()))?;
    for file in plan.iter_mut().filter(|file| file.is_addon_library) {
        let staged = sources.staging_dir.join(file.name);
        stage_library_file(&file.source, &staged, StripPolicy::Tolerate)?;
        file.source = staged;
    }
    Ok(())
}

/// Refuses a library Fcitx5 would load and find no addon in.
///
/// # Errors
///
/// Returns an error when the file does not export [`FACTORY_SYMBOL`], and when it
/// cannot be read or is not an inspectable ELF image -- the second of which is itself
/// the signature of an over-stripped library, so the file is refused rather than
/// assumed to be fine.
fn verify_factory_symbol(path: &Path) -> Result<()> {
    ensure!(
        elf::exports_path(path, FACTORY_SYMBOL)?,
        "install: {} does not export `{FACTORY_SYMBOL}`, so Fcitx5 would load it and find no \
         addon. `strip --strip-all` removes that name from the dynamic symbol table; rebuild \
         with `cargo build --release -p ime-fcitx5 --features fcitx5-host` and let the \
         installer or the packager strip the copy.",
        path.display()
    );
    Ok(())
}

/// Strips a copy of an addon library in place.
///
/// `--strip-unneeded` rather than `--strip-all`: the latter drops
/// [`FACTORY_SYMBOL`] from the dynamic symbol table, which produces exactly the broken
/// library [`stage_library_file`] refuses to hand on. What to do when `strip` is not
/// installed at all is the caller's decision, and [`StripPolicy`] is where it is made.
///
/// # Errors
///
/// Returns an error when `strip` runs and fails, and when it is missing under
/// [`StripPolicy::Require`].
fn strip(path: &Path, policy: StripPolicy) -> Result<()> {
    let output = match Command::new("strip")
        .arg("--strip-unneeded")
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => match policy {
            StripPolicy::Tolerate => {
                println!("install: `strip` is not installed; installing the library unstripped");
                return Ok(());
            }
            StripPolicy::Require => anyhow::bail!(
                "`strip` is not installed, so a stripped copy of {} cannot be produced and no \
                 release can be packaged. binutils is the dependency; `strip --strip-all` is \
                 not an alternative, because it removes `{FACTORY_SYMBOL}` from the dynamic \
                 symbol table and leaves a library that loads and contains no addon.",
                path.display()
            ),
        },
        Err(error) => {
            return Err(error).with_context(|| format!("running strip on {}", path.display()));
        }
    };
    ensure!(
        output.status.success(),
        "strip {} failed: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Measures every payload that carries a size budget.
///
/// # Errors
///
/// Returns an error when a file passes its budget.
fn verify_sizes(plan: &[PlannedFile], budgets: &SizeBudgets) -> Result<()> {
    for file in plan {
        let Some(budget) = file.budget else {
            continue;
        };
        check_size(budget, &file.source, budgets.limit(budget))?;
    }
    Ok(())
}

/// Fails when a file passes its size budget.
///
/// A budget is a gate rather than a warning: the specification's thresholds are
/// contractual, so a regression past one stops the install instead of being printed
/// and forgotten.
///
/// # Errors
///
/// Returns an error naming the budget, the measured size and the threshold.
fn check_size(budget: Budget, path: &Path, limit_mb: f64) -> Result<()> {
    check_size_in("install", budget, path, limit_mb)
}

/// Fails when a file passes its size budget, reporting under `scope`.
///
/// `scope` is the command the measurement belongs to (`install`, `package`), so a
/// failure says which of the two stopped rather than leaving the reader to guess from
/// the paths.
///
/// # Errors
///
/// Returns an error naming the budget, the measured size and the threshold.
pub(crate) fn check_size_in(scope: &str, budget: Budget, path: &Path, limit: f64) -> Result<()> {
    let bytes = fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let size_mb = bytes as f64 / (1024.0 * 1024.0);
    if size_mb > limit {
        report_sections(scope, path);
        anyhow::bail!(
            "{scope}: {} failed: {} is {size_mb:.2}MiB, over the {limit:.2}MiB threshold",
            budget.id(),
            path.display()
        );
    }
    println!(
        "{scope}: {} {} is {size_mb:.2}MiB, within {limit:.2}MiB",
        budget.id(),
        path.display()
    );
    Ok(())
}

/// Prints a per-section size breakdown, when a tool that can produce one is installed.
///
/// Neither `bloaty` nor `cargo-bloat` is a build dependency, so a machine without them
/// gets the measured size and nothing more rather than a failure for the wrong reason.
fn report_sections(scope: &str, path: &Path) {
    let breakdown = Command::new("bloaty")
        .arg("-n")
        .arg("0")
        .arg("-d")
        .arg("sections")
        .arg(path)
        .output();
    match breakdown {
        Ok(output) if output.status.success() => {
            println!(
                "{scope}: per-section breakdown\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        _ => println!(
            "{scope}: no `bloaty` on this system, so no per-section breakdown; \
             `cargo bloat` against the build tree is the alternative"
        ),
    }
}

/// Decides what each destination is before anything is copied, backing up what it
/// displaces.
///
/// # Errors
///
/// Returns an error when an existing file cannot be copied aside, and when a manifest
/// left by an earlier install cannot be read.
fn classify(plan: &[PlannedFile], layout: &Layout, elevation: Elevation) -> Result<Vec<Entry>> {
    let previous = Manifest::read(&layout.manifest_path())?;
    let mut entries = Vec::with_capacity(plan.len());
    for file in plan {
        // A destination this installer already owns keeps the entry it had: the backup
        // taken the first time holds what was there before, and overwriting it with the
        // file we installed would lose the only copy of it.
        match previous
            .as_ref()
            .and_then(|manifest| manifest.entry(&file.destination))
        {
            Some(entry) => entries.push(entry.clone()),
            None => entries.push(prepare_entry(file, elevation)?),
        }
    }
    Ok(entries)
}

/// Classifies one destination and copies aside what is already there.
///
/// # Errors
///
/// Returns an error when an existing file cannot be copied to its backup path.
fn prepare_entry(file: &PlannedFile, elevation: Elevation) -> Result<Entry> {
    if !file.destination.exists() {
        return Ok(Entry {
            path: file.destination.clone(),
            state: EntryState::Created,
            backup: None,
        });
    }
    let backup = manifest::backup_path(&file.destination);
    manifest::place_file(elevation, &file.destination, &backup, FILE_MODE)?;
    println!(
        "install: {} was already there; the copy it displaced is kept at {}",
        file.destination.display(),
        backup.display()
    );
    Ok(Entry {
        path: file.destination.clone(),
        state: EntryState::Replaced,
        backup: Some(backup),
    })
}

/// Copies every planned file to its destination.
///
/// # Errors
///
/// Returns an error when a copy fails. The manifest is already on disk at that point,
/// so an uninstall can still undo what was done.
fn copy_into_place(plan: &[PlannedFile], elevation: Elevation) -> Result<()> {
    for file in plan {
        manifest::place_file(elevation, &file.source, &file.destination, FILE_MODE)?;
        println!(
            "install: {} -> {}",
            file.source.display(),
            file.destination.display()
        );
    }
    Ok(())
}

/// Refreshes the icon theme cache, when the tool that does it is installed.
///
/// Skipped for a `DESTDIR` staging tree: the cache has to describe the tree the icons
/// end up in, which is the packaging step's business, and indexing the staging path
/// would leave an index describing directories that do not exist on the target system.
///
/// # Errors
///
/// Returns an error when the tool runs and fails.
fn update_icon_cache(layout: &Layout) -> Result<()> {
    if !layout.destdir.as_os_str().is_empty() || !layout.icon_dir.exists() {
        return Ok(());
    }
    let output = match Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-t")
        .arg("-f")
        .arg(&layout.icon_dir)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            println!(
                "install: `gtk-update-icon-cache` is not installed; the icon theme cache was \
                 not refreshed"
            );
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "running gtk-update-icon-cache on {}",
                    layout.icon_dir.display()
                )
            });
        }
    };
    ensure!(
        output.status.success(),
        "gtk-update-icon-cache {} failed: {}",
        layout.icon_dir.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Prints what an install would do, without doing any of it.
fn report_plan(plan: &[PlannedFile], layout: &Layout, elevation: Elevation, budgets: &SizeBudgets) {
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
fn report_installed(manifest: &Manifest, layout: &Layout) {
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
