//! How much of a drawable one capture reads, and in which pieces.
//!
//! Responsibility: the arithmetic of a capture -- how many bytes a frame takes, whether the
//! harness will spend them, and which row range each `get_image` request asks for. Nothing
//! here touches a connection, so the whole module is unit-testable with no display server.
//!
//! # Why a frame is read in row ranges
//!
//! `get_image` answers with one reply holding every pixel of the rectangle that was asked
//! for, and the reply's length field is 32 bits wide. A 4K screen at scale 2.0 is 7680x4320
//! pixels -- 132 MB, which is both a large allocation to ask a display server for in one
//! piece and close enough to the reply ceiling to be worth not testing. Reading it as row
//! ranges of at most [`CHUNK_ROWS`] keeps every reply bounded and every offset inside the
//! 16-bit fields the request carries, and the frame the pieces are copied into is the same
//! either way.

use ime_types::RectI;

use super::CaptureError;
use super::pixels::PixelFormat;

/// Rows one `get_image` request asks for.
///
/// At the widest capture the channel plans (7680 pixels) one reply is then 60 MB, which is
/// the largest single allocation the harness asks a server for.
pub const CHUNK_ROWS: u32 = 2048;

/// The widest or tallest capture this channel will plan.
///
/// `get_image` carries its `x` and `y` as signed 16-bit fields, so a rectangle past this
/// cannot be addressed at all. The limit comes from the protocol rather than from a policy
/// choice, which is why it is a plain constant and not a configuration.
pub const MAX_SIDE_PX: u32 = i16::MAX as u32;

/// The most bytes one capture's frame may take.
///
/// The frame, the reply a chunk arrives in and the PNG encoder's own buffers are alive at
/// the same time, so the ceiling is a third of what the harness is willing to see one
/// capture cost. A 4K screen at scale 2.0 is 132 MB and fits; an 8K one at 2.0 does not, and
/// the refusal names `Region` because narrowing the capture is the answer.
pub const MAX_FRAME_BYTES: u64 = 256 * 1024 * 1024;

/// Where a drawable sat on the screen, and how big it was, at one moment.
///
/// The position is what a `Window` target has and a root target does not: the root is
/// always at the screen's own origin. It is carried for every target anyway, so the check
/// that a drawable did not move while it was being read is one comparison rather than two
/// shapes of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    /// Top-left corner, in screen physical pixels.
    pub origin: (i32, i32),
    /// Size in physical pixels.
    pub size: (u32, u32),
}

impl Footprint {
    /// The footprint as `x`, `y`, `width`, `height`, which is how a refusal prints it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn quad(self) -> (i32, i32, u32, u32) {
        (self.origin.0, self.origin.1, self.size.0, self.size.1)
    }
}

/// One `get_image` request's row range inside the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRange {
    /// First row of the range, counted from the frame's top.
    pub y: u32,
    /// Rows the range covers; never zero, so a request is never empty.
    pub rows: u32,
}

/// One capture's frame: its size, the width of one row, and the ranges it is read in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramePlan {
    /// Width in physical pixels.
    width: u32,
    /// Height in physical pixels.
    height: u32,
    /// Bytes one row takes in the reply's layout.
    row_bytes: usize,
    /// The row ranges, in order, covering the whole frame exactly once.
    chunks: Vec<ChunkRange>,
}

impl FramePlan {
    /// Plans a frame of `width` x `height` physical pixels in `format`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::EmptyCapture`] for a target with no pixels, and
    /// [`CaptureError::TooLarge`] for a frame past the memory ceiling or a side the request's
    /// 16-bit fields cannot address.
    ///
    /// # Panics
    ///
    /// Never. The row count is computed with `div_ceil`, so no input can make it overflow.
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Result<Self, CaptureError> {
        if width == 0 || height == 0 {
            return Err(CaptureError::EmptyCapture { width, height });
        }
        let row_bytes = format.row_bytes(width);
        let bytes = row_bytes as u64 * u64::from(height);
        if width > MAX_SIDE_PX || height > MAX_SIDE_PX || bytes > MAX_FRAME_BYTES {
            return Err(CaptureError::TooLarge {
                width,
                height,
                bytes,
                limit: MAX_FRAME_BYTES,
            });
        }
        let chunks = (0..height.div_ceil(CHUNK_ROWS))
            .map(|index| {
                let y = index * CHUNK_ROWS;
                ChunkRange {
                    y,
                    rows: CHUNK_ROWS.min(height - y),
                }
            })
            .collect();
        Ok(Self {
            width,
            height,
            row_bytes,
            chunks,
        })
    }

    /// Width of the frame in physical pixels.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the frame in physical pixels.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes one row of the frame takes.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn row_bytes(&self) -> usize {
        self.row_bytes
    }

    /// The row ranges the frame is read in.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn chunks(&self) -> &[ChunkRange] {
        &self.chunks
    }

    /// Bytes the whole frame takes.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn buffer_len(&self) -> usize {
        self.row_bytes * self.height as usize
    }

    /// The slice of `frame` one chunk's rows belong in.
    ///
    /// Answers `None` when the frame is not the buffer this plan describes, which is the one
    /// way the copy could run past the end of it. The slice is exactly `rows * row_bytes`
    /// long, so the reader that fills it cannot write outside the range either.
    ///
    /// # Panics
    ///
    /// Never: the arithmetic is checked and the slice is taken with `get_mut`.
    pub fn chunk_slice<'f>(&self, frame: &'f mut [u8], chunk: ChunkRange) -> Option<&'f mut [u8]> {
        let start = chunk.y as usize * self.row_bytes;
        let end = start.checked_add(chunk.rows as usize * self.row_bytes)?;
        frame.get_mut(start..end)
    }
}

/// Checks that a region the caller named lies inside the screen.
///
/// # Errors
///
/// Returns [`CaptureError::EmptyCapture`] for a region with no pixels, and
/// [`CaptureError::RegionOffScreen`] for one that is not wholly on the screen. The bounds
/// are half-open, the rule the coordinate conversion uses: a region ending exactly on the
/// right or bottom edge is on the screen. The check is made in 64 bits, so a region that
/// would wrap when its size is added to its position is refused rather than folded back
/// onto the screen.
///
/// # Panics
///
/// Never.
pub fn check_region(region: RectI, screen: (u16, u16)) -> Result<(), CaptureError> {
    if region.w == 0 || region.h == 0 {
        return Err(CaptureError::EmptyCapture {
            width: region.w,
            height: region.h,
        });
    }
    let right = i64::from(region.x) + i64::from(region.w);
    let bottom = i64::from(region.y) + i64::from(region.h);
    let off_screen = region.x < 0
        || region.y < 0
        || right > i64::from(screen.0)
        || bottom > i64::from(screen.1)
        || right > i64::from(MAX_SIDE_PX)
        || bottom > i64::from(MAX_SIDE_PX);
    if off_screen {
        return Err(CaptureError::RegionOffScreen {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            screen_width: screen.0,
            screen_height: screen.1,
        });
    }
    Ok(())
}
