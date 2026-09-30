//! The diagnostics layer: the one fatal step, and the close the unload performs.
//!
//! Responsibility: install the process's single `tracing` subscriber, keep the handle it
//! answers with, and release that handle at unload. This is the first step of the
//! sequence and the only one whose failure declines the addon: every other step reports
//! its outcome through this layer, so without it there is nowhere left to report to.
//!
//! Boundaries: `ime-diag` owns the subscriber, the sink and the level policy. This module
//! decides only *where* the log goes and *when* it is installed, and it is the only place
//! in this crate that calls the initialiser.
//!
//! # Where the log goes
//!
//! `$XDG_DATA_HOME/rspinyin/logs/rspinyin.log`, which is the `logs` directory of the
//! layout the paths module derives. The directory is resolved here rather than through the
//! layout slot, because this step runs before the `data-dirs` step fills it: resolving it
//! is a pure computation over the environment, and the writer creates the directory it
//! needs.
//!
//! An environment that names no base directory at all has nowhere the plugin may write:
//! `ime-diag` refuses to guess a location, so the layer stays off rather than inventing
//! one. Every lifecycle line already goes through the crash channel — stderr, which
//! Fcitx5 captures into its own log — so the plugin is not left silent.
//!
//! # The policy
//!
//! The level, the rolling size and the number of kept files are the documented defaults:
//! `tracing` installs a global subscriber exactly once and offers no way to replace it, so
//! the sink has to be opened before the `config` step reads the document that carries the
//! `[diagnostics]` keys. The `probes` switch is the exception — it is a runtime switch on
//! a structure that already exists — and the `config` step applies it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ime_diag::log::{DiagConfig, DiagHandle, init_logging};
use ime_dict::paths::{BaseDirs, Paths};
use ime_types::ImeError;

use super::lock;
use super::probes;

/// The handle the `diagnostics` step installed; taken by the unload.
static DIAGNOSTICS: Mutex<Option<DiagHandle>> = Mutex::new(None);

/// Whether this step has already installed the process's subscriber.
///
/// Not the same question as whether a handle is held: the handle is released at unload,
/// while `tracing` keeps the subscriber it installed for the life of the process. A host
/// that reloads an addon it tore down therefore runs this step a second time, and the
/// second run finds the sink it would install rather than reporting the initialiser's
/// misuse — which would decline the addon and cost the user their input method over a log
/// file that is already open.
static SUBSCRIBER_INSTALLED: AtomicBool = AtomicBool::new(false);

/// The `diagnostics` step: installs the process's one `tracing` subscriber.
///
/// # Returns
///
/// `Ok(())` once the subscriber is installed, once the layer has been left off because the
/// environment names no directory to log into, and on a second run in a process that
/// already installed it.
///
/// # Errors
///
/// As [`init_logging`]: `config/invalid` with the key `diag.log` when another library in
/// the process claimed the global subscriber first. That is the one condition this step
/// treats as fatal, and it is the only way [`super::on_addon_init`] answers `false`.
///
/// # Panics
///
/// Never.
pub(super) fn init_diagnostics() -> Result<(), ImeError> {
    // The probes are created before anything else is loaded, so that the dictionary's
    // resident pages fall inside the plugin's memory growth rather than below its
    // baseline: `BUDGET-MEM-02` is stated for the whole of that growth.
    probes::create();
    if SUBSCRIBER_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    init_diagnostics_in(log_directory().as_deref())?;
    SUBSCRIBER_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// Installs the diagnostics subscriber with `log_dir` as its sink.
///
/// The entry point a test drives: with the directory injected, the sink is the test's own
/// and no environment is read.
///
/// # Arguments
///
/// * `log_dir` — the directory the log file and its rolled siblings live in, or `None`
///   when the environment names no base directory.
///
/// # Returns
///
/// `Ok(())` once the layer is up, or once it has been left off for want of a directory.
///
/// # Errors
///
/// As [`init_diagnostics`].
///
/// # Panics
///
/// Never.
pub(super) fn init_diagnostics_in(log_dir: Option<&Path>) -> Result<(), ImeError> {
    let Some(log_dir) = log_dir else {
        // Nowhere to write a log file, and `ime-diag` deliberately has no default of its
        // own. Diagnostics stay on the crash channel, which needs no subscriber.
        return Ok(());
    };
    let handle = init_logging(&DiagConfig::new(log_dir))?;
    install_diagnostics(handle);
    Ok(())
}

/// The directory the log file goes in, or `None` when the environment names none.
///
/// Derived from the layout the rest of the plugin writes to rather than assembled here, so
/// that the log lands beside the data it describes. The layout is not prepared: this step
/// runs before the step that creates the directories, and the writer creates the one it
/// needs with mode `0700`.
///
/// # Panics
///
/// Never.
fn log_directory() -> Option<PathBuf> {
    let bases = BaseDirs::from_env().ok()?;
    let paths = Paths::from_bases(&bases).ok()?;
    Some(paths.log_dir)
}

/// Records `handle` as the process's diagnostics handle.
///
/// A plain assignment rather than a first-install-wins slot: the only caller is the step
/// above, and `init_logging` succeeds at most once per process, so there is no second
/// handle for this to refuse.
///
/// # Panics
///
/// Never.
fn install_diagnostics(handle: DiagHandle) {
    *lock(&DIAGNOSTICS) = Some(handle);
}

/// Releases the diagnostics handle: the last thing the unload does.
///
/// `tracing` has no way to remove a global subscriber, so closing the layer is releasing
/// the handle the plugin holds — the redaction state goes with the last reference to it.
/// The order matters more than the act: this runs after every step above has had its say,
/// which is why the unload calls it last.
///
/// # Panics
///
/// Never.
pub(super) fn close_diagnostics() {
    drop(lock(&DIAGNOSTICS).take());
}

/// Whether a diagnostics handle is installed.
///
/// # Panics
///
/// Never.
#[cfg(test)]
pub(super) fn is_installed() -> bool {
    lock(&DIAGNOSTICS).is_some()
}
