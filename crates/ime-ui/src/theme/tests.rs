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

/// Records what a theme switch wrote, so "only properties change" can be asserted.
#[derive(Debug, Default, PartialEq)]
struct RecordingSink {
    dark: Option<bool>,
    accent: Option<Rgba8>,
    base_alpha: Option<f32>,
    /// The family the sink was told to draw with.
    ///
    /// `theme::apply` never calls `set_font_family` -- the family is chosen once at startup
    /// and does not change with the theme -- so this stays `None` in every `apply` test and
    /// a non-`None` value here would be the failure.
    font_family: Option<String>,
    writes: usize,
}

impl ThemeSink for RecordingSink {
    fn set_dark(&mut self, dark: bool) {
        self.dark = Some(dark);
        self.writes += 1;
    }

    fn set_accent(&mut self, accent: Rgba8) {
        self.accent = Some(accent);
        self.writes += 1;
    }

    fn set_base_alpha(&mut self, alpha: f32) {
        self.base_alpha = Some(alpha);
        self.writes += 1;
    }

    fn set_font_family(&mut self, family: &str) {
        self.font_family = Some(String::from(family));
        self.writes += 1;
    }
}

/// A compositor that either takes the blur request or refuses it.
struct StubSurface {
    accepts: bool,
    requests: usize,
}

impl StubSurface {
    fn new(accepts: bool) -> Self {
        Self {
            accepts,
            requests: 0,
        }
    }
}

impl BlurSurface for StubSurface {
    fn request_blur(&mut self, region: &[RectI]) -> Result<(), PlatformError> {
        self.requests += 1;
        if self.accepts && !region.is_empty() {
            Ok(())
        } else {
            Err(PlatformError::Unavailable)
        }
    }
}

#[test]
fn test_dark_palette_matches_the_specification_table() {
    // The alphas that `rgba()` builds are the truncated byte, not the rounded one:
    // `surface-stroke`, `state-pressed` and `separator` are a step below what rounding
    // 0.10, 0.14 and 0.10 would give, because that is the byte Slint paints with.
    //
    // The shipped request is acrylic at 0.85, and the dimmed-pair gate resolves it to the
    // opaque tier -- the separator pair sits under the 4.5 floor on every translucent
    // base the palette can produce -- so the bytes below are the resolved, opaque ones:
    // the 217 `surface-fill` of the design table is dormant until the palette itself
    // changes, which is a specification decision and not a code change.
    let tokens = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);

    assert!(tokens.dark);
    assert_eq!(tokens.base_alpha, 255);
    assert_eq!(tokens.surface_base, rgba(0x1C, 0x1C, 0x1E, 255));
    assert_eq!(tokens.surface_fill, rgba(0x1C, 0x1C, 0x1E, 255));
    assert_eq!(tokens.surface_stroke, rgba(255, 255, 255, 25));
    assert_eq!(tokens.text_primary, rgba(0xF2, 0xF2, 0xF7, 255));
    assert_eq!(tokens.text_secondary, rgba(0xF2, 0xF2, 0xF7, 158));
    assert_eq!(tokens.text_annotation, rgba(0xF2, 0xF2, 0xF7, 122));
    assert_eq!(tokens.text_separator, rgba(0xF2, 0xF2, 0xF7, 102));
    assert_eq!(tokens.accent, DEFAULT_ACCENT_DARK);
    assert_eq!(tokens.accent_on, rgba(255, 255, 255, 255));
    assert_eq!(tokens.state_hover, rgba(0xF2, 0xF2, 0xF7, 20));
    assert_eq!(tokens.state_selected_bg, rgba(0x4C, 0x9A, 0xFF, 46));
    assert_eq!(tokens.state_selected_stroke, rgba(0x4C, 0x9A, 0xFF, 140));
    assert_eq!(tokens.state_pressed, rgba(0xF2, 0xF2, 0xF7, 35));
    assert_eq!(tokens.separator, rgba(0xF2, 0xF2, 0xF7, 25));
    assert_eq!(tokens.shadow_inner, rgba(0, 0, 0, 89));
    assert_eq!(tokens.shadow_outer, rgba(0, 0, 0, 107));
    assert_eq!(tokens.status_dot_active, DEFAULT_ACCENT_DARK);
    assert_eq!(tokens.status_dot_idle, rgba(0xF2, 0xF2, 0xF7, 89));
}

#[test]
fn test_light_palette_matches_the_specification_table() {
    // As in the dark palette, the `rgba()` tokens carry the truncated byte:
    // `text-annotation`, `state-pressed`, `shadow-outer` and `status-dot-idle`. And as
    // there, the dimmed-pair gate resolves the shipped acrylic request to the opaque
    // tier -- every light dimmed pair is under the floor on a translucent base -- so the
    // 217 `surface-fill` of the design table is dormant until the palette changes.
    let tokens = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied);

    assert!(!tokens.dark);
    assert_eq!(tokens.base_alpha, 255);
    assert_eq!(tokens.surface_base, rgba(0xFF, 0xFF, 0xFF, 255));
    assert_eq!(tokens.surface_fill, rgba(0xFF, 0xFF, 0xFF, 255));
    assert_eq!(tokens.surface_stroke, rgba(0, 0, 0, 15));
    assert_eq!(tokens.text_primary, rgba(0x1C, 0x1C, 0x1E, 255));
    assert_eq!(tokens.text_secondary, rgba(0x1C, 0x1C, 0x1E, 153));
    assert_eq!(tokens.text_annotation, rgba(0x1C, 0x1C, 0x1E, 114));
    assert_eq!(tokens.text_separator, rgba(0x1C, 0x1C, 0x1E, 89));
    assert_eq!(tokens.accent, DEFAULT_ACCENT_LIGHT);
    assert_eq!(tokens.accent_on, rgba(255, 255, 255, 255));
    assert_eq!(tokens.state_hover, rgba(0x1C, 0x1C, 0x1E, 15));
    assert_eq!(tokens.state_selected_bg, rgba(0x0A, 0x6C, 0xFF, 36));
    assert_eq!(tokens.state_selected_stroke, rgba(0x0A, 0x6C, 0xFF, 128));
    assert_eq!(tokens.state_pressed, rgba(0x1C, 0x1C, 0x1E, 30));
    assert_eq!(tokens.separator, rgba(0x1C, 0x1C, 0x1E, 20));
    assert_eq!(tokens.shadow_inner, rgba(0, 0, 0, 20));
    assert_eq!(tokens.shadow_outer, rgba(0, 0, 0, 40));
    assert_eq!(tokens.status_dot_active, DEFAULT_ACCENT_LIGHT);
    assert_eq!(tokens.status_dot_idle, rgba(0x1C, 0x1C, 0x1E, 76));
}

#[test]
fn test_worst_case_backdrop_follows_the_scheme() {
    let dark = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);
    let light = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied);

    assert_eq!(dark.worst_case_backdrop(), WHITE);
    assert_eq!(light.worst_case_backdrop(), BLACK);
}

#[test]
fn test_surface_fill_carries_the_base_alpha_and_nothing_else() {
    // A translucent base never reaches the tokens any more -- the dimmed-pair gate
    // degrades the request to opaque first -- so the reachable assertion is that the
    // fill follows the base's *channels* and that both land opaque; the alpha pass-through
    // itself is pinned by `slint_palette`'s own byte tests, which read the mapping
    // without the resolution in the way.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 200);
    let tokens = tokens_for(&spec, BlurNegotiation::Applied);

    assert_eq!(tokens.surface_fill.r, tokens.surface_base.r);
    assert_eq!(tokens.surface_fill.g, tokens.surface_base.g);
    assert_eq!(tokens.surface_fill.b, tokens.surface_base.b);
    assert_eq!(tokens.surface_fill.a, OPAQUE_ALPHA);
    assert_eq!(tokens.surface_base.a, OPAQUE_ALPHA);
}

#[test]
fn test_contrast_dark_palette_clears_the_specification_thresholds() {
    // The three pairs 3.2 requires hold on the acrylic tier's own report; the five
    // dimmed pairs the view draws are measured beside them and gated by the resolution,
    // which is the subject of the degradation tests below.
    let report = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied).contrast();

    assert!(
        report.primary_on_base >= CONTRAST_BODY,
        "primary on base was {}",
        report.primary_on_base
    );
    assert!(
        report.primary_on_fill >= CONTRAST_MINIMUM,
        "primary on fill was {}",
        report.primary_on_fill
    );
    assert!(
        report.primary_on_selected >= CONTRAST_MINIMUM,
        "primary on selected was {}",
        report.primary_on_selected
    );
    assert!(report.passes());
}

#[test]
fn test_contrast_light_palette_clears_the_specification_thresholds() {
    let report = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied).contrast();

    assert!(
        report.primary_on_base >= CONTRAST_BODY,
        "primary on base was {}",
        report.primary_on_base
    );
    assert!(
        report.primary_on_fill >= CONTRAST_MINIMUM,
        "primary on fill was {}",
        report.primary_on_fill
    );
    assert!(
        report.primary_on_selected >= CONTRAST_MINIMUM,
        "primary on selected was {}",
        report.primary_on_selected
    );
    assert!(report.passes());
}

/// Asserts that a palette stays readable for every accent the corners of the RGB cube
/// can produce, which is the widest a user's accent choice can be.
fn assert_every_accent_corner_is_readable(scheme: ColorScheme) {
    for red in [0, 255] {
        for green in [0, 255] {
            for blue in [0, 255] {
                let accent = rgba(red, green, blue, OPAQUE_ALPHA);
                let spec = spec_with(scheme, accent, true, DEFAULT_BASE_ALPHA);
                let report = tokens_for(&spec, BlurNegotiation::Applied).contrast();
                assert!(
                    report.passes(),
                    "{scheme:?} palette with accent {accent:?} scored {report:?}"
                );
            }
        }
    }
}

#[test]
fn test_contrast_dark_palette_survives_every_accent_corner() {
    assert_every_accent_corner_is_readable(ColorScheme::Dark);
}

#[test]
fn test_contrast_light_palette_survives_every_accent_corner() {
    assert_every_accent_corner_is_readable(ColorScheme::Light);
}

/// `ui/candidate_grid.slint`, as text: the cells and their dimmed text.
const SLINT_GRID: &str = include_str!("../../ui/candidate_grid.slint");

/// The element opacities `ui/candidate_grid.slint` folds into its dimmed text, in
/// declaration order: the fraction each `opacity: <fraction> * dim` binding spells.
fn grid_dimmed_opacities(source: &str) -> Vec<f32> {
    source
        .lines()
        .filter_map(|line| {
            let text = line.split("//").next().unwrap_or_default().trim();
            let rest = text
                .strip_prefix("opacity:")?
                .trim()
                .strip_suffix(';')?
                .trim();
            let (fraction, factor) = rest.split_once('*')?;
            if factor.trim() != "dim" {
                return None;
            }
            fraction.trim().parse::<f32>().ok()
        })
        .collect()
}

#[test]
fn test_view_dimmed_text_opacities_are_the_frozen_fractions() {
    // The gate measures the number label and the annotation at the fractions 3.1.1
    // freezes; the view spells the same fractions as element opacities. One contract,
    // two copies: a fraction that moved on either side alone would have the gate measure
    // a text the window does not draw, so the test holds the bindings to the constants.
    assert_eq!(
        grid_dimmed_opacities(SLINT_GRID),
        vec![NUMBER_LABEL_OPACITY, ANNOTATION_OPACITY]
    );
}

#[test]
fn test_contrast_dimmed_alphas_round_the_frozen_fractions() {
    // The renderer folds an element opacity into the colour's alpha the way
    // `Color::with_alpha` does -- by rounding the fraction against 255 -- so the bytes
    // the gate flattens with are the rounded fractions, not truncations of them.
    assert_eq!(NUMBER_LABEL_ALPHA, rounded_alpha(NUMBER_LABEL_OPACITY));
    assert_eq!(ANNOTATION_ALPHA, rounded_alpha(ANNOTATION_OPACITY));
}

#[test]
fn test_dark_tokens_carry_the_frozen_separator_and_secondary_fractions() {
    // The preedit separator and the two secondary runs draw the tokens' own alphas, and
    // the dark column's fractions are the design's 0.40 and 0.62, which Slint's `rgba()`
    // truncates into the bytes the palette stores. The light column ships its own
    // fractions (0.35 and 0.60), so this pin is per column, and the parity tests hold
    // both columns to `ui/theme.slint`.
    let dark = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);
    assert_eq!(
        dark.text_separator.a,
        (SEPARATOR_FRACTION_DARK * 255.0) as u8
    );
    assert_eq!(
        dark.text_secondary.a,
        (SECONDARY_FRACTION_DARK * 255.0) as u8
    );
}

#[test]
fn test_contrast_acrylic_fill_puts_the_dark_dimmed_pairs_under_the_floor() {
    // At the shipped 0.85 base the dark palette's composited fill is barely lighter than
    // its opaque fill -- the panel's own fill token dominates the composite -- so the
    // number label (5.42) and the annotation (4.75) clear the 4.5 floor exactly as they do
    // on the opaque tier, and the separator (3.55) is the pair that drags the dark acrylic
    // tier down the degradation ladder. The spread is why the gate measures the pairs one
    // by one instead of degrading on the weakest alone.
    let report = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied).contrast();

    assert!(report.separator_on_fill < CONTRAST_MINIMUM);
    assert!(report.number_on_fill >= CONTRAST_MINIMUM);
    assert!(report.annotation_on_fill >= CONTRAST_MINIMUM);
    assert!(report.mode_label_on_fill >= CONTRAST_MINIMUM);
    assert!(
        report.number_on_fill > report.annotation_on_fill,
        "the number label draws brighter than the annotation"
    );
    assert!(
        report.annotation_on_fill > report.separator_on_fill,
        "and the annotation brighter than the separator"
    );
    assert_eq!(
        report.mode_label_on_fill, report.passthrough_on_fill,
        "both secondary runs draw the same token today"
    );
}

#[test]
fn test_contrast_acrylic_fill_puts_the_light_dimmed_pairs_under_the_floor() {
    // The light palette is washed towards white, where its near-black dimmed text fares
    // no better: every one of the five pairs sits under the floor at the shipped base,
    // which is what sends the light acrylic tier down the degradation ladder too.
    let report = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied).contrast();

    assert!(report.number_on_fill < CONTRAST_MINIMUM);
    assert!(report.annotation_on_fill < CONTRAST_MINIMUM);
    assert!(report.separator_on_fill < CONTRAST_MINIMUM);
    assert!(report.mode_label_on_fill < CONTRAST_MINIMUM);
    assert!(report.passthrough_on_fill < CONTRAST_MINIMUM);
}

#[test]
fn test_contrast_opaque_tier_measures_the_dimmed_pairs_at_their_design_values() {
    // On an opaque base the dimmed pairs sit exactly where the design tables put them:
    // on the dark palette the number label and the annotation clear the 4.5 floor, as
    // the specification's annotation note requires, while the separator and every light
    // palette pair are auxiliary strokes whose fixed alphas no base alpha can raise. The
    // report pins the measured values so a palette edit moves one of them as a decision,
    // never as a drift.
    let dark = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Refused).contrast();
    assert!(
        (dark.number_on_fill - 5.42).abs() < 0.1,
        "the dark number label measured {}",
        dark.number_on_fill
    );
    assert!(
        (dark.annotation_on_fill - 4.76).abs() < 0.1,
        "the dark annotation measured {}",
        dark.annotation_on_fill
    );
    assert!(
        (dark.separator_on_fill - 3.55).abs() < 0.1,
        "the dark separator measured {}",
        dark.separator_on_fill
    );
    assert!(
        (dark.mode_label_on_fill - 6.6).abs() < 0.1,
        "the dark secondary runs measured {}",
        dark.mode_label_on_fill
    );
    assert_eq!(dark.mode_label_on_fill, dark.passthrough_on_fill);
    assert!(dark.number_on_fill >= CONTRAST_MINIMUM);
    assert!(dark.annotation_on_fill >= CONTRAST_MINIMUM);
    assert!(dark.separator_on_fill < CONTRAST_MINIMUM);

    let light = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Refused).contrast();
    assert!(
        (light.number_on_fill - 3.84).abs() < 0.1,
        "the light number label measured {}",
        light.number_on_fill
    );
    assert!(
        (light.annotation_on_fill - 3.32).abs() < 0.1,
        "the light annotation measured {}",
        light.annotation_on_fill
    );
    assert!(
        (light.separator_on_fill - 2.17).abs() < 0.1,
        "the light separator measured {}",
        light.separator_on_fill
    );
    assert!(
        (light.mode_label_on_fill - 4.47).abs() < 0.1,
        "the light secondary runs measured {}",
        light.mode_label_on_fill
    );
    assert_eq!(light.mode_label_on_fill, light.passthrough_on_fill);
    assert!(light.number_on_fill < CONTRAST_MINIMUM);
    assert!(light.annotation_on_fill < CONTRAST_MINIMUM);
    assert!(light.separator_on_fill < CONTRAST_MINIMUM);
}

#[test]
fn test_resolve_base_alpha_keeps_the_requested_alpha_when_blur_is_applied() {
    let decision = resolve_base_alpha(BlurNegotiation::Applied, 217);

    assert_eq!(decision.alpha, 217);
    assert!(!decision.unavailable);
}

#[test]
fn test_resolve_base_alpha_forces_opaque_when_blur_is_refused() {
    let decision = resolve_base_alpha(BlurNegotiation::Refused, 217);

    assert_eq!(decision.alpha, OPAQUE_ALPHA);
    assert!(decision.unavailable);
}

#[test]
fn test_resolve_base_alpha_treats_disabled_acrylic_as_the_user_s_choice() {
    let decision = resolve_base_alpha(BlurNegotiation::Disabled, 217);

    assert_eq!(decision.alpha, 217);
    assert!(
        !decision.unavailable,
        "a user who turned acrylic off must not be told blur is unavailable"
    );
}

#[test]
fn test_resolve_forces_the_base_opaque_when_the_compositor_refuses_blur() {
    let resolution = ThemeResolution::resolve(&spec(ColorScheme::Dark), BlurNegotiation::Refused);

    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert!(resolution.blur_unavailable);
    assert!(!resolution.contrast_fallback);
    assert_eq!(
        resolution.diagnostic_codes(),
        [Some(BLUR_UNAVAILABLE), None]
    );
}

#[test]
fn test_resolve_ignores_the_blur_answer_when_acrylic_is_off() {
    // A caller that skipped the round trip and passed `Refused` by mistake must not be
    // able to report the compositor as the reason: with acrylic off no compositor was
    // ever asked. The translucent base the user kept is still gated on the dimmed pairs,
    // so the resolution may degrade -- but as a contrast fallback, never as a blur
    // refusal.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, false, 217);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Refused);

    assert!(!resolution.blur_unavailable);
    assert!(resolution.contrast_fallback);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(
        resolution.diagnostic_codes(),
        [None, Some(CONTRAST_FALLBACK)]
    );
}

#[test]
fn test_resolve_user_disabled_acrylic_still_gates_the_dimmed_pairs() {
    // Keeping the base translucent without blur does not keep it ungated: the wash a
    // 0.85 base adds is the same whether or not a compositor was asked to blur behind
    // it, so the dimmed pairs decide the tier either way.
    let spec = spec_with(
        ColorScheme::Light,
        DEFAULT_ACCENT_LIGHT,
        false,
        DEFAULT_BASE_ALPHA,
    );
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Disabled);

    assert!(resolution.contrast_fallback);
    assert!(!resolution.blur_unavailable);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert!(resolution.tokens.contrast().passes());
}

#[test]
fn test_resolve_opaque_base_skips_the_dimmed_pair_gate() {
    // A base pinned at full opacity is the ladder's last rung: the dimmed pairs sit at
    // the alphas the design tables fix there, and there is nothing above opaque to raise,
    // so the gate reports them without acting on them. Skipping the gate is what keeps
    // the ladder total -- every resolution that comes back has cleared it.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, OPAQUE_ALPHA);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Applied);

    assert!(!resolution.contrast_fallback);
    assert!(!resolution.blur_unavailable);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert!(
        !resolution.tokens.contrast().dimmed_pairs_pass(),
        "the dark separator sits under the floor even on the opaque base"
    );
}

#[test]
fn test_resolve_degrades_the_shipped_acrylic_default_for_the_dimmed_pairs() {
    // The shipped default is acrylic at 0.85. The dimmed text the view draws -- the
    // number label, the annotation and the preedit separator -- measures under the 4.5
    // floor against the worst-case fill that base produces, so the gate degrades a
    // default that only used to be measured on the three full-alpha pairs. What ships
    // is the opaque palette, and the degradation is recorded.
    let resolution = ThemeResolution::resolve(&spec(ColorScheme::Dark), BlurNegotiation::Applied);

    assert!(resolution.contrast_fallback);
    assert!(!resolution.blur_unavailable);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(
        resolution.diagnostic_codes(),
        [None, Some(CONTRAST_FALLBACK)]
    );
    assert!(resolution.tokens.contrast().passes());
}

#[test]
fn test_resolve_acrylic_tier_triggers_the_contrast_fallback_in_both_schemes() {
    // The dark palette's dimmed pairs miss the floor three out of five and the light
    // palette's miss it five out of five, so either scheme's acrylic tier ends on the
    // ladder's last rung. What lands there is readable, which is the property the gate
    // exists to guarantee.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let resolution = ThemeResolution::resolve(&spec(scheme), BlurNegotiation::Applied);

        assert!(
            resolution.contrast_fallback,
            "{scheme:?}: the acrylic tier has dimmed pairs under the floor"
        );
        assert!(!resolution.blur_unavailable);
        assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
        assert!(
            resolution.tokens.contrast().passes(),
            "{scheme:?}: the degraded palette keeps the required pairs readable"
        );
    }
}

#[test]
fn test_resolve_transparent_base_raises_to_opaque() {
    // A fully transparent base over a white desktop leaves the near-white text on
    // white: the window would be there but unreadable. The one allowed degradation
    // turns it opaque instead of drawing it.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 0);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Applied);

    assert!(resolution.contrast_fallback);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(resolution.tokens.surface_fill, rgba(0x1C, 0x1C, 0x1E, 255));
    assert!(resolution.tokens.contrast().passes());
    assert_eq!(
        resolution.diagnostic_codes(),
        [None, Some(CONTRAST_FALLBACK)]
    );
}

#[test]
fn test_resolve_alpha_below_the_contrast_floor_raises_to_opaque() {
    // 0.5 of the dark base over white leaves the text at 3:1, under the floor.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 128);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Applied);

    assert!(resolution.contrast_fallback);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert!(resolution.tokens.contrast().passes());
}

#[test]
fn test_resolve_reports_only_the_blur_degradation_when_blur_is_refused() {
    // A refused blur already forces the base opaque, and an opaque base always clears
    // the contrast gate, so the two degradations can never be reported together.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 64);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Refused);

    assert!(resolution.blur_unavailable);
    assert!(!resolution.contrast_fallback);
    assert_eq!(
        resolution.diagnostic_codes(),
        [Some(BLUR_UNAVAILABLE), None]
    );
    assert!(resolution.tokens.contrast().passes());
}

#[test]
fn test_resolve_reports_the_contrast_fallback_when_acrylic_is_off() {
    // The other way round: the user asked for a translucent base and no blur, and the
    // alpha they picked is too low to read. Only the contrast degradation is reported,
    // because no compositor was ever asked.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, false, 64);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Disabled);

    assert!(!resolution.blur_unavailable);
    assert!(resolution.contrast_fallback);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(
        resolution.diagnostic_codes(),
        [None, Some(CONTRAST_FALLBACK)]
    );
}

#[test]
fn test_degraded_base_keeps_text_readable_in_both_schemes() {
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let spec = spec_with(scheme, default_accent(scheme), true, DEFAULT_BASE_ALPHA);
        let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Refused);
        let report = resolution.tokens.contrast();

        assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
        assert!(
            report.passes(),
            "{scheme:?} palette degraded to {report:?}, which is under the floor"
        );
        assert!(report.primary_on_fill >= CONTRAST_MINIMUM);
    }
}

#[test]
fn test_light_palette_keeps_the_primary_floor_at_a_lower_alpha_than_the_dark_one() {
    // The asymmetry the title names is real but sits one rung lower than the base alpha:
    // the dimmed-pair gate degrades every translucent request before a report can be
    // taken on one -- at 0.63 both schemes resolve with the fallback set and report the
    // opaque tier's numbers -- so the observable asymmetry is on that tier. There the
    // light palette, read against black, keeps `text.primary` above the floor and loses
    // the number label; the dark palette, read against white, holds both.
    let light = tokens_for(
        &spec_with(ColorScheme::Light, DEFAULT_ACCENT_LIGHT, true, 160),
        BlurNegotiation::Applied,
    )
    .contrast();
    let dark = tokens_for(
        &spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 160),
        BlurNegotiation::Applied,
    )
    .contrast();

    assert!(light.primary_on_fill >= CONTRAST_MINIMUM);
    assert!(dark.primary_on_fill >= CONTRAST_MINIMUM);
    assert!(light.number_on_fill < CONTRAST_MINIMUM);
    assert!(dark.number_on_fill >= CONTRAST_MINIMUM);
}

#[test]
fn test_apply_writes_only_the_three_theme_inputs() {
    let tokens = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied);
    let mut sink = RecordingSink::default();

    apply(&mut sink, &tokens);

    assert_eq!(sink.writes, 3, "a theme switch must touch three properties");
    assert_eq!(sink.dark, Some(false));
    assert_eq!(sink.accent, Some(DEFAULT_ACCENT_LIGHT));
}

#[test]
fn test_apply_reports_the_alpha_as_a_fraction() {
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, OPAQUE_ALPHA);
    let tokens = tokens_for(&spec, BlurNegotiation::Applied);
    let mut sink = RecordingSink::default();

    apply(&mut sink, &tokens);

    assert_eq!(sink.base_alpha, Some(1.0));
}

#[test]
fn test_apply_uses_the_alpha_the_contrast_check_approved() {
    // Applying the raw spec instead of the resolution would put the unreadable alpha
    // back on the window, so the resolution is what `apply` takes.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 0);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Applied);
    let mut sink = RecordingSink::default();

    apply(&mut sink, &resolution.tokens);

    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(sink.base_alpha, Some(1.0));
    assert_ne!(sink.base_alpha, Some(0.0));
}

#[test]
fn test_apply_is_idempotent_across_a_scheme_switch() {
    // The window is never rebuilt, so a switch is two applies in a row on the same
    // sink; the second one has to leave the sink in the state of the second theme.
    let dark = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);
    let light = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied);
    let mut sink = RecordingSink::default();

    apply(&mut sink, &dark);
    apply(&mut sink, &dark);
    assert_eq!(sink.dark, Some(true));
    assert_eq!(sink.accent, Some(DEFAULT_ACCENT_DARK));

    apply(&mut sink, &light);
    assert_eq!(sink.writes, 9);
    assert_eq!(sink.dark, Some(false));
    assert_eq!(sink.accent, Some(DEFAULT_ACCENT_LIGHT));
}

#[test]
fn test_request_blur_reports_acceptance() {
    let mut surface = StubSurface::new(true);
    let region = [RectI {
        x: 0,
        y: 0,
        w: 1200,
        h: 280,
    }];

    let outcome = request_blur(&mut surface, &region);

    assert_eq!(outcome, BlurNegotiation::Applied);
    assert_eq!(surface.requests, 1);
}

#[test]
fn test_request_blur_reports_refusal_without_propagating_the_error() {
    // A compositor without blur is a supported configuration, so the failure is an
    // outcome the caller degrades from rather than an error it has to handle.
    let mut surface = StubSurface::new(false);
    let region = [RectI {
        x: 0,
        y: 0,
        w: 1200,
        h: 280,
    }];

    let outcome = request_blur(&mut surface, &region);

    assert_eq!(outcome, BlurNegotiation::Refused);
    assert_eq!(surface.requests, 1);
}

#[test]
fn test_request_blur_without_a_region_is_refused() {
    let mut surface = StubSurface::new(true);

    let outcome = request_blur(&mut surface, &[]);

    assert_eq!(outcome, BlurNegotiation::Refused);
}

#[test]
fn test_a_refused_request_still_produces_a_readable_theme() {
    // The end-to-end shape of the ladder: ask, degrade, and check that what is left is
    // readable. This is the path a machine without compositor blur takes.
    let spec = spec(ColorScheme::Dark);
    let mut surface = StubSurface::new(false);
    let region = [RectI {
        x: 0,
        y: 0,
        w: 1200,
        h: 280,
    }];
    let outcome = if spec.acrylic {
        request_blur(&mut surface, &region)
    } else {
        BlurNegotiation::Disabled
    };

    let resolution = ThemeResolution::resolve(&spec, outcome);

    assert_eq!(outcome, BlurNegotiation::Refused);
    assert_eq!(resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert_eq!(
        resolution.diagnostic_codes(),
        [Some(BLUR_UNAVAILABLE), None]
    );
    assert!(resolution.tokens.contrast().passes());
}

#[test]
fn test_contrast_worst_case_matches_the_worked_example_in_the_specification() {
    // 3.2 names the two worst-case composites and the floor they have to clear. Its
    // "8.9:1" and "14:1" are approximations -- the WCAG formula applied to the
    // composites it names gives 9.6:1 and 12.1:1 -- so what is pinned here is the
    // composite, which the document works out exactly, and the margin it leaves.
    let dark_backdrop = composite_over(rgba(0x1C, 0x1C, 0x1E, 217), WHITE);
    let light_backdrop = composite_over(rgba(0xFF, 0xFF, 0xFF, 217), BLACK);

    assert_eq!(dark_backdrop, rgba(0x3E, 0x3E, 0x40, 255));
    assert_eq!(light_backdrop, rgba(0xD9, 0xD9, 0xD9, 255));

    let dark_ratio = contrast_ratio(rgba(0xF2, 0xF2, 0xF7, 255), dark_backdrop);
    let light_ratio = contrast_ratio(rgba(0x1C, 0x1C, 0x1E, 255), light_backdrop);
    assert!(
        dark_ratio >= CONTRAST_BODY,
        "the dark worst case measured {dark_ratio}"
    );
    assert!(
        light_ratio >= CONTRAST_BODY,
        "the light worst case measured {light_ratio}"
    );
    assert!(
        dark_ratio >= 2.0 * CONTRAST_MINIMUM,
        "the dark worst case has less than twice the floor: {dark_ratio}"
    );
    assert!(
        light_ratio >= 2.0 * CONTRAST_MINIMUM,
        "the light worst case has less than twice the floor: {light_ratio}"
    );
}

#[test]
fn test_resolve_never_returns_a_palette_under_the_contrast_floor() {
    // The self-check is what makes a translucent base safe: whatever alpha the
    // configuration asks for, what comes back is readable. Sweeping every alpha value
    // covers the whole range the configuration can express.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        for alpha in 0..=OPAQUE_ALPHA {
            let custom = spec_with(scheme, default_accent(scheme), true, alpha);
            let resolution = ThemeResolution::resolve(&custom, BlurNegotiation::Applied);
            let report = resolution.tokens.contrast();

            assert!(
                report.passes(),
                "{scheme:?} at alpha {alpha} came back at {report:?}"
            );
        }
    }
}

#[test]
fn test_resolve_random_accents_never_return_a_palette_under_the_floor() {
    // Fifty pseudo-random accents stand in for everything a portal or a config file can
    // hand the window, in the colours the eight RGB corners cannot reach. Whatever the
    // accent, the resolution comes back with the three required pairs above their
    // thresholds, on whatever tier the ladder lands on -- and the dimmed pairs, which no
    // accent can reach, measure identically at the tier every accent resolves to.
    let mut state: u32 = 0x5EED_1A2B;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let mut reference: Option<ContrastReport> = None;
        for _ in 0..50 {
            let accent = rgba(
                (next() % 256) as u8,
                (next() % 256) as u8,
                (next() % 256) as u8,
                OPAQUE_ALPHA,
            );
            let custom = spec_with(scheme, accent, true, DEFAULT_BASE_ALPHA);
            let resolution = ThemeResolution::resolve(&custom, BlurNegotiation::Applied);
            let report = resolution.tokens.contrast();

            assert!(
                report.passes(),
                "{scheme:?} with accent {accent:?} came back at {report:?}"
            );
            match reference {
                None => reference = Some(report),
                Some(previous) => assert_eq!(
                    report.number_on_fill, previous.number_on_fill,
                    "{scheme:?}: the dimmed pairs must not move with the accent"
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The candidate window's shadow material (3.1.2)
// ---------------------------------------------------------------------------
//
// Three files hold one contract: 3.1.2 fixes the geometry, `ui/theme.slint` carries the falloff
// as per-band colours, and `ui/candidate.slint` tiles it. `slint_palette.rs` pins the ramps'
// shape; the tests below pin the view's side -- its geometry, and the height of each ramp.

use super::slint_palette::{SLINT_CANDIDATE, SLINT_THEME, evaluate_bands, rounded_alpha};

/// The number `ui/candidate.slint` declares for the `CandidateMetrics` constant `name`.
///
/// The block is read as text, the way `src/layout/metrics.rs` reads it: a generated binding
/// needs a live component and a display device, and a test may require neither.
fn candidate_metric(name: &str) -> Option<f32> {
    SLINT_CANDIDATE.lines().find_map(|line| {
        let text = line.split("//").next().unwrap_or_default().trim();
        let rest = text.strip_prefix("out property <")?;
        let (_, rest) = rest.split_once('>')?;
        let (declared, value) = rest.trim().split_once(':')?;
        if declared.trim() != name {
            return None;
        }
        let value = value.trim().strip_suffix(';')?.trim();
        let digits = value.strip_suffix("px").unwrap_or(value);
        digits.trim().parse::<f32>().ok()
    })
}

/// The peak alpha of the outer shadow's ramp: `CandidateMetrics.shadow-band-opacity`.
///
/// Named rather than passed to [`candidate_metric`] as a string, because this token is the one
/// the view declares and does not read -- the ramp reaches the rasterizer as the per-band
/// colours of `Theme.shadow-outer-bands` -- so the pin below is all that holds the two together.
fn shadow_band_opacity() -> Option<f32> {
    candidate_metric("shadow-band-opacity")
}

/// The share of `shadow-inner`'s own alpha the band against the panel carries:
/// `CandidateMetrics.shadow-inner-opacity`. Named for the reason above.
fn shadow_inner_opacity() -> Option<f32> {
    candidate_metric("shadow-inner-opacity")
}

/// Whether any line of `source` outside a comment mentions `needle`.
///
/// The same rule the audit scripts apply: a file that explains why it does *not* use a
/// construct must not read as a file that uses it.
fn code_mentions(source: &str, needle: &str) -> bool {
    source
        .lines()
        .any(|line| line.split("//").next().unwrap_or_default().contains(needle))
}

/// The body of the `export component CandidateShadow` declaration.
///
/// Sliced by name, because the assertions about it are about one component: a binding elsewhere
/// in the file belongs to another component, and the same binding inside this block is the
/// defect those assertions exist for.
fn shadow_component(source: &str) -> &str {
    let opening = "export component CandidateShadow";
    let start = source
        .find(opening)
        .expect("the shadow component is declared");
    let rest = source.get(start..).expect("the offset is a boundary");
    let end = rest.find("\nexport component ").unwrap_or(rest.len());
    rest.get(..end).expect("the slice is a boundary")
}

#[test]
fn test_view_shadow_material_matches_the_material_table() {
    // 3.1.2's two layers are `0 8dp 28dp` for L3 and `0 1dp 2dp` for L2, and 3.1.1 gives the
    // window a 32dp reserve around the panel to draw them in. These five numbers are the whole
    // geometry of the material, so pinning them is what keeps a later edit from widening the
    // inner layer back into the outline 3.1.2 rules out, or from moving the panel inside the
    // reserve the placement pass computes the surface from.
    assert_eq!(candidate_metric("shadow-margin"), Some(32.0));
    assert_eq!(candidate_metric("shadow-blur"), Some(28.0));
    assert_eq!(candidate_metric("shadow-offset-y"), Some(8.0));
    assert_eq!(candidate_metric("shadow-inner-spread"), Some(2.0));
    assert_eq!(candidate_metric("shadow-inner-offset-y"), Some(1.0));
}

#[test]
fn test_view_shadow_bands_tile_the_blur_without_a_gap() {
    // The view paints each band as a ring one step wide whose outer edge sits at the band's own
    // distance from the panel, so the rings tile the blur only while the step divides it
    // exactly: otherwise two bands overlap or a transparent seam opens between them, and either
    // one reads as a step in what is supposed to be a gradient.
    let blur = candidate_metric("shadow-blur").expect("3.1.2 states the blur");
    let count = candidate_metric("shadow-band-count").expect("the layer counts its bands");
    let step = blur / count;

    assert_eq!(
        count, 8.0,
        "eight bands of 3.5dp read as a gradient at 1.0x"
    );
    assert_eq!(step, 3.5, "the bands divide the 28dp blur evenly");
    assert_eq!(
        blur,
        step * count,
        "so the outermost band ends on the blur's edge"
    );
    assert!(
        step >= 1.0,
        "a band narrower than a pixel would band the 1.0x case"
    );
}

#[test]
fn test_view_inner_shadow_stays_within_its_two_dp() {
    // L2 keeps the acrylic edge from blurring into the desktop, and 3.1.2 makes it 2dp wide. A
    // wider ring, or one band carrying the whole spread, is the 4dp 35%-black halo an earlier
    // revision drew -- a line around the panel, which is the opposite of an edge darkening. Both
    // bounds are asserted, because either alone is satisfied by a layer that still reads as one.
    let spread = candidate_metric("shadow-inner-spread").expect("3.1.2 states the spread");
    let count = candidate_metric("shadow-inner-band-count").expect("the layer counts its bands");
    let step = spread / count;

    assert!(
        spread <= 2.0,
        "the layer is at most 2dp wide, found {spread}dp"
    );
    assert!(count >= 2.0, "one band of the whole spread is an outline");
    assert_eq!(step, 1.0, "so every band is a 1dp hairline");
    assert_eq!(
        spread,
        step * count,
        "and the bands tile the spread exactly"
    );
    let peak = shadow_inner_opacity().expect("the layer declares a peak");
    assert!(
        peak < 1.0,
        "a fully opaque ring is a drawn line, found {peak}"
    );
}

#[test]
fn test_view_shadow_layers_paint_from_the_theme_band_tables() {
    // The falloff cannot be an element `opacity`: the software renderer this project ships
    // takes a *bound* opacity down a compositing path that draws nothing at all, which is what
    // once turned the whole window transparent. The ramp therefore reaches the rasterizer as
    // the per-band colours `ui/theme.slint` declares, and the view has to read those tables
    // instead of restating an alpha of its own. Slint's `drop-shadow-*` is the other way this
    // could be drawn and the one the renderer stubs out, so neither may come back.
    let shadow = shadow_component(SLINT_CANDIDATE);

    assert!(
        shadow.contains("Theme.shadow-outer-bands[band]"),
        "{shadow}"
    );
    assert!(
        shadow.contains("Theme.shadow-inner-bands[band]"),
        "{shadow}"
    );
    assert!(
        !code_mentions(shadow, "opacity"),
        "a bound opacity draws nothing on this renderer"
    );
    assert!(
        !code_mentions(SLINT_CANDIDATE, "drop-shadow"),
        "the software renderer's draw_box_shadow is a stub"
    );
}

#[test]
fn test_view_shadow_band_counts_match_the_theme_tables() {
    // The view tiles each layer with the count `CandidateMetrics` declares and paints the ring
    // at index `band` with the table entry at that index, so the count and the table's length
    // are one contract: a count past the table would index out of it, and one short of it would
    // leave the outermost bands -- the faint ones the falloff is made of -- unpainted.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let outer = evaluate_bands(SLINT_THEME, "shadow-outer-bands", scheme);
        let inner = evaluate_bands(SLINT_THEME, "shadow-inner-bands", scheme);

        assert_eq!(
            candidate_metric("shadow-band-count"),
            Some(outer.len() as f32),
            "{scheme:?}: the outer layer tiles the table it paints from"
        );
        assert_eq!(
            candidate_metric("shadow-inner-band-count"),
            Some(inner.len() as f32),
            "{scheme:?}: the inner layer tiles the table it paints from"
        );
    }
}

#[test]
fn test_view_shadow_peaks_match_the_theme_ramps() {
    // The height of each ramp is declared twice: as the alpha the band against the panel paints
    // at in `ui/theme.slint`, and as `shadow-band-opacity` / `shadow-inner-opacity` in the
    // view. Nothing on the drawing path reads the two tokens, so these assertions are the only
    // thing that keeps an edit to one of the two copies from being silent.
    let peak = shadow_band_opacity().expect("the outer layer declares a peak");
    let outer = evaluate_bands(SLINT_THEME, "shadow-outer-bands", ColorScheme::Dark);
    assert_eq!(
        outer.last().map(|band| band.a),
        Some(rounded_alpha(peak)),
        "the band against the panel paints at the declared peak"
    );

    let weight = shadow_inner_opacity().expect("the inner layer declares one");
    let token = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied).shadow_inner;
    let inner = evaluate_bands(SLINT_THEME, "shadow-inner-bands", ColorScheme::Dark);
    assert_eq!(
        inner.last().map(|band| band.a),
        Some(rounded_alpha(f32::from(token.a) / 255.0 * weight)),
        "the band against the panel carries the declared share of the token"
    );
    assert!(
        inner.first().map(|band| band.a) < inner.last().map(|band| band.a),
        "and the band outside it is the lighter one"
    );
}
