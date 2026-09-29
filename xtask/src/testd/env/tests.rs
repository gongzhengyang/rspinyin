//! Tests for [`super`]: the assembly of an observation into the report.
//!
//! This file is the body of the `tests` module declared in `env.rs`; it lives beside that file
//! only because the two together exceed the project's file-length limit.
//!
//! The observations below are written out by hand, one per machine the report has to describe
//! correctly: a Wayland session under Weston, an X11 session, a machine with no display at all
//! -- which is what the `xtask` test job itself looks like -- and a named display that could
//! not be opened. Nothing here needs a display server, a compositor or Fcitx5, and the two
//! tests that do read this machine assert only what holds on any of them.

use std::path::PathBuf;

use super::*;

/// A process table row, as the reader would have built it.
fn row(pid: u32, ppid: u32, comm: &str) -> ProcessEntry {
    ProcessEntry {
        pid,
        ppid,
        comm: comm.to_owned(),
    }
}

/// A descriptor as the reader would have read it.
fn descriptor(name: &str, text: &str) -> AddonDescriptor {
    AddonDescriptor {
        name: name.to_owned(),
        text: text.to_owned(),
    }
}

/// An observation of a machine with nothing read: no display, no process table, no fonts.
fn nothing() -> Observation {
    Observation::default()
}

/// An observation of a Wayland session under Weston, the compositor of the development
/// machine, with both addons loaded and the readings a working machine would have.
fn weston_session() -> Observation {
    Observation {
        wayland_display: Some("wayland-0".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "weston"), row(11, 10, "weston-keyboard")]),
        own_pid: 99,
        fcitx5_version: Some("5.1.7".to_owned()),
        cjk_fonts: Some("Noto Sans CJK: style=Regular\nNoto Serif CJK: style=Regular\n".to_owned()),
        addon_descriptors: vec![
            descriptor("rspinyin", "[Addon]\nCategory=InputMethod\n"),
            descriptor("rspinyin-ui", "[Addon]\nCategory=UI\n"),
        ],
        session_log: Some("Loaded addon rspinyin\nLoaded addon rspinyin-ui\n".to_owned()),
        data_dir: Some(PathBuf::from("/home/gong/.local/share/rspinyin")),
        data_dir_writable: true,
        ..Observation::default()
    }
}

#[test]
fn test_from_observation_without_a_display_reports_headless() {
    // This is what an `xtask` test run looks like: no `DISPLAY`, no `WAYLAND_DISPLAY`, nothing
    // to connect to. It is not a failure, and a display that was never named is not a gap.
    let report = EnvCapabilities::from_observation(&nothing());
    assert_eq!(report.display_server, DisplayServer::Headless);
    assert_eq!(report.tier, WaylandTier::NotApplicable);
    assert!(!report.argb_visual);
    assert!(!report.compositor_present);
    assert_eq!(report.compositor, None);
    assert!(!report.wlr_layer_shell);
    assert!(!report.has_gap(ProbeGap::X11Unreachable));
    assert!(!report.has_gap(ProbeGap::CompositorUnknown));
}

#[test]
fn test_from_observation_records_a_display_it_could_not_open() {
    // A named display that cannot be opened is an anomaly worth a gap: every X11 reading
    // below it is unknown rather than false.
    let observation = Observation {
        display: Some(":0".to_owned()),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.display_server, DisplayServer::Headless);
    assert!(report.has_gap(ProbeGap::X11Unreachable));
    assert_eq!(report.display.as_deref(), Some(":0"));
    assert!(
        !report.argb_visual,
        "no server was reached, so no visual is known"
    );
}

#[test]
fn test_from_observation_reads_an_x11_session_from_the_connection() {
    let observation = Observation {
        display: Some(":0".to_owned()),
        x11: Some(X11Facts {
            argb_visual: true,
            compositor_present: true,
        }),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.display_server, DisplayServer::X11);
    assert!(report.argb_visual);
    assert!(report.compositor_present);
    assert!(!report.has_gap(ProbeGap::X11Unreachable));
    assert_eq!(report.tier, WaylandTier::NotApplicable);
    assert!(
        !report.wlr_layer_shell,
        "an X11 session has no Wayland tier"
    );
}

#[test]
fn test_from_observation_prefers_wayland_when_both_are_present() {
    let observation = Observation {
        display: Some(":0".to_owned()),
        x11: Some(X11Facts {
            argb_visual: true,
            compositor_present: false,
        }),
        wayland_display: Some("wayland-0".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "weston")]),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(
        report.display_server,
        DisplayServer::Wayland,
        "a session with a Wayland socket is a Wayland session"
    );
    assert_eq!(report.compositor.as_deref(), Some("weston"));
    assert!(
        report.argb_visual,
        "the X11 readings are still reported for the X11 backend"
    );
    assert!(!report.compositor_present);
}

#[test]
fn test_from_observation_reports_weston_as_outside_the_four_tiers() {
    // The development machine's shape, and the baseline `features.md` 0.5.5 registers: Weston
    // is `ASM-13`'s case, so no tier of the ladder applies to it.
    let report = EnvCapabilities::from_observation(&weston_session());
    assert_eq!(report.display_server, DisplayServer::Wayland);
    assert_eq!(report.compositor.as_deref(), Some("weston"));
    assert_eq!(report.tier, WaylandTier::NotApplicable);
    assert!(!report.wlr_layer_shell);
    assert!(report.has_gap(ProbeGap::WaylandRegistryUnread));
    assert!(!report.has_gap(ProbeGap::CompositorUnknown));
}

#[test]
fn test_from_observation_cannot_pick_a_wlroots_tier_without_the_registry() {
    let observation = Observation {
        wayland_display: Some("wayland-1".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "sway")]),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.compositor.as_deref(), Some("sway"));
    assert_eq!(report.tier, WaylandTier::NotApplicable);
    assert!(
        report.wlr_layer_shell,
        "a wlroots compositor offers the layer-shell interface"
    );
    assert!(report.has_gap(ProbeGap::WaylandRegistryUnread));
}

#[test]
fn test_from_observation_takes_the_tier_from_the_registry_when_it_was_read() {
    let observation = Observation {
        wayland_display: Some("wayland-1".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "sway")]),
        registry: RegistryFacts {
            layer_shell: Some(true),
            xdg_wm_base: Some(true),
        },
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.tier, WaylandTier::LayerShell);
    assert!(report.wlr_layer_shell);
    assert!(!report.has_gap(ProbeGap::WaylandRegistryUnread));
}

#[test]
fn test_from_observation_records_a_wayland_session_with_no_known_compositor() {
    let observation = Observation {
        wayland_display: Some("wayland-1".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "some-shell")]),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.compositor, None);
    assert!(report.has_gap(ProbeGap::CompositorUnknown));
    assert_eq!(report.tier, WaylandTier::NotApplicable);
}

#[test]
fn test_from_observation_reads_the_fcitx5_development_package() {
    let installed = EnvCapabilities::from_observation(&weston_session());
    assert_eq!(installed.fcitx5_version.as_deref(), Some("5.1.7"));
    assert!(installed.dev_packages);

    let absent = EnvCapabilities::from_observation(&nothing());
    assert_eq!(
        absent.fcitx5_version, None,
        "a machine without fcitx5 is not a failure"
    );
    assert!(!absent.dev_packages);
}

#[test]
fn test_from_observation_counts_cjk_fonts_and_records_the_gap_without_fc_list() {
    let counted = EnvCapabilities::from_observation(&weston_session());
    assert_eq!(counted.cjk_font_count, 2);
    assert!(!counted.has_gap(ProbeGap::CjkFontCountUnknown));

    let unknown = EnvCapabilities::from_observation(&nothing());
    assert_eq!(unknown.cjk_font_count, 0);
    assert!(unknown.has_gap(ProbeGap::CjkFontCountUnknown));
}

#[test]
fn test_from_observation_counts_agents_outside_the_probe_s_own_ancestry() {
    let observation = Observation {
        processes: Some(vec![
            row(7, 1, "cargo-nextest"),
            row(9, 7, "cargo"),
            row(50, 1, "cargo"),
            row(51, 50, "rustc"),
        ]),
        own_pid: 9,
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(
        report.concurrent_agents, 2,
        "the harness the probe runs under is not another agent"
    );
    assert!(!report.has_gap(ProbeGap::AgentCountUnknown));
}

#[test]
fn test_from_observation_reports_a_clean_machine_as_zero_agents() {
    let observation = Observation {
        processes: Some(vec![row(7, 1, "cargo-nextest"), row(9, 7, "cargo")]),
        own_pid: 9,
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(report.concurrent_agents, 0);
}

#[test]
fn test_from_observation_records_the_gap_when_the_process_table_is_unreadable() {
    let report = EnvCapabilities::from_observation(&nothing());
    assert_eq!(report.concurrent_agents, 0);
    assert!(report.has_gap(ProbeGap::AgentCountUnknown));
}

#[test]
fn test_from_observation_reads_both_addons_and_their_categories() {
    let report = EnvCapabilities::from_observation(&weston_session());
    let names: Vec<&str> = report
        .addons_loaded
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["rspinyin", "rspinyin-ui"]);
    assert!(report.addons_loaded.iter().all(|addon| addon.loaded));
    assert_eq!(report.addons_loaded[0].category, "InputMethod");
    assert_eq!(report.addons_loaded[1].category, "UI");
    assert!(report.missing_addons().is_empty());
    assert!(!report.has_gap(ProbeGap::AddonDescriptorsUnread));
    assert!(!report.has_gap(ProbeGap::SessionLogAbsent));
}

#[test]
fn test_missing_addons_names_the_one_a_session_did_not_load() {
    let observation = Observation {
        session_log: Some("Loaded addon rspinyin\n".to_owned()),
        ..Observation::default()
    };
    let report = EnvCapabilities::from_observation(&observation);
    assert_eq!(
        report.missing_addons(),
        ["rspinyin-ui"],
        "half an input method is a missing addon, not a quiet success"
    );
    assert_eq!(report.addons_loaded[0].category, "InputMethod");
    assert_eq!(report.addons_loaded[1].category, "UI");
}

#[test]
fn test_missing_addons_is_empty_without_a_session_log() {
    let report = EnvCapabilities::from_observation(&nothing());
    assert!(report.has_gap(ProbeGap::SessionLogAbsent));
    assert!(report.addons_loaded.iter().all(|addon| !addon.loaded));
    assert!(
        report.missing_addons().is_empty(),
        "an unknown must not be reported as a missing addon"
    );
}

#[test]
fn test_from_observation_of_a_machine_with_nothing_read_records_every_unknown() {
    let report = EnvCapabilities::from_observation(&nothing());
    let codes: Vec<&str> = report.gaps.iter().map(|gap| gap.code()).collect();
    assert_eq!(
        codes,
        [
            ProbeGap::CjkFontCountUnknown.code(),
            ProbeGap::AgentCountUnknown.code(),
            ProbeGap::AddonDescriptorsUnread.code(),
            ProbeGap::SessionLogAbsent.code(),
            ProbeGap::DataDirUnknown.code(),
        ],
        "the gaps are listed in the order the probe finds them"
    );
}

#[test]
fn test_probe_gap_codes_are_unique_and_shaped_like_diagnostic_codes() {
    const ALL: [ProbeGap; 8] = [
        ProbeGap::X11Unreachable,
        ProbeGap::CompositorUnknown,
        ProbeGap::WaylandRegistryUnread,
        ProbeGap::CjkFontCountUnknown,
        ProbeGap::AgentCountUnknown,
        ProbeGap::AddonDescriptorsUnread,
        ProbeGap::SessionLogAbsent,
        ProbeGap::DataDirUnknown,
    ];
    let mut codes: Vec<&str> = ALL.iter().map(|gap| gap.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), ALL.len(), "two gaps share a code");
    for gap in ALL {
        let code = gap.code();
        assert_eq!(
            code.split('/').count(),
            3,
            "{code} is not domain/action/reason"
        );
        assert!(
            !gap.needs().is_empty(),
            "{code} says nothing about what would settle it"
        );
    }
}

#[test]
fn test_labels_are_the_ones_the_case_tables_use() {
    assert_eq!(DisplayServer::Wayland.label(), "wayland");
    assert_eq!(DisplayServer::X11.label(), "x11");
    assert_eq!(DisplayServer::Headless.label(), "headless");
    assert_eq!(WaylandTier::LayerShell.label(), "T1");
    assert_eq!(WaylandTier::Popup.label(), "T2");
    assert_eq!(WaylandTier::CanvasPopup.label(), "T3");
    assert_eq!(WaylandTier::Fallback.label(), "T4");
    assert_eq!(WaylandTier::NotApplicable.label(), "not-applicable");
}

#[test]
fn test_lines_carry_every_reading_and_every_gap() {
    let report = EnvCapabilities::from_observation(&weston_session());
    let text = report.lines().join("\n");
    for expected in [
        "display_server: wayland",
        "wayland_display: wayland-0",
        "compositor: weston",
        "tier: not-applicable",
        "wlr_layer_shell: false",
        "fcitx5_version: 5.1.7",
        "dev_packages: true",
        "cjk_font_count: 2",
        "concurrent_agents: 0",
        "addon rspinyin: category InputMethod, loaded true",
        "addon rspinyin-ui: category UI, loaded true",
    ] {
        assert!(
            text.contains(expected),
            "the report is missing `{expected}`:\n{text}"
        );
    }
    for gap in &report.gaps {
        assert!(
            text.contains(gap.code()),
            "the report is missing {}",
            gap.code()
        );
    }
}

#[test]
fn test_probe_reports_a_report_that_agrees_with_itself() {
    // Runs wherever the suite runs, including a machine with no display at all, which is what
    // the `xtask` job looks like. It asserts the report's internal consistency, not the
    // machine's capabilities.
    let report = EnvCapabilities::probe();
    assert_eq!(report.dev_packages, report.fcitx5_version.is_some());
    assert_eq!(report.addons_loaded.len(), EXPECTED_ADDONS.len());
    for addon in &report.addons_loaded {
        assert!(!addon.category.is_empty(), "{addon:?} has no category");
    }
    if report.display_server != DisplayServer::Wayland {
        assert_eq!(
            report.tier,
            WaylandTier::NotApplicable,
            "only a Wayland session has a Wayland tier"
        );
    }
    if report.tier != WaylandTier::NotApplicable {
        assert_eq!(report.display_server, DisplayServer::Wayland);
    }
    assert!(!report.lines().is_empty());
}

#[test]
fn test_probe_with_session_log_reads_the_addon_verdicts_out_of_the_log() {
    let report = EnvCapabilities::probe_with_session_log("Loaded addon rspinyin\n");
    assert_eq!(report.missing_addons(), ["rspinyin-ui"]);
    let engine = report
        .addons_loaded
        .iter()
        .find(|addon| addon.name == "rspinyin");
    assert!(
        engine.is_some_and(|addon| addon.loaded),
        "the log names the engine"
    );
    assert!(!report.has_gap(ProbeGap::SessionLogAbsent));
}

#[test]
#[ignore = "asserts the registered machine's baseline (features.md 0.5.5); the lab job runs it"]
fn test_probe_matches_the_registered_development_machine_baseline() {
    let report = EnvCapabilities::probe();
    assert_eq!(
        report.fcitx5_version.as_deref(),
        Some("5.1.7"),
        "{report:?}"
    );
    assert!(!report.wlr_layer_shell, "{report:?}");
    assert_eq!(report.tier, WaylandTier::NotApplicable, "{report:?}");
    assert!(
        report.cjk_font_count >= 90,
        "0.5.5 registers 98 CJK fonts: {report:?}"
    );
    assert_eq!(report.display_server, DisplayServer::Wayland, "{report:?}");
    assert_eq!(report.compositor.as_deref(), Some("weston"), "{report:?}");
}
