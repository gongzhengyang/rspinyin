//! The diagnostics layer: the one fatal step, and the close the unload performs.
//!
//! Responsibility: install the process's single `tracing` subscriber, keep the handle it
//! answers with, and release that handle at unload. This is the first step of the
//! sequence and the only one whose failure declines the addon: every other step reports
//! its outcome through this layer, so without it there is nowhere left to report to. The
//! same step arms the crash forensics — the panic hook, the fault-signal channel and the
//! context provider the records are written with — because a crash channel that is armed
//! before anything else can run is the one thing that still reports when every other
//! subsystem is the thing that failed.
//!
//! Boundaries: `ime-diag` owns the subscriber, the sink, the level policy and the whole
//! crash channel. This module decides only *where* the log and the records go and *when*
//! they are installed, and it is the only place in this crate that calls the
//! initialiser or the arming sequence.
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
//! # Where the crash records go, and what they may say
//!
//! `$XDG_DATA_HOME/rspinyin/crash/`, the layout's own crash directory, with the directory
//! at `0700` and every record at `0600`. The arming sequence is the card the crash
//! channel deals in: name the directory, install the panic hook, arm the fault signals
//! behind the FFI module's registrar, and register the context provider. Every one of
//! those steps fails soft — a line and the sequence carries on — because forensics that
//! cost the user their input method would be a defect worse than the crashes it records.
//!
//! The context provider answers from atomic slots alone. A panic can be caught anywhere,
//! including on a thread that holds the session slot's lock, so a provider that took a
//! lock could deadlock the very crash it is trying to record; the facts it reads are
//! single atomic loads, and the richer context is what the crash channel's own
//! two-phase signal design already handles outside the handler.
//!
//! # The policy
//!
//! The level, the rolling size and the number of kept files are the documented defaults:
//! `tracing` installs a global subscriber exactly once and offers no way to replace it, so
//! the sink has to be opened before the `config` step reads the document that carries the
//! `[diagnostics]` keys. The `probes` switch is the exception — it is a runtime switch on
//! a structure that already exists — and the `config` step applies it.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ime_diag::crash::{self, CrashContext, CrashContextKey};
use ime_diag::log::{DiagConfig, DiagHandle, init_logging};
use ime_dict::paths::{self, BaseDirs, Paths};
use ime_types::ImeError;

use super::lock;
use super::probes;
use super::report_step_failure;

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
    // The forensics arm before the subscriber, and on every run of this step: each
    // arming call is first-wins or idempotent, so a host that reloads the addon re-arms
    // cheaply, and a crash during the load sequence is one the channel can record.
    let layout = environment_layout();
    assemble_crash_forensics_in(layout.as_ref().map(|paths| paths.crash_dir.as_path()));
    if SUBSCRIBER_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    init_diagnostics_in(layout.as_ref().map(|paths| paths.log_dir.as_path()))?;
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

/// The layout the environment names, or `None` when it names no base directory.
///
/// Derived from the layout the rest of the plugin writes to rather than assembled here, so
/// that the log and the crash records land beside the data they describe. The layout is
/// not prepared: this step runs before the step that creates the directories, and each
/// writer creates the one it needs with the mode the privacy baseline fixes.
///
/// # Panics
///
/// Never.
fn environment_layout() -> Option<Paths> {
    let bases = BaseDirs::from_env().ok()?;
    Paths::from_bases(&bases).ok()
}

/// Arms the crash forensics over `crash_dir`: the four calls the crash channel deals in.
///
/// The order is the channel's own: name the directory first, so a record that arrives
/// before the rest of the arming has somewhere to go; install the panic hook; arm the
/// fault signals behind the FFI module's registrar; register the context provider. Every
/// step fails soft — the one error in the sequence, a refused signal registration, is
/// reported and the step still succeeds — because forensics must never be the reason the
/// addon declines.
///
/// The entry point a test drives: with the directory injected, the arming is deterministic
/// and no environment is read.
///
/// # Panics
///
/// Never.
pub(super) fn assemble_crash_forensics_in(crash_dir: Option<&Path>) {
    let Some(dir) = crash_dir else {
        // Nowhere to write a record, and `ime-diag` deliberately has no default of its
        // own. Faults still terminate the process; they just leave no file behind, and
        // the crash channel's stderr half keeps reporting.
        report_step_failure(
            "crash-forensics",
            &"no data directory names a crash directory",
        );
        return;
    };
    crash::set_crash_directory(dir.to_path_buf());
    ime_diag::panic::install_panic_hook(dir.to_path_buf());
    crash::set_crash_context_provider(crash_context);
    if let Err(error) =
        crash::signal::install_signal_handlers(dir, crate::ffi::register_crash_signal)
    {
        report_step_failure("crash-forensics", &error);
    }
}

/// The structural facts a crash record carries, read from atomic slots alone.
///
/// A panic can be caught anywhere, including on a thread that already holds the session
/// slot's lock, so the provider must not take a lock: taking one could deadlock against
/// the very panic it is describing. What it answers with is therefore what the process
/// keeps in single atomic loads — the data layer's read-only degradation, the one
/// session-level bit the paths layer maintains process-wide. The richer facts a record
/// could carry are the crash channel's own business, handled outside the handler by the
/// channel's two-phase design, and the provider's key set is closed: there is no key a
/// free-form value could be smuggled through.
///
/// # Panics
///
/// Never.
pub(super) fn crash_context() -> CrashContext {
    let mut context = CrashContext::new();
    if paths::is_readonly_mode() {
        context.insert_identifier(CrashContextKey::SessionState, "readonly");
    }
    context
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
