//! Tests for the sweep's parsing, enumeration and metric extraction.
//!
//! Everything here runs on literal spec text and literal reports: no grid point
//! is ever compiled, nothing reads `/proc`, and the holdout set is never
//! opened. The parts of the sweep that touch the machine -- the compile child,
//! the container measurement, the purity read -- are exactly the parts these
//! tests keep out of a test binary.

use super::GridValue;
use super::SweepSpec;
use super::metrics_of;
use crate::dictc::quality::{LengthBandAccuracy, QualityReport, Top1Band};

/// Parses a spec from literal TOML, panicking with the parse error on failure.
fn spec(text: &str) -> SweepSpec {
    SweepSpec::parse(text).expect("the fixture spec parses")
}

#[test]
fn test_parse_grid_reads_the_declared_points() {
    let parsed = spec("[grid]\ncore_min_freq = [20, 40]\ntail_flat_weight = [100]\n");
    assert_eq!(parsed.grid.len(), 2);
    assert_eq!(
        parsed.grid["core_min_freq"],
        vec![GridValue::Integer(20), GridValue::Integer(40)]
    );
}

#[test]
fn test_parse_grid_accepts_whole_numbers_for_a_float_knob() {
    let parsed = spec("[grid]\nmin_score = [1, 2]\n");
    assert_eq!(
        parsed.grid["min_score"],
        vec![GridValue::Float(1.0), GridValue::Float(2.0)]
    );
}

#[test]
fn test_parse_grid_empty_section_refuses() {
    let error = SweepSpec::parse("[grid]\n").expect_err("an empty grid refuses");
    assert!(
        error.to_string().contains("names no parameter"),
        "the refusal names the empty grid: {error}"
    );
}

#[test]
fn test_parse_grid_not_a_table_refuses() {
    let error = SweepSpec::parse("core_min_freq = 20\n")
        .expect_err("a spec without [grid] refuses");
    assert!(
        error.to_string().contains("[grid]"),
        "the refusal names the missing section: {error}"
    );
}

#[test]
fn test_parse_grid_single_point_refuses() {
    let error = SweepSpec::parse("[grid]\ncore_min_freq = [20]\n")
        .expect_err("a one-point grid refuses");
    assert!(
        error.to_string().contains("nothing to compare"),
        "the refusal says why one point is not a sweep: {error}"
    );
}

#[test]
fn test_parse_grid_unknown_parameter_refuses_with_known_names() {
    let error = SweepSpec::parse("[grid]\nuni_weight = [128, 256]\n")
        .expect_err("an unknown parameter refuses");
    let text = error.to_string();
    assert!(text.contains("uni_weight"), "the refusal names the parameter: {text}");
    assert!(
        text.contains("core_min_freq"),
        "the refusal lists the known knobs: {text}"
    );
}

#[test]
fn test_parse_grid_blind_parameter_refuses_with_the_reason() {
    let error = SweepSpec::parse("[grid]\ncontext_tie_band_q16 = [128, 512]\n")
        .expect_err("an observation-blind parameter refuses");
    let text = error.to_string();
    assert!(
        text.contains("cannot observe"),
        "the refusal states the blindness: {text}"
    );
    assert!(
        text.contains("no channel"),
        "the refusal separates no-channel from no-effect: {text}"
    );
    let error = SweepSpec::parse("[grid]\ndecay_half_life_ticks = [100, 200]\n")
        .expect_err("the decay knob refuses too");
    assert!(
        error.to_string().contains("cannot observe"),
        "the refusal states the blindness: {error}"
    );
}

#[test]
fn test_parse_grid_wrong_value_kind_refuses() {
    let error = SweepSpec::parse("[grid]\ncore_min_freq = [1.5]\n")
        .expect_err("a float point on an integer knob refuses");
    assert!(
        error.to_string().contains("whole-number"),
        "the refusal names the kind: {error}"
    );
}

#[test]
fn test_parse_grid_repeated_point_refuses() {
    let error = SweepSpec::parse("[grid]\ncore_min_freq = [20, 20]\n")
        .expect_err("a repeated point refuses");
    assert!(
        error.to_string().contains("twice"),
        "the refusal names the repetition: {error}"
    );
}

#[test]
fn test_assignments_enumerate_the_whole_product() {
    let parsed = spec(
        "[grid]\ncore_min_freq = [10, 20]\ntail_flat_weight = [100, 200, 300]\n",
    );
    let assignments = parsed.assignments();
    assert_eq!(assignments.len(), 6, "2 x 3 grid is six points");
    assert!(
        assignments.iter().all(|point| point.values.len() == 2),
        "every assignment names both parameters"
    );
    let mut labels: Vec<String> = assignments
        .iter()
        .map(|point| point.label.clone())
        .collect();
    labels.sort();
    labels.dedup();
    assert_eq!(labels.len(), 6, "every point is distinct");
}

#[test]
fn test_assignments_order_is_deterministic_and_first_parameter_moves_slowest() {
    let text = "[grid]\ncore_min_freq = [10, 20]\ntail_flat_weight = [100, 200]\n";
    let first = spec(text).assignments();
    let second = spec(text).assignments();
    assert_eq!(first, second, "the enumeration is a function of the spec");
    assert_ne!(
        first[0].values["tail_flat_weight"], first[1].values["tail_flat_weight"],
        "the last declared parameter moves first"
    );
    assert_eq!(
        first[0].values["core_min_freq"], first[1].values["core_min_freq"],
        "the first declared parameter moves slowest"
    );
    assert_eq!(
        first[0].values["core_min_freq"], first[1].values["core_min_freq"],
        "the first declared parameter holds while the second walks its values"
    );
    assert_ne!(
        first[0].values["core_min_freq"], first[2].values["core_min_freq"],
        "the first parameter changes only after the second has walked its values"
    );
}

#[test]
fn test_grid_value_renders_flag_compatible_strings() {
    assert_eq!(GridValue::Integer(20).to_string(), "20");
    assert_eq!(GridValue::Float(1.5).to_string(), "1.5");
    assert_eq!(GridValue::Float(1.0).to_string(), "1");
}

#[test]
fn test_metrics_of_carries_the_objective_numbers() {
    let mut bands = LengthBandAccuracy::default();
    bands.single = Top1Band {
        cases: 50,
        first: 45,
    };
    bands.two = Top1Band {
        cases: 100,
        first: 88,
    };
    let report = QualityReport {
        top1_rate: 0.75,
        top9_rate: 0.96,
        unreachable: 3,
        served_top1_rate: 0.8,
        length_bands: bands,
    };
    let metrics = metrics_of(&report);
    assert_eq!(metrics.top1_overall, 0.75);
    assert_eq!(metrics.top1_single, Some(0.9));
    assert_eq!(metrics.top1_two, Some(0.88));
    assert_eq!(metrics.top1_long, None, "an unmeasured band stays unmeasured");
    assert_eq!(metrics.top9_overall, 0.96);
    assert_eq!(metrics.unreachable, 3);
}
