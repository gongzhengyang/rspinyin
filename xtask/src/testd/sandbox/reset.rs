//! What the sandbox writes into its own tree: staging the plugin, keeping the dictionary
//! copy honest, and the reset that takes mutable state away between cases.
//!
//! Responsibility: turn a case's files into a sandbox that can be run, and turn a sandbox
//! that has been run back into one that can be run again. It owns every path under the
//! root that the harness writes, and the naming discipline those writes follow.
//!
//! Boundaries: it never starts a process and never reads a session's state -- that is
//! [`super::session`]. It does not decide what a case is allowed to do, and it never
//! touches a path outside the root: every write goes through `Sandbox::inside`, which is
//! also what refuses a symbolic link.
//!
//! # Why a reset renames instead of deleting
//!
//! The plugin's own recovery renames a damaged file to `<name>.corrupt.<stamp>` and never
//! destroys it, and the destination name is claimed exclusively before the rename so that
//! a second corruption cannot overwrite the first one's file. A reset keeps the same
//! posture under a marker of its own, `<name>.reset.<n>`: a case that reset away the very
//! evidence of its own failure can still be diagnosed, and the counter rather than a
//! timestamp keeps the names deterministic, because a test may not read the clock.

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{DIR_MODE, Sandbox, SandboxError};

/// Marker a reset artifact's name carries: `user.redb` becomes `user.redb.reset.<n>`.
pub const RESET_MARK: &str = ".reset.";

/// Number of `.n` suffixes tried before a reset reports the name space as exhausted.
///
/// A hundred resets of one file inside one case directory is not a case worth more code,
/// and running out is reported rather than resolved by overwriting anything.
const MAX_RESET_SUFFIX: u32 = 99;

/// Mode the momentary name reservation carries, before the rename replaces it.
const CLAIM_MODE: u32 = 0o600;

/// What one reset moved aside.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResetReport {
    /// The paths that were renamed, in the order they were moved.
    pub moved: Vec<PathBuf>,
    /// Whether the dictionary copy had to be rebuilt from the pristine source.
    pub dict_restored: bool,
}

impl Sandbox {
    /// Resets the sandbox's mutable state.
    ///
    /// The user store, the configuration file, the takeover record and the runtime mirrors
    /// are renamed aside; the dictionary copy is rebuilt from the pristine source when it
    /// no longer hashes to it. Resetting a sandbox that is already clean moves nothing and
    /// writes nothing.
    ///
    /// # Errors
    /// Returns [`SandboxError::OutsideRoot`] or [`SandboxError::Symlink`] when a path the
    /// reset would move is not safely inside the sandbox, and [`SandboxError::Io`] when a
    /// rename or a copy fails.
    pub fn reset(&mut self) -> Result<ResetReport, SandboxError> {
        let mut report = ResetReport::default();
        for target in self.mutable_paths() {
            if let Some(moved) = self.move_aside(&target)? {
                report.moved.push(moved);
            }
        }
        self.clear_mirrors(&mut report)?;
        report.dict_restored = self.restore_dict()?;
        Ok(report)
    }

    /// The paths a reset takes away, in the order it takes them.
    ///
    /// The takeover record is on the list although the plugin's layout does not create it:
    /// it records which host configuration the plugin replaced, so a case that leaves it
    /// behind leaves state behind. The configuration file is on the list for the same
    /// reason in reverse -- the plugin never creates one, so a file that is there is a file
    /// a case put there, and the default configuration is the absent file.
    fn mutable_paths(&self) -> [PathBuf; 3] {
        [
            self.paths.user_db.clone(),
            self.paths.config_file.clone(),
            self.paths.takeover.clone(),
        ]
    }

    /// Moves every mirror entry aside, leaving the mirror directory itself in place.
    fn clear_mirrors(&self, report: &mut ResetReport) -> Result<(), SandboxError> {
        let mirror = self.mirror_dir();
        self.resolve(&mirror)?;
        create_private_dir(&mirror)?;
        for entry in entries_of(&mirror)? {
            // A reset artifact is left where it is: moving it again would only rename the
            // previous reset's evidence out of the way of the current one.
            if entry.to_string_lossy().contains(RESET_MARK) {
                continue;
            }
            if let Some(moved) = self.move_aside(&entry)? {
                report.moved.push(moved);
            }
        }
        Ok(())
    }

    /// Renames `target` to `<name>.reset.<n>` and returns where it went.
    ///
    /// # Return value
    /// `None` when `target` does not exist, which is what makes a reset of a clean sandbox
    /// a no-op rather than a failure.
    ///
    /// # Errors
    /// Returns [`SandboxError::Symlink`] for a link rather than following it,
    /// [`SandboxError::OutsideRoot`] for a path outside the sandbox, and
    /// [`SandboxError::ResetNameExhausted`] when every candidate name is taken.
    fn move_aside(&self, target: &Path) -> Result<Option<PathBuf>, SandboxError> {
        let target = self.resolve(target)?;
        let is_dir = match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(SandboxError::Io {
                    path: target,
                    source,
                });
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(SandboxError::Symlink { path: target });
            }
            Ok(meta) => meta.is_dir(),
        };
        let name = target
            .file_name()
            .ok_or_else(|| SandboxError::UnusablePath {
                path: target.clone(),
            })?;
        for suffix in 0..=MAX_RESET_SUFFIX {
            let mut aside = name.to_os_string();
            aside.push(RESET_MARK);
            aside.push(suffix.to_string());
            let candidate = target.with_file_name(aside);
            match claim(&candidate, is_dir) {
                Ok(()) => {
                    fs::rename(&target, &candidate).map_err(|source| SandboxError::Io {
                        path: target,
                        source,
                    })?;
                    return Ok(Some(candidate));
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(SandboxError::Io {
                        path: candidate,
                        source,
                    });
                }
            }
        }
        Err(SandboxError::ResetNameExhausted {
            path: target.clone(),
        })
    }

    /// Rebuilds the dictionary copy when it no longer matches the pristine source.
    ///
    /// # Return value
    /// Whether the copy had to be rebuilt. A copy that already hashes to the source is
    /// left exactly as it is, so the dictionary is never rewritten for no reason.
    ///
    /// # Errors
    /// Returns [`SandboxError::DictMismatch`] when the rebuilt copy still does not hash to
    /// the source, which means the source changed underneath the reset.
    fn restore_dict(&self) -> Result<bool, SandboxError> {
        let Some(source) = self.dict_source.as_ref() else {
            return Ok(false);
        };
        if self.dict_matches_source()? {
            return Ok(false);
        }
        let copy = self.dict_path();
        self.move_aside(&copy)?;
        copy_file(source, &copy)?;
        if !self.dict_matches_source()? {
            return Err(SandboxError::DictMismatch {
                copy,
                pristine: source.clone(),
            });
        }
        Ok(true)
    }

    /// Whether the sandbox's dictionary copy is byte-identical to the pristine source.
    ///
    /// # Errors
    /// Returns [`SandboxError::Io`] when either file cannot be read.
    pub fn dict_matches_source(&self) -> Result<bool, SandboxError> {
        let Some(source) = self.dict_source.as_ref() else {
            return Ok(false);
        };
        let copy = self.dict_path();
        if !copy.is_file() {
            return Ok(false);
        }
        Ok(sha256_file(&copy)? == sha256_file(source)?)
    }

    /// Stages the plugin's descriptors and libraries inside the sandbox.
    ///
    /// Reads `packaging_dir` for the Fcitx5 descriptors, resolves the library each addon
    /// descriptor names against `library_dir`, copies both into the sandbox, and records
    /// the addon ids a restarted session must report. Deriving the library names from the
    /// descriptors rather than naming them here is what keeps the sandbox from having its
    /// own opinion about what the packaging contains.
    ///
    /// # Return value
    /// The addon ids that were staged, in the order their descriptors were read.
    ///
    /// # Errors
    /// Returns [`SandboxError::UnknownDescriptor`] for a `.conf` whose first section is
    /// neither `[Addon]` nor `[InputMethod]`, [`SandboxError::AddonLibraryMissing`] when a
    /// descriptor names a library the build did not produce, and [`SandboxError::Io`] when
    /// a file cannot be read or copied.
    pub fn stage_plugin(
        &mut self,
        packaging_dir: &Path,
        library_dir: &Path,
    ) -> Result<Vec<String>, SandboxError> {
        let mut addons = Vec::new();
        for descriptor in files_with_extension(packaging_dir, "conf")? {
            let text = read_text(&descriptor)?;
            let stem = file_stem(&descriptor)?;
            match section_kind(&text) {
                Some(DescriptorKind::Addon) => {
                    self.stage_addon(&descriptor, &text, library_dir, &stem)?;
                    addons.push(stem);
                }
                Some(DescriptorKind::InputMethod) => {
                    let target = self
                        .share_home
                        .join("fcitx5/inputmethod")
                        .join(format!("{stem}.conf"));
                    copy_file(&descriptor, &target)?;
                }
                None => return Err(SandboxError::UnknownDescriptor { path: descriptor }),
            }
        }
        if addons.is_empty() {
            return Err(SandboxError::NoAddonStaged {
                dir: packaging_dir.to_path_buf(),
            });
        }
        self.required = addons.clone();
        Ok(addons)
    }

    /// Copies one addon's descriptor and the library it names into the sandbox.
    fn stage_addon(
        &self,
        descriptor: &Path,
        text: &str,
        library_dir: &Path,
        stem: &str,
    ) -> Result<(), SandboxError> {
        let library = library_name(text).unwrap_or_else(|| stem.to_owned());
        let built = library_dir.join(format!("{library}.so"));
        if !built.is_file() {
            return Err(SandboxError::AddonLibraryMissing {
                path: built,
                descriptor: descriptor.to_path_buf(),
            });
        }
        let target = self.addon_dir.join(format!("{library}.so"));
        self.resolve(&target)?;
        copy_file(&built, &target)?;
        let conf = self
            .share_home
            .join("fcitx5/addon")
            .join(format!("{stem}.conf"));
        copy_file(descriptor, &conf)
    }
}

/// Seeds the dictionary copy from `source` and verifies it.
///
/// Called once at creation, where the copy cannot be right yet: a fresh sandbox has none,
/// and a tree an earlier run left behind may hold one that run mutated.
pub(super) fn restore_root_dict(sandbox: &Sandbox, source: &Path) -> Result<(), SandboxError> {
    sandbox.restore_dict()?;
    if sandbox.dict_matches_source()? {
        return Ok(());
    }
    Err(SandboxError::DictMismatch {
        copy: sandbox.dict_path(),
        pristine: source.to_path_buf(),
    })
}

/// What a Fcitx5 descriptor's first section declares it to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DescriptorKind {
    /// An addon descriptor, read from `<datadir>/fcitx5/addon`.
    Addon,
    /// An input-method descriptor, read from `<datadir>/fcitx5/inputmethod`.
    InputMethod,
}

/// Reads the kind from a descriptor's first section header.
///
/// The first section is what decides, because a descriptor carries further sections --
/// `[Addon/Dependencies]`, `[Dependencies]` -- that say nothing about which directory the
/// file belongs in.
fn section_kind(text: &str) -> Option<DescriptorKind> {
    let header = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('['))?;
    match header {
        "[Addon]" => Some(DescriptorKind::Addon),
        "[InputMethod]" => Some(DescriptorKind::InputMethod),
        _ => None,
    }
}

/// Reads the library a descriptor names, without the `export:` prefix Fcitx5 allows.
fn library_name(text: &str) -> Option<String> {
    let value = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Library="))?;
    Some(
        value
            .strip_prefix("export:")
            .unwrap_or(value)
            .trim()
            .to_owned(),
    )
}

/// Creates `path` and its parents with the mode the plugin uses for its own directories.
pub(super) fn create_private_dir(path: &Path) -> Result<(), SandboxError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)
        .map_err(|source| SandboxError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Reserves `path` by creating it, so that a second writer cannot take the same name.
///
/// A directory is reserved by creating a directory, because a file cannot stand in for one
/// in the rename that follows.
fn claim(path: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        return fs::create_dir(path);
    }
    let reservation = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(CLAIM_MODE)
        .open(path)?;
    // Closed at once: the name is what is being reserved, and the descriptor would
    // otherwise be open across the rename that replaces this very file.
    drop(reservation);
    Ok(())
}

/// Copies `source` onto `target`, creating the parent directory when it is missing.
pub(super) fn copy_file(source: &Path, target: &Path) -> Result<(), SandboxError> {
    if let Some(parent) = target.parent() {
        create_private_dir(parent)?;
    }
    fs::copy(source, target).map_err(|source| SandboxError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// The lowercase hexadecimal SHA-256 of a file's contents.
///
/// # Errors
/// Returns [`SandboxError::Io`] when the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<String, SandboxError> {
    let bytes = fs::read(path).map_err(|source| SandboxError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex(&Sha256::digest(&bytes)))
}

/// The lowercase hexadecimal of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every file in `dir` with the given extension, ordered by full path.
pub(super) fn files_with_extension(
    dir: &Path,
    extension: &str,
) -> Result<Vec<PathBuf>, SandboxError> {
    let mut paths = entries_of(dir)?;
    paths.retain(|path| path.extension() == Some(OsStr::new(extension)));
    Ok(paths)
}

/// Every entry directly inside `dir`, ordered by full path.
fn entries_of(dir: &Path) -> Result<Vec<PathBuf>, SandboxError> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|source| SandboxError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SandboxError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    paths.sort();
    Ok(paths)
}

/// The file name of `path` without its extension.
fn file_stem(path: &Path) -> Result<String, SandboxError> {
    path.file_stem()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| SandboxError::UnusablePath {
            path: path.to_path_buf(),
        })
}

/// Reads `path` as UTF-8 text.
fn read_text(path: &Path) -> Result<String, SandboxError> {
    fs::read_to_string(path).map_err(|source| SandboxError::Io {
        path: path.to_path_buf(),
        source,
    })
}
