//! The pixel layout a capture arrives in, and the one it is written out in.
//!
//! Responsibility: say which layouts this channel can read, how wide a row of one is, and
//! the byte swap between the server's order and the PNG's. Nothing here touches a
//! connection, so the whole module is unit-testable with no display server.
//!
//! # Why the byte order is fixed rather than parameterised
//!
//! An X server sends image data in the byte order the client declared in its connection
//! setup, so a little-endian client reading a 32-bit TrueColor drawable receives each pixel
//! as `B, G, R, X` in memory for the mask triple `0x00ff0000 / 0x0000ff00 / 0x000000ff`.
//! That is the same layout `ime-ui` writes its `Argb8888` buffers in, and the same one the
//! candidate window's ARGB visual is selected for -- a visual whose masks differ would
//! render blue and red swapped, which is worse than the refusal this module answers with.
//!
//! A layout that is not that one is therefore refused rather than guessed at: a capture that
//! came back with its channels swapped would pass every geometric assertion in a visual
//! audit while reporting colours no user saw.

use x11rb::protocol::xproto::{Format, Screen, Visualid, Visualtype};

use super::CaptureError;

/// One drawable's pixel layout, as the server describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    /// Depth of the drawable: 24 when the fourth byte is padding, 32 when it is alpha.
    pub depth: u8,
    /// Bits one pixel occupies in a reply, which is the server's `pixmap_format`.
    pub bits_per_pixel: u8,
    /// Mask of the red channel inside a pixel.
    pub red_mask: u32,
    /// Mask of the green channel inside a pixel.
    pub green_mask: u32,
    /// Mask of the blue channel inside a pixel.
    pub blue_mask: u32,
}

impl PixelFormat {
    /// The one layout this channel unpacks: 32 bits per pixel, blue first in memory.
    ///
    /// The depth is not part of the identity: a depth-24 drawable is carried in the same
    /// four bytes, with the last one unused. What the depth decides is whether that fourth
    /// byte is the drawable's own alpha, which is [`PixelFormat::has_alpha`].
    pub const BGRA8888: Self = Self {
        depth: 24,
        bits_per_pixel: 32,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
    };

    /// Refuses a layout this channel cannot unpack.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::UnsupportedVisual`] when the drawable is not 32 bits per
    /// pixel or its channel masks are not the ones the byte swap assumes.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn checked(self) -> Result<Self, CaptureError> {
        let matches = self.bits_per_pixel == Self::BGRA8888.bits_per_pixel
            && self.red_mask == Self::BGRA8888.red_mask
            && self.green_mask == Self::BGRA8888.green_mask
            && self.blue_mask == Self::BGRA8888.blue_mask;
        if matches {
            return Ok(self);
        }
        Err(CaptureError::UnsupportedVisual {
            depth: self.depth,
            bits_per_pixel: self.bits_per_pixel,
            red_mask: self.red_mask,
            green_mask: self.green_mask,
            blue_mask: self.blue_mask,
        })
    }

    /// Bytes one row of `width` pixels takes in a reply.
    ///
    /// The X11 rule, not a choice: a scanline is padded to a multiple of four bytes, which
    /// for the 32-bit layout this channel reads is exactly `width * 4` and leaves no padding
    /// at all. The rule is written out rather than assumed so a layout that did pad would
    /// compute the wrong length here instead of reading the wrong pixels later.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn row_bytes(self, width: u32) -> usize {
        let bits = width as usize * self.bits_per_pixel as usize;
        bits.div_ceil(32) * 4
    }

    /// Whether the fourth byte of a pixel is the drawable's own alpha.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn has_alpha(self) -> bool {
        self.depth == 32
    }

    /// The format a screen's root window is drawn in, from its visual and its depth.
    ///
    /// `bits_per_pixel` is not a property of a visual on the wire, so it comes from the
    /// setup's pixmap format table: the entry whose depth matches the visual's is the one
    /// the server uses to carry that visual's pixels.
    ///
    /// Answers `None` when the setup does not list the visual, which is a server describing
    /// a drawable it has not described the layout of. The caller refuses rather than
    /// assuming a layout, because a wrong assumption here is a capture with its channels
    /// swapped.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of_screen(screen: &Screen, pixmap_formats: &[Format]) -> Option<Self> {
        let visual = find_visual(screen, screen.root_visual)?;
        Some(Self {
            depth: screen.root_depth,
            bits_per_pixel: bits_per_pixel(pixmap_formats, screen.root_depth),
            red_mask: visual.red_mask,
            green_mask: visual.green_mask,
            blue_mask: visual.blue_mask,
        })
    }

    /// The format a window is drawn in, from the visual the server reports for it.
    ///
    /// Answers `None` when the setup does not list the visual, as [`PixelFormat::of_screen`]
    /// does.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of_visual(
        screen: &Screen,
        pixmap_formats: &[Format],
        visual_id: Visualid,
        depth: u8,
    ) -> Option<Self> {
        let visual = find_visual(screen, visual_id)?;
        Some(Self {
            depth,
            bits_per_pixel: bits_per_pixel(pixmap_formats, depth),
            red_mask: visual.red_mask,
            green_mask: visual.green_mask,
            blue_mask: visual.blue_mask,
        })
    }
}

/// The visual with `visual_id`, from the screen's allowed depths.
fn find_visual(screen: &Screen, visual_id: Visualid) -> Option<&Visualtype> {
    screen
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == visual_id)
}

/// Bits one pixel of `depth` takes, from the setup's pixmap format table.
///
/// The table is the server's own statement of how it carries that depth, so a depth it does
/// not list falls back to the 32 bits per pixel every layout this channel reads uses; the
/// masks are what decide whether the layout is readable, and they are checked separately.
fn bits_per_pixel(pixmap_formats: &[Format], depth: u8) -> u8 {
    pixmap_formats
        .iter()
        .find(|format| format.depth == depth)
        .map_or(32, |format| format.bits_per_pixel)
}

/// Rewrites a whole frame from the reply's byte order into the PNG's, in place.
///
/// The swap is done in place because the frame of a 4K capture is 132 MB: a second buffer
/// for the converted copy would double what one capture costs for no gain. Every pixel is
/// four bytes and a row is a whole number of them, so the walk cannot end mid-pixel.
///
/// Where the drawable has no alpha channel the fourth byte is padding and is written as
/// opaque, because a PNG reader that found the padding there would show a window nobody can
/// see. Where it has one, the byte is the drawable's own alpha and is carried through
/// unchanged: it is the value the server sent, and premultiplying or unpremultiplying it
/// here would report pixels that were never drawn.
///
/// # Panics
///
/// Never: the walk is over whole pixels and a short frame only leaves a remainder unwritten.
pub fn to_rgba(frame: &mut [u8], format: PixelFormat) {
    let opaque = !format.has_alpha();
    for pixel in frame.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if opaque {
            pixel[3] = 255;
        }
    }
}
