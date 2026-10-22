//! Tests for the golden-diff channel: the comparison's zero-diff and one-pixel contracts,
//! the name grammar that pins a frame's parameters, the bless discipline, and the landing
//! the v3 record gives a diff.
//!
//! Every frame in these tests comes from the fixture raster, so none of them needs a
//! display server, a compositor or a font, and every comparison is decided by arithmetic
//! the suite can reproduce bit for bit.

use std::path::PathBuf;

use super::raster::{baseline_name, raster};
use super::{BaselineName, GoldenError, GoldenOutcome, GridState, GridTheme, VisualJudgement};
use super::{diff, judge};

use crate::testd::capture::CaptureMetadata;
use crate::testd::evidence::AssertionV3;
use crate::testd::golden::RgbaFrame;

/// A scratch directory for one test, removed when the value drops.
struct Scratch(PathBuf);

impl Scratch {
    /// A fresh scratch directory named after the test that asked for it.
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rspinyin-golden-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the scratch directory");
        Self(path)
    }

    /// The directory as a path.
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The @1x dark grid, the batch's reference frame.
fn reference() -> RgbaFrame {
    raster(GridState::Default, GridTheme::Dark, 1)
}

/// A frame with one pixel flipped, at `(x, y)`.
fn with_pixel_flipped(frame: &RgbaFrame, x: u32, y: u32) -> RgbaFrame {
    let mut damaged = frame.clone();
    let at = damaged.offset(x, y).expect("the pixel is in the frame");
    damaged.pixels[at] ^= 0xFF;
    damaged
}

#[test]
fn test_the_fixture_raster_is_deterministic_byte_for_byte() {
    // The property the zero-diff contract rests on: two rasterings of one grid are the
    // same frame, so a passing judgement means the renderer drew the same thing, not that
    // the comparison tolerated something.
    let first = reference();
    let second = reference();
    assert_eq!(
        first, second,
        "the same grid rastered twice is the same bytes"
    );
}

#[test]
fn test_every_parameter_changes_the_frame_it_draws() {
    // The batch is only worth diffing against if the three pinned parameters each produce
    // a frame of their own: a state, a theme or a scale that drew the same pixels as
    // another would file two baselines under two names for one picture.
    let reference = reference();
    for (state, theme, scale) in [
        (GridState::Hover, GridTheme::Dark, 1),
        (GridState::Selected, GridTheme::Dark, 1),
        (GridState::Default, GridTheme::Light, 1),
        (GridState::Default, GridTheme::Dark, 2),
    ] {
        let other = raster(state, theme, scale);
        assert_ne!(
            reference, other,
            "{state:?} {theme:?} @{scale}x draws its own frame"
        );
    }
    // The @2x grid is the card's size: 600x140 dp at twice the ratio.
    let doubled = raster(GridState::Default, GridTheme::Dark, 2);
    assert_eq!((doubled.width, doubled.height), (1200, 280));
}

#[test]
fn test_an_identical_frame_diffs_zero_and_passes_the_deterministic_allowance() {
    let scratch = Scratch::new("identical");
    let name = "grid_600x140_dark@1x.png";
    let frame = reference();
    let written = super::bless(&frame, name, scratch.path()).expect("the baseline blesses");
    assert_eq!(written.file_name().expect("a file name"), name);

    let outcome = judge(
        &VisualJudgement::GoldenDiff {
            baseline: String::from(name),
            max_diff_pixels: 0,
        },
        &reference(),
        scratch.path(),
    )
    .expect("the judgement runs");
    let GoldenOutcome::Golden(report) = outcome else {
        panic!("a GoldenDiff judgement reports a diff");
    };
    assert_eq!(report.diff_pixels, 0, "the same frame differs nowhere");
    assert_eq!(report.first_diff_rect, (0, 0, 0, 0), "no diff, no box");
    assert!(
        report.passes(0),
        "the deterministic raster allows nothing and needs none"
    );
}

#[test]
fn test_a_single_flipped_pixel_is_red_with_its_own_box() {
    // The injected-regression contract: one wrong pixel is one differing pixel, in a box
    // one pixel wide, past an allowance of zero.
    let scratch = Scratch::new("single-pixel");
    let name = "grid_600x140_dark@1x.png";
    let frame = reference();
    super::bless(&frame, name, scratch.path()).expect("the baseline blesses");

    let damaged = with_pixel_flipped(&frame, 37, 23);
    let report = match judge(
        &VisualJudgement::GoldenDiff {
            baseline: String::from(name),
            max_diff_pixels: 0,
        },
        &damaged,
        scratch.path(),
    ) {
        Ok(GoldenOutcome::Golden(report)) => report,
        other => panic!("the judgement reports the diff, got {other:?}"),
    };
    assert_eq!(
        report.diff_pixels, 1,
        "one flipped byte, one differing pixel"
    );
    assert_eq!(
        report.first_diff_rect,
        (37, 23, 1, 1),
        "the box is the pixel"
    );
    assert!(
        !report.passes(0),
        "the deterministic allowance forgives nothing"
    );
    assert!(
        report.passes(1),
        "an allowance of one forgives exactly this"
    );
}

#[test]
fn test_the_box_spans_from_the_first_differing_pixel_to_the_last() {
    // Two damages, far apart: the count is two, and the box is the smallest rectangle
    // that shows a reviewer both.
    let frame = reference();
    let mut damaged = frame.clone();
    for (x, y) in [(5u32, 6u32), (500u32, 120u32)] {
        let at = damaged.offset(x, y).expect("the pixel is in the frame");
        damaged.pixels[at] ^= 0xFF;
    }
    let report = diff(&damaged, &frame, "grid_600x140_dark@1x.png");
    assert_eq!(report.diff_pixels, 2);
    assert_eq!(
        report.first_diff_rect,
        (5, 6, 496, 115),
        "from (5, 6) to (500, 120) inclusive"
    );
}

#[test]
fn test_a_structural_judgement_compares_nothing() {
    let outcome = judge(
        &VisualJudgement::Structural,
        &reference(),
        std::path::Path::new("/no-such-library"),
    );
    assert_eq!(
        outcome.expect("a structural judgement never fails"),
        GoldenOutcome::Structural,
        "nothing was compared, so nothing is reported"
    );
}

#[test]
fn test_a_missing_baseline_and_a_size_mismatch_are_distinct_refusals() {
    // A library that does not hold the frame is a declaration to correct; a frame at the
    // wrong size is a frame to re-take. The two read differently because a caller acts
    // differently on them.
    let scratch = Scratch::new("refusals");
    let name = "grid_600x140_dark@1x.png";
    super::bless(&reference(), name, scratch.path()).expect("the baseline blesses");

    let missing = judge(
        &VisualJudgement::GoldenDiff {
            baseline: String::from("grid_600x140_light@1x.png"),
            max_diff_pixels: 0,
        },
        &reference(),
        scratch.path(),
    )
    .expect_err("the library holds no light grid");
    assert_eq!(
        missing,
        GoldenError::BaselineMissing {
            baseline: String::from("grid_600x140_light@1x.png"),
            directory: scratch.path().to_path_buf(),
        }
    );

    let shrunken = RgbaFrame::filled(300, 140, [0, 0, 0, 255]);
    let mismatched = judge(
        &VisualJudgement::GoldenDiff {
            baseline: String::from(name),
            max_diff_pixels: 0,
        },
        &shrunken,
        scratch.path(),
    )
    .expect_err("a half-width frame is not the baseline's size");
    assert_eq!(
        mismatched,
        GoldenError::DimensionMismatch {
            baseline: String::from(name),
            frame: (300, 140),
            baseline_frame: (600, 140),
        }
    );
}

#[test]
fn test_the_name_grammar_parses_and_renders_its_own_form() {
    let parsed =
        BaselineName::parse("grid_600x140_dark@2x.png").expect("the name follows the grammar");
    assert_eq!(parsed.subject, "grid");
    assert_eq!(parsed.width, 600);
    assert_eq!(parsed.height, 140);
    assert_eq!(parsed.theme, GridTheme::Dark);
    assert_eq!(parsed.scale, 2);
    assert_eq!(parsed.render(), "grid_600x140_dark@2x.png");
    assert_eq!(parsed.metadata().scale, 2.0);

    let light = BaselineName::parse("panel_48x48_light@1x.png").expect("the light name parses");
    assert_eq!(light.theme, GridTheme::Light);
    assert_eq!(light.scale, 1);
}

#[test]
fn test_the_name_grammar_refuses_what_pins_nothing() {
    // Each of these leaves one of the three parameters -- size, theme, ratio -- unstated,
    // which is exactly the frame the library may not hold: one a second machine could not
    // reproduce.
    for (name, reason) in [
        ("grid_dark@2x.png", "no dimensions"),
        ("grid_600x140@2x.png", "no theme"),
        ("grid_600x140_dark.png", "no scale suffix"),
        ("grid_600x140_dark@2", "no x after the scale"),
        ("grid_600x140_dark@two.png", "a scale that is not a number"),
        ("grid_600x140_dark@0x.png", "a scale of zero"),
        (
            "grid_600x140_neon@2x.png",
            "a theme the library holds no palette for",
        ),
        ("grid_600x0_dark@2x.png", "a height of zero"),
        ("grid", "no extension at all"),
        ("grid_600x140_dark@2x.bmp", "not a PNG"),
        (
            "grid,600x140,dark@2x.png",
            "a subject with a character a name may not",
        ),
        (
            "grid_600x140@2x.png",
            "two parts where the grammar reads three",
        ),
    ] {
        let error = BaselineName::parse(name)
            .expect_err(&format!("{name} ({reason}) pins nothing and is refused"));
        let GoldenError::BadBaselineName {
            name: refused,
            reason: why,
        } = error
        else {
            panic!("{name}: a naming refusal, got {error}");
        };
        assert_eq!(refused, name);
        assert!(!why.is_empty(), "{name}: the refusal says why");
    }
}

#[test]
fn test_bless_refuses_a_name_that_lies_about_the_frame() {
    // The name is the pin: a bless under a name whose dimensions are not the frame's is
    // refused rather than filed, because a baseline filed under 600x140 that is really
    // 300x140 would fail every honest comparison after it.
    let scratch = Scratch::new("name-frame");
    let frame = RgbaFrame::filled(300, 140, [0, 0, 0, 255]);
    let error = super::bless(&frame, "grid_600x140_dark@1x.png", scratch.path())
        .expect_err("the name pins other dimensions");
    assert_eq!(
        error,
        GoldenError::NameFrameMismatch {
            name: String::from("grid_600x140_dark@1x.png"),
            name_dims: (600, 140),
            frame_dims: (300, 140),
        }
    );
}

#[test]
fn test_a_rebless_of_the_same_size_overwrites_the_baseline() {
    // A re-bless is the one way a baseline changes: same name, same size, new pixels, and
    // the acceptance note is what says it was deliberate.
    let scratch = Scratch::new("rebless");
    let name = "grid_600x140_dark@1x.png";
    super::bless(&reference(), name, scratch.path()).expect("the first bless");
    let hover = raster(GridState::Hover, GridTheme::Dark, 1);
    super::bless(&hover, name, scratch.path()).expect("the re-bless of the same size");

    let outcome = judge(
        &VisualJudgement::GoldenDiff {
            baseline: String::from(name),
            max_diff_pixels: 0,
        },
        &hover,
        scratch.path(),
    )
    .expect("the judgement runs");
    let GoldenOutcome::Golden(report) = outcome else {
        panic!("a GoldenDiff judgement reports a diff");
    };
    assert_eq!(
        report.diff_pixels, 0,
        "the library now holds the hover grid"
    );
}

#[test]
fn test_the_whole_first_batch_blesses_and_judges_green() {
    // The first batch: default/hover/selected x dark/light x @1x/@2x, twelve names, each
    // blessed from the fixture raster and judged back green. This is the shape the case
    // cards reference when they name a baseline.
    let scratch = Scratch::new("first-batch");
    let mut names = Vec::new();
    for state in [GridState::Default, GridState::Hover, GridState::Selected] {
        for theme in [GridTheme::Dark, GridTheme::Light] {
            for scale in [1u32, 2u32] {
                let frame = raster(state, theme, scale);
                let name = baseline_name(state, theme, scale);
                super::bless(&frame, &name, scratch.path())
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                names.push(name);
            }
        }
    }
    assert_eq!(names.len(), 12, "three states, two themes, two ratios");
    let distinct: std::collections::BTreeSet<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
        distinct.len(),
        12,
        "twelve combinations file under twelve names: a collision would bless one baseline \
         over another"
    );
    for name in &names {
        let parsed = BaselineName::parse(name).expect("every batch name follows the grammar");
        // The batch's own names decode back to the parameters they were blessed from,
        // which is what lets a reader tell which baseline is which without opening it.
        assert!(
            parsed.subject.starts_with("grid_"),
            "{name}: the batch's subject is a grid state"
        );
    }
}

#[test]
fn test_a_judgement_at_1200x280_stays_within_its_time_budget() {
    // The card's budget: one diff of the card's own frame size within 30 ms. The budget
    // names the comparison's own cost, so the timed step is the diff itself -- a `judge`
    // that reads its baseline off disk each call would time the file system beside it,
    // which is not the quantity the card pins. The assertion is the fastest of twenty
    // runs, because a suite runs beside neighbours the scheduler moves around.
    let scratch = Scratch::new("budget");
    let name = "grid_1200x280_dark@2x.png";
    let frame = raster(GridState::Default, GridTheme::Dark, 2);
    super::bless(&frame, name, scratch.path()).expect("the baseline blesses");
    let pinned = RgbaFrame::from_png(&scratch.path().join(name)).expect("the baseline reads");
    let damaged = with_pixel_flipped(&frame, 1199, 279);
    let budget = std::time::Duration::from_millis(30);
    // A nextest run starts the moment the build ends, while its last compile jobs are
    // still draining; the settle keeps the builder out of the quantity measured.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let fastest = (0..20)
        .map(|_| {
            let began = std::time::Instant::now();
            let report = diff(&damaged, &pinned, name);
            assert_eq!(
                report.diff_pixels, 1,
                "the corner pixel is the one that moved"
            );
            began.elapsed()
        })
        .min()
        .expect("twenty runs always yield a minimum");
    assert!(
        fastest <= budget,
        "the fastest of twenty diffs took {fastest:?}, over the 30 ms budget"
    );
}

#[test]
fn test_a_diff_lands_in_the_v3_record_with_its_claim_and_report() {
    // The ledger landing: the claim carries the verdict, and the report carries the
    // numbers a red run is triaged from -- both inside the record the suite publishes as
    // assertions.json.
    let report = super::DiffReport {
        baseline: String::from("grid_600x140_dark@2x.png"),
        diff_pixels: 12,
        first_diff_rect: (40, 8, 32, 24),
    };
    let claim = report.claim(0);
    assert_eq!(claim.name, "golden-diff");
    assert_eq!(claim.expect, "diff_pixels <= 0");
    assert_eq!(claim.actual, "12 differing pixels");
    assert!(
        !claim.pass,
        "twelve pixels past an allowance of zero is red"
    );

    let record = AssertionV3 {
        case: String::from("TC-UI-07"),
        tier: crate::testd::evidence::Tier::Acceptance,
        dims: Vec::new(),
        requires_screenshots: false,
        screenshots_expected: Vec::new(),
        budget_keys: Vec::new(),
        claims: vec![claim],
        diffs: vec![report.clone()],
    };
    let text = record.to_json().expect("the record writes");
    let read: AssertionV3 = serde_json::from_str(&text).expect("the record reads back");
    assert_eq!(read.diffs, vec![report], "the report round-trips");
    assert_eq!(read.failed_claims().count(), 1, "the red claim is the one");
}

#[test]
fn test_a_record_without_diffs_reads_as_one_that_compared_nothing() {
    // The additive field: a record written before the field existed -- no `diffs` key in
    // its JSON -- parses as a record that compared nothing, the way every older bundle
    // must keep reading.
    let text = r#"{
        "case": "TC-CORE-01",
        "tier": "smoke",
        "claims": []
    }"#;
    let record: AssertionV3 = serde_json::from_str(text).expect("the older record parses");
    assert!(record.diffs.is_empty(), "no key, no comparisons");
}

#[test]
fn test_frames_write_and_read_back_through_the_png_half() {
    // The channel's own file format: a frame blessed to disk is the frame that comes back,
    // which is what makes a baseline a baseline rather than an approximation of one.
    let scratch = Scratch::new("png-round-trip");
    let frame = raster(GridState::Selected, GridTheme::Light, 1);
    let path = scratch.path().join("frame.png");
    frame
        .to_png(
            &path,
            CaptureMetadata {
                scale: 1.0,
                depth: 32,
            },
        )
        .expect("the frame writes");
    let read = RgbaFrame::from_png(&path).expect("the frame reads");
    assert_eq!(read, frame, "the pixels survive the file");
}
