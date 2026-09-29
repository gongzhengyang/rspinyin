//! Tests for [`super`]: the verdicts, the audit and the two documents the gate reads.
//!
//! Every verdict below is reached from a report built by hand, so none of these tests needs a
//! display server, a compositor, Fcitx5 or a process table, and none of them depends on the
//! machine the suite happens to run on. Two tests read the repository's own documents --
//! `docs/dev/tests.md` and `docs/dev/features.md` -- because agreeing with them is the point of
//! the cross-check; the one test that reads this machine's real environment asserts only that
//! the probe answers for every requirement without panicking.
//!
//! The machines are written out one per shape the gate has to tell apart: an X11 session, the
//! registered development machine (WSLg under Weston), a real wlroots session whose registry was
//! read, a display that was named but could not be opened, and a machine with nothing at all.

use std::path::PathBuf;

use super::*;
use crate::budget::repo_root;
use crate::testd::env::{
    AddonDescriptor, EnvCapabilities, Observation, ProbeGap, ProcessEntry, RegistryFacts, X11Facts,
};

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

/// An observation of the X11 session the development machine serves at `:0`.
fn x11() -> Observation {
    Observation {
        display: Some(":0".to_owned()),
        x11: Some(X11Facts {
            argb_visual: true,
            compositor_present: true,
        }),
        ..Observation::default()
    }
}

/// An observation of the registered development machine: WSLg, Weston, Fcitx5 5.1.7.
fn weston() -> Observation {
    Observation {
        display: Some(":0".to_owned()),
        x11: Some(X11Facts {
            argb_visual: true,
            compositor_present: false,
        }),
        wayland_display: Some("wayland-0".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "weston")]),
        own_pid: 99,
        fcitx5_version: Some("5.1.7".to_owned()),
        cjk_fonts: Some("Noto Sans CJK: style=Regular\n".to_owned()),
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

/// An observation of a real wlroots session whose `wl_registry` listing was read.
fn sway() -> Observation {
    Observation {
        wayland_display: Some("wayland-1".to_owned()),
        wayland_socket: true,
        processes: Some(vec![row(10, 1, "sway")]),
        own_pid: 99,
        fcitx5_version: Some("5.1.7".to_owned()),
        registry: RegistryFacts {
            layer_shell: Some(true),
            xdg_wm_base: Some(true),
        },
        ..Observation::default()
    }
}

/// The report an observation becomes.
fn caps(observation: &Observation) -> EnvCapabilities {
    EnvCapabilities::from_observation(observation)
}

/// A case that declares `requirement`.
fn case(id: &str, module: &str, requirement: EnvRequirement) -> TestCaseMeta {
    TestCaseMeta {
        id: id.to_owned(),
        module: module.to_owned(),
        requirement,
    }
}

/// A result reporting what the gate said and what the run claimed.
fn result(id: &str, verdict: Executability, outcome: CaseOutcome) -> CaseResult {
    CaseResult {
        id: id.to_owned(),
        verdict,
        outcome,
    }
}

/// The environment and the reason a withheld verdict carries, when it is one.
fn withheld_of(verdict: &Executability) -> Option<(String, String)> {
    match verdict {
        Executability::Unverifiable { needs, reason } => Some((needs.clone(), reason.clone())),
        _ => None,
    }
}

/// The task span and the reason a blocked verdict carries, when it is one.
fn blocked_of(verdict: &Executability) -> Option<(String, String)> {
    match verdict {
        Executability::Blocked { task, reason } => Some((task.clone(), reason.clone())),
        _ => None,
    }
}

/// The verdict the gate reaches, in a violation that reports one the result disagrees with.
fn derived_of(violation: &GateViolation) -> Option<Executability> {
    match violation {
        GateViolation::VerdictMismatch { derived, .. } => Some(derived.clone()),
        _ => None,
    }
}

/// A specification fragment holding one register table with the given rows.
fn spec_with(rows: &str) -> String {
    let heading = "#### 0.5.5 首轮测试设备与合成器登记表\n\n";
    let table = "| 任务 | 不可验证的验收项 | 需要的环境 |\n|---|---|---|\n";
    format!("{heading}{table}{rows}\n")
}

/// A register row naming `tasks`, with the environment the specification words.
fn register_row(tasks: &str, needs: &str) -> String {
    format!("| {tasks} | an acceptance item | {needs} |\n")
}

/// The rows of the real register, worded as `features.md` words them.
fn real_rows() -> String {
    let mut rows = register_row(
        "`TASK-1.04.07`",
        "真实 Sway/Hyprland/KWin/GNOME 会话（另需 `wlr-protocols`）。本机不可验证",
    );
    rows.push_str(&register_row(
        "`TASK-1.05.04`",
        "支持应用侧模糊的合成器（KWin / Hyprland / picom）；本机仅能验证降级路径",
    ));
    rows.push_str(&register_row("`TASK-1.05.07`", "wlroots 档会话"));
    rows.push_str(&register_row(
        "`TASK-1.02.07` / `TASK-1.08.03`",
        "裸机 Linux；本机数值仅作相对回归基线",
    ));
    rows.push_str(&register_row("`TASK-1.04.05`", "真实多显示器环境"));
    rows
}

/// Every requirement the decision table declares, in the order it declares them.
const ALL_REQUIREMENTS: [EnvRequirement; 9] = [
    EnvRequirement::X11Session,
    EnvRequirement::Fcitx5Session,
    EnvRequirement::WaylandTier,
    EnvRequirement::CompositorBlur,
    EnvRequirement::MultipleMonitors,
    EnvRequirement::LongRun,
    EnvRequirement::CandidateWindowUi,
    EnvRequirement::ConfigReload,
    EnvRequirement::LogRedaction,
];

#[test]
fn test_classify_clears_an_x11_case_when_the_server_answered() {
    let env = caps(&x11());
    let verdict = classify(&case("TC-UI-01", "ui", EnvRequirement::X11Session), &env);
    assert_eq!(verdict, Executability::Runnable);
    assert!(verdict.is_runnable());
    assert_eq!(verdict.reason(), None);
    assert_eq!(verdict.marker(), "[可执行]");
}

#[test]
fn test_classify_clears_an_x11_case_beside_a_wayland_session() {
    // The registered development machine is a Wayland session with an XWayland server beside
    // it, and `features.md` 0.5.5 registers its X11 row as verifiable for exactly that reason.
    let env = caps(&weston());
    let verdict = classify(&case("TC-UI-01", "ui", EnvRequirement::X11Session), &env);
    assert_eq!(verdict, Executability::Runnable, "{env:?}");
}

#[test]
fn test_classify_withholds_an_x11_case_whose_display_could_not_be_opened() {
    let observation = Observation {
        display: Some(":0".to_owned()),
        ..Observation::default()
    };
    let verdict = classify(
        &case("TC-UI-01", "ui", EnvRequirement::X11Session),
        &caps(&observation),
    );
    let (needs, reason) =
        withheld_of(&verdict).expect("a display nobody could open is not runnable");
    assert!(reason.contains(":0"), "{reason}");
    assert!(!needs.is_empty(), "{needs}");
}

#[test]
fn test_classify_withholds_an_x11_case_on_a_machine_with_no_display() {
    let verdict = classify(
        &case("TC-UI-01", "ui", EnvRequirement::X11Session),
        &caps(&nothing()),
    );
    let (_, reason) = withheld_of(&verdict).expect("a machine with no display cannot serve one");
    assert!(reason.contains("DISPLAY"), "{reason}");
}

#[test]
fn test_classify_clears_a_fcitx5_case_only_where_the_development_package_is() {
    let installed = classify(
        &case("TC-RT-01", "rt", EnvRequirement::Fcitx5Session),
        &caps(&weston()),
    );
    assert_eq!(installed, Executability::Runnable);
    let absent = classify(
        &case("TC-RT-01", "rt", EnvRequirement::Fcitx5Session),
        &caps(&nothing()),
    );
    let (_, reason) =
        withheld_of(&absent).expect("a machine without the package cannot load an addon");
    assert!(reason.contains("Fcitx5Core"), "{reason}");
}

#[test]
fn test_classify_withholds_every_wayland_tier_case_on_the_registered_machine() {
    // The baseline `features.md` 0.5.5 registers: Weston is `ASM-13`'s case for a compositor
    // outside the four tiers, so no tier of the ladder applies and no tier case may run.
    let env = caps(&weston());
    let verdict = classify(&case("TC-RT-31", "rt", EnvRequirement::WaylandTier), &env);
    let (needs, reason) =
        withheld_of(&verdict).expect("a session outside the four tiers is withheld");
    assert_eq!(needs, "真实 Sway/Hyprland/KWin/GNOME 会话");
    assert!(reason.contains("weston"), "{reason}");
    assert_eq!(verdict.marker(), "[不可验证]");
}

#[test]
fn test_classify_clears_a_wayland_tier_case_on_a_session_that_serves_one() {
    let env = caps(&sway());
    let verdict = classify(&case("TC-RT-31", "rt", EnvRequirement::WaylandTier), &env);
    assert_eq!(verdict, Executability::Runnable, "{env:?}");
}

#[test]
fn test_classify_withholds_a_wayland_tier_case_whose_registry_was_not_read() {
    // The tier is an inference from the compositor's name here rather than a reading, and a
    // capability nobody could read is one a case must not be cleared for.
    let mut env = caps(&sway());
    env.gaps.push(ProbeGap::WaylandRegistryUnread);
    let verdict = classify(&case("TC-RT-31", "rt", EnvRequirement::WaylandTier), &env);
    let (_, reason) = withheld_of(&verdict).expect("a tier nobody read is not a tier");
    assert!(reason.contains("wl_registry"), "{reason}");
}

#[test]
fn test_classify_withholds_a_wayland_tier_case_on_a_machine_with_no_display() {
    let verdict = classify(
        &case("TC-RT-31", "rt", EnvRequirement::WaylandTier),
        &caps(&nothing()),
    );
    let (_, reason) = withheld_of(&verdict).expect("a machine with no session cannot serve one");
    assert!(reason.contains("not a Wayland"), "{reason}");
}

#[test]
fn test_classify_withholds_the_blur_the_monitors_and_the_long_run_everywhere() {
    // The three requirements no report can settle. They are withheld on every machine, which
    // is what `features.md` 0.5.5 registers for the development one and the only answer a gate
    // that reads no blur capability, no monitor layout and no eight-hour run can give.
    let machines = [nothing(), x11(), weston(), sway()];
    let expected = [
        (EnvRequirement::CompositorBlur, "支持应用侧模糊的合成器"),
        (EnvRequirement::MultipleMonitors, "真实多显示器环境"),
        (EnvRequirement::LongRun, "裸机 Linux，8 小时独占"),
    ];
    for observation in &machines {
        let env = caps(observation);
        for (requirement, wants) in &expected {
            let verdict = classify(&case("TC-UI-21", "ui", requirement.clone()), &env);
            let (needs, _) = withheld_of(&verdict).expect("no report settles this requirement");
            assert_eq!(needs, *wants, "{requirement:?}");
        }
    }
}

#[test]
fn test_classify_blocks_the_candidate_window_on_its_task_span() {
    let verdict = classify(
        &case("TC-UI-16", "ui", EnvRequirement::CandidateWindowUi),
        &caps(&weston()),
    );
    let (task, reason) = blocked_of(&verdict).expect("the window's drawing does not exist yet");
    assert_eq!(task, "TASK-1.05.01–1.05.08");
    assert!(!reason.is_empty(), "{reason}");
    assert_eq!(verdict.marker(), "[待实现: TASK-1.05.01–1.05.08]");
}

#[test]
fn test_classify_blocks_the_config_reload_and_the_log_redaction() {
    let env = caps(&weston());
    let reload = case("TC-RT-26", "rt", EnvRequirement::ConfigReload);
    let redact = case("TC-DIAG-01", "diag", EnvRequirement::LogRedaction);
    let reloaded = classify(&reload, &env);
    let redacted = classify(&redact, &env);
    assert_eq!(reloaded.marker(), "[待实现: TASK-1.03.06]");
    assert_eq!(redacted.marker(), "[待实现: TASK-1.08.01]");
    assert_ne!(reloaded.reason(), redacted.reason());
    assert!(blocked_of(&reloaded).is_some());
    assert!(blocked_of(&redacted).is_some());
}

#[test]
fn test_classify_withholds_a_requirement_that_was_never_declared() {
    // Fail closed: a requirement outside the table is not a case the gate may clear, whatever
    // the machine looks like.
    for observation in [nothing(), x11(), weston(), sway()] {
        let env = caps(&observation);
        let undeclared = EnvRequirement::Undeclared("a projector".to_owned());
        let verdict = classify(&case("TC-UI-99", "ui", undeclared), &env);
        let (needs, reason) = withheld_of(&verdict).expect("an undeclared requirement");
        assert_eq!(needs, "<未声明>");
        assert_eq!(reason, "unknown requirement");
    }
}

#[test]
fn test_classify_withholds_a_case_that_declares_nothing_at_all() {
    let blank = case("TC-UI-99", "ui", EnvRequirement::Undeclared(String::new()));
    let verdict = classify(&blank, &caps(&x11()));
    let (needs, _) = withheld_of(&verdict).expect("a case that declares nothing runs nowhere");
    assert_eq!(needs, "<未声明>");
    assert_eq!(verdict.marker(), "[不可验证]");
}

#[test]
fn test_classify_never_clears_a_case_on_a_machine_with_nothing() {
    let env = caps(&nothing());
    for requirement in ALL_REQUIREMENTS {
        let verdict = classify(&case("TC-CORE-01", "core", requirement.clone()), &env);
        assert!(
            !verdict.is_runnable(),
            "{requirement:?} was cleared on a machine with no display, no Fcitx5 and no fonts"
        );
    }
}

#[test]
fn test_requirement_parse_round_trips_every_label_and_leaves_the_rest_undeclared() {
    for requirement in ALL_REQUIREMENTS {
        let label = requirement.label().to_owned();
        assert_eq!(EnvRequirement::parse(&label), requirement, "{label}");
        assert_eq!(
            EnvRequirement::parse(&format!("`{label}`")),
            requirement,
            "a document may wrap the declaration in backticks"
        );
        assert_eq!(
            EnvRequirement::parse(&format!("  {label}  ")),
            requirement,
            "a document may pad the declaration"
        );
    }
    assert_eq!(
        EnvRequirement::parse("a projector"),
        EnvRequirement::Undeclared("a projector".to_owned())
    );
    assert_eq!(
        EnvRequirement::parse(""),
        EnvRequirement::Undeclared(String::new())
    );
    assert_eq!(EnvRequirement::parse("a projector").label(), "a projector");
}

#[test]
fn test_labels_are_the_ones_a_batch_counts_by() {
    assert_eq!(Executability::Runnable.label(), "runnable");
    assert_eq!(Executability::Runnable.describe(), "may run here");
    let blocked = Executability::Blocked {
        task: String::from("TASK-1.03.06"),
        reason: String::from("nothing to reload"),
    };
    assert_eq!(blocked.label(), "blocked");
    assert!(blocked.marker().contains("TASK-1.03.06"), "{blocked:?}");
    let withheld = Executability::Unverifiable {
        needs: String::from("真实多显示器环境"),
        reason: String::from("nothing here settles it"),
    };
    assert_eq!(withheld.label(), "unverifiable");
    assert_eq!(withheld.marker(), "[不可验证]");
}

#[test]
fn test_executability_describe_names_the_task_or_the_environment() {
    let blocked = Executability::Blocked {
        task: String::from("TASK-1.03.06"),
        reason: String::from("nothing to reload"),
    };
    assert!(blocked.describe().contains("TASK-1.03.06"), "{blocked:?}");
    assert!(
        blocked.describe().contains("nothing to reload"),
        "{blocked:?}"
    );
    let withheld = Executability::Unverifiable {
        needs: String::from("真实多显示器环境"),
        reason: String::from("nothing here settles it"),
    };
    assert!(
        withheld.describe().contains("真实多显示器环境"),
        "{withheld:?}"
    );
    assert!(
        withheld.describe().contains("nothing here settles it"),
        "{withheld:?}"
    );
    assert_eq!(blocked.reason(), Some("nothing to reload"));
    assert_eq!(withheld.reason(), Some("nothing here settles it"));
}

#[test]
fn test_requirement_remedy_names_a_way_out_for_every_requirement() {
    // A withheld case that says what is missing without saying what to do about it leaves the
    // reader to guess, so every requirement carries a remedy and the ones a reader can act on
    // name what to act on.
    for requirement in ALL_REQUIREMENTS {
        let remedy = requirement.remedy();
        assert!(!remedy.is_empty(), "{requirement:?}");
        assert_ne!(remedy, requirement.label(), "{requirement:?}");
        assert!(remedy.len() >= 30, "{requirement:?}: {remedy}");
    }
    let fcitx5 = EnvRequirement::Fcitx5Session.remedy();
    assert!(fcitx5.contains("libfcitx5core-dev"), "{fcitx5}");
    let tier = EnvRequirement::WaylandTier.remedy();
    assert!(tier.contains("Sway"), "{tier}");
    let undeclared = EnvRequirement::Undeclared(String::new()).remedy();
    assert!(undeclared.contains("环境需求"), "{undeclared}");
}

#[test]
fn test_requirement_remedy_of_a_blocked_case_sends_no_one_looking_for_an_environment() {
    // A case whose code does not exist yet is not an environment problem, and its remedy has to
    // say so: a reader sent to install a compositor would be chasing the wrong thing.
    for requirement in [
        EnvRequirement::CandidateWindowUi,
        EnvRequirement::ConfigReload,
        EnvRequirement::LogRedaction,
    ] {
        let remedy = requirement.remedy();
        assert!(remedy.contains("no environment"), "{requirement:?}");
    }
}

#[test]
fn test_audit_batch_reports_a_withheld_case_recorded_as_a_pass() {
    // The assertion this gate exists for: a case that could not run may not report a pass.
    let blocked = Executability::Blocked {
        task: String::from("TASK-1.05.01–1.05.08"),
        reason: String::from("nothing to exercise"),
    };
    let withheld = Executability::Unverifiable {
        needs: String::from("真实 Sway/Hyprland/KWin/GNOME 会话"),
        reason: String::from("the compositor is weston"),
    };
    let results = [
        result("TC-UI-16", blocked, CaseOutcome::Pass),
        result("TC-RT-31", withheld, CaseOutcome::Pass),
        result("TC-CORE-01", Executability::Runnable, CaseOutcome::Pass),
    ];
    let violations = audit_batch(&results);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert_eq!(violations[0].case(), "TC-UI-16");
    assert_eq!(violations[1].case(), "TC-RT-31");
    assert!(violations[0].describe().contains("pass"), "{violations:?}");
    assert!(
        violations[1]
            .describe()
            .contains("真实 Sway/Hyprland/KWin/GNOME 会话"),
        "{violations:?}"
    );
}

#[test]
fn test_audit_batch_reports_a_withheld_case_recorded_as_a_failure() {
    // A failure claims the case was exercised, which a withheld case never was.
    let withheld = Executability::Unverifiable {
        needs: String::from("真实多显示器环境"),
        reason: String::from("nothing here settles it"),
    };
    let violations = audit_batch(&[result("TC-RT-14", withheld, CaseOutcome::Fail)]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].describe().contains("fail"), "{violations:?}");
}

#[test]
fn test_audit_batch_reports_a_cleared_case_that_never_ran() {
    let results = [result(
        "TC-CORE-01",
        Executability::Runnable,
        CaseOutcome::NotRun,
    )];
    let violations = audit_batch(&results);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].case(), "TC-CORE-01");
    assert!(
        violations[0].describe().contains("no result"),
        "{violations:?}"
    );
}

#[test]
fn test_audit_batch_accepts_a_batch_whose_results_agree_with_its_verdicts() {
    let blocked = Executability::Blocked {
        task: String::from("TASK-1.03.06"),
        reason: String::from("nothing to reload"),
    };
    let results = [
        result("TC-CORE-01", Executability::Runnable, CaseOutcome::Pass),
        result("TC-CORE-02", Executability::Runnable, CaseOutcome::Fail),
        result("TC-RT-26", blocked, CaseOutcome::NotRun),
    ];
    assert!(audit_batch(&results).is_empty());
    assert!(audit_batch(&[]).is_empty());
}

#[test]
fn test_audit_batch_against_reports_a_result_the_gate_would_not_clear() {
    // The anti-cheat case a bare result list cannot see: the result says the case ran, and the
    // gate says this machine may not run it at all.
    let cases = [case("TC-RT-31", "rt", EnvRequirement::WaylandTier)];
    let env = caps(&weston());
    let results = [result(
        "TC-RT-31",
        Executability::Runnable,
        CaseOutcome::Pass,
    )];
    let violations = audit_batch_against(&cases, &results, &env);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let derived = derived_of(&violations[0]).expect("a verdict the gate does not reach");
    assert!(!derived.is_runnable(), "{derived:?}");
    assert_eq!(derived.marker(), "[不可验证]");
    assert_eq!(violations[0].case(), "TC-RT-31");
}

#[test]
fn test_audit_batch_against_reports_a_result_for_a_case_the_batch_does_not_hold() {
    let cases = [case("TC-CORE-01", "core", EnvRequirement::X11Session)];
    let env = caps(&x11());
    let results = [
        result("TC-CORE-01", Executability::Runnable, CaseOutcome::Pass),
        result("TC-CORE-99", Executability::Runnable, CaseOutcome::Pass),
    ];
    let violations = audit_batch_against(&cases, &results, &env);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(matches!(violations[0], GateViolation::UnknownCase { .. }));
    assert_eq!(violations[0].case(), "TC-CORE-99");
    assert!(
        violations[0].describe().contains("TC-CORE-99"),
        "{violations:?}"
    );
}

#[test]
fn test_audit_batch_against_accepts_results_that_agree_with_the_gate() {
    let cases = [
        case("TC-CORE-01", "core", EnvRequirement::X11Session),
        case("TC-RT-31", "rt", EnvRequirement::WaylandTier),
    ];
    let env = caps(&weston());
    let results = [
        result("TC-CORE-01", classify(&cases[0], &env), CaseOutcome::Pass),
        result("TC-RT-31", classify(&cases[1], &env), CaseOutcome::NotRun),
    ];
    assert!(audit_batch_against(&cases, &results, &env).is_empty());
    assert!(audit_batch_against(&[], &[], &env).is_empty());
}

#[test]
fn test_audit_batch_against_keeps_the_outcome_the_bare_audit_reports() {
    // A result whose verdict is the gate's and whose outcome is not is still a violation, so the
    // stronger audit is the weaker one plus the re-derivation rather than a replacement for it.
    let cases = [case("TC-RT-31", "rt", EnvRequirement::WaylandTier)];
    let env = caps(&weston());
    let verdict = classify(&cases[0], &env);
    let results = [result("TC-RT-31", verdict, CaseOutcome::Pass)];
    let violations = audit_batch_against(&cases, &results, &env);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(matches!(violations[0], GateViolation::ClaimedRun { .. }));
    assert_eq!(audit_batch(&results).len(), 1);
}

#[test]
fn test_batch_report_counts_the_three_kinds_and_reasons_each_withheld_case() {
    let cases = [
        case("TC-UI-01", "ui", EnvRequirement::X11Session),
        case("TC-RT-31", "rt", EnvRequirement::WaylandTier),
        case("TC-UI-16", "ui", EnvRequirement::CandidateWindowUi),
    ];
    let report = BatchReport::of(&cases, &[], &caps(&weston()));
    assert_eq!(report.total(), 3);
    assert_eq!(report.runnable, 1);
    assert_eq!(report.unverifiable, 1);
    assert_eq!(report.blocked, 1);
    assert_eq!(report.withheld.len(), 2);
    assert!(report.violations.is_empty());
    assert_eq!(report.withheld[0].id, "TC-RT-31");
    assert_eq!(report.withheld[0].module, "rt");
    assert_eq!(report.withheld[1].id, "TC-UI-16");
}

#[test]
fn test_batch_report_lines_quote_every_withheld_case_and_every_violation() {
    let env = caps(&weston());
    let cases = [
        case("TC-UI-01", "ui", EnvRequirement::X11Session),
        case("TC-RT-31", "rt", EnvRequirement::WaylandTier),
    ];
    let verdict = classify(&cases[1], &env);
    let results = [result("TC-RT-31", verdict, CaseOutcome::Pass)];
    let report = BatchReport::of(&cases, &results, &env);
    assert_eq!(report.violations.len(), 1);
    let text = report.lines().join("\n");
    assert!(
        text.contains("cases: 2 total, 1 runnable, 0 blocked, 1 unverifiable"),
        "{text}"
    );
    assert!(text.contains("withheld TC-RT-31 [rt]"), "{text}");
    assert!(text.contains("[不可验证]"), "{text}");
    assert!(
        text.contains("真实 Sway/Hyprland/KWin/GNOME 会话"),
        "{text}"
    );
    assert!(
        text.contains(EnvRequirement::WaylandTier.remedy()),
        "{text}"
    );
    assert!(text.contains("violation TC-RT-31"), "{text}");
}

#[test]
fn test_batch_report_of_judges_the_results_against_the_gate_not_the_results() {
    // A batch whose own results claim a pass the gate did not allow: the report has to hold the
    // violation and the case has to stay withheld.
    let cases = [case("TC-RT-31", "rt", EnvRequirement::WaylandTier)];
    let env = caps(&weston());
    let results = [result(
        "TC-RT-31",
        Executability::Runnable,
        CaseOutcome::Pass,
    )];
    let report = BatchReport::of(&cases, &results, &env);
    assert_eq!(report.runnable, 0);
    assert_eq!(report.unverifiable, 1);
    assert_eq!(report.withheld.len(), 1);
    assert_eq!(report.violations.len(), 1, "{report:?}");
    let text = report.lines().join("\n");
    assert!(text.contains("violation TC-RT-31"), "{text}");
    assert!(text.contains("the gate reaches"), "{text}");
}

#[test]
fn test_withheld_describe_carries_the_case_its_reason_and_its_remedy() {
    let env = caps(&weston());
    let cases = [case("TC-RT-31", "rt", EnvRequirement::WaylandTier)];
    let report = BatchReport::of(&cases, &[], &env);
    let withheld = &report.withheld[0];
    assert_eq!(withheld.requirement, EnvRequirement::WaylandTier);
    assert_eq!(withheld.remedy(), EnvRequirement::WaylandTier.remedy());
    let text = withheld.describe();
    assert!(text.contains("TC-RT-31"), "{text}");
    assert!(text.contains("[rt]"), "{text}");
    assert!(text.contains("[不可验证]"), "{text}");
    assert!(
        text.contains("真实 Sway/Hyprland/KWin/GNOME 会话"),
        "{text}"
    );
    assert!(text.contains(withheld.remedy()), "{text}");
}

#[test]
fn test_parse_cases_reads_the_metadata_of_each_case() {
    let document = "\
### TC-UI-16 候选框骨架（`REQ-UI-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 对应功能条目编号：`REQ-UI-04`
  - 模块与类别：`ui` | 核心业务闭环
  - 环境需求：`候选框 UI`

### TC-CORE-01 合法音节全表识别（`REQ-CORE-01`）

- **基本属性**：
  - 模块与类别：`core` | 核心业务闭环
";
    let cases = parse_cases(document);
    assert_eq!(cases.len(), 2, "{cases:?}");
    assert_eq!(cases[0].id, "TC-UI-16");
    assert_eq!(cases[0].module, "ui");
    assert_eq!(cases[0].requirement, EnvRequirement::CandidateWindowUi);
    assert_eq!(cases[1].id, "TC-CORE-01");
    assert_eq!(cases[1].module, "core");
    assert_eq!(
        cases[1].requirement,
        EnvRequirement::Undeclared(String::new()),
        "a case that declares nothing must not acquire a default"
    );
}

#[test]
fn test_parse_cases_ignores_a_heading_that_is_not_a_case_identifier() {
    let document = "\
### TC-<模块>-<序号> 模板占位符

- **基本属性**：
  - 模块与类别：`<模块>` | 模板

### 4.2 用例模板

- **基本属性**：
";
    assert!(parse_cases(document).is_empty());
    assert!(parse_cases("").is_empty());
}

#[test]
fn test_parse_cases_of_the_case_document_never_clears_an_undeclared_case() {
    // The suite as it stands declares no environment requirement, and the gate's answer to
    // that is to withhold every one of those cases rather than to assume the machine is right.
    let root = repo_root().expect("the repository root resolves");
    let cases = read_cases(&root).expect("the case suite is readable");
    assert!(!cases.is_empty(), "the suite has cases");
    let env = caps(&weston());
    for meta in &cases {
        assert!(meta.id.starts_with("TC-"), "{meta:?}");
        if meta.requirement == EnvRequirement::Undeclared(String::new()) {
            assert!(
                !classify(meta, &env).is_runnable(),
                "{} declares nothing and was cleared anyway",
                meta.id
            );
            assert!(
                meta.requirement.remedy().contains("环境需求"),
                "{} has to be told which field declares a requirement",
                meta.id
            );
        }
    }
    let report = BatchReport::of(&cases, &[], &env);
    assert_eq!(report.total(), cases.len(), "the batch quotes the suite");
    for withheld in &report.withheld {
        assert!(!withheld.remedy().is_empty(), "{}", withheld.id);
    }
}

#[test]
fn test_cross_check_agrees_with_the_specification_document() {
    // The scripted comparison the acceptance criteria ask for: the decision table's citations
    // and environments are read back out of `features.md` 0.5.5 and compared with the table.
    let root = repo_root().expect("the repository root resolves");
    let register = read_register(&root).expect("section 0.5.5 is readable");
    assert!(
        register.entries.len() >= 5,
        "the register lists the items the baseline cannot verify: {:?}",
        register.entries
    );
    let drift = cross_check_spec(&register);
    assert!(drift.is_empty(), "{drift:#?}");
}

#[test]
fn test_classify_withholds_every_requirement_the_register_calls_unverifiable() {
    // What the register says this machine cannot verify has to be what the gate withholds on the
    // machine the register describes; otherwise the register is a note rather than a rule, and a
    // case the specification lists as unverifiable could still be cleared here.
    let root = repo_root().expect("the repository root resolves");
    let register = read_register(&root).expect("the register is readable");
    let env = caps(&weston());
    let tasks = register.tasks();
    assert!(!tasks.is_empty(), "the register names items to check");
    for task in tasks {
        let row = super::table::DECISION_TABLE
            .iter()
            .find(|row| row.cites(task))
            .expect("the cross-check reports a task no row answers");
        let label = row.requirement.label();
        let verdict = classify(&case("TC-REG-01", "rt", row.requirement.clone()), &env);
        let runnable = verdict.is_runnable();
        assert!(!runnable, "{label} is registered as unverifiable");
        assert_eq!(verdict.marker(), "[不可验证]", "{label}");
    }
}

#[test]
fn test_register_parse_reads_the_tasks_and_the_clause_of_each_row() {
    let text = spec_with(&real_rows());
    let register = UnverifiableRegister::parse(&text).expect("the register parses");
    assert_eq!(register.entries.len(), 5);
    assert_eq!(register.entries[0].tasks, ["TASK-1.04.07"]);
    assert_eq!(
        register.entries[0].clause(),
        "真实 Sway/Hyprland/KWin/GNOME 会话",
        "the qualifier after the parenthesis is commentary, not the environment"
    );
    assert_eq!(
        register.entries[3].tasks,
        ["TASK-1.02.07", "TASK-1.08.03"],
        "a row may name two tasks"
    );
    assert_eq!(register.entries[3].clause(), "裸机 Linux");
    assert_eq!(register.entries[4].clause(), "真实多显示器环境");
    assert_eq!(register.tasks().len(), 6);
    assert!(register.entry_for("TASK-1.05.04").is_some());
    assert!(register.entry_for("TASK-9.09.09").is_none());
}

#[test]
fn test_register_parse_refuses_a_document_without_the_section() {
    let parsed = UnverifiableRegister::parse("# a document with no sections\n");
    let message = parsed
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(message.contains(REGISTER_SECTION), "{message}");
}

#[test]
fn test_register_parse_refuses_a_section_without_the_table() {
    let prose = "#### 0.5.5 首轮测试设备与合成器登记表\n\nprose, and no table at all\n";
    assert!(UnverifiableRegister::parse(prose).is_err());
    assert!(
        UnverifiableRegister::parse(&spec_with("")).is_err(),
        "a register with no rows would make the cross-check agree over nothing"
    );
}

#[test]
fn test_cross_check_reports_a_row_the_register_no_longer_names() {
    let rows = register_row(
        "`TASK-1.05.04`",
        "支持应用侧模糊的合成器（KWin / Hyprland / picom）",
    );
    let register = UnverifiableRegister::parse(&spec_with(&rows)).expect("it parses");
    let drift = cross_check_spec(&register);
    let unregistered: Vec<&str> = drift
        .iter()
        .filter_map(|entry| match entry {
            TableDrift::Unregistered { task, .. } => Some(task.as_str()),
            _ => None,
        })
        .collect();
    assert!(unregistered.contains(&"TASK-1.04.07"), "{drift:#?}");
    assert!(unregistered.contains(&"TASK-1.05.07"), "{drift:#?}");
    assert!(
        drift.iter().all(|entry| !entry.describe().is_empty()),
        "{drift:#?}"
    );
}

#[test]
fn test_cross_check_reports_a_row_whose_environment_was_reworded() {
    let rows = real_rows().replace("真实 Sway/Hyprland/KWin/GNOME 会话", "一个真实合成器会话");
    let register = UnverifiableRegister::parse(&spec_with(&rows)).expect("it parses");
    let drift = cross_check_spec(&register);
    let reworded: Vec<&TableDrift> = drift
        .iter()
        .filter(|entry| matches!(entry, TableDrift::Reworded { .. }))
        .collect();
    assert_eq!(reworded.len(), 1, "{drift:#?}");
    assert!(
        reworded[0].describe().contains("一个真实合成器会话"),
        "{drift:#?}"
    );
}

#[test]
fn test_cross_check_reports_a_register_row_no_row_of_the_table_answers() {
    let mut rows = real_rows();
    rows.push_str(&register_row(
        "`TASK-9.09.09`",
        "an environment nobody registers",
    ));
    let register = UnverifiableRegister::parse(&spec_with(&rows)).expect("it parses");
    let drift = cross_check_spec(&register);
    assert_eq!(drift.len(), 1, "{drift:#?}");
    assert_eq!(
        drift[0],
        TableDrift::Uncovered {
            task: String::from("TASK-9.09.09"),
        }
    );
    assert!(drift[0].describe().contains("TASK-9.09.09"), "{drift:?}");
}

#[test]
fn test_cross_check_accepts_the_rows_worded_as_the_specification_words_them() {
    let register = UnverifiableRegister::parse(&spec_with(&real_rows())).expect("it parses");
    let drift = cross_check_spec(&register);
    assert!(drift.is_empty(), "{drift:#?}");
}

#[test]
fn test_table_drift_describe_names_the_requirement_and_the_task() {
    // `Malformed` is the one drift the decision table cannot produce, because every citation it
    // carries is a well-formed task identifier; the message is still part of the contract.
    let malformed = TableDrift::Malformed {
        requirement: String::from("X11 会话"),
        task: String::from("1.04.07"),
    };
    let text = malformed.describe();
    assert!(text.contains("1.04.07"), "{text}");
    assert!(text.contains("X11 会话"), "{text}");
    let uncovered = TableDrift::Uncovered {
        task: String::from("TASK-1.04.07"),
    };
    let text = uncovered.describe();
    assert!(text.contains("TASK-1.04.07"), "{text}");
    assert!(text.contains(REGISTER_SECTION), "{text}");
    let reworded = TableDrift::Reworded {
        requirement: String::from("多显示器"),
        register: String::from("真实多显示器环境"),
        row: String::from("one display"),
    };
    let text = reworded.describe();
    assert!(text.contains("one display"), "{text}");
    assert!(text.contains("真实多显示器环境"), "{text}");
}

#[test]
fn test_this_machine_is_probed_and_classified_without_panicking() {
    // The one test that reads the real environment, and it asserts nothing about what it
    // finds: whatever this machine is, every requirement has to get an answer.
    let env = EnvCapabilities::probe();
    for requirement in ALL_REQUIREMENTS {
        let verdict = classify(&case("TC-CORE-01", "core", requirement.clone()), &env);
        assert!(!verdict.label().is_empty(), "{requirement:?}");
        assert!(!verdict.marker().is_empty(), "{requirement:?}");
    }
    let report = BatchReport::of(&[], &[], &env);
    assert_eq!(report.total(), 0);
    assert_eq!(report.lines().len(), 1);
}
