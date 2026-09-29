//! Tests for [`super`]'s compositor table and tier decision.
//!
//! This file is the body of the `tests` module declared in `compositor.rs`; it lives beside
//! that file only because the two together exceed the project's file-length limit. Every test
//! here drives the table with a process list and a listing the test writes out itself, so none
//! of them needs a compositor, a display server, or a machine that has one installed.

use super::*;

/// A listing in which both interfaces were read.
fn listing(layer_shell: bool, xdg_wm_base: bool) -> RegistryFacts {
    RegistryFacts {
        layer_shell: Some(layer_shell),
        xdg_wm_base: Some(xdg_wm_base),
    }
}

#[test]
fn test_identify_reads_the_compositor_of_the_session_s_own_kind() {
    let name = |process: &'static str| {
        identify(&[process], DisplayServer::Wayland).map(|found| found.name)
    };
    assert_eq!(name("weston"), Some("weston"));
    assert_eq!(
        identify(&["picom"], DisplayServer::X11).map(|found| found.name),
        Some("picom")
    );
    assert_eq!(
        identify(&["weston"], DisplayServer::X11),
        None,
        "a Wayland compositor is not the compositor of an X11 session"
    );
    assert_eq!(identify(&[], DisplayServer::Wayland), None);
    assert_eq!(
        identify(&["bash", "sshd", "systemd"], DisplayServer::X11),
        None
    );
}

#[test]
fn test_identify_takes_the_first_table_entry_that_is_running() {
    let found = identify(&["sway", "weston"], DisplayServer::Wayland).expect("sway is running");
    assert_eq!(found.name, "sway");
    assert_eq!(found.family, CompositorFamily::Wlroots);
}

#[test]
fn test_identify_places_each_registered_compositor_in_its_family() {
    let family = |process: &'static str| {
        identify(&[process], DisplayServer::Wayland).map(|found| found.family)
    };
    assert_eq!(family("sway"), Some(CompositorFamily::Wlroots));
    assert_eq!(family("hyprland"), Some(CompositorFamily::Wlroots));
    assert_eq!(family("kwin_wayland"), Some(CompositorFamily::KWin));
    assert_eq!(family("gnome-shell"), Some(CompositorFamily::Mutter));
    assert_eq!(family("mutter"), Some(CompositorFamily::Mutter));
    assert_eq!(family("weston"), Some(CompositorFamily::Other));
    assert_eq!(family("cosmic-comp"), Some(CompositorFamily::Other));
    assert_eq!(
        family("kwin_x11"),
        None,
        "kwin_x11 is the X11 session's compositor, and this session is a Wayland one"
    );
}

#[test]
fn test_identify_names_the_x11_compositors_the_acrylic_path_uses() {
    let name =
        |process: &'static str| identify(&[process], DisplayServer::X11).map(|found| found.name);
    assert_eq!(name("picom"), Some("picom"));
    assert_eq!(
        name("kwin_x11"),
        Some("kwin"),
        "the report names the compositor, not the binary"
    );
}

#[test]
fn test_tier_for_follows_the_listing_when_it_was_read() {
    assert_eq!(
        tier_for(CompositorFamily::Wlroots, listing(true, true)),
        WaylandTier::LayerShell
    );
    assert_eq!(
        tier_for(CompositorFamily::Wlroots, listing(false, true)),
        WaylandTier::Popup
    );
    assert_eq!(
        tier_for(CompositorFamily::Wlroots, listing(false, false)),
        WaylandTier::Fallback
    );
    assert_eq!(
        tier_for(CompositorFamily::KWin, listing(true, true)),
        WaylandTier::LayerShell,
        "the listing is direct evidence and outranks the table"
    );
    assert_eq!(
        tier_for(CompositorFamily::KWin, listing(false, true)),
        WaylandTier::Popup
    );
    assert_eq!(
        tier_for(CompositorFamily::Mutter, listing(false, true)),
        WaylandTier::CanvasPopup
    );
    assert_eq!(
        tier_for(CompositorFamily::Other, listing(true, true)),
        WaylandTier::LayerShell,
        "a compositor that offers layer-shell can be served by T1 whatever it is called"
    );
}

#[test]
fn test_tier_for_falls_back_to_the_table_when_the_listing_was_not_read() {
    let unread = RegistryFacts::default();
    assert_eq!(tier_for(CompositorFamily::KWin, unread), WaylandTier::Popup);
    assert_eq!(
        tier_for(CompositorFamily::Mutter, unread),
        WaylandTier::CanvasPopup
    );
    assert_eq!(
        tier_for(CompositorFamily::Wlroots, unread),
        WaylandTier::NotApplicable,
        "T1 and T2 are separated by a global that only the listing names"
    );
    assert_eq!(
        tier_for(CompositorFamily::Other, unread),
        WaylandTier::NotApplicable
    );
}

#[test]
fn test_tier_for_treats_a_half_read_listing_as_unread() {
    let half = RegistryFacts {
        layer_shell: Some(false),
        xdg_wm_base: None,
    };
    assert_eq!(
        tier_for(CompositorFamily::Wlroots, half),
        WaylandTier::NotApplicable,
        "one field is not enough to choose between the tiers"
    );
    assert!(
        !layer_shell_for(CompositorFamily::Wlroots, half),
        "the field that was read is still a reading"
    );
}

#[test]
fn test_layer_shell_comes_from_the_listing_or_from_the_family() {
    let unread = RegistryFacts::default();
    assert!(layer_shell_for(CompositorFamily::Wlroots, unread));
    assert!(!layer_shell_for(CompositorFamily::KWin, unread));
    assert!(!layer_shell_for(CompositorFamily::Mutter, unread));
    assert!(!layer_shell_for(CompositorFamily::Other, unread));
    assert!(
        !layer_shell_for(CompositorFamily::Wlroots, listing(false, true)),
        "a wlroots compositor can be built without the interface, and then the listing wins"
    );
    assert!(layer_shell_for(CompositorFamily::KWin, listing(true, true)));
}

#[test]
fn test_registry_facts_report_a_half_read_listing_as_unread() {
    let unread = RegistryFacts::default();
    let half = RegistryFacts {
        layer_shell: Some(true),
        xdg_wm_base: None,
    };
    let read = RegistryFacts {
        layer_shell: Some(true),
        xdg_wm_base: Some(false),
    };
    assert!(!unread.was_read());
    assert!(!half.was_read(), "one field is not a listing");
    assert!(read.was_read());
}
