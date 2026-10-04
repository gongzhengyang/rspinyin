//! Tests for the theme pipeline: the two palettes against the 3.2 table, the
//! contrast gate at every degradation tier, and the three property writes a theme
//! switch performs.
//!
//! The fixtures below are `pub(super)` because the `ui/theme.slint` parity harness in
//! the sibling `slint_palette` module resolves its requests through the same helpers,
//! so that both files compare one palette rather than two.
//!
//! Nothing here needs a display server or a Slint runtime: the palette is plain data,
//! the sink is a mock that records what it was asked to write, and the compositor is
//! a stub that answers "yes" or "no" on demand. That is what makes the degradation
//! ladder, which cannot be exercised on a machine whose compositor has no blur,
//! covered by an ordinary test run.
//!
//! The tests themselves are grouped by the part of the pipeline they pin, in the
//! files beside this one: `palette` holds the specification tables and the frozen
//! fractions, `contrast` the contrast gate, `resolve` the degradation ladder, `apply`
//! the property writes and the blur round trip, and `shadow` the window's shadow
//! material.

mod apply;
mod contrast;
mod palette;
mod resolve;
mod shadow;

use super::*;

pub(super) const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba8 {
    Rgba8 { r, g, b, a }
}

/// A theme request with acrylic on, the shipped base alpha and the scheme's accent.
pub(super) fn spec(scheme: ColorScheme) -> ThemeSpec {
    ThemeSpec {
        scheme,
        accent: default_accent(scheme),
        acrylic: true,
        base_alpha: DEFAULT_BASE_ALPHA,
        corner_radius_dp: 12,
        scale: 1.0,
    }
}

/// A theme request with every field under the caller's control.
pub(super) fn spec_with(
    scheme: ColorScheme,
    accent: Rgba8,
    acrylic: bool,
    base_alpha: u8,
) -> ThemeSpec {
    ThemeSpec {
        scheme,
        accent,
        acrylic,
        base_alpha,
        corner_radius_dp: 12,
        scale: 1.0,
    }
}

/// Resolves a request as if the compositor had answered `blur`.
pub(super) fn tokens_for(spec: &ThemeSpec, blur: BlurNegotiation) -> ThemeTokens {
    ThemeResolution::resolve(spec, blur).tokens
}
