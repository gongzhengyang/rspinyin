//! Everything a sandbox operation can refuse to do.
//!
//! Responsibility: name each refusal, carry the path it is about, and render a message a
//! developer can act on. Splitting this out of [`super`] is what keeps the sandbox's own
//! file about the sandbox: nothing here touches the filesystem.
//!
//! Boundaries: it is a value type. It does not decide when a condition is an error, and it
//! never carries user input -- every path it names is a sandbox path or one of the
//! operator's own directories, and both are developer-facing.

use std::path::PathBuf;
use std::time::Duration;

use ime_types::ImeError;

/// Everything a sandbox operation can refuse to do.
///
/// Every variant names the path it is about, because a sandbox that fails without saying
/// which directory it failed on is a sandbox nobody can debug.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The requested root is not an absolute path.
    #[error("the sandbox root {root} is not an absolute path")]
    RootNotAbsolute {
        /// The path as given.
        root: PathBuf,
    },
    /// The requested root exists and is not a directory.
    #[error("the sandbox root {root} exists and is not a directory")]
    RootNotDirectory {
        /// The path as given.
        root: PathBuf,
    },
    /// The requested root is, or contains, a directory that holds the operator's data.
    #[error("the sandbox root {root} would own the real directory {owned}")]
    RootOwnsRealDirs {
        /// The path as given.
        root: PathBuf,
        /// The real directory the root would own.
        owned: PathBuf,
    },
    /// A path the sandbox was asked to write is not inside the sandbox.
    #[error("{path} is outside the sandbox root {root}")]
    OutsideRoot {
        /// The refused path.
        path: PathBuf,
        /// The sandbox root it was measured against.
        root: PathBuf,
    },
    /// A component of a path the sandbox was asked to write is a symbolic link.
    #[error("a component of {path} is a symbolic link")]
    Symlink {
        /// The offending component.
        path: PathBuf,
    },
    /// The plugin's own layout pass fell back to read-only mode.
    #[error("the layout under {root} is not writable: {notices}")]
    LayoutDegraded {
        /// The sandbox root.
        root: PathBuf,
        /// What the layout pass reported, as `code path: detail` entries.
        notices: String,
    },
    /// The compiled dictionary the sandbox copies from does not exist.
    #[error("the dictionary source {path} does not exist; run `xtask dictc` first")]
    DictSourceMissing {
        /// The missing source.
        path: PathBuf,
    },
    /// The dictionary copy does not hash to the source it was made from.
    ///
    /// The second field is named `pristine` rather than `source` on purpose: `thiserror`
    /// treats a field with that exact name as the error's `source()`, and requires it to
    /// implement `std::error::Error`. A `PathBuf` does not, so naming it `source` makes
    /// the derive emit a call to `as_dyn_error` on a path and the crate stops compiling.
    #[error("the dictionary copy {copy} does not match the source {pristine}")]
    DictMismatch {
        /// The sandbox copy.
        copy: PathBuf,
        /// The pristine source.
        pristine: PathBuf,
    },
    /// Every reset name beside a file is taken.
    #[error("every reset name beside {path} is taken")]
    ResetNameExhausted {
        /// The file whose names are exhausted.
        path: PathBuf,
    },
    /// A descriptor or library name could not be read.
    #[error("{path} has no usable file name")]
    UnusablePath {
        /// The path whose name is unusable.
        path: PathBuf,
    },
    /// A descriptor's first section is neither `[Addon]` nor `[InputMethod]`.
    #[error("{path} declares no addon and no input method")]
    UnknownDescriptor {
        /// The descriptor.
        path: PathBuf,
    },
    /// A descriptor names a library the build did not produce.
    #[error("{path} was not built, but {descriptor} names it")]
    AddonLibraryMissing {
        /// The library the build should have produced.
        path: PathBuf,
        /// The descriptor that names it.
        descriptor: PathBuf,
    },
    /// No addon has been staged, so a session could only load someone else's build.
    #[error("no addon was staged under {dir}; a session would load the installed build")]
    NoAddonStaged {
        /// The directory that should hold the staged library.
        dir: PathBuf,
    },
    /// Fcitx5 could not be started.
    #[error("could not start fcitx5: {source}")]
    FcitxSpawn {
        /// The failure the process spawn reported.
        #[source]
        source: std::io::Error,
    },
    /// The child could not be polled.
    #[error("could not read the state of the fcitx5 child: {source}")]
    FcitxWait {
        /// The failure the poll reported.
        #[source]
        source: std::io::Error,
    },
    /// Fcitx5 exited before it reported its addons.
    #[error("fcitx5 exited with {status} before reporting its addons; log kept at {log}")]
    FcitxExited {
        /// The exit status the child reported.
        status: String,
        /// The archived session log.
        log: PathBuf,
    },
    /// Fcitx5 reported that a staged addon failed to load.
    #[error("fcitx5 reported that the addon {addon} did not load; log kept at {log}")]
    FcitxAddonFailed {
        /// The addon the session refused.
        addon: String,
        /// The archived session log.
        log: PathBuf,
    },
    /// Fcitx5 did not report every staged addon within the deadline.
    #[error("fcitx5 did not report {missing:?} as loaded within {timeout:?}; log kept at {log}")]
    FcitxNotReady {
        /// The staged addons the session never reported.
        missing: Vec<String>,
        /// The deadline that expired.
        timeout: Duration,
        /// The archived session log.
        log: PathBuf,
    },
    /// The session did not map a staged addon library.
    #[error("the session did not map an addon library from {root} (mapped: {loaded:?})")]
    ForeignAddonLoaded {
        /// The addon libraries the session mapped.
        loaded: Vec<PathBuf>,
        /// The sandbox root they were required to be under.
        root: PathBuf,
    },
    /// The operator's real plugin directories changed while a case ran.
    #[error("the real plugin directories changed: {paths:?}")]
    RealDirsChanged {
        /// The paths whose existence or modification time moved.
        paths: Vec<PathBuf>,
    },
    /// The layout could not be resolved at all.
    #[error("the data layout is unavailable: {0}")]
    Layout(#[from] ImeError),
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The failure the operation reported.
        #[source]
        source: std::io::Error,
    },
}
