//! The property writes a theme switch performs, and the blur round trip that
//! precedes one: what [`apply`] writes and that it writes nothing else, and what
//! `request_blur` reports when the compositor takes or refuses the request.

use ime_types::{PlatformError, RectI, Rgba8};

use super::{spec, spec_with, tokens_for};
use crate::theme::{
    BLUR_UNAVAILABLE, BlurNegotiation, BlurSurface, ColorScheme, DEFAULT_ACCENT_DARK,
    DEFAULT_ACCENT_LIGHT, OPAQUE_ALPHA, ThemeResolution, ThemeSink, apply, request_blur,
};

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
