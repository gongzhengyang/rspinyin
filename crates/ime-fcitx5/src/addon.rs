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
//! thread. Every step below is either cheap or an integration point for work that has
//! not landed yet; the dictionary load and the user store's recovery step — which reads
//! the store's counts into memory, bounded by the store's own hydration ceiling — are
//! the two that count against this budget, and both are to watch as they grow.
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
//! # The user's own words
//!
//! The one piece of user data this addon owns is the frequency store, and the one thing
//! that cannot be rebuilt from anywhere else is what the user typed. The recovery step
//! does two things with it at load: it repairs a damaged file — isolating it under a
//! `.corrupt.` name, never deleting it — and it rebuilds the store from the newest backup
//! generation that imports cleanly, which is what turns "your store is damaged" into
//! "your words are back". The store is then adopted, so that the destructor has one
//! handle to flush and to copy; `redb` refuses a second handle on one path, so the process
//! wants exactly one.
//!
//! At unload the store is flushed on the host thread and copied on a worker thread of its
//! own. The split is the difference between the two costs: the flush writes the unflushed
//! delta, which the store's own batching caps, while a backup is the whole document — up
//! to the export ceiling — and a write of that size has no place inside an Fcitx5
//! callback. The worker is not joined, so the host thread never waits on it: a backup cut
//! short by the process exiting leaves the previous generation where it was, because a
//! generation is written to a temporary and renamed onto its name, and the next start
//! writes one once the interval has passed.
//!
//! The policy is the backup module's default — one day apart, three generations kept —
//! and its directory is derived from the data layout, which is what the configuration
//! step's `[data]` keys will override once it reads them.
//!
//! # Integration points
//!
//! Most steps below belong to a subsystem that is still documentation-only, so they
//! record the gap they are waiting on and report success. The ordering and the
//! degradation policy around them are final, and a step's body changes when its
//! subsystem lands rather than the step being replaced.
//!
//! A subsystem that needs its own place in the sequence gets one, inserted where its
//! inputs are ready and under the same policy: the phrase step is the first to have
//! landed that way, between the configuration that names its document and the data
//! directories that give the document its default location, and the store-recovery step is
//! the second, at the point where the configuration that names its policy has been read
//! and the data directories exist. The two routing steps are the third and the fourth: the
//! binding table is projected from the configuration as soon as it has been read, and the
//! reload subscription follows it, because what a reload replaces is what the projection
//! installed. Adding a step is a change to the sequence rather than
//! to a body, so the pinned list in this module's tests fails until the new name is
//! recorded there.

use std::ffi::c_void;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ime_config::{Config, ConfigStore, ReloadOutcome};
use ime_dict::paths::{self, READONLY_CODE};
use ime_dict::recover::USER_DB_RECOVERED_CODE;
use ime_dict::user_db::{
    BACKUP_FAILED_CODE, BACKUP_RESTORED_CODE, BackupConfig, BackupOutcome, Clock, LARGE_STORE_CODE,
    RestoreOutcome, SLOW_DISK_CODE, SystemClock, UserDb, backup_dir, list_backups,
    recover_user_db_with_backup, run_backup,
};
use ime_types::ImeError;

use crate::engine::router::RoutingConfig;
use crate::engine::router::phrases;
use crate::ffi::emit_diagnostic;

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
/// Most steps' subsystems are still documentation-only, so their bodies record the gap
/// and report success — the degraded state the module documentation describes. When a
/// subsystem lands, its step body calls into it and reports the outcome; the table
/// itself does not change. Four steps have landed so far: the phrase step, whose body
/// loads the user's phrase document and the table it merges into; the recovery step,
/// whose body repairs the user store, rolls it back from a backup and adopts it; and the
/// two routing steps, whose bodies project the configuration in force onto the routing
/// table and wire the reload subscription that replaces it.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, init_diagnostics),
    InitStep::new("data-dirs", false, prepare_data_dirs),
    InitStep::new("config", false, load_config),
    InitStep::new("key-bindings", false, init_key_bindings),
    InitStep::new("config-watch", false, start_config_watch),
    InitStep::new("phrases", false, load_phrases),
    InitStep::new("store-recovery", false, recover_stores),
    InitStep::new("lexicon", false, load_lexicon),
];

/// Records that a step is an integration point for work that has not landed yet.
///
/// Deliberately a diagnostic rather than a silent success: a plugin that quietly runs
/// without its dictionary is far harder to explain than one that names the pieces it is
/// missing. The code is stable, so the gap is greppable in a log.
fn pending_step(step: &str, awaiting: &str) {
    emit_diagnostic(&format!("lifecycle/pending: {step} awaits {awaiting}"));
}

/// Reports a lifecycle step that failed but handled the failure itself.
///
/// The line is the one [`run_init_steps`] writes for a step that returns an error; a step
/// whose body has more to do after a failure reports it through here instead of returning,
/// so that both spellings of the same condition reach the log identically.
/// Reports a step that failed, naming the step and the reason.
///
/// The reason is taken as a `Display` rather than as an [`ImeError`]: a step can fail
/// before there is a crate error to name — starting a thread fails with a
/// [`std::io::Error`] — and wrapping one in the other only to print it would add a
/// conversion whose only purpose is to be undone by the format.
fn report_step_failure(step: &str, error: &dyn std::fmt::Display) {
    let line = format!("lifecycle/step-failed: {step} ({error})");
    emit_diagnostic(&line);
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

/// Projects the configuration in force onto the routing layer's view.
///
/// The step that turns a document into a routing table: `[keys]` becomes the binding table,
/// `[ui]` and `[engine]` the values a session is stepped with, and `[scheme]` the label the
/// window's header shows. What it installs is what [`routing_config`] answers.
///
/// The step never fails, which is why it is not fatal. A configuration store is optional
/// here — the `config` step has not landed — and the built-in defaults are then the
/// configuration in force, which is what a fresh installation routes with; the gap is
/// recorded rather than left to be discovered. An entry of `[keys]` that cannot become a
/// binding is reported, because this is where it is dropped, and a typo in a key binding
/// costs the user that binding and never their input method.
fn init_key_bindings() -> Result<(), ImeError> {
    let projected = with_config_store(|store| RoutingConfig::from_config(store.current()));
    let (routing, warnings) = match projected {
        Some(projected) => projected,
        None => {
            pending_step("key-bindings", "the configuration loader's store");
            RoutingConfig::from_config(&Config::default())
        }
    };
    for warning in &warnings {
        emit_diagnostic(&warning.to_string());
    }
    install_routing_config(routing);
    Ok(())
}

/// Wires the configuration reload subscription.
///
/// The subscription is the handler the host calls when Fcitx5 reports that the
/// configuration changed: [`on_config_reload`] re-reads the document, reports what it found
/// and adopts the routing table the new document projects to. The store it re-reads and the
/// table it replaces are what the steps above install, so this step is where the sequence
/// records the subscription. The trigger itself is the host's `reloadConfig()` callback,
/// whose slot this build's ABI does not carry yet; the step reports the gap it is waiting on
/// and succeeds, because a reload no host can trigger costs the user nothing.
fn start_config_watch() -> Result<(), ImeError> {
    // Which half of the subscription is missing, when either is: a build with no store has
    // nothing to re-read, and one with a store is only waiting for the trigger.
    let awaiting = if has_config_store() {
        "the host's reloadConfig callback slot"
    } else {
        "the configuration loader's store"
    };
    pending_step("config-watch", awaiting);
    Ok(())
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

/// Repairs the user store, rolling it back from a generation when one is usable, and
/// adopts it.
///
/// The store is the one piece of user data the plugin owns that cannot be rebuilt from
/// anything else, so this step is what turns a damaged file back into the words the user
/// typed: the recovery pass isolates what is damaged — renamed, never deleted — and the
/// newest generation that imports cleanly is what the store is rebuilt from. One bad
/// generation costs the user that generation and not the rest, because they are tried
/// newest first.
///
/// Adopting the store is what gives the destructor something to flush and to copy. It is
/// opened here, once, because `redb` refuses a second handle on one path and the engine's
/// decode path reaches its frequencies through the handle this step installs.
///
/// The step never fails, which is why it is not fatal: a store that cannot be recovered or
/// opened leaves the plugin without learning, which is the read-only degradation and never
/// a lost input method.
fn recover_stores() -> Result<(), ImeError> {
    let layout = match paths::ensure_dirs() {
        Ok(layout) => layout,
        Err(error) => {
            // Without a layout there is no store to recover and nothing to back up. The
            // error carries `data/readonly-mode`, which is what the user is told.
            report_step_failure("store-recovery", &error);
            return Ok(());
        }
    };
    let directory = backup_dir(&layout.data_dir);
    match recover_store(&layout.user_db, &directory) {
        Ok(outcome) => emit_notice(restore_notice(outcome)),
        Err(error) => report_step_failure("store-recovery", &error),
    }
    match UserDb::open(&layout.user_db) {
        Ok(db) => {
            if !db.is_hydrated() {
                // The store is past its hydration ceiling and reads fall back to the
                // on-demand path. It has no logger, so the caller reports the code.
                emit_diagnostic(LARGE_STORE_CODE);
            }
            install_user_store(UserStore {
                db: Mutex::new(db),
                backup: BackupConfig::new(directory),
            });
        }
        Err(error) => report_step_failure("store-recovery", &error),
    }
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
/// destructor calls it unconditionally: stopping an empty lifecycle costs nothing.
pub fn on_addon_destroy(_handle: *mut c_void) {
    let started = Instant::now();

    // The user's words, while the files are still writable. The store is taken out of the
    // slot rather than read from it, so a destructor that runs twice flushes once and
    // copies once, and the second call finds nothing left to do.
    if let Some(store) = take_user_store() {
        // The flush is the last write the process can make, and it stays on this thread
        // because what it writes is the unflushed delta, which the store's own batching
        // caps.
        flush_store(&store);
        // The backup is a whole document and leaves this thread; the handle is dropped
        // rather than joined, so the host never waits on the write.
        drop(start_shutdown_backup(store, now_ms()));
    }

    // Close diagnostics last, so every step above still has a sink.
    pending_step("diagnostics-close", "the diagnostics logging shutdown");

    report_destroy_outcome(started.elapsed());
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
fn report_destroy_outcome(elapsed: Duration) {
    let micros = elapsed.as_micros();
    let budget_us = DESTROY_BUDGET.as_micros();
    let message = format!("lifecycle/destroy: {micros}us budget={budget_us}us");
    emit_diagnostic(&message);
}

// ── The user's own words ─────────────────────────────────────────────────────────
//
// The store the process runs on, the recovery that rebuilds it from a backup, and the two
// writes the shutdown makes: the flush, which stays on the host thread, and the backup,
// which does not. The module documentation describes why the split is where it is.

/// The name the shutdown backup's worker thread carries.
///
/// Linux caps a thread name at fifteen bytes, and the name is worth having because the
/// thread shows up in a stack dump of a process that is shutting down.
const BACKUP_THREAD_NAME: &str = "userdb-backup";

/// What the shutdown path needs: the store to flush and to copy, and where the copies go.
///
/// One handle for the whole process, because `redb` refuses a second one on the same path:
/// the engine's decode path reaches its frequencies through this store rather than opening
/// the database again.
struct UserStore {
    /// The user frequency store, behind a lock because the final flush takes `&mut`.
    db: Mutex<UserDb>,
    /// The backup policy: how many generations are kept, and how far apart they are.
    ///
    /// The directory is [`backup_dir`] of the data directory; the rest is the backup
    /// module's default until the configuration step reads the `[data]` keys.
    backup: BackupConfig,
}

/// The process's user store: installed by the recovery step, taken by the destructor.
///
/// A `Mutex` over a slot rather than a `OnceLock` because the slot is emptied as well as
/// filled: the destructor takes the store out, which is what makes a second call to it a
/// no-op instead of a second flush and a second backup. The lock is a leaf — nothing else
/// is taken while it is held — and it is uncontended, because its only two holders are the
/// host thread at shutdown and the worker that thread starts.
static USER_STORE: Mutex<Option<Arc<UserStore>>> = Mutex::new(None);

/// Borrows a store lock, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. What these locks hold — a database handle and a slot
/// — stays usable, and refusing to serve it would cost the user the flush this section
/// exists to perform.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Installs `store` as the process's user store; the first install wins.
///
/// A second store is refused rather than replacing the first: the handle already there is
/// the one a decode may be reading through, and swapping it would leave that reader with a
/// store nothing flushes and nothing copies.
fn install_user_store(store: UserStore) {
    let mut slot = lock(&USER_STORE);
    if slot.is_none() {
        *slot = Some(Arc::new(store));
    }
}

/// Takes the installed store, leaving the slot empty.
///
/// The handle it answers with is shared with whoever else holds one, so dropping it does
/// not close the database: the last holder is what ends the store and its idle sweep.
fn take_user_store() -> Option<Arc<UserStore>> {
    lock(&USER_STORE).take()
}

/// Wall-clock milliseconds since the Unix epoch: the stamp a generation is named after.
///
/// Read through the store's own clock rather than a second `SystemTime` call, so the
/// saturating conversion from a clock set before the epoch lives in one place.
fn now_ms() -> u64 {
    SystemClock::new().now_ms()
}

/// Repairs the user store at `path`, preferring the newest generation it can roll back to.
///
/// # Parameters
/// - `path`: the store, normally `user.redb` inside the data directory.
/// - `directory`: the backup directory, normally [`backup_dir`] of that data directory.
///
/// # Return value
/// What the recovery pass did with the store: [`RestoreOutcome::Healthy`] when it opened,
/// [`RestoreOutcome::RebuiltEmpty`] when a damaged one was isolated and replaced by an
/// empty store, and [`RestoreOutcome::RestoredFromBackup`] when the words came back from a
/// generation.
///
/// # Errors
/// As [`recover_user_db_with_backup`]: `data/backup-failed` when a generation existed but
/// none could be imported, and `data/readonly-mode` when a damaged store could not be
/// replaced at all.
///
/// # Panics
/// Never: a recovery pass runs during start-up and may not cost the user their input
/// method.
fn recover_store(path: &Path, directory: &Path) -> Result<RestoreOutcome, ImeError> {
    let backups = match list_backups(directory) {
        Ok(found) => found,
        Err(error) => {
            // Reported and then dropped rather than propagated: a damaged store must still
            // be isolated and rebuilt, and what the failure costs is the rollback, which is
            // what the listing was for. `list_backups` answers `dict/unavailable` only for
            // a directory that exists and cannot be read.
            report_step_failure("store-recovery", &error);
            Vec::new()
        }
    };
    recover_user_db_with_backup(path, &backups)
}

/// The line a recovery outcome is reported with, or `None` when it needs none.
///
/// Returned rather than emitted so that the mapping is a value: which outcome reports
/// which code is the contract, and the diagnostic channel has no seam this crate's tests
/// could read a line back from.
///
/// `data/db/recovered` is the code the plain recovery pass reports for the same work, so a
/// quarantine is greppable under one spelling whether or not a generation was involved.
/// `data/backup-restored` carries the stamp and the row count, because which generation the
/// words came back from is what the user has to be told. A row count is a count and never
/// the words themselves.
fn restore_notice(outcome: RestoreOutcome) -> Option<String> {
    match outcome {
        RestoreOutcome::Healthy => None,
        RestoreOutcome::RebuiltEmpty => Some(String::from(USER_DB_RECOVERED_CODE)),
        RestoreOutcome::RestoredFromBackup { from_unix, rows } => Some(format!(
            "{BACKUP_RESTORED_CODE}: from={from_unix} rows={rows}"
        )),
    }
}

/// The line a backup attempt is reported with, or `None` when it needs none.
///
/// `Written`, `Skipped` and `Disabled` need no line: a generation that landed is the
/// ordinary answer at shutdown, and the two others are the policy working as asked. The
/// two failures are reported, because a user whose copies stopped needs to be able to find
/// out why.
fn backup_notice(outcome: &BackupOutcome) -> Option<String> {
    match outcome {
        BackupOutcome::Written | BackupOutcome::Skipped | BackupOutcome::Disabled => None,
        BackupOutcome::Readonly { reason } => Some(format!("{READONLY_CODE}: {reason}")),
        BackupOutcome::Failed { reason } => Some(format!("{BACKUP_FAILED_CODE}: {reason}")),
    }
}

/// Writes one notice to the diagnostic channel, when there is one.
///
/// The code stays the line's first field, so a grep for it still matches every line it
/// produced.
fn emit_notice(notice: Option<String>) {
    if let Some(line) = notice {
        emit_diagnostic(&line);
    }
}

/// Flushes the store's pending records: the last write the process can make.
///
/// Runs on the host thread, unlike the backup. What it writes is the unflushed delta the
/// store has accumulated, which its own batching caps, and the alternative — losing the
/// last window of learning on every restart — is the durability loss `ASM-20` bounds.
fn flush_store(store: &UserStore) {
    let mut db = lock(&store.db);
    match db.final_commit() {
        Ok(report) => {
            if report.relaxed {
                // The store relaxed its batching policy because the disk is slow. It has no
                // logger, so the caller reports the code.
                emit_diagnostic(SLOW_DISK_CODE);
            }
        }
        Err(error) => report_step_failure("user-data-flush", &error),
    }
}

/// Writes the shutdown generation if one is due, and reports what happened.
///
/// The body of the worker thread, and the entry point a test drives when it wants the
/// backup without the thread.
fn run_shutdown_backup(store: &UserStore, now_unix_ms: u64) {
    let db = lock(&store.db);
    match run_backup(&db, &store.backup, now_unix_ms) {
        Ok(outcome) => emit_notice(backup_notice(&outcome)),
        // A backup that was due and could not be written. The error's own rendering is the
        // contract's `data/backup-failed` code, which has to stay the line's first field,
        // so the failure is not wrapped in a lifecycle step's line.
        Err(error) => emit_diagnostic(&error.to_string()),
    }
}

/// Starts the shutdown backup on a thread of its own.
///
/// The backup writes the whole store's document — up to the export ceiling — and a write
/// of that size has no place inside an Fcitx5 callback, so it leaves the host thread. The
/// thread is this section's own rather than the store's idle sweep, which is private to
/// `ime-dict` and wakes on a schedule of its own; it lives for one backup and ends.
///
/// # Return value
/// The worker's handle, or `None` when no thread could be started. The caller drops the
/// handle: joining would put the write back inside the callback, and a backup cut short by
/// the process exiting leaves the previous generation where it was, because a generation
/// is written to a temporary and renamed onto its name.
fn start_shutdown_backup(store: Arc<UserStore>, now_unix_ms: u64) -> Option<JoinHandle<()>> {
    let worker = std::thread::Builder::new()
        .name(String::from(BACKUP_THREAD_NAME))
        .spawn(move || run_shutdown_backup(&store, now_unix_ms));
    match worker {
        Ok(handle) => Some(handle),
        Err(error) => {
            // A thread that cannot be started is a process out of room, which the user can
            // act on; the flush above already landed, so nothing else is lost.
            report_step_failure("user-data-backup", &error);
            None
        }
    }
}

// ── The configuration in force ───────────────────────────────────────────────────
//
// What the `config` and `key-bindings` steps fill and a reload replaces, and the handler the
// host's reload callback calls.

/// The configuration in force: installed by the `config` step, re-read by a reload.
///
/// A lock over a slot rather than a `OnceLock` because a reload replaces the store in place.
/// The lock is a leaf: it is never held across a diagnostic write, and no other lock is
/// taken while it is held.
static CONFIG: Mutex<Option<ConfigStore>> = Mutex::new(None);

/// The routing layer's view of [`CONFIG`]: installed by the `key-bindings` step.
///
/// A router keeps its own copy — `RoutingConfig` is `Copy` — so the key path reads no lock
/// at all, and a reload reaches a live router through `KeyRouter::reload` rather than
/// through this slot.
static ROUTING: Mutex<Option<RoutingConfig>> = Mutex::new(None);

/// The routing configuration in force.
///
/// The value a caller builds a `KeyRouter` with: the one the `key-bindings` step projected,
/// or the shipped defaults when that step ran with no configuration store.
///
/// # Returns
///
/// A copy: a caller that takes one keeps routing the way it did, whatever a later reload
/// installs.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn routing_config() -> RoutingConfig {
    lock(&ROUTING).as_ref().copied().unwrap_or_default()
}

/// Installs `routing` as the routing configuration in force.
///
/// # Panics
///
/// Never.
fn install_routing_config(routing: RoutingConfig) {
    *lock(&ROUTING) = Some(routing);
}

/// Whether a configuration store is installed.
///
/// # Panics
///
/// Never.
fn has_config_store() -> bool {
    lock(&CONFIG).is_some()
}

/// Borrows the configuration store and runs `read` against it, or answers `None` when none
/// is installed.
///
/// The lock is held for the call, so `read` must not block on anything else and must not
/// report a diagnostic itself: a write to the crash channel is the one thing that must never
/// happen while this lock is held.
///
/// # Panics
///
/// Never.
fn with_config_store<T>(read: impl FnOnce(&ConfigStore) -> T) -> Option<T> {
    lock(&CONFIG).as_ref().map(read)
}

/// Re-reads the configuration file and adopts what changed.
///
/// The entry point the host's `reloadConfig()` callback calls; [`start_config_watch`] is
/// where the sequence records the slot that reaches it. A reload may improve the
/// configuration and nothing else: the file is never written, a document that cannot be
/// parsed never replaces one that can, and a composition in progress is never reset.
///
/// # Returns
///
/// The routing configuration in force after the reload: the value a caller hands to
/// [`KeyRouter::reload`](crate::engine::router::KeyRouter::reload) so that a live router
/// adopts it, and the value [`routing_config`] answers from now on.
///
/// # Errors
///
/// None: a document that cannot be read or parsed keeps the configuration in force, and the
/// reason travels on the diagnostic channel.
///
/// # Panics
///
/// Never.
pub fn on_config_reload() -> RoutingConfig {
    // The re-read and the projection happen under one lock, which is released before
    // anything is reported: a diagnostic write must never happen while it is held.
    let reloaded = {
        let mut slot = lock(&CONFIG);
        slot.as_mut().map(|store| {
            let outcome = store.reload();
            let routing = match &outcome {
                ReloadOutcome::Updated { .. } => Some(RoutingConfig::from_config(store.current())),
                ReloadOutcome::Unchanged | ReloadOutcome::Kept { .. } => None,
            };
            (outcome, routing)
        })
    };
    let Some((outcome, routing)) = reloaded else {
        // Nothing has been read, so nothing can have changed.
        pending_step("config-reload", "the configuration loader's store");
        return routing_config();
    };
    report_reload(&outcome);
    let Some((routing, warnings)) = routing else {
        return routing_config();
    };
    for warning in &warnings {
        emit_diagnostic(&warning.to_string());
    }
    install_routing_config(routing);
    routing
}

/// Reports what one reload produced on the diagnostic channel.
///
/// `Unchanged` needs no line: nothing changed, and the document in force was reported when
/// it was adopted. The other two carry the diagnostics of the re-read, and a line each is
/// what tells a user why their edit did or did not land.
///
/// # Panics
///
/// Never.
fn report_reload(outcome: &ReloadOutcome) {
    let warnings = match outcome {
        ReloadOutcome::Updated { warnings } | ReloadOutcome::Kept { warnings } => warnings,
        ReloadOutcome::Unchanged => return,
    };
    for warning in warnings {
        emit_diagnostic(&warning.to_string());
    }
}

#[cfg(test)]
mod tests;
