//! Tests for [`super`].
//!
//! They cover the classification rather than the channels: what each kind of evidence is
//! classified as, which class wins when two are supported at once, that a case nothing explains
//! is answered with the gaps rather than with a guess, and that a repair is never read before the
//! class it belongs to.
//!
//! Every fixture is built in memory. No test reads the specification, the metrics block, a
//! dictionary, a frame file or a display server: the facts a channel would produce arrive as
//! values, which is the property the module exists to have.

use crate::testd::evidence::{AssertionDiff, DefectRect, EvidenceValue, ImageDefect};
use crate::testd::heal::{AnchorView, HEAL_MARKER, HealCause, HealError, HealRecord};
use crate::testd::ui_metrics::{MetricMismatch, MetricSourceId, MetricValue};

use super::{
    Attribution, AttributionFacts, CaseRef, Diagnosis, Ground, LocatorFacts, RevisionFacts,
    attribute,
};

/// Every class, in the order the module declares them.
const CLASSES: [Attribution; 5] = [
    Attribution::ProductDefect,
    Attribution::StaleLocator,
    Attribution::RevisionRace,
    Attribution::CoordinateDrift,
    Attribution::InsufficientEvidence,
];

/// A recovery that re-derived a constant's name from the specification.
fn renamed(from: &str, to: &str) -> HealRecord {
    HealRecord {
        marker: HEAL_MARKER,
        locator: format!("the constant {from}"),
        cause: HealCause::Renamed {
            from: from.to_owned(),
            to: to.to_owned(),
        },
        evidence: vec![AnchorView {
            section: "#### 3.1.1",
            row: "候选单元高度",
            unit: "dp",
            value: String::from("48dp"),
        }],
    }
}

/// A recovery that moved a point the case had recorded.
fn reprojected() -> HealRecord {
    HealRecord {
        marker: HEAL_MARKER,
        locator: String::from("cell 1,0"),
        cause: HealCause::Reprojected {
            from: (0, 0),
            to: (48, 26),
        },
        evidence: Vec::new(),
    }
}

/// The refusal a name that moved together with its value earns.
fn drifted() -> HealError {
    HealError::Drifted {
        name: String::from("cell-height"),
        declared_name: String::from("candidate-cell-height"),
        declared: String::from("52px"),
        spec: String::from("48dp"),
    }
}

/// The refusal a locator that cannot re-derive its name earns.
fn unresolvable() -> HealError {
    HealError::Unresolvable {
        wanted: String::from("the constant cell-height"),
        detail: String::from("no declaration is built on it"),
    }
}

/// What the locator channel did for a case.
fn locator(healed: Vec<HealRecord>, refusals: Vec<HealError>) -> LocatorFacts {
    LocatorFacts { healed, refusals }
}

/// A failing assertion, named.
fn failed(name: &str) -> AssertionDiff {
    AssertionDiff {
        name: name.to_owned(),
        expected: EvidenceValue::fact("48"),
        actual: EvidenceValue::fact("52"),
    }
}

/// The case a fixture belongs to.
fn case() -> CaseRef {
    CaseRef {
        tc: String::from("TC-UI-07"),
        module: String::from("ui"),
    }
}

/// A metrics source that could not be read.
fn unreadable() -> MetricMismatch {
    MetricMismatch::Unavailable {
        source: MetricSourceId::Slint,
        reason: String::from("the file is not there"),
    }
}

/// A disagreement between the specification and a source.
fn disagreement() -> MetricMismatch {
    MetricMismatch::Value {
        name: String::from("grid-gap"),
        spec: MetricValue::LengthDp(6),
        code: MetricValue::LengthDp(10),
        source: MetricSourceId::Slint,
    }
}

/// A region of a snapshot the specification does not state.
fn defect() -> ImageDefect {
    ImageDefect {
        item: String::from("corner_radius"),
        image: String::from("01_default.png"),
        rect: DefectRect {
            x: 0,
            y: 0,
            w: 8,
            h: 8,
        },
        expected: EvidenceValue::fact("8dp"),
        measured: EvidenceValue::fact("2px"),
    }
}

/// The facts of a case that failed because the product disagrees with the specification.
fn product_facts() -> AttributionFacts {
    AttributionFacts {
        case: Some(case()),
        locator: locator(Vec::new(), vec![drifted()]),
        diffs: vec![failed("cell_height")],
        ..Default::default()
    }
}

/// The facts of a case that failed because a name it used is gone.
fn stale_facts() -> AttributionFacts {
    AttributionFacts {
        case: Some(case()),
        locator: locator(
            vec![renamed("cell-height", "candidate-cell-height")],
            Vec::new(),
        ),
        diffs: vec![failed("cell_height")],
        ..Default::default()
    }
}

/// The facts of a case that failed because the frame it asserted on was not the one it awaited.
fn race_facts() -> AttributionFacts {
    AttributionFacts {
        case: Some(case()),
        revision: Some(RevisionFacts {
            expected: 9,
            seen: 7,
            rewinds: 1,
        }),
        diffs: vec![failed("first_candidate")],
        ..Default::default()
    }
}

/// The facts of a case that failed because the point it recorded had drifted.
fn drift_facts() -> AttributionFacts {
    AttributionFacts {
        case: Some(case()),
        locator: locator(vec![reprojected()], Vec::new()),
        diffs: vec![failed("candidate_cell_centre")],
        ..Default::default()
    }
}

/// The facts of a case that failed with nothing that names a cause.
fn bare_facts() -> AttributionFacts {
    AttributionFacts {
        case: Some(case()),
        diffs: vec![failed("first_candidate")],
        ..Default::default()
    }
}

/// One diagnosis per class, each from the facts that support it.
fn one_of_each() -> Vec<Diagnosis> {
    vec![
        attribute(&product_facts()),
        attribute(&stale_facts()),
        attribute(&race_facts()),
        attribute(&drift_facts()),
        attribute(&bare_facts()),
    ]
}

/// The disposition section of a prompt, as the renderer writes it.
fn disposition_section(prompt: &str) -> String {
    let heading = "## 处置\n";
    let start = prompt
        .find(heading)
        .expect("every prompt has a disposition section");
    let rest = &prompt[start + heading.len()..];
    let end = rest
        .find("\n\n## 红线")
        .expect("the red lines follow the disposition");
    rest[..end].to_owned()
}

#[test]
fn test_attribute_renamed_constant_classifies_the_locator_as_stale() {
    let diagnosis = attribute(&stale_facts());

    assert_eq!(diagnosis.class(), Attribution::StaleLocator);
    assert!(
        diagnosis.repair().is_some(),
        "a stale name is repaired in the test script"
    );
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Renamed { from, to, row })
            if from == "cell-height" && to == "candidate-cell-height"
                && row.contains("候选单元高度")
    ));
    let prompt = diagnosis.prompt();
    assert!(prompt.contains("candidate-cell-height"), "{prompt}");
    assert!(
        !prompt.contains("hit_map"),
        "a stale name must not carry another class's repair: {prompt}"
    );
}

#[test]
fn test_attribute_unresolvable_name_classifies_the_locator_as_stale() {
    let facts = AttributionFacts {
        locator: locator(Vec::new(), vec![unresolvable()]),
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(diagnosis.class(), Attribution::StaleLocator);
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Unresolvable { detail, .. }) if detail.contains("no declaration")
    ));
}

#[test]
fn test_attribute_reprojected_point_classifies_the_coordinate_as_drifted() {
    let diagnosis = attribute(&drift_facts());

    assert_eq!(diagnosis.class(), Attribution::CoordinateDrift);
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Reprojected { locator, from, to })
            if locator == "cell 1,0" && *from == (0, 0) && *to == (48, 26)
    ));
    let prompt = diagnosis.prompt();
    assert!(
        prompt.contains("hit_map"),
        "the repair has to name the map the relative coordinates come from: {prompt}"
    );
    assert!(prompt.contains("几何关系断言"), "{prompt}");
    assert!(prompt.contains("coords.rs"), "{prompt}");
}

#[test]
fn test_attribute_stale_revision_classifies_the_race() {
    let diagnosis = attribute(&race_facts());

    assert_eq!(diagnosis.class(), Attribution::RevisionRace);
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Stale { expected, seen, rewinds }) if *expected == 9 && *seen == 7 && *rewinds == 1
    ));
    let prompt = diagnosis.prompt();
    assert!(prompt.contains("FrameWatch"), "{prompt}");
    assert!(prompt.contains("revision"), "{prompt}");
    assert!(
        prompt.contains("不要用 sleep"),
        "the repair has to forbid the sleep it replaces: {prompt}"
    );
}

#[test]
fn test_attribute_drifted_value_classifies_the_product_and_proposes_no_repair() {
    let diagnosis = attribute(&product_facts());

    assert_eq!(diagnosis.class(), Attribution::ProductDefect);
    assert!(
        diagnosis.repair().is_none(),
        "a finding about the product is reported and never repaired here"
    );
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Drifted { name, declared, spec, .. })
            if name == "cell-height" && declared == "52px" && spec == "48dp"
    ));
    let prompt = diagnosis.prompt();
    assert!(prompt.contains("停止自愈"), "{prompt}");
    assert!(
        prompt.contains("不产出任何修复建议"),
        "the disposition has to say that no repair follows: {prompt}"
    );
    assert!(
        !prompt.contains("改成重推导出的新名"),
        "no other class's repair may leak into a product finding: {prompt}"
    );
}

#[test]
fn test_attribute_metric_disagreement_classifies_the_product() {
    let facts = AttributionFacts {
        mismatches: vec![disagreement()],
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(diagnosis.class(), Attribution::ProductDefect);
    assert!(diagnosis.repair().is_none());
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Disagreement { item })
            if item.contains("grid-gap") && item.contains("6dp") && item.contains("10dp")
    ));
}

#[test]
fn test_attribute_snapshot_defect_classifies_the_product() {
    let facts = AttributionFacts {
        defects: vec![defect()],
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(diagnosis.class(), Attribution::ProductDefect);
    assert!(matches!(
        diagnosis.grounds().first(),
        Some(Ground::Defect { item, image, rect })
            if item == "corner_radius" && image == "01_default.png" && rect.w == 8
    ));
    assert!(diagnosis.prompt().contains("01_default.png"));
}

#[test]
fn test_attribute_evidence_that_names_no_cause_answers_with_the_gaps() {
    let diagnosis = attribute(&bare_facts());

    assert_eq!(diagnosis.class(), Attribution::InsufficientEvidence);
    assert!(
        diagnosis.repair().is_none(),
        "a class nothing supports may not propose a repair"
    );
    assert!(
        diagnosis.grounds().iter().any(|ground| matches!(
            ground,
            Ground::Gap { detail } if detail.contains("定位器通道")
        )),
        "the prompt has to name what the locator channel did not say: {:?}",
        diagnosis.grounds()
    );
    assert!(
        diagnosis.grounds().iter().any(|ground| matches!(
            ground,
            Ground::Gap { detail } if detail.contains("交叉检查")
        )),
        "{:?}",
        diagnosis.grounds()
    );
    assert!(
        diagnosis.prompt().contains("不猜"),
        "{}",
        diagnosis.prompt()
    );
}

#[test]
fn test_attribute_unreadable_metric_source_is_a_gap_and_not_a_finding() {
    let facts = AttributionFacts {
        mismatches: vec![unreadable()],
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(
        diagnosis.class(),
        Attribution::InsufficientEvidence,
        "a source that could not be read compared nothing, so it cannot be a finding"
    );
    assert!(
        diagnosis.grounds().iter().any(|ground| matches!(
            ground,
            Ground::Gap { detail } if detail.contains("the file is not there")
        )),
        "{:?}",
        diagnosis.grounds()
    );
}

#[test]
fn test_attribute_clean_revision_rules_the_race_out() {
    let facts = AttributionFacts {
        revision: Some(RevisionFacts {
            expected: 9,
            seen: 9,
            rewinds: 0,
        }),
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(diagnosis.class(), Attribution::InsufficientEvidence);
    assert!(
        diagnosis.grounds().iter().any(|ground| matches!(
            ground,
            Ground::RuledOut { class, .. } if *class == "RevisionRace"
        )),
        "a reading that could have shown the race and did not has to say so: {:?}",
        diagnosis.grounds()
    );
}

/// The name of the assertion each class's fixture failed on.
fn diagnosis_name(diagnosis: &Diagnosis) -> &'static str {
    match diagnosis.class() {
        Attribution::ProductDefect | Attribution::StaleLocator => "cell_height",
        Attribution::RevisionRace | Attribution::InsufficientEvidence => "first_candidate",
        Attribution::CoordinateDrift => "candidate_cell_centre",
    }
}

#[test]
fn test_attribute_carries_the_failed_assertions_into_every_diagnosis() {
    for diagnosis in one_of_each() {
        assert!(
            diagnosis.grounds().iter().any(|ground| matches!(
                ground,
                Ground::Failed { name } if name == diagnosis_name(&diagnosis)
            )),
            "{:?} lost the assertion that failed: {:?}",
            diagnosis.class(),
            diagnosis.grounds()
        );
        assert_eq!(
            diagnosis.grounds().last(),
            Some(&Ground::Failed {
                name: diagnosis_name(&diagnosis).to_owned()
            }),
            "the failed assertions are the last grounds of every diagnosis"
        );
    }
}

#[test]
fn test_attribute_prefers_the_product_over_a_locator_that_moved() {
    let facts = AttributionFacts {
        locator: locator(
            vec![renamed("cell-height", "candidate-cell-height")],
            vec![drifted()],
        ),
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(
        diagnosis.class(),
        Attribution::ProductDefect,
        "no repair may be proposed while the source disagrees with the specification"
    );
    assert_eq!(diagnosis.secondary(), &[Attribution::StaleLocator]);
    assert!(diagnosis.repair().is_none());
}

#[test]
fn test_attribute_keeps_an_outranked_class_as_secondary() {
    let facts = AttributionFacts {
        locator: locator(
            vec![
                renamed("cell-height", "candidate-cell-height"),
                reprojected(),
            ],
            Vec::new(),
        ),
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(diagnosis.class(), Attribution::StaleLocator);
    assert_eq!(diagnosis.secondary(), &[Attribution::CoordinateDrift]);
    assert!(
        diagnosis.prompt().contains("A 坐标漂移"),
        "an outranked class is named rather than dropped: {}",
        diagnosis.prompt()
    );
}

#[test]
fn test_attribute_prefers_the_race_over_a_coordinate_that_drifted() {
    let facts = AttributionFacts {
        locator: locator(vec![reprojected()], Vec::new()),
        revision: Some(RevisionFacts {
            expected: 9,
            seen: 9,
            rewinds: 2,
        }),
        ..bare_facts()
    };

    let diagnosis = attribute(&facts);

    assert_eq!(
        diagnosis.class(),
        Attribution::RevisionRace,
        "a file that went backwards is a race even when the revision read is the awaited one"
    );
    assert_eq!(diagnosis.secondary(), &[Attribution::CoordinateDrift]);
}

#[test]
fn test_attribute_answers_a_case_with_no_facts_at_all() {
    let diagnosis = attribute(&AttributionFacts::default());

    assert_eq!(diagnosis.class(), Attribution::InsufficientEvidence);
    assert_eq!(diagnosis.case(), None);
    assert!(
        !diagnosis.grounds().is_empty(),
        "even an empty input has to say which facts are missing"
    );
    assert!(
        diagnosis.prompt().contains("## 红线"),
        "{}",
        diagnosis.prompt()
    );
}

#[test]
fn test_label_and_code_name_every_class_distinctly() {
    let mut labels = Vec::new();
    let mut codes = Vec::new();
    for class in CLASSES {
        let label = class.label();
        let code = class.code();
        assert!(!label.is_empty() && !code.is_empty(), "{class:?}");
        assert_ne!(label, code, "the two renderings are not the same string");
        labels.push(label);
        codes.push(code);
    }
    labels.sort_unstable();
    labels.dedup();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(labels.len(), CLASSES.len(), "two classes share a label");
    assert_eq!(codes.len(), CLASSES.len(), "two classes share a code");
}

#[test]
fn test_repair_is_absent_exactly_for_the_classes_that_propose_none() {
    for class in CLASSES {
        let expected = match class {
            Attribution::ProductDefect | Attribution::InsufficientEvidence => None,
            Attribution::StaleLocator
            | Attribution::RevisionRace
            | Attribution::CoordinateDrift => Some(class.disposition()),
        };
        assert_eq!(class.repair(), expected, "{class:?}");
    }
}

#[test]
fn test_disposition_of_a_repairing_class_is_the_repair_itself() {
    for class in CLASSES {
        let disposition = class.disposition();
        assert!(!disposition.is_empty(), "{class:?}");
        match class.repair() {
            Some(repair) => assert_eq!(repair, disposition),
            None => assert!(
                disposition.contains("不产出") || disposition.contains("停止自愈"),
                "a class with no repair has to say what is done instead: {disposition}"
            ),
        }
    }
}

#[test]
fn test_prompt_reports_the_class_before_the_disposition() {
    let diagnoses = one_of_each();
    assert_eq!(diagnoses.len(), CLASSES.len());
    for (diagnosis, class) in diagnoses.iter().zip(CLASSES) {
        assert_eq!(diagnosis.class(), class);
        let prompt = diagnosis.prompt();
        let class_at = prompt
            .find(class.label())
            .unwrap_or_else(|| panic!("{} names no class: {prompt}", class.code()));
        let disposition_at = prompt
            .find("## 处置")
            .unwrap_or_else(|| panic!("{} has no disposition: {prompt}", class.code()));
        assert!(
            class_at < disposition_at,
            "{} reports its disposition before its class: {prompt}",
            class.code()
        );
        assert_eq!(
            disposition_section(&prompt),
            class.disposition(),
            "{} renders a disposition that is not its own",
            class.code()
        );
    }
}

#[test]
fn test_prompt_names_the_case_and_the_directory_its_evidence_lands_in() {
    let prompt = attribute(&stale_facts()).prompt();

    assert!(prompt.contains("TC-UI-07"), "{prompt}");
    assert!(prompt.contains("ui"), "{prompt}");
    assert!(
        prompt.contains("results/runs/<run>/ui/TC-UI-07/"),
        "{prompt}"
    );
    assert!(prompt.contains("# 归因与自愈"), "{prompt}");
    assert!(prompt.contains("## 证据"), "{prompt}");
}

#[test]
fn test_prompt_carries_the_red_lines_the_guard_enforces() {
    let prompt = attribute(&drift_facts()).prompt();

    for line in [
        "## 红线",
        "crates/",
        "docs/dev/budgets.json",
        "docs/dev/features.md",
    ] {
        assert!(
            prompt.contains(line),
            "the red lines must carry {line}: {prompt}"
        );
    }
    assert!(prompt.contains("xtask/src/testd/"), "{prompt}");
    assert!(prompt.contains("docs/dev/tests/"), "{prompt}");
    assert!(
        prompt.contains("越界即整轮失败"),
        "the prompt has to say what crossing a red line costs: {prompt}"
    );
}

#[test]
fn test_revision_facts_is_stale_for_a_frame_that_was_not_awaited() {
    let awaited = RevisionFacts {
        expected: 7,
        seen: 7,
        rewinds: 0,
    };
    assert!(
        !awaited.is_stale(),
        "the awaited frame is the awaited frame"
    );

    let behind = RevisionFacts {
        expected: 7,
        seen: 6,
        rewinds: 0,
    };
    assert!(behind.is_stale(), "the frame had not arrived yet");

    let ahead = RevisionFacts {
        expected: 7,
        seen: 8,
        rewinds: 0,
    };
    assert!(
        ahead.is_stale(),
        "the case read a frame it did not mean to read"
    );

    let rewound = RevisionFacts {
        expected: 7,
        seen: 7,
        rewinds: 1,
    };
    assert!(rewound.is_stale(), "the file went backwards");
}

#[test]
fn test_case_ref_renders_the_bundle_directory_and_the_case_line() {
    let case = case();

    assert_eq!(case.bundle_dir(), "results/runs/<run>/ui/TC-UI-07/");
    assert!(
        case.bundle_dir().ends_with('/'),
        "the bundle directory has to read as a directory"
    );
    let line = case.line();
    assert!(line.contains("TC-UI-07") && line.contains("ui"), "{line}");
    assert!(line.contains(&case.bundle_dir()), "{line}");
}

#[test]
fn test_ground_line_renders_every_variant_without_repeating_itself() {
    let grounds = [
        Ground::Renamed {
            from: String::from("grid-gap"),
            to: String::from("candidate-grid-gap"),
            row: String::from("docs/dev/features.md 的 网格列间距 = 6dp"),
        },
        Ground::Drifted {
            name: String::from("cell-height"),
            declared_name: String::from("candidate-cell-height"),
            declared: String::from("52px"),
            spec: String::from("48dp"),
        },
        Ground::Unresolvable {
            wanted: String::from("the constant cell-height"),
            detail: String::from("no declaration is built on it"),
        },
        Ground::Reprojected {
            locator: String::from("cell 1,0"),
            from: (0, 0),
            to: (48, 26),
        },
        Ground::Stale {
            expected: 9,
            seen: 7,
            rewinds: 1,
        },
        Ground::Disagreement {
            item: String::from("grid-gap: the specification says 6dp"),
        },
        Ground::Defect {
            item: String::from("corner_radius"),
            image: String::from("01_default.png"),
            rect: DefectRect {
                x: 0,
                y: 0,
                w: 8,
                h: 8,
            },
        },
        Ground::Failed {
            name: String::from("first_candidate"),
        },
        Ground::Gap {
            detail: String::from("nothing to go on"),
        },
        Ground::RuledOut {
            class: Attribution::RevisionRace.code(),
            detail: String::from("the frame was the awaited one"),
        },
    ];

    let mut lines = Vec::new();
    for ground in &grounds {
        let line = ground.line();
        assert!(!line.is_empty(), "{ground:?} renders nothing");
        lines.push(line);
    }
    assert_eq!(lines.len(), 10, "every variant has to be covered");
    assert!(
        lines[0].contains("grid-gap") && lines[0].contains("6dp"),
        "{}",
        lines[0]
    );
    assert!(
        lines[3].contains("0,0") && lines[3].contains("48,26"),
        "{}",
        lines[3]
    );
    assert!(
        lines[4].contains("9") && lines[4].contains("7"),
        "{}",
        lines[4]
    );
    assert!(lines[6].contains("8x8"), "{}", lines[6]);
    assert!(lines[9].contains("RevisionRace"), "{}", lines[9]);

    let mut unique = lines.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        lines.len(),
        "two variants render the same line"
    );
}

#[test]
fn test_diagnosis_accessors_report_what_the_evidence_held() {
    let diagnosis = attribute(&race_facts());

    assert_eq!(diagnosis.class(), Attribution::RevisionRace);
    assert_eq!(diagnosis.case(), Some(&case()));
    assert_eq!(diagnosis.repair(), Attribution::RevisionRace.repair());
    assert!(
        diagnosis.secondary().is_empty(),
        "{:?}",
        diagnosis.secondary()
    );
    assert!(
        diagnosis.grounds().len() >= 2,
        "the race and the failed assertion are both grounds: {:?}",
        diagnosis.grounds()
    );
}
