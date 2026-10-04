//! The degradation ladder: how [`ThemeResolution::resolve`] and `resolve_base_alpha`
//! answer the blur negotiation and the contrast gate, tier by tier, and what they
//! record when they degrade.

use ime_types::ColorScheme;

use super::{rgba, spec, spec_with, tokens_for};
use crate::theme::{
    BLUR_UNAVAILABLE, BlurNegotiation, CONTRAST_FALLBACK, CONTRAST_MINIMUM, DEFAULT_ACCENT_DARK,
    DEFAULT_ACCENT_LIGHT, DEFAULT_BASE_ALPHA, OPAQUE_ALPHA, ThemeResolution, default_accent,
    resolve_base_alpha,
};

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
