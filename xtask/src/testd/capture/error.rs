//! The refusals the screenshot channel reports.
//!
//! Each variant is a verdict a case acts on differently: a `RegionOffScreen` means the case
//! aimed at the wrong place, a `WindowMoved` means the run has to be retried rather than
//! believed, and a `TooLarge` means the target has to be narrowed. A single "capture failed"
//! would leave the reader to work that out from a string.

use std::path::PathBuf;

use ime_types::RectI;

use super::CaptureTarget;

/// Everything one capture can be refused with.
///
/// `PartialEq` is derived so a case can assert on a returned `Result` directly instead of
/// matching every variant by hand, the way the injection channel's error does.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// An X request the capture needs could not be answered.
    ///
    /// A window that is not viewable, a rectangle that reaches outside the drawable and a
    /// connection that is gone all arrive here, because the server reports all three the
    /// same way.
    #[error("X request failed while capturing: {detail}")]
    Request {
        /// What the failing request reported.
        detail: String,
    },

    /// The capture would have taken more memory than the harness will spend on one frame.
    #[error(
        "a {width}x{height} capture is {bytes} bytes, past the {limit} byte ceiling one frame \
         may take; name a Region of it instead of the whole screen"
    )]
    TooLarge {
        /// Width in physical pixels.
        width: u32,
        /// Height in physical pixels.
        height: u32,
        /// Bytes the frame would have taken.
        bytes: u64,
        /// The ceiling that was exceeded.
        limit: u64,
    },

    /// The target has no pixels.
    ///
    /// An unmapped window reports a zero-sized geometry, and a case that captured it anyway
    /// would write an empty PNG that no assertion could tell from a blank window.
    #[error(
        "the capture target has no pixels ({width}x{height}); an unmapped or withdrawn window \
         reports an empty geometry"
    )]
    EmptyCapture {
        /// Width in physical pixels.
        width: u32,
        /// Height in physical pixels.
        height: u32,
    },

    /// A region the caller named is not on the screen.
    #[error("the region {x},{y} {w}x{h} is not on the {screen_width}x{screen_height} screen")]
    RegionOffScreen {
        /// Left edge of the region, in screen pixels.
        x: i32,
        /// Top edge of the region, in screen pixels.
        y: i32,
        /// Width of the region.
        w: u32,
        /// Height of the region.
        h: u32,
        /// The screen's width in pixels.
        screen_width: u16,
        /// The screen's height in pixels.
        screen_height: u16,
    },

    /// The drawable moved or resized while its pixels were being read.
    ///
    /// The retry the channel makes before reporting this is what keeps a frame that was read
    /// across a move from being written as a baseline: the pixels would be a real rendering
    /// of a rectangle that is no longer where the capture says it is.
    #[error(
        "the {} moved while it was being read: it was at {before:?} and is now at \
         {after:?} (x, y, width, height)",
        target.label()
    )]
    WindowMoved {
        /// The target that was being captured.
        target: CaptureTarget,
        /// The footprint before the read.
        before: (i32, i32, u32, u32),
        /// The footprint after the read.
        after: (i32, i32, u32, u32),
    },

    /// The server returned fewer bytes than the geometry promised.
    #[error("the capture expected {expected} bytes and got {actual}")]
    ShortRead {
        /// Bytes the geometry called for.
        expected: usize,
        /// Bytes that arrived.
        actual: usize,
    },

    /// The drawable's pixels are in a layout this channel cannot unpack.
    #[error(
        "the drawable is depth {depth} at {bits_per_pixel} bits per pixel with masks \
         {red_mask:#010x}/{green_mask:#010x}/{blue_mask:#010x}, which is not the 32-bit \
         TrueColor layout this channel reads; a capture here would report swapped channels"
    )]
    UnsupportedVisual {
        /// Depth of the drawable.
        depth: u8,
        /// Bits one pixel occupies in the reply.
        bits_per_pixel: u8,
        /// Red channel mask.
        red_mask: u32,
        /// Green channel mask.
        green_mask: u32,
        /// Blue channel mask.
        blue_mask: u32,
    },

    /// The server did not describe the layout of the drawable's visual.
    #[error(
        "the server lists no visual {visual:#x} among the screen's, so the drawable's pixel \
         layout is unknown; a capture here would be read with the wrong byte order"
    )]
    UnknownVisual {
        /// The visual the drawable reported.
        visual: u32,
    },

    /// A capture including the pointer was asked for.
    ///
    /// `get_image` reads a drawable's own pixels and never composites the pointer, so a
    /// cursor-inclusive screenshot needs the XFIXES extension, which this harness does not
    /// link. Refusing is the point: a case that asked for the pointer and silently got a
    /// frame without it would compare two images that are not the same thing.
    #[error(
        "a capture cannot include the pointer: get_image reads the drawable's own pixels and \
         the XFIXES extension that would composite the cursor is not linked into this harness; \
         capture without it, or read the pointer position separately"
    )]
    CursorUnavailable,

    /// The frame could not be written as a PNG, or a PNG could not be read back.
    ///
    /// One variant covers both directions because the refusal a caller acts on is the same:
    /// the file in hand is not the image it was supposed to be.
    #[error("the capture could not be read or written as a PNG: {detail}")]
    Png {
        /// What the codec reported.
        detail: String,
    },

    /// A capture could not be read.
    #[error("the capture {path} could not be read: {detail}")]
    Read {
        /// The path that was tried.
        path: PathBuf,
        /// What the filesystem reported.
        detail: String,
    },

    /// The PNG could not be written.
    #[error("the capture could not be written to {path}: {detail}")]
    Write {
        /// The path that was tried.
        path: PathBuf,
        /// What the filesystem reported.
        detail: String,
    },

    /// A path segment a snapshot name is built from is not usable.
    #[error("`{segment}` cannot be used in a snapshot path: {reason}")]
    BadSegment {
        /// The segment that was refused.
        segment: String,
        /// Why it cannot be used.
        reason: &'static str,
    },
}
