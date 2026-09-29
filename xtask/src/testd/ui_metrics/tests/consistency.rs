//! Tests for the three sources read together: the specification's tables, the `.slint` metrics
//! block, and the window the case is looking at.
//!
//! The repository's own files are read here rather than a fixture, which is what makes these the
//! tests that would fail on a real drift rather than on a planted one. The metrics block had not
//! landed when the channel was written, so the case that reads the repository asserted the third
//! source was absent; it has since landed, and the case now asserts the three agree -- which is
//! the same assertion with the opposite expectation, and the one worth keeping.
//!
//! The file also holds the tests for what the channel reads *from*: a tree carrying the two
//! documents and nothing else, which is how the claim that it needs no budget document and states
//! no threshold of its own is checked rather than asserted.

use std::fs;

use ime_types::ColorScheme;

use super::super::slint::SlintConstants;
use super::super::{
    MetricMismatch, MetricSource, MetricSourceId, MetricSources, MetricValue, SLINT_DOCUMENT,
    SPEC_DOCUMENT, SpecTable, alpha_byte, cross_check_spec,
};
use super::{Scratch, live, only, repo_root, slint_source, spec, spec_text};

#[test]
fn test_metric_sources_read_the_repository() {
    let sources = MetricSources::read(&repo_root()).expect("the repository reads");

    assert!(sources.spec.geometry().len() >= 20);
    // The metrics block landed with the candidate window, so this reads a real third source
    // rather than reporting one missing. What the channel is for is the comparison, and a
    // mismatch here is a finding rather than a fixture: the specification, the running layout and
    // the declared constants have to agree.
    assert!(
        sources.slint.is_available(),
        "the candidate window declares its metrics: {:?}",
        sources.slint.reason()
    );

    let live = live(&sources.spec);
    let mismatches = cross_check_spec(&sources.spec, &live, &sources.slint);
    assert!(
        mismatches.is_empty(),
        "the repository's three sources agree: {mismatches:#?}"
    );
}

#[test]
fn test_metric_sources_read_a_tree_holding_only_the_two_documents() {
    let scratch = Scratch::new("metrics-documents");
    let copy = |relative: &str| {
        let destination = scratch.path.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("creating the document's directory");
        }
        fs::copy(repo_root().join(relative), &destination).expect("copying the document");
    };
    copy(SPEC_DOCUMENT);
    copy(SLINT_DOCUMENT);

    // The two documents and nothing else: no budget file, no lock file, no build output. A
    // channel that read a threshold of its own from anywhere would have to read a third file, and
    // there is no third file to read -- which is what keeps a budget number from being written
    // down twice, once in the document that owns it and once here.
    let sources = MetricSources::read(&scratch.path).expect("the two documents are all it reads");

    assert!(
        sources.slint.is_available(),
        "the copied metrics block reads: {:?}",
        sources.slint.reason()
    );
    let repository = spec();
    assert_eq!(
        sources.spec.geometry(),
        repository.geometry(),
        "the copied specification parses to the same rows"
    );
    assert_eq!(
        sources.spec.exceptions(),
        repository.exceptions(),
        "and to the same exceptions"
    );
}

#[test]
fn test_cross_check_reads_the_base_alpha_of_the_scheme_the_window_is_in() {
    // The two columns are edited apart, so a check that always read the dark one would compare a
    // light window against an alpha nothing asked it to have.
    let document = spec_text().replace(
        "| `surface.base` | `#1C1C1E @ 0.85` | `#FFFFFF @ 0.85` | 候选框底 |",
        "| `surface.base` | `#1C1C1E @ 0.85` | `#FFFFFF @ 0.70` | 候选框底 |",
    );
    let spec = SpecTable::parse(&document).expect("the edited table parses");
    let constants =
        SlintConstants::parse("fixture.slint", &slint_source(&spec, true, None)).expect("parses");
    let source = MetricSource::Available(constants);
    let dark_alpha = alpha_byte(0.85);
    let light_alpha = alpha_byte(0.70);
    assert_ne!(
        dark_alpha, light_alpha,
        "the two columns have to differ for this case to say anything"
    );

    let dark = live(&spec);
    assert_eq!(dark.theme.base_alpha, dark_alpha);
    assert!(
        cross_check_spec(&spec, &dark, &source).is_empty(),
        "a dark window is checked against the dark column"
    );

    let mut light = live(&spec);
    light.theme.scheme = ColorScheme::Light;
    let mismatch = only(cross_check_spec(&spec, &light, &source));
    assert_eq!(
        mismatch,
        MetricMismatch::Value {
            name: String::from("surface.base @ alpha"),
            spec: MetricValue::Alpha8(light_alpha),
            code: MetricValue::Alpha8(dark_alpha),
            source: MetricSourceId::Runtime,
        },
        "a light window painted with the dark column's alpha is reported"
    );

    light.theme.base_alpha = light_alpha;
    light.base_alpha = light_alpha;
    assert!(
        cross_check_spec(&spec, &light, &source).is_empty(),
        "and the light column's own alpha is accepted"
    );
}

#[test]
fn test_cross_check_reports_a_constant_the_source_states_as_an_expression() {
    let spec = spec();
    let live = live(&spec);
    let documented = spec
        .exception("grid-gap")
        .expect("3.1.4 lists grid-gap as an exception");
    // A constant rewritten as an expression is no longer a number, and the reader keeps the text
    // rather than dropping the declaration: a declaration that was dropped is one a comparison can
    // never report, so an expression would silently stop being checked.
    let source = slint_source(&spec, true, None).replace("grid-gap: 6px", "grid-gap: 2 * 3px");
    let constants = SlintConstants::parse("fixture.slint", &source).expect("the fixture parses");

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
            code: MetricValue::Unreadable(String::from("2 * 3px")),
            source: MetricSourceId::Slint,
        }
    );
}
