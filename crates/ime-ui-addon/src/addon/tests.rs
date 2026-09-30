//! Tests for the user-interface addon's lifecycle.
//!
//! Everything here runs with no display server. The window is the in-memory
//! [`MockBackend`](crate::platform::mock::MockBackend), the platform probe is driven with
//! environments a test builds, and the one step that would open a connection on a machine
//! that has a display — `probe_platform` — is therefore never called from this file: the
//! tests install the probe's *answer* instead, which is the same call the step makes.
//!
//! The readiness flags and the backend slot are process-wide, so the tests that write them
//! are independent only under the project's test runner, which gives each test a process of
//! its own.

use std::fs;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, PreeditSpan,
    RectI, ScreenId, SpanKind, StatusStrip, UiCommand, UiFrame,
};
use ime_ui::channel::UiEventQueue;
use ime_ui::ui_thread::SurfaceUpdate;

use super::*;
use crate::platform::mock::{MockBackend, MockState, lock_state};
use crate::platform::{ProbeOutcome, SessionTier};

/// The logical size the tests pre-create the window at.
///
/// Smaller than the size the platform layer really uses, which is fine: a backend reports
/// its own geometry and the surface adopts it, so the mock only has to be big enough to
/// draw a candidate panel into.
const TEST_WINDOW_WIDTH_DP: u32 = 420;
const TEST_WINDOW_HEIGHT_DP: u32 = 200;

/// How long a test waits for the UI thread to draw something.
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a test samples the surface while waiting for it to settle.
const SETTLE_POLL: Duration = Duration::from_millis(20);

/// A step that succeeds.
fn step_ok() -> Result<(), ImeError> {
    Ok(())
}

/// A step that fails. The variant is irrelevant — the policy only reads `is_fatal`.
fn step_fails() -> Result<(), ImeError> {
    Err(ImeError::UiChannelClosed)
}

/// Installs a mock backend, and returns the state the test observes it through.
fn install_mock_backend() -> Arc<Mutex<MockState>> {
    let (backend, state) = MockBackend::new(TEST_WINDOW_WIDTH_DP, TEST_WINDOW_HEIGHT_DP, 1.0);
    let installed = crate::platform::install(ProbeOutcome::Ready {
        backend: Box::new(backend),
    });
    assert!(installed, "the mock backend must be installed");
    state
}

/// A surface that draws nothing, for the tests that drive the thread's lifecycle alone.
struct IdleSurface;

impl UiSurface for IdleSurface {
    fn event_fd(&self) -> Option<BorrowedFd<'_>> {
        None
    }

    fn apply(&mut self, _update: SurfaceUpdate) -> Result<(), ImeError> {
        Ok(())
    }

    fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
        Ok(())
    }

    fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
        Ok(None)
    }

    fn close(&mut self) -> Result<(), ImeError> {
        Ok(())
    }
}

/// Builds a start-up around an arbitrary factory body, so the deadline in
/// [`stop_ui_startup`] is reachable without the real window.
fn startup_with(body: impl FnOnce() -> Box<dyn UiSurface> + Send + 'static) -> Option<UiStartup> {
    let (ready_tx, ready) = channel();
    let config = UiThreadConfig {
        thread_name: UI_THREAD_NAME,
        ..UiThreadConfig::default()
    };
    let spawned = UiThread::spawn(config, move |_context| {
        let surface = body();
        let _ = ready_tx.send(());
        Ok(surface)
    });
    let thread = spawned.ok()?;
    Some(UiStartup::new(thread, ready))
}

/// An anchor at a caret in the middle of the screen.
fn anchor() -> Anchor {
    Anchor {
        cursor: RectI {
            x: 100,
            y: 200,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Below,
    }
}

/// One frame, as the engine builds one.
fn frame(revision: u32, preedit: &str, candidates: &[&str]) -> UiFrame {
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(index, text)| Candidate {
            index: index as u16 + 1,
            text: (*text).to_owned(),
            annotation: None,
            source: CandidateSource::Dict,
            score: 1.0,
            consumed_syllables: 1,
        })
        .collect();
    UiFrame {
        revision,
        preedit: Preedit {
            text: preedit.to_owned(),
            caret: preedit.len() as u32,
            spans: vec![PreeditSpan {
                start: 0,
                end: preedit.len() as u16,
                kind: SpanKind::Syllable,
            }],
        },
        candidates,
        page: PageState {
            current: 1,
            total: 1,
            page_size: 5,
        },
        status: StatusStrip::default(),
        anchor: anchor(),
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
    }
}

/// Waits until the surface has committed at least `count` frames.
fn wait_for_commits(state: &Arc<Mutex<MockState>>, count: usize) {
    let deadline = Instant::now() + FRAME_TIMEOUT;
    while Instant::now() < deadline {
        if lock_state(state).commits >= count {
            return;
        }
        thread::sleep(SETTLE_POLL);
    }
}

/// Waits until the candidate window is reported ready.
///
/// Longer than [`SURFACE_READY_DEADLINE`], because what these tests assert is that the
/// window arrives at all rather than how quickly: the build includes the font warm-up, whose
/// first call in a process reads the machine's fonts, and a slow machine may well miss the
/// load deadline the production path waits for.
fn wait_for_ready() -> bool {
    let deadline = Instant::now() + FRAME_TIMEOUT;
    while Instant::now() < deadline {
        if candidate_window_ready() {
            return true;
        }
        thread::sleep(SETTLE_POLL);
    }
    candidate_window_ready()
}

/// Waits until the surface stops changing, and answers the frame it settled on.
///
/// The appear motion advances by the difference between two renders, so a frame sampled
/// while it is still running would differ from the next one for a reason that has nothing
/// to do with the content. Waiting for three consecutive identical checksums is what makes
/// the comparison in [`test_the_pre_created_window_redraws_for_a_new_frame`] an assertion
/// about the frame rather than about the motion.
fn settled_checksum(state: &Arc<Mutex<MockState>>) -> u64 {
    let deadline = Instant::now() + FRAME_TIMEOUT;
    let mut previous = lock_state(state).checksum();
    let mut stable = 0;
    while Instant::now() < deadline {
        thread::sleep(SETTLE_POLL);
        let current = lock_state(state).checksum();
        if current == previous {
            stable += 1;
            if stable == 3 {
                return current;
            }
        } else {
            stable = 0;
            previous = current;
        }
    }
    previous
}

#[test]
fn test_init_steps_pin_the_documented_lifecycle() {
    let names: Vec<&str> = INIT_STEPS.iter().map(|step| step.name).collect();
    assert_eq!(
        names,
        [
            "diagnostics",
            "config",
            "platform",
            "ui-startup",
            "ui-registration",
        ]
    );
    // The order is load-bearing in one place: the start-up builds the window into the
    // surface the probe constructed, so a start-up that ran first would have nothing to
    // draw into and would leave the addon permanently unavailable.
    let position = |name: &str| names.iter().position(|step| *step == name);
    assert!(
        position("platform") < position("ui-startup"),
        "the probe must answer before the window is built: {names:?}"
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
fn test_ui_startup_with_a_backend_creates_the_window_and_destroy_releases_it() {
    let _state = install_mock_backend();
    start_ui_startup().expect("the UI thread can be started");
    assert!(
        wait_for_ready(),
        "the pre-created window must be reported ready: the host reads availability once \
         the addon is constructed, and a window that exists by then is one it can choose"
    );
    assert!(
        ui_impl::is_available(),
        "a window with a backend behind it is what the host may route the panel to"
    );
    on_addon_destroy();
    assert!(
        !candidate_window_ready(),
        "the window goes away with the addon, so a reload is told there is none"
    );
    assert!(
        !ui_impl::window_backend_available(),
        "the backend went away with it, so the next load has to probe again"
    );
    assert!(
        take_ui_startup().is_none(),
        "destroy must stop the start-up it made"
    );
}

#[test]
fn test_ui_startup_without_a_backend_starts_no_thread() {
    crate::platform::install(ProbeOutcome::Unsupported {
        tier: SessionTier::None,
        reason: "no display server names a surface",
    });
    assert!(
        start_ui_startup().is_ok(),
        "a session that can host no window is not a failed step"
    );
    assert!(
        take_ui_startup().is_none(),
        "there is no surface to build, so there is no thread to own one"
    );
    assert!(!candidate_window_ready());
    assert!(!ui_impl::is_available());
}

#[test]
fn test_stop_ui_startup_reports_whether_the_thread_stopped() {
    let quick = startup_with(|| Box::new(IdleSurface));
    assert!(quick.is_some(), "the test thread must start");
    if let Some(quick) = quick {
        assert!(
            stop_ui_startup(quick),
            "a thread parked on its wakeup counter must stop inside the deadline"
        );
    }
    // The second factory blocks well past the deadline, so the timeout branch is reached
    // whatever the machine's speed: the loop cannot even reach its wait, which is what a
    // surface that takes too long to build looks like.
    let slow = startup_with(|| {
        thread::sleep(UI_SHUTDOWN_TIMEOUT * 3);
        Box::new(IdleSurface)
    });
    assert!(slow.is_some(), "the test thread must start");
    if let Some(slow) = slow {
        assert!(
            !stop_ui_startup(slow),
            "a thread still building at the deadline must be reported and detached"
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

#[test]
#[ignore = "opens a display connection; the lab job runs it with DISPLAY set"]
fn test_on_addon_init_is_usable_and_leaves_no_window_behind() {
    // The real entry point, which the rest of this file deliberately avoids: it runs the
    // platform probe, so on a machine with a display server it creates the candidate
    // window. What the session offers decides whether one exists, which is why the
    // assertion is the invariant that holds either way.
    assert!(
        on_addon_init(),
        "only a failed diagnostics step may decline the addon"
    );
    if !ui_impl::window_backend_available() {
        assert!(
            !candidate_window_ready(),
            "a window is built into the surface the probe constructed, so no backend means \
             no window and the readiness flag has to say so"
        );
    }
    on_addon_destroy();
    assert!(
        take_ui_startup().is_none(),
        "destroy must stop whatever the load started"
    );
}

#[test]
fn test_register_takeover_reaches_the_host_once_the_window_exists() {
    let _state = install_mock_backend();
    start_ui_startup().expect("the UI thread can be started");
    assert!(wait_for_ready(), "the window must exist before the host is asked");
    let outcome = ui_impl::register_takeover();
    // The two outcomes that mean the host was never asked are the ones this test exists to
    // rule out. What the host answers when it *is* asked depends on the build: a pure-Rust
    // build has no ABI to ask, and the host-ABI build answers that no user interface of
    // ours is registered in a test process.
    assert_ne!(
        outcome,
        ui_impl::TakeoverOutcome::NotReady,
        "the window exists, so the takeover must not skip the host"
    );
    assert_ne!(
        outcome,
        ui_impl::TakeoverOutcome::Unsupported,
        "the backend exists, so the takeover must not report the fallback tier"
    );
    on_addon_destroy();
}

#[test]
fn test_the_pre_created_window_redraws_for_a_new_frame() {
    let state = install_mock_backend();
    start_ui_startup().expect("the UI thread can be started");
    let Some(startup) = take_ui_startup() else {
        panic!("the start-up must be running");
    };
    let thread = &startup.thread;
    // Whatever the surface draws before the first frame is the baseline. Reading it first
    // is what makes the counts below mean "a frame was drawn" rather than "something was
    // drawn": the comparison has to be between the two frames and nothing else.
    let _ = settled_checksum(&state);
    let baseline = lock_state(&state).commits;
    let show = UiCommand::Show {
        revision: 1,
        anchor: anchor(),
    };
    thread.send(show).expect("the pre-created window can be shown");
    let first_frame = UiCommand::Frame(Box::new(frame(1, "ni", &["你", "泥"])));
    thread.send(first_frame).expect("the first frame is posted");
    wait_for_commits(&state, baseline + 1);
    let before = settled_checksum(&state);
    assert!(
        lock_state(&state).painted_pixels() > 0,
        "the frame must be painted, not merely committed: the window is mostly transparent \
         reserve, so a surface handed a frame and drawing none of it would pass a check on \
         the commit count alone"
    );
    assert!(
        !lock_state(&state).region.is_empty(),
        "the placed panel is what the pointer may hit; the shadow reserve is not"
    );
    assert!(
        lock_state(&state).visible,
        "showing the window maps the surface the probe pre-created"
    );
    let second_frame = UiCommand::Frame(Box::new(frame(2, "ni hao", &["你好", "尼好"])));
    thread.send(second_frame).expect("the second frame is posted");
    wait_for_commits(&state, baseline + 2);
    let after = settled_checksum(&state);
    // The defect this catches cannot be seen any other way: a window that rasterizes its
    // first frame and is never asked to draw again passes every test that only asserts
    // "something was drawn".
    assert_ne!(
        before, after,
        "a new frame must reach the surface: a window that draws once leaves the two \
         frames identical"
    );
    assert!(stop_ui_startup(startup), "the UI thread stops when asked");
    on_addon_destroy();
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
