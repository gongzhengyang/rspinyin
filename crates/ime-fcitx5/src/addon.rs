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
//! The candidate window is not here. It belongs to the user-interface addon
//! (`crates/ime-ui-addon`), which Fcitx5 loads as a separate addon with its own
//! lifecycle: this one decodes and commits text, that one draws. Neither waits on the
//! other, and a window that failed to start costs the user the custom look, never the
//! input.
//!
//! # Load budget
//!
//! `BUDGET-LAT-05` gives the synchronous part of [`on_addon_init`] 120 ms, and Fcitx5
//! runs it on the main loop, so nothing here may block on work that belongs to another
//! thread. Two steps count against that budget: the dictionary load, which maps the
//! container and verifies its checksums, and the user store's recovery step, which reads
//! the store's counts into memory, bounded by the store's own hydration ceiling. Both
//! are to watch as they grow; every other step is a few hundred microseconds.
//!
//! # Degradation
//!
//! Every step except diagnostics initialisation fails soft: a step that cannot run
//! leaves the plugin in a reduced but usable state and [`on_addon_init`] still returns
//! `true`, so a damaged dictionary never costs the user their input method. Diagnostics
//! initialisation is the exception — without a sink there is nowhere left to report
//! anything else — and it is the only path that returns `false`. Fcitx5 records that as
//! an unavailable addon while the other input methods keep working.
//!
//! # The steps
//!
//! Each step's body lives in the module that owns its subsystem, and the sequence below is
//! the only place their order is written down:
//!
//! | Step | What it does |
//! |---|---|
//! | `diagnostics` | installs the process's one subscriber, and creates the probes |
//! | `data-dirs` | prepares and records the XDG layout |
//! | `config` | reads `config.toml` into the store in force |
//! | `key-bindings` | projects `[keys]` onto the routing table |
//! | `config-watch` | records the reload subscription |
//! | `phrases` | loads the user's phrase document |
//! | `store-recovery` | repairs the user store, rolls it back, adopts it |
//! | `lexicon` | maps the compiled dictionary and builds the decode sources |
//! | `session-host` | hands those sources to the router every key goes through |
//!
//! A subsystem that needs its own place in the sequence gets one, inserted where its inputs
//! are ready and under the same policy. Adding a step is a change to the sequence rather
//! than to a body, so the pinned list in this module's tests fails until the new name is
//! recorded there.
//!
//! # What is still missing
//!
//! One gap, and it is not a step: the host's `reloadConfig()` callback slot. The
//! subscription the `config-watch` step records is the handler the host would call, and
//! this build's ABI carries no slot to reach it, so a configuration the user edits is
//! re-read only through [`on_config_reload`] — which is complete and tested, and waits for a
//! caller. The step reports that gap as `lifecycle/pending` rather than leaving it to be
//! discovered.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ime_types::ImeError;

use crate::engine::router::phrases;
use crate::ffi::emit_diagnostic;
use crate::session_host;

mod config;
mod diagnostics;
mod layout;
pub mod probes;
mod session;
mod user_store;

pub use self::config::{on_config_reload, routing_config};
pub use self::session::dictionary_unavailable;

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
/// Each step's body lives in the module that owns its subsystem; what is written down here
/// is the order and the policy, which is what the sequence's tests pin.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, diagnostics::init_diagnostics),
    InitStep::new("data-dirs", false, layout::prepare_data_dirs),
    InitStep::new("config", false, config::load_config),
    InitStep::new("key-bindings", false, config::init_key_bindings),
    InitStep::new("config-watch", false, config::start_config_watch),
    InitStep::new("phrases", false, load_phrases),
    InitStep::new("store-recovery", false, user_store::recover_stores),
    InitStep::new("lexicon", false, session::load_lexicon),
    InitStep::new("session-host", false, session::install_session_host),
    // Last, deliberately: whichever addon initialises second is the one that can find
    // the other already loaded, so this probe is what makes the handshake survive both
    // load orders (ADR-0011).
    InitStep::new("transport-probe", false, transport_probe),
];

/// Probes for the user-interface addon's transport and registers, if it is loaded.
///
/// The UI addon runs the mirrored probe at its own `ui-registration` step; the two
/// together are what let the handshake connect whichever addon initialises second.
/// A probe that finds nothing is recorded, not fatal and not retried: the frame
/// channel comes up on the next addon load, and `post()` keeps its `ui/not-ready`
/// degradation until then.
fn transport_probe() -> Result<(), ImeError> {
    if crate::ffi::engine_transport_probe() == 0 {
        emit_diagnostic("ui/transport/handshake-unavailable");
    }
    Ok(())
}

/// Records that a step is an integration point for work that has not landed yet.
///
/// Deliberately a diagnostic rather than a silent success: a plugin that quietly runs
/// without part of its wiring is far harder to explain than one that names the piece it is
/// missing. The code is stable, so the gap is greppable in a log.
fn pending_step(step: &str, awaiting: &str) {
    emit_diagnostic(&format!("lifecycle/pending: {step} awaits {awaiting}"));
}

/// Reports a step that failed but handled the failure itself.
///
/// The line is the one [`run_init_steps`] writes for a step that returns an error; a step
/// whose body has more to do after a failure reports it through here instead of returning,
/// so that both spellings of the same condition reach the log identically.
///
/// The reason is taken as a `Display` rather than as an [`ImeError`]: a step can fail
/// before there is a crate error to name — starting a thread fails with a
/// [`std::io::Error`] — and wrapping one in the other only to print it would add a
/// conversion whose only purpose is to be undone by the format.
fn report_step_failure(step: &str, error: &dyn std::fmt::Display) {
    let line = format!("lifecycle/step-failed: {step} ({error})");
    emit_diagnostic(&line);
}

/// Loads the user's phrase document and the table it merges into.
///
/// The phrase document is the one piece of user data the plugin reads before the first
/// key arrives, so that the routing layer has a table to match against and a file to
/// append a saved phrase to. Everything about where it lives and how it degrades belongs
/// to `engine::router::phrases`; what this step owns is its place in the sequence: after
/// the configuration, which names the document, and after the data directories, which
/// give it its default location.
///
/// The step never fails, which is why it is not fatal: a document that is missing or
/// unreadable leaves the built-in table in force and is reported as
/// `phrase/table-unavailable`. A phrase table the user can live without must not be able
/// to cost them their input method.
fn load_phrases() -> Result<(), ImeError> {
    phrases::install()
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
/// `_handle` is the opaque host context the plugin entry returned. The lifecycle steps do
/// not read it: everything they need they take from the process-wide slots the steps
/// themselves fill, and a step that later needs the host's context will be handed it here.
pub fn on_addon_init(_handle: *mut c_void) -> bool {
    let started = Instant::now();
    let is_usable = run_init_steps(INIT_STEPS);
    report_init_outcome(is_usable, started.elapsed());
    is_usable
}

/// Releases everything [`on_addon_init`] took.
///
/// The sweep runs in a frozen order: the sessions first, so no composition outlives the
/// subsystems it decoded against; the phrase writer's drain second, so the rows the last
/// keystrokes queued are written rather than lost with the process; then the user store's
/// flush and its backup, and the diagnostics last, so every step above still has a sink.
/// Each bounded step waits within a budget of its own, a step that outlives its budget is
/// recorded and left behind rather than holding up the host, and the whole of the sweep
/// stays inside [`DESTROY_BUDGET`].
///
/// Safe to call when initialisation never ran or declined, which is why the addon
/// destructor calls it unconditionally: stopping an empty lifecycle costs nothing.
pub fn on_addon_destroy(_handle: *mut c_void) {
    let started = Instant::now();

    // The sessions go first, before anything they decode against is released: the sweep
    // drops every composition without committing it, so no candidate is left that the
    // plugin could still take.
    let sessions = session_host::shutdown();

    // The phrase writer drains while every file it writes to is still writable: its rows
    // are the user's own words from the session that just ended, and a graceful stop
    // loses nothing only if someone waits for the queue. The wait is bounded by
    // `PHRASE_DRAIN_BUDGET`; a writer that outlives it is recorded and detached by the
    // phrases module, which is what keeps the flush below in possession of its share of
    // the destroy budget.
    phrases::drain_writer(PHRASE_DRAIN_BUDGET);

    // The user's words, while the files are still writable. The store is taken out of the
    // slot rather than read from it, so a destructor that runs twice flushes once and
    // copies once, and the second call finds nothing left to do.
    if let Some(store) = user_store::take_user_store() {
        // The flush is the last write the process can make, and it stays on this thread
        // because what it writes is the unflushed delta, which the store's own batching
        // caps.
        user_store::flush_store(&store);
        // The backup is a whole document and leaves this thread; the handle is dropped
        // rather than joined, so the host never waits on the write.
        drop(user_store::start_shutdown_backup(
            store,
            user_store::now_ms(),
        ));
    }

    // Close diagnostics last, so every step above still has a sink.
    diagnostics::close_diagnostics();

    report_destroy_outcome(started.elapsed(), sessions);
}

/// The synchronous part of [`on_addon_init`]: `BUDGET-LAT-05`.
///
/// Mirrored from `docs/dev/budgets.json`, the machine-readable half of the budget table;
/// the number is stated here as well because this is the code that has to stay inside it,
/// and the line the outcome is reported with carries both numbers.
pub const INIT_BUDGET: Duration = Duration::from_millis(120);

/// The whole of [`on_addon_destroy`], the flush on the host thread included.
///
/// The backup the destructor starts is not counted: it runs on a worker this side never
/// joins, which is what keeps a whole document's write out of an Fcitx5 callback.
pub const DESTROY_BUDGET: Duration = Duration::from_millis(250);

/// How long [`on_addon_destroy`] waits for the phrase writer to drain its queue.
///
/// A slice of [`DESTROY_BUDGET`], which covers every wait the destructor pays: the session
/// sweep and the diagnostics close cost microseconds, and the user store's flush writes
/// only the unflushed delta its own batching caps. A writer that has not stopped within
/// the slice is recorded under `phrase/shutdown-timeout` and detached -- its thread keeps
/// draining what it accepted on its own, and the host's exit never waits for it.
pub const PHRASE_DRAIN_BUDGET: Duration = Duration::from_millis(100);

/// Records how the synchronous initialisation ended and how long it took.
///
/// The duration is the measurement `BUDGET-LAT-05` is asserted against. Fcitx5 captures
/// stderr into its log, so a load that regressed past the budget is visible in the
/// start-up output without a profiler attached.
fn report_init_outcome(is_usable: bool, elapsed: Duration) {
    let state = if is_usable { "ready" } else { "declined" };
    let micros = elapsed.as_micros();
    let budget_us = INIT_BUDGET.as_micros();
    let message = format!("lifecycle/init: {micros}us budget={budget_us}us state={state}");
    emit_diagnostic(&message);
}

/// Records how the shutdown went and how long it took.
///
/// The session count rides on this line rather than getting one of its own: it is the
/// evidence that the unload left nothing behind.
fn report_destroy_outcome(elapsed: Duration, sessions: usize) {
    let micros = elapsed.as_micros();
    let budget_us = DESTROY_BUDGET.as_micros();
    let message = format!("lifecycle/destroy: {micros}us budget={budget_us}us sessions={sessions}");
    emit_diagnostic(&message);
}

/// Borrows a slot lock, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. What these locks hold — a slot, a layout, a database
/// handle — stays usable, and refusing to serve it would leave the plugin unable to route
/// a single key, flush a single word or read a single setting for the rest of the
/// process's life.
///
/// The locks are leaves: nothing else is taken while one is held, and none is held across
/// a diagnostic write.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests;
