//! Staging an addon library: a stripped copy whose factory symbol is still exported.
//!
//! Responsibility: copy one library into the staging directory, strip the copy, and read
//! the copy's dynamic symbol table back to confirm Fcitx5 would find an addon in it. It
//! also holds the pass over a whole plan that does this for every library the plan names.
//!
//! Both commands that install or ship a library go through this module, because both have
//! to hand the next step a copy rather than the build tree's own file: `strip` rewrites
//! its argument in place, and an artifact a later step can no longer inspect is a worse
//! trade than a copy. Neither command may ship a library Fcitx5 would load and find no
//! addon in, so the check is made on the file that is about to leave the repository.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};

use super::FACTORY_SYMBOL;
use super::elf;
use super::layout::Sources;
use super::payload::PlannedFile;

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
pub(super) fn stage_library(sources: &Sources, plan: &mut [PlannedFile]) -> Result<()> {
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
pub(super) fn verify_factory_symbol(path: &Path) -> Result<()> {
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
