//! Criterion benchmarks for the frame path: the copy out of the scratch, and the whole
//! `render_if_dirty` call it sits in.
//!
//! Three cases, answering two different questions.
//!
//! `frame/render_if_dirty` is the end-to-end frame at the surface size `BUDGET-LAT-03` is
//! stated for: a steady-state keystroke -- a property change the size of a highlight move --
//! followed by one call on the platform, measured the way the UI thread makes it. It covers
//! the rasterization of the changed region and the copy together, which is what one keystroke
//! actually costs.
//!
//! `frame/blit_full` and `frame/blit_pending_shown` isolate the copy, which is the part of
//! the frame the merge is about: a full-window frame (1200x280 physical pixels, the working
//! set `ASM-17` budgets for) and a steady-state one, where the two damage lists the copy
//! covers overlap. They are measured as the interval between the frame path acquiring its
//! buffer and committing it, which is the window the copy runs in: the rasterization that
//! precedes it and the backend call that follows it are outside. What the interval does
//! contain besides the copy is the fold over the two damage lists and the loop setup, tens
//! of instructions against a megabyte of memory traffic.
//!
//! # What this file does not do
//!
//! It records no number. A figure belongs in the task document once it has been taken on a
//! quiet machine, and a run under load is not evidence about one. What the cases produce is
//! `criterion`'s own report, plus the estimates it leaves in `target/criterion`, which is
//! where a budget check reads them from.
//!
//! The scene carries no text. Slint's software renderer shapes glyphs through a font backend
//! whose cost depends on the fonts installed, which would put a second, unrelated variable
//! into a frame measurement; the copy these cases are about does not care whether the pixels
//! came from a rectangle or from a glyph.
//!
//! # The surface
//!
//! [`TimedSurface`] is this target's own [`SurfaceBackend`]. The crate's mock lives behind
//! `cfg(test)`, so it is not linked into a benchmark, and this one has to record timestamps
//! anyway. A fixture that cannot be built skips its cases instead of aborting the run, which
//! is the convention the other benchmark targets in this workspace follow.
//!
//! Slint installs one platform per thread, so the whole group runs on the thread the
//! benchmark function is called on, and the platform is installed once for it.

use std::hint::black_box;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use slint::ComponentHandle as _;

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};
use ime_ui::slint_platform::RspinyinPlatform;

/// The surface width in logical pixels: 600dp at scale 2.0 is the 1200px window `ASM-17`
/// states the frame working set for.
const WIDTH_DP: u32 = 600;

/// The surface height in logical pixels: 140dp at scale 2.0 is the other side of that window.
const HEIGHT_DP: u32 = 140;

/// The device pixel ratio the frame budgets are stated for.
const SCALE: f32 = 2.0;

/// Frames drawn before the measurements start, so the first layout is out of the way.
const SETTLE_FRAMES: usize = 4;

/// Bytes per pixel of the `Argb8888` surface format.
///
/// Named here because the crate's own constant is crate-private: the benchmark only needs it
/// to size the two buffers it hands out.
const BYTES_PER_PIXEL: usize = 4;

/// The frame scene: a container with two neighbouring cells whose highlight toggles.
///
/// Two cells rather than one because a highlight that appears damages the neighbourhood the
/// previous frame damaged, which is the steady state the copy has to carry over: the two
/// damage lists overlap, and the per-rectangle form copied the overlap twice.
///
/// The module is private and nothing in it is exported -- the candidate window must never
/// become a Slint surface a third party can program against (Slint Royalty-free 2.0,
/// obligation `OB-4`). The allow covers the property accessors the macro generates and these
/// cases never call.
#[allow(dead_code)]
mod scene {
    slint::slint! {
        export component FrameCard inherits Window {
            width: 600px;
            height: 140px;
            background: transparent;

            // The container, inset by the shadow reserve the placement leaves around it.
            Rectangle {
                x: 32px;
                y: 32px;
                width: 536px;
                height: 76px;
                background: #1f2430;
            }

            in-out property <bool> highlight: false;

            Rectangle {
                x: 120px;
                y: 48px;
                width: 96px;
                height: 44px;
                visible: highlight;
                background: #3a6ea5;
            }

            Rectangle {
                x: 216px;
                y: 48px;
                width: 96px;
                height: 44px;
                visible: highlight;
                background: #3a6ea5;
            }
        }
    }
}

/// The surface size in physical pixels, which is the unit a damage rectangle is expressed in.
///
/// The platform layer does this conversion for the real backends; it is crate-private, so the
/// benchmark repeats the two multiplications rather than reaching into the crate for them.
fn physical_size() -> (u32, u32) {
    (
        (WIDTH_DP as f32 * SCALE).round() as u32,
        (HEIGHT_DP as f32 * SCALE).round() as u32,
    )
}

/// What the benchmark observes about a frame.
#[derive(Default)]
struct Observation {
    /// When `acquire_buffer` handed its buffer out.
    acquired: Option<Instant>,
    /// The interval between that instant and the `commit` that followed it, which is the
    /// window the copy out of the scratch runs in.
    copy: Duration,
    /// Events the next `poll_events` call delivers.
    pending: Vec<SurfaceEvent>,
}

/// Forgets the last frame's copy, so the next one is timed on its own.
fn forget_copy(observed: &Arc<Mutex<Observation>>) {
    if let Ok(mut seen) = observed.lock() {
        seen.acquired = None;
        seen.copy = Duration::ZERO;
    }
}

/// The interval the last committed frame spent copying.
///
/// Zero when the frame path acquired a buffer but never committed it, which is what a frame
/// skipped for want of a free buffer does.
fn copy_time(observed: &Arc<Mutex<Observation>>) -> Duration {
    observed.lock().map_or(Duration::ZERO, |seen| seen.copy)
}

/// Queues an event the platform picks up on its next poll.
fn queue(observed: &Arc<Mutex<Observation>>, event: SurfaceEvent) {
    if let Ok(mut seen) = observed.lock() {
        seen.pending.push(event);
    }
}

/// A [`SurfaceBackend`] that needs no display server and times the frame copy.
///
/// It double buffers like the real backends do, so the copy writes into the buffer that
/// holds the frame before the one on screen, exactly as it does on a real surface.
struct TimedSurface {
    /// Everything the cases read, behind a handle they keep: the platform takes ownership of
    /// the surface itself.
    observed: Arc<Mutex<Observation>>,
    /// The two draw buffers, `Argb8888`, `width_px * 4` bytes per row.
    buffers: [Vec<u8>; 2],
    /// Which buffer `acquire_buffer` hands out next.
    back: usize,
    width_px: u32,
    height_px: u32,
}

impl TimedSurface {
    /// Creates the surface the frame budgets are stated for: 600x140 logical pixels at a
    /// scale of 2.0, which is 1200x280 physical pixels.
    fn new() -> (Self, Arc<Mutex<Observation>>) {
        let (width_px, height_px) = physical_size();
        let observed = Arc::new(Mutex::new(Observation::default()));
        let length = width_px as usize * height_px as usize * BYTES_PER_PIXEL;
        let surface = Self {
            observed: Arc::clone(&observed),
            buffers: [vec![0; length], vec![0; length]],
            back: 0,
            width_px,
            height_px,
        };
        (surface, observed)
    }
}

impl SurfaceBackend for TimedSurface {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        let back = self.back;
        let data = self
            .buffers
            .get_mut(back)
            .map(|buffer| buffer.as_mut_slice())
            .ok_or(PlatformError::Unavailable)?;
        if let Ok(mut seen) = self.observed.lock() {
            seen.acquired = Some(Instant::now());
        }
        Ok(PixelBufferMut {
            data,
            stride: self.width_px as usize * BYTES_PER_PIXEL,
            width: self.width_px,
            height: self.height_px,
        })
    }

    fn commit(&mut self, _damage: &[RectI]) -> Result<(), PlatformError> {
        // Read before the lock: the interval the copy runs in ends here, and the lock the
        // observation state needs is not part of it.
        let now = Instant::now();
        let back = self.back;
        if let Ok(mut seen) = self.observed.lock() {
            // The frame path copied into the buffer between the acquisition and this call,
            // so the interval is the copy and the few instructions around it.
            seen.copy = seen
                .acquired
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        }
        self.back = 1 - back;
        Ok(())
    }

    fn set_input_region(&mut self, _rects: &[RectI]) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_visible(&mut self, _visible: bool) -> Result<(), PlatformError> {
        Ok(())
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        // A compositor that has frame callbacks: the token bookkeeping in the renderer is
        // part of the frame path, so it runs here as it does on a real surface.
        Some(FrameToken(1))
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        if let Ok(mut seen) = self.observed.lock() {
            out.append(&mut seen.pending);
        }
        Ok(())
    }

    fn geometry(&self) -> (u32, u32, f32) {
        (WIDTH_DP, HEIGHT_DP, SCALE)
    }

    fn backend_id(&self) -> &'static str {
        "bench"
    }
}

/// A platform with the benchmark's scene shown on it, or `None` when one cannot be built.
fn setup() -> Option<(RspinyinPlatform, scene::FrameCard, Arc<Mutex<Observation>>)> {
    let (surface, observed) = TimedSurface::new();
    let platform = RspinyinPlatform::new(Box::new(surface));
    platform.install().ok()?;
    let card = scene::FrameCard::new().ok()?;
    card.show().ok()?;
    for _ in 0..SETTLE_FRAMES {
        platform.render_if_dirty().ok()?;
    }
    Some((platform, card, observed))
}

/// Runs every case of this benchmark target.
fn frame_bench(criterion: &mut Criterion) {
    let Some((platform, card, observed)) = setup() else {
        return;
    };
    let mut group = criterion.benchmark_group("frame");
    // Toggled rather than set, so every iteration is a frame the renderer has work for
    // instead of an idle return.
    let mut highlight = false;

    // The frame the UI thread makes after a keystroke: one property change and one call.
    group.bench_function("render_if_dirty", |bencher| {
        bencher.iter(|| {
            highlight = !highlight;
            card.set_highlight(highlight);
            black_box(platform.render_if_dirty())
        });
    });

    // The copy a full-window frame pays. A same-size resize is the geometry path's way of
    // marking the whole surface dirty -- the scratch no longer holds what the surface shows
    // -- so every iteration copies 1200x280 pixels and nothing else worth measuring.
    group.bench_function("blit_full", |bencher| {
        bencher.iter_custom(|iterations| {
            let mut total = Duration::ZERO;
            for _ in 0..iterations {
                let (width_px, height_px) = physical_size();
                queue(
                    &observed,
                    SurfaceEvent::Resize {
                        w: width_px,
                        h: height_px,
                    },
                );
                let _ = platform.poll_events(&mut Vec::new());
                forget_copy(&observed);
                let _ = platform.render_if_dirty();
                total += copy_time(&observed);
            }
            total
        });
    });

    // The copy a steady-state keystroke pays: the damage of this frame and the damage of the
    // frame before it cover the same cells, so the region is the highlight neighbourhood and
    // the per-rectangle form would have copied it twice.
    group.bench_function("blit_pending_shown", |bencher| {
        bencher.iter_custom(|iterations| {
            let mut total = Duration::ZERO;
            for _ in 0..iterations {
                highlight = !highlight;
                card.set_highlight(highlight);
                forget_copy(&observed);
                let _ = platform.render_if_dirty();
                total += copy_time(&observed);
            }
            total
        });
    });

    group.finish();
}

criterion_group!(benches, frame_bench);
criterion_main!(benches);
