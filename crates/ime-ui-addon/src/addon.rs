//! User-interface addon lifecycle: start-up, the candidate window, and shutdown.
//!
//! Fcitx5 finds this addon through `packaging/fcitx5/rspinyin-ui.conf`, loads
//! `librspinyin-ui.so` and constructs the addon instance through the C++ factory. The
//! factory calls [`on_addon_init`] while the process starts and [`on_addon_destroy`]
//! while it tears down; everything between those two calls is this module's concern.
//!
//! # Boundary
//!
//! `ffi::abi` owns the C ABI: the callback table, the version handshake and the guard
//! that keeps a panic from unwinding into C++. This module owns the *sequence* of steps
//! the addon runs at load and at unload, and the readiness state the host's
//! `UserInterface::available()` query is answered from.
//!
//! # Why the readiness state lives here and not in the engine
//!
//! Whether the candidate window can be drawn into is a fact about *this* library: it
//! owns the window, the UI thread and the platform backend. The engine addon is a
//! separate `dlopen`'d library with its own copy of every `static`, so it could never
//! read this state even if it wanted to — which is why it no longer tries. Fcitx5 asks
//! this addon directly through `available()`, and [`ui_impl::register_takeover`] is the
//! nudge that makes the host re-evaluate.
//!
//! # Load budget
//!
//! `BUDGET-LAT-05` gives the synchronous part of [`on_addon_init`] 120 ms, and Fcitx5
//! runs it on the main loop, so nothing here may block on work that belongs to another
//! thread. The UI thread, the pre-created window and the font warm-up are therefore
//! started in the background. A frame that arrives before that start-up reports ready
//! commits its text and draws no candidate window, which is what
//! [`candidate_window_available`] answers and what the `ui/not-ready` diagnostic
//! records — at most one line per second, because the throttle in [`crate::ffi`] counts
//! the repeats instead of writing one line per keystroke.
//!
//! # Degradation
//!
//! Every step except diagnostics initialisation fails soft: a step that cannot run
//! leaves the plugin in a reduced but usable state and [`on_addon_init`] still returns
//! `true`, so an unavailable compositor never costs the user their input method — the
//! candidates simply stay with ClassicUI, which is the documented fallback. Diagnostics
//! initialisation is the exception — without a sink there is nowhere left to report
//! anything else — and it is the only path that returns `false`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ime_types::ImeError;

use crate::ffi::emit_diagnostic;
use crate::ui_impl;

/// Name of the thread that owns the candidate window.
///
/// Fcitx5's threads are visible in `ps -T`, so the name is how an operator checks that
/// the UI thread really went away after [`on_addon_destroy`]. Linux caps a thread name
/// at 15 bytes and this one fits.
const UI_THREAD_NAME: &str = "rspinyin-ui";

/// How long [`on_addon_destroy`] waits for the UI thread to stop.
///
/// The destroy budget is 250 ms in total, so the wait takes most of it and leaves room
/// for the flush that has to happen before it.
const UI_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(200);

/// Recorded when a frame arrives before the candidate window can be drawn.
const UI_NOT_READY_CODE: &str = "ui/not-ready";

/// Recorded when the UI thread outlived its shutdown deadline and had to be detached.
const UI_SHUTDOWN_TIMEOUT_CODE: &str = "ui/shutdown-timeout";

/// One step of the synchronous initialisation sequence.
struct InitStep {
    /// Step name, used in the diagnostic a failure is reported under.
    name: &'static str,
    /// Whether a failure of this step declines the whole addon.
    ///
    /// Only diagnostics initialisation is fatal; see the module documentation.
    is_fatal: bool,
    /// The work itself.
    run: fn() -> Result<(), ImeError>,
}

impl InitStep {
    /// Builds one step of the initialisation sequence.
    const fn new(name: &'static str, is_fatal: bool, run: fn() -> Result<(), ImeError>) -> Self {
        Self {
            name,
            is_fatal,
            run,
        }
    }
}

/// The synchronous initialisation sequence, in order.
///
/// The engine addon's sequence covers the dictionary and the session; this one covers
/// only what a candidate window needs, and the two run in their own processes' addon
/// load order. Most steps' subsystems are still documentation-only, so their bodies
/// record the gap and report success — the degraded state the module documentation
/// describes. When a subsystem lands, its step body calls into it and reports the
/// outcome; the table itself does not change.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, init_diagnostics),
    InitStep::new("config", false, load_config),
    InitStep::new("ui-startup", false, start_ui_startup),
    InitStep::new("platform", false, probe_platform),
    InitStep::new("ui-registration", false, register_ui),
];

/// Records that a step is an integration point for work that has not landed yet.
///
/// Deliberately a diagnostic rather than a silent success: a plugin that quietly runs
/// without its window is far harder to explain than one that names the pieces it is
/// missing. The code is stable, so the gap is greppable in a log.
fn pending_step(step: &str, awaiting: &str) {
    emit_diagnostic(&format!("lifecycle/pending: {step} awaits {awaiting}"));
}

/// Initialises the diagnostics layer.
///
/// The one fatal step: every other step reports its outcome through this layer, so
/// without it there is nothing left to report to.
///
/// Integration point: `ime-diag` has no logging initialiser yet, so the crash channel —
/// stderr, which Fcitx5 captures into its own log — stays the only sink. When the
/// initialiser lands, this body calls it and returns its error; that error is the only
/// thing that makes [`on_addon_init`] return `false`.
fn init_diagnostics() -> Result<(), ImeError> {
    pending_step("diagnostics", "the diagnostics logging initialiser");
    Ok(())
}

/// Loads the theme and window configuration.
///
/// Integration point: `ime-config` has no loader yet. A configuration that cannot be
/// read keeps the built-in defaults and the window still appears, which is the case a
/// corrupt file has to end in.
fn load_config() -> Result<(), ImeError> {
    pending_step("config", "the configuration loader");
    Ok(())
}

/// Starts the UI thread, the window and the font warm-up in the background.
///
/// The step itself is cheap — one thread spawn — which is what keeps it inside the load
/// budget; everything expensive runs on the worker. A thread that cannot be created is
/// the only failure here, and it leaves the addon unavailable so the host keeps drawing
/// the candidates itself.
fn start_ui_startup() -> Result<(), ImeError> {
    set_ui_startup(spawn_ui_startup()?);
    Ok(())
}

/// Probes the platform backend.
///
/// Integration point: the X11 / Wayland probe and the backend ladder belong to the
/// platform work. A session that offers neither leaves the backend flag clear, so
/// `available()` answers `false` and ClassicUI keeps the candidates rather than the
/// plugin suppressing them with nothing to draw in their place.
fn probe_platform() -> Result<(), ImeError> {
    pending_step("platform", "the X11 / Wayland backend probe");
    Ok(())
}

/// Registers the self-drawn candidate window with the host.
///
/// The takeover is what keeps ClassicUI from drawing a second candidate window: Fcitx5
/// routes input-panel updates to the active user interface only, so once this addon is
/// active the host's own window receives nothing to draw. The decision itself is
/// [`ui_impl::register_takeover`], which reports which part of the takeover the session
/// is still waiting on; nothing here is fatal — a window that stays with ClassicUI
/// costs the user the custom look, not the input.
fn register_ui() -> Result<(), ImeError> {
    let outcome = ui_impl::register_takeover();
    emit_diagnostic(&outcome.diagnostic());
    Ok(())
}

/// Runs the synchronous initialisation sequence and reports whether the addon is usable.
///
/// Returns `false` only when a fatal step failed. A fatal failure stops the sequence
/// there: the host does not call [`on_addon_destroy`] for an addon that declined, so a
/// later step would leak whatever it had already taken.
fn run_init_steps(steps: &[InitStep]) -> bool {
    for step in steps {
        let Err(err) = (step.run)() else {
            continue;
        };
        emit_diagnostic(&format!("lifecycle/step-failed: {} ({err})", step.name));
        if step.is_fatal {
            return false;
        }
    }
    true
}

/// Runs the addon initialisation sequence.
///
/// Returns whether the plugin may be used. `false` leaves the addon registered but
/// unavailable, so Fcitx5 keeps ClassicUI — the documented fallback rather than a
/// broken session.
pub fn on_addon_init() -> bool {
    let started = std::time::Instant::now();
    let is_usable = run_init_steps(INIT_STEPS);
    report_init_outcome(is_usable, started.elapsed());
    is_usable
}

/// Stops the UI thread and releases what the start-up took.
///
/// Runs on the host thread during unload, so the wait is bounded by
/// [`UI_SHUTDOWN_TIMEOUT`]; a worker that outlives it is detached and recorded rather
/// than blocking the host.
pub fn on_addon_destroy() {
    let started = std::time::Instant::now();
    let stopped_cleanly = stop_ui();
    report_destroy_outcome(started.elapsed(), stopped_cleanly);
}

/// Records the outcome of the initialisation sequence.
fn report_init_outcome(is_usable: bool, elapsed: Duration) {
    emit_diagnostic(&format!(
        "lifecycle/init: usable={is_usable} elapsed_ms={}",
        elapsed.as_millis()
    ));
}

/// Records the outcome of the shutdown.
fn report_destroy_outcome(elapsed: Duration, stopped_cleanly: bool) {
    emit_diagnostic(&format!(
        "lifecycle/destroy: stopped_cleanly={stopped_cleanly} elapsed_ms={}",
        elapsed.as_millis()
    ));
}

/// Whether the candidate window can be drawn into.
///
/// The readiness flag on its own, for callers that ask repeatedly: the host queries
/// availability every time it re-evaluates which user interface is active, and
/// [`candidate_window_available`] records a diagnostic each time it answers "not
/// ready", which would fill the log with one line per query.
pub fn candidate_window_ready() -> bool {
    UI_READY.load(Ordering::Acquire)
}

/// Whether a frame built now may be drawn into the candidate window.
///
/// Answers `false` until the background start-up reports ready — the first few hundred
/// milliseconds of the process, and the whole of a start-up that failed. A caller that
/// receives `false` must still commit the text it holds: only the window is skipped,
/// never the input. Each declined frame records `ui/not-ready`, the diagnostic that
/// explains a candidate window which is briefly absent; the throttle behind it writes
/// that code once per second and carries the count of what it swallowed on the next
/// line, so a session that never becomes ready costs a rate rather than a line per
/// keystroke.
pub fn candidate_window_available() -> bool {
    match gate_candidate_window(candidate_window_ready()) {
        Ok(()) => true,
        Err(code) => {
            emit_diagnostic(code);
            false
        }
    }
}

/// The candidate-window decision, taken over the readiness flag rather than the
/// process-wide one so that both branches are reachable from a test.
fn gate_candidate_window(is_ui_ready: bool) -> Result<(), &'static str> {
    if is_ui_ready {
        Ok(())
    } else {
        Err(UI_NOT_READY_CODE)
    }
}

/// Whether the candidate window can be drawn into.
static UI_READY: AtomicBool = AtomicBool::new(false);

/// The background start-up, or `None` before it starts and after it is stopped.
///
/// A `Mutex` because a thread handle cannot be an atomic. The host thread is the only
/// writer — once at load, once at unload — so the lock is uncontended except against a
/// test that drives the lifecycle itself.
static UI_STARTUP: Mutex<Option<UiStartup>> = Mutex::new(None);

/// The deferred part of the UI start-up.
struct UiStartup {
    /// The worker running the start-up.
    worker: JoinHandle<()>,
    /// Reports that the worker has finished.
    ///
    /// A channel rather than the handle itself: a thread handle has no timed join, and
    /// blocking the host for as long as the UI thread lives would break the destroy
    /// budget.
    finished: Receiver<()>,
}

impl UiStartup {
    /// Pairs a worker with the channel that reports it finished.
    fn new(worker: JoinHandle<()>, finished: Receiver<()>) -> Self {
        Self { worker, finished }
    }
}

/// Spawns the worker that starts the UI thread, the window and the font warm-up.
///
/// # Errors
///
/// Returns [`ImeError::UiChannelClosed`] when the thread cannot be created. The UI
/// channel never opens in that case, which is the same reduced state as a window that
/// failed to appear: text still commits, the window is simply absent.
fn spawn_ui_startup() -> Result<UiStartup, ImeError> {
    let (finished_tx, finished) = channel();
    let worker = thread::Builder::new()
        .name(String::from(UI_THREAD_NAME))
        .spawn(move || {
            if ui_startup_body().is_ok() {
                mark_ui_ready();
            }
            // A send that fails means nobody is waiting, which is what happens when the
            // host stopped the worker by dropping its handle.
            let _ = finished_tx.send(());
        })
        .map_err(|_| ImeError::UiChannelClosed)?;
    Ok(UiStartup::new(worker, finished))
}

/// The body of the UI start-up worker.
///
/// Integration point: the UI thread, the pre-created window and the font warm-up belong
/// to the candidate-window work, and the backend they need comes from the platform
/// probe. Until both land there is no window to report, so the start-up fails the way a
/// session without a usable backend would — `ui/channel-closed` is the frozen code for
/// "the UI is not ready" — and the readiness flag stays clear, which keeps the host on
/// ClassicUI rather than suppressing it with nothing to draw.
///
/// # Errors
///
/// Always [`ImeError::UiChannelClosed`] while the integration point stands. The body
/// reports success once the window exists, and that is when readiness is raised.
fn ui_startup_body() -> Result<(), ImeError> {
    pending_step("ui-startup", "the UI thread and the pre-created window");
    Err(ImeError::UiChannelClosed)
}

/// Records that the candidate window may be drawn into.
fn mark_ui_ready() {
    UI_READY.store(true, Ordering::Release);
}

/// Stores the background start-up, stopping a previous one first.
///
/// A host that initialises the addon twice without destroying it in between would
/// otherwise leave the first worker running with nothing holding its handle.
fn set_ui_startup(startup: UiStartup) {
    if let Some(previous) = take_ui_startup() {
        stop_ui_startup(previous);
    }
    *lock_ui_startup() = Some(startup);
}

/// Stops the background start-up if one is running.
///
/// Returns whether it stopped inside the deadline; `true` when there was none.
fn stop_ui() -> bool {
    match take_ui_startup() {
        Some(startup) => stop_ui_startup(startup),
        None => true,
    }
}

/// Takes the background start-up out of the slot.
fn take_ui_startup() -> Option<UiStartup> {
    lock_ui_startup().take()
}

/// Borrows the start-up slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The handle it guards is independent of whatever
/// that was, and the alternative — treating the UI as permanently unstartable — would
/// leak the thread for the rest of the process.
fn lock_ui_startup() -> MutexGuard<'static, Option<UiStartup>> {
    match UI_STARTUP.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Stops the background start-up, waiting at most [`UI_SHUTDOWN_TIMEOUT`].
///
/// Returns whether the worker stopped inside the deadline. A worker that outlived it is
/// detached: dropping its handle leaves it to finish on its own, which is the only
/// option left once the host is tearing the process down. Stopping the real UI thread
/// means delivering a shutdown command to the channel its poll loop waits on; that
/// command is an integration point for the candidate-window work, so today the worker
/// is a start-up sequence that ends by itself.
fn stop_ui_startup(startup: UiStartup) -> bool {
    let UiStartup { worker, finished } = startup;
    match finished.recv_timeout(UI_SHUTDOWN_TIMEOUT) {
        Ok(()) => {
            let _ = worker.join();
            true
        }
        Err(RecvTimeoutError::Timeout) => {
            emit_diagnostic(UI_SHUTDOWN_TIMEOUT_CODE);
            // Dropping the handle detaches the worker; it ends when the process does.
            drop(worker);
            false
        }
        Err(RecvTimeoutError::Disconnected) => {
            // The worker went away without reporting. It cannot be waited for, but it is
            // also no longer running, so this is not a clean stop.
            let _ = worker.join();
            false
        }
    }
}

#[cfg(test)]
mod tests;
