//! Tests for [`super`].
//!
//! They cover the three things the module promises: that the checklist is a complete reading of
//! the specification's visual baseline, that the prompt it renders is answerable and carries the
//! case it was built for, and that an answer is judged by the rules rather than by its wording.
//!
//! The coverage tests parse the repository's own `docs/dev/features.md` rather than restating it.
//! That is the point of them: a test that spelled the token table out again would pass while the
//! document and the checklist drifted apart, which is exactly the failure the checklist exists to
//! make impossible. None of these tests needs a display server, a compositor or a snapshot.
//!
//! The `.slint` reader and the metrics cross-check have their own tests; this file only borrows the
//! specification parser, and only to ask it what the document holds.

use std::fs;
use std::path::{Path, PathBuf};

use ime_types::{ColorScheme, LayoutHint, Rgba8, ThemeSpec};

use crate::testd::evidence::Verdict;
use crate::testd::ui_metrics::{BackendReading, SPEC_DOCUMENT, SpecTable, UiMetrics};

use super::{
    Finding, PROMPT_NAME, Refusal, ReviewError, Sample, SpecRef, Summary, VISUAL_AUDIT_PROMPT,
    anchor_line, audit_request, audit_request_of_snapshot, check_items,
};

/// The repository root, derived from the compile-time location of this crate.
fn repo_root() -> PathBuf {
    crate::budget::repo_root().expect("xtask lives in a subdirectory of the repository root")
}

/// The repository's own specification, as text.
fn spec_text() -> String {
    fs::read_to_string(repo_root().join(SPEC_DOCUMENT)).expect("reading the specification")
}

/// The repository's own specification, parsed.
fn spec() -> SpecTable {
    SpecTable::parse(&spec_text()).expect("the specification parses")
}

/// A snapshot path the evidence archive would have written.
fn snapshot() -> &'static Path {
    Path::new("results/runs/run-20260930-120000/ui/TC-UI-16/01_default.png")
}

/// What the running window reported, in the state the X11 tier reaches without a compositor.
fn live_metrics() -> UiMetrics {
    UiMetrics::new(
        BackendReading {
            backend_id: "x11",
            window_dp: (600, 140),
            window_px: (1200, 280),
            scale: 2.0,
            argb_visual: false,
            compositor_present: false,
            base_alpha: 255,
        },
        LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
        ThemeSpec {
            scheme: ColorScheme::Dark,
            accent: Rgba8 {
                r: 0x4C,
                g: 0x9A,
                b: 0xFF,
                a: 255,
            },
            acrylic: false,
            base_alpha: 217,
            corner_radius_dp: 12,
            scale: 2.0,
        },
    )
}

/// A sampling point a reviewer would report.
fn sample() -> Sample {
    Sample {
        x: 48,
        y: 12,
        measured: String::from("24px"),
        expected: String::from("12dp × 2.0 = 24px"),
    }
}

/// A finding that carries a sample and no reason.
fn finding(item: &str, verdict: Verdict) -> Finding {
    Finding {
        item: item.to_owned(),
        verdict,
        sample: Some(sample()),
        reason: String::new(),
    }
}

/// A submission in which every item passes.
fn all_pass(items: &[super::CheckItem]) -> Vec<Finding> {
    let mut submission = Vec::new();
    for item in items {
        submission.push(finding(item.id, Verdict::Pass));
    }
    submission
}

/// The request the tests audit, built the way a caller would build it.
fn built_request(case_id: &str, scale: f32, metrics: Option<&UiMetrics>) -> super::AuditRequest {
    audit_request(case_id, snapshot(), scale, metrics).expect("the request is built")
}

#[test]
fn test_checklist_covers_every_geometry_row_of_the_specification() {
    let spec = spec();
    let items = check_items();
    for row in spec.geometry() {
        let cited = items
            .iter()
            .any(|item| item.anchor.row == row.label.as_str());
        assert!(cited, "3.1.1 `{}` has no checklist item", row.label);
    }
}

#[test]
fn test_checklist_covers_every_colour_token_of_the_specification() {
    let spec = spec();
    let items = check_items();
    for token in spec.colours().keys() {
        let cited = items
            .iter()
            .any(|item| item.anchor.row == format!("`{token}`").as_str());
        assert!(cited, "3.2 token `{token}` has no checklist item");
    }
}

#[test]
fn test_checklist_covers_every_grid_exception_of_the_specification() {
    let spec = spec();
    let items = check_items();
    for (name, value) in spec.exceptions() {
        let cited = items
            .iter()
            .any(|item| item.anchor.row == format!("`{value}dp`").as_str());
        assert!(
            cited,
            "3.1.4 exception `{name}` ({value}dp) has no checklist item"
        );
    }
}

#[test]
fn test_checklist_covers_the_five_component_states() {
    let items = check_items();
    for state in ["Default", "Hover", "Active", "Focus Ring", "Disabled"] {
        let cited = items.iter().any(|item| {
            item.anchor.section == "### 3.4" && item.anchor.row == format!("`{state}`").as_str()
        });
        assert!(cited, "3.4 state `{state}` has no checklist item");
    }
}

#[test]
fn test_every_item_names_an_observable_and_a_citation() {
    for item in check_items() {
        assert!(!item.id.trim().is_empty(), "an item has no identifier");
        assert!(
            !item.criterion.trim().is_empty(),
            "`{}` states no criterion",
            item.id
        );
        assert!(
            !item.sampling.trim().is_empty(),
            "`{}` names no observable to sample",
            item.id
        );
        assert!(
            !item.anchor.section.trim().is_empty(),
            "`{}` cites no section",
            item.id
        );
        assert!(
            !item.anchor.row.trim().is_empty(),
            "`{}` cites no row",
            item.id
        );
        assert!(
            item.criterion.chars().any(|ch| ch.is_ascii_digit()) || item.criterion.contains('`'),
            "`{}` quotes neither a value nor a name",
            item.id
        );
    }
}

#[test]
fn test_every_dimension_has_at_least_one_item() {
    let items = check_items();
    for dimension in super::Dimension::ALL {
        assert!(
            items.iter().any(|item| item.dimension == dimension),
            "`{}` has no item",
            dimension.label()
        );
    }
}

#[test]
fn test_item_identifiers_are_unique_and_namespaced() {
    let items = check_items();
    for (index, item) in items.iter().enumerate() {
        assert!(item.id.contains('.'), "`{}` is not namespaced", item.id);
        assert!(
            item.id
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-'),
            "`{}` is not a plain identifier",
            item.id
        );
        for other in &items[index + 1..] {
            assert_ne!(item.id, other.id, "two items share an identifier");
        }
    }
}

#[test]
fn test_no_item_states_a_criterion_nobody_could_falsify() {
    const SUBJECTIVE: [&str; 8] = [
        "看起来",
        "是否好看",
        "美观",
        "舒服",
        "协调",
        "感觉",
        "印象",
        "主观",
    ];
    for item in check_items() {
        for phrase in SUBJECTIVE {
            assert!(
                !item.criterion.contains(phrase),
                "`{}` states a subjective criterion: {phrase}",
                item.id
            );
            assert!(
                !item.sampling.contains(phrase),
                "`{}` asks for a judgement rather than a sample: {phrase}",
                item.id
            );
        }
    }
}

#[test]
fn test_every_anchor_resolves_to_a_line_of_the_specification() {
    let document = spec_text();
    for item in check_items() {
        let line = anchor_line(&document, &item.anchor);
        assert!(
            line.is_some(),
            "`{}` cites {} `{}`, which the document does not state",
            item.id,
            item.anchor.section,
            item.anchor.row
        );
    }
}

#[test]
fn test_spec_ref_describes_the_document_the_section_and_the_row() {
    let described = SpecRef::of("#### 3.1.1", "容器圆角").describe();
    assert!(described.contains(SPEC_DOCUMENT), "{described}");
    assert!(described.contains("3.1.1"), "{described}");
    assert!(described.contains("容器圆角"), "{described}");
    assert!(
        !described.contains('#'),
        "a citation carries no heading hashes: {described}"
    );
}

#[test]
fn test_anchor_line_finds_the_row_inside_its_section() {
    let document = "# Title\n\n#### 3.1.1 Geometry\n\n| element | spec |\n|---|---|\n| 容器圆角 | `12dp` |\n\n#### 3.1.2 Material\n\n| 容器圆角 | another meaning |\n";
    assert_eq!(
        anchor_line(document, &SpecRef::of("#### 3.1.1", "容器圆角")),
        Some(7)
    );
}

#[test]
fn test_anchor_line_refuses_a_row_its_section_does_not_state() {
    let document = "#### 3.1.1 Geometry\n\n| element | spec |\n|---|---|\n| 容器圆角 | `12dp` |\n\n#### 3.1.2 Material\n\n| 容器圆角 | another meaning |\n";
    assert_eq!(
        anchor_line(document, &SpecRef::of("#### 3.1.2", "光标箭头")),
        None
    );
    assert_eq!(
        anchor_line(document, &SpecRef::of("#### 9.9", "容器圆角")),
        None
    );
    assert_eq!(
        anchor_line(document, &SpecRef::of("#### 3.1.1", "another meaning")),
        None,
        "a row that only occurs in the next section is not this section's row"
    );
}

#[test]
fn test_audit_request_substitutes_the_case_the_snapshot_and_the_scale() {
    let request = built_request("TC-UI-16", 2.0, None);
    let prompt = request.prompt();
    assert!(prompt.contains("TC-UI-16"), "{prompt}");
    assert!(prompt.contains("ui/TC-UI-16/01_default.png"), "{prompt}");
    assert!(prompt.contains("device pixel ratio = 2"), "{prompt}");
    assert!(
        prompt.contains(&format!("prompt://{PROMPT_NAME}")),
        "the prompt names the resource it came from: {prompt}"
    );
    assert!(
        prompt.contains("space.corner-radius"),
        "the checklist reaches the prompt: {prompt}"
    );
    assert!(
        !prompt.contains('{'),
        "a placeholder was left unresolved: {prompt}"
    );
}

#[test]
fn test_audit_request_refuses_a_case_identifier_the_suite_could_not_name() {
    for case_id in [
        "",
        "TC-UI-16-extra",
        "tc-ui-16",
        "TC-UI",
        "UI-16",
        "TC-UI-ab",
        "TC--16",
        "TC-UI-16 ",
    ] {
        let refused = audit_request(case_id, snapshot(), 1.0, None);
        assert!(
            matches!(refused, Err(ReviewError::BadCaseId { .. })),
            "`{case_id}` was accepted as a case identifier"
        );
    }
}

#[test]
fn test_audit_request_of_snapshot_reads_the_case_out_of_the_evidence_layout() {
    let path = Path::new("results/runs/run-20260930-120000/ui/TC-UI-16/02_hover.png");
    let built = audit_request_of_snapshot(path, 1.0, None);
    let request = built.expect("the snapshot is one of the archive's");
    assert_eq!(request.case_id, "TC-UI-16");
    assert_eq!(request.screenshot.as_path(), path);
}

#[test]
fn test_audit_request_of_snapshot_refuses_a_path_outside_the_layout() {
    for path in [
        "results/runs/run-1/ui/TC-UI-16/01_default.png.orig",
        "results/runs/run-1/ui/TC-UI-16/01_default",
        "results/runs/run-1/ui/01_default.png",
        "01_default.png",
    ] {
        let refused = audit_request_of_snapshot(Path::new(path), 1.0, None);
        assert!(
            matches!(refused, Err(ReviewError::BadSnapshotPath { .. })),
            "`{path}` was accepted as a snapshot"
        );
    }
}

#[test]
fn test_audit_request_normalizes_a_scale_no_window_could_have() {
    for scale in [0.0, -2.0, f32::NAN, f32::INFINITY] {
        let request = built_request("TC-UI-16", scale, None);
        assert_eq!(request.scale, 1.0, "the scale {scale} was not normalised");
    }
    let request = built_request("TC-UI-16", 1.25, None);
    assert_eq!(request.scale, 1.25, "a usable scale is kept");
}

#[test]
fn test_prompt_states_the_three_verdicts_and_the_rule_about_the_third() {
    for phrase in ["通过", "不通过", "无法判定", "采样坐标", "实测", "规范值"] {
        assert!(
            VISUAL_AUDIT_PROMPT.contains(phrase),
            "the prompt never states `{phrase}`"
        );
    }
    assert!(
        VISUAL_AUDIT_PROMPT.contains("永远不等于"),
        "the prompt must forbid counting the third verdict as a pass"
    );
}

#[test]
fn test_prompt_carries_the_metrics_the_window_reported() {
    let metrics = live_metrics();
    let request = built_request("TC-UI-16", 2.0, Some(&metrics));
    let prompt = request.prompt();
    assert!(prompt.contains("600 × 140 dp"), "{prompt}");
    assert!(prompt.contains("1200 × 280 px"), "{prompt}");
    assert!(prompt.contains("容器圆角 12dp"), "{prompt}");
    assert!(prompt.contains("base_alpha 请求 217"), "{prompt}");
    assert!(
        prompt.contains("本窗口应为 255"),
        "the prompt states the alpha the window should carry: {prompt}"
    );
}

#[test]
fn test_prompt_says_so_when_no_metrics_were_read() {
    let request = built_request("TC-UI-16", 1.0, None);
    let prompt = request.prompt();
    assert!(prompt.contains("未注入"), "{prompt}");
    assert!(
        prompt.contains("无法判定"),
        "an item needing a metric it never got has to be answered as unjudgeable: {prompt}"
    );
}

#[test]
fn test_finding_without_a_sampling_coordinate_is_refused() {
    for verdict in [Verdict::Pass, Verdict::Fail] {
        let mut finding = finding("space.corner-radius", verdict);
        finding.sample = None;
        assert_eq!(
            finding.refusal(),
            Some(Refusal::NoSample),
            "a {verdict:?} verdict with no coordinate is not evidence"
        );
    }
}

#[test]
fn test_finding_without_measured_or_expected_values_is_refused() {
    let mut without_measured = finding("space.corner-radius", Verdict::Pass);
    without_measured.sample = Some(Sample {
        x: 48,
        y: 12,
        measured: String::new(),
        expected: String::from("12dp × 2.0 = 24px"),
    });
    assert_eq!(without_measured.refusal(), Some(Refusal::NoValues));

    let mut without_expected = finding("space.corner-radius", Verdict::Pass);
    without_expected.sample = Some(Sample {
        x: 48,
        y: 12,
        measured: String::from("24px"),
        expected: String::from("   "),
    });
    assert_eq!(without_expected.refusal(), Some(Refusal::NoValues));

    assert_eq!(
        finding("space.corner-radius", Verdict::Pass).refusal(),
        None
    );
}

#[test]
fn test_unverifiable_finding_without_a_reason_is_refused() {
    let mut finding = finding("material.blur", Verdict::Unverifiable);
    finding.sample = None;
    assert_eq!(finding.refusal(), Some(Refusal::NoReason));

    finding.reason = String::from("no compositor on this machine, so the blur cannot be read");
    assert_eq!(finding.refusal(), None);
    assert_eq!(
        Refusal::NoReason.describe(),
        "the item was not judged and the finding does not say why"
    );
}

#[test]
fn test_summary_never_counts_an_unverifiable_item_as_a_pass() {
    let items = check_items();
    let mut submission = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index == 0 {
            let mut unverifiable = finding(item.id, Verdict::Unverifiable);
            unverifiable.sample = None;
            unverifiable.reason =
                String::from("no compositor on this machine, so the blur cannot be read");
            submission.push(unverifiable);
        } else {
            submission.push(finding(item.id, Verdict::Pass));
        }
    }
    let summary = Summary::of(&submission, &items);
    assert_eq!(summary.passed, items.len() - 1);
    assert_eq!(summary.unverifiable, 1);
    assert_eq!(summary.failed, 0);
    assert!(summary.refused.is_empty());
    assert!(summary.missing.is_empty());
    assert!(
        !summary.is_clean(),
        "an item nobody could judge must keep the audit from passing"
    );
    assert_eq!(
        summary.describe(),
        format!(
            "{} passed, 0 failed, 1 unverifiable, 0 refused, 0 unanswered",
            items.len() - 1
        )
    );
}

#[test]
fn test_summary_is_clean_only_when_every_item_passed() {
    let items = check_items();
    let passing = all_pass(&items);
    assert!(Summary::of(&passing, &items).is_clean());

    let mut one_failed = passing.clone();
    one_failed[0].verdict = Verdict::Fail;
    let summary = Summary::of(&one_failed, &items);
    assert!(!summary.is_clean());
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.passed, items.len() - 1);
}

#[test]
fn test_summary_names_the_items_a_submission_never_answered() {
    let items = check_items();
    let submission = vec![finding(items[0].id, Verdict::Pass)];
    let summary = Summary::of(&submission, &items);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.missing.len(), items.len() - 1);
    assert!(summary.missing.iter().any(|id| id.as_str() == items[1].id));
    assert!(!summary.missing.iter().any(|id| id.as_str() == items[0].id));
    assert!(!summary.is_clean());
}

#[test]
fn test_summary_refuses_a_finding_about_an_item_the_checklist_does_not_ask_about() {
    let items = check_items();
    let submission = vec![finding("space.this-item-does-not-exist", Verdict::Pass)];
    let summary = Summary::of(&submission, &items);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.refused.len(), 1);
    assert_eq!(summary.refused[0].1, Refusal::UnknownItem);
    assert_eq!(
        summary.refused[0].1.describe(),
        "the finding names an item the checklist does not ask about"
    );
    assert!(!summary.is_clean());
}
