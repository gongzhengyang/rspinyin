//! The acrylic negotiation, over the mock backend.
//!
//! The three compositor answers are covered here -- applied, refused, and the user's
//! own acrylic switch -- together with the request's timing (after the window is mapped
//! and placed, before the first frame, exactly once) and the region the request names,
//! which follows a placement that moves the panel. Everything runs against the mock,
//! so the assertions are about what the surface asked and what it recorded, with no
//! display server and no window manager.
//!
//! One interaction is asserted on purpose rather than explained away: with the palette
//! as shipped, an *applied* answer still resolves to an opaque base, because the
//! contrast gate degrades every translucent tier. The negotiation is therefore pinned
//! through the diagnostic codes and the recorded requests, not through the base alpha
//! alone -- a refusal reports `ui/theme/blur-unavailable`, an applied or disabled
//! resolution does not, and only the recorded calls tell applied from disabled.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ime_types::{ColorScheme, RectI, ThemeSpec};

use crate::renderer::mock::{MockState, MockSurface, on_own_thread};
use crate::surface::CandidateSurface;
use crate::theme::{BLUR_UNAVAILABLE, CONTRAST_FALLBACK, default_accent};
use crate::ui_thread::SurfaceUpdate;

use super::{STRIDE, SURFACE_HEIGHT_DP, SURFACE_WIDTH_DP, settle, show_and_draw, show_at};

/// The placed container of the shared fixture at a ratio of 1.0, inset by the one
/// physical pixel the rounded corners need: what a blur request must name.
const INSET_AT_ONE: RectI = RectI {
    x: 33,
    y: 33,
    w: 218,
    h: 86,
};

/// The same container at twice the ratio, inset the same way.
const INSET_AT_TWO: RectI = RectI {
    x: 65,
    y: 65,
    w: 438,
    h: 174,
};

/// An acrylic theme request, with the base alpha under the test's control.
fn acrylic(base_alpha: u8) -> ThemeSpec {
    ThemeSpec {
        scheme: ColorScheme::Dark,
        accent: default_accent(ColorScheme::Dark),
        acrylic: true,
        base_alpha,
        corner_radius_dp: 12,
        scale: 1.0,
    }
}

/// The same request with acrylic off, which must never reach the compositor.
fn opaque_preference(base_alpha: u8) -> ThemeSpec {
    ThemeSpec {
        acrylic: false,
        ..acrylic(base_alpha)
    }
}

/// Builds a surface whose blur capability is wired to the mock's compositor, on a
/// fresh thread, and runs `scene` against it.
///
/// `available` decides what the mock's compositor answers; the capability reads the
/// flag at request time, so a scene may flip it through the shared state.
fn with_blur_surface<R: Send + 'static>(
    available: bool,
    scene: impl FnOnce(&mut CandidateSurface, Arc<Mutex<MockState>>) -> R + Send + 'static,
) -> R {
    on_own_thread(move || {
        let (backend, state) = MockSurface::new(SURFACE_WIDTH_DP, SURFACE_HEIGHT_DP, 1.0);
        state
            .lock()
            .expect("the mock is not poisoned")
            .blur_available = available;
        let blur = backend.blur_handle();
        let mut surface = CandidateSurface::new(Box::new(backend))
            .expect("the mock component binds")
            .with_blur_capability(Box::new(blur));
        scene(&mut surface, state)
    })
}

/// The blur requests the mock has recorded so far.
fn recorded(state: &Arc<Mutex<MockState>>) -> Vec<Vec<RectI>> {
    state
        .lock()
        .expect("the mock is not poisoned")
        .blur_requests
        .clone()
}

/// The alpha of the pixel at the centre of the placed panel.
fn centre_alpha(state: &Arc<Mutex<MockState>>, region: RectI) -> u8 {
    state.lock().expect("the mock is not poisoned").pixel(
        STRIDE,
        (region.x + region.w as i32 / 2) as usize,
        (region.y + region.h as i32 / 2) as usize,
    )[3]
}

#[test]
fn test_surface_acrylic_theme_requests_blur_once_after_the_first_placement() {
    let (requests, codes, base_alpha, after_idle) = with_blur_surface(true, |surface, state| {
        // The theme arrives before the window exists on screen: there is no
        // container rectangle to name yet, so the request waits.
        surface
            .apply(SurfaceUpdate::Theme(acrylic(217)))
            .expect("the theme is applied");
        assert!(
            recorded(&state).is_empty(),
            "nothing is asked before a placement exists"
        );
        show_and_draw(surface, 1);
        let settled = settle(surface);
        let requests = recorded(&state);
        let codes = surface.theme_diagnostics();
        let region = surface.input_region().expect("the panel was placed");
        let base_alpha = centre_alpha(&state, region);
        // More frames at the same placement: the negotiation is one-shot and the
        // rectangle is unchanged, so nothing further is sent.
        surface
            .render(settled + Duration::from_millis(8))
            .expect("an idle frame is handled");
        (requests, codes, base_alpha, recorded(&state).len())
    });
    assert_eq!(
        requests,
        [vec![INSET_AT_ONE]],
        "exactly one request, naming the placed container inset by the 1px the rounded \
         corners need"
    );
    assert_eq!(
        codes,
        [None, Some(CONTRAST_FALLBACK)],
        "an applied answer reaches the resolution -- a refusal would report \
         blur-unavailable instead"
    );
    assert_eq!(
        base_alpha, 255,
        "the base still ships opaque: the contrast gate degrades the shipped palette's \
         translucent tier whatever the compositor answered"
    );
    assert_eq!(
        after_idle, 1,
        "an unchanged placement sends nothing further"
    );
}

#[test]
fn test_surface_acrylic_theme_with_a_refusing_compositor_degrades_opaque_and_reports_it() {
    let (requests, codes) = with_blur_surface(false, |surface, state| {
        surface
            .apply(SurfaceUpdate::Theme(acrylic(255)))
            .expect("the theme is applied");
        show_and_draw(surface, 1);
        (recorded(&state), surface.theme_diagnostics())
    });
    assert_eq!(
        requests,
        [vec![INSET_AT_ONE]],
        "the attempt is made once and recorded, even though the mock's compositor \
         refuses it"
    );
    assert_eq!(
        codes,
        [Some(BLUR_UNAVAILABLE), None],
        "a refusal forces the opaque base and reports the code -- the behaviour a \
         compositor without blur has always shown"
    );
}

#[test]
fn test_surface_acrylic_switched_off_never_reaches_the_compositor() {
    let (requests, codes) = with_blur_surface(true, |surface, state| {
        surface
            .apply(SurfaceUpdate::Theme(opaque_preference(255)))
            .expect("the theme is applied");
        show_and_draw(surface, 1);
        settle(surface);
        (recorded(&state), surface.theme_diagnostics())
    });
    assert!(
        requests.is_empty(),
        "a user who turned acrylic off must not cause a compositor round trip"
    );
    assert_eq!(
        codes,
        [None, None],
        "an opaque request with acrylic off has nothing to degrade and nothing to report"
    );
}

#[test]
fn test_surface_blur_region_follows_a_changed_placement() {
    let requests = with_blur_surface(true, |surface, state| {
        surface
            .apply(SurfaceUpdate::Theme(acrylic(255)))
            .expect("the theme is applied");
        show_and_draw(surface, 1);
        settle(surface);
        // The anchor moves the panel to twice the ratio: the negotiation is not
        // re-run, but the region it named is re-sent at the new rectangle. The panel
        // is already placed, so this show restarts no appear motion and a few
        // stepped frames only have to drain what the moved placement produced.
        show_at(surface, 2, 2.0, 1);
        let mut now = Instant::now();
        for _ in 0..8 {
            now += Duration::from_millis(8);
            surface.render(now).expect("a settling frame is drawn");
        }
        let requests = recorded(&state);
        // Frames at the settled placement add nothing further.
        surface
            .render(Instant::now() + Duration::from_millis(8))
            .expect("an idle frame is handled");
        assert_eq!(
            recorded(&state).len(),
            requests.len(),
            "a placement that has not moved sends nothing"
        );
        requests
    });
    assert_eq!(
        requests,
        [vec![INSET_AT_ONE], vec![INSET_AT_TWO]],
        "the request named the first container once, then the moved one once"
    );
}

#[test]
fn test_surface_without_a_blur_capability_keeps_the_opaque_refusal() {
    // The zero-regression half: a session whose backend exposes no capability at all
    // negotiates exactly as it did before the negotiation existed.
    let codes = on_own_thread(move || {
        let (backend, _state) = MockSurface::new(SURFACE_WIDTH_DP, SURFACE_HEIGHT_DP, 1.0);
        let mut surface = CandidateSurface::new(Box::new(backend)).expect("the surface starts");
        surface
            .apply(SurfaceUpdate::Theme(acrylic(255)))
            .expect("the theme is applied");
        show_and_draw(&mut surface, 1);
        surface.theme_diagnostics()
    });
    assert_eq!(
        codes,
        [Some(BLUR_UNAVAILABLE), None],
        "no capability is a refusal: the base stays opaque and the code is reported, \
         with no behavioural change from the hardcoded answer this negotiation replaced"
    );
}
