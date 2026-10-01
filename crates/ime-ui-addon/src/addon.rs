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
//! started in the background — with one bounded exception, [`SURFACE_READY_DEADLINE`],
//! which waits for the window to exist before the sequence ends. That wait is what makes
//! the takeover reachable: the host reads `UserInterface::available()` after the addon is
//! constructed, and a window that exists by then is a window the host can choose. A frame
//! that arrives before the start-up reports ready commits its text and draws no candidate
//! window, which is what [`candidate_window_available`] answers and what the `ui/not-ready`
//! diagnostic records — at most one line per second, because the throttle in [`crate::ffi`]
//! counts the repeats instead of writing one line per keystroke.
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
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ime_types::{ImeError, SurfaceBackend, UiCommand};
use ime_ui::surface::CandidateSurface;
use ime_ui::ui_thread::{UiSurface, UiThread, UiThreadConfig};

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
#[cfg(not(test))]
const UI_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(200);

/// The same bound, widened for the test build and for one reason only: a debug-profile
/// build rasterizes a frame orders of magnitude slower than the release one the budget
/// was written against (the software renderer's font work dominates unbaked), so a stop
/// that arrives beside an in-flight frame cannot be served inside 200 ms there. What the
/// tests assert is that an asked thread stops at all rather than hanging forever; the
/// production bound is untouched.
#[cfg(test)]
const UI_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the load sequence waits for the candidate window to exist.
///
/// `BUDGET-LAT-05` gives the whole synchronous part of the load 120 ms, and the probe, the
/// configuration and the diagnostics have to fit in what this wait leaves. The wait earns
/// its place because of *when* the host reads the readiness state: it evaluates
/// `UserInterface::available()` after the addon is constructed, so a window that exists by
/// then is a window the host can choose. A window that appears later is only reachable if
/// something asks the host to look again, and nothing in this library can: asking is
/// `UserInterfaceManager::updateAvailability()`, which belongs on the host thread, and the
/// one host-thread callback this addon receives while it is inactive — `available()` — is a
/// query it has to answer without side effects. A start-up that misses this deadline still
/// raises the flag when it finishes; what it loses is the host's first look.
const SURFACE_READY_DEADLINE: Duration = Duration::from_millis(80);

/// Diagnostic code recorded when a frame arrives before the candidate window can be
/// drawn.
///
/// Public because it is a stable identifier rather than an implementation detail: the
/// takeover reports the same code when it skips the switch for the same reason, and a
/// grep for it has to match every line either path produced.
pub const UI_NOT_READY_CODE: &str = "ui/not-ready";

/// Recorded when the UI thread did not stop inside its shutdown deadline.
///
/// Two ways of missing it share the code, because both leave the same state behind — a
/// thread that is still running while the addon is being unloaded: a stop request that
/// could not be delivered, and a thread that was asked and did not answer in time. In both
/// cases the thread is detached rather than waited for.
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
/// load order. The order below is load-bearing in one place: the platform probe runs
/// before the UI start-up, because the start-up builds the window into the surface the
/// probe constructed and has nothing to build without it. The remaining steps' subsystems
/// are still documentation-only, so their bodies record the gap and report success — the
/// degraded state the module documentation describes. When such a subsystem lands, its
/// step body calls into it and reports the outcome; the table itself does not change.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, init_diagnostics),
    InitStep::new("config", false, load_config),
    InitStep::new("platform", false, probe_platform),
    InitStep::new("ui-startup", false, start_ui_startup),
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

/// Starts the UI thread, the pre-created window and the font warm-up.
///
/// The step takes the backend the platform probe constructed. Without one there is no
/// surface to build and no thread to own one, and the probe has already recorded why, so
/// the step returns success rather than repeating the reason under a second code.
///
/// The thread starts in the background and the step waits for the window for at most
/// [`SURFACE_READY_DEADLINE`], which is the sequence's one bounded wait on another thread's
/// work; that constant records why the wait is what makes the takeover reachable.
///
/// # Errors
///
/// Returns [`ImeError::UiChannelClosed`] when the thread cannot be created. The UI channel
/// never opens in that case, which leaves the addon unavailable: the host keeps drawing the
/// candidates itself, and typing is unaffected.
fn start_ui_startup() -> Result<(), ImeError> {
    let Some(backend) = crate::platform::take_backend() else {
        return Ok(());
    };
    set_ui_startup(spawn_ui_startup(backend)?);
    Ok(())
}

/// Probes the platform backend and constructs it.
///
/// The probe's outcome is recorded by [`crate::platform::install`], which owns the
/// diagnostic and the availability flag the host reads: a session that can host no window
/// gets `platform/compositor/unsupported` with the tier it offered, and the host keeps
/// drawing the candidates. The step itself never fails — "no backend" is the answer to the
/// question the probe asks rather than a step that went wrong, and reporting it as a failed
/// step would name the same condition under two codes.
fn probe_platform() -> Result<(), ImeError> {
    crate::platform::install(crate::platform::probe_from_process());
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
///
/// This step runs on the Fcitx5 host thread, and so must any later attempt: asking the
/// host is `UserInterfaceManager::updateAvailability()`, which walks every
/// user-interface addon and calls into each one, so it is neither thread-safe nor
/// reentrant. A thread that notices a change in the state the takeover reads — the UI
/// start-up reporting ready, the platform probe answering — has to get back onto the
/// host loop before re-running it, rather than calling it from where the change happened.
///
/// The attempt is therefore made once per load, after both facts it reads are known: the
/// probe has answered, and the start-up has had [`SURFACE_READY_DEADLINE`] to produce the
/// window. A window that appears after that is not retried from here, and cannot be — the
/// only host-thread callback this addon receives while it is inactive is `available()`,
/// which is a query it has to answer without side effects, so a later attempt needs an
/// entry point that posts `updateAvailability()` onto the host's own loop, which this
/// library does not have. What a late window still changes is `available()` itself, which
/// the host reads the next time it re-evaluates.
fn register_ui() -> Result<(), ImeError> {
    // The transport handshake runs here because this is the step where both ends are
    // known to be up: this addon's thread started in the previous step, and the engine
    // addon is loaded by the same host that loaded this one (ADR-0011). A failed
    // handshake is recorded, not fatal -- the engine keeps its own `ui/not-ready`
    // degradation, and the next addon load is a fresh chance.
    crate::ffi::transport::register_transport();
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
/// [`UI_SHUTDOWN_TIMEOUT`]; a thread that outlives it is detached and recorded rather than
/// blocking the host.
///
/// Both readiness flags are cleared afterwards. The window and the backend go away with the
/// addon, so a host that asks in between — a reload that re-evaluates availability before
/// the next initialisation has run — has to be told there is no window rather than be routed
/// input-panel updates into one that no longer exists.
pub fn on_addon_destroy() {
    let started = std::time::Instant::now();
    // The sink slot clears before the thread stops: from this point the engine's posts
    // are back on their `ui/not-ready` degradation instead of arriving at a thread that
    // is being torn down (ADR-0011).
    crate::ffi::transport::unregister_transport();
    let stopped_cleanly = stop_ui();
    clear_ui_ready();
    ui_impl::set_window_backend_available(false);
    report_destroy_outcome(started.elapsed(), stopped_cleanly);
}

/// Records the outcome of the initialisation sequence.
///
/// The backend is named here rather than in a line of its own: which surface serves the
/// window is part of the same "did the load work" summary, and a code of its own would be a
/// second name for something this line already carries. `none` means the probe found no
/// backend, whose reason was recorded by the platform layer when it looked.
fn report_init_outcome(is_usable: bool, elapsed: Duration) {
    let backend = crate::platform::backend_id().unwrap_or("none");
    emit_diagnostic(&format!(
        "lifecycle/init: usable={is_usable} backend={backend} elapsed_ms={}",
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
///
/// The UI thread raises the flag when the surface exists and [`on_addon_destroy`] clears it
/// again, so the answer follows the window rather than the process: `true` while there is a
/// surface to draw into, `false` before one exists and after one has been released.
pub fn candidate_window_ready() -> bool {
    UI_READY.load(Ordering::Acquire)
}

/// Whether a frame built now may be drawn into the candidate window.
///
/// Answers `false` until the surface exists: a start-up that failed answers `false` for the
/// rest of the session, and a start-up that is merely slow answers `false` until it
/// finishes, which is at most [`SURFACE_READY_DEADLINE`] of the load and the whole of a
/// first frame that arrives during it. A caller that receives `false` must still commit the
/// text it holds: only the window is skipped, never the input. Each declined frame records
/// `ui/not-ready`, the diagnostic that explains a candidate window which is briefly absent;
/// the throttle behind it writes that code once per second and carries the count of what it
/// swallowed on the next line, so a session that never becomes ready costs a rate rather
/// than a line per keystroke.
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
///
/// Raised by the UI thread once the surface exists ([`mark_ui_ready`]) and cleared by the
/// host thread when the addon is released ([`clear_ui_ready`]).
static UI_READY: AtomicBool = AtomicBool::new(false);

/// The background start-up, or `None` before it starts and after it is stopped.
///
/// A `Mutex` because a thread handle cannot be an atomic. The host thread is the only
/// writer — once at load, once at unload — so the lock is uncontended except against a
/// test that drives the lifecycle itself.
static UI_STARTUP: Mutex<Option<UiStartup>> = Mutex::new(None);

/// The deferred part of the UI start-up: the thread, and the signal that the window exists.
struct UiStartup {
    /// The thread that owns the window.
    thread: UiThread,
    /// Reports that the surface was built, and disconnects when the build failed.
    ///
    /// A channel rather than the thread handle: [`UiThread::shutdown`] takes a timeout of
    /// its own, but the host has to learn that the window exists *before* it decides
    /// whether to take the candidates over, which is a wait on a signal rather than on a
    /// thread.
    ready: Receiver<()>,
}

impl UiStartup {
    /// Pairs the thread with the signal that reports the window.
    fn new(thread: UiThread, ready: Receiver<()>) -> Self {
        Self { thread, ready }
    }

    /// Waits up to `deadline` for the candidate window to exist.
    ///
    /// The outcome is deliberately not a return value: readiness is the flag the rest of
    /// the plugin reads, the takeover reports `ui/not-ready` when the window is not there,
    /// and a wait that ran out is not a condition to act on — the thread keeps building,
    /// and the flag it raises on the way is what the host will read next time.
    fn await_window(&self, deadline: Duration) {
        // A disconnect means the build failed, which is the same answer as a wait that ran
        // out: no window, and nothing different to do about it.
        let _ = self.ready.recv_timeout(deadline);
    }
}

/// Starts the UI thread and waits, bounded, for the candidate window to exist.
///
/// The factory runs on the UI thread because that is where the Slint platform has to be
/// installed; it builds the surface, raises the readiness flag and reports back. Raising
/// the flag there rather than in the waiter is what makes a start-up that misses the
/// deadline still count: the thread that built the window is the one that knows it exists.
///
/// # Errors
///
/// Returns [`ImeError::UiChannelClosed`] when the thread cannot be created. The UI channel
/// never opens in that case, which is the same reduced state as a window that failed to
/// appear: text still commits, the window is simply absent.
fn spawn_ui_startup(backend: Box<dyn SurfaceBackend>) -> Result<UiStartup, ImeError> {
    let (ready_tx, ready) = channel();
    let config = UiThreadConfig {
        thread_name: UI_THREAD_NAME,
        ..UiThreadConfig::default()
    };
    let thread = UiThread::spawn(config, move |_context| {
        let surface = CandidateSurface::new(backend)?;
        mark_ui_ready();
        // A failed send means nobody is waiting any more, which is what a start-up that
        // outlived its deadline looks like; the flag above is the answer either way.
        let _ = ready_tx.send(());
        let surface: Box<dyn UiSurface> = Box::new(surface);
        Ok(surface)
    })?;
    let startup = UiStartup::new(thread, ready);
    startup.await_window(SURFACE_READY_DEADLINE);
    Ok(startup)
}

/// Records that the candidate window may be drawn into.
///
/// Called on the UI thread, once the surface exists. The host thread clears it again in
/// [`on_addon_destroy`], because the window goes away with the addon.
fn mark_ui_ready() {
    UI_READY.store(true, Ordering::Release);
}

/// Records that the candidate window is gone.
fn clear_ui_ready() {
    UI_READY.store(false, Ordering::Release);
}

/// Stores the background start-up, stopping a previous one first.
///
/// A host that initialises the addon twice without destroying it in between would
/// otherwise leave the first UI thread running with nothing holding it.
/// Posts one command into the UI thread, for the cross-addon sink (ADR-0011).
///
/// The sink runs on the engine's main loop thread, so the only state it may touch is
/// what that thread can reach safely: the startup slot's mutex, held for the length of
/// one queue push. `false` means no thread is up -- the addon has not initialised or is
/// already torn down -- and the caller records that rather than retrying.
pub(crate) fn post_command(command: UiCommand) -> bool {
    let guard = lock_ui_startup();
    match guard.as_ref() {
        Some(startup) => startup.thread.send(command).is_ok(),
        None => false,
    }
}

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

/// Stops the UI thread, waiting at most [`UI_SHUTDOWN_TIMEOUT`].
///
/// Returns whether it stopped inside the deadline. The stop request travels on the channel
/// the UI thread's `poll` loop waits on, and the loop wakes on it, closes the surface and
/// exits, so a window with nothing to draw is stopped rather than waited out. A thread that
/// outlived the deadline — or that could not be asked at all — is detached and reported
/// under [`UI_SHUTDOWN_TIMEOUT_CODE`]: dropping its handle leaves it to finish on its own,
/// which is the only option left once the host is tearing the process down.
fn stop_ui_startup(startup: UiStartup) -> bool {
    let UiStartup { thread, .. } = startup;
    let delivered = thread.shutdown(UI_SHUTDOWN_TIMEOUT).is_ok();
    // The thread counts a join that ran out in its own statistics, because a shutdown that
    // gives up is still a successful call: the request was delivered, the answer was not.
    let timed_out = thread.stats().shutdown_timeouts > 0;
    if !delivered || timed_out {
        emit_diagnostic(UI_SHUTDOWN_TIMEOUT_CODE);
    }
    delivered && !timed_out
}

#[cfg(test)]
mod tests;
