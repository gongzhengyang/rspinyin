//! `xtask package` -- turn the build tree into a release.
//!
//! Responsibility: stage the payloads a release consists of, strip and verify every
//! addon library, measure each file against its size budget, and write the three files a
//! release is -- the tarball, `rspinyin-release.json`, and `SHA256SUMS`.
//!
//! # Why this is not `install --destdir`
//!
//! `xtask install` writes into a live Fcitx5 installation and records a manifest so an
//! uninstall can restore what it displaced. A release is the opposite: it writes into an
//! empty directory that nobody has installed into, and what it must record is not "what
//! was displaced" but "what this artifact is, and how a user can prove it arrived
//! intact". Sharing the stripping and symbol-checking code is the point; sharing the
//! destination model is not.
//!
//! # Why the strip happens here rather than in `[profile.release]`
//!
//! `strip = true` makes rustc pass `--strip-all` to the linker, which removes
//! `fcitx_addon_factory_instance` from the dynamic symbol table -- the name Fcitx5
//! resolves with `dlsym`. The resulting library loads and contains no addon. Stripping at
//! packaging time with `--strip-unneeded` keeps the dynamic table intact, and
//! [`crate::install::elf`] re-reads it afterwards, so the promise is checked rather than
//! assumed.
//!
//! # What the release directory holds
//!
//! `dist/` gets the payloads themselves, the archive that bundles them, the manifest and
//! the checksum list. The payloads are written beside the archive as well as inside it
//! because the two are read by different things: a distribution packager copies the files
//! it needs, and `xtask budget --measure` asserts the sizes of the files that were
//! actually stripped, without unpacking anything.
//!
//! # Why GNU tar rather than a crate
//!
//! The archive is written by `tar` and compressed by `gzip -n`. The alternative is a
//! `tar` + `flate2` dependency pair for the packaging step alone, and the flags that make
//! the archive reproducible -- `--sort=name`, `--mtime`, `--owner=0 --group=0
//! --numeric-owner` -- are GNU tar's own, so shelling out to the tool the build already
//! requires keeps xtask's dependency set where it is.
//!
//! # What this step does not claim
//!
//! The manifest's `signature` field is `null` until a release is signed: a placeholder
//! key id would look like a signature that was checked. The archive's own digest is
//! recorded in `SHA256SUMS` rather than in the manifest, whose `artifacts` list describes
//! what the archive contains.

mod manifest;
mod stamp;

#[cfg(test)]
mod tests;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use clap::Args;

use self::manifest::{Artifact, Release};
use crate::install::layout::Sources;
use crate::install::{
    Budget, PACKAGE_VERSION, SizeBudgets, StripPolicy, check_size_in, elf, stage_library_file,
};

/// File name of the base dictionary inside a release.
pub(crate) const DICTIONARY_FILE: &str = "base.dict";

/// The addon libraries a release ships, in the order they are written.
///
/// Both are addons: ADR-0003 splits the plugin into an `InputMethod` addon and a `UI`
/// addon, each a separate cdylib, because Fcitx5 resolves the active user interface from
/// the addons it discovered with `Category=UI` and one addon cannot belong to two
/// categories. The names carry an underscore because Cargo rejects a hyphen in a library
/// target's name while Fcitx5 appends `.so` to whatever the descriptor's `Library=` says.
///
/// The size gate measures these two names, so they are written down once and read from
/// here; the payload table below and the gate's table of measured files are both guarded
/// against drift by a test.
pub(crate) const ADDON_LIBRARIES: [&str; 2] = ["librspinyin.so", "librspinyin_ui.so"];

/// Directory the release tree is staged in, inside the workspace target directory.
const STAGING_DIR: &str = "package";

/// The compressor the archive is written through.
///
/// `gzip -n` rather than `tar --gzip`: the gzip header carries a file name and a
/// timestamp, and `-n` is what clears them. Two builds of the same commit producing the
/// same archive bytes is not something this step asserts, but a release that embeds the
/// build machine's clock and staging path in its header is not one anybody can compare
/// later either.
const COMPRESSOR: &str = "gzip -n";

/// One file a release ships.
struct Payload {
    /// File name in the release directory and inside the archive.
    name: &'static str,
    /// Build outputs the file is taken from, relative to the repository root; the first
    /// one present is the one shipped.
    candidates: &'static [&'static str],
    /// What the file is for, as the manifest records it.
    role: &'static str,
    /// Whether a missing build output is an error.
    ///
    /// The licence texts are the only optional payloads. The icons are not: they are
    /// committed to the repository rather than produced by the build, so an absent one is
    /// a broken checkout, and a release that left them out would install a plugin
    /// `fcitx5-configtool` shows with a placeholder. What must not happen to an optional
    /// payload is a *silent* omission, so one that is absent is reported by name and
    /// reason at the end of every run.
    optional: bool,
    /// The size budget the file is measured against, if it has one.
    budget: Option<Budget>,
    /// Symbols the file must export. Empty for everything but the two libraries.
    exports: &'static [&'static str],
}

/// Everything a release ships, in the order it is written.
///
/// The two libraries come first because they are the files that are stripped and read
/// back, and the descriptors follow them because a library without a descriptor is an
/// addon Fcitx5 never discovers.
const PAYLOADS: &[Payload] = &[
    Payload {
        name: ADDON_LIBRARIES[0],
        candidates: &["target/release/librspinyin.so"],
        role: "addon-inputmethod",
        optional: false,
        budget: Some(Budget::StrippedLibrary),
        exports: &[crate::install::FACTORY_SYMBOL],
    },
    Payload {
        name: ADDON_LIBRARIES[1],
        candidates: &["target/release/librspinyin_ui.so"],
        role: "addon-ui",
        optional: false,
        budget: Some(Budget::StrippedLibrary),
        exports: &[crate::install::FACTORY_SYMBOL],
    },
    Payload {
        name: "rspinyin.conf",
        candidates: &["packaging/fcitx5/rspinyin.conf"],
        role: "addon-descriptor",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "rspinyin-ui.conf",
        candidates: &["packaging/fcitx5/rspinyin-ui.conf"],
        role: "addon-descriptor",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "rspinyin-im.conf",
        candidates: &["packaging/fcitx5/rspinyin-im.conf"],
        role: "inputmethod-descriptor",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: DICTIONARY_FILE,
        candidates: crate::install::DICTIONARY,
        role: "dictionary",
        optional: false,
        budget: Some(Budget::BaseDictionary),
        exports: &[],
    },
    Payload {
        name: "fcitx-rspinyin.png",
        candidates: &["assets/icon-48.png"],
        role: "icon",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "fcitx-rspinyin.svg",
        candidates: &["assets/icon.svg"],
        role: "icon",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "NOTICE",
        candidates: &["docs/dev/NOTICE"],
        role: "notice",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "LICENSE-APACHE",
        candidates: &["LICENSE-APACHE"],
        role: "license",
        optional: true,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "LICENSE-MIT",
        candidates: &["LICENSE-MIT"],
        role: "license",
        optional: true,
        budget: None,
        exports: &[],
    },
    Payload {
        // A name with a separator in it: the archive carries the same directory
        // structure the repository does, so a recipient of the tarball finds the
        // licence text where `NOTICE` says it is. Both copy sites create the parent
        // directory for exactly this entry.
        name: "LICENSES/LicenseRef-Slint-Royalty-free-2.0.md",
        candidates: &["LICENSES/LicenseRef-Slint-Royalty-free-2.0.md"],
        role: "license",
        optional: false,
        budget: None,
        exports: &[],
    },
    Payload {
        name: "org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
        candidates: &["packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml"],
        role: "appstream-metadata",
        optional: false,
        budget: None,
        exports: &[],
    },
];

/// One payload whose build output exists, with the path it is staged to.
#[derive(Debug)]
struct Planned {
    /// File name in the release directory and inside the archive.
    name: &'static str,
    /// What the file is for, as the manifest records it.
    role: &'static str,
    /// Path of the build output the file is taken from.
    source: PathBuf,
    /// Path the file is staged to, inside the release tree.
    path: PathBuf,
    /// The size budget the file is measured against, if it has one.
    budget: Option<Budget>,
    /// Symbols the file must export.
    required_exports: &'static [&'static str],
    /// Symbols the staged file was read back as exporting. Empty until it is staged.
    exports: Vec<String>,
}

/// What a release will ship, and what it will not.
#[derive(Debug)]
struct Plan {
    /// Payloads whose build output exists, in the order they are written.
    payloads: Vec<Planned>,
    /// Payloads the tree does not carry, as `(name, reason)`.
    absent: Vec<(&'static str, String)>,
}

impl Plan {
    /// The payloads as the manifest records them.
    ///
    /// The size threshold each artifact is recorded with comes from the budget document
    /// through [`SizeBudgets`], so the manifest states the same ceiling the gate measured
    /// against rather than a second copy of the number.
    fn artifacts(&self, budgets: &SizeBudgets) -> Vec<Artifact> {
        self.payloads
            .iter()
            .map(|payload| Artifact {
                name: payload.name.to_owned(),
                role: payload.role,
                path: payload.path.clone(),
                budget_mb: payload.budget.map(|budget| budgets.limit(budget)),
                exports: payload.exports.clone(),
            })
            .collect()
    }
}

/// Command-line surface of `xtask package`.
#[derive(Debug, Args)]
pub struct PackageArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Directory the release is written to.
    #[arg(long, default_value = "dist")]
    out: PathBuf,
    /// Architecture the release is labelled with; the artifacts must have been built for
    /// it. `just package` passes the host's, which is what the recipe's build produced.
    #[arg(long, default_value = std::env::consts::ARCH)]
    arch: String,
}

/// Entry point for `xtask package`.
///
/// # Errors
///
/// Returns an error when the addon descriptors disagree with the workspace version, when
/// a payload that is not optional has no build output, when a library cannot be stripped
/// or no longer exports the addon factory symbol, when an artifact is past its size
/// budget, and when the archive, the manifest or the checksum list cannot be written.
pub fn run(args: PackageArgs) -> Result<()> {
    let root = resolve_root(args.root.as_deref())?;
    // Fcitx5 reports the version from the descriptor, not from the library, and a release
    // ships the descriptors: a tree whose descriptors have drifted from the workspace
    // version packages a plugin that mislabels itself. The gate that catches it already
    // exists; running it here is what keeps the two from drifting.
    crate::versions::run()?;

    let sources = Sources {
        staging_dir: root.join("target").join(STAGING_DIR),
        root,
    };
    let budgets = SizeBudgets::load(&sources)?;
    let epoch = stamp::release_epoch();
    let stem = stamp::archive_stem(PACKAGE_VERSION, &args.arch);
    let mut plan = plan_release(&sources, &sources.staging_dir.join(&stem))?;
    collect(&mut plan, StripPolicy::Require)?;
    measure(&plan, &budgets)?;
    // The manifest is built before the release directory is written: every digest in it
    // is read from the staged file that is about to be copied, so a record that cannot be
    // produced stops the release rather than leaving a directory without a manifest.
    let artifacts = plan.artifacts(&budgets);
    let release = Release::capture(PACKAGE_VERSION, &args.arch, &artifacts, epoch)?;
    publish(
        &args.out,
        &sources.staging_dir,
        &stem,
        &plan,
        &release,
        epoch,
    )?;
    report(&release, &args.out, &plan);
    Ok(())
}

/// Resolves every payload the release will ship.
///
/// # Errors
///
/// Returns an error when a payload that is not optional has no build output. A payload
/// that is optional and absent is recorded in [`Plan::absent`] instead, so that the run
/// reports it rather than quietly shipping a release without it.
fn plan_release(sources: &Sources, tree: &Path) -> Result<Plan> {
    let mut payloads = Vec::new();
    let mut absent = Vec::new();
    for payload in PAYLOADS {
        match sources.artifact(payload.candidates) {
            Ok(source) => payloads.push(Planned {
                name: payload.name,
                role: payload.role,
                source,
                path: tree.join(payload.name),
                budget: payload.budget,
                required_exports: payload.exports,
                exports: Vec::new(),
            }),
            Err(error) if payload.optional => absent.push((payload.name, error.to_string())),
            Err(error) => return Err(error),
        }
    }
    Ok(Plan { payloads, absent })
}

/// Stages every planned payload into the release tree.
///
/// A library is copied, stripped and read back before the release carries it; everything
/// else is copied as the build produced it.
///
/// # Errors
///
/// Returns an error when the release tree cannot be created, when a copy fails, and when
/// a library cannot be stripped or does not export the symbols it must.
fn collect(plan: &mut Plan, policy: StripPolicy) -> Result<()> {
    let Some(tree) = plan
        .payloads
        .first()
        .and_then(|payload| payload.path.parent())
    else {
        return Ok(());
    };
    // The tree is rebuilt rather than added to: a file an earlier run left behind would
    // otherwise be archived into this release even though nothing produces it any more.
    if tree.exists() {
        fs::remove_dir_all(tree).with_context(|| format!("clearing {}", tree.display()))?;
    }
    fs::create_dir_all(tree).with_context(|| format!("creating {}", tree.display()))?;
    for payload in &mut plan.payloads {
        if let Some(parent) = payload.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        if payload.required_exports.is_empty() {
            fs::copy(&payload.source, &payload.path).with_context(|| {
                format!(
                    "staging {} at {}",
                    payload.source.display(),
                    payload.path.display()
                )
            })?;
            continue;
        }
        stage_library_file(&payload.source, &payload.path, policy)?;
        payload.exports = verified_exports(&payload.path, payload.required_exports)?;
    }
    Ok(())
}

/// The symbols `path` exports, read back from its dynamic symbol table.
///
/// The manifest's `exports` list is what a consumer can resolve with `dlsym`, so it is
/// read from the file that ships rather than written down beside it: a library that has
/// lost the name fails here, with the delivery-channel code for it, instead of producing
/// a manifest that promises a symbol the archive does not carry.
///
/// # Errors
///
/// Returns an error when the file is not an inspectable ELF image, and when it does not
/// export one of `required`.
fn verified_exports(path: &Path, required: &[&str]) -> Result<Vec<String>> {
    let mut verified = Vec::with_capacity(required.len());
    for symbol in required {
        ensure!(
            elf::exports_path(path, symbol)?,
            "dist/verify/factory-symbol-missing: {} does not export `{symbol}`",
            path.display()
        );
        verified.push((*symbol).to_owned());
    }
    Ok(verified)
}

/// Fails when a staged payload is past its size budget.
///
/// The failure carries `dist/verify/size-budget-exceeded`, the delivery-channel code the
/// contract defines for it, so a release that would ship an oversized artifact is stopped
/// here rather than at the user's disk.
///
/// # Errors
///
/// Returns an error naming the budget, the file and the measured size.
fn measure(plan: &Plan, budgets: &SizeBudgets) -> Result<()> {
    for payload in &plan.payloads {
        let Some(budget) = payload.budget else {
            continue;
        };
        check_size_in("package", budget, &payload.path, budgets.limit(budget))
            .map_err(|error| anyhow::anyhow!("dist/verify/size-budget-exceeded: {error}"))?;
    }
    Ok(())
}

/// Copies the release tree into `out` and writes the three files a release consists of.
///
/// # Errors
///
/// Returns an error when the release directory cannot be created, when a payload cannot
/// be copied, when the archive cannot be written, and when the manifest or the checksum
/// list cannot be written.
fn publish(
    out: &Path,
    staging: &Path,
    stem: &str,
    plan: &Plan,
    release: &Release,
    epoch: i64,
) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let mut shipped = Vec::with_capacity(plan.payloads.len() + 2);
    for payload in &plan.payloads {
        let destination = out.join(payload.name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&payload.path, &destination).with_context(|| {
            format!(
                "copying {} to {}",
                payload.path.display(),
                destination.display()
            )
        })?;
        shipped.push(payload.name.to_owned());
    }

    let archive = write_archive(out, staging, stem, epoch)?;
    release.write(out)?;
    if let Some(name) = archive.file_name() {
        shipped.push(name.to_string_lossy().into_owned());
    }
    shipped.push(manifest::MANIFEST_FILE.to_owned());
    manifest::write_checksums(out, &shipped)?;
    Ok(())
}

/// Writes the release archive from the staged tree.
///
/// The archive holds one top-level directory named after the release, so that unpacking
/// it cannot scatter files across whatever directory the user happened to be in, and the
/// manifest's `artifacts` list names the files inside that directory.
///
/// # Errors
///
/// Returns an error when `tar` is not installed, when it runs and fails, and when the
/// archive cannot be written.
fn write_archive(out: &Path, staging: &Path, stem: &str, epoch: i64) -> Result<PathBuf> {
    let archive = out.join(format!("{stem}.tar.gz"));
    let output = Command::new("tar")
        .arg("--create")
        .arg(format!("--use-compress-program={COMPRESSOR}"))
        .arg("--file")
        .arg(&archive)
        .arg("--directory")
        .arg(staging)
        .arg("--sort=name")
        .arg(format!("--mtime=@{epoch}"))
        .arg("--owner=0")
        .arg("--group=0")
        .arg("--numeric-owner")
        .arg(stem)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => anyhow::bail!(
            "`tar` is not installed, so {} cannot be written. The archive is produced by GNU \
             tar rather than by a crate of this workspace: the flags that make it \
             reproducible are tar's own, and the build environment already requires it.",
            archive.display()
        ),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("running tar to write {}", archive.display()));
        }
    };
    ensure!(
        output.status.success(),
        "tar failed to write {}: {}",
        archive.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(archive)
}

/// Prints what the run produced, and what it did not.
fn report(release: &Release, out: &Path, plan: &Plan) {
    for payload in &plan.payloads {
        println!(
            "package: {} {} ({})",
            payload.role,
            payload.name,
            human_size(fs::metadata(&payload.path).map(|meta| meta.len()).ok())
        );
    }
    for (name, reason) in &plan.absent {
        println!("package: not shipped: {name}: {reason}");
    }
    println!("package: {} {}", release.version, release.request_id);
    println!(
        "package: {} artifacts written to {}",
        release.artifacts.len(),
        out.display()
    );
    println!("package: run `sha256sum --check SHA256SUMS` in that directory to verify it");
}

/// Renders a byte count for the run's log.
fn human_size(bytes: Option<u64>) -> String {
    // Bytes in a mebibyte: the unit the size budgets are stated in.
    const MEBIBYTE: f64 = 1024.0 * 1024.0;
    match bytes {
        Some(bytes) if bytes as f64 >= MEBIBYTE => format!("{:.2}MiB", bytes as f64 / MEBIBYTE),
        Some(bytes) => format!("{:.1}KiB", bytes as f64 / 1024.0),
        None => "unreadable".to_owned(),
    }
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
