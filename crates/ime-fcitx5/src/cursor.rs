//! Caret resolution: client coordinates to screen physical pixels.
//!
//! Fcitx5 hands the engine a caret rectangle relative to the client window, while the candidate
//! window is placed in the physical pixel space of the virtual desktop. This module normalises
//! that rectangle, decides which output it belongs to, and returns the [`Anchor`] the UI thread
//! positions the window with.
//!
//! # The source ladder
//!
//! | Tier | Source | When it answers |
//! |---|---|---|
//! | 1 | the frontend's rectangle, if it is already screen-absolute | when the heuristic below holds |
//! | 2 | the client rectangle plus the focused window's origin | X11 (`_NET_ACTIVE_WINDOW` + `xcb_translate_coordinates`); Wayland through its tier's coordinate space |
//! | 3 | the fallback: the screen's horizontal centre, 60% down | always |
//!
//! Tier 1 is a **heuristic** -- a rectangle whose centre falls inside an output is taken to be
//! absolute -- and it lives in one function ([`looks_absolute`]) so that
//! `docs/dev/spikes/cursor-probe.md`, which records what each frontend really reports, can revise
//! it in one place. Tiers 1 and 3 cost nothing; tier 2 costs one platform round trip.
//!
//! The rules the ladder applies -- physical pixels, centre-point hit testing, degenerate
//! rectangles, scale substitution -- and the arithmetic implementing them are in [`crate::screen`].
//! What lives here is the ladder, the cache that keeps the window still, and the diagnostics.

use ime_types::{Anchor, ImeError, RectI, ScreenId};

use crate::ffi::{FcitxCursorRect, emit_diagnostic};
use crate::screen::{
    ScreenEnumerator, ScreenInfo, ScreenLayout, ScriptedScreens, centre_of, check_scale,
    estimated_line_height, is_jitter, is_plausible, screen_id_at, to_i32, to_u32,
};

/// Recorded when no source could place the caret and the fallback position was used.
pub const CURSOR_UNRESOLVED_CODE: &str = "platform/cursor/unresolved";

/// Recorded when a scale factor outside `[1.0, 3.0]` was replaced with `1.0`.
pub const SCALE_INVALID_CODE: &str = "platform/scale/invalid";

/// Identifies one Fcitx5 input context.
///
/// The host identifies a context by its `ICUUID`; the ABI carries a 64-bit digest of that uuid
/// instead, because a C ABI has no place for a 16-byte value. The resolver keys its cache by this
/// id, so a caret remembered for one client never positions another's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IcId(u64);

impl IcId {
    /// Wraps a raw context id as the ABI carries it.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw id.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl From<IcId> for u64 {
    fn from(id: IcId) -> Self {
        id.0
    }
}

/// Where the focused client window sits in the virtual desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowOrigin {
    /// Left edge of the client window, in screen physical pixels.
    pub x: i32,
    /// Top edge of the client window, in screen physical pixels.
    pub y: i32,
}

/// The seam the platform backends implement to answer where the focused window is.
///
/// X11 answers with `_NET_ACTIVE_WINDOW` plus `xcb_translate_coordinates`; Wayland answers from the
/// coordinate space of its tier (see `features.md` 2.5.2) -- a layer-shell tier maps the client
/// rectangle directly, a fullscreen-parent tier adds the client window's offset inside that parent.
/// Implementations run on the host thread and must not block: the tier 2 budget is 500 us, which is
/// one round trip.
pub trait WindowGeometrySource: Send {
    /// The focused window's screen-absolute origin, or `None` when the backend cannot say.
    ///
    /// `None` is the honest answer for a platform that cannot report absolute window positions, and
    /// it is what sends the resolution to the fallback. It is not an error: nothing is delayed by it
    /// and no key is lost.
    fn window_origin(&mut self, ic: IcId) -> Option<WindowOrigin>;
}

/// A geometry source that never answers, so that tier 2 is never taken.
///
/// What the pure-Rust build has until the X11 and Wayland backends land, and what the tests use to
/// reach the fallback.
pub struct UnknownWindowGeometry;

impl WindowGeometrySource for UnknownWindowGeometry {
    fn window_origin(&mut self, _ic: IcId) -> Option<WindowOrigin> {
        None
    }
}

/// A deterministic in-memory [`WindowGeometrySource`].
///
/// A context that is not in the list has no answer, exactly as a real backend that lost its window
/// does.
pub struct FixedGeometry {
    origins: Vec<(IcId, WindowOrigin)>,
}

impl FixedGeometry {
    /// Builds a source that answers for the listed contexts only.
    pub fn new(origins: impl IntoIterator<Item = (IcId, WindowOrigin)>) -> Self {
        Self {
            origins: origins.into_iter().collect(),
        }
    }
}

impl WindowGeometrySource for FixedGeometry {
    fn window_origin(&mut self, ic: IcId) -> Option<WindowOrigin> {
        for (id, origin) in &self.origins {
            if *id == ic {
                return Some(*origin);
            }
        }
        None
    }
}

/// Which rung of the source ladder produced a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorTier {
    /// The frontend's rectangle was already screen-absolute.
    FrontendAbsolute,
    /// The client rectangle plus the focused window's origin.
    WindowGeometry,
    /// No source could place the caret: the fallback position was used, or the previous position was
    /// kept.
    Fallback,
}

/// One resolution: the anchor to use and what the resolver learned while producing it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    /// Where the candidate window should be placed.
    pub anchor: Anchor,
    /// Which rung answered. `Fallback` is the signal that the caret could not be located this
    /// frame, even when the anchor itself is a kept-over one.
    pub tier: CursorTier,
    /// Whether a scale factor outside `[1.0, 3.0]` was replaced with `1.0` on the way.
    pub scale_invalid: bool,
}

/// The remembered anchor and the context it belongs to.
#[derive(Clone, Copy, Debug)]
struct CachedAnchor {
    ic: IcId,
    anchor: Anchor,
    tier: CursorTier,
}

/// Resolves caret rectangles into screen anchors, and remembers the last one.
///
/// The resolver owns no connection and no clock: the two platform-facing pieces it needs are
/// injected ([`ScreenEnumerator`], [`WindowGeometrySource`]), which is what lets it be driven
/// entirely from memory in a test. It is `Send` and single-threaded -- it runs on the Fcitx5 host
/// thread, inside the callback that needs an anchor.
pub struct CursorResolver {
    /// Output enumeration, injected so the platform backends stay replaceable.
    screens: Box<dyn ScreenEnumerator>,
    /// Focused-window geometry, injected for the same reason.
    geometry: Box<dyn WindowGeometrySource>,
    /// The layout the last enumeration adopted.
    layout: Option<ScreenLayout>,
    /// The last anchor that was produced, for fallback stability and jitter suppression.
    cache: Option<CachedAnchor>,
    /// Whether the last resolution was unresolved, so the diagnostic is recorded once per transition
    /// into that state instead of once per frame.
    last_unresolved: bool,
    /// Whether the last resolution replaced an invalid scale, for the same reason.
    last_scale_invalid: bool,
}

impl CursorResolver {
    /// Builds a resolver over the platform seams.
    pub fn new(
        screens: Box<dyn ScreenEnumerator>,
        geometry: Box<dyn WindowGeometrySource>,
    ) -> Self {
        Self {
            screens,
            geometry,
            layout: None,
            cache: None,
            last_unresolved: false,
            last_scale_invalid: false,
        }
    }

    /// Builds a resolver with the placeholder sources.
    ///
    /// Nothing can be enumerated and no window origin is known, so every client-relative caret
    /// resolves through the fallback. A caller that supplies the layout itself still reaches tier 1
    /// and tier 3, because [`CursorResolver::resolve`] never reads the internal layout.
    pub fn without_platform() -> Self {
        let screens = ScriptedScreens::unavailable();
        Self::new(Box::new(screens), Box::new(UnknownWindowGeometry))
    }

    /// Resolves a caret rectangle and records the diagnostics it produced.
    ///
    /// `rect` is the caret rectangle as the host reported it, in client coordinates, and `layout`
    /// the outputs to normalise against. Passing the layout in rather than reading an internal one
    /// is what keeps this a function of its inputs. The anchor's `scale` is the scale of the output
    /// the caret is on, never the client's.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the layout contains no output at all: there
    /// is no coordinate space to place a window in, and the caller has to keep the previous
    /// position. Every other input -- a degenerate rectangle, a sentinel coordinate, an unusable
    /// scale factor -- resolves to an anchor.
    ///
    /// # Side effects
    ///
    /// Records `platform/cursor/unresolved` and `platform/scale/invalid`, each once per transition
    /// into that state. [`CursorResolver::resolve_with_tier`] records nothing.
    pub fn resolve(
        &mut self,
        ic: IcId,
        rect: FcitxCursorRect,
        layout: &ScreenLayout,
    ) -> Result<Anchor, ImeError> {
        let resolution = self.plan(ic, rect, layout)?;
        self.record(&resolution);
        Ok(resolution.anchor)
    }

    /// Resolves a caret rectangle without recording anything, reporting the rung that answered.
    ///
    /// # Errors
    ///
    /// As [`CursorResolver::resolve`].
    pub fn resolve_with_tier(
        &mut self,
        ic: IcId,
        rect: FcitxCursorRect,
        layout: &ScreenLayout,
    ) -> Result<Resolution, ImeError> {
        self.plan(ic, rect, layout)
    }

    /// Resolves against the layout this resolver enumerated last, so the caller does not have to
    /// clone it out of [`CursorResolver::layout`] to satisfy the borrow checker.
    ///
    /// # Errors
    ///
    /// As [`CursorResolver::resolve`], plus the case where nothing has been enumerated yet, which is
    /// the same "no coordinate space" failure.
    pub fn resolve_cached(&mut self, ic: IcId, rect: FcitxCursorRect) -> Result<Anchor, ImeError> {
        let layout = self.layout.clone().ok_or_else(no_output_error)?;
        self.resolve(ic, rect, &layout)
    }

    /// Re-enumerates the outputs, and is the only path that does.
    ///
    /// Called from the platform's own invalidation event -- RandR's `SCREEN_CHANGE_NOTIFY` under
    /// X11, the `wl_output` `geometry` / `mode` / `scale` events under Wayland -- never from a timer
    /// and never from `resolve`. A cached anchor whose output is no longer in the layout is dropped
    /// here, so a window is never placed on a monitor that has been unplugged.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the enumeration failed; the previously
    /// enumerated layout is then kept, because a stale layout places the window on the right output
    /// far more often than no layout at all.
    pub fn refresh_layout(&mut self) -> Result<(), ImeError> {
        let layout = self.screens.enumerate()?;
        self.adopt(layout);
        Ok(())
    }

    /// Adopts a layout that was enumerated somewhere else.
    ///
    /// A backend whose connection belongs to the UI thread cannot be pulled from the host thread:
    /// the Wayland tiers own their `wl_display` there, because a Wayland connection may only be used
    /// from the thread that created it. Such a backend enumerates on its own thread and hands the
    /// result over here, which applies exactly the invalidation
    /// [`CursorResolver::refresh_layout`] applies.
    pub fn adopt_layout(&mut self, layout: ScreenLayout) {
        self.adopt(layout);
    }

    /// The layout the last enumeration adopted.
    pub fn layout(&self) -> Option<&ScreenLayout> {
        self.layout.as_ref()
    }

    /// Drops the cached anchor of an input context that lost focus.
    ///
    /// The cache exists to keep the window still while the caret cannot be located, and a context
    /// that lost focus has no caret to keep it still for. A focus-out naming a different context
    /// than the cached one is ignored: it describes a client that is not the one being positioned.
    pub fn on_focus_out(&mut self, ic: IcId) {
        let cached_here = self.cache.is_some_and(|cached| cached.ic == ic);
        if cached_here {
            self.cache = None;
            self.last_unresolved = false;
            self.last_scale_invalid = false;
        }
    }

    /// The resolution itself: normalise, try each rung in order, settle on an anchor.
    fn plan(
        &mut self,
        ic: IcId,
        rect: FcitxCursorRect,
        layout: &ScreenLayout,
    ) -> Result<Resolution, ImeError> {
        let Some(default_screen) = layout.primary_screen() else {
            return Err(no_output_error());
        };
        let client_scale = check_scale(rect.scale as f32);
        let mut scale_invalid = !client_scale.is_valid;

        let Some(caret) = physical_caret(rect, client_scale.value) else {
            return Ok(self.fallback(ic, layout, None, default_screen, scale_invalid));
        };

        // Tier 1: a frontend that reports screen-absolute coordinates needs no help.
        if let Some(screen) = looks_absolute(layout, caret) {
            let (anchor, scale_valid) = screen.anchor_for(caret);
            scale_invalid |= !scale_valid;
            return Ok(self.settle(ic, anchor, CursorTier::FrontendAbsolute, scale_invalid));
        }

        // Tier 2: the client rectangle plus the focused window's origin.
        let origin = self.geometry.window_origin(ic);
        if let Some(origin) = origin {
            let absolute = translated(caret, origin);
            if let Some(screen) = looks_absolute(layout, absolute) {
                let (anchor, scale_valid) = screen.anchor_for(absolute);
                scale_invalid |= !scale_valid;
                return Ok(self.settle(ic, anchor, CursorTier::WindowGeometry, scale_invalid));
            }
        }

        // Tier 3: the output the window is on when tier 2 named one, else the primary.
        let preferred = origin.and_then(|origin| screen_id_at(layout, (origin.x, origin.y)));
        Ok(self.fallback(ic, layout, preferred, default_screen, scale_invalid))
    }

    /// The tier 3 answer: keep the previous position when there is one, otherwise place the caret at
    /// the screen's centre.
    fn fallback(
        &mut self,
        ic: IcId,
        layout: &ScreenLayout,
        preferred: Option<ScreenId>,
        default_screen: &ScreenInfo,
        mut scale_invalid: bool,
    ) -> Resolution {
        let kept = self.cache.filter(|cached| cached.ic == ic);
        let anchor = match kept {
            Some(kept) => kept.anchor,
            None => {
                let screen = match preferred {
                    Some(id) => layout.screen(id).unwrap_or(default_screen),
                    None => default_screen,
                };
                let (placed, scale_valid) = screen.fallback_anchor();
                scale_invalid |= !scale_valid;
                placed
            }
        };
        let tier = CursorTier::Fallback;
        self.cache = Some(CachedAnchor { ic, anchor, tier });
        Resolution {
            anchor,
            tier,
            scale_invalid,
        }
    }

    /// Returns the anchor to use, reusing the cached one when the caret has barely moved, and stores
    /// it as the new cache.
    fn settle(
        &mut self,
        ic: IcId,
        candidate: Anchor,
        tier: CursorTier,
        scale_invalid: bool,
    ) -> Resolution {
        let (anchor, tier) = match self.cache {
            Some(cached) if cached.ic == ic && is_jitter(&cached.anchor, &candidate) => {
                (cached.anchor, cached.tier)
            }
            _ => (candidate, tier),
        };
        self.cache = Some(CachedAnchor { ic, anchor, tier });
        Resolution {
            anchor,
            tier,
            scale_invalid,
        }
    }

    /// Records each condition once per transition into it, so a caret that stays unresolvable does
    /// not write a line per frame.
    fn record(&mut self, resolution: &Resolution) {
        let unresolved = resolution.tier == CursorTier::Fallback;
        if unresolved != self.last_unresolved {
            self.last_unresolved = unresolved;
            if unresolved {
                emit_diagnostic(CURSOR_UNRESOLVED_CODE);
            }
        }
        if resolution.scale_invalid != self.last_scale_invalid {
            self.last_scale_invalid = resolution.scale_invalid;
            if resolution.scale_invalid {
                emit_diagnostic(SCALE_INVALID_CODE);
            }
        }
    }

    /// Replaces the held layout and drops a cached anchor whose output is gone.
    fn adopt(&mut self, layout: ScreenLayout) {
        let held = self.cache.as_ref();
        let keeps_cache = held.is_some_and(|cached| layout.screen(cached.anchor.screen).is_some());
        if !keeps_cache {
            self.cache = None;
        }
        self.layout = Some(layout);
        // A layout change re-arms both diagnostics: whatever they last described, it was the layout
        // that just went away.
        self.last_unresolved = false;
        self.last_scale_invalid = false;
    }
}

/// The tier 1 heuristic: whether a caret rectangle is already screen-absolute.
///
/// The rule is `features.md` 2.5.3's: a rectangle whose centre point falls inside an output is
/// absolute. It cannot be exact, which is why it lives here alone -- revising it after the probe is
/// a one-function change.
fn looks_absolute(layout: &ScreenLayout, caret: RectI) -> Option<&ScreenInfo> {
    layout.screen_at(centre_of(caret))
}

/// Converts a client-reported caret rectangle into physical pixels.
///
/// Returns `None` when the coordinates cannot be trusted: a client that reports `INT_MIN` or a value
/// far outside any desktop is reporting "no caret", and the caller must fall back rather than scale
/// a sentinel into a plausible-looking position.
fn physical_caret(rect: FcitxCursorRect, scale: f32) -> Option<RectI> {
    if !is_plausible(rect.x) || !is_plausible(rect.y) {
        return None;
    }
    let x = to_i32(f64::from(rect.x) * f64::from(scale));
    let y = to_i32(f64::from(rect.y) * f64::from(scale));
    // A zero or negative size is the degenerate caret terminals and Electron clients report.
    let (w, h) = if rect.w <= 0 || rect.h <= 0 {
        (1, estimated_line_height(scale))
    } else {
        let w = to_u32(f64::from(rect.w) * f64::from(scale));
        let h = to_u32(f64::from(rect.h) * f64::from(scale));
        (w, h)
    };
    Some(RectI { x, y, w, h })
}

/// Translates a caret rectangle by a window origin.
///
/// Saturating: an origin from a backend that reported nonsense must not wrap the coordinate into a
/// value that looks plausible.
fn translated(caret: RectI, origin: WindowOrigin) -> RectI {
    let x = caret.x.saturating_add(origin.x);
    let y = caret.y.saturating_add(origin.y);
    RectI {
        x,
        y,
        w: caret.w,
        h: caret.h,
    }
}

/// The error for "the platform enumerated no output".
fn no_output_error() -> ImeError {
    ImeError::CompositorUnsupported {
        detail: String::from("screen layout has no output"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use super::*;

    use ime_types::Placement;

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
}
