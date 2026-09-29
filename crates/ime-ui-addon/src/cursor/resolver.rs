//! The resolution ladder, the cache that keeps the window still, and the diagnostics.
//!
//! Responsibility: normalise a client-reported caret rectangle into physical pixels, try
//! tier 1, tier 2 and tier 3 in order, and report which rung answered. The cache is what
//! keeps the candidate window still: an anchor the caret has barely moved away from is
//! reused, and a caret that cannot be located at all keeps the previous position instead
//! of jumping to the screen centre.
//!
//! Boundaries: the resolver owns no connection and no clock. The two platform-facing
//! pieces it needs are injected ([`ScreenEnumerator`], [`WindowGeometrySource`]), which
//! is what lets it be driven entirely from memory in a test. It never enumerates an
//! output on the resolve path -- [`CursorResolver::refresh_layout`] is the only entry
//! point that does -- and it records each diagnostic once per transition into the
//! condition it names rather than once per frame.

use ime_types::{Anchor, ImeError, RectI, ScreenId};

use crate::ffi::{FcitxCursorRect, emit_diagnostic};
use crate::screen::{
    ScreenEnumerator, ScreenInfo, ScreenLayout, ScriptedScreens, centre_of, check_scale,
    estimated_line_height, is_jitter, is_plausible, screen_id_at, to_i32, to_u32,
};

use super::IcId;
use super::sources::{UnknownWindowGeometry, WindowGeometrySource, WindowOrigin};

/// Recorded when no source could place the caret and the fallback position was used.
pub const CURSOR_UNRESOLVED_CODE: &str = "platform/cursor/unresolved";

/// Recorded when a scale factor outside `[1.0, 3.0]` was replaced with `1.0`.
pub const SCALE_INVALID_CODE: &str = "platform/scale/invalid";

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
pub(super) fn looks_absolute(layout: &ScreenLayout, caret: RectI) -> Option<&ScreenInfo> {
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
