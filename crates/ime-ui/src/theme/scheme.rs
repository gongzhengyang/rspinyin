//! Which palette the window uses, and which accent colour it paints with.
//!
//! Both decisions are pure functions of their arguments. The environment and the
//! portal are read once, at startup, by the caller that builds [`SchemeSignals`]; the
//! design forbids polling for a theme change, so a later change arrives as a
//! [`UiCommand::Theme`](ime_types::UiCommand::Theme) carrying a fresh
//! [`ThemeSpec`](ime_types::ThemeSpec) rather than as a re-read here.

use ime_config::{Config, ThemeScheme};
use ime_types::{ColorScheme, Rgba8};

use super::OPAQUE_ALPHA;
use super::color::with_alpha;

/// The accent the dark palette ships with: the dark column of 3.2 `accent.default`.
pub const DEFAULT_ACCENT_DARK: Rgba8 = Rgba8 {
    r: 0x4C,
    g: 0x9A,
    b: 0xFF,
    a: OPAQUE_ALPHA,
};

/// The accent the light palette ships with: the light column of 3.2 `accent.default`.
pub const DEFAULT_ACCENT_LIGHT: Rgba8 = Rgba8 {
    r: 0x0A,
    g: 0x6C,
    b: 0xFF,
    a: OPAQUE_ALPHA,
};

/// The accent a scheme uses when nothing overrides it.
///
/// # Parameters
///
/// - `scheme`: the palette in use.
///
/// # Returns
///
/// [`DEFAULT_ACCENT_DARK`] or [`DEFAULT_ACCENT_LIGHT`].
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn default_accent(scheme: ColorScheme) -> Rgba8 {
    match scheme {
        ColorScheme::Dark => DEFAULT_ACCENT_DARK,
        ColorScheme::Light => DEFAULT_ACCENT_LIGHT,
    }
}

/// The `color-scheme` value `org.freedesktop.appearance` reports.
///
/// The interface defines three values, and a portal that answers with anything else
/// is not understood: an unrecognised number must never decide a colour, so it is
/// read as "no preference" and the next source in the chain gets its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalColorScheme {
    /// `0`: the portal has no preference.
    Unspecified,
    /// `1`: the user prefers a dark interface.
    PreferDark,
    /// `2`: the user prefers a light interface.
    PreferLight,
}

impl PortalColorScheme {
    /// Reads the integer the portal reports.
    ///
    /// # Parameters
    ///
    /// - `value`: the `color-scheme` variant value.
    ///
    /// # Returns
    ///
    /// [`PreferDark`](Self::PreferDark) for `1`, [`PreferLight`](Self::PreferLight)
    /// for `2`, and [`Unspecified`](Self::Unspecified) for `0` or for any value the
    /// interface does not define.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn from_portal_value(value: u32) -> Self {
        match value {
            1 => Self::PreferDark,
            2 => Self::PreferLight,
            _ => Self::Unspecified,
        }
    }
}

/// The environment-derived signals the scheme decision reads, plus the portal's
/// answer when the portal thread got one.
///
/// Owned strings rather than borrows because the caller reads the variables into
/// `String`s and would otherwise have to keep them alive across the call for no
/// benefit: this is a once-per-session decision, not a hot path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemeSignals {
    /// The portal's answer, when the portal answered at all.
    pub portal: Option<PortalColorScheme>,
    /// `GTK_THEME`, when it is set.
    pub gtk_theme: Option<String>,
    /// `QT_STYLE_OVERRIDE`, when it is set.
    pub qt_style_override: Option<String>,
}

impl SchemeSignals {
    /// Reads the two environment variables, taking the portal's answer as given.
    ///
    /// This is the only place in the theme module that touches the environment. It is
    /// called once while the plugin starts; nothing re-reads these variables later,
    /// because a periodically-waking reader would break the idle CPU budget.
    ///
    /// # Parameters
    ///
    /// - `portal`: what the portal reported, or `None` when it could not be reached.
    ///
    /// # Returns
    ///
    /// The signals, with each variable absent when it is unset or not valid UTF-8.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. An unreadable variable is
    /// simply not a signal.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn from_environment(portal: Option<PortalColorScheme>) -> Self {
        Self {
            portal,
            gtk_theme: std::env::var("GTK_THEME").ok(),
            qt_style_override: std::env::var("QT_STYLE_OVERRIDE").ok(),
        }
    }
}

/// Decides which palette the window uses.
///
/// `theme.scheme` wins outright: a user who pinned a scheme is not overridden by the
/// desktop. Under `auto` the sources are consulted in the order 3.2 fixes -- the
/// portal, then `GTK_THEME`, then `QT_STYLE_OVERRIDE` -- and the first one that names
/// a scheme decides. A theme name that does not mention `dark` is a light theme, so
/// `GTK_THEME=Adwaita` means light rather than "no signal"; when nothing names a
/// scheme at all the window is dark, which is the documented default.
///
/// # Parameters
///
/// - `cfg`: the resolved configuration, for the `theme.scheme` key.
/// - `signals`: the environment and portal signals.
///
/// # Returns
///
/// The palette to render with.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn resolve_scheme(cfg: &Config, signals: &SchemeSignals) -> ColorScheme {
    match cfg.theme.scheme {
        ThemeScheme::Light => ColorScheme::Light,
        ThemeScheme::Dark => ColorScheme::Dark,
        ThemeScheme::Auto => auto_scheme(signals),
    }
}

/// The `auto` branch of [`resolve_scheme`].
fn auto_scheme(signals: &SchemeSignals) -> ColorScheme {
    match signals.portal {
        Some(PortalColorScheme::PreferDark) => return ColorScheme::Dark,
        Some(PortalColorScheme::PreferLight) => return ColorScheme::Light,
        Some(PortalColorScheme::Unspecified) | None => {}
    }
    let names = [
        signals.gtk_theme.as_deref(),
        signals.qt_style_override.as_deref(),
    ];
    for name in names.into_iter().flatten() {
        if let Some(scheme) = scheme_named_by(name) {
            return scheme;
        }
    }
    ColorScheme::Dark
}

/// Reads a toolkit theme name.
///
/// Returns `None` for a name that is empty or only whitespace, which is how an unset
/// variable usually reaches us, so the chain falls through instead of picking a
/// palette from nothing.
fn scheme_named_by(name: &str) -> Option<ColorScheme> {
    if name.trim().is_empty() {
        return None;
    }
    if name.to_ascii_lowercase().contains("dark") {
        Some(ColorScheme::Dark)
    } else {
        Some(ColorScheme::Light)
    }
}

/// Decides the accent colour the window paints with.
///
/// Priority is the portal's `accent-color`, then the configured `theme.accent`, then
/// the scheme's built-in default. The returned colour is opaque: an accent is an RGB
/// choice, and the alphas that make the selected-cell background and stroke are the
/// design's, applied on top of it.
///
/// # Parameters
///
/// - `scheme`: the palette in use, which picks the built-in default.
/// - `configured`: `theme.accent`, or `None` when the caller has no configuration.
/// - `portal`: the portal's accent, or `None` when the portal did not report one.
///
/// # Returns
///
/// The accent, with an alpha of [`OPAQUE_ALPHA`].
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn resolve_accent(
    scheme: ColorScheme,
    configured: Option<Rgba8>,
    portal: Option<Rgba8>,
) -> Rgba8 {
    let source = portal.or(configured);
    let accent = source.unwrap_or_else(|| default_accent(scheme));
    with_alpha(accent, OPAQUE_ALPHA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ime_config::ThemeConfig;

    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba8 {
        Rgba8 { r, g, b, a }
    }

    fn signals(
        portal: Option<PortalColorScheme>,
        gtk: Option<&str>,
        qt: Option<&str>,
    ) -> SchemeSignals {
        SchemeSignals {
            portal,
            gtk_theme: gtk.map(String::from),
            qt_style_override: qt.map(String::from),
        }
    }

    fn config_with(scheme: ThemeScheme) -> Config {
        Config {
            // `ThemeConfig` deliberately has no `Default` of its own — `Config::default()`
            // is the single source of default values — so the base comes from there.
            theme: ThemeConfig {
                scheme,
                ..Config::default().theme
            },
            ..Config::default()
        }
    }

    #[test]
    fn test_resolve_scheme_explicit_setting_wins_over_every_signal() {
        let dark = config_with(ThemeScheme::Dark);
        let light = config_with(ThemeScheme::Light);
        let says_light = signals(Some(PortalColorScheme::PreferLight), None, None);
        let says_dark = signals(Some(PortalColorScheme::PreferDark), Some("Adwaita"), None);

        assert_eq!(resolve_scheme(&dark, &says_light), ColorScheme::Dark);
        assert_eq!(resolve_scheme(&light, &says_dark), ColorScheme::Light);
    }

    #[test]
    fn test_resolve_scheme_portal_preference_decides() {
        let cfg = config_with(ThemeScheme::Auto);
        let dark = signals(Some(PortalColorScheme::PreferDark), Some("Adwaita"), None);
        let light = signals(
            Some(PortalColorScheme::PreferLight),
            Some("Adwaita-Dark"),
            None,
        );

        assert_eq!(resolve_scheme(&cfg, &dark), ColorScheme::Dark);
        assert_eq!(resolve_scheme(&cfg, &light), ColorScheme::Light);
    }

    #[test]
    fn test_resolve_scheme_unspecified_portal_falls_through_to_gtk_theme() {
        let cfg = config_with(ThemeScheme::Auto);
        let unspecified = signals(
            Some(PortalColorScheme::Unspecified),
            Some("Adwaita-Dark"),
            None,
        );
        let absent = signals(None, Some("Adwaita-Dark"), None);

        assert_eq!(resolve_scheme(&cfg, &unspecified), ColorScheme::Dark);
        assert_eq!(resolve_scheme(&cfg, &absent), ColorScheme::Dark);
    }

    #[test]
    fn test_resolve_scheme_gtk_theme_dark_is_case_insensitive() {
        let cfg = config_with(ThemeScheme::Auto);
        let upper = signals(None, Some("Adwaita-DARK"), None);
        let mixed = signals(None, Some("Yaru-dark"), None);

        assert_eq!(resolve_scheme(&cfg, &upper), ColorScheme::Dark);
        assert_eq!(resolve_scheme(&cfg, &mixed), ColorScheme::Dark);
    }

    #[test]
    fn test_resolve_scheme_gtk_theme_without_dark_selects_light() {
        let cfg = config_with(ThemeScheme::Auto);
        let named = signals(None, Some("Adwaita"), None);

        assert_eq!(resolve_scheme(&cfg, &named), ColorScheme::Light);
    }

    #[test]
    fn test_resolve_scheme_qt_style_override_used_when_gtk_theme_absent() {
        let cfg = config_with(ThemeScheme::Auto);
        let dark = signals(None, None, Some("Breeze-Dark"));
        let light = signals(None, None, Some("Breeze"));

        assert_eq!(resolve_scheme(&cfg, &dark), ColorScheme::Dark);
        assert_eq!(resolve_scheme(&cfg, &light), ColorScheme::Light);
    }

    #[test]
    fn test_resolve_scheme_ignores_a_blank_theme_name() {
        let cfg = config_with(ThemeScheme::Auto);
        let blank = signals(None, Some("   "), Some(""));

        assert_eq!(resolve_scheme(&cfg, &blank), ColorScheme::Dark);
    }

    #[test]
    fn test_resolve_scheme_without_signals_defaults_to_dark() {
        let cfg = config_with(ThemeScheme::Auto);
        let nothing = SchemeSignals::default();

        assert_eq!(resolve_scheme(&cfg, &nothing), ColorScheme::Dark);
    }

    #[test]
    fn test_from_environment_carries_the_portal_answer() {
        // The environment half of the result is whatever this machine has set, so the
        // assertion covers the portal argument, which is the part the caller owns.
        let signals = SchemeSignals::from_environment(Some(PortalColorScheme::PreferLight));
        assert_eq!(signals.portal, Some(PortalColorScheme::PreferLight));
    }

    #[test]
    fn test_portal_color_scheme_reads_the_defined_values() {
        assert_eq!(
            PortalColorScheme::from_portal_value(0),
            PortalColorScheme::Unspecified
        );
        assert_eq!(
            PortalColorScheme::from_portal_value(1),
            PortalColorScheme::PreferDark
        );
        assert_eq!(
            PortalColorScheme::from_portal_value(2),
            PortalColorScheme::PreferLight
        );
    }

    #[test]
    fn test_portal_color_scheme_rejects_values_outside_the_interface() {
        // A portal that answers with a value we do not know must not be able to pick a
        // palette on its own.
        assert_eq!(
            PortalColorScheme::from_portal_value(3),
            PortalColorScheme::Unspecified
        );
        assert_eq!(
            PortalColorScheme::from_portal_value(u32::MAX),
            PortalColorScheme::Unspecified
        );
    }

    #[test]
    fn test_default_accent_follows_the_scheme() {
        assert_eq!(default_accent(ColorScheme::Dark), DEFAULT_ACCENT_DARK);
        assert_eq!(default_accent(ColorScheme::Light), DEFAULT_ACCENT_LIGHT);
        assert_ne!(DEFAULT_ACCENT_DARK, DEFAULT_ACCENT_LIGHT);
    }

    #[test]
    fn test_resolve_accent_prefers_the_portal() {
        let portal = rgba(0x11, 0x22, 0x33, 255);
        let configured = rgba(0x44, 0x55, 0x66, 255);

        assert_eq!(
            resolve_accent(ColorScheme::Dark, Some(configured), Some(portal)),
            portal
        );
    }

    #[test]
    fn test_resolve_accent_falls_back_to_the_configuration() {
        let configured = rgba(0x44, 0x55, 0x66, 255);

        assert_eq!(
            resolve_accent(ColorScheme::Light, Some(configured), None),
            configured
        );
    }

    #[test]
    fn test_resolve_accent_without_sources_uses_the_scheme_default() {
        assert_eq!(
            resolve_accent(ColorScheme::Dark, None, None),
            DEFAULT_ACCENT_DARK
        );
        assert_eq!(
            resolve_accent(ColorScheme::Light, None, None),
            DEFAULT_ACCENT_LIGHT
        );
    }

    #[test]
    fn test_resolve_accent_forces_an_opaque_colour() {
        // The layer alphas belong to the tokens that use the accent, not to the accent
        // itself, so a translucent source cannot silently weaken the focus ring.
        let translucent = rgba(0x4C, 0x9A, 0xFF, 0);

        let resolved = resolve_accent(ColorScheme::Dark, None, Some(translucent));

        assert_eq!(resolved, DEFAULT_ACCENT_DARK);
        assert_eq!(resolved.a, OPAQUE_ALPHA);
    }
}
