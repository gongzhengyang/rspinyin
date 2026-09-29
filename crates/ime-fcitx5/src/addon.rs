//! Addon lifecycle: discovery, initialisation, degradation and shutdown.
//!
//! Fcitx5 finds this plugin through `packaging/fcitx5/rspinyin.conf`, loads
//! `librspinyin.so` and constructs the addon instance through the C++ factory. The
//! factory calls [`on_addon_init`] while the process starts and [`on_addon_destroy`]
//! while it tears down; everything between those two calls is this module's concern.
//!
//! # Boundary
//!
//! `ffi::abi` owns the C ABI: the callback table, the version handshake and the guard
//! that keeps a panic from unwinding into C++. This module owns the *sequence* of steps
//! the addon runs at load and at unload. It is plain Rust — the only pointer it ever
//! receives is the opaque host context, and it passes that on without reading through
//! it.
//!
//! # Load budget
//!
//! `BUDGET-LAT-05` gives the synchronous part of [`on_addon_init`] 120 ms, and Fcitx5
//! runs it on the main loop, so nothing here may block on work that belongs to another
//! thread. The UI thread, the pre-created window and the font warm-up are therefore
//! started in the background. A frame that arrives before that start-up reports ready
//! commits its text and draws no candidate window, which is what
//! [`candidate_window_available`] answers and what the `ui/not-ready` diagnostic
//! records.
//!
//! # Degradation
//!
//! Every step except diagnostics initialisation fails soft: a step that cannot run
//! leaves the plugin in a reduced but usable state and [`on_addon_init`] still returns
//! `true`, so a damaged dictionary or an unavailable compositor never costs the user
//! their input method. Diagnostics initialisation is the exception — without a sink
//! there is nowhere left to report anything else — and it is the only path that returns
//! `false`. Fcitx5 records that as an unavailable addon while the other input methods
//! keep working.
//!
//! # Integration points
//!
//! Every step below belongs to a subsystem that is still documentation-only, so each
//! one records the gap it is waiting on and reports success. The sequence, the ordering
//! and the degradation policy around them are final: a step's body changes when its
//! subsystem lands, the table above it does not.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

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
/// Each step's subsystem is still documentation-only, so the bodies record the gap and
/// report success — the degraded state the module documentation describes. When a
/// subsystem lands, its step body calls into it and reports the outcome; the table
/// itself does not change.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, init_diagnostics),
    InitStep::new("data-dirs", false, prepare_data_dirs),
    InitStep::new("config", false, load_config),
    InitStep::new("store-recovery", false, recover_stores),
    InitStep::new("lexicon", false, load_lexicon),
    InitStep::new("ui-startup", false, start_ui_startup),
    InitStep::new("platform", false, probe_platform),
    InitStep::new("ui-registration", false, register_ui),
];

/// Records that a step is an integration point for work that has not landed yet.
///
/// Deliberately a diagnostic rather than a silent success: a plugin that quietly runs
/// without its dictionary or its window is far harder to explain than one that names
/// the pieces it is missing. The code is stable, so the gap is greppable in a log.
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

/// Prepares the data directories the plugin writes to.
///
/// Integration point: the XDG data and log directories belong to the diagnostics and
/// storage work. A directory that cannot be created is never fatal — the plugin
/// degrades to read-only mode rather than costing the user their input method.
fn prepare_data_dirs() -> Result<(), ImeError> {
    pending_step("data-dirs", "the XDG data-directory setup");
    Ok(())
}

/// Loads the user configuration.
///
/// Integration point: `ime-config` has no loader yet. A configuration that cannot be
/// read keeps the built-in defaults and the plugin stays usable, which is the case a
/// corrupt file has to end in.
fn load_config() -> Result<(), ImeError> {
    pending_step("config", "the configuration loader");
    Ok(())
}

/// Repairs a dictionary or user database left damaged by an interrupted write.
///
/// Integration point: the recovery pass belongs to the storage work. A store that
/// cannot be repaired is rebuilt rather than trusted, so a failure here costs learned
/// data and never input.
fn recover_stores() -> Result<(), ImeError> {
    pending_step("store-recovery", "the store recovery pass");
    Ok(())
}

/// Maps the compiled dictionary.
///
/// Integration point: the memory-mapped lexicon belongs to the dictionary work. Until
/// it lands the plugin has no candidates at all. The CRC check the load performs counts
/// against the load budget, so this step is the one to watch when it arrives.
fn load_lexicon() -> Result<(), ImeError> {
    pending_step("lexicon", "the memory-mapped lexicon");
    Ok(())
}

/// Starts the UI thread, the window and the font warm-up in the background.
///
/// The step itself is cheap — one thread spawn — which is what keeps it inside the load
/// budget; everything expensive runs on the worker. A thread that cannot be created is
/// the only failure here, and it leaves the plugin committing text without a window.
fn start_ui_startup() -> Result<(), ImeError> {
    set_ui_startup(spawn_ui_startup()?);
    Ok(())
}

/// Probes the platform backend.
///
/// Integration point: the X11 / Wayland probe and the backend ladder belong to the
/// platform work. A session that offers neither leaves the plugin usable without a
/// window rather than refusing to load.
fn probe_platform() -> Result<(), ImeError> {
    pending_step("platform", "the X11 / Wayland backend probe");
    Ok(())
}

/// Registers the self-drawn candidate window with the host.
///
/// Integration point: the `UserInterface` takeover is not wired yet, so Fcitx5 keeps
/// drawing candidates itself. Nothing here is fatal — an unregistered window costs the
/// user the custom look, not the input.
fn register_ui() -> Result<(), ImeError> {
    pending_step("ui-registration", "the UserInterface takeover");
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
/// Returns whether the plugin may be used. `false` puts the addon in pure-engine mode:
/// Fcitx5 marks it unavailable and the other input methods keep working. The module
/// documentation describes which failures reach that answer.
///
/// # Arguments
///
/// `_handle` is the opaque host context the plugin entry returned. The lifecycle steps
/// do not read it — it is the callback table's first parameter, and the steps that will
/// need it take it when their subsystems land.
pub fn on_addon_init(_handle: *mut c_void) -> bool {
    let started = Instant::now();
    let is_usable = run_init_steps(INIT_STEPS);
    report_init_outcome(is_usable, started.elapsed());
    is_usable
}

/// Releases everything [`on_addon_init`] took.
///
/// Safe to call when initialisation never ran or declined, which is why the addon
/// destructor calls it unconditionally: stopping an empty lifecycle costs nothing, and
/// skipping it after a partial one would strand a worker thread.
pub fn on_addon_destroy(_handle: *mut c_void) {
    let started = Instant::now();

    // Flush the learned frequencies first, while the files are still writable: this is
    // the last point at which the process can write them.
    pending_step("user-data-flush", "the user-database final commit");

    // Stop the UI thread. Its start-up is an integration point, but the deadline and the
    // detach belong here: a UI thread that outlived the host would hold a connection and
    // a window open with nobody left to draw them.
    let stopped_cleanly = stop_ui();

    // Close diagnostics last, so every step above still has a sink.
    pending_step("diagnostics-close", "the diagnostics logging shutdown");

    report_destroy_outcome(started.elapsed(), stopped_cleanly);
}

/// Whether a frame built now may be drawn into the candidate window.
///
/// Answers `false` until the background start-up reports ready — the first few hundred
/// milliseconds of the process, and the whole of a start-up that failed. A caller that
/// receives `false` must still commit the text it holds: only the window is skipped,
/// never the input. Each declined frame records `ui/not-ready`, which is the diagnostic
/// that explains a candidate window which is briefly absent.
pub fn candidate_window_available() -> bool {
    match gate_candidate_window(UI_READY.load(Ordering::Acquire)) {
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
/// "the UI is not ready" — and the readiness flag stays clear, which keeps every frame
/// on the commit-only path.
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

/// Takes the background start-up out of its slot.
fn take_ui_startup() -> Option<UiStartup> {
    lock_ui_startup().take()
}

/// Borrows the start-up slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The handle it guards is independent of whatever
/// that was, so refusing to look at it would only strand the worker.
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

/// Records how the synchronous initialisation ended and how long it took.
///
/// The duration is the measurement `BUDGET-LAT-05` is asserted against. Fcitx5 captures
/// stderr into its log, so a load that regressed past the budget is visible in the
/// start-up output without a profiler attached.
fn report_init_outcome(is_usable: bool, elapsed: Duration) {
    let state = if is_usable { "ready" } else { "declined" };
    let micros = elapsed.as_micros();
    let message = format!("lifecycle/init: {micros}us state={state}");
    emit_diagnostic(&message);
}

/// Records how the shutdown went and how long it took.
fn report_destroy_outcome(elapsed: Duration, stopped_cleanly: bool) {
    let stop = if stopped_cleanly { "clean" } else { "forced" };
    let micros = elapsed.as_micros();
    let message = format!("lifecycle/destroy: {micros}us ui-stop={stop}");
    emit_diagnostic(&message);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// A step that succeeds.
    fn step_ok() -> Result<(), ImeError> {
        Ok(())
    }

    /// A step that fails. The variant is irrelevant — the policy only reads `is_fatal`.
    fn step_fails() -> Result<(), ImeError> {
        Err(ImeError::UiChannelClosed)
    }

    /// Builds a start-up around an arbitrary worker body, so the deadline in
    /// [`stop_ui_startup`] is reachable without the real window.
    fn startup_with(body: impl FnOnce() + Send + 'static) -> Option<UiStartup> {
        let (finished_tx, finished) = channel();
        thread::Builder::new()
            .name(String::from(UI_THREAD_NAME))
            .spawn(move || {
                body();
                let _ = finished_tx.send(());
            })
            .ok()
            .map(|worker| UiStartup::new(worker, finished))
    }

    #[test]
    fn test_init_steps_pin_the_documented_lifecycle() {
        let names: Vec<&str> = INIT_STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            [
                "diagnostics",
                "data-dirs",
                "config",
                "store-recovery",
                "lexicon",
                "ui-startup",
                "platform",
                "ui-registration",
            ]
        );
        let fatal: Vec<&str> = INIT_STEPS
            .iter()
            .filter(|step| step.is_fatal)
            .map(|step| step.name)
            .collect();
        assert_eq!(
            fatal,
            ["diagnostics"],
            "every other step must fail soft so input keeps working"
        );
    }

    #[test]
    fn test_run_init_steps_keeps_the_addon_usable_when_a_non_fatal_step_fails() {
        let steps = [
            InitStep::new("diagnostics", true, step_ok),
            InitStep::new("lexicon", false, step_fails),
            InitStep::new("platform", false, step_ok),
        ];
        assert!(
            run_init_steps(&steps),
            "a damaged subsystem must not decline the addon"
        );
    }

    #[test]
    fn test_run_init_steps_declines_the_addon_after_a_fatal_failure() {
        static REACHED: AtomicUsize = AtomicUsize::new(0);
        fn later_step() -> Result<(), ImeError> {
            REACHED.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        REACHED.store(0, Ordering::SeqCst);
        let steps = [
            InitStep::new("diagnostics", true, step_fails),
            InitStep::new("config", false, later_step),
        ];
        assert!(
            !run_init_steps(&steps),
            "a failed diagnostics step must decline the addon"
        );
        assert_eq!(
            REACHED.load(Ordering::SeqCst),
            0,
            "a declined addon must not run the steps after the failing one"
        );
    }

    #[test]
    fn test_lifecycle_runs_and_leaves_no_start_up_behind() {
        assert!(
            on_addon_init(std::ptr::null_mut()),
            "every step is pending, so initialisation must still succeed"
        );
        assert!(
            !candidate_window_available(),
            "the window does not exist yet, so no frame may be drawn into it"
        );
        on_addon_destroy(std::ptr::null_mut());
        assert!(
            take_ui_startup().is_none(),
            "destroy must stop the background start-up it started"
        );
    }

    #[test]
    fn test_stop_ui_startup_reports_whether_the_worker_stopped() {
        let quick = startup_with(|| {});
        assert!(quick.is_some(), "the test worker must start");
        if let Some(quick) = quick {
            assert!(
                stop_ui_startup(quick),
                "a worker that finished must report a clean stop"
            );
        }
        // The second worker sleeps well past the deadline, so the timeout branch is
        // reached whatever the machine's speed.
        let slow = startup_with(|| thread::sleep(UI_SHUTDOWN_TIMEOUT * 3));
        assert!(slow.is_some(), "the test worker must start");
        if let Some(slow) = slow {
            assert!(
                !stop_ui_startup(slow),
                "a worker still running at the deadline must be reported and detached"
            );
        }
    }

    #[test]
    fn test_gate_candidate_window_follows_the_readiness_flag() {
        assert_eq!(
            gate_candidate_window(false),
            Err(UI_NOT_READY_CODE),
            "a frame before the start-up reports ready must skip the window"
        );
        assert_eq!(gate_candidate_window(true), Ok(()));
        assert_eq!(UI_NOT_READY_CODE, "ui/not-ready");
    }

    /// The addon description, relative to this crate's manifest directory.
    const ADDON_CONF: &str = "../../packaging/fcitx5/rspinyin.conf";

    /// The input-method description, relative to this crate's manifest directory.
    const INPUT_METHOD_CONF: &str = "../../packaging/fcitx5/rspinyin-im.conf";

    /// Reads a packaging file, or `None` when it is missing.
    fn packaging_file(relative: &str) -> Option<String> {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).ok()
    }

    #[test]
    fn test_addon_conf_pins_what_fcitx5_resolves() {
        let conf = packaging_file(ADDON_CONF);
        assert!(conf.is_some(), "the addon description must ship");
        if let Some(conf) = conf {
            // The artifact is librspinyin.so: a bare `Library=rspinyin` would not resolve.
            assert!(
                conf.contains("Library=librspinyin"),
                "Library must name the artifact this crate builds"
            );
            assert!(conf.contains("Type=SharedLibrary"));
            assert!(
                conf.contains("OnDemand=False"),
                "the UI thread has to exist before the first key arrives"
            );
            // The frontends must be optional, not required. fcitx5 treats every entry in
            // [Addon/Dependencies] as mandatory, so listing xcb and wayland there makes
            // the plugin refuse to load on an X11-only or Wayland-only system.
            let required = conf_section(&conf, "[Addon/Dependencies]");
            let optional = conf_section(&conf, "[Addon/OptionalDependencies]");
            assert!(
                !required.contains("xcb") && !required.contains("wayland"),
                "a frontend in [Addon/Dependencies] makes the plugin unloadable when that \
                 frontend is absent; required section was: {required:?}"
            );
            assert!(
                optional.contains("xcb") && optional.contains("wayland"),
                "both frontends belong in [Addon/OptionalDependencies]"
            );
            assert!(
                required.contains("core"),
                "the core addon is the one genuine dependency"
            );
            // The packaged version has to track the crate version.
            let version = format!("Version={}", env!("CARGO_PKG_VERSION"));
            assert!(conf.contains(&version), "must declare {version}");
        }
    }

    /// Body of the named INI section, or an empty string when the section is absent.
    ///
    /// Section-scoped rather than a plain `contains`, because the whole point of these
    /// assertions is *which* section a key lives in.
    fn conf_section(conf: &str, header: &str) -> String {
        let mut body = String::new();
        let mut inside = false;
        for line in conf.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                inside = trimmed == header;
                continue;
            }
            if inside {
                body.push_str(trimmed);
                body.push('\n');
            }
        }
        body
    }

    #[test]
    fn test_input_method_conf_points_at_the_addon() {
        let conf = packaging_file(INPUT_METHOD_CONF);
        assert!(conf.is_some(), "the input-method entry must ship");
        if let Some(conf) = conf {
            assert!(
                conf.contains("Addon=rspinyin"),
                "the entry must select this addon"
            );
            assert!(
                conf.contains("Name=Rust Pinyin"),
                "this is the name configtool lists"
            );
            assert!(conf.contains("LangCode=zh_CN"));
        }
    }
}
