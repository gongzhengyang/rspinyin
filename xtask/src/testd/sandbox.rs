//! The isolated, resettable environment every case runs in.
//!
//! Responsibility: build one directory tree per case, mirroring the layout the plugin
//! itself creates, hand out the environment a child process must be launched with so that
//! the plugin writes inside that tree and nowhere else, reset the tree between cases, and
//! restart the Fcitx5 session against it.
//!
//! Boundaries: it owns directories, environment variables and one child process, and
//! nothing else. It does not decode, it does not render, it does not read the plugin's
//! configuration, and it never looks at a data file's contents beyond hashing it. What it
//! detects is carried out as [`SandboxError`] values.
//!
//! The module is split by responsibility: this root holds the type, its construction, the
//! root guard and the environment; [`reset`] holds everything written into the tree, from
//! staging the plugin to the reset that takes mutable state away; [`session`] holds
//! everything about the Fcitx5 child; [`error`] holds the refusals.
//!
//! # The real user directories are a hard red line
//!
//! A case must never create, move or delete a file under the operator's real
//! `$XDG_DATA_HOME`, `$XDG_CONFIG_HOME` or `$XDG_RUNTIME_DIR`. Three mechanisms enforce
//! that, in this order:
//!
//! 1. [`Sandbox::create`] refuses a root that is, or contains, the real `$HOME` or either
//!    real XDG base directory -- a sandbox that owned them could reset them.
//! 2. Every path the sandbox writes goes through [`Sandbox::resolve`], which requires the
//!    path to lie below the root and refuses any component of it that is a symbolic link.
//!    A link planted in the tree cannot redirect a reset out of it.
//! 3. [`RealDirWitness`] records whether the real plugin directories exist and when they
//!    were last written, and reports any that changed, which is the assertion the promise
//!    above is checked with.
//!
//! # The layout is the plugin's own, not a second copy of it
//!
//! The tree is created by `ime_dict::paths::ensure_dirs_in`, the same function the plugin
//! calls at start-up, and the paths handed out are the fields of that call's `Paths`. A
//! sandbox that invented its own shape could pass a case against a directory structure the
//! plugin never builds, which is the one failure a test harness cannot detect about itself.
//!
//! ```text
//! <root>/data            $XDG_DATA_HOME     rspinyin/{user.redb,ui_takeover.json,logs/,crash/}
//! <root>/config          $XDG_CONFIG_HOME   rspinyin/config.toml
//! <root>/cache           $XDG_CACHE_HOME    (Fcitx5's own caches)
//! <root>/runtime         $XDG_RUNTIME_DIR   rspinyin-test/ (frame mirrors, probe snapshots)
//! <root>/share           Fcitx5 data dirs   fcitx5/{addon,inputmethod}/*.conf
//! <root>/addon           FCITX_ADDON_DIRS   librspinyin.so
//! <root>/harness         the harness's own archive of the session log
//! ```
//!
//! A case's root is `RUN/<module>/<TC-ID>/sandbox`; the directory name is the caller's,
//! because where a run's evidence lives is the run's business and not the sandbox's.
//!
//! # The stale system installation
//!
//! This machine has a copy of the addon installed system-wide. Fcitx5 resolves addon
//! libraries through its own `StandardPath`, which reads `FCITX_ADDON_DIRS`; the dynamic
//! loader's search path has no effect on that lookup at all. Every child is therefore
//! built by [`Sandbox::fcitx5_command`], the only place a child is built, and that
//! function always carries [`Sandbox::env`]. [`Sandbox::check_isolation`] refuses to start
//! a session that has staged no library of its own, and
//! [`Sandbox::mapped_addon_libraries`] reads the running child's memory map afterwards,
//! which is the only offline proof that the session loaded the sandbox's build rather than
//! the installed one.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use ime_dict::paths::{BaseDirs, Paths, ensure_dirs_in};

mod error;
mod reset;
mod session;

#[cfg(test)]
mod tests;

pub use self::error::SandboxError;
pub use self::reset::{RESET_MARK, ResetReport};
pub use self::session::{READY_TIMEOUT, Readiness, parse_addons, parse_maps};

use self::reset::{create_private_dir, restore_root_dict};

/// Directory the plugin keeps its own files in, inside each XDG base directory.
const PROGRAM_DIR: &str = "rspinyin";

/// Directory the harness keeps its runtime mirrors in, inside `$XDG_RUNTIME_DIR`.
///
/// The plugin's frame mirror writes here, so a reset has to clear it or a case would read
/// the previous case's frame and see a `revision` that never restarted.
const MIRROR_DIR: &str = "rspinyin-test";

/// Directory the harness keeps its own archives in, inside the sandbox root.
const HARNESS_DIR: &str = "harness";

/// Name of the archived session log inside [`HARNESS_DIR`].
const SESSION_LOG: &str = "fcitx5.log";

/// Program a sandbox session runs.
const FCITX5_BINARY: &str = "fcitx5";

/// Mode every directory the sandbox creates carries.
pub(super) const DIR_MODE: u32 = 0o700;

/// Fallback for `$XDG_DATA_DIRS` when the environment does not name one.
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// Fallback for `$XDG_CONFIG_DIRS` when the environment does not name one.
const DEFAULT_CONFIG_DIRS: &str = "/etc/xdg";

/// The directories a sandbox root must never own.
///
/// Resolved through the plugin's own base-directory lookup, so the guard protects exactly
/// the directories the plugin would write to and not a second guess at them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealDirs {
    /// The operator's home directory.
    pub home: PathBuf,
    /// The operator's `$XDG_DATA_HOME`.
    pub data_home: PathBuf,
    /// The operator's `$XDG_CONFIG_HOME`.
    pub config_home: PathBuf,
}

impl RealDirs {
    /// Resolves the directories from the process environment.
    ///
    /// # Return value
    /// `None` when `$HOME` is unset, empty or relative. An operator with no home directory
    /// has nothing for a sandbox to protect, and the sandbox still refuses to own an XDG
    /// base directory it can resolve.
    pub fn from_env() -> Option<Self> {
        let home = env::var_os("HOME").filter(|value| !value.is_empty())?;
        let home = PathBuf::from(home);
        if !home.is_absolute() {
            return None;
        }
        let bases = BaseDirs::from_env().ok()?;
        Some(Self { home, data_home: bases.data_home, config_home: bases.config_home })
    }
}

/// Returns the real directory that `root` is, or contains.
///
/// # Return value
/// The first of the operator's home, data and configuration directories that lies at or
/// below `root`, or `None` when `root` is safely separate from all three.
pub fn owned_real_dir(root: &Path, real: &RealDirs) -> Option<PathBuf> {
    [&real.home, &real.data_home, &real.config_home]
        .into_iter()
        .find(|dir| dir.starts_with(root))
        .cloned()
}

/// The existence and modification time of the operator's real plugin directories.
///
/// Taken before a case and compared after it, this is the assertion the isolation promise
/// is checked with: a case that wrote through to the real directories would move a
/// modification time, and one that created `$HOME/.local/share/rspinyin` would turn an
/// absent entry into a present one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealDirWitness {
    /// One entry per watched path, in the order they were given.
    entries: Vec<Witness>,
}

/// One watched path and what it looked like when the witness was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Witness {
    /// The watched path.
    path: PathBuf,
    /// Its modification time in seconds and nanoseconds, or `None` when it did not exist.
    stamp: Option<(i64, i64)>,
}

impl RealDirWitness {
    /// Records the two directories the plugin owns under the operator's real bases.
    ///
    /// # Return value
    /// A witness over `$XDG_DATA_HOME/rspinyin` and `$XDG_CONFIG_HOME/rspinyin`, or an
    /// empty witness when the operator's home cannot be resolved.
    pub fn capture() -> Self {
        let Some(real) = RealDirs::from_env() else {
            return Self { entries: Vec::new() };
        };
        Self::of(&[real.data_home.join(PROGRAM_DIR), real.config_home.join(PROGRAM_DIR)])
    }

    /// Records `paths`, whether or not they exist.
    pub fn of(paths: &[PathBuf]) -> Self {
        let entries = paths
            .iter()
            .map(|path| Witness { path: path.clone(), stamp: stamp_of(path) })
            .collect();
        Self { entries }
    }

    /// The watched paths whose existence or modification time has changed.
    pub fn changed(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| stamp_of(&entry.path) != entry.stamp)
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Fails when any watched path changed since the witness was taken.
    ///
    /// # Errors
    /// Returns [`SandboxError::RealDirsChanged`] naming every path that changed.
    pub fn assert_unchanged(&self) -> Result<(), SandboxError> {
        let changed = self.changed();
        if changed.is_empty() {
            return Ok(());
        }
        Err(SandboxError::RealDirsChanged { paths: changed })
    }
}

/// The modification time of `path`, or `None` when it does not exist.
fn stamp_of(path: &Path) -> Option<(i64, i64)> {
    fs::metadata(path).ok().map(|meta| (meta.mtime(), meta.mtime_nsec()))
}

/// One case's isolated environment.
///
/// The tree is built by [`Sandbox::create`], cleaned by [`Sandbox::reset`], and handed to a
/// child process as an environment by [`Sandbox::env`]. Dropping the sandbox stops the
/// session it started, so a failed case cannot leave a Fcitx5 process behind holding the
/// sandbox's directories.
#[derive(Debug)]
pub struct Sandbox {
    /// The sandbox root; the only region this type may write to.
    root: PathBuf,
    /// `$XDG_DATA_HOME` for the child.
    data_home: PathBuf,
    /// `$XDG_CONFIG_HOME` for the child.
    config_home: PathBuf,
    /// `$XDG_CACHE_HOME` for the child.
    cache_home: PathBuf,
    /// `$XDG_RUNTIME_DIR` for the child.
    runtime_home: PathBuf,
    /// Fcitx5's data directory for the child.
    share_home: PathBuf,
    /// The addon library directory for the child.
    addon_dir: PathBuf,
    /// The layout the plugin itself resolved for these bases.
    paths: Paths,
    /// The compiled dictionary every copy is made from.
    dict_source: Option<PathBuf>,
    /// The addon ids a restarted session must report as loaded.
    required: Vec<String>,
    /// The session this sandbox started, when one is running.
    child: Option<std::process::Child>,
    /// The `$XDG_DATA_DIRS` value the child keeps after the sandbox's own directory.
    data_dirs: String,
    /// The `$XDG_CONFIG_DIRS` value the child keeps after the sandbox's own directory.
    config_dirs: String,
}

impl Sandbox {
    /// Creates a sandbox under `root`, seeded from the repository's compiled dictionary.
    ///
    /// # Errors
    /// As [`Sandbox::create_with_dict`], plus a missing `data/compiled/base.dict`.
    pub fn create(root: &Path) -> Result<Self, SandboxError> {
        let source = repo_root()?.join(crate::dictc::DEFAULT_OUTPUT);
        Self::create_with_dict(root, &source)
    }

    /// Creates a sandbox under `root`, seeded from `dict_source`.
    ///
    /// The root is created private when it is missing and narrowed to the mode the plugin
    /// uses when it is not; an existing tree is left as it is, because cleaning it is what
    /// [`Sandbox::reset`] is for.
    ///
    /// # Errors
    /// Returns [`SandboxError::RootNotAbsolute`] when `root` is relative,
    /// [`SandboxError::RootOwnsRealDirs`] when it is or contains one of the operator's own
    /// directories, [`SandboxError::Symlink`] when the root itself is a link,
    /// [`SandboxError::LayoutDegraded`] when the plugin's own layout pass fell back to
    /// read-only, and [`SandboxError::DictSourceMissing`] when `dict_source` does not
    /// exist.
    pub fn create_with_dict(root: &Path, dict_source: &Path) -> Result<Self, SandboxError> {
        let root = prepare_root(root)?;
        let bases = BaseDirs { config_home: root.join("config"), data_home: root.join("data") };
        // The plugin's own layout pass, so the sandbox cannot drift from the shape the
        // plugin really builds. It creates each directory with the mode the plugin uses
        // and never creates `config.toml` or `user.redb`, which is exactly what a fresh
        // sandbox needs: those two files are what a reset takes away.
        let paths = ensure_dirs_in(&bases)?;
        if paths.is_readonly() {
            let notices = describe_notices(&paths);
            return Err(SandboxError::LayoutDegraded { root, notices });
        }
        if !dict_source.is_file() {
            return Err(SandboxError::DictSourceMissing { path: dict_source.to_path_buf() });
        }
        let sandbox = Self {
            cache_home: root.join("cache"),
            runtime_home: root.join("runtime"),
            share_home: root.join("share"),
            addon_dir: root.join("addon"),
            data_home: bases.data_home,
            config_home: bases.config_home,
            root,
            paths,
            dict_source: Some(dict_source.to_path_buf()),
            required: Vec::new(),
            child: None,
            data_dirs: inherited_or("XDG_DATA_DIRS", DEFAULT_DATA_DIRS),
            config_dirs: inherited_or("XDG_CONFIG_DIRS", DEFAULT_CONFIG_DIRS),
        };
        sandbox.prepare_dirs()?;
        restore_root_dict(&sandbox, dict_source)?;
        Ok(sandbox)
    }

    /// The sandbox root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `$XDG_DATA_HOME` for the child.
    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    /// `$XDG_CONFIG_HOME` for the child.
    pub fn config_home(&self) -> &Path {
        &self.config_home
    }

    /// `$XDG_RUNTIME_DIR` for the child.
    pub fn runtime_home(&self) -> &Path {
        &self.runtime_home
    }

    /// `$XDG_DATA_HOME/rspinyin`, as the plugin's own layout resolves it.
    pub fn data_dir(&self) -> &Path {
        &self.paths.data_dir
    }

    /// `$XDG_CONFIG_HOME/rspinyin`, as the plugin's own layout resolves it.
    pub fn config_dir(&self) -> &Path {
        &self.paths.config_dir
    }

    /// The user frequency store the plugin writes.
    pub fn user_db(&self) -> &Path {
        &self.paths.user_db
    }

    /// The configuration file the plugin reads.
    pub fn config_file(&self) -> &Path {
        &self.paths.config_file
    }

    /// The sandbox's own copy of the compiled dictionary.
    ///
    /// It sits where an installed plugin finds one, `<datadir>/rspinyin/base.dict`, so a
    /// case that exercises the dictionary exercises the same lookup an installed plugin
    /// performs.
    pub fn dict_path(&self) -> PathBuf {
        self.paths.data_dir.join("base.dict")
    }

    /// The directory the plugin's frame mirror writes into.
    pub fn mirror_dir(&self) -> PathBuf {
        self.runtime_home.join(MIRROR_DIR)
    }

    /// The addon library directory the child is given.
    pub fn addon_dir(&self) -> &Path {
        &self.addon_dir
    }

    /// The environment a child must be launched with.
    ///
    /// # Return value
    /// Every variable that redirects the plugin's own files, and Fcitx5's own data and
    /// configuration lookups, into the sandbox. `HOME` is deliberately not among them: the
    /// operator's home directory is left exactly as it is, and [`RealDirWitness`] is what
    /// proves the child did not write to it.
    pub fn env(&self) -> Vec<(String, String)> {
        let mut vars = vec![
            pair("XDG_DATA_HOME", &self.data_home),
            pair("XDG_CONFIG_HOME", &self.config_home),
            pair("XDG_CACHE_HOME", &self.cache_home),
            pair("XDG_RUNTIME_DIR", &self.runtime_home),
            // The addon library lookup, which the dynamic loader's search path does not
            // affect: Fcitx5 resolves `Library=` through its own StandardPath.
            pair("FCITX_ADDON_DIRS", &self.addon_dir),
            // The sandbox's own directory first and the inherited value after it, so that
            // Fcitx5's own addons and the system profile are still found. Appending is
            // what makes this correct whether the variable replaces the search path or
            // adds to it.
            ("FCITX_DATA_DIRS".to_owned(), search_path(&self.share_home, &self.data_dirs)),
            ("FCITX_CONFIG_DIRS".to_owned(), search_path(&self.config_home, &self.config_dirs)),
        ];
        vars.sort();
        vars
    }

    /// The command that starts a session against this sandbox.
    ///
    /// This is the only place a child is built, and it always carries [`Sandbox::env`]. A
    /// session started any other way would load the copy of the addon installed
    /// system-wide, and the case would pass for code it never ran.
    pub fn fcitx5_command(&self) -> Command {
        let mut command = Command::new(FCITX5_BINARY);
        command.envs(self.env());
        command
    }

    /// Creates the directories the sandbox owns beyond the plugin's own layout.
    fn prepare_dirs(&self) -> Result<(), SandboxError> {
        let owned = [
            self.cache_home.clone(),
            self.runtime_home.clone(),
            self.share_home.clone(),
            self.addon_dir.clone(),
            self.harness_dir(),
            self.mirror_dir(),
        ];
        for dir in owned {
            self.resolve(&dir)?;
            create_private_dir(&dir)?;
        }
        Ok(())
    }

    /// The directory the harness keeps its own archives in.
    pub(super) fn harness_dir(&self) -> PathBuf {
        self.root.join(HARNESS_DIR)
    }

    /// Resolves `path` as a place inside the sandbox that is safe to write.
    ///
    /// The root is the only region a reset may write to, and a link inside it is refused
    /// for the reason the plugin refuses one: a link is a path somebody else chose, and
    /// following it would move or delete a file outside the sandbox. Everything the
    /// harness writes -- screenshots, fixtures, probe snapshots -- goes through this, so a
    /// path built from a case's own data cannot leave the sandbox.
    ///
    /// # Return value
    /// `path` itself, once it is known to lie below the root with no symbolic link among
    /// its components.
    ///
    /// # Errors
    /// Returns [`SandboxError::OutsideRoot`] for a path that is not below the root, and
    /// [`SandboxError::Symlink`] for a link among its components.
    pub fn resolve(&self, path: &Path) -> Result<PathBuf, SandboxError> {
        let outside = || SandboxError::OutsideRoot {
            path: path.to_path_buf(),
            root: self.root.clone(),
        };
        let relative = path.strip_prefix(&self.root).map_err(|_| outside())?;
        let mut prefix = self.root.clone();
        for component in relative.components() {
            if !matches!(component, Component::Normal(_)) {
                return Err(outside());
            }
            prefix.push(component);
            let Ok(meta) = fs::symlink_metadata(&prefix) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                return Err(SandboxError::Symlink { path: prefix });
            }
        }
        Ok(path.to_path_buf())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.stop_fcitx5();
    }
}

/// Resolves and creates the sandbox root, refusing one that could hold real user data.
fn prepare_root(root: &Path) -> Result<PathBuf, SandboxError> {
    if !root.is_absolute() {
        return Err(SandboxError::RootNotAbsolute { root: root.to_path_buf() });
    }
    if let Some(owned) = RealDirs::from_env().and_then(|real| owned_real_dir(root, &real)) {
        return Err(SandboxError::RootOwnsRealDirs { root: root.to_path_buf(), owned });
    }
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| SandboxError::Io { path: parent.to_path_buf(), source })?;
    }
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(SandboxError::Symlink { path: root.to_path_buf() });
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(SandboxError::RootNotDirectory { root: root.to_path_buf() });
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => create_private_dir(root)?,
        Err(source) => return Err(SandboxError::Io { path: root.to_path_buf(), source }),
    }
    // An existing root is narrowed to the mode the plugin's own directories carry: a
    // sandbox a second account can read is not a sandbox.
    fs::set_permissions(root, fs::Permissions::from_mode(DIR_MODE))
        .map_err(|source| SandboxError::Io { path: root.to_path_buf(), source })?;
    Ok(root.to_path_buf())
}

/// A `(name, value)` pair of the environment vector.
fn pair(name: &str, value: &Path) -> (String, String) {
    (name.to_owned(), value.display().to_string())
}

/// `<dir>` followed by the inherited search path, colon-separated.
fn search_path(dir: &Path, rest: &str) -> String {
    format!("{}:{rest}", dir.display())
}

/// The inherited value of `name`, or `fallback` when it is unset or empty.
fn inherited_or(name: &str, fallback: &str) -> String {
    env::var(name).ok().filter(|value| !value.is_empty()).unwrap_or_else(|| fallback.to_owned())
}

/// Renders a layout pass's notices for an error message.
fn describe_notices(paths: &Paths) -> String {
    paths
        .notices()
        .iter()
        .map(|notice| format!("{} {}: {}", notice.code, notice.path.display(), notice.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The repository root, derived from the compile-time location of this crate.
fn repo_root() -> Result<PathBuf, SandboxError> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| SandboxError::UnusablePath { path: manifest.to_path_buf() })
}
