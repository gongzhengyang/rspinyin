//! Writing a captured frame as a PNG, and reading back what it says about itself.
//!
//! Responsibility: turn a frame of RGBA bytes into the file a case archives, and read the
//! record that file carries. Nothing here touches a connection.
//!
//! # Why the record is in the file
//!
//! The image is physical pixels, and the specification it is audited against is written in
//! logical ones. A later audit that had to guess the ratio would be comparing two different
//! things, so the ratio travels inside the file: the `scale` text chunk is the device pixel
//! ratio the surface was rasterised at, and `depth` says whether the fourth byte of a pixel
//! is the drawable's own alpha or padding. Both are read back by
//! [`read_metadata`], and a case that asserts the capture against `X11Backend::geometry()`
//! compares the two numbers rather than trusting that they match.
//!
//! The file is written directly, without a buffering layer, because a buffered writer that
//! is dropped flushes into nothing: its failure would be reported as a successful capture of
//! a truncated file.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use png::text_metadata::TEXtChunk;

use super::CaptureError;

/// The PNG text chunk the device pixel ratio is recorded in.
pub const SCALE_KEYWORD: &str = "scale";

/// The PNG text chunk the drawable's depth is recorded in.
pub const DEPTH_KEYWORD: &str = "depth";

/// What a capture records about itself beside its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureMetadata {
    /// The device pixel ratio the captured surface was rasterised at.
    ///
    /// It is the value the caller reported -- the one `X11Backend::geometry()` hands out --
    /// carried through unchanged, so an audit compares the file against the backend's own
    /// answer rather than against a value the harness recomputed.
    pub scale: f32,
    /// Depth of the drawable: 32 when the fourth byte of a pixel is alpha, 24 when it is
    /// padding the unpack wrote as opaque.
    pub depth: u8,
}

/// Writes a frame of RGBA bytes as a PNG, with its capture record.
///
/// The directory the file goes into is created if it is not there: a case's evidence lives
/// under `RUN/<module>/<TC-ID>/`, and a run that has not written a snapshot yet has no such
/// directory.
///
/// # Errors
///
/// Returns [`CaptureError::Png`] when the frame cannot be encoded, and
/// [`CaptureError::Write`] when the directory or the file cannot be created or the bytes
/// cannot be written.
///
/// # Panics
///
/// Never. The frame's length is checked by the encoder, which reports a mismatch as an
/// encoding error rather than reading past the end of it.
pub fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
    meta: CaptureMetadata,
) -> Result<(), CaptureError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| write_error(path, &error))?;
    }
    let file = File::create(path).map_err(|error| write_error(path, &error))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_text_chunk(SCALE_KEYWORD.to_owned(), scale_text(meta.scale))
        .map_err(codec_error)?;
    encoder
        .add_text_chunk(DEPTH_KEYWORD.to_owned(), meta.depth.to_string())
        .map_err(codec_error)?;
    let mut writer = encoder.write_header().map_err(codec_error)?;
    writer.write_image_data(rgba).map_err(codec_error)?;
    writer.finish().map_err(codec_error)
}

/// Reads the capture record a PNG carries.
///
/// Answers `None` when the file is a readable PNG that does not carry the record -- either
/// chunk missing, or the ratio's text not a number -- because a file that does not say what
/// ratio it was taken at cannot be audited against a specification written in logical
/// pixels, and a default would hide that.
///
/// # Errors
///
/// Returns [`CaptureError::Read`] when the file cannot be opened, and [`CaptureError::Png`]
/// when it is not a PNG this build can read.
///
/// # Panics
///
/// Never.
pub fn read_metadata(path: &Path) -> Result<Option<CaptureMetadata>, CaptureError> {
    let file = File::open(path).map_err(|error| CaptureError::Read {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    // `png::Decoder` reads through a `BufRead`, which a bare `File` is not.
    let reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(codec_error)?;
    let chunks = &reader.info().uncompressed_latin1_text;
    let scale = text_of(chunks, SCALE_KEYWORD).and_then(|text| text.parse::<f32>().ok());
    let depth = text_of(chunks, DEPTH_KEYWORD).and_then(|text| text.parse::<u8>().ok());
    Ok(match (scale, depth) {
        (Some(scale), Some(depth)) => Some(CaptureMetadata { scale, depth }),
        _ => None,
    })
}

/// The text of one chunk, if the file carries it.
fn text_of<'a>(chunks: &'a [TEXtChunk], keyword: &str) -> Option<&'a str> {
    chunks
        .iter()
        .find(|chunk| chunk.keyword == keyword)
        .map(|chunk| chunk.text.as_str())
}

/// The ratio as the text a chunk carries.
///
/// The shortest decimal that reads back as the same `f32`: `1.25` stays `1.25` and `2.0`
/// becomes `2`, which parses to the same value. A ratio that cannot be used at all is
/// written as `1`, the value the capture normalises it to, so a file never carries a number
/// no audit can divide by.
fn scale_text(scale: f32) -> String {
    format!("{}", crate::testd::coords::normalize_scale(scale))
}

/// The refusal a filesystem failure is reported with.
fn write_error(path: &Path, error: &std::io::Error) -> CaptureError {
    CaptureError::Write {
        path: path.to_path_buf(),
        detail: error.to_string(),
    }
}

/// The refusal a codec failure is reported with.
fn codec_error<E: std::fmt::Display>(error: E) -> CaptureError {
    CaptureError::Png {
        detail: error.to_string(),
    }
}
