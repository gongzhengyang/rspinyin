//! Tests for the screenshot channel.
//!
//! # Two groups
//!
//! The first group needs no display server: the frame arithmetic and its refusals, the byte
//! order, the read loop's retry and its verdict, the snapshot naming rule, and the PNG
//! round trip -- a file is written and read back, which is the acceptance criterion that the
//! ratio travels inside the image. It also drives the bytes a server sends for a red window
//! through the whole pixel path, so the criterion that a capture carries the colour the
//! server painted is checked here as well; only the server's own end of that path needs one.
//!
//! The second group drives a live X server and is marked `#[ignore]`. It is where a window
//! the server was told to paint red is captured end to end and where a capture is timed
//! against the card's budget, and the lab job runs it with a display present:
//!
//! ```text
//! DISPLAY=:0 cargo nextest run -p xtask --run-ignored all
//! ```
//!
//! # What the live group cannot say
//!
//! The screen it captures is this machine's, so the 4K-at-2x timing the card names is not
//! measurable here: the plan for such a frame is asserted offline, and the live test
//! measures whatever screen the run has. A capture of the candidate window is likewise out
//! of reach until that window exists; the window under test here is one this file creates.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use ime_types::RectI;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, CreateWindowAux, WindowClass};
use x11rb::rust_connection::RustConnection;

use super::pixels::{PixelFormat, to_rgba};
use super::plan::{ChunkRange, FramePlan, MAX_FRAME_BYTES, MAX_SIDE_PX, check_region};
use super::source::{FrameSource, read_stable};
use super::*;
use crate::testd::coords::normalize_scale;
use crate::testd::x11::{Window, X11Session};

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-capture-<tag>-<pid>`, empty.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-capture-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }

    /// A path inside the scratch directory whose parent does not exist yet.
    fn file(&self, name: &str) -> PathBuf {
        self.path.join("RUN").join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A drawable that answers from memory, so the read loop runs without a server.
///
/// Each row is filled with a byte derived from its own index, which is what makes a
/// misassembled frame visible: the tests below compare the frame against the pattern the
/// rows were supposed to produce, not merely against its length.
struct DoubleSource {
    /// The target the loop is reading, echoed back in a refusal.
    target: CaptureTarget,
    /// Footprints to report, in order; the last one repeats once the list runs out.
    footprints: Vec<Footprint>,
    /// How many footprints have been asked for.
    asked: Cell<usize>,
    /// Whether a read answers with fewer bytes than the chunk asked for.
    short: bool,
}

impl DoubleSource {
    /// A drawable that stays where `footprints` says.
    fn staying(target: CaptureTarget, footprints: Vec<Footprint>) -> Self {
        Self {
            target,
            footprints,
            asked: Cell::new(0),
            short: false,
        }
    }
}

impl FrameSource for DoubleSource {
    fn target(&self) -> CaptureTarget {
        self.target
    }

    fn footprint(&self) -> Result<Footprint, CaptureError> {
        let asked = self.asked.get();
        self.asked.set(asked.saturating_add(1));
        let index = asked.min(self.footprints.len().saturating_sub(1));
        let footprint = self
            .footprints
            .get(index)
            .expect("a double is built with at least one footprint");
        Ok(*footprint)
    }

    fn read_rows(
        &self,
        chunk: ChunkRange,
        row_bytes: usize,
        out: &mut [u8],
    ) -> Result<(), CaptureError> {
        if self.short {
            return Err(CaptureError::ShortRead {
                expected: out.len(),
                actual: 0,
            });
        }
        for (index, row) in out.chunks_mut(row_bytes).enumerate() {
            let value = (chunk.y as u8).wrapping_add(index as u8);
            row.fill(value);
        }
        Ok(())
    }
}

/// The frame `plan` describes, filled the way [`DoubleSource`] fills it.
fn expected_frame(plan: &FramePlan) -> Vec<u8> {
    let mut frame = vec![0u8; plan.buffer_len()];
    for chunk in plan.chunks() {
        let slice = plan
            .chunk_slice(&mut frame, *chunk)
            .expect("the plan's own chunks fit its own frame");
        for (index, row) in slice.chunks_mut(plan.row_bytes()).enumerate() {
            row.fill((chunk.y as u8).wrapping_add(index as u8));
        }
    }
    frame
}

/// The RGBA pixels of a PNG, decoded.
fn decode_rgba(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = File::open(path).expect("the capture opens");
    let mut reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .expect("the capture is a PNG this build reads");
    let mut buffer = vec![0u8; reader.output_buffer_size().expect("a frame size")];
    let info = reader.next_frame(&mut buffer).expect("the frame decodes");
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "a capture is written as RGBA"
    );
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

/// One pixel of a decoded frame.
fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let at = ((y * width + x) * 4) as usize;
    (frame[at], frame[at + 1], frame[at + 2], frame[at + 3])
}

/// A window filled with one colour by the server itself.
///
/// The background pixel is what the server paints a newly mapped window with, so the colour
/// is exact without the harness drawing anything: the test is about the capture path, not
/// about a drawing path that would have to be trusted first.
fn red_window(conn: &RustConnection, width: u16, height: u16) -> Window {
    let (root, depth, visual) = {
        let setup = conn.setup();
        let screen = setup.roots.first().expect("the server lists a screen");
        (screen.root, screen.root_depth, screen.root_visual)
    };
    let window = conn.generate_id().expect("a free window id");
    conn.create_window(
        depth,
        window,
        root,
        100,
        100,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux {
            background_pixel: Some(RED_PIXEL),
            // Not managed by a window manager: nothing has to place it for the capture to
            // find it where the geometry says it is.
            override_redirect: Some(1),
            ..Default::default()
        },
    )
    .expect("the window is created");
    conn.map_window(window).expect("the window is mapped");
    // The server has to have processed the mapping before its pixels can be read; a round
    // trip through the queue is what makes that certain.
    conn.get_input_focus()
        .expect("the query is queued")
        .reply()
        .expect("the server answers");
    window
}

/// The pixel the test window is filled with: opaque red, in the root visual's own layout.
const RED_PIXEL: u32 = 0x00ff_0000;

/// Size of the test window the live capture reads.
const RED_WINDOW_WIDTH: u16 = 64;

/// Height of the test window the live capture reads.
const RED_WINDOW_HEIGHT: u16 = 48;

#[test]
fn test_frame_plan_chunks_a_4k_frame_into_ranges_of_at_most_2048_rows() {
    let plan = FramePlan::new(7680, 4320, PixelFormat::BGRA8888).expect("4K at scale 2.0 fits");
    assert_eq!(plan.width(), 7680);
    assert_eq!(plan.height(), 4320);
    assert_eq!(plan.row_bytes(), 7680 * 4, "one row is width times four");
    assert_eq!(
        plan.buffer_len(),
        7680 * 4 * 4320,
        "the frame is the whole image, not a chunk of it"
    );
    let ranges: Vec<(u32, u32)> = plan.chunks().iter().map(|c| (c.y, c.rows)).collect();
    assert_eq!(
        ranges,
        vec![(0, 2048), (2048, 2048), (4096, 224)],
        "4K at scale 2.0 is read in three ranges"
    );
    let rows: u32 = plan.chunks().iter().map(|chunk| chunk.rows).sum();
    assert_eq!(
        rows,
        plan.height(),
        "the ranges cover every row exactly once"
    );
    assert!(
        plan.buffer_len() as u64 <= MAX_FRAME_BYTES,
        "the card's 4K case is inside the memory ceiling"
    );
}

#[test]
fn test_frame_plan_covers_every_row_for_any_height() {
    for height in [1_u32, 2047, 2048, 2049, 4096, 4097, 4320, 5000] {
        let plan = FramePlan::new(16, height, PixelFormat::BGRA8888).expect("a small frame fits");
        let mut expected = 0;
        for chunk in plan.chunks() {
            assert_eq!(chunk.y, expected, "the ranges are contiguous and in order");
            assert!(chunk.rows > 0 && chunk.rows <= super::CHUNK_ROWS);
            expected += chunk.rows;
        }
        assert_eq!(expected, height, "height {height} is covered exactly once");
    }
}

#[test]
fn test_frame_plan_refuses_a_frame_past_the_ceiling_and_names_region() {
    let too_wide = FramePlan::new(MAX_SIDE_PX + 1, 1, PixelFormat::BGRA8888)
        .expect_err("a side past the request's field is refused");
    match &too_wide {
        CaptureError::TooLarge { width, .. } => assert_eq!(*width, MAX_SIDE_PX + 1),
        other => panic!("a side the request cannot address must be refused: {other:?}"),
    }
    let too_big = FramePlan::new(32767, 32767, PixelFormat::BGRA8888)
        .expect_err("a frame past the memory ceiling is refused");
    match &too_big {
        CaptureError::TooLarge { bytes, limit, .. } => {
            assert!(
                *bytes > *limit,
                "{bytes} must be past the {limit} byte ceiling"
            );
            assert_eq!(*limit, MAX_FRAME_BYTES);
        }
        other => panic!("a frame past the ceiling must be refused: {other:?}"),
    }
    let message = too_big.to_string();
    assert!(
        message.contains("Region"),
        "the refusal has to name the way out: {message}"
    );
}

#[test]
fn test_frame_plan_refuses_a_target_with_no_pixels() {
    for (width, height) in [(0, 0), (0, 10), (10, 0)] {
        assert_eq!(
            FramePlan::new(width, height, PixelFormat::BGRA8888),
            Err(CaptureError::EmptyCapture { width, height }),
            "an unmapped window reports an empty geometry"
        );
    }
}

#[test]
fn test_chunk_slices_reassemble_the_frame_a_single_read_would_have_filled() {
    let plan = FramePlan::new(4, 5000, PixelFormat::BGRA8888).expect("a narrow tall frame fits");
    let mut frame = vec![0u8; plan.buffer_len()];
    let source = DoubleSource::staying(CaptureTarget::FullScreen, vec![footprint(0, 0, 4, 5000)]);
    read_stable(&source, &plan, &mut frame).expect("the drawable stayed put");
    assert_eq!(
        frame,
        expected_frame(&plan),
        "the ranges land in the frame exactly where one whole read would have put them"
    );
}

#[test]
fn test_chunk_slice_refuses_a_frame_that_is_not_the_buffer_it_describes() {
    let plan = FramePlan::new(4, 4096, PixelFormat::BGRA8888).expect("a narrow tall frame fits");
    let chunk = *plan
        .chunks()
        .first()
        .expect("a frame has at least one range");
    let mut short = vec![0u8; plan.row_bytes()];
    assert!(
        plan.chunk_slice(&mut short, chunk).is_none(),
        "a frame shorter than the plan is refused rather than sliced past its end"
    );
}

#[test]
fn test_check_region_accepts_the_screen_and_refuses_what_leaves_it() {
    let screen = (1920, 1080);
    let region = |x, y, w, h| RectI { x, y, w, h };
    assert!(check_region(region(0, 0, 1920, 1080), screen).is_ok());
    assert!(
        check_region(region(1880, 1040, 40, 40), screen).is_ok(),
        "a region ending on the far edge is on the screen"
    );
    assert!(matches!(
        check_region(region(1900, 0, 40, 10), screen),
        Err(CaptureError::RegionOffScreen { .. })
    ));
    assert!(matches!(
        check_region(region(-1, 0, 10, 10), screen),
        Err(CaptureError::RegionOffScreen { .. })
    ));
    assert!(matches!(
        check_region(region(0, 1080, 10, 10), screen),
        Err(CaptureError::RegionOffScreen { .. })
    ));
    assert_eq!(
        check_region(region(0, 0, 0, 10), screen),
        Err(CaptureError::EmptyCapture {
            width: 0,
            height: 10
        })
    );
    // A region that would wrap when its size is added to its position is refused rather
    // than folded back onto the screen.
    assert!(matches!(
        check_region(region(i32::MAX - 4, 0, 10, 10), screen),
        Err(CaptureError::RegionOffScreen { .. })
    ));
}

#[test]
fn test_pixel_format_accepts_the_layout_the_platform_writes() {
    let accepted = PixelFormat::BGRA8888
        .checked()
        .expect("the project's own layout");
    assert_eq!(accepted.bits_per_pixel, 32);
    assert!(!accepted.has_alpha(), "depth 24 is carried in four bytes");
    let argb_layout = PixelFormat {
        depth: 32,
        ..PixelFormat::BGRA8888
    };
    let argb = argb_layout
        .checked()
        .expect("a depth-32 drawable is the same layout with alpha");
    assert!(argb.has_alpha());

    let swapped = PixelFormat {
        red_mask: 0x0000_00ff,
        blue_mask: 0x00ff_0000,
        ..PixelFormat::BGRA8888
    };
    match swapped.checked() {
        Err(CaptureError::UnsupportedVisual { red_mask, .. }) => {
            assert_eq!(red_mask, 0x0000_00ff, "the refusal reports what it saw");
        }
        other => panic!("a swapped visual must be refused: {other:?}"),
    }
    let narrow = PixelFormat {
        bits_per_pixel: 24,
        ..PixelFormat::BGRA8888
    };
    assert!(matches!(
        narrow.checked(),
        Err(CaptureError::UnsupportedVisual { .. })
    ));
}

#[test]
fn test_row_bytes_follows_the_scanline_padding_rule() {
    assert_eq!(PixelFormat::BGRA8888.row_bytes(0), 0);
    assert_eq!(PixelFormat::BGRA8888.row_bytes(1), 4);
    assert_eq!(PixelFormat::BGRA8888.row_bytes(7680), 30_720);
    // A three-pixel row of a 24-bits-per-pixel layout is nine bytes padded to twelve, which
    // is the rule the constant documents rather than an assumption about the layout read.
    let narrow = PixelFormat {
        bits_per_pixel: 24,
        ..PixelFormat::BGRA8888
    };
    assert_eq!(narrow.row_bytes(3), 12);
}

#[test]
fn test_to_rgba_swaps_the_channels_and_fills_the_alpha_of_an_opaque_drawable() {
    // The server sends blue first for the mask triple this project selects: a red pixel
    // arrives as `0, 0, 255`.
    let mut opaque = vec![0u8, 0, 255, 0, 12, 34, 56, 78];
    to_rgba(&mut opaque, PixelFormat::BGRA8888);
    assert_eq!(
        opaque,
        vec![255u8, 0, 0, 255, 56, 34, 12, 255],
        "the padding byte becomes opaque, the channels are swapped"
    );
    // A depth-32 drawable carries its own alpha, which is passed through unchanged.
    let mut translucent = vec![0u8, 0, 255, 128];
    to_rgba(
        &mut translucent,
        PixelFormat {
            depth: 32,
            ..PixelFormat::BGRA8888
        },
    );
    assert_eq!(translucent, vec![255u8, 0, 0, 128]);
}

#[test]
fn test_read_stable_accepts_a_drawable_that_stays_put() {
    let plan = FramePlan::new(8, 100, PixelFormat::BGRA8888).expect("a small frame fits");
    let mut frame = vec![0u8; plan.buffer_len()];
    let source = DoubleSource::staying(
        CaptureTarget::Window(0x40_0001),
        vec![footprint(10, 20, 8, 100)],
    );
    read_stable(&source, &plan, &mut frame).expect("a drawable that does not move is read once");
    assert_eq!(frame, expected_frame(&plan));
    assert_eq!(
        source.asked.get(),
        2,
        "the footprint is read before and after the frame, and no more"
    );
}

#[test]
fn test_read_stable_retries_once_and_accepts_a_drawable_that_settles() {
    let plan = FramePlan::new(8, 100, PixelFormat::BGRA8888).expect("a small frame fits");
    let mut frame = vec![0u8; plan.buffer_len()];
    // The first reading sees the window at one place and then at another; the second
    // reading sees it still.
    let source = DoubleSource::staying(
        CaptureTarget::Window(0x40_0001),
        vec![
            footprint(10, 20, 8, 100),
            footprint(12, 20, 8, 100),
            footprint(12, 20, 8, 100),
            footprint(12, 20, 8, 100),
        ],
    );
    read_stable(&source, &plan, &mut frame).expect("a window that settled is captured");
    assert_eq!(frame, expected_frame(&plan));
    assert_eq!(source.asked.get(), 4, "the frame was read twice");
}

#[test]
fn test_read_stable_refuses_a_drawable_that_keeps_moving() {
    let plan = FramePlan::new(8, 100, PixelFormat::BGRA8888).expect("a small frame fits");
    let mut frame = vec![0u8; plan.buffer_len()];
    let source = DoubleSource::staying(
        CaptureTarget::Window(0x40_0001),
        vec![
            footprint(10, 20, 8, 100),
            footprint(11, 20, 8, 100),
            footprint(12, 20, 8, 100),
            footprint(13, 20, 8, 100),
        ],
    );
    let refusal = read_stable(&source, &plan, &mut frame)
        .expect_err("a window that will not stay still fails the capture");
    match &refusal {
        CaptureError::WindowMoved {
            target,
            before,
            after,
        } => {
            assert_eq!(*target, CaptureTarget::Window(0x40_0001));
            assert_eq!(*before, (12, 20, 8, 100), "the retry's own reading");
            assert_eq!(*after, (13, 20, 8, 100));
        }
        other => panic!("a moved drawable must be reported as moved: {other:?}"),
    }
    let message = refusal.to_string();
    assert!(
        message.contains("window 0x400001"),
        "the refusal names the target: {message}"
    );
}

#[test]
fn test_read_stable_reports_a_short_read_from_the_source() {
    let plan = FramePlan::new(8, 100, PixelFormat::BGRA8888).expect("a small frame fits");
    let mut frame = vec![0u8; plan.buffer_len()];
    let mut source =
        DoubleSource::staying(CaptureTarget::FullScreen, vec![footprint(0, 0, 8, 100)]);
    source.short = true;
    assert!(matches!(
        read_stable(&source, &plan, &mut frame),
        Err(CaptureError::ShortRead { .. })
    ));
}

#[test]
fn test_snapshot_path_builds_the_run_layout() {
    let root = Path::new("/run/2026-09-30");
    let path = snapshot_path(root, "ui", "UI-001", 1, "composing").expect("a plain name");
    assert_eq!(
        path,
        root.join("ui").join("UI-001").join("01_composing.png")
    );
    assert_eq!(
        snapshot_path(root, "ui", "UI-001", 12, "candidates").expect("a plain name"),
        root.join("ui").join("UI-001").join("12_candidates.png"),
        "the step is padded so a listing sorts in the order the case ran"
    );
    assert_eq!(
        snapshot_path(root, "ui", "UI-001", 100, "candidates").expect("a plain name"),
        root.join("ui").join("UI-001").join("100_candidates.png"),
        "a step past the padding keeps its digits"
    );
}

#[test]
fn test_snapshot_state_tag_is_slugified() {
    let root = Path::new("/run");
    let name = |state: &str| {
        snapshot_path(root, "ui", "UI-001", 3, state)
            .expect("a state tag with a letter in it")
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned()
    };
    assert_eq!(name("Hover cell"), "03_hover-cell.png");
    assert_eq!(name("  Composing  "), "03_composing.png");
    assert_eq!(name("hover--cell"), "03_hover-cell.png");
    assert_eq!(
        snapshot_path(root, "ui", "UI-001", 3, "***"),
        Err(CaptureError::BadSegment {
            segment: String::new(),
            reason: "the state tag has no letter or digit in it",
        }),
        "a tag that slugifies to nothing cannot name a file"
    );
}

#[test]
fn test_snapshot_path_refuses_a_segment_that_could_leave_the_run_tree() {
    let root = Path::new("/run");
    for (module, case_id) in [
        ("", "UI-001"),
        ("ui", ""),
        ("..", "UI-001"),
        ("ui", ".."),
        (".", "UI-001"),
        ("../ui", "UI-001"),
        ("ui", "UI/001"),
        ("ui", "UI 001"),
        ("ui", "UI\\001"),
    ] {
        assert!(
            matches!(
                snapshot_path(root, module, case_id, 1, "composing"),
                Err(CaptureError::BadSegment { .. })
            ),
            "`{module}`/`{case_id}` must be refused"
        );
    }
    let long = "u".repeat(MAX_SEGMENT_BYTES + 1);
    assert!(matches!(
        snapshot_path(root, &long, "UI-001", 1, "composing"),
        Err(CaptureError::BadSegment { .. })
    ));
    assert!(
        snapshot_path(root, &"u".repeat(MAX_SEGMENT_BYTES), "UI-001", 1, "c").is_ok(),
        "a name exactly at the limit is still a name"
    );
}

#[test]
fn test_png_round_trip_carries_the_scale_the_depth_and_the_pixels() {
    let scratch = Scratch::new("round-trip");
    let path = scratch.file("round-trip/01_composing.png");
    let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 4];
    let metadata = CaptureMetadata {
        scale: 1.25,
        depth: 32,
    };
    encode::write_png(&path, 2, 2, &rgba, metadata).expect("the frame is written");
    assert!(
        path.parent().is_some_and(Path::exists),
        "the directory a capture goes into is created"
    );
    assert_eq!(
        read_metadata(&path).expect("the file is readable"),
        Some(metadata),
        "the record travels inside the image"
    );
    let (width, height, decoded) = decode_rgba(&path);
    assert_eq!((width, height), (2, 2));
    assert_eq!(decoded, rgba, "no colour conversion happens on the way out");
}

#[test]
fn test_png_record_survives_every_registered_ratio() {
    let scratch = Scratch::new("scales");
    for (index, scale) in [1.0_f32, 1.25, 1.5, 2.0, 3.0].into_iter().enumerate() {
        let path = scratch.file(&format!("scales/{index:02}_composing.png"));
        encode::write_png(
            &path,
            1,
            1,
            &[255, 255, 255, 255],
            CaptureMetadata { scale, depth: 24 },
        )
        .expect("the frame is written");
        let read = read_metadata(&path)
            .expect("the file is readable")
            .expect("the record is there");
        assert_eq!(
            read.scale.to_bits(),
            scale.to_bits(),
            "scale {scale} must read back bit for bit"
        );
        assert_eq!(read.depth, 24);
    }
}

#[test]
fn test_read_metadata_answers_none_for_a_png_without_the_record() {
    let scratch = Scratch::new("no-record");
    let path = scratch.file("plain/01_composing.png");
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is created");
    let file = File::create(&path).expect("the file is created");
    let mut encoder = png::Encoder::new(file, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("the header is written");
    writer
        .write_image_data(&[0, 0, 0, 255])
        .expect("the pixel is written");
    writer.finish().expect("the file is closed");
    assert_eq!(
        read_metadata(&path).expect("a readable PNG"),
        None,
        "a PNG that does not say what ratio it was taken at cannot be audited"
    );

    let not_a_png = scratch.file("plain/notes.txt");
    fs::write(&not_a_png, b"not a png").expect("the file is written");
    assert!(matches!(
        read_metadata(&not_a_png),
        Err(CaptureError::Png { .. })
    ));
    assert!(matches!(
        read_metadata(&scratch.file("plain/absent.png")),
        Err(CaptureError::Read { .. })
    ));
}

#[test]
fn test_capture_refusals_name_the_way_out() {
    let too_large = CaptureError::TooLarge {
        width: 32767,
        height: 32767,
        bytes: 4_294_836_196,
        limit: MAX_FRAME_BYTES,
    };
    assert!(too_large.to_string().contains("Region"), "{too_large}");
    let cursor = CaptureError::CursorUnavailable.to_string();
    assert!(cursor.contains("XFIXES"), "{cursor}");
    assert!(cursor.contains("get_image"), "{cursor}");
    let unsupported = CaptureError::UnsupportedVisual {
        depth: 16,
        bits_per_pixel: 16,
        red_mask: 0x7c00,
        green_mask: 0x03e0,
        blue_mask: 0x001f,
    };
    // Each mask is rendered at the width a `u32` mask is written in, so the assertion
    // carries the padding the message does.
    let rendered = unsupported.to_string();
    for mask in ["0x00007c00", "0x000003e0", "0x0000001f"] {
        assert!(
            rendered.contains(mask),
            "the refusal reports the layout it found: {rendered}"
        );
    }
    assert_eq!(CaptureTarget::Window(0x40_0001).label(), "window 0x400001");
    assert_eq!(CaptureTarget::FullScreen.label(), "the whole screen");
    assert_eq!(
        CaptureTarget::Region(RectI {
            x: 10,
            y: 20,
            w: 30,
            h: 40
        })
        .label(),
        "the region 10,20 30x40"
    );
}

#[test]
fn test_normalize_scale_is_the_value_a_capture_records() {
    // The capture writes the ratio through `normalize_scale`, so the two cannot disagree.
    for unusable in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(normalize_scale(unusable).to_bits(), 1.0f32.to_bits());
    }
    assert_eq!(normalize_scale(2.0).to_bits(), 2.0f32.to_bits());
}

#[test]
fn test_capture_pixel_chain_samples_the_colour_the_server_painted() {
    let scratch = Scratch::new("red-chain");
    let path = scratch.file("ui/UI-001/01_red.png");
    let plan = FramePlan::new(4, 3, PixelFormat::BGRA8888).expect("a small frame fits");
    // The bytes a server sends for the red test window: opaque red in the byte order the
    // mask triple this channel reads produces, so the fourth byte is padding rather than
    // alpha. The frame is filled here rather than read through the loop above, which is
    // what keeps this test about the colour path instead of about the chunk arithmetic.
    let mut frame = vec![0u8; plan.buffer_len()];
    for pixel in frame.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0, 0, 255, 0]);
    }
    to_rgba(&mut frame, PixelFormat::BGRA8888);
    let metadata = CaptureMetadata {
        scale: 2.0,
        depth: 24,
    };
    encode::write_png(&path, plan.width(), plan.height(), &frame, metadata)
        .expect("the frame is written");

    assert_eq!(
        read_metadata(&path).expect("the capture is readable"),
        Some(metadata),
        "the file a red window produces carries its ratio and its depth"
    );
    let (width, height, decoded) = decode_rgba(&path);
    assert_eq!((width, height), (plan.width(), plan.height()));
    for y in 0..height {
        for x in 0..width {
            // Exactly, rather than within one: the live test's tolerance covers a server's
            // own rounding, while this path is arithmetic over bytes that never left the
            // machine, so anything but the red that went in is a conversion nobody asked
            // for -- a channel swap would answer (0, 0, 255) here.
            assert_eq!(
                pixel(&decoded, width, x, y),
                (255, 0, 0, 255),
                "the pixel path must carry the colour the server sent: {x},{y}"
            );
        }
    }
}

#[test]
fn test_frame_plan_accepts_a_frame_exactly_at_the_memory_ceiling() {
    // Four bytes a pixel makes a row of 8192 pixels 32 KiB wide, so 8192 rows is exactly
    // the ceiling: the boundary the refusal above is one byte past.
    let at_limit =
        FramePlan::new(8192, 8192, PixelFormat::BGRA8888).expect("the ceiling is not past it");
    assert_eq!(at_limit.buffer_len() as u64, MAX_FRAME_BYTES);
    assert_eq!(at_limit.chunks().len(), 4, "and it is still read in ranges");

    let past = FramePlan::new(8192, 8193, PixelFormat::BGRA8888)
        .expect_err("one row more than the ceiling is refused");
    match &past {
        CaptureError::TooLarge { bytes, limit, .. } => {
            assert_eq!(
                *bytes,
                MAX_FRAME_BYTES + 8192 * 4,
                "the refusal counts the row that took it past"
            );
            assert_eq!(*limit, MAX_FRAME_BYTES);
        }
        other => panic!("a frame past the ceiling must be refused: {other:?}"),
    }
}

#[test]
fn test_frame_plan_accepts_the_widest_addressable_side_and_refuses_one_pixel_more() {
    let widest = FramePlan::new(MAX_SIDE_PX, 1, PixelFormat::BGRA8888)
        .expect("the widest side a request can address is planned");
    assert_eq!(widest.row_bytes(), MAX_SIDE_PX as usize * 4);
    let tallest = FramePlan::new(1, MAX_SIDE_PX, PixelFormat::BGRA8888)
        .expect("the tallest side a request can address is planned");
    for chunk in tallest.chunks() {
        // `get_image` carries the origin of a range as signed 16-bit values, so the last
        // row of the last range is what decides whether the limit is the right one.
        let last_row = chunk.y + chunk.rows;
        assert!(
            last_row <= MAX_SIDE_PX,
            "the last row a request asks for has to fit the request's own field: {last_row}"
        );
    }
    for (width, height) in [(MAX_SIDE_PX + 1, 1), (1, MAX_SIDE_PX + 1)] {
        assert!(
            matches!(
                FramePlan::new(width, height, PixelFormat::BGRA8888),
                Err(CaptureError::TooLarge { .. })
            ),
            "a {width}x{height} frame is past what a request can address"
        );
    }
}

#[test]
fn test_png_record_replaces_an_unusable_ratio_with_the_neutral_one() {
    let scratch = Scratch::new("bad-scale");
    for (index, scale) in [0.0_f32, -2.0, f32::NAN, f32::INFINITY]
        .into_iter()
        .enumerate()
    {
        let path = scratch.file(&format!("scales/{index:02}_composing.png"));
        encode::write_png(
            &path,
            1,
            1,
            &[0, 0, 0, 255],
            CaptureMetadata { scale, depth: 24 },
        )
        .expect("the frame is written");
        let read = read_metadata(&path)
            .expect("the file is readable")
            .expect("the record is there");
        assert_eq!(
            read.scale.to_bits(),
            1.0f32.to_bits(),
            "a ratio no audit can divide by ({scale}) has to be recorded as the neutral one"
        );
    }
}

/// A footprint for the doubles, written as `x`, `y`, `width`, `height`.
fn footprint(x: i32, y: i32, width: u32, height: u32) -> Footprint {
    Footprint {
        origin: (x, y),
        size: (width, height),
    }
}

#[test]
#[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
fn test_capture_of_a_red_window_samples_the_colour_the_server_painted() {
    let scratch = Scratch::new("red-window");
    let session = X11Session::connect(None).expect("a live X server on $DISPLAY");
    let conn = session.connection();
    let window = red_window(conn, RED_WINDOW_WIDTH, RED_WINDOW_HEIGHT);
    let path = scratch.file("ui/UI-001/01_window.png");
    let request = CaptureRequest {
        target: CaptureTarget::Window(window),
        include_cursor: false,
        scale: 2.0,
        out_path: path.clone(),
    };

    let started = std::time::Instant::now();
    let captured = capture(&session, &request).expect("the window is captured");
    let elapsed = started.elapsed();

    let (width, height, frame) = decode_rgba(&path);
    let centre = (
        u32::from(RED_WINDOW_WIDTH) / 2,
        u32::from(RED_WINDOW_HEIGHT) / 2,
    );
    let sampled = pixel(&frame, width, centre.0, centre.1);
    let still = read_metadata(&path)
        .expect("the capture is readable")
        .expect("the capture carries its record");
    conn.destroy_window(window).expect("the window is gone");

    assert_eq!(
        (captured.width_px, captured.height_px),
        (u32::from(RED_WINDOW_WIDTH), u32::from(RED_WINDOW_HEIGHT))
    );
    assert_eq!((width, height), (captured.width_px, captured.height_px));
    assert_eq!(captured.scale, 2.0);
    assert_eq!(
        still.scale, 2.0,
        "the file records the ratio it was read at"
    );
    assert_eq!(
        still.depth, 24,
        "the test window is drawn in the root's depth"
    );
    let (red, green, blue, alpha) = sampled;
    assert!(
        red.abs_diff(255) <= 1 && green <= 1 && blue <= 1,
        "the sampled pixel is the red the server painted, not a channel swap: {sampled:?}"
    );
    assert_eq!(
        alpha, 255,
        "an opaque drawable's padding is written as opaque"
    );
    assert!(
        elapsed <= std::time::Duration::from_secs(2),
        "one snapshot, PNG and all, has to finish inside the card's budget: {elapsed:?}"
    );
}

#[test]
#[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
fn test_capture_of_the_screen_finishes_inside_the_card_budget() {
    let scratch = Scratch::new("full-screen");
    let session = X11Session::connect(None).expect("a live X server on $DISPLAY");
    let (screen_width, screen_height) = session.screen();
    let path = scratch.file("ui/UI-001/01_full-screen.png");
    let request = CaptureRequest {
        target: CaptureTarget::FullScreen,
        include_cursor: false,
        scale: 1.0,
        out_path: path.clone(),
    };

    let started = std::time::Instant::now();
    let captured = capture(&session, &request).expect("the screen is captured");
    let elapsed = started.elapsed();

    assert_eq!(
        (captured.width_px, captured.height_px),
        (u32::from(screen_width), u32::from(screen_height))
    );
    assert!(
        elapsed <= std::time::Duration::from_secs(3),
        "one capture has to finish inside the card's budget: {elapsed:?}"
    );
    let still = read_metadata(&path)
        .expect("the capture is readable")
        .expect("the capture carries its record");
    assert_eq!(still.scale, 1.0);
}

#[test]
#[ignore = "needs a live X server; the lab job runs it with DISPLAY set"]
fn test_capture_refuses_the_pointer_and_a_region_off_the_screen() {
    let scratch = Scratch::new("refusals");
    let session = X11Session::connect(None).expect("a live X server on $DISPLAY");
    let pointer = CaptureRequest {
        target: CaptureTarget::FullScreen,
        include_cursor: true,
        scale: 1.0,
        out_path: scratch.file("ui/UI-001/01_cursor.png"),
    };
    assert_eq!(
        capture(&session, &pointer),
        Err(CaptureError::CursorUnavailable)
    );
    assert!(
        !pointer.out_path.exists(),
        "a refused capture writes no file at all"
    );

    let (screen_width, screen_height) = session.screen();
    let off_screen = CaptureRequest {
        target: CaptureTarget::Region(RectI {
            x: i32::from(screen_width) - 1,
            y: 0,
            w: 8,
            h: 8,
        }),
        include_cursor: false,
        scale: 1.0,
        out_path: scratch.file("ui/UI-001/02_region.png"),
    };
    assert!(matches!(
        capture(&session, &off_screen),
        Err(CaptureError::RegionOffScreen { .. })
    ));
    assert!(screen_height > 0, "the screen has a size");
}
