//! Tests for [`super`].
//!
//! They cover the channel rather than the window: what the three sources are read as, what
//! happens when one of them cannot be read, and the three comparisons the acceptance criteria
//! name -- a drifted `.slint` constant reported by name with both values, a source that is
//! absent reported as absent rather than as an empty table, and the effective alpha following
//! the backend's own rule.
//!
//! The tests that read the specification read the repository's own `docs/dev/features.md`, and
//! the ones that need a `.slint` source write one into a scratch directory under the system
//! temporary directory. None of them needs a display server, and none leaves anything behind.
//!
//! Where a test builds a window that agrees with the specification, it derives every number from
//! the parsed tables rather than spelling it out: a fixture that restated the document's values
//! would be a second copy of the contract, and the point of the cross-check is that there is
//! only one.
//!
//! The `.slint` reader's own tests live in [`slint`], which keeps this file to the helpers, the
//! specification reader and the cross-check; the tests that read the repository's three sources
//! together live in [`consistency`], and the ones that compare the window's own two sizes in
//! [`geometry`].

mod consistency;
mod geometry;
mod slint;

use std::fs;
use std::path::{Path, PathBuf};

use ime_types::{ColorScheme, LayoutHint, Rgba8, ThemeSpec};

use super::slint::SlintConstants;
use super::spec::SpecTable;
use super::value::{MetricValue, Unit, alpha_byte};
use super::{
    BackendReading, MetricError, MetricMismatch, MetricSource, MetricSourceId, OPAQUE_ALPHA,
    SLINT_DOCUMENT, SPEC_DOCUMENT, UiMetrics, cross_check_spec, effective_base_alpha,
};
use crate::testd::env::X11Facts;

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-metrics-<tag>-<pid>`, empty.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-metrics-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

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

/// The value a row states, which the test knows is there.
fn row_value(spec: &SpecTable, label: &str, unit: Unit) -> MetricValue {
    spec.row(label)
        .unwrap_or_else(|| panic!("3.1.1 has no row `{label}`"))
        .value(unit, 0)
        .unwrap_or_else(|| panic!("3.1.1 `{label}` states no value in {}", unit.suffix()))
}

/// The whole number a value is.
fn number(value: MetricValue) -> u16 {
    value.number().expect("the value is a whole number")
}

/// `grid-gap` written the way the `public constant` form declares it: `GridGap`.
fn pascal(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A `.slint` source declaring every exception 3.1.4 names, with `drift` written instead of the
/// documented value for the constant it names.
///
/// `global` selects the form: `true` writes the `CandidateMetrics` properties
/// `scripts/check-ui-spec.sh` reads, `false` the `public constant` declarations the window's own
/// sketch uses. Both must satisfy the cross-check, which is why the test builds them rather than
/// hard-coding one.
fn slint_source(spec: &SpecTable, global: bool, drift: Option<(&str, u16)>) -> String {
    let mut text = String::new();
    if global {
        text.push_str("export global CandidateMetrics {\n");
    } else {
        text.push_str("export component CandidateWindow inherits Window {\n");
    }
    for (name, value) in spec.exceptions() {
        let value = match drift {
            Some((drifted, replacement)) if drifted == name.as_str() => replacement,
            _ => *value,
        };
        if global {
            text.push_str(&format!(
                "    in-out property <length> {name}: {value}px;\n"
            ));
        } else {
            text.push_str(&format!(
                "    public constant {}: {value}px;\n",
                pascal(name)
            ));
        }
    }
    text.push_str("}\n");
    text
}

/// A window that agrees with `spec` in every value the cross-check looks at.
fn live(spec: &SpecTable) -> UiMetrics {
    let base_alpha = spec
        .colour("surface.base")
        .and_then(|row| row.dark.alpha)
        .expect("3.2 states the acrylic alpha of the surface base");
    let theme = ThemeSpec {
        scheme: ColorScheme::Dark,
        accent: Rgba8 {
            r: 0x4C,
            g: 0x9A,
            b: 0xFF,
            a: 255,
        },
        acrylic: true,
        base_alpha: alpha_byte(base_alpha),
        corner_radius_dp: number(row_value(spec, "容器圆角", Unit::Dp)),
        scale: 1.0,
    };
    let layout = LayoutHint {
        max_per_row: u8::try_from(number(row_value(spec, "单行最大候选数", Unit::Count)))
            .expect("a per-row maximum fits in a byte"),
        show_annotation: true,
        max_width_dp: number(row_value(spec, "候选框最大宽度", Unit::Dp)),
    };
    UiMetrics::new(reading(&theme, true, true), layout, theme)
}

/// A backend reading for `theme`, with the two capability flags set as given.
///
/// The window is the same 320x140 logical pixels in every fixture, at a scale of 1.0, so the
/// physical size is that same pair; the tests that move the scale live in [`geometry`] and build
/// their own.
fn reading(theme: &ThemeSpec, argb_visual: bool, compositor_present: bool) -> BackendReading {
    BackendReading {
        backend_id: "x11",
        window_dp: (320, 140),
        window_px: (320, 140),
        scale: 1.0,
        argb_visual,
        compositor_present,
        base_alpha: theme.base_alpha,
    }
}

/// The one mismatch a cross-check that found exactly one produced.
fn only(mut mismatches: Vec<MetricMismatch>) -> MetricMismatch {
    assert_eq!(
        mismatches.len(),
        1,
        "expected exactly one mismatch, got {mismatches:#?}"
    );
    match mismatches.pop() {
        Some(mismatch) => mismatch,
        None => panic!("the assertion above leaves exactly one mismatch"),
    }
}

// ---------------------------------------------------------------------------
// Reading the specification
// ---------------------------------------------------------------------------

#[test]
fn test_spec_table_parses_the_geometry_rows_of_3_1_1() {
    let spec = spec();

    assert!(
        spec.geometry().len() >= 20,
        "3.1.1 lists the whole geometry, found {} rows",
        spec.geometry().len()
    );
    for row in spec.geometry() {
        assert!(!row.label.is_empty(), "every row carries its element label");
    }
    // Two spot checks: the row the theme's corner radius is compared against, and the row the
    // cell height is stated in. A change to the document is meant to fail here, because the
    // harness then has to be re-read against the new table.
    assert_eq!(
        spec.row("容器圆角").map(|row| row.dp.clone()),
        Some(vec![12])
    );
    assert_eq!(
        spec.row("候选单元高度").map(|row| row.dp.clone()),
        Some(vec![36])
    );
    // The rows that state more than one value keep all of them.
    assert_eq!(spec.row("候选单元内边距").map(|row| row.dp.len()), Some(2));
    assert_eq!(spec.row("候选框最大宽度").map(|row| row.dp.len()), Some(2));
    assert_eq!(
        spec.row("Header 字号/字重").map(|row| row.sp.len()),
        Some(3)
    );
    assert_eq!(spec.row("单行最大候选数").map(|row| row.dp.len()), Some(0));
}

#[test]
fn test_spec_table_reads_the_range_a_row_states_for_a_configurable_value() {
    let spec = spec();

    let row = spec
        .row("容器圆角")
        .expect("3.1.1 states the corner radius");
    assert_eq!(row.configurable, Some((8, 20)));
    assert_eq!(row.contains(MetricValue::LengthDp(8)), Some(true));
    assert_eq!(row.contains(MetricValue::LengthDp(20)), Some(true));
    assert_eq!(row.contains(MetricValue::LengthDp(21)), Some(false));

    let fixed = spec
        .row("候选单元高度")
        .expect("3.1.1 states the cell height");
    assert_eq!(fixed.configurable, None);
    assert_eq!(
        fixed.contains(MetricValue::LengthDp(36)),
        None,
        "a row with no range answers nothing rather than `false`"
    );
}

#[test]
fn test_spec_table_reads_the_exception_table_of_3_1_4() {
    let spec = spec();

    assert!(
        spec.exceptions().len() >= 10,
        "3.1.4 names the constants that sit off the 4dp grid, found {}",
        spec.exceptions().len()
    );
    // Spot checks on two of the rows, taken from the document rather than from a list here.
    assert_eq!(spec.exception("grid-gap"), Some(6));
    assert_eq!(spec.exception("header-height"), Some(34));
    assert!(
        spec.exceptions().values().all(|value| *value > 0),
        "an exception of zero would be a row that states nothing"
    );
}

#[test]
fn test_spec_table_reads_the_colour_rows_of_3_2() {
    let spec = spec();

    let base = spec.colour("surface.base").expect("3.2 lists surface.base");
    assert_eq!(base.dark.alpha, Some(0.85));
    assert_eq!(
        base.dark.colour,
        Some(Rgba8 {
            r: 0x1C,
            g: 0x1C,
            b: 0x1E,
            a: 255
        })
    );
    assert_eq!(base.light.alpha, Some(0.85));
    assert_eq!(
        spec.colour("text.primary").and_then(|row| row.dark.colour),
        Some(Rgba8 {
            r: 0xF2,
            g: 0xF2,
            b: 0xF7,
            a: 255
        })
    );
    // An `rgba()` cell truncates its alpha, the way the Slint builtin does.
    assert_eq!(
        spec.colour("state.hover").and_then(|row| row.dark.colour),
        Some(Rgba8 {
            r: 242,
            g: 242,
            b: 247,
            a: 20
        })
    );
    // A token derived from another states no colour of its own.
    let dot = spec
        .colour("status.dot.active")
        .expect("3.2 lists status.dot.active");
    assert_eq!(dot.dark.colour, None);
    assert_eq!(dot.dark.alpha, None);
    assert!(
        spec.colours().len() >= 15,
        "3.2 lists the whole palette, found {} tokens",
        spec.colours().len()
    );
    for token in [
        "surface.base",
        "text.primary",
        "accent.default",
        "status.dot.idle",
    ] {
        assert!(spec.colour(token).is_some(), "3.2 still lists {token}");
    }
}

#[test]
fn test_spec_table_refuses_a_table_whose_columns_changed() {
    let document = spec_text().replace("| 元素 | 规格 |", "| 元素 | 规格 | 说明 |");

    let error = SpecTable::parse(&document).expect_err("a renamed column is a structure change");
    assert!(
        matches!(error, MetricError::SpecUnparsable { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("columns"), "{error}");
}

#[test]
fn test_spec_table_refuses_a_section_that_gained_a_second_table() {
    let document = spec_text().replace(
        "#### 3.1.2 材质层次（由内到外）",
        "| a | b |\n|---|---|\n| 1 | 2 |\n\n#### 3.1.2 材质层次（由内到外）",
    );

    let error = SpecTable::parse(&document).expect_err("two tables is a structure change");
    assert!(error.to_string().contains("exactly one"), "{error}");
}

#[test]
fn test_spec_table_refuses_a_document_without_the_geometry_section() {
    let document = spec_text().replace("#### 3.1.1 候选框整体几何", "#### 3.1.0 geometry");

    let error = SpecTable::parse(&document).expect_err("the anchor heading is gone");
    assert!(error.to_string().contains("3.1.1"), "{error}");
    assert!(error.to_string().contains(SPEC_DOCUMENT), "{error}");
}

// ---------------------------------------------------------------------------
// The cross-check
// ---------------------------------------------------------------------------

#[test]
fn test_cross_check_passes_a_window_that_agrees_with_every_source() {
    let spec = spec();
    let live = live(&spec);
    for global in [true, false] {
        let constants = SlintConstants::parse("fixture.slint", &slint_source(&spec, global, None))
            .expect("the fixture parses");
        let mismatches = cross_check_spec(&spec, &live, &MetricSource::Available(constants));
        assert!(
            mismatches.is_empty(),
            "the {global} declaration form must satisfy the check: {mismatches:#?}"
        );
    }
}

#[test]
fn test_cross_check_reports_a_drifted_slint_constant() {
    let spec = spec();
    let live = live(&spec);
    let documented = spec
        .exception("grid-gap")
        .expect("3.1.4 lists grid-gap as an exception");
    let drifted = documented + 2;
    let constants = SlintConstants::parse(
        "fixture.slint",
        &slint_source(&spec, true, Some(("grid-gap", drifted))),
    )
    .expect("the fixture parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Value {
            name: String::from("grid-gap"),
            spec: MetricValue::LengthDp(documented),
            code: MetricValue::LengthDp(drifted),
            source: MetricSourceId::Slint,
        }
    );
    assert!(
        mismatch.describe().contains("grid-gap"),
        "the report names the item: {}",
        mismatch.describe()
    );
}

#[test]
fn test_cross_check_reports_a_constant_the_specification_names_and_the_source_does_not() {
    let spec = spec();
    let live = live(&spec);
    let mut source = slint_source(&spec, true, None);
    let line = source
        .lines()
        .find(|line| line.contains("grid-gap"))
        .map(str::to_owned)
        .expect("the fixture declares grid-gap");
    source = source.replace(&format!("{line}\n"), "");

    let constants = SlintConstants::parse("fixture.slint", &source).expect("the fixture parses");
    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Missing {
            name: String::from("grid-gap"),
            source: MetricSourceId::Slint,
        }
    );
}

#[test]
fn test_cross_check_reports_a_missing_slint_source_as_unavailable() {
    let spec = spec();
    let live = live(&spec);
    let source = MetricSource::Unavailable {
        reason: String::from("the file does not exist"),
    };

    let mismatches = cross_check_spec(&spec, &live, &source);

    assert_eq!(mismatches.len(), 1, "{mismatches:#?}");
    assert_eq!(
        mismatches[0],
        MetricMismatch::Unavailable {
            source: MetricSourceId::Slint,
            reason: String::from("the file does not exist"),
        },
        "an unreadable source is reported, never read as agreement"
    );
}

#[test]
fn test_cross_check_reports_a_runtime_value_outside_the_documented_range() {
    let spec = spec();
    let mut live = live(&spec);
    let (min, max) = spec
        .row("容器圆角")
        .and_then(|row| row.configurable)
        .expect("3.1.1 states a range for the corner radius");
    live.theme.corner_radius_dp = max + 4;
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::OutOfRange {
            name: String::from("容器圆角"),
            value: MetricValue::LengthDp(max + 4),
            min,
            max,
            source: MetricSourceId::Runtime,
        }
    );
}

#[test]
fn test_cross_check_accepts_a_runtime_value_inside_the_documented_range() {
    let spec = spec();
    let mut live = live(&spec);
    // The configuration may move a value anywhere inside the range the document states; that is
    // a setting, not drift, and a reader that compared it for equality would report it.
    live.theme.corner_radius_dp = spec
        .row("容器圆角")
        .and_then(|row| row.configurable)
        .map(|(min, _)| min)
        .expect("3.1.1 states a range");
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatches = cross_check_spec(&spec, &live, &MetricSource::Available(constants));

    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn test_cross_check_reports_a_runtime_value_that_drifted_from_the_specification() {
    let spec = spec();
    let mut live = live(&spec);
    let documented = number(row_value(&spec, "候选框最大宽度", Unit::Dp));
    live.layout.max_width_dp = documented - 80;
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Value {
            name: String::from("候选框最大宽度"),
            spec: MetricValue::LengthDp(documented),
            code: MetricValue::LengthDp(documented - 80),
            source: MetricSourceId::Runtime,
        }
    );
}

#[test]
fn test_cross_check_reports_a_theme_alpha_that_drifted_from_the_specification() {
    let spec = spec();
    let mut live = live(&spec);
    let expected = alpha_byte(
        spec.colour("surface.base")
            .and_then(|row| row.dark.alpha)
            .expect("3.2 states the alpha"),
    );
    live.theme.base_alpha = expected - 17;
    // The backend paints what it was asked for, so the only disagreement is with the document.
    live.base_alpha = live.theme.base_alpha;
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Value {
            name: String::from("surface.base @ alpha"),
            spec: MetricValue::Alpha8(expected),
            code: MetricValue::Alpha8(expected - 17),
            source: MetricSourceId::Runtime,
        }
    );
}

#[test]
fn test_cross_check_reports_a_degradation_that_did_not_take_effect() {
    let spec = spec();
    let mut live = live(&spec);
    // No compositor and no ARGB visual: the base must be opaque, and a window that still paints
    // the requested alpha is the one degradation the design says must never happen.
    live.compositor_present = false;
    live.argb_visual = false;
    live.base_alpha = live.theme.base_alpha;
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Value {
            name: String::from("the effective base alpha"),
            spec: MetricValue::Alpha8(OPAQUE_ALPHA),
            code: MetricValue::Alpha8(live.theme.base_alpha),
            source: MetricSourceId::Runtime,
        }
    );
}

#[test]
fn test_cross_check_accepts_the_documented_degradation() {
    let spec = spec();
    let mut live = live(&spec);
    live.compositor_present = false;
    live.base_alpha = OPAQUE_ALPHA;
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");

    let mismatches = cross_check_spec(&spec, &live, &MetricSource::Available(constants));

    assert!(
        mismatches.is_empty(),
        "an opaque base without a compositor is the documented fallback: {mismatches:#?}"
    );
}

#[test]
fn test_cross_check_reports_every_mismatch_rather_than_the_first() {
    let spec = spec();
    let mut live = live(&spec);
    live.layout.max_width_dp = 1;
    live.theme.corner_radius_dp = 1;
    let constants = SlintConstants::parse(
        "fixture.slint",
        &slint_source(&spec, true, Some(("grid-gap", 99))),
    )
    .expect("parses");

    let mismatches = cross_check_spec(&spec, &live, &MetricSource::Available(constants));

    assert_eq!(mismatches.len(), 3, "{mismatches:#?}");
}

// ---------------------------------------------------------------------------
// The alpha rule
// ---------------------------------------------------------------------------

#[test]
fn test_effective_base_alpha_forces_an_opaque_base_without_a_compositor() {
    let requested = 217;
    assert_eq!(
        effective_base_alpha(requested, false, false),
        OPAQUE_ALPHA,
        "no ARGB visual and no compositor"
    );
    assert_eq!(
        effective_base_alpha(requested, true, false),
        OPAQUE_ALPHA,
        "an alpha channel nothing will blend shows as black"
    );
    assert_eq!(
        effective_base_alpha(requested, false, true),
        OPAQUE_ALPHA,
        "a compositor with no alpha channel has nothing to blend"
    );
    assert_eq!(
        effective_base_alpha(requested, true, true),
        requested,
        "both halves present: the request survives"
    );
    assert_eq!(OPAQUE_ALPHA, 255);
}

#[test]
fn test_backend_reading_takes_its_capability_flags_from_the_probe() {
    let facts = X11Facts {
        argb_visual: true,
        compositor_present: false,
    };

    let reading = BackendReading::from_facts(&facts, (320, 140, 2.0), (640, 280), 217);

    assert_eq!(reading.backend_id, "x11");
    assert_eq!(reading.window_dp, (320, 140));
    assert_eq!(
        reading.window_px,
        (640, 280),
        "the physical size is carried as the surface reported it, not derived from the scale"
    );
    assert_eq!(reading.scale, 2.0);
    assert!(reading.argb_visual);
    assert!(!reading.compositor_present);
    let theme = ThemeSpec {
        scheme: ColorScheme::Dark,
        accent: Rgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        acrylic: true,
        base_alpha: 217,
        corner_radius_dp: 12,
        scale: 2.0,
    };
    let layout = LayoutHint {
        max_per_row: 5,
        show_annotation: true,
        max_width_dp: 720,
    };
    let live = UiMetrics::new(reading, layout, theme);
    assert_eq!(
        live.expected_base_alpha(),
        OPAQUE_ALPHA,
        "the probe says there is no compositor, so the base is opaque"
    );
}

#[test]
fn test_alpha_byte_rounds_the_way_the_renderer_does() {
    assert_eq!(alpha_byte(0.85), 217);
    assert_eq!(alpha_byte(0.0), 0);
    assert_eq!(alpha_byte(1.0), 255);
    assert_eq!(alpha_byte(-1.0), 0, "a fraction below the range clamps");
    assert_eq!(alpha_byte(2.0), 255, "a fraction above the range clamps");
    assert_eq!(alpha_byte(f32::NAN), 0, "a fraction that is not a number");
    assert_eq!(alpha_byte(f32::INFINITY), 0);
}

#[test]
fn test_metric_value_describes_each_shape() {
    assert_eq!(MetricValue::LengthDp(36).describe(), "36dp");
    assert_eq!(MetricValue::FontSp(14).describe(), "14sp");
    assert_eq!(MetricValue::Count(5).describe(), "5");
    assert_eq!(MetricValue::Alpha8(217).describe(), "alpha 217");
    assert_eq!(
        MetricValue::Colour(Rgba8 {
            r: 0x1C,
            g: 0x1C,
            b: 0x1E,
            a: 255
        })
        .describe(),
        "#1C1C1EFF"
    );
    assert_eq!(
        MetricValue::Unreadable(String::from("1.0")).describe(),
        "1.0"
    );
    assert_eq!(
        MetricValue::Colour(Rgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0
        })
        .number(),
        None
    );
}

#[test]
fn test_mismatch_describe_names_both_values_and_the_source() {
    let mismatch = MetricMismatch::Value {
        name: String::from("grid-gap"),
        spec: MetricValue::LengthDp(6),
        code: MetricValue::LengthDp(8),
        source: MetricSourceId::Slint,
    };
    let text = mismatch.describe();
    assert!(text.contains("grid-gap"), "{text}");
    assert!(text.contains("6dp") && text.contains("8dp"), "{text}");
    assert!(text.contains(SLINT_DOCUMENT), "{text}");

    let missing = MetricMismatch::Missing {
        name: String::from("grid-gap"),
        source: MetricSourceId::Slint,
    };
    let missing_text = missing.describe();
    assert!(missing_text.contains("missing"), "{missing_text}");
    assert!(missing_text.contains("grid-gap"), "{missing_text}");
    assert!(
        missing_text.contains(SLINT_DOCUMENT),
        "a constant the source does not declare has to name the file that should: {missing_text}"
    );
}

#[test]
fn test_metric_error_names_the_file_it_could_not_read() {
    let path = Path::new("/nowhere/candidate.slint");
    let error = MetricError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
    };
    assert!(
        error.to_string().contains("/nowhere/candidate.slint"),
        "{error}"
    );
}
