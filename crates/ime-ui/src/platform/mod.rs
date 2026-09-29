//! Platform window backends.
//!
//! One module per backend, each implementing the frozen
//! [`SurfaceBackend`](ime_types::surface::SurfaceBackend) trait: the X11 backend here,
//! and the Wayland tiers beside it. The trait is what keeps the renderer free of
//! platform detail, so every backend hands out the same premultiplied `Argb8888` buffer
//! and speaks the same event vocabulary.
//!
//! Every backend obeys the same three rules, and a backend that breaks one of them is
//! broken rather than merely imperfect:
//!
//! * **It never takes keyboard focus.** No `SetInputFocus`, no `grab_keyboard`, no
//!   `keyboard_interactivity` above `none`. A candidate window that steals focus is the
//!   project's highest-severity defect.
//! * **It never blocks.** Every call returns within itself; nothing waits on the
//!   compositor, on a lock or on a clock. A frame that cannot be delivered is skipped,
//!   never waited for.
//! * **It never rasterizes.** A backend owns a surface and its pixels; what goes into
//!   those pixels is decided elsewhere.
//!
//! # The display-free half
//!
//! This module holds the part of the platform layer that needs no display server: size
//! conversion, buffer sizing, region clipping and pixel-format repacking are the same
//! problem on every backend, and the X11 event and geometry translation is pure as well.
//! Keeping it here is what lets the test suite below cover the whole layer -- including
//! the parts of [`x11`] that would otherwise need a live X session -- in an ordinary
//! `cargo nextest` run. The backend module keeps everything that does need a connection:
//! the window, its buffers, and the trait implementation.

use ime_types::RectI;
use x11rb::protocol::xproto::Rectangle;

pub mod x11;

/// Largest surface dimension any backend will create, in physical pixels.
///
/// A candidate window is a few hundred pixels across, so a size far beyond this is a
/// caller bug; clamping keeps a bogus request from allocating gigabytes of buffers while
/// staying far above any window that can be displayed.
pub(crate) const MAX_DIMENSION: u32 = 8192;

/// Bytes per pixel of the `Argb8888` buffer format the contract fixes.
pub(crate) const BYTES_PER_PIXEL: u32 = 4;

/// Corrects a scale factor that cannot be used.
///
/// The scale comes from configuration, so a zero, negative, NaN or infinite value is
/// corrected to `1.0` here rather than rejected: a window at the wrong size is still
/// usable, while a backend that refuses to start is not.
pub(crate) fn normalize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Converts a logical size into the physical pixel size of the surface.
pub(crate) fn physical_size(width_dp: u32, height_dp: u32, scale: f32) -> (u32, u32) {
    (
        physical_dimension(width_dp, scale),
        physical_dimension(height_dp, scale),
    )
}

/// Scales one logical dimension and clamps it into the range a surface can have.
pub(crate) fn physical_dimension(dp: u32, scale: f32) -> u32 {
    let px = (dp as f32 * scale).round();
    let px = if px.is_finite() { px } else { 1.0 };
    px.clamp(1.0, MAX_DIMENSION as f32) as u32
}

/// Converts a physical dimension back into logical pixels.
pub(crate) fn logical_dimension(px: u32, scale: f32) -> u32 {
    let dp = (px as f32 / scale).round();
    let dp = if dp.is_finite() { dp } else { 1.0 };
    dp.clamp(1.0, MAX_DIMENSION as f32) as u32
}

/// Clamps a physical dimension into the range a surface can have.
pub(crate) fn clamp_dimension(px: u32) -> u32 {
    px.clamp(1, MAX_DIMENSION)
}

/// Byte length of one `Argb8888` draw buffer.
pub(crate) fn buffer_len(width_px: u32, height_px: u32) -> usize {
    width_px as usize * BYTES_PER_PIXEL as usize * height_px as usize
}

/// Clips an interactive region to the surface and drops whatever is left empty.
///
/// The shadow reserve around the candidate container is the caller's business: whatever
/// it leaves out of `rects` is what the pointer falls through to the application
/// underneath, which is why an empty set has to stay empty rather than defaulting to the
/// whole surface.
pub(crate) fn clip_rects(rects: &[RectI], width_px: u32, height_px: u32) -> Vec<RectI> {
    let mut clipped = Vec::with_capacity(rects.len());
    for rect in rects {
        // Widened to i64: a rectangle may carry i32::MIN and would overflow when offset
        // by the surface origin.
        let x0 = i64::from(rect.x).max(0);
        let y0 = i64::from(rect.y).max(0);
        let x1 = (i64::from(rect.x) + i64::from(rect.w)).min(i64::from(width_px));
        let y1 = (i64::from(rect.y) + i64::from(rect.h)).min(i64::from(height_px));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        clipped.push(RectI {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        });
    }
    clipped
}

/// Converts a clipped interactive region into the protocol's rectangle type.
///
/// The casts cannot truncate: the region was clipped to a surface of at most
/// [`MAX_DIMENSION`] pixels.
pub(crate) fn x11_rectangles(rects: &[RectI]) -> Vec<Rectangle> {
    rects
        .iter()
        .map(|rect| Rectangle {
            x: rect.x as i16,
            y: rect.y as i16,
            width: rect.w as u16,
            height: rect.h as u16,
        })
        .collect()
}

/// Bytes per pixel of a 24-bit surface.
pub(crate) const RGB24_BYTES_PER_PIXEL: u32 = 3;

/// Bytes per row of a 24-bit surface: three bytes per pixel, rounded up to a 32-bit
/// boundary, which is the row length a `ZPixmap` upload is read with.
pub(crate) fn packed_stride(width_px: u32) -> usize {
    (width_px as usize * RGB24_BYTES_PER_PIXEL as usize).next_multiple_of(4)
}

/// Allocates the repacking scratch for a 24-bit surface.
pub(crate) fn repack_scratch(width_px: u32, height_px: u32) -> Vec<u8> {
    vec![0; packed_stride(width_px) * height_px as usize]
}

/// Repacks an `Argb8888` buffer into the 24-bit layout a surface without alpha expects.
///
/// The alpha byte is dropped and every row is padded to [`packed_stride`]. The padding
/// stays as the scratch was allocated -- zero -- because only the first three bytes of
/// each pixel are written. Those three bytes are copied in memory order, which is the
/// order a server expects from a client whose byte order is little-endian: the only order
/// this project ships on.
pub(crate) fn pack_rgb24(src: &[u8], dst: &mut [u8], width_px: u32, height_px: u32) {
    let src_stride = width_px as usize * BYTES_PER_PIXEL as usize;
    let dst_stride = packed_stride(width_px);
    let rows = src
        .chunks(src_stride.max(1))
        .zip(dst.chunks_mut(dst_stride.max(1)));
    for (src_row, dst_row) in rows.take(height_px as usize) {
        let pixels = src_row
            .chunks_exact(BYTES_PER_PIXEL as usize)
            .zip(dst_row.chunks_exact_mut(RGB24_BYTES_PER_PIXEL as usize));
        for (pixel, out) in pixels {
            out.copy_from_slice(&pixel[..RGB24_BYTES_PER_PIXEL as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::x11::{
        Decoded, OPAQUE_ALPHA, X11Backend, classify_event, effective_alpha, scroll_axis,
        select_argb_visual,
    };
    use ime_types::{FrameToken, PixelBufferMut, PlatformError, SurfaceBackend, SurfaceEvent};
    use x11rb::protocol::ErrorKind;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        BackingStore, ButtonPressEvent, ConfigureNotifyEvent, Depth, EnterNotifyEvent, EventMask,
        ExposeEvent, KeyButMask, Motion, MotionNotifyEvent, NotifyDetail, NotifyMode, Screen,
        VisualClass, Visualtype,
    };
    use x11rb::x11_utils::X11Error;

    /// A window size that fits any display a test might run against.
    const TEST_WIDTH_DP: u32 = 320;
    const TEST_HEIGHT_DP: u32 = 80;

    #[test]
    fn test_physical_size_scales_logical_pixels() {
        assert_eq!(physical_size(600, 140, 2.0), (1200, 280));
        assert_eq!(physical_size(601, 140, 1.25), (751, 175));
        assert_eq!(physical_size(600, 140, 1.0), (600, 140));
    }

    #[test]
    fn test_scale_helpers_of_degenerate_input_stay_usable() {
        assert_eq!(physical_dimension(0, 1.0), 1);
        assert_eq!(physical_dimension(u32::MAX, 1.0), MAX_DIMENSION);
        assert_eq!(logical_dimension(1200, 2.0), 600);
        assert_eq!(clamp_dimension(0), 1);
        assert_eq!(clamp_dimension(u32::MAX), MAX_DIMENSION);
        // A scale that cannot be used is replaced, not propagated.
        let fallback = 1.0f32.to_bits();
        assert_eq!(normalize_scale(0.0).to_bits(), fallback);
        assert_eq!(normalize_scale(-2.0).to_bits(), fallback);
        assert_eq!(normalize_scale(f32::NAN).to_bits(), fallback);
        assert_eq!(normalize_scale(f32::INFINITY).to_bits(), fallback);
        assert_eq!(normalize_scale(1.5).to_bits(), 1.5f32.to_bits());
    }

    #[test]
    fn test_buffer_len_covers_a_whole_frame() {
        assert_eq!(buffer_len(2, 3), 24);
        assert_eq!(buffer_len(1200, 280), 1_344_000);
    }

    #[test]
    fn test_clip_rects_clips_outside_and_drops_degenerate_rects() {
        let rects = [
            RectI {
                x: -5,
                y: -5,
                w: 10,
                h: 10,
            },
            RectI {
                x: 12,
                y: 12,
                w: 4,
                h: 4,
            },
            RectI {
                x: 2,
                y: 2,
                w: 0,
                h: 4,
            },
            RectI {
                x: 4,
                y: 4,
                w: 3,
                h: 3,
            },
        ];
        let clipped = clip_rects(&rects, 10, 10);
        assert_eq!(clipped.len(), 2, "only the two visible rectangles survive");
        assert_eq!(
            (clipped[0].x, clipped[0].y, clipped[0].w, clipped[0].h),
            (0, 0, 5, 5)
        );
        assert_eq!(
            (clipped[1].x, clipped[1].y, clipped[1].w, clipped[1].h),
            (4, 4, 3, 3)
        );
    }

    #[test]
    fn test_clip_rects_empty_set_stays_empty() {
        // An empty interactive region is how the caller makes the surface click-through,
        // so it must never be widened back to the whole surface.
        assert!(clip_rects(&[], 100, 50).is_empty());
    }

    #[test]
    fn test_pack_rgb24_drops_alpha_and_pads_rows() {
        // One row of two premultiplied ARGB pixels, as the little-endian bytes B, G, R, A.
        let src = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
        let mut dst = [0xff; 8];
        pack_rgb24(&src, &mut dst, 2, 1);
        assert_eq!(dst, [0x10, 0x20, 0x30, 0x50, 0x60, 0x70, 0xff, 0xff]);
        assert_eq!(packed_stride(2), 8, "pads to a 32-bit boundary");
        assert_eq!(packed_stride(4), 12, "twelve bytes need no padding");
        assert_eq!(repack_scratch(4, 2).len(), 24);
    }

    /// A visual with the given channel masks.
    fn visual(visual_id: u32, class: VisualClass, red: u32, green: u32, blue: u32) -> Visualtype {
        Visualtype {
            visual_id,
            class,
            bits_per_rgb_value: 8,
            colormap_entries: 256,
            red_mask: red,
            green_mask: green,
            blue_mask: blue,
        }
    }

    /// The standard 32-bit ARGB visual every X server offers.
    fn argb_visual(visual_id: u32) -> Visualtype {
        visual(
            visual_id,
            VisualClass::TRUE_COLOR,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff,
        )
    }

    /// A screen whose only interesting part is its depth list.
    fn screen(depths: Vec<Depth>) -> Screen {
        Screen {
            root: 0x1234,
            default_colormap: 0x20,
            white_pixel: 0xffff_ffff,
            black_pixel: 0,
            current_input_masks: EventMask::default(),
            width_in_pixels: 1920,
            height_in_pixels: 1080,
            width_in_millimeters: 500,
            height_in_millimeters: 280,
            min_installed_maps: 1,
            max_installed_maps: 1,
            root_visual: 0x21,
            backing_stores: BackingStore::default(),
            save_unders: false,
            root_depth: 24,
            allowed_depths: depths,
        }
    }

    /// A button event; `ButtonRelease` shares this struct.
    fn button_event(detail: u8, x: i16, y: i16) -> ButtonPressEvent {
        ButtonPressEvent {
            response_type: 4,
            detail,
            sequence: 0,
            time: 0,
            root: 0x1234,
            event: 0x1234,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: x,
            event_y: y,
            state: KeyButMask::default(),
            same_screen: true,
        }
    }

    /// A crossing event; `LeaveNotify` shares this struct.
    fn crossing_event(x: i16, y: i16) -> EnterNotifyEvent {
        EnterNotifyEvent {
            response_type: 7,
            detail: NotifyDetail::default(),
            sequence: 0,
            time: 0,
            root: 0x1234,
            event: 0x1234,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: x,
            event_y: y,
            state: KeyButMask::default(),
            mode: NotifyMode::default(),
            same_screen_focus: 0,
        }
    }

    #[test]
    fn test_select_argb_visual_prefers_matching_32_bit_true_color() {
        let depths = vec![
            Depth {
                depth: 24,
                visuals: vec![visual(
                    0x21,
                    VisualClass::TRUE_COLOR,
                    0xff_0000,
                    0xff00,
                    0xff,
                )],
            },
            Depth {
                depth: 32,
                visuals: vec![argb_visual(0x40)],
            },
        ];
        assert_eq!(select_argb_visual(&screen(depths)), Some((0x40, 32)));
    }

    #[test]
    fn test_select_argb_visual_without_usable_32_bit_visual_returns_none() {
        let swapped = vec![Depth {
            depth: 32,
            visuals: vec![visual(
                0x41,
                VisualClass::TRUE_COLOR,
                0xff,
                0xff00,
                0xff_0000,
            )],
        }];
        assert_eq!(select_argb_visual(&screen(swapped)), None);
        let gray = vec![Depth {
            depth: 32,
            visuals: vec![visual(0x42, VisualClass::STATIC_GRAY, 0, 0, 0)],
        }];
        assert_eq!(select_argb_visual(&screen(gray)), None);
        let only_24 = vec![Depth {
            depth: 24,
            visuals: vec![argb_visual(0x21)],
        }];
        assert_eq!(select_argb_visual(&screen(only_24)), None);
    }

    #[test]
    fn test_scroll_axis_normalises_wheel_direction() {
        let cases = [
            (4, -1, false),
            (5, 1, false),
            (6, -1, true),
            (7, 1, true),
            (8, -1, true),
            (9, 1, true),
        ];
        for (button, delta, horizontal) in cases {
            let expected = SurfaceEvent::Axis {
                x: 3,
                y: 4,
                delta,
                horizontal,
            };
            assert_eq!(scroll_axis(button, 3, 4), Some(expected), "button {button}");
        }
        assert_eq!(scroll_axis(1, 3, 4), None, "a real button is not a wheel");
        assert_eq!(scroll_axis(0, 3, 4), None);
    }

    #[test]
    fn test_classify_pointer_buttons_and_wheel() {
        let press = Event::ButtonPress(button_event(1, 7, 9));
        let expected = Decoded::Surface(SurfaceEvent::PointerButton {
            x: 7,
            y: 9,
            button: 1,
            pressed: true,
        });
        assert_eq!(classify_event(&press, 100, 50), expected);
        let release = Event::ButtonRelease(button_event(1, 7, 9));
        let expected = Decoded::Surface(SurfaceEvent::PointerButton {
            x: 7,
            y: 9,
            button: 1,
            pressed: false,
        });
        assert_eq!(classify_event(&release, 100, 50), expected);
        let wheel = Event::ButtonPress(button_event(5, 7, 9));
        let expected = Decoded::Surface(SurfaceEvent::Axis {
            x: 7,
            y: 9,
            delta: 1,
            horizontal: false,
        });
        assert_eq!(classify_event(&wheel, 100, 50), expected);
        // A wheel release must not become a second scroll step.
        let wheel_release = Event::ButtonRelease(button_event(5, 7, 9));
        assert_eq!(classify_event(&wheel_release, 100, 50), Decoded::Ignored);
    }

    #[test]
    fn test_classify_motion_and_crossing_events() {
        let motion = Event::MotionNotify(MotionNotifyEvent {
            response_type: 6,
            detail: Motion::default(),
            sequence: 0,
            time: 0,
            root: 0x1234,
            event: 0x1234,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: 1,
            event_y: 2,
            state: KeyButMask::default(),
            same_screen: true,
        });
        assert_eq!(
            classify_event(&motion, 100, 50),
            Decoded::Surface(SurfaceEvent::PointerMotion { x: 1, y: 2 })
        );
        let enter = Event::EnterNotify(crossing_event(5, 6));
        assert_eq!(
            classify_event(&enter, 100, 50),
            Decoded::Surface(SurfaceEvent::PointerEnter { x: 5, y: 6 })
        );
        let leave = Event::LeaveNotify(crossing_event(0, 0));
        assert_eq!(
            classify_event(&leave, 100, 50),
            Decoded::Surface(SurfaceEvent::PointerLeave)
        );
    }

    #[test]
    fn test_classify_expose_configure_and_protocol_errors() {
        let expose = Event::Expose(ExposeEvent {
            response_type: 12,
            sequence: 0,
            window: 0x1234,
            x: 0,
            y: 0,
            width: 100,
            height: 50,
            count: 0,
        });
        assert_eq!(classify_event(&expose, 100, 50), Decoded::Repaint);
        let resized = Event::ConfigureNotify(ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: 0x1234,
            window: 0x1234,
            above_sibling: 0,
            x: 0,
            y: 0,
            width: 200,
            height: 60,
            border_width: 0,
            override_redirect: true,
        });
        assert_eq!(
            classify_event(&resized, 100, 50),
            Decoded::Resized {
                width_px: 200,
                height_px: 60
            }
        );
        // The echo of our own configure request is not a resize.
        assert_eq!(classify_event(&resized, 200, 60), Decoded::Ignored);
        let error = Event::Error(X11Error {
            error_kind: ErrorKind::Match,
            error_code: 8,
            sequence: 0,
            bad_value: 0,
            minor_opcode: 0,
            major_opcode: 1,
            extension_name: None,
            request_name: None,
        });
        assert_eq!(classify_event(&error, 100, 50), Decoded::ProtocolError);
        // An event this backend never selected is ignored rather than misread.
        let unknown = Event::Unknown(Vec::new());
        assert_eq!(classify_event(&unknown, 100, 50), Decoded::Ignored);
    }

    #[test]
    fn test_effective_alpha_requires_argb_and_compositor() {
        assert_eq!(effective_alpha(217, true, true), 217);
        assert_eq!(effective_alpha(217, true, false), OPAQUE_ALPHA);
        assert_eq!(effective_alpha(217, false, true), OPAQUE_ALPHA);
        assert_eq!(effective_alpha(217, false, false), OPAQUE_ALPHA);
    }

    #[test]
    fn test_x11_rectangles_keeps_the_clipped_region() {
        let rects = [
            RectI {
                x: 0,
                y: 0,
                w: 5,
                h: 5,
            },
            RectI {
                x: 4,
                y: 4,
                w: 3,
                h: 3,
            },
        ];
        let rectangles = x11_rectangles(&rects);
        assert_eq!(rectangles.len(), 2);
        assert_eq!((rectangles[0].x, rectangles[0].y), (0, 0));
        assert_eq!((rectangles[1].width, rectangles[1].height), (3, 3));
    }

    /// A display-free stand-in for the backend contract.
    ///
    /// It exists to prove that the call sequence below is a sequence the trait allows,
    /// independently of any display server.
    struct MockBackend {
        pixels: Vec<u8>,
        geometry: (u32, u32, f32),
        visible: bool,
        commits: usize,
        region: usize,
    }

    impl MockBackend {
        fn new(width: u32, height: u32) -> Self {
            Self {
                pixels: vec![0; buffer_len(width, height)],
                geometry: (width, height, 1.0),
                visible: false,
                commits: 0,
                region: 0,
            }
        }
    }

    impl SurfaceBackend for MockBackend {
        fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
            let stride = self.geometry.0 as usize * BYTES_PER_PIXEL as usize;
            Ok(PixelBufferMut {
                data: &mut self.pixels,
                stride,
                width: self.geometry.0,
                height: self.geometry.1,
            })
        }

        fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
            self.commits += damage.len();
            Ok(())
        }

        fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
            self.region = rects.len();
            Ok(())
        }

        fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
            self.visible = visible;
            Ok(())
        }

        fn request_frame(&mut self) -> Option<FrameToken> {
            Some(FrameToken(1))
        }

        fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
            out.push(SurfaceEvent::PointerLeave);
            Ok(())
        }

        fn geometry(&self) -> (u32, u32, f32) {
            self.geometry
        }

        fn backend_id(&self) -> &'static str {
            "mock"
        }
    }

    /// The call sequence the UI thread performs on whichever backend it owns.
    ///
    /// Everything the trait promises about a buffer is asserted here, so the mock and the
    /// real backend are held to the same contract.
    fn exercise(backend: &mut dyn SurfaceBackend) -> Result<(), PlatformError> {
        backend.set_visible(true)?;
        let damaged = RectI {
            x: 4,
            y: 4,
            w: 40,
            h: 12,
        };
        backend.set_input_region(&[damaged])?;
        {
            let buffer = backend.acquire_buffer()?;
            let row = buffer.width as usize * BYTES_PER_PIXEL as usize;
            let needed = row * buffer.height as usize;
            assert!(buffer.data.len() >= needed, "a buffer holds a whole frame");
            assert!(buffer.stride >= row);
            buffer.data[..4].copy_from_slice(&[0x40, 0x20, 0x10, 0x80]);
        }
        backend.commit(&[damaged])?;
        let _ = backend.request_frame();
        let mut events = Vec::new();
        backend.poll_events(&mut events)?;
        let (width, height, scale) = backend.geometry();
        assert!(width > 0 && height > 0 && scale > 0.0);
        backend.set_input_region(&[])?;
        backend.set_visible(false)?;
        Ok(())
    }

    #[test]
    fn test_backend_call_sequence_matches_the_mock_backend() {
        let mut mock = MockBackend::new(TEST_WIDTH_DP, TEST_HEIGHT_DP);
        exercise(&mut mock).expect("the mock backend accepts the sequence");
        assert_eq!(mock.commits, 1);
        assert_eq!(mock.region, 0);
        assert!(!mock.visible);
    }

    #[test]
    #[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
    fn test_x11_backend_serves_the_same_call_sequence() {
        let mut backend = X11Backend::connect(TEST_WIDTH_DP, TEST_HEIGHT_DP, 1.0, None)
            .expect("a live X server reachable through DISPLAY");
        assert_eq!(backend.backend_id(), "x11");
        assert!(
            backend.request_frame().is_none(),
            "X11 has no frame callbacks"
        );
        exercise(&mut backend).expect("the X11 backend accepts the sequence");
    }

    #[test]
    #[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
    fn test_x11_backend_never_takes_input_focus() {
        let mut backend = X11Backend::connect(TEST_WIDTH_DP, TEST_HEIGHT_DP, 1.0, None)
            .expect("a live X server reachable through DISPLAY");
        let before = backend.input_focus().expect("the focus can be queried");
        backend.set_visible(true).expect("the window can be shown");
        {
            let buffer = backend.acquire_buffer().expect("a buffer is always free");
            buffer.data.fill(0xff);
        }
        backend
            .commit(&[RectI {
                x: 0,
                y: 0,
                w: TEST_WIDTH_DP,
                h: TEST_HEIGHT_DP,
            }])
            .expect("the frame is committed");
        let mut events = Vec::new();
        backend
            .poll_events(&mut events)
            .expect("polling does not fail");
        let after = backend.input_focus().expect("the focus can be queried");
        backend
            .set_visible(false)
            .expect("the window can be hidden");
        assert_eq!(
            before, after,
            "showing the candidate window must not move the input focus"
        );
    }
}
