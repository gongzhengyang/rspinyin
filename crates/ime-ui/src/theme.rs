//! Theme tokens: the palette of `features.md` 3.2, the light/dark decision and the
//! acrylic degradation ladder.
//!
//! Responsibility: turn a [`ThemeSpec`] into the concrete colours the candidate
//! window paints with, and decide how opaque the base has to be for its text to stay
//! readable. Everything here is a pure function of its arguments -- no filesystem, no
//! clock, no display server, no global state -- which is what lets both palettes,
//! the contrast gate and every degradation tier be covered by an ordinary test run,
//! and what keeps a theme switch from being able to fail halfway through.
//!
//! Boundaries: this module decides *what* the colours are and nothing else. It does
//! not rasterize, it does not read the portal (the portal thread reads it and posts a
//! [`UiCommand::Theme`]), and it holds no Slint type in its public API: the write
//! side of the Slint global arrives as the [`ThemeSink`] trait, so the generated
//! `Theme` type stays private to the module that owns the generated bindings.
//!
//! [`UiCommand::Theme`]: ime_types::UiCommand::Theme
//!
//! # Where the palette lives
//!
//! `ui/theme.slint` carries the palette as the defaults of the Slint `Theme` global,
//! because that is the file `scripts/check-ui-spec.sh` compares against the 3.2 table
//! and because the window has to look right before the first [`apply`] runs. The Rust
//! side repeats it, because the contrast self-check has to evaluate the colours
//! without a renderer. The two copies are pinned to each other by the unit tests
//! beside this file and by that script; no third copy may exist.
//!
//! # Determinism
//!
//! Every token is an 8-bit integer. Alpha compositing is integer arithmetic with
//! round-half-up, which reproduces the worked examples in 3.2 exactly: `#1C1C1E` at
//! alpha 0.85 over white is `#3E3E40`, and `#FFFFFF` at 0.85 over black is `#D9D9D9`.
//! Floating point appears only in the contrast gate, which compares against a
//! threshold the closest pair clears by more than 20%, so no rounding difference can
//! move a decision. The same [`ThemeSpec`] therefore produces the same tokens byte
//! for byte.
//!
//! # The degradation ladder
//!
//! Transparency and blur degrade in the order `features.md` §0.5.2 fixes, and every
//! tier keeps text at or above [`CONTRAST_MINIMUM`]:
//!
//! | Tier | Base alpha | Diagnostic |
//! |---|---|---|
//! | The compositor blurs behind the window | `ui.base_alpha` (0.85) | none |
//! | Blur refused, or no way to ask | opaque | `ui/theme/blur-unavailable` |
//! | The user turned acrylic off | `ui.base_alpha` (0.85) | none |
//! | A colour pair turns out unreadable | opaque | `ui/theme/contrast-fallback` |
//!
//! Raising the base to opaque is the only automatic degradation the design allows: a
//! user whose accent makes a pair unreadable gets a darker base, never a rejected
//! configuration and never an unreadable window.

use ime_types::{ColorScheme, PlatformError, RectI, Rgba8, ThemeSpec};

mod color;
mod scheme;
#[cfg(test)]
mod tests;

pub use color::{BLACK, WHITE, composite_over, contrast_ratio, relative_luminance, with_alpha};
pub use scheme::{
    DEFAULT_ACCENT_DARK, DEFAULT_ACCENT_LIGHT, PortalColorScheme, SchemeSignals, default_accent,
    resolve_accent, resolve_scheme,
};

/// The contrast `text.primary` must reach against the opaque surface base (3.2).
pub const CONTRAST_BODY: f32 = 7.0;

/// The contrast every text/background pair must keep at every degradation tier
/// (`features.md` §0.5.2 and 3.2).
pub const CONTRAST_MINIMUM: f32 = 4.5;

/// The alpha that makes the base fully opaque.
pub const OPAQUE_ALPHA: u8 = 255;

/// The alpha the base is painted with while the compositor blurs behind the window:
/// `ui.base_alpha`'s default, 0.85 x 255.
pub const DEFAULT_BASE_ALPHA: u8 = 217;

/// Recorded when the compositor refuses the blur request. This is an expected
/// degradation, not a failure, and it is never shown to the user as an error.
pub const BLUR_UNAVAILABLE: &str = "ui/theme/blur-unavailable";

/// Recorded when the contrast self-check forces an opaque base.
pub const CONTRAST_FALLBACK: &str = "ui/theme/contrast-fallback";

/// What the compositor answered when the window asked it to blur what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurNegotiation {
    /// The compositor took the request, so the base may stay translucent.
    Applied,
    /// The compositor refused it, or the round trip did not answer in time. Failure
    /// is the expected outcome on most compositors and must never block the UI thread.
    Refused,
    /// No request was made, because the user turned acrylic off.
    Disabled,
}

/// How opaque the base is painted, and whether the compositor was the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurDecision {
    /// The alpha to paint `surface.base` with.
    pub alpha: u8,
    /// The compositor refused the request, so [`BLUR_UNAVAILABLE`] belongs in the
    /// diagnostics. A user-disabled acrylic is not reported.
    pub unavailable: bool,
}

/// Decides how opaque the base has to be, given the acrylic negotiation.
///
/// # Parameters
///
/// - `outcome`: what the compositor answered, or that the user turned acrylic off.
/// - `requested`: `ui.base_alpha`, the alpha to use when blur is available.
///
/// # Returns
///
/// The alpha to paint with, and whether the refusal should be reported.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn resolve_base_alpha(outcome: BlurNegotiation, requested: u8) -> BlurDecision {
    match outcome {
        BlurNegotiation::Applied | BlurNegotiation::Disabled => BlurDecision {
            alpha: requested,
            unavailable: false,
        },
        BlurNegotiation::Refused => BlurDecision {
            alpha: OPAQUE_ALPHA,
            unavailable: true,
        },
    }
}

/// A surface that can ask its compositor to blur what is behind the window.
///
/// The frozen `SurfaceBackend` trait has no blur request and this crate may not
/// change it, so the compositor-side round trip arrives as this separate capability:
/// the backend that can speak it implements it -- setting
/// `_KDE_NET_WM_BLUR_BEHIND_REGION` on KWin, relying on the namespace rule on
/// Hyprland -- and a backend that cannot ask simply does not, which the caller
/// negotiates as [`BlurNegotiation::Refused`].
///
/// # Concurrency
///
/// Implementations must not block: a request the compositor does not answer within
/// the negotiation budget reports [`PlatformError`] instead of waiting, because this
/// call happens on the UI thread and a stalled compositor must not stall the window.
/// The trait is not `Send`, since a surface is owned by the thread that created it.
pub trait BlurSurface {
    /// Asks the compositor to blur `region`, in physical pixels relative to the
    /// window's top-left corner.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] when the request cannot be delivered, which the
    /// caller reads as [`BlurNegotiation::Refused`] and degrades to an opaque base.
    fn request_blur(&mut self, region: &[RectI]) -> Result<(), PlatformError>;
}

/// Asks a surface to blur the region behind the window.
///
/// # Parameters
///
/// - `surface`: the window's backend, behind the [`BlurSurface`] capability.
/// - `region`: the container rectangle, in physical pixels.
///
/// # Returns
///
/// [`BlurNegotiation::Applied`] when the compositor took the request and
/// [`BlurNegotiation::Refused`] when it did not. The error itself is not propagated:
/// a compositor without blur is a supported configuration, not a failure.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn request_blur(surface: &mut dyn BlurSurface, region: &[RectI]) -> BlurNegotiation {
    if surface.request_blur(region).is_ok() {
        BlurNegotiation::Applied
    } else {
        BlurNegotiation::Refused
    }
}

/// Every colour the candidate window paints with, resolved for one scheme.
///
/// The fields are the tokens of `features.md` 3.2 in the order the table lists them,
/// plus [`ThemeTokens::surface_fill`], the composited base the panel is actually
/// filled with. All of them are 8-bit: no token depends on a floating-point rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeTokens {
    /// Whether the dark palette is in use.
    pub dark: bool,
    /// The alpha of the base, `0..=255`; [`OPAQUE_ALPHA`] when the compositor cannot
    /// blur behind the window.
    pub base_alpha: u8,
    /// 3.2 `surface.base`, opaque; the acrylic alpha lives in `surface_fill`.
    pub surface_base: Rgba8,
    /// 3.2 `surface.base` at [`ThemeTokens::base_alpha`]: the panel's fill.
    pub surface_fill: Rgba8,
    /// 3.2 `surface.stroke`: the container's 1dp border.
    pub surface_stroke: Rgba8,
    /// 3.2 `text.primary`: candidate text and the preedit line.
    pub text_primary: Rgba8,
    /// 3.2 `text.secondary`: the status strip.
    pub text_secondary: Rgba8,
    /// 3.2 `text.annotation`: the reading hint and the candidate number.
    pub text_annotation: Rgba8,
    /// 3.2 `text.separator`: the `'` between syllables.
    pub text_separator: Rgba8,
    /// 3.2 `accent.default`: the accent in use, opaque.
    pub accent: Rgba8,
    /// 3.2 `accent.on`: text on a solid accent fill. Reserved.
    pub accent_on: Rgba8,
    /// 3.2 `state.hover`: the pointer hovering a cell that is not selected.
    pub state_hover: Rgba8,
    /// 3.2 `state.selected.bg`: the first candidate and the keyboard highlight.
    pub state_selected_bg: Rgba8,
    /// 3.2 `state.selected.stroke`: the focus ring.
    pub state_selected_stroke: Rgba8,
    /// 3.2 `state.pressed`: the pointer held down on a cell.
    pub state_pressed: Rgba8,
    /// 3.2 `separator`: the line under the header.
    pub separator: Rgba8,
    /// 3.2 `shadow.inner`: the hard layer of §3.1.2 that keeps the edge crisp.
    pub shadow_inner: Rgba8,
    /// 3.2 `shadow.outer`: the soft layer of §3.1.2 that gives the window height.
    pub shadow_outer: Rgba8,
    /// 3.2 `status.dot.active`: the Chinese-mode indicator.
    pub status_dot_active: Rgba8,
    /// 3.2 `status.dot.idle`: the English-mode indicator.
    pub status_dot_idle: Rgba8,
}

impl ThemeTokens {
    /// The backdrop this palette's worst case is measured against: white for the dark
    /// palette and black for the light one, because that is the desktop colour that
    /// washes the translucent base out the most (3.2).
    ///
    /// # Returns
    ///
    /// [`WHITE`] or [`BLACK`].
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn worst_case_backdrop(&self) -> Rgba8 {
        if self.dark { WHITE } else { BLACK }
    }

    /// The three contrast pairs 3.2 requires, evaluated worst case.
    ///
    /// # Returns
    ///
    /// A report holding the three ratios; [`ContrastReport::passes`] says whether all
    /// of them clear their threshold.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn contrast(&self) -> ContrastReport {
        let backdrop = self.worst_case_backdrop();
        let fill = composite_over(self.surface_fill, backdrop);
        let selected = composite_over(self.state_selected_bg, fill);
        ContrastReport {
            primary_on_base: contrast_ratio(self.text_primary, self.surface_base),
            primary_on_fill: contrast_ratio(self.text_primary, fill),
            primary_on_selected: contrast_ratio(self.text_primary, selected),
        }
    }
}

/// The three contrast ratios a theme has to keep.
///
/// These are the pairs 3.2 names, and they are what makes "the acrylic degraded to an
/// opaque colour" a supported state rather than a worse-looking one: the base may
/// lose its transparency, but the text on it never loses its legibility.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContrastReport {
    /// `text.primary` on the opaque surface base; must reach [`CONTRAST_BODY`].
    pub primary_on_base: f32,
    /// `text.primary` on the base composited over the worst-case backdrop; must reach
    /// [`CONTRAST_MINIMUM`].
    pub primary_on_fill: f32,
    /// `text.primary` on the selected cell's background; must reach
    /// [`CONTRAST_MINIMUM`].
    pub primary_on_selected: f32,
}

impl ContrastReport {
    /// Whether all three pairs clear their threshold.
    ///
    /// # Returns
    ///
    /// `true` when the theme is readable and no degradation is needed.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn passes(&self) -> bool {
        self.primary_on_base >= CONTRAST_BODY
            && self.primary_on_fill >= CONTRAST_MINIMUM
            && self.primary_on_selected >= CONTRAST_MINIMUM
    }
}

/// A theme request resolved into tokens, with the degradations that were applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeResolution {
    /// The colours to paint with.
    pub tokens: ThemeTokens,
    /// The compositor refused the blur request and the base was forced opaque.
    pub blur_unavailable: bool,
    /// The contrast self-check failed and the base was raised to opaque.
    pub contrast_fallback: bool,
}

impl ThemeResolution {
    /// Resolves a theme request into the tokens the window paints with.
    ///
    /// # Parameters
    ///
    /// - `spec`: the host's theme request. `acrylic` decides whether `blur` is
    ///   consulted at all: with acrylic off the round trip was skipped, so the answer
    ///   is ignored and the base stays translucent.
    /// - `blur`: what the compositor answered.
    ///
    /// # Returns
    ///
    /// The tokens, plus the two degradation flags the caller reports as
    /// [`BLUR_UNAVAILABLE`] and [`CONTRAST_FALLBACK`]. The tokens are readable in
    /// every case, including a request that asked for a fully transparent base.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A theme request can be
    /// degenerate, but it can never fail -- there is always a palette to fall back on.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn resolve(spec: &ThemeSpec, blur: BlurNegotiation) -> Self {
        let palette = palette_for(spec.scheme);
        // An accent is an RGB choice. The alphas that make the selected cell and its
        // ring belong to the tokens below, so a translucent accent cannot silently
        // weaken them.
        let accent = with_alpha(spec.accent, OPAQUE_ALPHA);
        let outcome = if spec.acrylic {
            blur
        } else {
            BlurNegotiation::Disabled
        };
        let decision = resolve_base_alpha(outcome, spec.base_alpha);
        let tokens = palette.tokens(accent, decision.alpha);
        if tokens.contrast().passes() {
            return Self {
                tokens,
                blur_unavailable: decision.unavailable,
                contrast_fallback: false,
            };
        }
        // The one automatic degradation the design allows: raise the base to opaque
        // rather than refuse the user's accent or draw unreadable text.
        Self {
            tokens: palette.tokens(accent, OPAQUE_ALPHA),
            blur_unavailable: decision.unavailable,
            contrast_fallback: true,
        }
    }

    /// The diagnostic codes this resolution should record, in the order the
    /// degradations happened.
    ///
    /// # Returns
    ///
    /// Up to two codes; `None` for a degradation that did not happen. The caller
    /// writes them to the diagnostics channel -- this module does not log, because it
    /// has no subscriber and must stay a pure function.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn diagnostic_codes(&self) -> [Option<&'static str>; 2] {
        [
            self.blur_unavailable.then_some(BLUR_UNAVAILABLE),
            self.contrast_fallback.then_some(CONTRAST_FALLBACK),
        ]
    }
}

/// The window's theme global, as far as this module is concerned.
///
/// The Slint compiler generates the `Theme` global's Rust type, and a Slint type may
/// not appear in this crate's public API -- that is a condition of the royalty-free
/// licence, enforced in CI by the Slint-leak audit. The write side therefore arrives
/// as this trait: the module that owns the generated bindings implements it once, in
/// as many lines as there are properties, and every rule about *what* the colours are
/// stays here, where it is testable without a renderer.
pub trait ThemeSink {
    /// Writes the light/dark switch.
    ///
    /// # Parameters
    ///
    /// - `dark`: `true` for the dark palette.
    fn set_dark(&mut self, dark: bool);

    /// Writes the accent colour, which the selected-cell tokens derive from.
    ///
    /// # Parameters
    ///
    /// - `accent`: an opaque accent.
    fn set_accent(&mut self, accent: Rgba8);

    /// Writes the base alpha, `0.0..=1.0`.
    ///
    /// # Parameters
    ///
    /// - `alpha`: the fraction of the base that is painted; `1.0` is opaque.
    fn set_base_alpha(&mut self, alpha: f32);
}

/// Writes a resolved theme into the window's theme global.
///
/// Three properties are written and nothing else is touched: the Slint global derives
/// every other token from them, so a theme switch -- including the dark-to-light one
/// -- is a property update, never a rebuild, and cannot flash.
///
/// # Parameters
///
/// - `sink`: the window's theme global, behind [`ThemeSink`].
/// - `tokens`: the resolution to apply. Apply the tokens rather than the raw
///   [`ThemeSpec`], so the alpha written is the one the contrast gate approved.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn apply(sink: &mut dyn ThemeSink, tokens: &ThemeTokens) {
    sink.set_dark(tokens.dark);
    sink.set_accent(tokens.accent);
    sink.set_base_alpha(f32::from(tokens.base_alpha) / 255.0);
}

/// One scheme's palette, exactly as `features.md` 3.2 tabulates it.
///
/// The alphas are the table's fractions rounded into 8 bits with
/// `round(fraction * 255)`, which is the rounding Slint's `with-alpha` applies, so a
/// token means the same byte here and in `ui/theme.slint`.
#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
    surface_base: Rgba8,
    surface_stroke: Rgba8,
    text_primary: Rgba8,
    text_secondary: Rgba8,
    text_annotation: Rgba8,
    text_separator: Rgba8,
    accent_on: Rgba8,
    state_hover: Rgba8,
    state_pressed: Rgba8,
    separator: Rgba8,
    shadow_inner: Rgba8,
    shadow_outer: Rgba8,
    status_dot_idle: Rgba8,
    /// `state.selected.bg`: the accent at 0.18 in the dark palette, 0.14 in the light.
    selected_bg_alpha: u8,
    /// `state.selected.stroke`: the accent at 0.55 dark, 0.50 light.
    selected_stroke_alpha: u8,
}

/// The dark column of the 3.2 table.
const DARK_PALETTE: Palette = Palette {
    dark: true,
    // 3.2 `surface.base`: #1C1C1E, opaque here; the acrylic alpha is `base-alpha`.
    surface_base: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `surface.stroke`: rgba(255,255,255,0.10).
    surface_stroke: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: 26,
    },
    // 3.2 `text.primary`: #F2F2F7.
    text_primary: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `text.secondary`: rgba(242,242,247,0.62).
    text_secondary: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 158,
    },
    // 3.2 `text.annotation`: rgba(242,242,247,0.48).
    text_annotation: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 122,
    },
    // 3.2 `text.separator`: rgba(242,242,247,0.40).
    text_separator: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 102,
    },
    // 3.2 `accent.on`: #FFFFFF.
    accent_on: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `state.hover`: rgba(242,242,247,0.08).
    state_hover: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 20,
    },
    // 3.2 `state.pressed`: rgba(242,242,247,0.14).
    state_pressed: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 36,
    },
    // 3.2 `separator`: rgba(242,242,247,0.10).
    separator: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 26,
    },
    // 3.2 `shadow.inner`: rgba(0,0,0,0.35).
    shadow_inner: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 89,
    },
    // 3.2 `shadow.outer`: rgba(0,0,0,0.42).
    shadow_outer: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 107,
    },
    // 3.2 `status.dot.idle`: rgba(242,242,247,0.35).
    status_dot_idle: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 89,
    },
    // 3.2 `state.selected.bg`: accent @ 0.18.
    selected_bg_alpha: 46,
    // 3.2 `state.selected.stroke`: accent @ 0.55.
    selected_stroke_alpha: 140,
};

/// The light column of the 3.2 table.
const LIGHT_PALETTE: Palette = Palette {
    dark: false,
    // 3.2 `surface.base`: #FFFFFF.
    surface_base: Rgba8 {
        r: 0xFF,
        g: 0xFF,
        b: 0xFF,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `surface.stroke`: rgba(0,0,0,0.06).
    surface_stroke: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 15,
    },
    // 3.2 `text.primary`: #1C1C1E.
    text_primary: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `text.secondary`: rgba(28,28,30,0.60).
    text_secondary: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 153,
    },
    // 3.2 `text.annotation`: rgba(28,28,30,0.45).
    text_annotation: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 115,
    },
    // 3.2 `text.separator`: rgba(28,28,30,0.35).
    text_separator: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 89,
    },
    // 3.2 `accent.on`: #FFFFFF.
    accent_on: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `state.hover`: rgba(28,28,30,0.06).
    state_hover: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 15,
    },
    // 3.2 `state.pressed`: rgba(28,28,30,0.12).
    state_pressed: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 31,
    },
    // 3.2 `separator`: rgba(28,28,30,0.08).
    separator: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 20,
    },
    // 3.2 `shadow.inner`: rgba(0,0,0,0.08).
    shadow_inner: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 20,
    },
    // 3.2 `shadow.outer`: rgba(0,0,0,0.16).
    shadow_outer: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 41,
    },
    // 3.2 `status.dot.idle`: rgba(28,28,30,0.30).
    status_dot_idle: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 77,
    },
    // 3.2 `state.selected.bg`: accent @ 0.14.
    selected_bg_alpha: 36,
    // 3.2 `state.selected.stroke`: accent @ 0.50.
    selected_stroke_alpha: 128,
};

/// The palette of a scheme.
fn palette_for(scheme: ColorScheme) -> Palette {
    match scheme {
        ColorScheme::Dark => DARK_PALETTE,
        ColorScheme::Light => LIGHT_PALETTE,
    }
}

impl Palette {
    /// Builds the token set for an accent and a base alpha.
    ///
    /// The four tokens the table derives from the accent -- the selected cell's
    /// background and ring, and the two status dots -- are computed here, in the one
    /// place, and mirrored by the expressions in `ui/theme.slint`.
    fn tokens(self, accent: Rgba8, base_alpha: u8) -> ThemeTokens {
        ThemeTokens {
            dark: self.dark,
            base_alpha,
            surface_base: self.surface_base,
            surface_fill: with_alpha(self.surface_base, base_alpha),
            surface_stroke: self.surface_stroke,
            text_primary: self.text_primary,
            text_secondary: self.text_secondary,
            text_annotation: self.text_annotation,
            text_separator: self.text_separator,
            accent,
            accent_on: self.accent_on,
            state_hover: self.state_hover,
            state_selected_bg: with_alpha(accent, self.selected_bg_alpha),
            state_selected_stroke: with_alpha(accent, self.selected_stroke_alpha),
            state_pressed: self.state_pressed,
            separator: self.separator,
            shadow_inner: self.shadow_inner,
            shadow_outer: self.shadow_outer,
            status_dot_active: accent,
            status_dot_idle: self.status_dot_idle,
        }
    }
}
