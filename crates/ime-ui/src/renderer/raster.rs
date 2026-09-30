//! The raster pixel type, the scratch a frame is drawn into, and the rectangle
//! arithmetic the copy out of it needs.
//!
//! # Why the pixel type is ours
//!
//! A `wl_shm` buffer created as `WL_SHM_FORMAT_ARGB8888` and a 32-bit X11 `TrueColor`
//! visual both hold one `0xAARRGGBB` word per pixel, which on the little-endian hosts this
//! project ships on means the bytes `B, G, R, A`. Slint 1.13 implements its `TargetPixel`
//! trait for `Rgb8Pixel`, `Rgb565Pixel` and `PremultipliedRgbaColor` only, and the last of
//! those stores its channels as `R, G, B, A`. Implementing the trait here, in the surface
//! order, keeps the copy into the surface buffer a plain byte copy: no channel is ever
//! rearranged, and the format agreement is a property of the type rather than something
//! that has to be re-checked against a spike.
//!
//! # Why the scratch exists at all
//!
//! A surface buffer arrives as `&mut [u8]`. Viewing a byte slice as a slice of a
//! four-byte pixel type needs the alignment and the length to be argued for, which means
//! `unsafe`, and `ime-ui` is not on the allow-list for it. The frame is therefore
//! rasterized into a scratch this crate owns and copied across afterwards.

use ime_types::{PlatformError, RectI};
use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};

/// Bytes per pixel of the `Argb8888` format, named here so the copy reads without casts.
pub(super) const BYTES_PER_PIXEL: usize = crate::platform::BYTES_PER_PIXEL as usize;

/// Extra pixels appended to every scratch row.
///
/// Slint asserts that the buffer it is handed covers the window size it derived from the
/// root item's geometry, and that size goes through a logical-to-physical round trip
/// (`width_px` -> `width_px / scale` -> `(width_px / scale) * scale`). The trip is exact
/// for the integral scale factors this project targets, but a rounding that came out one
/// pixel high would abort inside the renderer, so the scratch carries a little slack.
const STRIDE_SLACK: usize = 2;

/// Extra rows appended below every scratch frame, for the same reason as [`STRIDE_SLACK`].
const ROW_SLACK: usize = 1;

/// Frames a shrunk scratch must serve before its allocation is given back.
///
/// Shrinking on the first small frame would trade an allocation for a window resize -- a
/// user dragging the scale factor, an output change, a candidate list that shrank -- which
/// is exactly the churn the grow-only rule avoided. A sustained shrink is a different
/// situation: the window is genuinely smaller now, and holding the peak allocation costs
/// resident memory against `BUDGET-MEM-01` for the rest of the session.
///
/// The count is in frames the scratch actually served, not in wall-clock time. A frame the
/// window did not have to redraw never reaches the scratch, and waking the UI thread to
/// look at the allocation would be the polling timer `BUDGET-CPU-01` forbids. A window that
/// shrank and then went completely idle therefore keeps the allocation until it is redrawn
/// again -- the deliberate trade, since such a window pays for it only in resident bytes.
pub(super) const SHRINK_AFTER_FRAMES: u32 = 3_600;

/// How much of the allocation a frame may need and still count towards a shrink.
///
/// A quarter, written as a divisor so the comparison stays in integer arithmetic. A frame
/// needing more than this is not evidence that the window has settled at a smaller size: a
/// window whose size fluctuates around half the allocation must never give the peak away
/// and then ask for it back.
const SHRINK_DIVISOR: usize = 4;

/// One premultiplied `Argb8888` pixel, in the byte order the surface buffers use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Argb8888Pixel(u32);

impl Argb8888Pixel {
    /// A fully transparent pixel: what a fresh scratch is filled with.
    pub(super) const TRANSPARENT: Self = Self(0);

    /// Packs four channels into the little-endian `0xAARRGGBB` word.
    pub(super) fn pack(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self(u32::from_le_bytes([blue, green, red, alpha]))
    }

    /// Splits the word back into `(red, green, blue, alpha)`.
    fn channels(self) -> (u8, u8, u8, u8) {
        let [blue, green, red, alpha] = self.0.to_le_bytes();
        (red, green, blue, alpha)
    }

    /// The four bytes as they are written into a surface buffer.
    pub(super) fn to_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }

    /// The alpha channel, which is what tells ink from background.
    pub(super) fn alpha(self) -> u8 {
        self.channels().3
    }
}

impl TargetPixel for Argb8888Pixel {
    /// Blends `color` over this pixel.
    ///
    /// Both sides are premultiplied, so the arithmetic is the one Slint's own
    /// `PremultipliedRgbaColor` uses, widened to `u16` and saturated: a colour whose
    /// channels do not satisfy the premultiplied invariant (`channel <= alpha`) then
    /// clamps instead of wrapping, and wrapping would be a debug-build panic.
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let (red, green, blue, alpha) = self.channels();
        let inverse = u16::from(u8::MAX - color.alpha);
        let over = |destination: u8, source: u8| -> u8 {
            (u16::from(destination) * inverse / 255 + u16::from(source)).min(255) as u8
        };
        let red = over(red, color.red);
        let green = over(green, color.green);
        let blue = over(blue, color.blue);
        let alpha = (u16::from(alpha) + u16::from(color.alpha)
            - u16::from(alpha) * u16::from(color.alpha) / 255)
            .min(255) as u8;
        *self = Self::pack(red, green, blue, alpha);
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::pack(red, green, blue, u8::MAX)
    }

    fn background() -> Self {
        // Transparent rather than opaque black: the candidate window is drawn over the
        // application, so a pixel no element covers must stay see-through.
        Self::TRANSPARENT
    }
}

/// The buffer a frame is rasterized into before it is copied to the surface.
pub(super) struct PixelScratch {
    /// The pixels, `stride * rows` of them.
    pub(super) pixels: Vec<Argb8888Pixel>,
    /// Row length in pixels; never smaller than the surface width.
    pub(super) stride: usize,
    /// Consecutive frames that fit in a quarter of the allocation, counted towards
    /// [`SHRINK_AFTER_FRAMES`].
    small_frames: u32,
}

impl PixelScratch {
    /// Creates an empty scratch; the first [`Self::ensure`] allocates.
    pub(super) fn new() -> Self {
        Self {
            pixels: Vec::new(),
            stride: 0,
            small_frames: 0,
        }
    }

    /// Grows or shrinks the scratch to cover `width_px` by `height_px`, reporting whether
    /// it reallocated.
    ///
    /// Growth is immediate: a frame that does not fit has to be served, so the allocation
    /// happens on the spot. Shrinkage waits for [`SHRINK_AFTER_FRAMES`] consecutive frames
    /// that need no more than a [`SHRINK_DIVISOR`]-th of the allocation, so a one-off
    /// resize does not cause churn and a sustained one does not hold the peak allocation
    /// for the rest of the session.
    ///
    /// # Returns
    ///
    /// Whether the buffer was reallocated, in either direction. The previous frame's
    /// pixels are gone when it returns `true`, which is why the caller has to schedule a
    /// full repaint. Forgetting that on a shrink is a correctness bug rather than a cost:
    /// the surface still shows the frame the smaller scratch no longer holds, so a partial
    /// copy would leave the rest of the window showing whatever the buffer happened to
    /// contain.
    pub(super) fn ensure(&mut self, width_px: u32, height_px: u32) -> bool {
        self.stride = width_px as usize + STRIDE_SLACK;
        let rows = height_px as usize + ROW_SLACK;
        let needed = self.stride * rows;
        if self.pixels.len() < needed {
            self.pixels.resize(needed, Argb8888Pixel::TRANSPARENT);
            // A fresh allocation is not a candidate for being given back.
            self.small_frames = 0;
            return true;
        }
        // `saturating_mul`: the surface size is caller-supplied, and a size whose quarter
        // overflows `usize` is one no scratch could hold. Saturating keeps it out of the
        // shrink path rather than wrapping into it.
        if needed.saturating_mul(SHRINK_DIVISOR) > self.pixels.len() {
            self.small_frames = 0;
            return false;
        }
        self.small_frames = self.small_frames.saturating_add(1);
        if self.small_frames < SHRINK_AFTER_FRAMES {
            return false;
        }
        self.small_frames = 0;
        // Truncating the length is not enough: the memory budget counts the allocation, so
        // the tail has to be handed back to the allocator.
        self.pixels.truncate(needed);
        self.pixels.shrink_to_fit();
        true
    }

    /// Copies one rectangle of the scratch into a surface buffer.
    ///
    /// `rect` is expected to be clipped to the surface already; the copy is refused rather
    /// than clamped so that a geometry mistake shows up as a skipped frame instead of as
    /// pixels in the wrong place.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Unavailable`] when either side is shorter than the
    /// rectangle needs. The backend contract promises a buffer of at least
    /// `width * height * 4` bytes, so a short one is a backend defect: there is no code for
    /// that case, and refusing to write is better than writing out of range.
    pub(super) fn blit_into(
        &self,
        dst: &mut [u8],
        dst_stride: usize,
        rect: RectI,
    ) -> Result<(), PlatformError> {
        let x0 = rect.x.max(0) as usize;
        let y0 = rect.y.max(0) as usize;
        let columns = rect.w as usize;
        let rows = rect.h as usize;
        for row in y0..y0 + rows {
            let source_start = row * self.stride + x0;
            let Some(source) = self.pixels.get(source_start..source_start + columns) else {
                return Err(PlatformError::Unavailable);
            };
            let destination_start = row * dst_stride + x0 * BYTES_PER_PIXEL;
            let Some(destination) =
                dst.get_mut(destination_start..destination_start + columns * BYTES_PER_PIXEL)
            else {
                return Err(PlatformError::Unavailable);
            };
            for (pixel, out) in source
                .iter()
                .zip(destination.chunks_exact_mut(BYTES_PER_PIXEL))
            {
                out.copy_from_slice(&pixel.to_bytes());
            }
        }
        Ok(())
    }
}

/// Clips a rectangle to a surface, or drops it when nothing of it is left.
///
/// This is the single-rectangle form of [`crate::platform::clip_rects`], kept separate
/// because the per-frame path must not allocate and the whole damage list is rebuilt every
/// frame.
pub(super) fn clip_rect(rect: RectI, width_px: u32, height_px: u32) -> Option<RectI> {
    // Widened to `i64`: a rectangle may carry `i32::MIN` and would overflow when offset by
    // the surface origin.
    let x0 = i64::from(rect.x).max(0);
    let y0 = i64::from(rect.y).max(0);
    let x1 = (i64::from(rect.x) + i64::from(rect.w)).min(i64::from(width_px));
    let y1 = (i64::from(rect.y) + i64::from(rect.h)).min(i64::from(height_px));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(RectI {
        x: x0 as i32,
        y: y0 as i32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    })
}

/// The bounding box of two rectangles.
///
/// The two-rectangle form of [`union_rect`], kept separate because the frame path folds over
/// two lists it must not concatenate: a temporary vector of damage rectangles is exactly the
/// allocation the per-frame path may not make. Like `union_rect` it returns a superset rather
/// than a difference, which is what keeps the result a single rectangle -- and a superset is
/// always safe for a copy.
pub(super) fn union_pair(left: RectI, right: RectI) -> RectI {
    // Widened to `i64` for the reason [`clip_rect`] gives: a rectangle may carry `i32::MIN`
    // and would overflow when its extent is added to its origin.
    let x0 = i64::from(left.x).min(i64::from(right.x));
    let y0 = i64::from(left.y).min(i64::from(right.y));
    let x1 = (i64::from(left.x) + i64::from(left.w)).max(i64::from(right.x) + i64::from(right.w));
    let y1 = (i64::from(left.y) + i64::from(left.h)).max(i64::from(right.y) + i64::from(right.h));
    RectI {
        x: x0 as i32,
        y: y0 as i32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    }
}

/// Whether two rectangles share at least one pixel.
///
/// This is the predicate the damage merge uses: two regions that share a pixel are folded
/// into their bounding box, while two that merely touch along an edge are left apart,
/// because merging those would add area to the copy without removing a rectangle from the
/// list.
///
/// # Why the arithmetic is widened
///
/// A rectangle may carry `i32::MIN` for its origin and `u32::MAX` for its extent -- the
/// damage bookkeeping clips whatever a renderer reports -- so both far edges are computed
/// in `i64` for the reason [`clip_rect`] gives.
pub(super) fn overlaps(left: RectI, right: RectI) -> bool {
    // An empty rectangle covers no pixel, so it cannot share one. The classic interval
    // test below would answer "yes" for an empty rectangle whose origin falls inside the
    // other, and the callers clip before they get here, so this is a guard rather than a
    // case that occurs.
    if left.w == 0 || left.h == 0 || right.w == 0 || right.h == 0 {
        return false;
    }
    let left_right = i64::from(left.x) + i64::from(left.w);
    let left_bottom = i64::from(left.y) + i64::from(left.h);
    let right_right = i64::from(right.x) + i64::from(right.w);
    let right_bottom = i64::from(right.y) + i64::from(right.h);
    i64::from(left.x) < right_right
        && i64::from(right.x) < left_right
        && i64::from(left.y) < right_bottom
        && i64::from(right.y) < left_bottom
}

/// The bounding box of a rectangle list, empty for an empty list.
pub(super) fn union_rect(rects: &[RectI]) -> RectI {
    let Some((first, rest)) = rects.split_first() else {
        return RectI {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
    };
    rest.iter()
        .fold(*first, |bounds, rect| union_pair(bounds, *rect))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour with the given non-premultiplied channels, as the renderer would blend it.
    fn premultiplied(red: u8, green: u8, blue: u8, alpha: u8) -> PremultipliedRgbaColor {
        let scale = u16::from(alpha);
        PremultipliedRgbaColor {
            red: (u16::from(red) * scale / 255) as u8,
            green: (u16::from(green) * scale / 255) as u8,
            blue: (u16::from(blue) * scale / 255) as u8,
            alpha,
        }
    }

    #[test]
    fn test_argb8888_pixel_writes_the_surface_byte_order() {
        // The layout the compositor reads: one `0xAARRGGBB` word, so the bytes of a
        // half-transparent red are B=0, G=0, R=128, A=128 on a little-endian host.
        let half_red = Argb8888Pixel::pack(128, 0, 0, 128);
        assert_eq!(half_red.to_bytes(), [0, 0, 128, 128]);
        assert_eq!(
            Argb8888Pixel::from_rgb(255, 0, 0).to_bytes(),
            [0, 0, 255, 255]
        );
        assert_eq!(Argb8888Pixel::TRANSPARENT.to_bytes(), [0, 0, 0, 0]);
        assert_eq!(Argb8888Pixel::TRANSPARENT.alpha(), 0);
        // The channels survive a round trip through the word.
        let opaque_teal = Argb8888Pixel::from_rgb(0x12, 0x34, 0x56);
        assert_eq!(opaque_teal.channels(), (0x12, 0x34, 0x56, 0xff));
    }

    #[test]
    fn test_blend_matches_the_premultiplied_reference() {
        // Every case is checked against Slint's own `PremultipliedRgbaColor` arithmetic,
        // which is the definition of "source over destination" the renderer assumes.
        let cases = [
            (premultiplied(255, 0, 0, 255), (0u8, 0u8, 0u8, 0u8)),
            (premultiplied(0, 0, 255, 128), (0, 0, 0, 0)),
            (premultiplied(255, 255, 255, 128), (255, 0, 0, 255)),
            (premultiplied(0, 255, 0, 64), (10, 20, 30, 200)),
            (premultiplied(255, 255, 255, 255), (128, 128, 128, 255)),
        ];
        for (source, (red, green, blue, alpha)) in cases {
            let mut ours = Argb8888Pixel::pack(red, green, blue, alpha);
            let mut reference = PremultipliedRgbaColor {
                red,
                green,
                blue,
                alpha,
            };
            ours.blend(source);
            reference.blend(source);
            assert_eq!(
                ours.channels(),
                (
                    reference.red,
                    reference.green,
                    reference.blue,
                    reference.alpha
                ),
                "blending {source:?} over ({red}, {green}, {blue}, {alpha})"
            );
        }
    }

    #[test]
    fn test_blend_saturates_a_colour_that_breaks_the_premultiplied_invariant() {
        // `red = 255` with `alpha = 0` cannot come from a valid premultiplied colour, but
        // the renderer must not wrap if one ever reaches it.
        let mut pixel = Argb8888Pixel::from_rgb(255, 255, 255);
        pixel.blend(PremultipliedRgbaColor {
            red: 255,
            green: 255,
            blue: 255,
            alpha: 0,
        });
        assert_eq!(pixel, Argb8888Pixel::from_rgb(255, 255, 255));
    }

    #[test]
    fn test_background_is_transparent() {
        assert_eq!(Argb8888Pixel::background(), Argb8888Pixel::TRANSPARENT);
        assert_eq!(Argb8888Pixel::background().alpha(), 0);
    }

    #[test]
    fn test_ensure_grows_the_scratch_and_reports_it() {
        let mut scratch = PixelScratch::new();
        assert!(scratch.ensure(4, 2), "the first call allocates");
        assert_eq!(scratch.stride, 4 + STRIDE_SLACK);
        assert!(scratch.pixels.len() >= scratch.stride * (2 + ROW_SLACK));
        assert!(!scratch.ensure(4, 2), "the same size needs no reallocation");
        assert!(
            !scratch.ensure(2, 1),
            "a smaller surface fits the allocation"
        );
        assert!(scratch.ensure(64, 32), "a larger surface reallocates");
        assert_eq!(scratch.stride, 64 + STRIDE_SLACK);
        assert!(
            scratch
                .pixels
                .iter()
                .all(|p| *p == Argb8888Pixel::TRANSPARENT),
            "grown pixels start transparent"
        );
    }

    #[test]
    fn test_blit_copies_only_the_rectangle() {
        let mut scratch = PixelScratch::new();
        scratch.ensure(4, 3);
        let stride = scratch.stride;
        scratch.pixels[0] = Argb8888Pixel::from_rgb(1, 2, 3);
        scratch.pixels[stride + 1] = Argb8888Pixel::from_rgb(4, 5, 6);
        let mut destination = vec![0xeeu8; 4 * 4 * 3];
        scratch
            .blit_into(
                &mut destination,
                16,
                RectI {
                    x: 1,
                    y: 1,
                    w: 1,
                    h: 1,
                },
            )
            .expect("the destination covers the rectangle");
        assert_eq!(&destination[20..24], &[6, 5, 4, 255]);
        assert_eq!(
            destination[0], 0xee,
            "the pixel outside the rectangle is untouched"
        );
        assert_eq!(destination[16], 0xee);
        assert_eq!(destination[24], 0xee);
    }

    #[test]
    fn test_blit_refuses_a_destination_that_is_too_small() {
        let mut scratch = PixelScratch::new();
        scratch.ensure(8, 8);
        let mut destination = vec![0u8; 4 * 4];
        let result = scratch.blit_into(
            &mut destination,
            16,
            RectI {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
        );
        assert_eq!(result, Err(PlatformError::Unavailable));
    }

    #[test]
    fn test_clip_rect_clamps_and_drops_empty_rectangles() {
        let surface = (10u32, 10u32);
        let clipped = clip_rect(
            RectI {
                x: -5,
                y: -5,
                w: 10,
                h: 10,
            },
            surface.0,
            surface.1,
        );
        assert_eq!(
            clipped,
            Some(RectI {
                x: 0,
                y: 0,
                w: 5,
                h: 5
            })
        );
        let outside = clip_rect(
            RectI {
                x: 12,
                y: 0,
                w: 4,
                h: 4,
            },
            surface.0,
            surface.1,
        );
        assert_eq!(outside, None);
        let degenerate = clip_rect(
            RectI {
                x: 2,
                y: 2,
                w: 0,
                h: 4,
            },
            surface.0,
            surface.1,
        );
        assert_eq!(degenerate, None);
        // `i32::MIN` must not overflow the offset arithmetic.
        let extreme = clip_rect(
            RectI {
                x: i32::MIN,
                y: i32::MIN,
                w: u32::MAX,
                h: u32::MAX,
            },
            surface.0,
            surface.1,
        );
        assert_eq!(
            extreme,
            Some(RectI {
                x: 0,
                y: 0,
                w: 10,
                h: 10
            })
        );
    }

    #[test]
    fn test_union_rect_covers_every_rectangle() {
        assert_eq!(
            union_rect(&[]),
            RectI {
                x: 0,
                y: 0,
                w: 0,
                h: 0
            }
        );
        let rects = [
            RectI {
                x: 4,
                y: 8,
                w: 2,
                h: 2,
            },
            RectI {
                x: 1,
                y: 2,
                w: 3,
                h: 3,
            },
            RectI {
                x: 10,
                y: 0,
                w: 1,
                h: 20,
            },
        ];
        // The bounding box of the three: leftmost edge 1, topmost edge 0, and the far
        // edges come from the third rectangle (right 11, bottom 20).
        assert_eq!(
            union_rect(&rects),
            RectI {
                x: 1,
                y: 0,
                w: 10,
                h: 20,
            }
        );
    }

    #[test]
    fn test_union_pair_covers_both_rectangles() {
        let left = RectI {
            x: 4,
            y: 8,
            w: 2,
            h: 2,
        };
        let right = RectI {
            x: 1,
            y: 2,
            w: 3,
            h: 3,
        };
        assert_eq!(
            union_pair(left, right),
            RectI {
                x: 1,
                y: 2,
                w: 5,
                h: 8
            }
        );
        // Commutative, and a rectangle is its own bounding box.
        assert_eq!(union_pair(right, left), union_pair(left, right));
        assert_eq!(union_pair(left, left), left);
        // A rectangle far to the negative side must not overflow the extent arithmetic.
        assert_eq!(
            union_pair(
                RectI {
                    x: i32::MIN,
                    y: i32::MIN,
                    w: 0,
                    h: 0
                },
                RectI {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1
                }
            ),
            RectI {
                x: i32::MIN,
                y: i32::MIN,
                w: 2_147_483_649,
                h: 2_147_483_649
            }
        );
    }

    #[test]
    fn test_union_rect_folds_with_union_pair() {
        // The fold and the list form must agree, which is what keeps the two definitions of
        // "bounding box" from drifting apart.
        let rects = [
            RectI {
                x: 7,
                y: 3,
                w: 4,
                h: 4,
            },
            RectI {
                x: 0,
                y: 9,
                w: 2,
                h: 2,
            },
            RectI {
                x: 12,
                y: 0,
                w: 1,
                h: 1,
            },
        ];
        let folded = rects
            .iter()
            .fold(rects[0], |bounds, rect| union_pair(bounds, *rect));
        assert_eq!(union_rect(&rects), folded);
    }
}
