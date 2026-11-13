//! The degradation ladder: [`ThemeResolution`], which resolves a theme request and
//! the blur answer into the tokens the window paints with, raising the base to
//! opaque when the contrast gate demands it and recording what degraded.

use ime_types::ThemeSpec;

use super::OPAQUE_ALPHA;
use super::blur::{BLUR_UNAVAILABLE, BlurNegotiation, resolve_base_alpha};
use super::color::with_alpha;
use super::palette::palette_for;
use super::tokens::ThemeTokens;

/// Recorded when the contrast self-check forces an opaque base.
pub const CONTRAST_FALLBACK: &str = "ui/theme/contrast-fallback";

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
    ///   is ignored and the base keeps the configured alpha unless the contrast gate
    ///   raises it.
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
        let report = tokens.contrast();
        // The three pairs 3.2 requires must hold at every tier. While the base is
        // translucent the five dimmed pairs must hold too: they are the text the user
        // reads most, and the wash of a translucent base is what erodes them first. At
        // the opaque tier the dimmed pairs sit at the alphas the design tables fix, and
        // opaque is the last rung the ladder can raise to, so there they are reported
        // rather than gated.
        let readable =
            report.passes() && (decision.alpha == OPAQUE_ALPHA || report.dimmed_pairs_pass());
        if readable {
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
