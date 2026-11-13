//! Theme tokens: the palette of `features.md` 3.2, the light/dark decision and the
//! acrylic degradation ladder.
//!
//! Responsibility: turn a [`ime_types::ThemeSpec`] into the concrete colours the candidate
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
//! The shadow layers of §3.1.2 are the one exception, and it is a deliberate one: the
//! bands they are drawn as are a falloff rather than a token, no Rust code reads them,
//! and their only copy is the one `ui/theme.slint` computes, whose tests hold it to the
//! falloff -- and to each table's peak -- that 3.1.2 describes. Nothing on the drawing
//! path reads a peak: the ramp reaches the rasterizer as the per-band colours themselves.
//!
//! # Determinism
//!
//! Every token is an 8-bit integer. Alpha compositing is integer arithmetic with
//! round-half-up, which reproduces the worked examples in 3.2 exactly: `#1C1C1E` at
//! alpha 0.85 over white is `#3E3E40`, and `#FFFFFF` at 0.85 over black is `#D9D9D9`.
//! Floating point appears only in the contrast gate, and it only ever feeds a
//! threshold comparison; every call the palettes make sits orders of magnitude
//! further from its floor than a rounding step can reach, so no rounding difference
//! can move a decision. The same [`ime_types::ThemeSpec`] therefore produces the same tokens byte
//! for byte.
//!
//! # Agreement with `ui/theme.slint`
//!
//! The palette below is byte for byte what the Slint global computes, because the
//! self-check has to measure the colours the renderer will paint. Slint turns 3.2's
//! fractions into bytes two different ways: the `rgba()` builtin writes
//! `(255 * a) as u8`, which truncates, and `with-alpha()` writes `(a * 255) as u8`
//! after rounding. A fraction that is not an exact multiple of 1/255 can therefore
//! land one step below its rounded value -- `rgba(255,255,255,0.10)` is 25, not 26 --
//! and the tokens that take that path carry the truncated byte. `ui/theme.slint` keeps
//! the table's spelling, and `slint_palette.rs` evaluates its expressions with Slint's
//! own conversions, so the two copies are compared as bytes instead of being trusted to
//! stay in step.
//!
//! # The degradation ladder
//!
//! Transparency and blur degrade in the order `features.md` §0.5.2 fixes, and every
//! tier keeps text at or above [`CONTRAST_MINIMUM`]:
//!
//! | Tier | Base alpha | Diagnostic |
//! |---|---|---|
//! | The compositor blurs behind the window | `ui.base_alpha` (0.85), if the gate accepts it | `ui/theme/contrast-fallback` otherwise |
//! | Blur refused, or no way to ask | opaque | `ui/theme/blur-unavailable` |
//! | The user turned acrylic off | `ui.base_alpha` (0.85), if the gate accepts it | `ui/theme/contrast-fallback` otherwise |
//! | A colour pair turns out unreadable | opaque | `ui/theme/contrast-fallback` |
//!
//! The contrast gate measures the three pairs 3.2 requires at every tier, and the five
//! dimmed pairs the view actually draws -- the number label, the annotation, the
//! preedit separator and the two secondary runs -- while the base is translucent. The
//! shipped dimmed alphas (3.1.1's 0.55 and 0.50, plus the two tokens' own fractions) do
//! not clear 4.5:1 over a 0.85 base, so a translucent tier resolves through the last
//! row: the base ships opaque and the fallback is recorded -- the gate working, not a defect.
//!
//! Raising the base to opaque is the only automatic degradation the design allows: a
//! user whose accent makes a pair unreadable gets a darker base, never a rejected
//! configuration and never an unreadable window.

#[cfg(test)]
use ime_types::{ColorScheme, Rgba8, ThemeSpec};

mod blur;
mod color;
mod palette;
mod resolve;
mod scheme;
mod sink;
#[cfg(test)]
mod slint_palette;
#[cfg(test)]
mod tests;
mod tokens;

pub use blur::{
    BLUR_UNAVAILABLE, BlurDecision, BlurNegotiation, BlurSurface, request_blur, resolve_base_alpha,
};
pub use color::{
    BLACK, CONTRAST_BODY, CONTRAST_MINIMUM, ContrastReport, WHITE, composite_over, contrast_ratio,
    relative_luminance, with_alpha,
};
pub use resolve::{CONTRAST_FALLBACK, ThemeResolution};
pub use scheme::{
    DEFAULT_ACCENT_DARK, DEFAULT_ACCENT_LIGHT, PortalColorScheme, SchemeSignals, default_accent,
    resolve_accent, resolve_scheme,
};
pub use sink::{ThemeSink, apply};
pub use tokens::ThemeTokens;

/// The alpha that makes the base fully opaque; `ui.base_alpha`'s default is 0.85 of it.
pub const OPAQUE_ALPHA: u8 = 255;

/// The base the window paints while the compositor blurs behind it: 0.85 x 255.
pub const DEFAULT_BASE_ALPHA: u8 = 217;

/// The element opacity of a candidate cell's number label: 3.1.1's 0.55, fixed by the
/// specification's annotation note as the label's *effective* alpha. `ui/candidate_grid.slint`
/// spells the same fraction; the tests beside this file hold the two copies together.
/// Test-only because the pixels are drawn from [`NUMBER_LABEL_ALPHA`], the folded byte.
#[cfg(test)]
const NUMBER_LABEL_OPACITY: f32 = 0.55;

/// The candidate annotation's element opacity: 3.1.1's 0.50, same ruling as above.
#[cfg(test)]
const ANNOTATION_OPACITY: f32 = 0.50;

/// The fraction 3.2 fixes for the dark palette's `text.separator`; light ships 0.35.
#[cfg(test)]
const SEPARATOR_FRACTION_DARK: f32 = 0.40;

/// The fraction 3.2 fixes for the dark palette's `text.secondary` (light ships 0.60),
/// the token the mode label and the passthrough runs draw with.
#[cfg(test)]
const SECONDARY_FRACTION_DARK: f32 = 0.62;

/// The byte the renderer's opacity folding turns [`NUMBER_LABEL_OPACITY`] into:
/// `0.55 x 255` is 140.25, and `Color::with_alpha` rounds.
const NUMBER_LABEL_ALPHA: u8 = 140;

/// The byte the folding turns [`ANNOTATION_OPACITY`] into: `0.50 x 255` lands exactly on
/// the halfway point, which rounds away from zero to 128.
const ANNOTATION_ALPHA: u8 = 128;
