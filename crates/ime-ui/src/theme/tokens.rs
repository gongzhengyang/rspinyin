//! The token set: [`ThemeTokens`], the resolved colours one window paints with,
//! and the worst-case contrast report the resolution gates its tiers on.

use ime_types::Rgba8;

use super::color::{BLACK, ContrastReport, WHITE, composite_over, contrast_ratio, with_alpha};
use super::{ANNOTATION_ALPHA, NUMBER_LABEL_ALPHA};

/// Every colour the candidate window paints with, resolved for one scheme.
///
/// The fields are the tokens of `features.md` 3.2 in the order the table lists them,
/// plus [`ThemeTokens::surface_fill`], the composited base the panel is actually
/// filled with. All of them are 8-bit: no token depends on a floating-point rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeTokens {
    /// Whether the dark palette is in use.
    pub dark: bool,
    /// The alpha of the base, `0..=255`; [`super::OPAQUE_ALPHA`] when the compositor cannot
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
    /// 3.2 `text.annotation`: the read-only lock's tint (the status strip's lock
    /// marker, the `lock-color` bindings). Its former annotation-text role draws from
    /// `text_primary` at a folded element alpha.
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
    /// 3.2 `shadow.inner`: the hard layer of §3.1.2 that keeps the edge crisp. The layer
    /// is drawn as the two bands `ui/theme.slint` declares in `shadow-inner-bands`, each
    /// at a fraction of this token's alpha.
    pub shadow_inner: Rgba8,
    /// 3.2 `shadow.outer`: the soft layer of §3.1.2 that gives the window height. The
    /// layer is drawn as the eight bands `ui/theme.slint` declares in
    /// `shadow-outer-bands`, whose alphas are the layer's falloff; this token supplies
    /// the three channels they are painted in.
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

    /// The eight contrast pairs the window paints, evaluated worst case.
    ///
    /// The first three are the pairs 3.2 requires. The other five are the dimmed text
    /// the view actually draws: the number label and the annotation at the element
    /// opacities 3.1.1 freezes, and the preedit separator and the two secondary runs at
    /// their tokens' own alphas. Each dimmed colour is flattened over the background it
    /// draws on before it is measured, which is the value the renderer commits.
    ///
    /// # Returns
    ///
    /// A report holding the eight ratios; [`ContrastReport::passes`] says whether the
    /// three required pairs clear their thresholds, and
    /// [`ContrastReport::dimmed_pairs_pass`] whether the dimmed five clear theirs.
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
            number_on_fill: contrast_ratio(with_alpha(self.text_primary, NUMBER_LABEL_ALPHA), fill),
            annotation_on_fill: contrast_ratio(
                with_alpha(self.text_primary, ANNOTATION_ALPHA),
                fill,
            ),
            separator_on_fill: contrast_ratio(self.text_separator, fill),
            mode_label_on_fill: contrast_ratio(self.text_secondary, fill),
            passthrough_on_fill: contrast_ratio(self.text_secondary, fill),
        }
    }
}
