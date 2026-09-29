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
//! not landed yet; the dictionary load, which is the one that will count against this
//! budget, is the step to watch when it arrives.
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
//! # Integration points
//!
//! Most steps below belong to a subsystem that is still documentation-only, so they
//! record the gap they are waiting on and report success. The sequence, the ordering
//! and the degradation policy around them are final: a step's body changes when its
//! subsystem lands, the table above it does not.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use ime_types::ImeError;

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
/// itself does not change.
const INIT_STEPS: &[InitStep] = &[
    InitStep::new("diagnostics", true, init_diagnostics),
    InitStep::new("data-dirs", false, prepare_data_dirs),
    InitStep::new("config", false, load_config),
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

    // Flush the learned frequencies first, while the files are still writable: this is
    // the last point at which the process can write them.
    pending_step("user-data-flush", "the user-database final commit");

    // Close diagnostics last, so every step above still has a sink.
    pending_step("diagnostics-close", "the diagnostics logging shutdown");

    report_destroy_outcome(started.elapsed());
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
fn report_destroy_outcome(elapsed: Duration) {
    let micros = elapsed.as_micros();
    let message = format!("lifecycle/destroy: {micros}us");
    emit_diagnostic(&message);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A step that succeeds.
    fn step_ok() -> Result<(), ImeError> {
        Ok(())
    }

    /// A step that fails. The variant is irrelevant — the policy only reads `is_fatal`.
    fn step_fails() -> Result<(), ImeError> {
        Err(ImeError::UiChannelClosed)
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
            InitStep::new("config", false, step_ok),
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
    fn test_lifecycle_runs_and_shuts_down_cleanly() {
        assert!(
            on_addon_init(std::ptr::null_mut()),
            "every step is pending, so initialisation must still succeed"
        );
        on_addon_destroy(std::ptr::null_mut());
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
                "the engine has to exist before the first key arrives"
            );
            // The engine addon is an input method and nothing else. `Category` is a
            // single-valued enum in Fcitx5, so declaring `UI` here would make the host
            // look for a `fcitx::UserInterface` this library does not provide — and
            // dispatch through a vtable slot the object does not have.
            assert!(
                conf.contains("Category=InputMethod"),
                "the engine addon must be registered under Category=InputMethod"
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
