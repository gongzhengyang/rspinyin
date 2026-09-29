//! Tests for the theme pipeline: the two palettes against the 3.2 table, the
//! contrast gate at every degradation tier, and the three property writes a theme
//! switch performs.
//!
//! Nothing here needs a display server or a Slint runtime: the palette is plain data,
//! the sink is a mock that records what it was asked to write, and the compositor is
//! a stub that answers "yes" or "no" on demand. That is what makes the degradation
//! ladder, which cannot be exercised on a machine whose compositor has no blur,
//! covered by an ordinary test run.

use super::*;

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba8 {
    Rgba8 { r, g, b, a }
}

/// A theme request with acrylic on, the shipped base alpha and the scheme's accent.
fn spec(scheme: ColorScheme) -> ThemeSpec {
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
fn spec_with(scheme: ColorScheme, accent: Rgba8, acrylic: bool, base_alpha: u8) -> ThemeSpec {
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
fn tokens_for(spec: &ThemeSpec, blur: BlurNegotiation) -> ThemeTokens {
    ThemeResolution::resolve(spec, blur).tokens
}

/// Records what a theme switch wrote, so "only properties change" can be asserted.
#[derive(Debug, Default, PartialEq)]
struct RecordingSink {
    dark: Option<bool>,
    accent: Option<Rgba8>,
    base_alpha: Option<f32>,
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
    let tokens = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);

    assert!(tokens.dark);
    assert_eq!(tokens.base_alpha, 217);
    assert_eq!(tokens.surface_base, rgba(0x1C, 0x1C, 0x1E, 255));
    assert_eq!(tokens.surface_fill, rgba(0x1C, 0x1C, 0x1E, 217));
    assert_eq!(tokens.surface_stroke, rgba(255, 255, 255, 26));
    assert_eq!(tokens.text_primary, rgba(0xF2, 0xF2, 0xF7, 255));
    assert_eq!(tokens.text_secondary, rgba(0xF2, 0xF2, 0xF7, 158));
    assert_eq!(tokens.text_annotation, rgba(0xF2, 0xF2, 0xF7, 122));
    assert_eq!(tokens.text_separator, rgba(0xF2, 0xF2, 0xF7, 102));
    assert_eq!(tokens.accent, DEFAULT_ACCENT_DARK);
    assert_eq!(tokens.accent_on, rgba(255, 255, 255, 255));
    assert_eq!(tokens.state_hover, rgba(0xF2, 0xF2, 0xF7, 20));
    assert_eq!(tokens.state_selected_bg, rgba(0x4C, 0x9A, 0xFF, 46));
    assert_eq!(tokens.state_selected_stroke, rgba(0x4C, 0x9A, 0xFF, 140));
    assert_eq!(tokens.state_pressed, rgba(0xF2, 0xF2, 0xF7, 36));
    assert_eq!(tokens.separator, rgba(0xF2, 0xF2, 0xF7, 26));
    assert_eq!(tokens.shadow_inner, rgba(0, 0, 0, 89));
    assert_eq!(tokens.shadow_outer, rgba(0, 0, 0, 107));
    assert_eq!(tokens.status_dot_active, DEFAULT_ACCENT_DARK);
    assert_eq!(tokens.status_dot_idle, rgba(0xF2, 0xF2, 0xF7, 89));
}

#[test]
fn test_light_palette_matches_the_specification_table() {
    let tokens = tokens_for(&spec(ColorScheme::Light), BlurNegotiation::Applied);

    assert!(!tokens.dark);
    assert_eq!(tokens.base_alpha, 217);
    assert_eq!(tokens.surface_base, rgba(0xFF, 0xFF, 0xFF, 255));
    assert_eq!(tokens.surface_fill, rgba(0xFF, 0xFF, 0xFF, 217));
    assert_eq!(tokens.surface_stroke, rgba(0, 0, 0, 15));
    assert_eq!(tokens.text_primary, rgba(0x1C, 0x1C, 0x1E, 255));
    assert_eq!(tokens.text_secondary, rgba(0x1C, 0x1C, 0x1E, 153));
    assert_eq!(tokens.text_annotation, rgba(0x1C, 0x1C, 0x1E, 115));
    assert_eq!(tokens.text_separator, rgba(0x1C, 0x1C, 0x1E, 89));
    assert_eq!(tokens.accent, DEFAULT_ACCENT_LIGHT);
    assert_eq!(tokens.accent_on, rgba(255, 255, 255, 255));
    assert_eq!(tokens.state_hover, rgba(0x1C, 0x1C, 0x1E, 15));
    assert_eq!(tokens.state_selected_bg, rgba(0x0A, 0x6C, 0xFF, 36));
    assert_eq!(tokens.state_selected_stroke, rgba(0x0A, 0x6C, 0xFF, 128));
    assert_eq!(tokens.state_pressed, rgba(0x1C, 0x1C, 0x1E, 31));
    assert_eq!(tokens.separator, rgba(0x1C, 0x1C, 0x1E, 20));
    assert_eq!(tokens.shadow_inner, rgba(0, 0, 0, 20));
    assert_eq!(tokens.shadow_outer, rgba(0, 0, 0, 41));
    assert_eq!(tokens.status_dot_active, DEFAULT_ACCENT_LIGHT);
    assert_eq!(tokens.status_dot_idle, rgba(0x1C, 0x1C, 0x1E, 77));
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
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 200);
    let tokens = tokens_for(&spec, BlurNegotiation::Applied);

    assert_eq!(tokens.surface_fill, with_alpha(tokens.surface_base, 200));
    assert_eq!(tokens.surface_fill.r, tokens.surface_base.r);
    assert_eq!(tokens.surface_base.a, OPAQUE_ALPHA);
}

#[test]
fn test_contrast_dark_palette_clears_every_threshold() {
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
fn test_contrast_light_palette_clears_every_threshold() {
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
    // able to force the window opaque against the user's explicit `acrylic = false`.
    let spec = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, false, 217);
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Refused);

    assert_eq!(resolution.tokens.base_alpha, 217);
    assert!(!resolution.blur_unavailable);
    assert_eq!(resolution.diagnostic_codes(), [None, None]);
}

#[test]
fn test_resolve_keeps_the_configured_alpha_at_the_shipped_default() {
    let resolution = ThemeResolution::resolve(&spec(ColorScheme::Dark), BlurNegotiation::Applied);

    assert_eq!(resolution.tokens.base_alpha, DEFAULT_BASE_ALPHA);
    assert!(!resolution.contrast_fallback);
    assert!(resolution.tokens.contrast().passes());
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
fn test_light_palette_tolerates_a_lower_base_alpha_than_the_dark_one() {
    // The light palette is measured against black and the dark one against white, so
    // the light base can lose more of its opacity before its text stops being
    // readable: at 0.63 of full opacity the light palette still clears 4.5:1 while the
    // dark one has fallen to 4.4:1 and has to be raised to opaque. The test pins that
    // asymmetry, so neither floor can move unnoticed.
    let light = spec_with(ColorScheme::Light, DEFAULT_ACCENT_LIGHT, true, 160);
    let dark = spec_with(ColorScheme::Dark, DEFAULT_ACCENT_DARK, true, 160);
    let light_resolution = ThemeResolution::resolve(&light, BlurNegotiation::Applied);
    let dark_resolution = ThemeResolution::resolve(&dark, BlurNegotiation::Applied);

    assert!(!light_resolution.contrast_fallback);
    assert_eq!(light_resolution.tokens.base_alpha, 160);
    assert!(light_resolution.tokens.contrast().passes());
    assert!(dark_resolution.contrast_fallback);
    assert_eq!(dark_resolution.tokens.base_alpha, OPAQUE_ALPHA);
    assert!(dark_resolution.tokens.contrast().passes());
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
