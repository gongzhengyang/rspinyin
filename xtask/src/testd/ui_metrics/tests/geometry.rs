//! Tests for the window's own numbers: the logical-to-physical conversion, and the check that the
//! surface's size follows from it at the device pixel ratio the window reports.
//!
//! The conversion is a rule rather than a table, and it is the one rule this channel states
//! without reading a document: `xtask` links the frozen contract and not the renderer, so the
//! arithmetic of `ime_ui::platform::physical_dimension` is repeated in the parent module and has
//! to stay in step with it. What these tests pin is that arithmetic -- at every device pixel ratio
//! the platform baseline registers -- and the verdict a surface sized for another one produces.
//!
//! Nothing here reads a document of its own: the fixtures come from the parent module, which
//! derives them from the parsed tables, so the only verdict a case in this file can produce is the
//! geometric one.

use super::super::slint::SlintConstants;
use super::super::{
    MAX_SURFACE_DIMENSION, MetricMismatch, MetricSource, SpecTable, UiMetrics, cross_check_spec,
    physical_dimension,
};
use super::{live, only, slint_source, spec};

/// The logical size the cases below draw a window at.
const WINDOW_DP: (u32, u32) = (320, 140);

/// The device pixel ratios the platform baseline registers, including the 3x tier the contract's
/// own `Anchor::scale` names beside them.
const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 3.0];

/// A window that agrees with `spec` everywhere, drawn at `dp` logical pixels and `scale`, with a
/// surface of `px` physical pixels.
///
/// `px` is a parameter rather than something derived here, so that a surface sized for another
/// scale can be built; the cases that want the agreeing one pass [`physical_dimension`]'s own
/// answer.
fn window_at(spec: &SpecTable, dp: (u32, u32), scale: f32, px: (u32, u32)) -> UiMetrics {
    let mut live = live(spec);
    live.window_dp = dp;
    live.window_px = px;
    live.scale = scale;
    live
}

/// The physical size `dp` has at `scale`, dimension by dimension.
fn physical_size(dp: (u32, u32), scale: f32) -> (u32, u32) {
    (
        physical_dimension(dp.0, scale),
        physical_dimension(dp.1, scale),
    )
}

#[test]
fn test_physical_dimension_scales_a_logical_size_at_every_registered_ratio() {
    assert_eq!(physical_dimension(320, 1.0), 320);
    assert_eq!(physical_dimension(320, 1.25), 400);
    assert_eq!(physical_dimension(320, 1.5), 480);
    assert_eq!(physical_dimension(320, 2.0), 640);
    assert_eq!(physical_dimension(320, 3.0), 960);
    // A dimension that does not divide evenly rounds rather than truncating, which is what keeps a
    // 1.25x surface from coming out a pixel short of the logical size it was asked for.
    assert_eq!(physical_dimension(141, 1.25), 176);
    assert_eq!(physical_dimension(141, 1.5), 212);
}

#[test]
fn test_physical_dimension_keeps_a_degenerate_size_usable() {
    // A size a surface cannot have is clamped rather than propagated: this number is what the
    // backend allocates its buffers from, so a zero or an overflow would be a hole rather than a
    // window.
    assert_eq!(physical_dimension(0, 1.0), 1);
    assert_eq!(physical_dimension(0, 3.0), 1);
    assert_eq!(physical_dimension(u32::MAX, 1.0), MAX_SURFACE_DIMENSION);
    // A scale that cannot be used is not this function's to correct -- the backend replaces one
    // before it gets here -- but it must not answer a size of zero either.
    assert_eq!(physical_dimension(320, 0.0), 1);
    assert_eq!(physical_dimension(320, f32::NAN), 1);
    assert_eq!(physical_dimension(320, f32::INFINITY), 1);
}

#[test]
fn test_cross_check_accepts_a_surface_that_matches_its_scale_at_every_ratio() {
    let spec = spec();
    let constants = SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None))
        .expect("the fixture parses");
    let source = MetricSource::Available(constants);

    for scale in SCALES {
        let live = window_at(&spec, WINDOW_DP, scale, physical_size(WINDOW_DP, scale));
        let mismatches = cross_check_spec(&spec, &live, &source);
        assert!(mismatches.is_empty(), "scale {scale}: {mismatches:#?}");
    }
}

#[test]
fn test_cross_check_reports_a_surface_sized_for_another_scale() {
    let spec = spec();
    // The 2x scale with a surface of the 1x height: one dimension right, one wrong, which is the
    // shape a scale that landed after the buffer was allocated leaves behind.
    let live = window_at(&spec, WINDOW_DP, 2.0, (640, 141));
    let constants = SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None))
        .expect("the fixture parses");

    let mismatch = only(cross_check_spec(
        &spec,
        &live,
        &MetricSource::Available(constants),
    ));

    assert_eq!(
        mismatch,
        MetricMismatch::Geometry {
            axis: "height",
            dp: 140,
            scale: 2.0,
            expected: 280,
            found: 141,
        },
        "the verdict names the dimension, the scale and both numbers"
    );
    let text = mismatch.describe();
    assert!(text.contains("280px"), "{text}");
    assert!(text.contains("141px"), "{text}");
    assert!(text.contains("140dp"), "{text}");
}

#[test]
fn test_cross_check_reports_both_axes_when_the_whole_surface_is_the_wrong_size() {
    let spec = spec();
    let live = window_at(&spec, WINDOW_DP, 3.0, WINDOW_DP);
    let constants = SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None))
        .expect("the fixture parses");

    let mismatches = cross_check_spec(&spec, &live, &MetricSource::Available(constants));

    assert_eq!(
        mismatches,
        vec![
            MetricMismatch::Geometry {
                axis: "width",
                dp: 320,
                scale: 3.0,
                expected: 960,
                found: 320,
            },
            MetricMismatch::Geometry {
                axis: "height",
                dp: 140,
                scale: 3.0,
                expected: 420,
                found: 140,
            },
        ],
        "one verdict per dimension, in the order the axes are read"
    );
}
