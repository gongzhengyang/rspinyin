//! What an install put on the system, and the file operations that change it.
//!
//! Responsibility: read and write the manifest that makes an install reversible,
//! classify a destination before it is overwritten, and run the filesystem operations
//! -- directly, or through `sudo` when the destination belongs to root.
//!
//! # Why a manifest rather than a fixed list of paths
//!
//! Uninstall has to remove exactly what install added. The list of files is fixed and
//! known, but *whether a file was there before* is not: a distribution package or a
//! previous release may have left one at the same path, and deleting it would destroy
//! something this installer never owned. The manifest records, per file, whether the
//! install created it or replaced it and where the displaced copy went, so an
//! uninstall restores instead of deletes. It is written before the first file is
//! copied, so a run interrupted half way is still fully reversible -- and an uninstall
//! that finds no manifest removes nothing at all.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::layout::BACKUP_SUFFIX;

/// Schema version of the manifest this build writes.
pub const MANIFEST_VERSION: u32 = 1;

/// Mode every installed file carries.
///
/// 0644 rather than 0600: these files are system-wide, every other addon under
/// `/usr/share/fcitx5` is world-readable, and Fcitx5 reads them as the user who runs
/// it rather than as root.
pub const FILE_MODE: u32 = 0o644;

/// How a destination file got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryState {
    /// The install created the file; an uninstall removes it.
    Created,
    /// A file was already there; an uninstall puts the displaced copy back.
    Replaced,
}

/// One file an install owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Absolute path of the installed file, with `DESTDIR` already applied.
    pub path: PathBuf,
    /// Whether the install created the file or replaced an existing one.
    pub state: EntryState,
    /// Copy of the file the install displaced, kept next to it. `None` for
    /// [`EntryState::Created`], and for a replacement whose backup was never taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

/// The record of one install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version; a manifest written by a newer installer is refused.
    pub version: u32,
    /// Workspace version that was installed.
    pub package_version: String,
    /// Files the install owns, in the order they were copied.
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// Reads the manifest at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed, and when its
    /// schema version is not [`MANIFEST_VERSION`]. A file that is not there is
    /// `Ok(None)`: it means this installer has never run here.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("{}: invalid install manifest", path.display()))?;
        ensure!(
            manifest.version == MANIFEST_VERSION,
            "{}: install manifest schema version {} is not the {MANIFEST_VERSION} this build \
             writes; upgrade or remove the file by hand",
            path.display(),
            manifest.version
        );
        Ok(Some(manifest))
    }

    /// Writes the manifest, replacing any previous one in one step.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be serialized, or when the temporary
    /// file cannot be written or renamed into place.
    pub fn write(&self, path: &Path) -> Result<()> {
        let text =
            serde_json::to_string_pretty(self).context("serializing the install manifest")?;
        let parent = path
            .parent()
            .with_context(|| format!("{} has no parent directory", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text.as_bytes())
            .with_context(|| format!("writing {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("moving {} to {}", temporary.display(), path.display()))
    }

    /// The entry for `path`, if this manifest already owns it.
    pub fn entry(&self, path: &Path) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.path == path)
    }
}

/// How a filesystem operation reaches a destination the caller cannot write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// Run in this process; the destination is writable as it is.
    Direct,
    /// Run through `sudo`; the destination belongs to another account.
    Sudo,
}

impl Elevation {
    /// Decides how this run will reach its destinations.
    ///
    /// A process that is already root needs no help, and `no_sudo` forbids it -- which
    /// is what a `DESTDIR` staging tree wants, because that tree belongs to the user
    /// running the packaging step.
    ///
    /// # Errors
    ///
    /// Returns an error when the effective user id cannot be read.
    pub fn detect(no_sudo: bool) -> Result<Self> {
        if no_sudo || effective_uid()? == 0 {
            return Ok(Self::Direct);
        }
        Ok(Self::Sudo)
    }

    /// Whether operations will be escalated through `sudo`.
    pub fn is_sudo(self) -> bool {
        matches!(self, Self::Sudo)
    }
}

/// Copies `source` to `destination` with `mode`, creating the directories above it.
///
/// The copy lands in a sibling temporary file that is renamed into place, so an
/// interrupted install never leaves a half-written library where Fcitx5 would `dlopen`
/// it and crash. A destination the caller cannot write is reported with the path and
/// the way out.
///
/// # Errors
///
/// Returns an error when the parent directory cannot be created, when the copy, the
/// mode change or the rename fails, and when `sudo` cannot run the equivalent
/// `install(1)` command.
pub fn place_file(
    elevation: Elevation,
    source: &Path,
    destination: &Path,
    mode: u32,
) -> Result<()> {
    if elevation.is_sudo() {
        let arguments = vec![
            "-D".to_owned(),
            format!("-m{mode:04o}"),
            source.display().to_string(),
            destination.display().to_string(),
        ];
        return run_sudo("install", &arguments);
    }
    copy_directly(source, destination, mode).map_err(|error| {
        if is_permission_denied(&error) {
            error.context(format!(
                "{} is not writable as this user; re-run as root, or let the installer \
                 escalate through `sudo` (do not pass --no-sudo)",
                destination.display()
            ))
        } else {
            error
        }
    })
}

/// Removes `path`, treating a file that is not there as already done.
///
/// # Errors
///
/// Returns an error when the file exists and cannot be removed, and when `sudo` cannot
/// run the equivalent `rm` command.
pub fn remove_file(elevation: Elevation, path: &Path) -> Result<()> {
    if elevation.is_sudo() {
        return run_sudo("rm", &["-f".to_owned(), path.display().to_string()]);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

/// Removes a directory an install created, leaving it alone when it is not empty.
///
/// A directory that still holds something belongs to whoever put it there -- another
/// addon's descriptor, another theme's icons -- and the install never owned it. Under
/// `sudo` the attempt is `rmdir(1)`, whose refusal to remove a non-empty directory is
/// exactly the outcome wanted, so a non-zero exit is reported as "left in place"
/// rather than as a failure.
///
/// # Errors
///
/// Returns an error when the directory is empty and still cannot be removed.
pub fn remove_directory(elevation: Elevation, path: &Path) -> Result<()> {
    if elevation.is_sudo() {
        let status = Command::new("sudo")
            .arg("rmdir")
            .arg(path)
            .status()
            .with_context(|| format!("running `sudo rmdir {}`", path.display()))?;
        if !status.success() && path.exists() {
            println!(
                "uninstall: {} was left in place; it is not empty, or not removable",
                path.display()
            );
        }
        return Ok(());
    }
    match fs::remove_dir(path) {
        Ok(()) => {
            println!("uninstall: removed {}", path.display());
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

/// Refuses to start when a destination directory cannot be written.
///
/// Run before the first file is copied, so that an install which cannot finish does not
/// start at all. Each directory is probed with a temporary file in its nearest existing
/// ancestor, which is the only question that matters -- a directory that does not exist
/// yet is created by whoever can create its parent.
///
/// # Errors
///
/// Returns an error naming the first destination that is not writable, with the command
/// that would clear it.
pub fn check_writable(directories: &[PathBuf]) -> Result<()> {
    for directory in directories {
        probe_writable(directory)?;
    }
    Ok(())
}

/// The path the displaced copy of `destination` is kept at.
pub fn backup_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Copies a file in this process, through a temporary sibling.
fn copy_directly(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let parent = destination
        .parent()
        .with_context(|| format!("{} has no parent directory", destination.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temporary = temporary_path(destination)?;
    let outcome = fs::copy(source, &temporary)
        .and_then(|_| fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)))
        .and_then(|()| fs::rename(&temporary, destination));
    if outcome.is_err() {
        // Best effort: a leftover temporary file must not make the next run fail.
        let _ = fs::remove_file(&temporary);
    }
    outcome.with_context(|| {
        format!(
            "installing {} at {}",
            source.display(),
            destination.display()
        )
    })
}

/// The name a copy is staged under before it replaces its destination.
fn temporary_path(destination: &Path) -> Result<PathBuf> {
    let name = destination
        .file_name()
        .with_context(|| format!("{} has no file name", destination.display()))?;
    Ok(destination.with_file_name(format!(".{}.rspinyin-tmp", name.to_string_lossy())))
}

/// Creates and removes a probe file in the nearest existing ancestor of `directory`.
fn probe_writable(directory: &Path) -> Result<()> {
    let existing = nearest_existing(directory)?;
    let probe = existing.join(format!(".rspinyin-probe-{}", std::process::id()));
    match fs::write(&probe, []) {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "{} is not writable, so the plugin cannot be installed there",
                existing.display()
            )
        }),
    }
}

/// The deepest ancestor of `path` that exists.
///
/// # Errors
///
/// Returns an error when no ancestor exists at all, which means `path` is relative and
/// names nothing on disk.
fn nearest_existing(path: &Path) -> Result<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.exists() {
            return Ok(candidate.to_path_buf());
        }
        candidate = candidate
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .with_context(|| format!("{} has no existing ancestor", path.display()))?;
    }
}

/// Runs one privileged command through `sudo`.
///
/// The child inherits the terminal, so `sudo` can prompt for a password.
///
/// # Errors
///
/// Returns an error when `sudo` cannot be started or exits non-zero; the message names
/// the command, so it can be run by hand to see the underlying failure.
fn run_sudo(program: &str, arguments: &[String]) -> Result<()> {
    let status = Command::new("sudo")
        .arg(program)
        .args(arguments)
        .status()
        .with_context(|| format!("running `sudo {program}`; install sudo, or re-run as root"))?;
    ensure!(
        status.success(),
        "`sudo {program} {}` failed with {status}; run it by hand to see why",
        arguments.join(" ")
    );
    Ok(())
}

/// Reads the effective user id from `/proc/self/status`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, and as [`effective_uid_from`].
fn effective_uid() -> Result<u32> {
    let status = fs::read_to_string("/proc/self/status").context("reading /proc/self/status")?;
    effective_uid_from(&status)
}

/// Extracts the effective user id from the contents of `/proc/self/status`.
///
/// The `Uid:` line holds four ids -- real, effective, saved and filesystem -- and the
/// effective one is what decides whether a write to `/usr` is going to be refused.
///
/// # Errors
///
/// Returns an error when the text has no `Uid:` line, when that line has fewer than
/// three fields, or when the effective field is not a number.
fn effective_uid_from(status: &str) -> Result<u32> {
    let line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .context("/proc/self/status has no Uid: line")?;
    let effective = line
        .split_whitespace()
        .nth(2)
        .context("/proc/self/status has no effective user id")?;
    effective
        .parse()
        .context("the effective user id is not a number")
}

/// Whether any cause of `error` is a permission failure.
fn is_permission_denied(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|cause| cause.kind() == ErrorKind::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rspinyin-install-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// One entry, for the manifest fixtures.
    fn entry(path: &Path, state: EntryState, backup: Option<PathBuf>) -> Entry {
        Entry {
            path: path.to_path_buf(),
            state,
            backup,
        }
    }

    #[test]
    fn test_place_file_writes_the_content_and_the_mode() {
        let dir = scratch("place");
        let source = dir.join("source");
        fs::write(&source, b"payload").expect("writing the fixture");
        let destination = dir.join("nested/deeper/target");

        place_file(Elevation::Direct, &source, &destination, FILE_MODE).expect("placing the file");
        assert_eq!(
            fs::read(&destination).expect("reading back"),
            b"payload".to_vec()
        );
        let mode = fs::metadata(&destination)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, FILE_MODE);
        assert!(
            !temporary_path(&destination)
                .expect("a temporary name")
                .exists(),
            "the staged copy is renamed away"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_place_file_replaces_an_existing_file_and_leaves_no_temporary_behind() {
        let dir = scratch("replace");
        let source = dir.join("source");
        let destination = dir.join("target");
        fs::write(&source, b"new").expect("writing the fixture");
        fs::write(&destination, b"old").expect("writing the fixture");

        place_file(Elevation::Direct, &source, &destination, FILE_MODE).expect("placing the file");
        assert_eq!(
            fs::read(&destination).expect("reading back"),
            b"new".to_vec()
        );
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("listing the scratch directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("rspinyin-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_place_file_reports_a_missing_source() {
        let dir = scratch("missing");
        let failure = place_file(
            Elevation::Direct,
            &dir.join("absent"),
            &dir.join("target"),
            FILE_MODE,
        )
        .expect_err("a missing source must fail");
        assert!(failure.to_string().contains("target"), "{failure}");
        assert!(
            !dir.join("target").exists(),
            "nothing is installed from a failed copy"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_remove_file_and_remove_directory_treat_absence_as_done() {
        let dir = scratch("remove");
        let file = dir.join("file");
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).expect("creating the fixture");
        fs::write(&file, b"x").expect("writing the fixture");

        remove_file(Elevation::Direct, &file).expect("removing a file that is there");
        remove_file(Elevation::Direct, &file).expect("removing a file that is not");
        remove_directory(Elevation::Direct, &nested).expect("removing an empty directory");
        remove_directory(Elevation::Direct, &nested).expect("removing a directory that is not");
        assert!(!file.exists() && !nested.exists());
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_remove_directory_leaves_a_directory_that_is_not_empty() {
        let dir = scratch("nonempty");
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).expect("creating the fixture");
        fs::write(nested.join("someone-elses-file"), b"x").expect("writing the fixture");

        remove_directory(Elevation::Direct, &nested)
            .expect("a non-empty directory is not an error");
        assert!(
            nested.join("someone-elses-file").exists(),
            "the other file must survive"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_backup_path_sits_next_to_the_file_it_copies() {
        assert_eq!(
            backup_path(Path::new("/usr/share/fcitx5/addon/rspinyin.conf")),
            PathBuf::from("/usr/share/fcitx5/addon/rspinyin.conf.rspinyin-bak")
        );
    }

    #[test]
    fn test_manifest_round_trips_through_json() {
        let dir = scratch("manifest");
        let path = dir.join("nested/install-manifest.json");
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            package_version: "0.1.0".to_owned(),
            entries: vec![
                entry(
                    Path::new("/usr/lib/fcitx5/librspinyin.so"),
                    EntryState::Created,
                    None,
                ),
                entry(
                    Path::new("/usr/share/fcitx5/addon/rspinyin.conf"),
                    EntryState::Replaced,
                    Some(PathBuf::from(
                        "/usr/share/fcitx5/addon/rspinyin.conf.rspinyin-bak",
                    )),
                ),
            ],
        };

        manifest.write(&path).expect("writing the manifest");
        let read = Manifest::read(&path)
            .expect("reading the manifest")
            .expect("the manifest exists");
        assert_eq!(read, manifest);
        assert_eq!(
            read.entry(Path::new("/usr/lib/fcitx5/librspinyin.so"))
                .expect("the entry is found")
                .state,
            EntryState::Created
        );
        assert!(read.entry(Path::new("/nowhere")).is_none());
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_manifest_read_reports_absence_and_refuses_a_newer_schema() {
        let dir = scratch("schema");
        let path = dir.join("install-manifest.json");
        assert!(
            Manifest::read(&path)
                .expect("absence is not an error")
                .is_none()
        );

        fs::write(
            &path,
            r#"{"version":99,"package_version":"0.1.0","entries":[]}"#,
        )
        .expect("writing the fixture");
        let failure = Manifest::read(&path).expect_err("a newer schema is refused");
        assert!(failure.to_string().contains("99"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_writable_accepts_a_scratch_directory_and_reports_a_missing_ancestor() {
        let dir = scratch("writable");
        check_writable(&[dir.join("not-created-yet"), dir.clone()])
            .expect("a scratch directory is writable");

        let failure = check_writable(&[PathBuf::from("rspinyin-nowhere/nowhere")])
            .expect_err("a relative path has no ancestor to probe");
        assert!(failure.to_string().contains("ancestor"), "{failure}");

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("listing the scratch directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("rspinyin-probe"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_nearest_existing_walks_up_to_the_first_directory_that_is_there() {
        let dir = scratch("nearest");
        assert_eq!(
            nearest_existing(&dir.join("a/b/c")).expect("the scratch directory exists"),
            dir
        );
        assert_eq!(
            nearest_existing(&dir).expect("the scratch directory exists"),
            dir
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_elevation_detect_honours_the_no_sudo_flag() {
        // The flag has to win regardless of which user runs the tests, which is the
        // property a `DESTDIR` staging step depends on.
        assert_eq!(
            Elevation::detect(true).expect("the effective user id is readable"),
            Elevation::Direct
        );
        assert!(!Elevation::Direct.is_sudo());
        assert!(Elevation::Sudo.is_sudo());
    }

    #[test]
    fn test_effective_uid_from_reads_the_effective_field() {
        // The `Uid:` line is `real effective saved filesystem`; reading the wrong field
        // would escalate on a setuid invocation that needs no escalation.
        assert_eq!(
            effective_uid_from("Name:\tcat\nUid:\t1000\t1001\t1002\t1003\n").expect("a full line"),
            1001
        );
    }

    #[test]
    fn test_effective_uid_from_rejects_text_it_cannot_read() {
        assert!(effective_uid_from("Name:\tcat\n").is_err(), "no Uid: line");
        assert!(
            effective_uid_from("Uid:\t1000\n").is_err(),
            "too few fields"
        );
        assert!(
            effective_uid_from("Uid:\t1000\troot\t1000\t1000\n").is_err(),
            "the effective field is not a number"
        );
    }
}
