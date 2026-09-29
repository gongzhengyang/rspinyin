//! Tests for the user-interface addon's lifecycle.

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
            "config",
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
        "every other step must fail soft so the host keeps drawing candidates"
    );
}

#[test]
fn test_run_init_steps_keeps_the_addon_usable_when_a_non_fatal_step_fails() {
    let steps = [
        InitStep::new("diagnostics", true, step_ok),
        InitStep::new("ui-startup", false, step_fails),
        InitStep::new("platform", false, step_ok),
    ];
    assert!(
        run_init_steps(&steps),
        "a window that failed to start must not decline the addon"
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
        on_addon_init(),
        "every step is pending, so initialisation must still succeed"
    );
    assert!(
        !candidate_window_available(),
        "the window does not exist yet, so no frame may be drawn into it"
    );
    on_addon_destroy();
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

/// The user-interface addon description, relative to this crate's manifest directory.
const UI_ADDON_CONF: &str = "../../packaging/fcitx5/rspinyin-ui.conf";

/// Reads a packaging file, or `None` when it is missing.
fn packaging_file(relative: &str) -> Option<String> {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).ok()
}

/// The value of `key` inside the `[section]` block of an addon description.
fn conf_value(conf: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in conf.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == format!("[{section}]");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

#[test]
fn test_ui_addon_conf_pins_what_fcitx5_resolves() {
    let Some(conf) = packaging_file(UI_ADDON_CONF) else {
        // The descriptor is delivered alongside the packaging work; until it exists the
        // assertion cannot be made, and silently passing would be worse than saying so.
        panic!("{UI_ADDON_CONF} is missing; the UI addon cannot be loaded without it");
    };

    // Fcitx5 finds the addon by category. `UI` is what makes it a candidate for the
    // active user interface at all; under any other category `updateAvailability()`
    // never sees it and the candidates stay with ClassicUI.
    assert_eq!(
        conf_value(&conf, "Addon", "Category").as_deref(),
        Some("UI"),
        "the UI addon must be registered under Category=UI"
    );
    // `Library=` names the file Fcitx5 `dlopen`s, without the `.so`: the installed
    // `libclassicui.so` is declared as `Library=libclassicui`, so the value is used
    // verbatim. It must therefore match the artifact Cargo actually produces, which is
    // `librspinyin_ui.so` — Cargo rejects a hyphen in `[lib] name`, so the underscore is
    // not a style choice.
    assert_eq!(
        conf_value(&conf, "Addon", "Library").as_deref(),
        Some("librspinyin_ui"),
        "Library= is what Fcitx5 dlopens; it must name this crate's cdylib"
    );
    assert_eq!(
        conf_value(&conf, "Addon", "Type").as_deref(),
        Some("SharedLibrary")
    );

    // A priority above ClassicUI's 0 is what wins the selection; the value is small and
    // explicit so a future addon can deliberately outrank this one.
    let priority: i32 = conf_value(&conf, "Addon", "UIPriority")
        .expect("UIPriority must be declared explicitly")
        .parse()
        .expect("UIPriority must be a number");
    assert!(
        priority > 0,
        "UIPriority must beat ClassicUI's default of 0, or the takeover never happens"
    );

    // The candidate window follows a physical keyboard, so declaring the type keeps the
    // addon out of the way on touch-only setups where it would have no cursor to follow.
    assert_eq!(
        conf_value(&conf, "Addon", "UIType").as_deref(),
        Some("PhysicalKeyboard")
    );

    // The floor must equal the engine descriptor's. The two addons are one product, so
    // a UI addon that loads where the engine does not — or the reverse — leaves the user
    // with half an input method, and the failure is silent either way.
    let dependency = conf_value(&conf, "Addon/Dependencies", "0").unwrap_or_default();
    let engine_conf = packaging_file(ENGINE_ADDON_CONF)
        .expect("the engine descriptor must ship; the two are one product");
    let engine_dependency = conf_value(&engine_conf, "Addon/Dependencies", "0").unwrap_or_default();
    assert_eq!(
        dependency, engine_dependency,
        "the UI and engine descriptors must declare the same core floor"
    );
    assert!(
        !dependency.is_empty(),
        "an empty floor would let the addon load against an unsupported fcitx5"
    );
}

/// The engine addon description, relative to this crate's manifest directory.
const ENGINE_ADDON_CONF: &str = "../../packaging/fcitx5/rspinyin.conf";
