//! The golden-diff channel: how a captured frame is judged against a committed baseline.
//!
//! # What this channel is for
//!
//! A `Structural` judgement keeps the screenshot channel's own behaviour: the case asserts
//! measurements and the pixels are archived, never compared. A `GoldenDiff` judgement goes
//! further -- the frame is compared against a baseline under `tests/fixtures/golden/`, and
//! because the raster this project draws with is a deterministic software rasterizer, any
//! pixel that differs is a real regression. There is no fuzzy threshold and no perceptual
//! hash: a frame that is not the baseline, minus at most the pixels the case allows, fails.
//!
//! # What may enter the baseline library
//!
//! Only frames that are reproducible by construction: the fixture raster this module draws
//! in [`raster`], and captures whose every input -- size, theme, scale -- the caller pinned.
//! The three parameters are locked into the baseline's own file name
//! (`grid_600x140_dark@2x.png`), so a frame taken at another size or ratio cannot silently
//! masquerade as the baseline it resembles. Frames that depend on the environment never
//! enter: acrylic blur is the compositor's, glyph shapes are fontconfig's, and a baseline
//! that only one machine can reproduce is a baseline that fails everywhere else. A case
//! whose frame is environment-dependent declares `Structural` instead and says why in its
//! own case table -- the skip is a declaration, not a comparison that quietly passed.
//!
//! # How a baseline is updated
//!
//! Baselines change through an explicit `--bless`, run by a person who records the update
//! in the acceptance notes. Nothing in the gate or the CI may bless: a red diff is fixed by
//! correcting the renderer or by a human decision that the new frame is the design, never
//! by regenerating the baseline in the same breath that failed it. Blessing also refuses a
//! frame whose dimensions differ from the ones its own file name pins: a size change is a
//! new baseline under a new name, and the old name keeps its old meaning.
//!
//! # What lands in the evidence pack
//!
//! The comparison itself is a [`DiffReport`]: how many pixels differ and the rectangle from
//! the first differing pixel to the last, which is the box a reviewer looks at first. The
//! verdict rides in the v3 record twice -- as the claim `golden-diff`, and as the report
//! itself under the record's `diffs`, so a red run can be triaged from `assertions.json`
//! alone.

// The channel is reachable through `xtask golden`, which names the bless and judge paths.
// What stays under the allowance is the case-author API the CLI does not name -- the claim
// mapping and the `Structural` variant, which the suite channel names as it lands -- and a
// `pub use` in a binary crate is reported as unused whenever nothing in the crate names it,
// the same situation the capture module documents.
#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};

use crate::testd::capture::{CaptureError, CaptureMetadata, read_rgba, write_png};
use crate::testd::evidence::Claim;

pub mod raster;

#[cfg(test)]
mod tests;

pub use self::raster::{GridState, GridTheme};

/// The directory of the repository's committed baseline library, from the workspace root.
pub const GOLDEN_DIR: &str = "tests/fixtures/golden";

/// The name the golden-diff claim carries in the v3 record.
pub const GOLDEN_CLAIM: &str = "golden-diff";

/// How a captured frame is judged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisualJudgement {
    /// The case asserts structure -- measurements, counts, geometry -- and the pixels are
    /// archived but never compared. This is the behaviour every case had before this
    /// channel existed.
    Structural,
    /// The frame is compared against a committed baseline. The software rasterizer is
    /// deterministic, so any pixel delta is a real regression: no fuzzy thresholds, no
    /// perceptual hashing. Scale-dependent baselines live under a scale-suffixed name.
    GoldenDiff {
        /// Baseline file under `tests/fixtures/golden/`, e.g. `grid_600x140_dark@2x.png`.
        baseline: String,
        /// Maximum allowed differing pixels; 0 for the deterministic raster.
        max_diff_pixels: u32,
    },
}

/// The diff record that lands in the case's evidence pack.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffReport {
    /// The baseline the frame was compared against.
    pub baseline: String,
    /// How many pixels differ.
    pub diff_pixels: u32,
    /// Bounding box of the first differing region, for one-look triage.
    ///
    /// The rectangle from the first differing pixel, in row-major order, to the last one:
    /// small when the regression is local, and `(0, 0, 0, 0)` when nothing differs.
    pub first_diff_rect: (u32, u32, u32, u32),
}

impl DiffReport {
    /// Returns `true` when the diff stays within `max_diff_pixels`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn passes(&self, max_diff_pixels: u32) -> bool {
        self.diff_pixels <= max_diff_pixels
    }

    /// The claim the v3 record carries for this comparison.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn claim(&self, max_diff_pixels: u32) -> Claim {
        Claim {
            name: String::from(GOLDEN_CLAIM),
            expect: format!("diff_pixels <= {max_diff_pixels}"),
            actual: format!("{} differing pixels", self.diff_pixels),
            pass: self.passes(max_diff_pixels),
        }
    }
}

/// One RGBA frame, as the channels produce and the comparison consumes.
///
/// The bytes are 8-bit RGBA, `width * height * 4` long, row-major, top-left first -- the
/// layout `capture`'s PNG half writes and reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaFrame {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// The pixels, `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl RgbaFrame {
    /// A frame of the given size, every pixel set to `rgba`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        Self {
            width,
            height,
            pixels: std::iter::repeat_n(rgba, width as usize * height as usize)
                .flatten()
                .collect(),
        }
    }

    /// Reads a PNG as a frame.
    ///
    /// # Errors
    ///
    /// As [`read_rgba`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_png(path: &Path) -> Result<Self, CaptureError> {
        let (width, height, pixels) = read_rgba(path)?;
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Writes the frame as a PNG with the capture record `meta`.
    ///
    /// # Errors
    ///
    /// As [`write_png`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_png(&self, path: &Path, meta: CaptureMetadata) -> Result<(), CaptureError> {
        write_png(path, self.width, self.height, &self.pixels, meta)
    }

    /// The byte offset of one pixel, or `None` when it is outside the frame.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn offset(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(((y as usize) * (self.width as usize) + x as usize) * 4)
    }
}

/// What one judgement came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoldenOutcome {
    /// The judgement was `Structural`: nothing was compared, and the frame stands only as
    /// archived evidence.
    Structural,
    /// The frame was compared against its baseline, and this is what differed.
    Golden(DiffReport),
}

/// Everything a golden-diff judgement or a bless can be refused with.
///
/// Each variant is a verdict a caller acts on differently: a `BaselineMissing` is a library
/// that does not hold the frame yet, a `DimensionMismatch` is a frame taken at another size
/// than its baseline, and a `NameFrameMismatch` is a bless whose file name lies about the
/// frame it would pin.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum GoldenError {
    /// The baseline the judgement names is not in the library.
    #[error(
        "the baseline {baseline:?} is not in {}: the library holds no frame under that name, \
         which is a declaration to correct or a frame to bless",
        directory.display()
    )]
    BaselineMissing {
        /// The name the judgement named.
        baseline: String,
        /// The directory that was looked in.
        directory: PathBuf,
    },

    /// The frame and its baseline are different sizes, so pixel order means nothing.
    #[error(
        "the frame is {frame:?} but baseline {baseline:?} is {baseline_frame:?}: a comparison \
         would diff the padding, so the frame is re-taken at the baseline's size instead"
    )]
    DimensionMismatch {
        /// The baseline that was compared against.
        baseline: String,
        /// The frame's size, `(width, height)`.
        frame: (u32, u32),
        /// The baseline's size, `(width, height)`.
        baseline_frame: (u32, u32),
    },

    /// A baseline name that does not follow the grammar the library fixes.
    #[error("`{name}` is not a baseline name: {reason}")]
    BadBaselineName {
        /// The name that was refused.
        name: String,
        /// Why it was refused.
        reason: &'static str,
    },

    /// A bless whose file name pins different dimensions than the frame carries.
    #[error(
        "{name} pins {name_dims:?} but the frame is {frame_dims:?}: the name is the pin, so \
         the frame is blessed under a name that states its own size"
    )]
    NameFrameMismatch {
        /// The name that was refused.
        name: String,
        /// The size the name pins, `(width, height)`.
        name_dims: (u32, u32),
        /// The frame's size, `(width, height)`.
        frame_dims: (u32, u32),
    },

    /// A baseline or a frame could not be read or written as an image this channel compares.
    #[error("a golden frame could not be used: {0}")]
    Image(String),
}

/// Judges `frame` the way `judgement` asks.
///
/// A `Structural` judgement compares nothing. A `GoldenDiff` judgement loads the baseline
/// it names from `golden_dir`, refuses a size mismatch before comparing a single pixel, and
/// reports the diff.
///
/// # Errors
///
/// Returns [`GoldenError::BaselineMissing`] when the library holds no such baseline,
/// [`GoldenError::DimensionMismatch`] when the two frames are different sizes, and
/// [`GoldenError::Image`] when either file cannot be read as an 8-bit RGBA PNG.
///
/// # Panics
///
/// Never.
pub fn judge(
    judgement: &VisualJudgement,
    frame: &RgbaFrame,
    golden_dir: &Path,
) -> Result<GoldenOutcome, GoldenError> {
    let (baseline, _max) = match judgement {
        VisualJudgement::Structural => return Ok(GoldenOutcome::Structural),
        VisualJudgement::GoldenDiff {
            baseline,
            max_diff_pixels,
        } => (baseline, *max_diff_pixels),
    };
    let path = golden_dir.join(baseline.as_str());
    if !path.is_file() {
        return Err(GoldenError::BaselineMissing {
            baseline: baseline.clone(),
            directory: golden_dir.to_path_buf(),
        });
    }
    let pinned =
        RgbaFrame::from_png(&path).map_err(|error| GoldenError::Image(error.to_string()))?;
    if (pinned.width, pinned.height) != (frame.width, frame.height) {
        return Err(GoldenError::DimensionMismatch {
            baseline: baseline.clone(),
            frame: (frame.width, frame.height),
            baseline_frame: (pinned.width, pinned.height),
        });
    }
    Ok(GoldenOutcome::Golden(diff(frame, &pinned, baseline)))
}

/// Counts the pixels `frame` and `pinned` disagree on, and the box they span.
///
/// One row-major pass; the first differing pixel is the box's top-left corner and the last
/// one its bottom-right, which is the smallest rectangle that still shows a reviewer every
/// pixel that moved.
///
/// # Panics
///
/// Never. The two frames were size-checked before this runs, so the chunk counts agree.
fn diff(frame: &RgbaFrame, pinned: &RgbaFrame, baseline: &str) -> DiffReport {
    let width = frame.width as usize;
    let mut diff_pixels: u32 = 0;
    let mut first: Option<(u32, u32)> = None;
    let mut last: Option<(u32, u32)> = None;
    for (index, (a, b)) in frame
        .pixels
        .chunks_exact(4)
        .zip(pinned.pixels.chunks_exact(4))
        .enumerate()
    {
        if a != b {
            diff_pixels += 1;
            let at = (
                u32::try_from(index % width).unwrap_or(0),
                u32::try_from(index / width).unwrap_or(0),
            );
            first.get_or_insert(at);
            last = Some(at);
        }
    }
    let rect = match (first, last) {
        (Some((x0, y0)), Some((x1, y1))) => (x0, y0, x1 - x0 + 1, y1 - y0 + 1),
        _ => (0, 0, 0, 0),
    };
    DiffReport {
        baseline: String::from(baseline),
        diff_pixels,
        first_diff_rect: rect,
    }
}

/// A baseline file name, as the grammar the library pins frames with.
///
/// `<subject>_<W>x<H>_<theme>[@<S>x].png` -- `grid_600x140_dark@2x.png` is the shape. The
/// subject is the frame's scene, the dimensions are the frame's physical pixels, the theme
/// is the palette it was drawn with, and the scale suffix is the device pixel ratio. A name
/// that leaves any of the three parameters out does not pin a reproducible frame, and the
/// library refuses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineName {
    /// The scene the frame shows, e.g. `grid`.
    pub subject: String,
    /// The frame's width in physical pixels.
    pub width: u32,
    /// The frame's height in physical pixels.
    pub height: u32,
    /// The palette the frame was drawn with.
    pub theme: GridTheme,
    /// The device pixel ratio the frame was rasterised at.
    pub scale: u32,
}

impl BaselineName {
    /// Reads a baseline name.
    ///
    /// # Errors
    ///
    /// Returns [`GoldenError::BadBaselineName`] with the reason when the name does not
    /// follow the grammar: a missing `.png`, a missing or malformed scale suffix, a
    /// subject with a character a file name may not carry, dimensions that do not parse or
    /// are zero, or a theme that is neither `dark` nor `light`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(name: &str) -> Result<Self, GoldenError> {
        let refused = |reason| GoldenError::BadBaselineName {
            name: String::from(name),
            reason,
        };
        let stem = name.strip_suffix(".png").ok_or_else(|| {
            refused("it does not end in .png, and every baseline in the library is one")
        })?;
        let (body, scale) = match stem.rsplit_once('@') {
            Some((body, scale)) => {
                let digits = scale
                    .strip_suffix('x')
                    .ok_or_else(|| refused("the scale suffix does not end in 'x', as in @2x"))?;
                let scale: u32 = digits
                    .parse()
                    .map_err(|_| refused("the scale suffix does not carry a number, as in @2x"))?;
                if scale == 0 {
                    return Err(refused("a scale of 0 pins no raster"));
                }
                (body, scale)
            }
            None => {
                return Err(refused(
                    "it pins no scale suffix, as in @2x: the ratio is one of the three \
                     parameters the name exists to lock",
                ));
            }
        };
        // The theme and the dimensions are read from the right, so a subject may carry
        // underscores of its own -- `grid_selected_600x140_dark` reads as the subject
        // `grid_selected`, not as four segments the grammar refuses.
        let (rest, theme) = body
            .rsplit_once('_')
            .ok_or_else(|| refused("it names no theme, as in grid_600x140_dark"))?;
        let (subject, dims) = rest
            .rsplit_once('_')
            .ok_or_else(|| refused("it does not read <subject>_<W>x<H>, as in grid_600x140"))?;
        if subject.is_empty()
            || !subject
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err(refused(
                "the subject is empty or carries a character a file name may not",
            ));
        }
        let (w, h) = dims
            .split_once('x')
            .ok_or_else(|| refused("the dimensions do not read <W>x<H>, as in 600x140"))?;
        let width: u32 = w
            .parse()
            .map_err(|_| refused("the width does not carry a number, as in 600x140"))?;
        let height: u32 = h
            .parse()
            .map_err(|_| refused("the height does not carry a number, as in 600x140"))?;
        if width == 0 || height == 0 {
            return Err(refused("a frame of zero width or height pins no pixels"));
        }
        let theme = GridTheme::from_name(theme)
            .ok_or_else(|| refused("the theme is neither dark nor light"))?;
        Ok(Self {
            subject: String::from(subject),
            width,
            height,
            theme,
            scale,
        })
    }

    /// The name as the library files it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn render(&self) -> String {
        format!(
            "{}_{}x{}_{}@{}x.png",
            self.subject,
            self.width,
            self.height,
            self.theme.name(),
            self.scale
        )
    }

    /// The capture record a frame blessed under this name carries.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn metadata(&self) -> CaptureMetadata {
        CaptureMetadata {
            scale: self.scale as f32,
            depth: 32,
        }
    }
}

/// Blesses `frame` as the baseline `name`, writing it into `golden_dir`.
///
/// This is the one way a baseline changes: a person runs it, names the frame, and records
/// the update in the acceptance notes. The name is checked against the grammar first, and
/// then against the frame itself -- a name that pins other dimensions than the frame has is
/// refused, because the name is the pin and a lying name files a frame under a size it does
/// not have. An existing baseline of the same name and the same size is overwritten: that
/// is what a re-bless is, and the acceptance note is what says it was deliberate.
///
/// # Errors
///
/// Returns [`GoldenError::BadBaselineName`] when the name does not follow the grammar,
/// [`GoldenError::NameFrameMismatch`] when the name pins other dimensions than the frame
/// carries, and [`GoldenError::Image`] when the frame cannot be written.
///
/// # Panics
///
/// Never.
pub fn bless(frame: &RgbaFrame, name: &str, golden_dir: &Path) -> Result<PathBuf, GoldenError> {
    let parsed = BaselineName::parse(name)?;
    if (parsed.width, parsed.height) != (frame.width, frame.height) {
        return Err(GoldenError::NameFrameMismatch {
            name: String::from(name),
            name_dims: (parsed.width, parsed.height),
            frame_dims: (frame.width, frame.height),
        });
    }
    let path = golden_dir.join(name);
    frame
        .to_png(&path, parsed.metadata())
        .map_err(|error| GoldenError::Image(error.to_string()))?;
    Ok(path)
}
