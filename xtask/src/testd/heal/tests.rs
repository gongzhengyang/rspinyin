//! Tests for [`super`].
//!
//! They cover the locators rather than the window: what a name resolves to, what happens when a
//! name is gone and the specification can supply it again, what happens when a *value* moved --
//! which must never heal -- and what a recovery leaves in the report.
//!
//! The tests that need a specification read the repository's own `docs/dev/features.md`, and the
//! ones that need a `.slint` source build one in memory from the values that document states, so
//! no fixture is a second copy of a number the specification owns. None of them needs a display
//! server, a running plugin or a dictionary.
//!
//! The one fixture that is deliberately not the repository's own is the sweep over the grid
//! table's exceptions: it asserts that every constant that table names resolves against the real
//! source, and that the one constant whose row is not a geometry row is the only one that does
//! not.

use std::fs;
use std::path::PathBuf;

use ime_types::{ColorScheme, LayoutHint, Rgba8, ThemeSpec};

use crate::testd::ui_metrics::{
    BackendReading, MetricMismatch, MetricSource, MetricSourceId, MetricValue, SLINT_DOCUMENT,
    SPEC_DOCUMENT, SlintConstants, SpecTable, UiMetrics, Unit, alpha_byte, cross_check_spec,
};

use super::{
    AnchorView, ConstRef, HEAL_MARKER, HealCause, HealError, HealLog, HitQuery, HitRef, resolve,
    resolve_hit,
};

/// The invariants a re-derivation keeps, swept over every constant the specification's own
/// exception table anchors: one step, exact, and never a value the row does not state.
mod invariants;

/// The repository root, derived from the compile-time location of this crate.
fn repo_root() -> PathBuf {
    crate::budget::repo_root().expect("xtask lives in a subdirectory of the repository root")
}

/// The repository's own specification, parsed.
fn spec() -> SpecTable {
    let path = repo_root().join(SPEC_DOCUMENT);
    let text = fs::read_to_string(&path).expect("reading the specification");
    SpecTable::parse(&text).expect("the specification parses")
}

/// The repository's own metrics block, parsed.
fn repository_constants() -> SlintConstants {
    let path = repo_root().join(SLINT_DOCUMENT);
    match SlintConstants::load(&path).expect("reading the metrics block") {
        MetricSource::Available(constants) => constants,
        MetricSource::Unavailable { reason } => {
            panic!("the metrics block the cases are written against is not readable: {reason}")
        }
    }
}

/// The value the specification states for one row of the geometry table.
fn stated(spec: &SpecTable, row: &str, unit: Unit, index: usize) -> u16 {
    spec.row(row)
        .unwrap_or_else(|| panic!("the geometry table has no row `{row}`"))
        .value(unit, index)
        .unwrap_or_else(|| panic!("`{row}` states no value at {index}"))
        .number()
        .unwrap_or_else(|| panic!("`{row}` states a value that is not a whole number"))
}

/// A metrics block declaring one `out property` per pair.
fn declarations(pairs: &[(&str, u16)]) -> SlintConstants {
    let mut text = String::from("export global CandidateMetrics {\n");
    for (name, value) in pairs {
        text.push_str(&format!("    out property <length> {name}: {value}px;\n"));
    }
    text.push_str("}\n");
    SlintConstants::parse("a metrics fixture", &text).expect("the fixture parses")
}

/// A component declaring one `public constant` per pair, the form the window itself uses.
fn component_constants(pairs: &[(&str, u16)]) -> SlintConstants {
    let mut text = String::from("export component CandidateWindow inherits Window {\n");
    for (name, value) in pairs {
        text.push_str(&format!("    public constant {name}: {value}px;\n"));
    }
    text.push_str("}\n");
    SlintConstants::parse("a component fixture", &text).expect("the fixture parses")
}

/// A metrics block declaring every exception the grid table names, with one value replaced.
fn exception_source(spec: &SpecTable, drift: Option<(&str, u16)>) -> SlintConstants {
    let mut text = String::from("export global CandidateMetrics {\n");
    for (name, value) in spec.exceptions() {
        let value = match drift {
            Some((drifted, replacement)) if drifted == name.as_str() => replacement,
            _ => *value,
        };
        text.push_str(&format!("    out property <length> {name}: {value}px;\n"));
    }
    text.push_str("}\n");
    SlintConstants::parse("an exception fixture", &text).expect("the fixture parses")
}

/// The declarations a cell's centre is derived from, at the values the specification states.
fn cell_declarations(spec: &SpecTable) -> Vec<(&'static str, u16)> {
    vec![
        ("container-padding", stated(spec, "容器内边距", Unit::Dp, 0)),
        ("header-height", stated(spec, "Header 高度", Unit::Dp, 0)),
        (
            "separator-height",
            stated(spec, "Header 分隔线", Unit::Dp, 0),
        ),
        (
            "cell-min-width",
            stated(spec, "候选单元最小宽度", Unit::Dp, 0),
        ),
        ("cell-height", stated(spec, "候选单元高度", Unit::Dp, 0)),
        ("grid-gap", stated(spec, "网格列间距", Unit::Dp, 0)),
        (
            "max-per-row-limit",
            stated(spec, "单行最大候选数", Unit::Count, 2),
        ),
    ]
}

/// The same declarations with one name replaced, as a rename would leave them.
fn with_renamed(
    pairs: &[(&'static str, u16)],
    from: &str,
    to: &'static str,
) -> Vec<(&'static str, u16)> {
    pairs
        .iter()
        .map(|(name, value)| {
            if *name == from {
                (to, *value)
            } else {
                (*name, *value)
            }
        })
        .collect()
}

/// The centre of a cell, computed from the values the specification itself states.
fn expected_centre(spec: &SpecTable, column: u16, row: u16) -> (i32, i32) {
    let padding = i32::from(stated(spec, "容器内边距", Unit::Dp, 0));
    let header = i32::from(stated(spec, "Header 高度", Unit::Dp, 0));
    let separator = i32::from(stated(spec, "Header 分隔线", Unit::Dp, 0));
    let width = i32::from(stated(spec, "候选单元最小宽度", Unit::Dp, 0));
    let height = i32::from(stated(spec, "候选单元高度", Unit::Dp, 0));
    let gap = i32::from(stated(spec, "网格列间距", Unit::Dp, 0));
    let x = padding + i32::from(column) * (width + gap) + width / 2;
    let y = header + separator + padding + i32::from(row) * (height + gap) + height / 2;
    (x, y)
}

/// A window that agrees with the specification in every value the cross-check reads.
fn live(spec: &SpecTable) -> UiMetrics {
    let theme = ThemeSpec {
        scheme: ColorScheme::Dark,
        accent: Rgba8 {
            r: 0x4C,
            g: 0x9A,
            b: 0xFF,
            a: 255,
        },
        acrylic: true,
        base_alpha: alpha_byte(
            spec.colour("surface.base")
                .and_then(|row| row.dark.alpha)
                .expect("the colour table states the acrylic alpha"),
        ),
        corner_radius_dp: stated(spec, "容器圆角", Unit::Dp, 0),
        scale: 1.0,
    };
    let layout = LayoutHint {
        max_per_row: u8::try_from(stated(spec, "单行最大候选数", Unit::Count, 0))
            .expect("a per-row maximum fits in a byte"),
        show_annotation: true,
        max_width_dp: stated(spec, "候选框最大宽度", Unit::Dp, 0),
    };
    let backend = BackendReading {
        backend_id: "x11",
        window_dp: (320, 140),
        window_px: (320, 140),
        scale: 1.0,
        argb_visual: true,
        compositor_present: true,
        base_alpha: theme.base_alpha,
    };
    UiMetrics::new(backend, layout, theme)
}

#[test]
fn test_resolve_by_name_finds_the_declared_constant_and_its_row() {
    let spec = spec();
    let constants = repository_constants();

    let resolved =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the name resolves");

    assert!(
        !resolved.healed,
        "the name is declared, so nothing had to be re-derived"
    );
    assert_eq!(resolved.name, "cell-height");
    assert_eq!(resolved.renamed_from, None);
    assert_eq!(resolved.anchor.row, "候选单元高度");
    assert_eq!(
        resolved.anchor.number().map(u32::from),
        Some(resolved.value_dp)
    );
    assert_eq!(
        resolved.value_dp,
        u32::from(stated(&spec, "候选单元高度", Unit::Dp, 0))
    );
    assert!(resolved.matches_spec());
}

#[test]
fn test_resolve_by_name_accepts_either_spelling_of_a_name() {
    let spec = spec();
    let constants = repository_constants();

    let kebab = resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("it resolves");
    let pascal = resolve(ConstRef::ByName("CellHeight"), &constants, &spec).expect("it resolves");

    assert_eq!(
        kebab.name, pascal.name,
        "both spellings name the one declaration"
    );
    assert_eq!(kebab.value_dp, pascal.value_dp);
    // A name that merely resembles one the harness knows is not a spelling of it.
    assert!(matches!(
        resolve(ConstRef::ByName("cellheight"), &constants, &spec),
        Err(HealError::Unresolvable { .. })
    ));
}

#[test]
fn test_resolve_by_spec_row_finds_the_constant_the_row_fixes() {
    let spec = spec();
    let constants = repository_constants();

    for section in ["#### 3.1.1", "3.1.1"] {
        let resolved = resolve(
            ConstRef::BySpecRow {
                section,
                row: "候选单元高度",
            },
            &constants,
            &spec,
        )
        .expect("the row fixes one constant");
        assert_eq!(
            resolved.name, "cell-height",
            "section written as {section:?}"
        );
        assert!(!resolved.healed);
    }
}

#[test]
fn test_resolve_after_a_rename_heals_from_the_specification_row() {
    let spec = spec();
    let renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "candidate-cell-height",
    );
    let constants = declarations(&renamed);

    let resolved =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the rename heals");

    assert!(
        resolved.healed,
        "the name was gone and the row supplied it again"
    );
    assert_eq!(resolved.name, "candidate-cell-height");
    assert_eq!(resolved.renamed_from, Some("cell-height"));
    assert_eq!(resolved.anchor.row, "候选单元高度");
    assert!(
        resolved.matches_spec(),
        "a healed constant carries the value the row states, by construction"
    );
}

#[test]
fn test_resolve_heals_a_public_constant_declared_in_pascal_case() {
    let spec = spec();
    let renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "CandidateCellHeight",
    );
    let constants = component_constants(&renamed);

    let resolved =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the rename heals");

    assert!(resolved.healed);
    assert_eq!(
        resolved.name, "candidate-cell-height",
        "the metrics reader indexes a component constant under both spellings, and the spelling \
         matching the locator's own is the one reported"
    );
    assert!(resolved.matches_spec());
}

#[test]
fn test_resolve_leaves_a_changed_value_to_the_cross_check() {
    let spec = spec();
    let drifted = 10;
    let source = exception_source(&spec, Some(("grid-gap", drifted)));

    let resolved =
        resolve(ConstRef::ByName("grid-gap"), &source, &spec).expect("the name is declared");

    assert!(
        !resolved.healed,
        "a value that moved is not a rename, so nothing is healed"
    );
    assert_eq!(resolved.name, "grid-gap");
    assert_eq!(resolved.value_dp, u32::from(drifted));
    assert!(
        !resolved.matches_spec(),
        "the declaration no longer states what the row states"
    );
    assert_eq!(
        cross_check_spec(&spec, &live(&spec), &MetricSource::Available(source)),
        vec![MetricMismatch::Value {
            name: String::from("grid-gap"),
            spec: MetricValue::LengthDp(stated(&spec, "网格列间距", Unit::Dp, 0)),
            code: MetricValue::LengthDp(drifted),
            source: MetricSourceId::Slint,
        }],
        "the drift has to be reported, and reported as the only disagreement"
    );
}

#[test]
fn test_resolve_refuses_a_name_and_a_row_that_are_both_absent() {
    let spec = spec();
    let constants = repository_constants();

    let by_name = resolve(ConstRef::ByName("candidate-cell-height"), &constants, &spec);
    assert!(
        matches!(&by_name, Err(HealError::Unresolvable { wanted, .. }) if wanted.contains("candidate-cell-height")),
        "a name no row fixes is refused by name: {by_name:?}"
    );

    let by_row = resolve(
        ConstRef::BySpecRow {
            section: "#### 3.1.1",
            row: "候选单元高度上限",
        },
        &constants,
        &spec,
    );
    assert!(
        matches!(by_row, Err(HealError::Unresolvable { .. })),
        "a row the document does not have is refused: {by_row:?}"
    );
}

#[test]
fn test_resolve_refuses_a_name_the_geometry_table_has_no_row_for() {
    let spec = spec();
    let constants = repository_constants();

    // A real constant of the metrics block, fixed by a material row the geometry table does not
    // hold: a locator may not invent a row for it, and may not guess one from the value either.
    let resolved = resolve(ConstRef::ByName("shadow-blur"), &constants, &spec);
    assert!(
        matches!(resolved, Err(HealError::Unresolvable { .. })),
        "a constant no geometry row fixes is refused: {resolved:?}"
    );
}

#[test]
fn test_resolve_reports_a_rename_whose_value_moved_too_as_drift() {
    let spec = spec();
    let value = stated(&spec, "候选单元高度", Unit::Dp, 0);
    let moved = value + 4;
    let mut renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "candidate-cell-height",
    );
    for entry in &mut renamed {
        if entry.0 == "candidate-cell-height" {
            entry.1 = moved;
        }
    }
    let constants = declarations(&renamed);

    let resolved = resolve(ConstRef::ByName("cell-height"), &constants, &spec);
    // Both sides are stated in the row's own unit. The `.slint` source writes `px` and the
    // specification writes `dp`, and they are the same unit here, so the refusal normalises
    // them: two numbers a reader has to convert before comparing them are two numbers the
    // message has not finished delivering.
    assert_eq!(
        resolved,
        Err(HealError::Drifted {
            name: String::from("cell-height"),
            declared_name: String::from("candidate-cell-height"),
            declared: format!("{moved}dp"),
            spec: format!("{value}dp"),
        }),
        "a name that moved together with its value is drift, and the refusal has to name both \
         sides so that a reader can tell it from a locator that merely lost its constant"
    );
}

#[test]
fn test_resolve_refuses_a_row_that_fixes_more_than_one_constant() {
    let spec = spec();
    let constants = repository_constants();

    let resolved = resolve(
        ConstRef::BySpecRow {
            section: "#### 3.1.1",
            row: "候选单元内边距",
        },
        &constants,
        &spec,
    );
    assert!(
        matches!(&resolved, Err(HealError::Unresolvable { detail, .. }) if detail.contains("2 constants")),
        "a row that states two values has not said which constant it means: {resolved:?}"
    );
}

#[test]
fn test_resolve_against_the_repository_sources_heals_nothing() {
    let spec = spec();
    let constants = repository_constants();

    let mut unresolved = Vec::new();
    let mut checked = 0;
    for name in spec.exceptions().keys() {
        match resolve(ConstRef::ByName(name.as_str()), &constants, &spec) {
            Ok(resolved) => {
                checked += 1;
                assert!(!resolved.healed, "{name} is declared, so nothing is healed");
                assert!(
                    resolved.matches_spec(),
                    "{name}: the declaration and the specification disagree"
                );
            }
            Err(_) => unresolved.push(name.clone()),
        }
    }

    assert_eq!(
        unresolved,
        ["shadow-inner-offset-y", "shadow-inner-spread"],
        "the exceptions whose rows are not geometry rows are the constants the table cannot \
         anchor; any other is a gap in the table"
    );
    assert_eq!(
        checked + unresolved.len(),
        spec.exceptions().len(),
        "the sweep has to walk every exception, not pass over some of them"
    );
    assert!(
        checked >= 11,
        "the table has to cover the geometry rows rather than resolve nothing: {checked}"
    );
}

#[test]
fn test_resolve_hit_derives_a_cell_centre_from_the_specification() {
    let spec = spec();
    let constants = declarations(&cell_declarations(&spec));
    let query = HitQuery {
        target: HitRef::Cell { column: 1, row: 0 },
        recorded: None,
    };

    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    assert_eq!(hit.point, expected_centre(&spec, 1, 0));
    assert!(!hit.healed, "nothing was recorded and nothing moved");
    assert_eq!(hit.moved_from, None);
    assert_eq!(
        hit.constants.len(),
        6,
        "the derivation is anchored on six rows, which is what the report cites"
    );
    assert!(hit.constants.iter().all(|c| !c.healed && c.matches_spec()));
}

#[test]
fn test_resolve_hit_heals_a_recorded_point_the_constants_moved() {
    let spec = spec();
    let constants = declarations(&cell_declarations(&spec));
    let stale = (0, 0);
    let query = HitQuery {
        target: HitRef::Cell { column: 2, row: 1 },
        recorded: Some(stale),
    };

    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    assert!(
        hit.healed,
        "the recorded point is not the one the specification produces"
    );
    assert_eq!(hit.moved_from, Some(stale));
    assert_eq!(hit.point, expected_centre(&spec, 2, 1));
}

#[test]
fn test_resolve_hit_heals_when_a_constant_it_is_built_on_was_renamed() {
    let spec = spec();
    let renamed = with_renamed(&cell_declarations(&spec), "grid-gap", "candidate-grid-gap");
    let constants = declarations(&renamed);
    let query = HitQuery {
        target: HitRef::Cell { column: 1, row: 0 },
        recorded: None,
    };

    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    assert!(hit.healed, "a constant the point is built on was renamed");
    assert_eq!(
        hit.point,
        expected_centre(&spec, 1, 0),
        "the point follows the renamed constant to the same value"
    );
    let mut log = HealLog::new();
    assert!(log.note_hit(&query, &hit), "the recovery is recorded");
    assert!(matches!(
        &log.records()[0].cause,
        HealCause::Renamed { from, to } if from == "grid-gap" && to == "candidate-grid-gap"
    ));
}

#[test]
fn test_resolve_hit_does_not_heal_a_point_a_drifted_value_moved() {
    let spec = spec();
    let drifted = stated(&spec, "网格列间距", Unit::Dp, 0) + 4;
    let mut pairs = cell_declarations(&spec);
    for entry in &mut pairs {
        if entry.0 == "grid-gap" {
            entry.1 = drifted;
        }
    }
    let constants = declarations(&pairs);
    let query = HitQuery {
        target: HitRef::Cell { column: 1, row: 0 },
        recorded: Some(expected_centre(&spec, 1, 0)),
    };

    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    assert_ne!(
        hit.point,
        expected_centre(&spec, 1, 0),
        "the drifted gap does move the point"
    );
    assert_eq!(hit.moved_from, Some(expected_centre(&spec, 1, 0)));
    assert!(
        !hit.healed,
        "a point that moved because a value moved is drift, not a recovery, so the case must fail"
    );
    let mut log = HealLog::new();
    assert!(!log.note_hit(&query, &hit), "nothing is recorded as healed");
    assert!(log.is_empty());
}

#[test]
fn test_resolve_hit_refuses_a_cell_the_specification_cannot_draw() {
    let spec = spec();
    let constants = declarations(&cell_declarations(&spec));
    let ceiling = stated(&spec, "单行最大候选数", Unit::Count, 2);

    let last = HitQuery {
        target: HitRef::Cell {
            column: ceiling - 1,
            row: 0,
        },
        recorded: None,
    };
    assert!(
        resolve_hit(&last, &constants, &spec).is_ok(),
        "the last column the ceiling allows is drawable"
    );

    let past = HitQuery {
        target: HitRef::Cell {
            column: ceiling,
            row: 0,
        },
        recorded: None,
    };
    let refused = resolve_hit(&past, &constants, &spec);
    assert!(
        matches!(&refused, Err(HealError::Unresolvable { detail, .. }) if detail.contains("ceiling")),
        "a column past the ceiling names a cell no window can draw: {refused:?}"
    );
}

#[test]
fn test_resolve_hit_handles_a_supplied_point_against_its_record() {
    let spec = spec();
    let constants = declarations(&cell_declarations(&spec));
    let point = (120, 64);
    let agreed = HitQuery {
        target: HitRef::Point {
            x: point.0,
            y: point.1,
        },
        recorded: Some(point),
    };

    let hit = resolve_hit(&agreed, &constants, &spec).expect("the point is the one recorded");
    assert_eq!(hit.point, point);
    assert!(!hit.healed, "a supplied point is not derived from anything");
    assert!(hit.constants.is_empty());

    let drifted = HitQuery {
        recorded: Some((point.0 + 1, point.1)),
        ..agreed
    };
    let refused = resolve_hit(&drifted, &constants, &spec);
    assert!(
        matches!(refused, Err(HealError::Unresolvable { .. })),
        "nothing about a supplied point can be re-derived, so a drifted one is refused: \
         {refused:?}"
    );
}

#[test]
fn test_heal_log_records_the_old_name_the_new_name_and_the_row() {
    let spec = spec();
    let renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "candidate-cell-height",
    );
    let constants = declarations(&renamed);
    let healed =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the rename heals");
    let declared = resolve(ConstRef::ByName("cell-min-width"), &constants, &spec)
        .expect("a constant the fixture declares at the value its row states");

    let mut log = HealLog::new();
    assert!(log.note(&healed), "a recovery is recorded");
    assert!(
        !log.note(&declared),
        "a locator that found its name is not a recovery"
    );
    assert_eq!(log.len(), 1);

    let record = &log.records()[0];
    assert_eq!(record.marker, HEAL_MARKER);
    assert_eq!(
        record.cause,
        HealCause::Renamed {
            from: String::from("cell-height"),
            to: String::from("candidate-cell-height"),
        }
    );
    assert_eq!(
        record.evidence,
        vec![AnchorView {
            section: "#### 3.1.1",
            row: "候选单元高度",
            unit: "dp",
            value: format!("{}dp", stated(&spec, "候选单元高度", Unit::Dp, 0)),
        }],
        "the record has to cite the row the derivation is anchored on"
    );

    let line = record.line();
    for part in [
        "[HEALED]",
        "cell-height",
        "candidate-cell-height",
        "候选单元高度",
    ] {
        assert!(line.contains(part), "the line must carry {part:?}: {line}");
    }
}

#[test]
fn test_heal_log_hands_the_archive_a_record_for_every_recovery() {
    let empty = HealLog::new();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    assert!(
        empty.evidence().is_empty(),
        "a run that re-derived nothing leaves the archive no record"
    );
    assert!(
        empty.index_markdown().contains("No locator was re-derived"),
        "an empty run says so rather than leaving the section out: {}",
        empty.index_markdown()
    );

    let spec = spec();
    let renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "candidate-cell-height",
    );
    let constants = declarations(&renamed);
    let stale = (0, 0);
    let query = HitQuery {
        target: HitRef::Cell { column: 1, row: 0 },
        recorded: Some(stale),
    };
    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    let mut log = HealLog::new();
    assert!(log.note_hit(&query, &hit), "both recoveries are recorded");

    let records = log.evidence();
    assert_eq!(
        records.len(),
        2,
        "one record per recovery: the constant whose name was re-derived, and the point that \
         moved: {records:?}"
    );

    let renamed_record = &records[0];
    assert_eq!(renamed_record.old, "cell-height");
    assert_eq!(renamed_record.new, "candidate-cell-height");
    assert!(
        renamed_record.basis.starts_with(SPEC_DOCUMENT),
        "a citation has to name the document a reviewer opens: {}",
        renamed_record.basis
    );
    assert!(
        renamed_record.basis.contains("3.1.1") && renamed_record.basis.contains("候选单元高度"),
        "a citation has to name the row the derivation is anchored on: {}",
        renamed_record.basis
    );

    let moved_record = &records[1];
    let centre = expected_centre(&spec, 1, 0);
    assert_eq!(moved_record.old, format!("{},{}", stale.0, stale.1));
    assert_eq!(moved_record.new, format!("{},{}", centre.0, centre.1));
    assert_eq!(
        moved_record.basis.matches(';').count(),
        5,
        "a reprojection cites every row its arithmetic is anchored on: {}",
        moved_record.basis
    );

    let json = serde_json::to_value(renamed_record).expect("the record is serializable");
    assert_eq!(json["old"], serde_json::Value::from("cell-height"));
    assert_eq!(
        json["new"],
        serde_json::Value::from("candidate-cell-height")
    );
    assert!(
        json["basis"].is_string(),
        "the record has to reach assertions.json in the archive's own shape: {json}"
    );

    let summary = log.index_markdown();
    assert!(summary.contains(HEAL_MARKER), "{summary}");
    assert!(summary.contains("2 locator(s)"), "{summary}");
    assert!(summary.contains("候选单元高度"), "{summary}");
}

#[test]
fn test_heal_log_records_a_reprojected_hit() {
    let spec = spec();
    let constants = declarations(&cell_declarations(&spec));
    let stale = (0, 0);
    let query = HitQuery {
        target: HitRef::Cell { column: 1, row: 0 },
        recorded: Some(stale),
    };
    let hit = resolve_hit(&query, &constants, &spec).expect("the cell resolves");

    let mut log = HealLog::new();
    assert!(log.note_hit(&query, &hit), "the moved point is recorded");

    let record = &log.records()[0];
    assert_eq!(record.marker, HEAL_MARKER);
    assert_eq!(record.locator, "cell 1,0");
    assert_eq!(
        record.cause,
        HealCause::Reprojected {
            from: stale,
            to: expected_centre(&spec, 1, 0),
        }
    );
    assert_eq!(
        record.evidence.len(),
        6,
        "a reprojection cites every row the arithmetic is anchored on"
    );
    assert!(record.line().contains("0,0"), "{}", record.line());
}
