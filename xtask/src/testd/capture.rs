//! The screenshot channel: what the candidate window actually put on the screen.
//!
//! # What this channel is for
//!
//! A case that asserts on pixels -- a `1dp` hairline, the 4dp grid, a contrast ratio, the
//! colour of a hovered cell -- needs the pixels. Everything else in this harness reads
//! structure: the `UiFrame` snapshot says what the engine *asked* the window to draw, and
//! the geometry module says where the cells are. Neither can tell a stroke that was drawn
//! from one that was clipped, blurred or never rasterised, so the pixels are read back from
//! the server and archived as a PNG under the run's own directory.
//!
//! The two channels never stand in for each other. A snapshot is not a screenshot, and a
//! screenshot is not evidence about the frame that produced it: it is evidence about what
//! the X server holds for a drawable at one moment.
//!
//! # Physical pixels, never downsampled
//!
//! The image is the drawable's own resolution, one file byte per screen byte, because the
//! things being audited are at the edge of what a resample survives: a `1dp` stroke is two
//! physical pixels at `scale = 2.0`, and a capture that had been scaled to logical pixels
//! would report a blur where the design specifies an edge. The ratio is recorded inside the
//! file rather than assumed (see `encode`), so an audit can convert between the two without
//! guessing, and the DPI mapping itself lives with the rest of the coordinate arithmetic in
//! [`crate::testd::coords`].
//!
//! # What it reads, and what that costs
//!
//! A whole frame is read as row ranges of at most [`CHUNK_ROWS`] rows and assembled in
//! memory before it is encoded, so one capture holds the frame plus one reply at a time. A
//! frame past [`MAX_FRAME_BYTES`] is refused with a message that names `Region`:
//! narrowing the capture is the answer, not raising the ceiling.
//!
//! # What this channel does not do
//!
//! It does not composite: `get_image` returns the drawable's own pixels, so a capture of a
//! window is what that window holds and not what a compositor would blend it with, and a
//! capture that asked for the pointer is refused rather than answered with a frame that
//! quietly lacks it. It does not read the window's identity, and it asserts nothing about
//! the application on the other side -- that is the client-text channel's job.
//!
//! # Evidence layout
//!
//! [`snapshot_path`] builds the one path shape a run uses,
//! `RUN/<module>/<TC-ID>/[step]_[state].png`, and refuses a segment that could place a file
//! outside the run's tree. The directory is created when the file is written, so a case does
//! not have to know the layout exists.
//!
//! # Modules
//!
//! `plan` holds the frame arithmetic, `pixels` the byte order, `source` the server side and
//! the read loop, `encode` the PNG, and `error` the refusals.

// The channel is exercised by the tests below and is not yet reachable from `xtask`'s
// subcommand tree, which lives in `xtask/src/main.rs` and in `xtask/src/testd/mod.rs` -- two
// files this module does not own. Until that wiring lands, every item here is reported as
// dead code in a non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the `pub use` lines below are this
// module's surface, and a `pub use` in a *binary* crate is reported as unused whenever
// nothing in the crate names it.
#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};

use ime_types::RectI;

use crate::testd::coords::normalize_scale;
use crate::testd::x11::{Window, X11Session};

mod encode;
mod error;
mod pixels;
mod plan;
mod source;

#[cfg(test)]
mod tests;

pub use self::encode::{CaptureMetadata, DEPTH_KEYWORD, SCALE_KEYWORD, read_metadata};
pub use self::error::CaptureError;
pub use self::plan::{CHUNK_ROWS, Footprint, MAX_FRAME_BYTES, MAX_SIDE_PX};

/// The longest path segment a snapshot name may be built from.
const MAX_SEGMENT_BYTES: usize = 64;

/// What a capture reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTarget {
    /// One window's own pixels, at its own resolution.
    Window(Window),
    /// The whole screen.
    FullScreen,
    /// A rectangle of the screen, in screen physical pixels.
    ///
    /// This is the target a case uses when it only audits the candidate window's rectangle:
    /// the window's position comes from the probe or from the injector's own placement, and
    /// reading the rest of the screen would cost memory for pixels nothing asserts on.
    Region(RectI),
}

impl CaptureTarget {
    /// The target as a phrase, for a refusal or a report.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(&self) -> String {
        match self {
            Self::Window(window) => format!("window {window:#x}"),
            Self::FullScreen => String::from("the whole screen"),
            Self::Region(region) => format!(
                "the region {},{} {}x{}",
                region.x, region.y, region.w, region.h
            ),
        }
    }
}

/// One capture: what to read, and where the PNG goes.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureRequest {
    /// What to read.
    pub target: CaptureTarget,
    /// Whether the pointer must appear in the image.
    ///
    /// A capture cannot include it -- `get_image` reads a drawable's own pixels and the
    /// extension that would composite the cursor is not linked -- so `true` is refused
    /// rather than answered with a frame that quietly lacks it. The field is here because a
    /// case that wanted the pointer has to be told the answer is no.
    pub include_cursor: bool,
    /// The device pixel ratio the target was rasterised at.
    ///
    /// It is the value `X11Backend::geometry()` reports. The harness cannot read it out of
    /// the plugin's process, so the caller carries it in -- and the value is written into
    /// the PNG, which is what lets a case assert the two agree.
    pub scale: f32,
    /// The file the PNG is written to; its directory is created if it is missing.
    pub out_path: PathBuf,
}

/// What one capture produced.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedImage {
    /// Width in physical pixels.
    pub width_px: u32,
    /// Height in physical pixels.
    pub height_px: u32,
    /// The device pixel ratio the file records, normalised.
    pub scale: f32,
}

/// Captures a target and writes it as a PNG.
///
/// The frame is read in row ranges, checked for a drawable that moved while it was being
/// read, converted from the server's byte order, and encoded. Nothing is written until the
/// whole frame is in hand, so a failed capture leaves no partial file behind.
///
/// # Errors
///
/// Returns [`CaptureError::CursorUnavailable`] when the request asked for the pointer,
/// [`CaptureError::Request`] when the server refuses a request, [`CaptureError::WindowMoved`]
/// when the drawable would not stay still, [`CaptureError::TooLarge`] or
/// [`CaptureError::EmptyCapture`] when the target's size cannot be captured, and
/// [`CaptureError::Png`] or [`CaptureError::Write`] when the PNG cannot be produced.
///
/// # Panics
///
/// Never.
pub fn capture(
    session: &X11Session,
    request: &CaptureRequest,
) -> Result<CapturedImage, CaptureError> {
    if request.include_cursor {
        return Err(CaptureError::CursorUnavailable);
    }
    let scale = normalize_scale(request.scale);
    let resolved = source::resolve(session, request.target)?;
    let frame_plan = plan::FramePlan::new(resolved.size.0, resolved.size.1, resolved.format)?;
    let mut frame = vec![0u8; frame_plan.buffer_len()];
    let drawable = source::X11Drawable::new(session, request.target, &resolved);
    source::read_stable(&drawable, &frame_plan, &mut frame)?;
    pixels::to_rgba(&mut frame, resolved.format);
    let metadata = CaptureMetadata {
        scale,
        depth: resolved.format.depth,
    };
    encode::write_png(
        &request.out_path,
        frame_plan.width(),
        frame_plan.height(),
        &frame,
        metadata,
    )?;
    Ok(CapturedImage {
        width_px: frame_plan.width(),
        height_px: frame_plan.height(),
        scale,
    })
}

/// The path a case's snapshot is written to, under the run's own directory.
///
/// The shape is the one the report layout fixes: `RUN/<module>/<TC-ID>/[step]_[state].png`,
/// with the step zero-padded so a directory listing sorts in the order the case ran. The
/// state tag is slugified -- lower case, runs of anything else collapsed to a single `-` --
/// so a case author writes the state the way it reads and the file name stays a name.
///
/// `module` and `case_id` are checked rather than rewritten: a case id that is not a plain
/// name is a mistake in the case, and a harness that silently turned it into something else
/// would file the evidence under a name nobody looks for. A segment that could place a file
/// outside the run's tree is refused outright.
///
/// # Errors
///
/// Returns [`CaptureError::BadSegment`] for an empty, oversized or unusable segment, and for
/// a state tag that slugifies to nothing.
///
/// # Panics
///
/// Never.
pub fn snapshot_path(
    run_root: &Path,
    module: &str,
    case_id: &str,
    step: u32,
    state: &str,
) -> Result<PathBuf, CaptureError> {
    let module = checked_segment(module)?;
    let case_id = checked_segment(case_id)?;
    let state = slug(state);
    if state.is_empty() {
        return Err(CaptureError::BadSegment {
            segment: state,
            reason: "the state tag has no letter or digit in it",
        });
    }
    Ok(run_root
        .join(module)
        .join(case_id)
        .join(format!("{step:02}_{state}.png")))
}

/// Checks that a segment is a plain path component.
///
/// # Errors
///
/// Returns [`CaptureError::BadSegment`] when it is empty, longer than
/// [`MAX_SEGMENT_BYTES`], a relative-path step, or carries a character that is not a letter,
/// a digit, a dash, an underscore or a dot.
fn checked_segment(segment: &str) -> Result<&str, CaptureError> {
    if segment.is_empty() {
        return Err(bad_segment(segment, "it is empty"));
    }
    if segment.len() > MAX_SEGMENT_BYTES {
        return Err(bad_segment(segment, "it is longer than a name may be"));
    }
    if segment == "." || segment == ".." {
        return Err(bad_segment(
            segment,
            "it would step out of the run's directory",
        ));
    }
    let plain = segment
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if !plain {
        return Err(bad_segment(
            segment,
            "it carries a character a file name may not",
        ));
    }
    Ok(segment)
}

/// The refusal a path segment that cannot name a file is reported with.
fn bad_segment(segment: &str, reason: &'static str) -> CaptureError {
    CaptureError::BadSegment {
        segment: segment.to_owned(),
        reason,
    }
}

/// The slug form of a state tag: lower case, runs of anything else collapsed to one dash.
///
/// A tag that starts or ends with a separator loses it, so `Hover cell` and `hover-cell`
/// produce the same name and a case cannot file two snapshots of one step under two names.
fn slug(state: &str) -> String {
    let mut slug = String::with_capacity(state.len());
    for ch in state.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}
