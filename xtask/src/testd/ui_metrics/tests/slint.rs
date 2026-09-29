//! Tests for the `.slint` metrics reader.
//!
//! They cover the two declaration forms the two authoritative documents use, the refusals, and
//! the three ways a source can be missing: a file that is not there, a file that declares nothing,
//! and a declaration that cannot be read. The block itself is present, so the last of the three is
//! the one a real tree can still hit.

use std::fs;

use super::super::MetricError;
use super::super::slint::SlintConstants;
use super::super::value::MetricValue;
use super::{Scratch, slint_source, spec};

#[test]
fn test_slint_constants_reads_the_global_property_form() {
    let source = "\
export global CandidateMetrics {
    in-out property <length> shadow-margin: 32px;
    in-out property <length> cell-height: 36px;
    in-out property <int> max-per-row: 5;
    out property <length> window-width: container-width + 2 * ShadowMargin;
    in-out property <string> mode-label: \"中\";
}
";
    let constants = SlintConstants::parse("fixture.slint", source).expect("the fixture parses");

    assert_eq!(
        constants.value("cell-height"),
        Some(&MetricValue::LengthDp(36))
    );
    assert_eq!(
        constants.value("shadow-margin"),
        Some(&MetricValue::LengthDp(32))
    );
    assert_eq!(constants.value("max-per-row"), Some(&MetricValue::Count(5)));
    assert_eq!(
        constants.value("window-width"),
        Some(&MetricValue::Unreadable(String::from(
            "container-width + 2 * ShadowMargin"
        ))),
        "a declaration that is not a number is kept rather than dropped"
    );
    assert_eq!(
        constants.value("mode-label"),
        Some(&MetricValue::Unreadable(String::from("\"中\"")))
    );
    assert_eq!(constants.len(), 5);
}

#[test]
fn test_slint_constants_reads_the_public_constant_form_under_both_names() {
    let source = "\
export component CandidateWindow inherits Window {
    public constant ShadowMargin: 32px;
    public constant GridGap: 6px;
    in-out property <length> container-width: 220px;
}
";
    let constants = SlintConstants::parse("fixture.slint", source).expect("the fixture parses");

    assert_eq!(
        constants.value("shadow-margin"),
        Some(&MetricValue::LengthDp(32)),
        "the specification names the constant in kebab-case"
    );
    assert_eq!(
        constants.value("ShadowMargin"),
        Some(&MetricValue::LengthDp(32)),
        "the declaration's own spelling is readable too"
    );
    assert_eq!(constants.value("grid-gap"), Some(&MetricValue::LengthDp(6)));
    assert_eq!(
        constants.value("container-width"),
        None,
        "a property outside a global belongs to the component, not to the metrics"
    );
}

#[test]
fn test_slint_constants_refuse_a_declaration_it_cannot_read() {
    let source = "\
export global CandidateMetrics {
    in-out property <length> cell-height
}
";
    let error = SlintConstants::parse("fixture.slint", source)
        .expect_err("a property declaration without a value cannot be read");

    assert!(
        matches!(error, MetricError::SlintUnparsable { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("fixture.slint:2"), "{error}");
}

#[test]
fn test_slint_constants_load_reports_a_missing_file_as_unavailable() {
    let scratch = Scratch::new("slint-missing");
    let path = scratch.path.join("candidate.slint");

    let source = SlintConstants::load(&path).expect("a missing file is not an I/O failure");

    assert!(!source.is_available());
    assert!(
        source
            .reason()
            .is_some_and(|reason| reason.contains("does not exist")),
        "the reason names what is missing: {source:?}"
    );
}

#[test]
fn test_slint_constants_load_reports_a_file_without_metrics_as_unavailable() {
    let scratch = Scratch::new("slint-empty");
    let path = scratch.path.join("candidate.slint");
    fs::write(
        &path,
        "export component CandidateWindow inherits Window {\n}\n",
    )
    .expect("writing the fixture");

    let source = SlintConstants::load(&path).expect("an empty block is not a parse failure");

    assert!(!source.is_available());
    assert!(
        source
            .reason()
            .is_some_and(|reason| reason.contains("declares no metrics")),
        "the reason says what the file is missing: {source:?}"
    );
}

#[test]
fn test_slint_constants_load_reads_a_file_that_declares_metrics() {
    let scratch = Scratch::new("slint-present");
    let path = scratch.path.join("candidate.slint");
    fs::write(&path, slint_source(&spec(), true, None)).expect("writing the fixture");

    let source = SlintConstants::load(&path).expect("the fixture reads");

    assert!(source.is_available());
    assert!(source.available().is_some_and(|c| !c.is_empty()));
}
