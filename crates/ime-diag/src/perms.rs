//! Private modes for the files the diagnostics layer writes.
//!
//! Responsibility: create and narrow the log files, the crash records and the directories
//! that hold them, so that nothing this plugin writes is readable by another account.
//! Directories are [`DIR_MODE`] and files are [`FILE_MODE`], set through `mkdir(2)` and
//! `open(2)` rather than by a `chmod` afterwards, because a file that exists for even one
//! syscall with the umask's default of `0644` is a file another process can already have
//! opened.
//!
//! Boundaries: this is the diagnostics half of the permission baseline; the layout half is
//! `ime-dict::paths`, which resolves the XDG directories and prepares them. The layer order
//! (`ime-types <- ime-core <- ime-dict <- ime-config <- ime-ui <- ime-fcitx5`) keeps
//! `ime-dict` from seeing this leaf crate, so the modes and the diagnostic codes are stated
//! in both files and have to be changed together.
//!
//! Errors are `std::io::Error`: what an unwritable log directory means is the caller's
//! decision -- the logger degrades to `stderr` -- and this module has no policy to impose.

use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Mode every directory this module creates carries.
pub const DIR_MODE: u32 = 0o700;
/// Mode every file this module creates carries.
pub const FILE_MODE: u32 = 0o600;
/// Diagnostic code reported when a mode had to be narrowed.
pub const PERMS_FIXED_CODE: &str = "data/perms/fixed";
/// Diagnostic code reported when a path is a symbolic link.
pub const PATH_SYMLINK_CODE: &str = "data/path/symlink";

/// Opens `path` for reading and writing, creating it private when it is new.
///
/// The mode is passed to `open(2)` rather than applied afterwards, so the file never exists
/// -- not even for one syscall -- with the umask's default of `0644`. `open` applies the
/// mode only when it creates the file, so a file that is already there is narrowed through
/// the descriptor that was just opened: the same inode the caller will write to, and not a
/// second lookup of the path that could be raced.
///
/// The path is followed, so a symbolic link here opens its target. A caller that must not
/// write through a link checks the path first with [`first_symlink`].
///
/// # Errors
///
/// Returns the underlying `std::io::Error` when the file cannot be opened or its mode cannot
/// be set.
pub fn create_private(path: &Path) -> std::io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(FILE_MODE)
        .open(path)?;
    if file.metadata()?.permissions().mode() & 0o777 != FILE_MODE {
        file.set_permissions(std::fs::Permissions::from_mode(FILE_MODE))?;
    }
    Ok(file)
}

/// Brings `path` down to `mode` when another account can still reach it.
///
/// The test is "does group or other hold a bit", not "is the mode exactly `mode`": a mode
/// the user narrowed on purpose -- a read-only file, a directory without the write bit -- is
/// left alone, while what this baseline promises is that nothing the plugin creates or
/// adopts is reachable by a second account.
///
/// Returns the mode `path` had before it was narrowed, so that the caller reports
/// [`PERMS_FIXED_CODE`] for it; `None` means the mode was already private.
///
/// # Errors
///
/// Returns the underlying `std::io::Error` when the metadata cannot be read or the mode
/// cannot be set.
pub fn tighten(path: &Path, mode: u32) -> std::io::Result<Option<u32>> {
    let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if current & 0o077 == 0 {
        return Ok(None);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(Some(current))
}

/// Prepares `dir` as a private directory below the trusted `root`.
///
/// The directory is created with [`DIR_MODE`] in the `mkdir` call itself when it is missing,
/// and narrowed when it already exists. A symbolic link anywhere in `dir` below `root` is
/// refused rather than followed: a log directory that points somewhere else would put the
/// plugin's files where the plugin did not choose them.
///
/// Returns the mode an existing directory had before it was narrowed, so that the caller
/// reports [`PERMS_FIXED_CODE`] for it; `None` means the directory was created private or
/// was already private.
///
/// # Errors
///
/// Returns an error of kind [`ErrorKind::InvalidInput`] when a component of `dir` below
/// `root` is a symbolic link (the caller reports [`PATH_SYMLINK_CODE`], and [`first_symlink`]
/// names the component), of kind [`ErrorKind::AlreadyExists`] when `dir` exists and is not a
/// directory, and the underlying `std::io::Error` when the directory cannot be created or
/// its mode cannot be set.
pub fn ensure_private_dir(root: &Path, dir: &Path) -> std::io::Result<Option<u32>> {
    if let Some(link) = first_symlink(root, dir) {
        let detail = format!("{} is a symbolic link", link.display());
        return Err(std::io::Error::new(ErrorKind::InvalidInput, detail));
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => tighten(dir, DIR_MODE),
        Ok(_) => Err(not_a_directory()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(DIR_MODE).create(dir)?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The error reported when a path that has to be a directory is something else.
fn not_a_directory() -> std::io::Error {
    std::io::Error::new(ErrorKind::AlreadyExists, "the path is not a directory")
}

/// Returns the first symbolic link among the components of `target` below `root`.
///
/// `root` is the trusted base the caller owns; the components below it are the ones this
/// plugin creates, and a link among them is a path another account could have placed. A
/// component that cannot be read counts as "no link": the step that creates or opens the
/// path reports that failure with the real error.
pub fn first_symlink(root: &Path, target: &Path) -> Option<PathBuf> {
    let relative = target.strip_prefix(root).ok()?;
    let mut prefix = root.to_path_buf();
    for component in relative.components() {
        prefix.push(component);
        let Ok(meta) = std::fs::symlink_metadata(&prefix) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            return Some(prefix);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A per-test root under the system temp directory.
    fn temp_root(label: &str) -> PathBuf {
        let name = format!("rspinyin-perms-{}-{label}", std::process::id());
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("creating the test root");
        root
    }

    /// The permission bits of `path`.
    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).expect("mode").permissions().mode() & 0o777
    }

    /// Sets the permission bits of `path`.
    fn set_mode(path: &Path, mode: u32) {
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, permissions).expect("setting the mode");
    }

    #[test]
    fn test_create_private_creates_a_file_with_the_owner_only_mode() {
        let root = temp_root("create");
        let file = root.join("rspinyin.log");
        let _handle = create_private(&file).expect("creating the log file");
        assert_eq!(mode_of(&file), FILE_MODE, "created private, not narrowed later");
    }

    #[test]
    fn test_create_private_narrows_an_existing_file() {
        let root = temp_root("create-existing");
        let file = root.join("rspinyin.log");
        std::fs::write(&file, b"first line").expect("placing the log file");
        set_mode(&file, 0o644);

        let _handle = create_private(&file).expect("opening the log file");
        assert_eq!(mode_of(&file), FILE_MODE, "an existing log is narrowed");
        let text = std::fs::read(&file).expect("reading the log file");
        assert_eq!(text, b"first line", "opening a log does not truncate it");
    }

    #[test]
    fn test_tighten_reports_the_mode_it_replaced() {
        let root = temp_root("tighten");
        let file = root.join("rspinyin.log");
        std::fs::write(&file, b"x").expect("placing the log file");
        set_mode(&file, 0o664);

        let previous = tighten(&file, FILE_MODE).expect("narrowing the log file");
        assert_eq!(previous, Some(0o664), "the caller needs the mode to report it");
        assert_eq!(mode_of(&file), FILE_MODE);
    }

    #[test]
    fn test_tighten_leaves_a_private_mode_alone() {
        let root = temp_root("tighten-private");
        let file = root.join("rspinyin.log");
        std::fs::write(&file, b"x").expect("placing the log file");
        set_mode(&file, 0o400);

        let previous = tighten(&file, FILE_MODE).expect("inspecting the log file");
        assert_eq!(previous, None, "a mode the user narrowed is left alone");
        assert_eq!(mode_of(&file), 0o400, "the file keeps the mode it had");
    }

    #[test]
    fn test_tighten_reports_a_missing_path() {
        let root = temp_root("tighten-missing");
        let error = tighten(&root.join("absent.log"), FILE_MODE).expect_err("no metadata");
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn test_ensure_private_dir_creates_the_directory_private() {
        let root = temp_root("dir-create");
        let dir = root.join("logs");
        let previous = ensure_private_dir(&root, &dir).expect("creating the log directory");
        assert_eq!(previous, None, "a fresh directory has no mode to report");
        assert_eq!(mode_of(&dir), DIR_MODE, "created private");
    }

    #[test]
    fn test_ensure_private_dir_narrows_an_existing_directory() {
        let root = temp_root("dir-narrow");
        let dir = root.join("logs");
        std::fs::create_dir_all(&dir).expect("creating the log directory");
        set_mode(&dir, 0o755);

        let previous = ensure_private_dir(&root, &dir).expect("preparing the log directory");
        assert_eq!(previous, Some(0o755), "the caller reports the mode it replaced");
        assert_eq!(mode_of(&dir), DIR_MODE);
    }

    #[test]
    fn test_ensure_private_dir_refuses_a_symlinked_component() {
        let root = temp_root("dir-symlink");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("creating the link target");
        let dir = root.join("logs");
        symlink(&elsewhere, &dir).expect("placing the link");

        let error = ensure_private_dir(&root, &dir).expect_err("a link is refused");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(
            first_symlink(&root, &dir),
            Some(dir.clone()),
            "the caller reports the component it refused"
        );
        assert!(elsewhere.is_dir(), "the link target is left untouched");
    }

    #[test]
    fn test_ensure_private_dir_reports_a_path_that_is_not_a_directory() {
        let root = temp_root("dir-file");
        let dir = root.join("logs");
        std::fs::write(&dir, b"not a directory").expect("taking the directory's place");

        let error = ensure_private_dir(&root, &dir).expect_err("a file is not a directory");
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    }

    #[test]
    fn test_first_symlink_accepts_a_tree_without_links() {
        let root = temp_root("walk");
        let nested = root.join("logs").join("archive");
        std::fs::create_dir_all(&nested).expect("creating the nested directory");
        assert_eq!(
            first_symlink(&root, &nested),
            None,
            "a directory tree with no link in it is accepted"
        );
    }
}
