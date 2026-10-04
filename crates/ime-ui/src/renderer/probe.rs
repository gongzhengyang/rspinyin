//! The font probe: can this build actually draw the candidate text, and with which family?
//!
//! `ASM-16` assumes a usable CJK font is present, and the degraded mode it specifies --
//! report `ui/font/missing-cjk`, fall back to Latin placeholder glyphs -- has to be
//! *detected* rather than assumed. Slint's software renderer exposes no text-measurement
//! API that a platform may call, so the probe measures what the user would see: it
//! rasterizes a scene carrying the same string in both scripts and counts the pixels each
//! half painted.
//!
//! The measurement is taken once per family of [`CJK_FAMILIES`], best first, and the first
//! family that draws CJK ink is the family the window draws with. That is what makes the
//! family a measurement instead of a guess about which font the machine has: a desktop
//! carrying none of the preferred families falls through the list to the family `fontdb`
//! resolves for `sans-serif`, and a desktop with nothing that can draw CJK is reported as
//! the degraded status rather than drawn as a window full of boxes.
//!
//! The probe renders through [`MinimalSoftwareWindow`], the window adapter Slint ships for
//! exactly this "draw into a buffer of my own" case, on a thread of its own. It has to be
//! a thread of its own because Slint installs its platform per thread: sharing the UI
//! thread's platform would either hand the probe the candidate window's surface or be
//! refused, and either way the probe could not run before the window exists.
//!
//! The answer is due within [`PROBE_BUDGET`]. A probe that outlives it -- a pathological
//! font that wedges the shaper, a renderer that never finishes a frame -- is abandoned
//! rather than allowed to park the thread that is building the candidate window: the
//! window then draws with the first family of [`CJK_FAMILIES`], and the cached answer
//! records [`FontStatus::TimedOut`] under [`FontStatus::PROBE_TIMEOUT_CODE`].

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use ime_types::UiError;
use slint::ComponentHandle as _;
use slint::LogicalSize;
use slint::PlatformError as SlintError;
use slint::SharedString;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

use super::raster::Argb8888Pixel;

/// The probe scene width in logical pixels; it matches the `.slint` source below.
const PROBE_WIDTH: u32 = 160;

/// The probe scene height in logical pixels; it matches the `.slint` source below.
const PROBE_HEIGHT: u32 = 64;

/// The column where the CJK half ends and the Latin half begins.
const PROBE_SPLIT: u32 = 80;

/// The probe scene.
///
/// Both halves carry the same information in different scripts, so one frame says which of
/// them the font stack can draw. The CJK half is measured once per family, so the family is
/// a property the probe writes before each draw rather than a constant of the scene. The
/// module is private and nothing in it is exported: the candidate window must never become a
/// Slint surface a third party can program against (Slint Royalty-free 2.0, obligation
/// `OB-4`).
///
/// The allow is for the property accessors the macro generates and this probe never calls;
/// they cannot be removed, because they are produced by the macro rather than written
/// here.
#[allow(dead_code)]
mod scene {
    slint::slint! {
        export component FontProbe inherits Window {
            width: 160px;
            height: 64px;
            background: transparent;

            // The family the CJK half is measured with, written by the probe before every
            // draw. The default is the family the fallback list ends with, which is what a
            // frame drawn before the probe writes anything has to be read as.
            in property <string> family: "sans-serif";

            Text {
                x: 0px;
                y: 0px;
                text: "你好啊";
                font-family: root.family;
                font-size: 24px;
                color: white;
            }

            Text {
                x: 80px;
                y: 0px;
                text: "abc";
                font-size: 24px;
                color: white;
            }
        }
    }
}

/// How long the probe thread has to produce its answer.
///
/// The probe is a once-per-process step, but the thread waiting for it is the one
/// building the candidate window, so a probe that never comes back must not keep the
/// window from ever appearing. A healthy first shaping pass takes milliseconds, so
/// 500 ms leaves orders of magnitude of headroom, and a probe that still has not
/// answered by then is one whose answer is worth less than the window it is holding up.
const PROBE_BUDGET: Duration = Duration::from_millis(500);

/// The CJK families the probe tries, best first.
///
/// The order is by how likely a Linux desktop is to carry the family and how complete its
/// Simplified-Chinese coverage is. `sans-serif` is last because it is the family `fontdb`
/// resolves to when nothing else matched, which is exactly the case this probe exists to
/// detect: a frame it drew is the "no CJK fallback is installed" answer, not a usable one.
pub const CJK_FAMILIES: [&str; 7] = [
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "WenQuanYi Micro Hei",
    "Microsoft YaHei",
    "PingFang SC",
    "Droid Sans Fallback",
    "sans-serif",
];

/// The family the probe settled on, and what it found.
///
/// The index travels beside the status because the window needs the family itself -- it is
/// the value of its `Theme.font-family` token -- while the degradation path only needs to
/// know whether CJK can be drawn at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontChoice {
    /// What the probe found.
    pub status: FontStatus,
    /// Index into [`CJK_FAMILIES`] of the first family that drew CJK ink, or `None` when
    /// none did. `None` is the degraded answer: the window draws with the last entry of
    /// [`CJK_FAMILIES`], which is the family `fontdb` falls back to.
    pub family_index: Option<usize>,
}

impl FontChoice {
    /// The answer of a probe that settled on no family; `status` says why.
    fn without_family(status: FontStatus) -> Self {
        Self {
            status,
            family_index: None,
        }
    }

    /// The answer of a probe that outlived [`PROBE_BUDGET`].
    ///
    /// No family was measured, but the window still has to draw with one, and the first
    /// entry of [`CJK_FAMILIES`] -- the head of the existing fallback order -- is a better
    /// default than the `None` that would hand the window the last-resort family. The
    /// index is therefore adopted, never reported as a measurement:
    /// [`FontStatus::TimedOut`] is what keeps [`probe_fonts`] from reading it as ready.
    fn timed_out() -> Self {
        // The list is a fixed array with a non-zero length, so entry 0 always exists.
        Self {
            status: FontStatus::TimedOut,
            family_index: Some(0),
        }
    }
}

/// What the font probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStatus {
    /// Both scripts produced glyphs; the candidate window renders as designed.
    Ready,
    /// Latin produced glyphs and CJK did not, so the CJK fallback font is missing
    /// (`ASM-16`).
    MissingCjk,
    /// Nothing produced glyphs: the build has no usable font backend at all.
    NoFonts,
    /// The probe could not run, so nothing is known about the fonts.
    Unavailable,
    /// The probe did not answer within [`PROBE_BUDGET`], so nothing was measured -- but
    /// the window still draws, with the first family of [`CJK_FAMILIES`], because the
    /// answer it adopted is a deadline's rather than a measurement's.
    TimedOut,
}

impl FontStatus {
    /// The stable code a probe that outlived its budget is reported under.
    ///
    /// The timeout says nothing about which fonts the machine carries, so it must not
    /// share a code with the missing-CJK degradation: reporters emit this string as-is.
    pub const PROBE_TIMEOUT_CODE: &str = "ui/font/probe-timeout";

    /// The error to record for this status, if it is a degraded one.
    ///
    /// Both degraded statuses map onto `ui/font/missing-cjk`: they differ in cause, not in
    /// what the user sees, and the caller falls back to the Latin placeholder set either
    /// way. A timeout maps onto neither: it is a fact about the machine's speed rather
    /// than about its fonts, and it is reported under [`Self::PROBE_TIMEOUT_CODE`]
    /// instead.
    pub fn error(self) -> Option<UiError> {
        match self {
            Self::Ready | Self::Unavailable | Self::TimedOut => None,
            Self::MissingCjk | Self::NoFonts => Some(UiError::FontMissingCjk),
        }
    }
}

/// Renders probe frames off-screen and reports which family the window should draw with.
///
/// Call this once while the UI layer starts up, before the candidate window is shown: the
/// first shaping pass is the expensive one, and the probe is what turns the `ASM-16`
/// degradation into a recorded diagnostic instead of a silently blank window. Every family
/// of [`CJK_FAMILIES`] is measured in turn and the first that draws CJK ink is the answer, so
/// the family the window draws with is a measurement rather than a guess about which font
/// the machine has. The probe rasterizes into a scratch of its own and drops it.
///
/// The probe runs on a thread of its own and the call waits for it for [`PROBE_BUDGET`]:
/// a probe that answers in time leaves no thread behind, and one that outlives the budget
/// is abandoned, leaving its join parked on a detached helper that disappears with the
/// process. It never touches the candidate window's surface.
///
/// The answer is resolved once per process and cached, because it is a property of the
/// machine the process runs on: this function and [`probe_fonts`] read the same measurement,
/// so asking both costs one probe rather than two.
///
/// # Panics
///
/// Never panics. A panic inside the probe thread is caught by the join and reported as
/// [`FontStatus::NoFonts`], which is exactly what a build without a font backend produces:
/// Slint's software renderer aborts when it is asked to shape text with no font to shape it
/// with.
pub fn probe_font_choice() -> FontChoice {
    *FONT_CHOICE.get_or_init(run_probe_on_its_own_thread)
}

/// Renders a probe frame off-screen and reports whether CJK text can be drawn.
///
/// The three-state view of [`probe_font_choice`]: a family that drew CJK ink is the ready
/// case, and the status the probe settled on is the degraded one otherwise. The one
/// exception is the timeout, whose answer carries a family it adopted rather than
/// measured: that answer reports [`FontStatus::TimedOut`] instead of ready. A caller that
/// also needs the family the window draws with -- the value of its `Theme.font-family`
/// token -- calls [`probe_font_choice`] instead.
///
/// # Panics
///
/// Never panics, for the reasons [`probe_font_choice`] gives.
pub fn probe_fonts() -> FontStatus {
    three_state_view(probe_font_choice())
}

/// The status [`probe_fonts`] reports for one probe answer.
///
/// A family the probe measured drawing CJK ink is what makes the ready case; the family a
/// timeout adopted is explicitly not one, because nothing was measured. Every other
/// answer is its own status unchanged.
fn three_state_view(choice: FontChoice) -> FontStatus {
    match choice.status {
        FontStatus::TimedOut => FontStatus::TimedOut,
        _ if choice.family_index.is_some() => FontStatus::Ready,
        _ => choice.status,
    }
}

/// The probe's answer, resolved at most once per process.
static FONT_CHOICE: OnceLock<FontChoice> = OnceLock::new();

/// Runs [`run_probe`] on a thread of its own, so the probe platform is installed on a thread
/// that shares no Slint window with the candidate window.
///
/// The thread is given [`PROBE_BUDGET`] to answer and abandoned past it: the caller is the
/// thread building the candidate window, and a window that waits forever for a wedged
/// probe is a worse defect than a window that draws with the fallback family.
fn run_probe_on_its_own_thread() -> FontChoice {
    let probe = std::thread::Builder::new()
        .name(String::from("rspinyin-font-probe"))
        .spawn(run_probe);
    match probe {
        Ok(handle) => join_probe_within(handle, PROBE_BUDGET),
        Err(_) => FontChoice::without_family(FontStatus::Unavailable),
    }
}

/// Waits for the probe thread for at most `budget`, falling back to the timeout answer.
///
/// `JoinHandle::join` has no timeout, so -- the same way the UI thread bounds its own
/// joins -- the join happens on a helper thread of its own and the caller waits on a
/// channel instead. On timeout the helper is detached: it stays parked on the join and
/// disappears with the process, which is strictly better than making the window wait for
/// a probe that will not finish. The probe thread itself is left running too; if it
/// answers after all, its answer lands on a channel nobody reads anymore.
///
/// A probe thread that panicked is still the no-font-backend answer, exactly as the
/// un-timed join reported it before a budget existed. A helper that dies without
/// delivering one is `Unavailable`, because nothing is then known about the fonts.
fn join_probe_within(handle: JoinHandle<FontChoice>, budget: Duration) -> FontChoice {
    let (finished, done) = mpsc::channel();
    let helper = std::thread::Builder::new()
        .name(String::from("rspinyin-font-probe-join"))
        .spawn(move || {
            let joined = handle
                .join()
                .unwrap_or_else(|_| FontChoice::without_family(FontStatus::NoFonts));
            let _ = finished.send(joined);
        });
    match helper {
        Ok(_helper) => match done.recv_timeout(budget) {
            Ok(choice) => choice,
            Err(mpsc::RecvTimeoutError::Timeout) => FontChoice::timed_out(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                FontChoice::without_family(FontStatus::Unavailable)
            }
        },
        Err(_) => FontChoice::without_family(FontStatus::Unavailable),
    }
}

/// The probe itself, on the thread that owns the probe platform.
fn run_probe() -> FontChoice {
    let slot: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>> = Rc::new(RefCell::new(None));
    let platform = ProbePlatform {
        window: Rc::clone(&slot),
    };
    if slint::platform::set_platform(Box::new(platform)).is_err() {
        return FontChoice::without_family(FontStatus::Unavailable);
    }
    let Ok(component) = scene::FontProbe::new() else {
        return FontChoice::without_family(FontStatus::Unavailable);
    };
    // `component` stays alive until every frame has been rasterized: the window holds only a
    // weak reference to the item tree, so dropping the handle would leave the renderer with
    // nothing to draw.
    component
        .window()
        .set_size(LogicalSize::new(PROBE_WIDTH as f32, PROBE_HEIGHT as f32));
    let Some(window) = slot.borrow().as_ref().map(Rc::clone) else {
        return FontChoice::without_family(FontStatus::Unavailable);
    };
    let mut scratch =
        vec![Argb8888Pixel::TRANSPARENT; PROBE_WIDTH as usize * PROBE_HEIGHT as usize];
    let stride = PROBE_WIDTH as usize;
    let mut inks = Vec::with_capacity(CJK_FAMILIES.len());
    for family in CJK_FAMILIES {
        component.set_family(SharedString::from(family));
        // One frame per family, into the same scratch. The scratch is cleared and the frame
        // is asked for explicitly: a family that draws nothing has to be measured as
        // nothing rather than against the ink the family before it left behind.
        scratch.fill(Argb8888Pixel::TRANSPARENT);
        window.request_redraw();
        if !draw_frame(&window, &mut scratch) {
            return FontChoice::without_family(FontStatus::Unavailable);
        }
        let cjk_ink = count_ink(&scratch, stride, 0..PROBE_SPLIT as usize);
        let latin_ink = count_ink(&scratch, stride, PROBE_SPLIT as usize..stride);
        inks.push((cjk_ink, latin_ink));
    }
    choose_family(&inks)
}

/// Rasterizes the probe scene into `scratch`, reporting whether Slint drew a frame at all.
fn draw_frame(window: &MinimalSoftwareWindow, scratch: &mut [Argb8888Pixel]) -> bool {
    let mut rendered = false;
    window.draw_if_needed(|renderer| {
        renderer.render(scratch, PROBE_WIDTH as usize);
        rendered = true;
    });
    rendered
}

/// The ink one family's probe frame carried: the CJK half and the Latin half.
type FamilyInk = (u32, u32);

/// Picks the family the window draws with, from the ink each family's frame carried.
///
/// `inks` is in [`CJK_FAMILIES`] order, so the first family with CJK ink is the best one the
/// machine carries and no later family can improve on it. The Latin ink of every family
/// measured so far is what tells a missing CJK fallback apart from a build with no font
/// backend at all, and the family that drew it is not necessarily the first one.
fn choose_family(inks: &[FamilyInk]) -> FontChoice {
    let mut latin_ink = 0;
    for (index, (cjk, latin)) in inks.iter().enumerate() {
        latin_ink = latin_ink.max(*latin);
        if *cjk > 0 {
            return FontChoice {
                status: classify_probe_ink(*cjk, latin_ink),
                family_index: Some(index),
            };
        }
    }
    FontChoice {
        status: classify_probe_ink(0, latin_ink),
        family_index: None,
    }
}

/// Turns the ink counts of the two halves of a probe frame into a status.
///
/// Latin is what every font stack has; CJK is the one that is missing when `ASM-16` is
/// violated, so CJK ink alone is enough to call the fonts ready.
fn classify_probe_ink(cjk_ink: u32, latin_ink: u32) -> FontStatus {
    match (cjk_ink > 0, latin_ink > 0) {
        (true, _) => FontStatus::Ready,
        (false, true) => FontStatus::MissingCjk,
        (false, false) => FontStatus::NoFonts,
    }
}

/// Counts the pixels carrying ink in one column range of a rasterized probe frame.
///
/// Antialiased glyph edges are partially transparent rather than absent, so any alpha above
/// zero counts: the question is whether the font stack drew anything at all.
fn count_ink(pixels: &[Argb8888Pixel], stride: usize, columns: Range<usize>) -> u32 {
    let mut ink = 0u32;
    for row in pixels.chunks(stride.max(1)) {
        for pixel in row.get(columns.clone()).unwrap_or_default() {
            if pixel.alpha() > 0 {
                ink = ink.saturating_add(1);
            }
        }
    }
    ink
}

/// The platform the probe thread installs for itself.
struct ProbePlatform {
    /// Filled in when Slint asks the probe platform for its window.
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for ProbePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, SlintError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.window.borrow_mut() = Some(Rc::clone(&window));
        let adapter: Rc<dyn WindowAdapter> = window;
        Ok(adapter)
    }
}

#[cfg(test)]
mod tests {
    use slint::platform::software_renderer::TargetPixel as _;

    use super::*;

    #[test]
    fn test_classify_probe_ink_separates_the_three_outcomes() {
        assert_eq!(classify_probe_ink(120, 40), FontStatus::Ready);
        assert_eq!(
            classify_probe_ink(0, 40),
            FontStatus::MissingCjk,
            "Latin ink without CJK ink is the missing CJK fallback"
        );
        assert_eq!(
            classify_probe_ink(0, 0),
            FontStatus::NoFonts,
            "no ink at all means no font backend"
        );
        assert_eq!(
            classify_probe_ink(7, 0),
            FontStatus::Ready,
            "CJK ink alone is enough"
        );
    }

    #[test]
    fn test_choose_family_takes_the_first_family_that_drew_cjk() {
        // The list is ordered by preference, so a later family's larger ink count must not
        // displace the first family that drew anything.
        let choice = choose_family(&[(0, 12), (40, 12), (90, 12)]);

        assert_eq!(choice.family_index, Some(1));
        assert_eq!(choice.status, FontStatus::Ready);
    }

    #[test]
    fn test_choose_family_without_cjk_ink_reports_the_missing_fallback() {
        // Latin ink and no CJK ink is `ASM-16` violated: the font stack draws, but not the
        // script the candidate window is for.
        let choice = choose_family(&[(0, 0), (0, 7)]);

        assert_eq!(choice.family_index, None);
        assert_eq!(choice.status, FontStatus::MissingCjk);
    }

    #[test]
    fn test_choose_family_without_any_ink_reports_no_fonts() {
        let choice = choose_family(&[(0, 0); CJK_FAMILIES.len()]);

        assert_eq!(choice.family_index, None);
        assert_eq!(choice.status, FontStatus::NoFonts);
    }

    #[test]
    fn test_choose_family_empty_measurement_reports_no_fonts() {
        // The boundary of the list: a probe that measured nothing at all is the same
        // degraded answer as one whose every family drew nothing, rather than a panic or a
        // "ready" the measurement cannot justify.
        let choice = choose_family(&[]);

        assert_eq!(choice.family_index, None);
        assert_eq!(choice.status, FontStatus::NoFonts);
    }

    #[test]
    fn test_font_status_error_reports_the_missing_cjk_code() {
        assert_eq!(FontStatus::Ready.error(), None);
        assert_eq!(FontStatus::Unavailable.error(), None);
        assert_eq!(
            FontStatus::MissingCjk.error(),
            Some(UiError::FontMissingCjk)
        );
        assert_eq!(FontStatus::NoFonts.error(), Some(UiError::FontMissingCjk));
        assert_eq!(
            FontStatus::NoFonts.error().map(|error| error.to_string()),
            Some(String::from("ui/font/missing-cjk"))
        );
    }

    #[test]
    fn test_count_ink_counts_only_pixels_with_alpha() {
        let mut pixels = vec![Argb8888Pixel::TRANSPARENT; 4 * 4];
        // Column 0 of rows 0 and 1: the walk has to cover every row of the strip, not
        // just the first one.
        pixels[0] = Argb8888Pixel::from_rgb(1, 1, 1);
        pixels[5] = Argb8888Pixel::from_rgb(2, 2, 2);
        // Column 3 of row 0: an antialiased edge is partially transparent rather than
        // absent, and any alpha above zero still counts as ink.
        pixels[3] = Argb8888Pixel::pack(0, 0, 0, 1);
        assert_eq!(count_ink(&pixels, 4, 0..2), 2);
        assert_eq!(count_ink(&pixels, 4, 2..4), 1);
        assert_eq!(count_ink(&pixels, 4, 0..4), 3);
        assert_eq!(count_ink(&pixels, 4, 4..4), 0);
        // A strip holding only transparent pixels stays at zero, which is what keeps a
        // blank probe frame from being read as a font stack that drew something.
        assert_eq!(count_ink(&pixels, 4, 1..2), 1, "row 0 column 1 is empty");
    }

    #[test]
    fn test_probe_fonts_runs_to_a_status_of_its_own() {
        // The probe installs a platform on a thread of its own and rasterizes off-screen.
        // Asserting that it completes is what catches a broken thread or platform setup;
        // which of the ready-or-degraded statuses it reports depends on the fonts installed
        // on the machine and on the renderer features this build was compiled with.
        let status = probe_fonts();
        assert_ne!(
            status,
            FontStatus::Unavailable,
            "the probe runs to completion on its own thread"
        );
    }

    #[test]
    fn test_probe_font_choice_agrees_with_its_own_status() {
        // The two halves of one answer have to agree whatever the machine carries: an index
        // means a family drew CJK ink, and no index means none did. Which of the two cases
        // this machine produces depends on the fonts installed, so what is asserted is the
        // invariant rather than one of the outcomes. A probe that outlived its budget is
        // the one exception: its family is adopted, not measured, and the status says so.
        let choice = probe_font_choice();

        assert_ne!(
            choice.status,
            FontStatus::Unavailable,
            "the probe runs to completion on its own thread"
        );
        if choice.status == FontStatus::TimedOut {
            assert_eq!(
                choice.family_index,
                Some(0),
                "a timed-out answer adopts the first family of the list"
            );
        } else {
            match choice.family_index {
                Some(index) => {
                    assert_eq!(choice.status, FontStatus::Ready);
                    assert!(
                        index < CJK_FAMILIES.len(),
                        "the index names a family of the list"
                    );
                }
                None => assert_ne!(
                    choice.status,
                    FontStatus::Ready,
                    "a ready status needs a family that drew CJK ink"
                ),
            }
        }
    }

    #[test]
    fn test_probe_fonts_and_probe_font_choice_read_one_answer() {
        // Both entry points read the cached measurement, so the three-state answer is the
        // one the answer itself already implies rather than a second probe's. The mapping
        // is spelled out here instead of calling the helper, so a change to either side of
        // it fails this test.
        let choice = probe_font_choice();
        let expected = if choice.status == FontStatus::TimedOut {
            FontStatus::TimedOut
        } else if choice.family_index.is_some() {
            FontStatus::Ready
        } else {
            choice.status
        };

        assert_eq!(probe_fonts(), expected);
    }

    #[test]
    fn test_join_probe_within_thread_answers_in_time_returns_its_choice() {
        // A real thread is the only honest stand-in for the probe thread: the helper
        // joins it through the same `JoinHandle` the production path hands over.
        let answer = FontChoice {
            status: FontStatus::Ready,
            family_index: Some(3),
        };
        let handle = std::thread::spawn(move || answer);
        let choice = join_probe_within(handle, Duration::from_secs(5));

        assert_eq!(choice, answer);
    }

    #[test]
    fn test_join_probe_within_thread_outliving_budget_returns_the_timeout_answer() {
        // The stand-in sleeps far past the budget, so the timer -- not the thread -- has
        // to end the wait: the helper must hand back the deadline's answer instead of
        // parking on a probe that will not finish.
        let handle = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(30));
            FontChoice {
                status: FontStatus::Ready,
                family_index: Some(0),
            }
        });
        let choice = join_probe_within(handle, Duration::from_millis(50));

        assert_eq!(choice.status, FontStatus::TimedOut);
        assert_eq!(
            choice.family_index,
            Some(0),
            "the timeout adopts the first candidate of the fallback order"
        );
    }

    #[test]
    fn test_join_probe_within_panicking_thread_reports_no_fonts() {
        // A probe thread's panic is what join reports as an error, and it is the
        // no-font-backend shape: the answer must stay the one the un-timed join gave.
        let handle = std::thread::spawn(|| panic!("the renderer found no font to shape with"));
        let choice = join_probe_within(handle, Duration::from_secs(5));

        assert_eq!(choice, FontChoice::without_family(FontStatus::NoFonts));
    }

    #[test]
    fn test_three_state_view_measured_family_reads_as_ready() {
        // The unchanged half of the mapping: a family the probe measured drawing CJK ink
        // is ready, and a degraded answer without a family keeps its own status.
        let measured = FontChoice {
            status: FontStatus::Ready,
            family_index: Some(2),
        };

        assert_eq!(three_state_view(measured), FontStatus::Ready);
        assert_eq!(
            three_state_view(FontChoice::without_family(FontStatus::MissingCjk)),
            FontStatus::MissingCjk
        );
    }

    #[test]
    fn test_three_state_view_timeout_with_adopted_family_keeps_the_timeout() {
        // The half the family index alone cannot express: the timeout answer carries a
        // family the window draws with, but nothing was measured, so reading it as ready
        // would silence the very degradation the timeout path exists to report.
        let choice = FontChoice::timed_out();

        assert_eq!(three_state_view(choice), FontStatus::TimedOut);
    }

    #[test]
    fn test_font_status_timeout_carries_no_error_but_its_own_code() {
        // A timeout is a fact about the machine's speed, not about its fonts: it must not
        // travel under the missing-CJK code a diagnostic matches on, and its own code is
        // the stable string the reporters emit as-is.
        assert_eq!(FontStatus::TimedOut.error(), None);
        assert_eq!(FontStatus::PROBE_TIMEOUT_CODE, "ui/font/probe-timeout");
    }
}
