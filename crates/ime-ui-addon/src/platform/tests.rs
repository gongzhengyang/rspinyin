//! Tests for the platform probe, the backend slot and the window's pre-created size.
//!
//! Every test here runs with no display server: the environment is a value, the X11 rung is
//! driven with a display name that cannot be parsed (so the client fails before it opens a
//! socket), and the backend that does get constructed is the in-memory
//! [`MockBackend`](super::mock::MockBackend).
//!
//! The backend slot and the availability flag are process-wide, so the tests that write
//! them are independent only under the project's test runner, which gives each test a
//! process of its own. The same is true of the `ui_impl` tests that write the flag.

use std::env;

use ime_ui::layout::{self, GridLayout};

use super::mock::{MockBackend, lock_state};
use super::*;

/// The largest page the design allows, which the pre-created window has to fit.
const LARGEST_PAGE: usize = 9;

/// The metrics of the real `candidate.slint`, which every size assertion builds on.
fn metrics() -> &'static layout::Metrics {
    layout::metrics().expect("ui/candidate.slint declares a readable metrics block")
}

/// A backend that answers, for the tests that need a probe to succeed.
fn ready_outcome() -> ProbeOutcome {
    let (backend, _state) = MockBackend::new(64, 32, 1.0);
    ProbeOutcome::Ready {
        backend: Box::new(backend),
    }
}

#[test]
fn test_session_tier_prefers_x11_and_falls_back_in_order() {
    let both = Environment::new(Some(":0"), Some("wayland-0"));
    assert_eq!(
        both.tier(),
        SessionTier::X11,
        "an XWayland session offers both, and X11 is the tier the ladder tries first"
    );
    let wayland = Environment::new(None, Some("wayland-0"));
    assert_eq!(wayland.tier(), SessionTier::Wayland);
    assert_eq!(Environment::new(None, None).tier(), SessionTier::None);
    assert_eq!(SessionTier::X11.name(), "x11");
    assert_eq!(SessionTier::Wayland.name(), "wayland");
    assert_eq!(SessionTier::None.name(), "none");
}

#[test]
fn test_environment_treats_an_empty_name_as_unset() {
    // An empty `$DISPLAY` reaches no server, and reporting it as a name would make the
    // probe fail with the wrong reason: "the X server could not be reached" instead of
    // "this session names no display server".
    let environment = Environment::new(Some(""), Some(""));
    assert_eq!(environment.display(), None);
    assert_eq!(environment.wayland_display(), None);
    assert_eq!(environment.tier(), SessionTier::None);
}

#[test]
fn test_environment_from_process_reads_the_documented_variables() {
    // The assertion holds in every environment, so the test needs no display server: what
    // it pins is that the two names are read from the two variables the module documents,
    // and that an empty one counts as unset.
    let environment = Environment::from_process();
    let expected = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
    assert_eq!(environment.display(), expected("DISPLAY").as_deref());
    let wayland = expected("WAYLAND_DISPLAY");
    assert_eq!(environment.wayland_display(), wayland.as_deref());
}

#[test]
fn test_probe_without_a_display_reports_the_fallback_tier() {
    let outcome = probe(&Environment::new(None, None));
    assert_eq!(outcome.backend_id(), None, "there is nothing to draw into");
    assert_eq!(outcome.tier(), Some(SessionTier::None));
    assert!(
        outcome.into_backend().is_none(),
        "an unsupported probe hands over no surface"
    );
}

#[test]
fn test_probe_with_a_wayland_only_session_does_not_pretend_to_have_a_window() {
    // The Wayland rungs are not part of this build, so the honest answer is the fallback
    // tier. Reporting a backend here would suppress ClassicUI with nothing drawing in its
    // place, which is the failure mode the availability flag exists to prevent.
    let outcome = probe(&Environment::new(None, Some("wayland-0")));
    assert_eq!(outcome.backend_id(), None);
    assert_eq!(outcome.tier(), Some(SessionTier::Wayland));
    let line = outcome
        .diagnostic()
        .expect("an unsupported probe records a line");
    assert!(
        line.starts_with(crate::ui_impl::NO_BACKEND_CODE),
        "the frozen code is the line's first field: {line}"
    );
    assert!(
        line.contains("tier=wayland"),
        "the tier tells an operator which rung was missing: {line}"
    );
}

#[test]
fn test_probe_with_an_unreachable_x_server_reports_the_stable_code() {
    // A display name without a colon cannot be parsed, so the client fails before it opens
    // a socket: the test needs no X server, and the branch it reaches is the one a session
    // with a stale `$DISPLAY` reaches.
    let outcome = probe(&Environment::new(Some("rspinyin-invalid"), None));
    assert_eq!(outcome.tier(), Some(SessionTier::X11));
    let line = outcome.diagnostic().expect("a failed probe records a line");
    assert!(line.starts_with(crate::ui_impl::NO_BACKEND_CODE), "{line}");
    assert!(line.contains("tier=x11"), "{line}");
}

#[test]
fn test_probe_outcome_records_nothing_when_a_backend_was_found() {
    let outcome = ready_outcome();
    assert_eq!(outcome.backend_id(), Some("mock"));
    assert_eq!(outcome.tier(), None);
    assert_eq!(
        outcome.diagnostic(),
        None,
        "a session that can host the window is reported by the lifecycle summary, not by a \
         code of its own"
    );
    assert!(
        outcome.into_backend().is_some(),
        "the surface is handed to whoever took the outcome"
    );
}

#[test]
fn test_install_without_a_backend_clears_the_availability_flag() {
    let installed = install(ProbeOutcome::Unsupported {
        tier: SessionTier::None,
        reason: "no display server names a surface",
    });
    assert!(!installed, "a session with no backend is not available");
    assert!(
        !crate::ui_impl::window_backend_available(),
        "the host must keep drawing its own candidates"
    );
    assert!(
        take_backend().is_none(),
        "an unsupported probe stores no surface"
    );
}

#[test]
fn test_install_with_a_backend_raises_the_flag_and_hands_the_surface_over() {
    let (backend, state) = MockBackend::new(64, 32, 1.0);
    let installed = install(ProbeOutcome::Ready {
        backend: Box::new(backend),
    });
    assert!(installed, "a constructed backend makes the addon available");
    assert!(crate::ui_impl::window_backend_available());
    assert_eq!(backend_id(), Some("mock"), "the summary names the backend");
    let taken = take_backend().expect("the probe's surface is waiting to be taken");
    assert_eq!(taken.backend_id(), "mock");
    assert!(
        take_backend().is_none(),
        "a surface has one owner: a second start-up must not be handed a second one"
    );
    assert!(
        !lock_state(&state).visible,
        "the pre-created window is not mapped until the first frame is shown"
    );
}

#[test]
fn test_initial_window_size_covers_the_largest_panel_the_design_allows() {
    let metrics = metrics();
    let size = initial_window_size(metrics, 1.0);
    // The window cannot be resized after it is created, so the assertion is the property
    // that matters: it is at least as large as the largest panel a frame can ask for.
    let grid = GridLayout {
        cols: metrics.max_per_row,
        rows: LARGEST_PAGE.div_ceil(usize::from(metrics.min_per_row.max(1))) as u8,
        pages: 1,
        overflow: false,
    };
    let widest = layout::container_size(&grid, metrics.max_width, metrics.max_width, metrics);
    let needed = layout::window_size(widest.width, widest.height, 1.0, metrics);
    assert!(
        size.0 >= needed.0 && size.1 >= needed.1,
        "the pre-created window {size:?} must fit the largest panel {needed:?}"
    );
    // The width is the design's own cap on the panel plus the shadow reserve on both sides,
    // which is what makes the assertion above hold for a page of maximally wide cells.
    let from_cap = layout::window_size(metrics.max_width, metrics.header_height, 1.0, metrics).0;
    assert_eq!(size.0, from_cap);
}

#[test]
fn test_initial_window_size_scales_and_survives_an_unusable_scale() {
    let metrics = metrics();
    let single = initial_window_size(metrics, 1.0);
    assert_eq!(
        initial_window_size(metrics, 2.0),
        (single.0 * 2, single.1 * 2),
        "the window is created at the output's ratio"
    );
    // A ratio that cannot describe a surface is corrected rather than propagated, exactly
    // as `layout::window_size` corrects it: a window at the wrong size is usable, a window
    // that cannot be created is not.
    for unusable in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            initial_window_size(metrics, unusable),
            single,
            "scale {unusable} must fall back to 1.0"
        );
    }
    assert!(single.0 > 0 && single.1 > 0);
}

#[test]
#[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
fn test_probe_from_process_agrees_with_the_session_it_reads() {
    // The one entry point the lifecycle calls, and the only one this file cannot drive
    // hermetically: it reads the real environment, so on a machine that has a display
    // server it opens a connection and creates the candidate window.
    let environment = Environment::from_process();
    let outcome = probe_from_process();
    match environment.tier() {
        SessionTier::X11 => assert!(
            outcome.backend_id().is_some(),
            "a session with a reachable X server must get a backend: {outcome:?}"
        ),
        tier => assert_eq!(
            outcome.tier(),
            Some(tier),
            "a session with no X server is answered with the tier it offered: {outcome:?}"
        ),
    }
}
