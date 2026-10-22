//! `xtask golden` -- the command-line surface of the golden-diff channel.
//!
//! # Responsibility
//!
//! The three acts the channel's discipline names, and nothing else: blessing a baseline
//! into the library (either the fixture grid this repository can draw anywhere, or a frame
//! a capture wrote), and judging one frame against one baseline. Blessing is a human act --
//! the acceptance note is written by whoever runs it -- and judging from the command line
//! is how a red diff is triaged without a whole suite run.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::budget::repo_root;
use crate::testd::golden::{
    self, BaselineName, GOLDEN_DIR, GoldenOutcome, GridState, GridTheme, RgbaFrame, VisualJudgement,
};

/// Command-line surface of `xtask golden`.
#[derive(Debug, Args)]
pub struct GoldenArgs {
    /// The baseline library directory; defaults to the repository's own
    /// `tests/fixtures/golden/`.
    #[arg(long, value_name = "DIR")]
    pub golden_dir: Option<PathBuf>,

    /// Bless the fixture candidate grid, in this interaction state (`default`, `hover` or
    /// `selected`), as a baseline. The grid's size is the fixture's own; pair with
    /// `--theme` and `--scale`.
    #[arg(
        long,
        value_name = "STATE",
        conflicts_with_all = ["bless_from", "judge", "as_baseline", "against"]
    )]
    pub bless_raster: Option<String>,

    /// The palette the blessed grid is drawn with: `dark` or `light`.
    #[arg(long, value_name = "THEME", requires = "bless_raster")]
    pub theme: Option<String>,

    /// The device pixel ratio the blessed grid is rasterised at: `1` or `2`.
    #[arg(long, value_name = "N", requires = "bless_raster")]
    pub scale: Option<u32>,

    /// Bless a frame an earlier capture or raster wrote, reading it from this PNG.
    #[arg(
        long,
        value_name = "PNG",
        conflicts_with_all = ["bless_raster", "judge"]
    )]
    pub bless_from: Option<PathBuf>,

    /// The baseline name the blessed frame is filed under; with `--bless-from`.
    #[arg(long, value_name = "NAME", requires = "bless_from")]
    pub as_baseline: Option<String>,

    /// Judge a frame from this PNG against a baseline, print the diff, and fail the run
    /// when it exceeds the allowance.
    #[arg(
        long,
        value_name = "PNG",
        conflicts_with_all = ["bless_raster", "bless_from"]
    )]
    pub judge: Option<PathBuf>,

    /// The baseline the frame is judged against; with `--judge`.
    #[arg(long, value_name = "NAME", requires = "judge")]
    pub against: Option<String>,

    /// How many differing pixels the judgement tolerates; 0 is the deterministic raster's
    /// own allowance.
    #[arg(long, value_name = "N", requires = "judge", default_value_t = 0)]
    pub max_diff_pixels: u32,
}

/// Entry point for `xtask golden`.
///
/// # Errors
///
/// Returns an error when no mode is named, when a mode's arguments are missing or
/// unusable, when the frame or the baseline cannot be read or written, and -- for a
/// judgement -- when the diff exceeds the allowance.
///
/// # Panics
///
/// Never.
pub fn run(args: GoldenArgs) -> Result<()> {
    let golden_dir = match &args.golden_dir {
        Some(directory) => directory.clone(),
        None => repo_root()
            .context("resolving the baseline library's default directory")?
            .join(GOLDEN_DIR),
    };
    if let Some(state) = &args.bless_raster {
        return bless_raster(state, &args, &golden_dir);
    }
    if let (Some(png), Some(name)) = (&args.bless_from, &args.as_baseline) {
        return bless_from(png, name, &golden_dir);
    }
    if let (Some(png), Some(name)) = (&args.judge, &args.against) {
        return judge_frame(png, name, args.max_diff_pixels, &golden_dir);
    }
    bail!(
        "nothing to do: name a --bless-raster STATE with --theme, a --bless-from PNG with \
         --as NAME, or a --judge PNG with --against NAME"
    )
}

/// Blesses the fixture grid the arguments name.
///
/// # Errors
///
/// As [`golden::bless`], plus an error when the state, the theme or the scale is not one
/// the fixture draws.
///
/// # Panics
///
/// Never.
fn bless_raster(state: &str, args: &GoldenArgs, golden_dir: &std::path::Path) -> Result<()> {
    let state = GridState::from_name(state).with_context(|| {
        format!("the grid state is one of default, hover, selected, not {state:?}")
    })?;
    let theme_name = args
        .theme
        .as_deref()
        .with_context(|| format!("{state:?}: the grid is blessed with a --theme"))?;
    let theme = GridTheme::from_name(theme_name)
        .with_context(|| format!("the theme is dark or light, not {theme_name:?}"))?;
    let scale = args.scale.unwrap_or(1);
    if scale == 0 {
        bail!("a scale of 0 rasterises nothing");
    }
    let frame = golden::raster::raster(state, theme, scale);
    let name = golden::raster::baseline_name(state, theme, scale);
    let written = golden::bless(&frame, &name, golden_dir)?;
    println!(
        "blessed {name}: {}x{} at {scale}x, {} theme, written to {} -- record the update in \
         the acceptance notes",
        frame.width,
        frame.height,
        theme.name(),
        written.display()
    );
    Ok(())
}

/// Blesses a frame from an existing PNG.
///
/// # Errors
///
/// As [`golden::bless`], plus an error when the PNG cannot be read as a frame.
///
/// # Panics
///
/// Never.
fn bless_from(png: &std::path::Path, name: &str, golden_dir: &std::path::Path) -> Result<()> {
    let frame =
        RgbaFrame::from_png(png).with_context(|| format!("reading the frame {}", png.display()))?;
    let written = golden::bless(&frame, name, golden_dir)?;
    println!(
        "blessed {name}: {}x{}, written to {} -- record the update in the acceptance notes",
        frame.width,
        frame.height,
        written.display()
    );
    Ok(())
}

/// Judges one frame against one baseline and prints the diff.
///
/// # Errors
///
/// As [`golden::judge`], plus an error when the diff exceeds the allowance, which is what
/// makes a red comparison red on the command line too.
///
/// # Panics
///
/// Never.
fn judge_frame(
    png: &std::path::Path,
    name: &str,
    max_diff_pixels: u32,
    golden_dir: &std::path::Path,
) -> Result<()> {
    // The name is checked before the frame is read, so a typo in the baseline's name is a
    // naming error rather than a missing-file error.
    BaselineName::parse(name)?;
    let frame =
        RgbaFrame::from_png(png).with_context(|| format!("reading the frame {}", png.display()))?;
    let judgement = VisualJudgement::GoldenDiff {
        baseline: String::from(name),
        max_diff_pixels,
    };
    let outcome = golden::judge(&judgement, &frame, golden_dir)?;
    match outcome {
        GoldenOutcome::Structural => {
            bail!("a judgement from the command line is always a comparison")
        }
        GoldenOutcome::Golden(report) => {
            let (x, y, w, h) = report.first_diff_rect;
            println!(
                "{name}: {} differing pixel(s), first at {x},{y} in a {w}x{h} box, within \
                 {max_diff_pixels} -- {}",
                report.diff_pixels,
                if report.passes(max_diff_pixels) {
                    "PASS"
                } else {
                    "FAIL"
                }
            );
            if !report.passes(max_diff_pixels) {
                bail!(
                    "{name}: {} differing pixel(s) is past the {max_diff_pixels} the \
                     judgement allows",
                    report.diff_pixels
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the command-line surface: the three modes' argument wiring and the
    //! outputs they print.

    use super::*;

    /// The arguments of a run against a scratch library.
    fn args() -> GoldenArgs {
        GoldenArgs {
            golden_dir: None,
            bless_raster: None,
            theme: None,
            scale: None,
            bless_from: None,
            as_baseline: None,
            judge: None,
            against: None,
            max_diff_pixels: 0,
        }
    }

    #[test]
    fn test_run_without_a_mode_names_the_three_acts() {
        let outcome = run(args());
        let error = outcome
            .expect_err("a run without a mode is refused")
            .to_string();
        assert!(
            error.contains("--bless-raster")
                && error.contains("--bless-from")
                && error.contains("--judge"),
            "the refusal names every mode: {error}"
        );
    }

    #[test]
    fn test_bless_raster_writes_the_pinned_name_into_the_library() {
        let scratch = std::env::temp_dir().join(format!("golden-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.bless_raster = Some(String::from("selected"));
        arguments.theme = Some(String::from("light"));
        arguments.scale = Some(2);
        assert!(run(arguments).is_ok(), "the grid blesses");
        let name = golden::raster::baseline_name(GridState::Selected, GridTheme::Light, 2);
        let frame = RgbaFrame::from_png(&scratch.join(&name)).expect("the blessed frame reads");
        assert_eq!(
            (frame.width, frame.height),
            (1200, 280),
            "the @2x grid is twice the fixture's dp in each direction"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn test_bless_raster_refuses_an_unknown_state_and_a_missing_theme() {
        let scratch = std::env::temp_dir().join(format!("golden-cli-bad-{}", std::process::id()));
        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.bless_raster = Some(String::from("pressed"));
        assert!(
            run(arguments).is_err(),
            "a state the fixture does not draw is refused"
        );
        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.bless_raster = Some(String::from("hover"));
        assert!(
            run(arguments).is_err(),
            "a bless without a theme is refused"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn test_bless_and_judge_round_trip_through_the_cli() {
        let scratch = std::env::temp_dir().join(format!("golden-cli-trip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let frame_path = scratch.join("frame.png");
        std::fs::create_dir_all(&scratch).expect("the scratch directory");
        let frame = golden::raster::raster(GridState::Hover, GridTheme::Dark, 1);
        frame
            .to_png(
                &frame_path,
                crate::testd::capture::CaptureMetadata {
                    scale: 1.0,
                    depth: 32,
                },
            )
            .expect("the frame is written");

        let name = golden::raster::baseline_name(GridState::Hover, GridTheme::Dark, 1);
        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.bless_from = Some(frame_path.clone());
        arguments.as_baseline = Some(name.clone());
        assert!(run(arguments).is_ok(), "the capture blesses");

        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.judge = Some(frame_path);
        arguments.against = Some(name);
        assert!(
            run(arguments).is_ok(),
            "the same frame judges green against its own baseline"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn test_judge_reports_a_red_diff_as_an_error() {
        let scratch = std::env::temp_dir().join(format!("golden-cli-red-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("the scratch directory");
        let name = golden::raster::baseline_name(GridState::Default, GridTheme::Light, 1);
        let frame = golden::raster::raster(GridState::Default, GridTheme::Light, 1);
        golden::bless(&frame, &name, &scratch).expect("the baseline blesses");

        let mut damaged = frame.clone();
        let at = damaged.offset(5, 5).expect("the pixel is in the frame");
        damaged.pixels[at] ^= 0xFF;
        let frame_path = scratch.join("damaged.png");
        damaged
            .to_png(
                &frame_path,
                crate::testd::capture::CaptureMetadata {
                    scale: 1.0,
                    depth: 32,
                },
            )
            .expect("the frame is written");

        let mut arguments = args();
        arguments.golden_dir = Some(scratch.clone());
        arguments.judge = Some(frame_path);
        arguments.against = Some(name);
        assert!(
            run(arguments).is_err(),
            "a single flipped pixel is past the 0 the deterministic raster allows"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
