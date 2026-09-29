//! Tests for the caret resolution ladder.
//!
//! Every test drives the resolver through in-memory seams -- a scripted enumeration and a fixed
//! window origin -- so nothing here depends on a display server, a real screen layout or the
//! clock. The single timing assertion is a smoke bound, not the measurement.

use std::sync::atomic::Ordering;
use std::time::Instant;

use ime_types::{Anchor, Placement, RectI, ScreenId};

use crate::ffi::FcitxCursorRect;
use crate::screen::{ScreenInfo, ScreenLayout, ScriptedScreens};

use super::*;

/// A caret rectangle as the host reports it.
fn caret(x: i32, y: i32, w: i32, h: i32, scale: f64) -> FcitxCursorRect {
    FcitxCursorRect { x, y, w, h, scale }
}

/// One output.
fn screen(id: u32, origin: (i32, i32), size: (u32, u32), scale: f32) -> ScreenInfo {
    ScreenInfo::new(ScreenId::new(id), origin, size, scale, "test")
}

/// A layout of the given outputs, with `primary` as the platform's primary output.
fn layout_of(screens: Vec<ScreenInfo>, primary: u32) -> ScreenLayout {
    ScreenLayout::new(screens, ScreenId::new(primary))
}

/// A single 1920x1080 output at the desktop origin.
fn solo_layout() -> ScreenLayout {
    layout_of(vec![screen(0, (0, 0), (1920, 1080), 1.0)], 0)
}

/// Two outputs side by side at different ratios: 1920x1080 @1.0, then 2560x1440 @2.0.
fn mixed_layout() -> ScreenLayout {
    let screens = vec![
        screen(0, (0, 0), (1920, 1080), 1.0),
        screen(1, (1920, 0), (2560, 1440), 2.0),
    ];
    layout_of(screens, 0)
}

/// A desktop whose only output starts at x = 1920, as if the left monitor were off.
fn gapped_layout() -> ScreenLayout {
    layout_of(vec![screen(0, (1920, 0), (1920, 1080), 1.0)], 0)
}

/// A resolver that can neither enumerate nor name a window origin.
fn blind_resolver() -> CursorResolver {
    CursorResolver::without_platform()
}

/// A resolver over one known window origin and no enumeration.
fn resolver_with_origin(ic: IcId, origin: (i32, i32)) -> CursorResolver {
    let (x, y) = origin;
    let geometry = FixedGeometry::new(vec![(ic, WindowOrigin { x, y })]);
    let screens = ScriptedScreens::unavailable();
    CursorResolver::new(Box::new(screens), Box::new(geometry))
}

/// A resolver over a scripted enumeration and no window geometry.
fn enumerating_resolver(screens: ScriptedScreens) -> CursorResolver {
    CursorResolver::new(Box::new(screens), Box::new(UnknownWindowGeometry))
}

/// The anchor a caret should resolve to.
fn anchor_on(caret: (i32, i32, u32, u32), screen: u32, scale: f32) -> Anchor {
    let (x, y, w, h) = caret;
    Anchor {
        cursor: RectI { x, y, w, h },
        screen: ScreenId::new(screen),
        scale,
        placement: Placement::Auto,
    }
}

/// Resolves one caret for context 1, reporting the rung that answered.
fn resolved(
    resolver: &mut CursorResolver,
    rect: FcitxCursorRect,
    layout: &ScreenLayout,
) -> Option<Resolution> {
    resolver.resolve_with_tier(IcId::new(1), rect, layout).ok()
}

/// The anchor one caret for context 1 resolves to.
fn anchor_of(
    resolver: &mut CursorResolver,
    rect: FcitxCursorRect,
    layout: &ScreenLayout,
) -> Option<Anchor> {
    resolved(resolver, rect, layout).map(|resolution| resolution.anchor)
}

#[test]
fn test_resolve_uses_a_screen_absolute_caret_unchanged() {
    let layout = solo_layout();
    let rect = caret(300, 400, 2, 24, 1.0);
    let expected = anchor_on((300, 400, 2, 24), 0, 1.0);
    let resolution = resolved(&mut blind_resolver(), rect, &layout);
    assert_eq!(resolution.map(|r| r.anchor), Some(expected));
    let tier = resolution.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::FrontendAbsolute));
    let emitting = blind_resolver().resolve(IcId::new(1), rect, &layout);
    assert_eq!(emitting.ok(), Some(expected));
}

#[test]
fn test_resolve_scales_client_coordinates_and_replaces_an_unusable_scale() {
    let layout = layout_of(vec![screen(0, (0, 0), (3840, 2160), 2.0)], 0);
    let mut resolver = blind_resolver();
    let rect = caret(100, 100, 2, 20, 2.0);
    let expected = anchor_on((200, 200, 4, 40), 0, 2.0);
    assert_eq!(anchor_of(&mut resolver, rect, &layout), Some(expected));
    // A client scale outside [1.0, 3.0] becomes 1.0; the anchor still carries the output's
    // ratio, because that is what the UI rasterises with.
    let doubled = anchor_on((300, 400, 2, 20), 0, 2.0);
    for bad in [0.0_f64, 0.5, 3.5, -2.0, f64::NAN, f64::INFINITY] {
        let rect = caret(300, 400, 2, 20, bad);
        assert_eq!(anchor_of(&mut resolver, rect, &layout), Some(doubled));
    }
    // An output whose own ratio is broken is substituted the same way.
    let broken = layout_of(vec![screen(0, (0, 0), (1920, 1080), 0.0)], 0);
    let solid = caret(300, 400, 2, 24, 1.0);
    let expected = anchor_on((300, 400, 2, 24), 0, 1.0);
    assert_eq!(anchor_of(&mut resolver, solid, &broken), Some(expected));
}

#[test]
fn test_resolve_hit_tests_the_caret_centre_and_keeps_negative_coordinates() {
    // The first caret's centre (2001, 312) is on the second output, whose ratio is 2.0; the
    // second straddles the boundary at 1920 and is decided by its centre (1945, 312).
    let layout = mixed_layout();
    let mut resolver = blind_resolver();
    let on_second = caret(2000, 300, 2, 24, 1.0);
    let straddling = caret(1900, 300, 90, 24, 1.0);
    let expected = anchor_on((2000, 300, 2, 24), 1, 2.0);
    assert_eq!(anchor_of(&mut resolver, on_second, &layout), Some(expected));
    let expected = anchor_on((1900, 300, 90, 24), 1, 2.0);
    assert_eq!(
        anchor_of(&mut resolver, straddling, &layout),
        Some(expected)
    );
    // A desktop that extends left of the primary output keeps its negative coordinates.
    let left = layout_of(vec![screen(0, (-1920, 0), (1920, 1080), 1.0)], 0);
    let rect = caret(-1000, 500, 2, 20, 1.0);
    let expected = anchor_on((-1000, 500, 2, 20), 0, 1.0);
    assert_eq!(anchor_of(&mut resolver, rect, &left), Some(expected));
}

#[test]
fn test_resolve_adds_the_window_origin_to_client_coordinates() {
    // The client-relative centre (51, 60) misses the only output, so the window origin is added
    // and the caret lands at 1970. Tier 1 still wins when it can answer on its own.
    let layout = gapped_layout();
    let rect = caret(50, 50, 2, 20, 1.0);
    let expected = anchor_on((1970, 50, 2, 20), 0, 1.0);
    let mut resolver = resolver_with_origin(IcId::new(1), (1920, 0));
    let resolution = resolved(&mut resolver, rect, &layout);
    assert_eq!(resolution.map(|r| r.anchor), Some(expected));
    let tier = resolution.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::WindowGeometry));
    let absolute = caret(300, 400, 2, 20, 1.0);
    let resolution = resolved(&mut resolver, absolute, &solo_layout());
    let tier = resolution.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::FrontendAbsolute), "tier 1 wins");
    // The seam itself answers only for the contexts it was given.
    let origin = WindowOrigin { x: 1920, y: 0 };
    let mut geometry = FixedGeometry::new(vec![(IcId::new(1), origin)]);
    assert_eq!(geometry.window_origin(IcId::new(1)), Some(origin));
    assert_eq!(geometry.window_origin(IcId::new(2)), None);
    assert_eq!(UnknownWindowGeometry.window_origin(IcId::new(1)), None);
    assert_eq!(IcId::new(u64::MAX).value(), u64::MAX);
    assert_eq!(u64::from(IcId::new(1)), 1);
}

#[test]
fn test_resolve_falls_back_to_the_screen_centre_when_no_source_places_the_caret() {
    // 1920 + 1920 / 2 = 2880 across, 1080 * 0.6 = 648 down. A layout whose primary output is
    // missing falls back to the first one.
    let layout = gapped_layout();
    let rect = caret(50, 50, 2, 20, 1.0);
    let expected = anchor_on((2880, 648, 1, 20), 0, 1.0);
    let resolution = resolved(&mut blind_resolver(), rect, &layout);
    assert_eq!(resolution.map(|r| r.anchor), Some(expected));
    let tier = resolution.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::Fallback));
    let no_primary = layout_of(vec![screen(0, (0, 0), (1920, 1080), 1.0)], 9);
    let lost = caret(i32::MIN, 0, 2, 20, 1.0);
    let expected = anchor_on((960, 648, 1, 20), 0, 1.0);
    let mut resolver = blind_resolver();
    assert_eq!(anchor_of(&mut resolver, lost, &no_primary), Some(expected));
}

#[test]
fn test_resolve_falls_back_for_unusable_coordinates_and_repairs_degenerate_sizes() {
    let layout = solo_layout();
    let mut resolver = blind_resolver();
    let expected = anchor_on((960, 648, 1, 20), 0, 1.0);
    for bad in [i32::MIN, i32::MAX, 100_001, -100_001] {
        let rect = caret(bad, 0, 2, 20, 1.0);
        assert_eq!(anchor_of(&mut resolver, rect, &layout), Some(expected));
    }
    // A zero or negative size becomes one pixel wide and one estimated line tall.
    let scaled = layout_of(vec![screen(0, (0, 0), (3840, 2160), 2.0)], 0);
    let expected = anchor_on((200, 200, 1, 40), 0, 2.0);
    for size in [(0, 0), (-4, -9)] {
        let rect = caret(100, 100, size.0, size.1, 2.0);
        assert_eq!(anchor_of(&mut resolver, rect, &scaled), Some(expected));
    }
}

#[test]
fn test_resolve_cache_suppresses_jitter_and_survives_an_unresolvable_caret() {
    let layout = solo_layout();
    let mut resolver = blind_resolver();
    let rect = caret(300, 400, 2, 20, 1.0);
    let expected = anchor_on((300, 400, 2, 20), 0, 1.0);
    let resolution = resolved(&mut resolver, rect, &layout);
    assert_eq!(resolution.map(|r| r.anchor), Some(expected));
    let tier = resolution.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::FrontendAbsolute));
    // One pixel is jitter, so the previous anchor is reused; three pixels is a real move.
    let jittered = anchor_of(&mut resolver, caret(301, 400, 2, 20, 1.0), &layout);
    assert_eq!(jittered, Some(expected), "jitter");
    let moved = anchor_of(&mut resolver, caret(303, 400, 2, 20, 1.0), &layout);
    let moved_on = anchor_on((303, 400, 2, 20), 0, 1.0);
    assert_eq!(moved, Some(moved_on), "a real move");
    // A caret that cannot be located keeps the last position rather than jumping.
    let lost = caret(i32::MIN, i32::MIN, 2, 20, 1.0);
    let kept = resolved(&mut resolver, lost, &layout);
    assert_eq!(kept.map(|r| r.anchor), Some(moved_on), "kept over");
    let tier = kept.map(|r| r.tier);
    assert_eq!(tier, Some(CursorTier::Fallback));
    // Losing focus drops the cache, but only for the context that lost it.
    resolver.on_focus_out(IcId::new(2));
    let kept = anchor_of(&mut resolver, lost, &layout);
    assert_eq!(kept, Some(moved_on), "another context");
    // Another client never inherits it.
    let other = resolver.resolve_with_tier(IcId::new(2), rect, &layout);
    assert_eq!(other.ok().map(|r| r.anchor), Some(expected));
    resolver.on_focus_out(IcId::new(2));
    let dropped = anchor_of(&mut resolver, lost, &layout);
    let fallback = anchor_on((960, 648, 1, 20), 0, 1.0);
    assert_eq!(dropped, Some(fallback), "cache cleared");
}

#[test]
fn test_layout_adoption_keeps_both_paths_in_step_and_drops_a_dead_screen() {
    let screens = ScriptedScreens::scripted(vec![solo_layout(), mixed_layout()]);
    let mut resolver = enumerating_resolver(screens);
    assert!(resolver.layout().is_none());
    let outcome = resolver.resolve_cached(IcId::new(1), caret(0, 0, 2, 20, 1.0));
    assert!(outcome.is_err(), "nothing has been enumerated yet");
    assert!(resolver.refresh_layout().is_ok());
    assert_eq!(resolver.layout(), Some(&solo_layout()));
    assert!(resolver.refresh_layout().is_ok());
    assert_eq!(resolver.layout(), Some(&mixed_layout()));
    // A layout pushed from the thread that owns the connection is adopted the same way.
    let mut pushed = blind_resolver();
    pushed.adopt_layout(mixed_layout());
    assert_eq!(pushed.layout(), Some(&mixed_layout()));
    let anchor = pushed.resolve_cached(IcId::new(1), caret(2000, 300, 2, 24, 1.0));
    assert_eq!(anchor.ok(), Some(anchor_on((2000, 300, 2, 24), 1, 2.0)));
    // Only the second output contains this caret's centre; the first refresh still reports it,
    // the second is the layout after it was unplugged, and the cached anchor must go with it.
    let unplugged = ScriptedScreens::scripted(vec![mixed_layout(), solo_layout()]);
    let mut resolver = enumerating_resolver(unplugged);
    let on_second = mixed_layout();
    let placed = caret(2000, 300, 2, 20, 1.0);
    assert!(resolved(&mut resolver, placed, &on_second).is_some());
    assert!(resolver.refresh_layout().is_ok());
    assert!(resolver.refresh_layout().is_ok());
    let lost = caret(i32::MIN, 0, 2, 20, 1.0);
    let after = anchor_of(&mut resolver, lost, &solo_layout());
    let expected = anchor_on((960, 648, 1, 20), 0, 1.0);
    assert_eq!(after, Some(expected), "output 1 is gone");
    // A layout with no output at all has no coordinate space to place a window in.
    let empty = ScreenLayout::new(Vec::new(), ScreenId::new(0));
    let mut resolver = blind_resolver();
    let outcome = resolver.resolve_with_tier(IcId::new(1), placed, &empty);
    assert!(outcome.is_err(), "an empty layout has no space");
    assert!(resolver.refresh_layout().is_err(), "nor can a failed probe");
}

#[test]
fn test_resolve_never_enumerates_the_screens() {
    let screens = ScriptedScreens::fixed(solo_layout());
    let calls = screens.handle();
    let mut resolver = enumerating_resolver(screens);
    let layout = solo_layout();
    let placed = caret(300, 400, 2, 20, 1.0);
    assert!(anchor_of(&mut resolver, placed, &layout).is_some());
    let lost = caret(i32::MIN, 0, 2, 20, 1.0);
    assert!(anchor_of(&mut resolver, lost, &layout).is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 0, "resolve never enumerates");
    assert!(resolver.refresh_layout().is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "refresh enumerates once");
}

#[test]
fn test_resolve_stays_within_the_latency_budget() {
    // A smoke test, not the measurement: the probe takes the real P99 on a live session. The
    // bound is the tier 1 / tier 3 budget of 200us per call.
    let layout = solo_layout();
    let mut resolver = blind_resolver();
    let started = Instant::now();
    for step in 0..5_000_i32 {
        let rect = caret(300 + (step % 7) * 3, 400, 2, 20, 1.0);
        assert!(anchor_of(&mut resolver, rect, &layout).is_some());
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_micros() < 1_000_000, "took {elapsed:?}");
}
