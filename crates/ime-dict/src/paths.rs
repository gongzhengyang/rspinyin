//! Where the plugin keeps what it owns, and how private that is.
//!
//! Responsibility: resolve the XDG base directories into this project's layout, create the
//! directories it owns with mode [`DIR_MODE`], and hand out the paths of the files inside
//! them. It is the first line of the "user data does not leak" promise: every path a caller
//! can obtain from [`Paths`] is either created private or refused.
//!
//! Boundaries: this module knows about directories, modes and links, and nothing else. It
//! does not read the configuration and does not open the user database -- `user_db` creates
//! its own file private -- and it does not log, because this crate has no logger. What it
//! detects is carried out as [`Notice`] values, which the caller reports with the codes
//! [`PERMS_FIXED_CODE`], [`PATH_SYMLINK_CODE`] and [`READONLY_CODE`].
//!
//! ```text
//! $XDG_CONFIG_HOME/rspinyin   0700   config.toml       0600
//! $XDG_DATA_HOME/rspinyin     0700   user.redb         0600
//!                                    ui_takeover.json  0600
//!   logs                      0700
//!   crash                     0700
//! ```
//!
//! Read-only degradation (`ASM-15`): a step that fails -- a directory that cannot be
//! created, a mode that cannot be narrowed, a link where a directory belongs -- does not
//! fail the plugin. The layout is still returned, [`Paths::is_readonly`] and
//! [`is_readonly_mode`] turn true, and the caller stops writing: input keeps working from
//! the read-only dictionary, learning is off, and the status strip renders its lock. Only
//! the failures that leave no layout to hand out -- a base directory that cannot be
//! resolved, a path past [`MAX_PATH_BYTES`] -- return [`ImeError::DataReadonly`].
//!
//! The base directories are never re-moded: they hold every other application's data, and
//! narrowing them would be changing a permission the user chose. A link *inside* the
//! subtree is refused, because a base directory is a place another account could pre-place
//! one; a base directory that is itself a link is accepted, because keeping a home
//! directory on another volume is a legitimate setup rather than an attack.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ime_types::ImeError;

/// Mode every directory this module creates carries.
pub const DIR_MODE: u32 = 0o700;
/// Mode every file the layout owns carries.
pub const FILE_MODE: u32 = 0o600;
/// Length in bytes past which a layout path is refused rather than attempted.
///
/// Linux counts the terminating NUL inside `PATH_MAX`, so a path of this many bytes is
/// already one the kernel would reject.
pub const MAX_PATH_BYTES: usize = 4096;
/// Diagnostic code reported when a mode the layout owns had to be narrowed.
pub const PERMS_FIXED_CODE: &str = "data/perms/fixed";
/// Diagnostic code reported when a path the layout owns is a symbolic link.
pub const PATH_SYMLINK_CODE: &str = "data/path/symlink";
/// Diagnostic code reported when a step failed and the plugin stopped writing.
pub const READONLY_CODE: &str = "data/readonly-mode";

/// Directory the layout adds to each base directory.
const PROGRAM_DIR: &str = "rspinyin";
/// Configuration file inside the configuration directory.
const CONFIG_FILE: &str = "config.toml";
/// User frequency store inside the data directory.
const USER_DB_FILE: &str = "user.redb";
/// Log directory inside the data directory.
const LOG_SUBDIR: &str = "logs";
/// Crash-record directory inside the data directory.
const CRASH_SUBDIR: &str = "crash";
/// Saved host configuration inside the data directory.
const TAKEOVER_FILE: &str = "ui_takeover.json";
/// Detail both link refusals carry.
const LINK_DETAIL: &str = "a component of the path is a symbolic link";

/// Set once any preparation step has failed; read by [`is_readonly_mode`].
static READONLY_MODE: AtomicBool = AtomicBool::new(false);

/// The XDG base directories the layout is derived from.
///
/// Injected rather than read inside the layout, so that a test can name the directories it
/// owns and a caller that already resolved them does not pay for a second lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseDirs {
    /// `$XDG_CONFIG_HOME`, or `$HOME/.config`.
    pub config_home: PathBuf,
    /// `$XDG_DATA_HOME`, or `$HOME/.local/share`.
    pub data_home: PathBuf,
}

impl BaseDirs {
    /// Resolves the base directories from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::DataReadonly`] when neither an absolute `XDG_*` variable nor an
    /// absolute `$HOME` is set, and sets the process-wide read-only flag with it.
    pub fn from_env() -> Result<Self, ImeError> {
        // A closure rather than the function item: `var_os` is generic over its key type, so
        // the item cannot coerce to the higher-ranked `for<'a> Fn(&'a str)` that
        // `from_lookup` needs.
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Resolves the base directories through `lookup`, which a test supplies so that the
    /// rules below are exercised without an environment to arrange.
    ///
    /// # Errors
    ///
    /// As [`BaseDirs::from_env`].
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, ImeError> {
        let home = lookup("HOME").filter(|value| !value.is_empty());
        Ok(Self {
            config_home: base_dir(&lookup, "XDG_CONFIG_HOME", home.as_deref(), ".config")?,
            data_home: base_dir(&lookup, "XDG_DATA_HOME", home.as_deref(), ".local/share")?,
        })
    }
}

/// Resolves one base directory: the `XDG_*` variable when it holds an absolute path, and
/// otherwise `$HOME` with `fallback` appended. An empty or relative value is ignored, as the
/// XDG specification requires and because a relative base would put the user's data under
/// whatever working directory the host process happened to start in.
///
/// # Errors
///
/// Returns [`ImeError::DataReadonly`] when the variable is unusable and no absolute `$HOME`
/// is set.
fn base_dir(
    lookup: &impl Fn(&str) -> Option<OsString>,
    variable: &str,
    home: Option<&OsStr>,
    fallback: &str,
) -> Result<PathBuf, ImeError> {
    if let Some(value) = lookup(variable) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    match home {
        Some(home) if Path::new(home).is_absolute() => Ok(Path::new(home).join(fallback)),
        _ => Err(readonly_error(format!(
            "neither {variable} nor HOME names an absolute directory"
        ))),
    }
}

/// One diagnostic the path layer raised while preparing the layout.
///
/// This crate has no logger, so a condition worth reporting is carried out of the module as
/// data and logged by the caller -- the shape `user_db` uses for its slow-disk code. A
/// notice is developer-facing, and never contains user input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// Stable `domain/action/reason` code: one of [`PERMS_FIXED_CODE`],
    /// [`PATH_SYMLINK_CODE`] or [`READONLY_CODE`].
    pub code: &'static str,
    /// The path the notice is about; for a link, the offending component.
    pub path: PathBuf,
    /// Human-readable detail for the log line.
    pub detail: String,
}

/// The layout this plugin writes to.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/rspinyin`, mode [`DIR_MODE`].
    pub config_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/rspinyin/config.toml`, mode [`FILE_MODE`].
    pub config_file: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin`, mode [`DIR_MODE`].
    pub data_dir: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin/user.redb`, mode [`FILE_MODE`].
    pub user_db: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin/logs`, mode [`DIR_MODE`].
    pub log_dir: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin/crash`, mode [`DIR_MODE`].
    pub crash_dir: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin/ui_takeover.json`, mode [`FILE_MODE`].
    pub takeover: PathBuf,
    /// Whether this layout may be written to.
    readonly: bool,
    /// What went wrong, or had to be narrowed, while the layout was prepared.
    notices: Vec<Notice>,
}

impl Paths {
    /// Derives the layout from `bases` without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::DataReadonly`] when a path of the layout would not fit
    /// [`MAX_PATH_BYTES`], and sets the process-wide read-only flag with it.
    pub fn from_bases(bases: &BaseDirs) -> Result<Self, ImeError> {
        let config_dir = bases.config_home.join(PROGRAM_DIR);
        let data_dir = bases.data_home.join(PROGRAM_DIR);
        let paths = Self {
            config_file: config_dir.join(CONFIG_FILE),
            user_db: data_dir.join(USER_DB_FILE),
            log_dir: data_dir.join(LOG_SUBDIR),
            crash_dir: data_dir.join(CRASH_SUBDIR),
            takeover: data_dir.join(TAKEOVER_FILE),
            config_dir,
            data_dir,
            readonly: false,
            notices: Vec::new(),
        };
        paths.check_length()?;
        Ok(paths)
    }

    /// Whether the plugin may write to this layout: the answer for this call, which is what
    /// a test asserts on, while [`is_readonly_mode`] is the process-wide flag.
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// What the preparation pass found, in the order it found it.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// Refuses a layout whose paths would not fit the kernel's path limit.
    ///
    /// Checked here rather than left to the first syscall, so that an absurd
    /// `$XDG_DATA_HOME` degrades to read-only with a reason instead of failing every later
    /// call with `ENAMETOOLONG`.
    fn check_length(&self) -> Result<(), ImeError> {
        let owned = [
            &self.config_dir,
            &self.config_file,
            &self.data_dir,
            &self.user_db,
            &self.log_dir,
            &self.crash_dir,
            &self.takeover,
        ];
        for path in owned {
            let len = path.as_os_str().len();
            if len >= MAX_PATH_BYTES {
                return Err(readonly_error(format!(
                    "layout path of {len} bytes exceeds the {MAX_PATH_BYTES}-byte limit"
                )));
            }
        }
        Ok(())
    }
}

/// Resolves the layout from the environment and prepares every directory in it.
///
/// # Errors
///
/// Returns [`ImeError::DataReadonly`] when no base directory can be resolved or a path
/// exceeds [`MAX_PATH_BYTES`]. A failure *inside* the layout -- an unwritable directory, a
/// symbolic link, a mode that cannot be narrowed -- is not an error: the layout is returned
/// with [`Paths::is_readonly`] set, because the plugin keeps working without learning.
pub fn ensure_dirs() -> Result<Paths, ImeError> {
    let bases = BaseDirs::from_env()?;
    ensure_dirs_in(&bases)
}

/// Prepares the layout under `bases`; the entry point a test drives, since with the bases
/// injected the layout and the modes that result are deterministic.
///
/// # Errors
///
/// As [`ensure_dirs`].
pub fn ensure_dirs_in(bases: &BaseDirs) -> Result<Paths, ImeError> {
    let mut paths = Paths::from_bases(bases)?;
    let mut prep = Preparation::default();
    // The base directories belong to every application the user runs: they are created
    // with the umask's default and never re-moded.
    let config_base = prepare_base(&bases.config_home, &mut prep);
    let config_dir_ready = prepare_dir(&bases.config_home, &paths.config_dir, &mut prep);
    if config_base && config_dir_ready {
        prepare_existing_file(&bases.config_home, &paths.config_file, &mut prep);
    }
    let data_base = prepare_base(&bases.data_home, &mut prep);
    let data_dir_ready = prepare_dir(&bases.data_home, &paths.data_dir, &mut prep);
    if data_base && data_dir_ready {
        prepare_dir(&bases.data_home, &paths.log_dir, &mut prep);
        prepare_dir(&bases.data_home, &paths.crash_dir, &mut prep);
        prepare_existing_file(&bases.data_home, &paths.user_db, &mut prep);
        prepare_existing_file(&bases.data_home, &paths.takeover, &mut prep);
    }
    paths.readonly = prep.readonly;
    paths.notices = prep.notices;
    Ok(paths)
}

/// Whether this process has fallen back to read-only mode.
///
/// Process-wide and sticky: [`ensure_dirs`] sets it when any step fails, and the status
/// strip renders its lock from it. The per-call answer is [`Paths::is_readonly`].
pub fn is_readonly_mode() -> bool {
    READONLY_MODE.load(Ordering::Acquire)
}

/// Opens `path` for reading and writing, creating it private when it is new.
///
/// The mode is passed to `open(2)` rather than applied afterwards, so the file never exists
/// -- not even for one syscall -- with the umask's default of `0644`. `open` applies the
/// mode only when it creates the file, so a file that is already there is narrowed through
/// the descriptor that was just opened: the same inode the caller will write to, and not a
/// second lookup of the path that could be raced. The file is not truncated, so a caller
/// that appends keeps what is there and one that rewrites truncates through the handle.
///
/// The path is followed, so a symbolic link here opens its target: [`ensure_dirs`] is what
/// refuses a link among the paths the layout owns, and a caller that names a path of its own
/// decides for itself.
///
/// # Errors
///
/// Returns the underlying `std::io::Error` when the file cannot be opened or its mode
/// cannot be set. The caller decides what that means -- the user database reports
/// [`ImeError::DataReadonly`] and stops learning -- so the error is left as it is rather
/// than wrapped in an [`ImeError`] variant this layer cannot choose.
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

/// Narrows an existing file to [`FILE_MODE`]. The path is followed, so a symbolic link here
/// narrows its target: a caller that must not write through a link checks the path first,
/// which is what [`ensure_dirs`] does for the paths the layout owns.
///
/// # Errors
///
/// Returns the underlying `std::io::Error` when the metadata cannot be read or the mode
/// cannot be set.
pub fn chmod_private(path: &Path) -> std::io::Result<()> {
    if std::fs::metadata(path)?.permissions().mode() & 0o777 == FILE_MODE {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE))
}

/// Sets the process-wide read-only flag and builds the error to propagate. The flag is
/// flipped where the condition is detected rather than by the caller, so that no path out of
/// this module can report a failure and leave the plugin believing it may still write; it is
/// never cleared, so a process that has fallen back once stays read-only rather than
/// oscillating as directories appear and disappear.
fn readonly_error(reason: String) -> ImeError {
    READONLY_MODE.store(true, Ordering::Release);
    ImeError::DataReadonly { reason }
}

/// What one preparation pass found.
#[derive(Debug, Default)]
struct Preparation {
    /// Conditions worth reporting, in the order they were found.
    notices: Vec<Notice>,
    /// Set by the first step that failed.
    readonly: bool,
}

impl Preparation {
    /// Records a step that failed, which is what turns the plugin read-only (`ASM-15`): the
    /// layout is still handed out and input keeps working, but nothing is written.
    ///
    /// The process-wide flag is set here, where the condition is detected, for the reason
    /// [`readonly_error`] gives: [`is_readonly_mode`] is what the status strip renders its
    /// lock from, so a degradation that left it clear would show the user a writable state
    /// for a layout nothing can write to.
    fn degrade(&mut self, code: &'static str, path: &Path, detail: impl Into<String>) {
        READONLY_MODE.store(true, Ordering::Release);
        self.notices.push(Notice {
            code,
            path: path.to_path_buf(),
            detail: detail.into(),
        });
        self.readonly = true;
    }

    /// Records a mode that had to be narrowed.
    fn fixed(&mut self, path: &Path, from: u32, to: u32) {
        self.notices.push(Notice {
            code: PERMS_FIXED_CODE,
            path: path.to_path_buf(),
            detail: format!("mode {from:04o} -> {to:04o}"),
        });
    }
}

/// Creates a base directory when it is missing, leaving an existing one exactly as it is:
/// this directory is shared with the user's other applications, and the layout's own modes
/// start one level below it. Returns whether the directory is usable.
fn prepare_base(base: &Path, prep: &mut Preparation) -> bool {
    match std::fs::create_dir_all(base) {
        Ok(()) => true,
        Err(error) => {
            prep.degrade(READONLY_CODE, base, error.to_string());
            false
        }
    }
}

/// Prepares one directory the layout owns. Returns whether it is usable; a caller skips the
/// paths inside it when it is not, so that one refusal is reported once rather than once per
/// path beneath it.
fn prepare_dir(base: &Path, dir: &Path, prep: &mut Preparation) -> bool {
    if let Some(link) = first_symlink(base, dir) {
        prep.degrade(PATH_SYMLINK_CODE, &link, LINK_DETAIL);
        return false;
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => tighten(dir, meta.permissions().mode(), DIR_MODE, prep),
        Ok(_) => {
            prep.degrade(READONLY_CODE, dir, "the path exists and is not a directory");
            false
        }
        Err(error) if error.kind() == ErrorKind::NotFound => create_dir(dir, prep),
        Err(error) => {
            prep.degrade(READONLY_CODE, dir, error.to_string());
            false
        }
    }
}

/// Creates a directory with [`DIR_MODE`] in the `mkdir` call itself, so there is no moment
/// at which it exists with the umask's default of `0755`. Returns whether it is usable.
fn create_dir(dir: &Path, prep: &mut Preparation) -> bool {
    match std::fs::DirBuilder::new().mode(DIR_MODE).create(dir) {
        Ok(()) => true,
        Err(error) => {
            prep.degrade(READONLY_CODE, dir, error.to_string());
            false
        }
    }
}

/// Narrows a file the layout owns when it is already there. A file that does not exist is
/// left to its owner: creating an empty `user.redb` here would hand `redb` a file it never
/// wrote, and creating an empty `config.toml` would hand the configuration loader a file the
/// user never edited. The owner creates it through [`create_private`], which is where a new
/// file's mode comes from.
fn prepare_existing_file(base: &Path, file: &Path, prep: &mut Preparation) {
    if let Some(link) = first_symlink(base, file) {
        prep.degrade(PATH_SYMLINK_CODE, &link, LINK_DETAIL);
        return;
    }
    // A path that cannot be inspected is left to the step that opens it, which reports the
    // real error: this pass only narrows what is already there.
    let Ok(meta) = std::fs::symlink_metadata(file) else {
        return;
    };
    if meta.is_file() {
        tighten(file, meta.permissions().mode(), FILE_MODE, prep);
    }
}

/// Brings `path` down to `mode` when another account can still reach it. `mode_bits` is the
/// full `st_mode` of the metadata, of which only the permission bits matter here.
///
/// The test is "does group or other hold a bit", not "is the mode exactly `mode`": a mode
/// the user narrowed on purpose -- a read-only file, a directory without the write bit -- is
/// left alone, while what this baseline promises is that nothing the plugin creates or
/// adopts is reachable by a second account. A mode that cannot be narrowed leaves the path
/// unusable for us: user data would be written where someone else can read it, which is the
/// condition read-only mode exists for. Returns whether the path is usable afterwards.
fn tighten(path: &Path, mode_bits: u32, mode: u32, prep: &mut Preparation) -> bool {
    let current = mode_bits & 0o777;
    if current & 0o077 == 0 {
        return true;
    }
    match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        Ok(()) => {
            prep.fixed(path, current, mode);
            true
        }
        Err(error) => {
            prep.degrade(READONLY_CODE, path, error.to_string());
            false
        }
    }
}

/// Returns the first symbolic link among the components of `target` below `root`.
///
/// The base directories come from the user's environment and are the trusted root: keeping
/// a home directory or an `$XDG_DATA_HOME` on another volume is a legitimate setup. What the
/// layout will not do is follow a link *inside* its own subtree, where anyone able to write
/// to the base directory could have moved user data somewhere the plugin did not choose. A
/// component that cannot be read counts as "no link": the step that creates or opens the
/// path reports that failure with the real error.
fn first_symlink(root: &Path, target: &Path) -> Option<PathBuf> {
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
mod tests;
