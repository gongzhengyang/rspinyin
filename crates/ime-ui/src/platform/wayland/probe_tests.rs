//! Tests for [`super`]'s tier probe and ladder.
//!
//! This file is the body of the `tests` module declared in `probe.rs`; it lives beside that
//! file only because the two together exceed the project's file-length limit. Every test here
//! drives the ladder with an explicit elapsed time, so none of them waits for a clock and none
//! of them needs a compositor.

use super::*;

/// A registry as a wlroots compositor would announce it.
fn wlroots_globals() -> Vec<Global> {
    vec![
        Global::new(1, "wl_compositor", 6),
        Global::new(2, "wl_shm", 1),
        Global::new(3, "wl_seat", 8),
        Global::new(4, "xdg_wm_base", 6),
        Global::new(5, LAYER_SHELL_INTERFACE, 4),
    ]
}

/// A registry as a plain xdg-shell compositor would announce it.
fn xdg_globals() -> Vec<Global> {
    vec![
        Global::new(1, "wl_compositor", 4),
        Global::new(2, "wl_shm", 1),
        Global::new(3, "xdg_wm_base", 2),
    ]
}

fn capabilities(globals: &[Global]) -> Capabilities {
    Capabilities::from_globals(globals)
}

#[test]
fn test_capabilities_read_the_registry() {
    let wlroots = capabilities(&wlroots_globals());
    assert!(wlroots.layer_shell);
    assert!(wlroots.xdg_wm_base);
    assert!(wlroots.input_region);
    assert!(wlroots.shm);
    assert!(wlroots.usable());
    let xdg = capabilities(&xdg_globals());
    assert!(!xdg.layer_shell);
    assert!(xdg.xdg_wm_base);
    assert!(xdg.input_region);
}

#[test]
fn test_capabilities_without_shm_are_unusable() {
    let globals = vec![
        Global::new(1, "wl_compositor", 6),
        Global::new(2, LAYER_SHELL_INTERFACE, 4),
    ];
    let reduced = capabilities(&globals);
    assert!(!reduced.shm);
    assert!(!reduced.usable());
    assert_eq!(probe_tier(&reduced), Tier::Fallback);
}

#[test]
fn test_capabilities_need_a_new_compositor_to_shape_the_input_region() {
    let globals = vec![
        Global::new(1, "wl_compositor", 3),
        Global::new(2, "wl_shm", 1),
        Global::new(3, LAYER_SHELL_INTERFACE, 4),
    ];
    let reduced = capabilities(&globals);
    assert!(
        !reduced.input_region,
        "version 3 predates set_input_region"
    );
    assert_eq!(probe_tier(&reduced), Tier::LayerShell);
}

#[test]
fn test_probe_tier_prefers_layer_shell_and_falls_back_without_either() {
    assert_eq!(
        probe_tier(&capabilities(&wlroots_globals())),
        Tier::LayerShell
    );
    assert_eq!(probe_tier(&capabilities(&xdg_globals())), Tier::Popup);
    let bare = vec![Global::new(1, "wl_shm", 1)];
    assert_eq!(probe_tier(&capabilities(&bare)), Tier::Fallback);
}

#[test]
fn test_compositor_kind_is_inferred_from_the_registry() {
    assert_eq!(
        CompositorKind::from_capabilities(&capabilities(&wlroots_globals())),
        CompositorKind::WlrootsFamily
    );
    assert_eq!(
        CompositorKind::from_capabilities(&capabilities(&xdg_globals())),
        CompositorKind::XdgShellOnly
    );
    assert_eq!(
        CompositorKind::from_capabilities(&capabilities(&[])),
        CompositorKind::Unsupported
    );
    assert_eq!(CompositorKind::XdgShellOnly.label(), "xdg-shell");
}

#[test]
fn test_ladder_confirms_the_tier_when_a_configure_arrives() {
    let mut ladder = TierLadder::begin(capabilities(&wlroots_globals()));
    assert_eq!(ladder.tier(), Tier::LayerShell);
    ladder.arm(Duration::from_millis(10));
    assert_eq!(
        ladder.on_event(
            &WireEvent::PointerMotion { x: 0.0, y: 0.0 },
            Duration::from_millis(20)
        ),
        LadderStep::Pending,
        "an unrelated event does not confirm the tier"
    );
    assert_eq!(
        ladder.on_event(
            &WireEvent::LayerConfigure {
                width: 300,
                height: 70
            },
            Duration::from_millis(20)
        ),
        LadderStep::Confirmed(Tier::LayerShell)
    );
    assert!(ladder.is_confirmed());
    assert!(
        ladder.deadline().is_none(),
        "a confirmed tier has no budget"
    );
    assert!(ladder.attempts().is_empty());
}

#[test]
fn test_ladder_promotes_after_the_configure_budget_expires() {
    let mut ladder = TierLadder::begin(capabilities(&wlroots_globals()));
    ladder.arm(Duration::ZERO);
    assert_eq!(
        ladder.on_timeout(CONFIGURE_TIMEOUT - Duration::from_millis(1)),
        LadderStep::Pending,
        "the budget is not spent a millisecond early"
    );
    assert_eq!(
        ladder.on_timeout(CONFIGURE_TIMEOUT),
        LadderStep::Promote {
            failed: Tier::LayerShell,
            reason: TierFailure::ConfigureTimeout,
            next: Tier::Popup,
        }
    );
    assert_eq!(ladder.tier(), Tier::Popup);
    assert_eq!(ladder.attempts().len(), 1);
    assert_eq!(
        ladder.attempts()[0].failure.code(),
        "platform/wayland/configure-timeout"
    );
}

#[test]
fn test_ladder_without_an_armed_budget_never_times_out() {
    let mut ladder = TierLadder::begin(capabilities(&wlroots_globals()));
    assert_eq!(ladder.deadline(), None);
    assert_eq!(
        ladder.on_timeout(Duration::from_secs(30)),
        LadderStep::Pending,
        "time cannot run out on a surface that does not exist yet"
    );
}

#[test]
fn test_ladder_walks_all_three_tiers_and_then_falls_back() {
    let mut ladder = TierLadder::begin(capabilities(&wlroots_globals()));
    ladder.arm(Duration::ZERO);
    assert!(matches!(
        ladder.on_event(&WireEvent::LayerClosed, Duration::from_millis(1)),
        LadderStep::Promote {
            next: Tier::Popup,
            ..
        }
    ));
    assert!(matches!(
        ladder.on_event(&WireEvent::PopupDone, Duration::from_millis(2)),
        LadderStep::Promote {
            next: Tier::CanvasPopup,
            ..
        }
    ));
    assert_eq!(
        ladder.on_event(&WireEvent::PopupDone, Duration::from_millis(3)),
        LadderStep::Fallback {
            reason: TierFailure::PopupDone
        }
    );
    assert_eq!(ladder.tier(), Tier::Fallback);
    assert_eq!(ladder.attempts().len(), 3);
    assert_eq!(ladder.attempts()[2].tier, Tier::CanvasPopup);
}

#[test]
fn test_ladder_skips_the_popup_tiers_without_xdg_shell() {
    let globals = vec![
        Global::new(1, "wl_compositor", 6),
        Global::new(2, "wl_shm", 1),
        Global::new(3, LAYER_SHELL_INTERFACE, 4),
    ];
    let mut ladder = TierLadder::begin(capabilities(&globals));
    ladder.arm(Duration::ZERO);
    assert_eq!(
        ladder.on_timeout(CONFIGURE_TIMEOUT),
        LadderStep::Fallback {
            reason: TierFailure::ConfigureTimeout
        },
        "a compositor without xdg_wm_base has nothing to promote to"
    );
}

#[test]
fn test_ladder_fails_the_tier_when_the_keyboard_is_taken() {
    let mut ladder = TierLadder::begin(capabilities(&wlroots_globals()));
    ladder.arm(Duration::ZERO);
    let step = ladder.on_event(&WireEvent::KeyboardEnter, Duration::from_millis(5));
    assert!(matches!(
        step,
        LadderStep::Promote {
            reason: TierFailure::FocusTaken,
            next: Tier::Popup,
            ..
        }
    ));
    assert_eq!(TierFailure::FocusTaken.code(), "platform/wayland/focus-taken");
}

#[test]
fn test_ladder_keeps_a_confirmed_tier_that_is_dismissed() {
    let mut ladder = TierLadder::begin(capabilities(&xdg_globals()));
    ladder.arm(Duration::ZERO);
    assert_eq!(
        ladder.on_event(
            &WireEvent::PopupConfigure {
                x: 0,
                y: 0,
                width: 300,
                height: 70
            },
            Duration::from_millis(1)
        ),
        LadderStep::Confirmed(Tier::Popup)
    );
    assert_eq!(
        ladder.on_event(&WireEvent::PopupDone, Duration::from_millis(2)),
        LadderStep::Pending,
        "a dismissed popup is a hidden window, not a failed tier"
    );
    assert_eq!(ladder.tier(), Tier::Popup);
    assert!(ladder.attempts().is_empty());
}

#[test]
fn test_ladder_reports_a_protocol_error_at_any_time() {
    let mut ladder = TierLadder::begin(capabilities(&xdg_globals()));
    ladder.arm(Duration::ZERO);
    let _ = ladder.on_event(
        &WireEvent::PopupConfigure {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
        Duration::ZERO,
    );
    assert!(matches!(
        ladder.on_event(&WireEvent::DisplayError, Duration::from_millis(1)),
        LadderStep::Promote {
            reason: TierFailure::ProtocolError,
            ..
        }
    ));
}
