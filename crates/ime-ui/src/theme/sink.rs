//! The write side: the [`ThemeSink`] abstraction over the window's Slint theme
//! global, and [`apply`], which writes a resolved token set through it.

use ime_types::Rgba8;

use super::tokens::ThemeTokens;

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

    /// Writes the font family the candidate window draws with.
    ///
    /// Deliberately not part of `apply`: the family is chosen once at startup by probing
    /// which families this machine actually has, and it does not change with the theme.
    /// Routing it through `apply` would rewrite it on every light/dark switch, which is
    /// the one thing the probe exists to avoid doing more than once.
    ///
    /// # Parameters
    ///
    /// - `family`: a family name, or the empty string to leave the view's own default.
    fn set_font_family(&mut self, family: &str);

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
/// every colour token from them, so a theme switch -- including the dark-to-light one
/// -- is a property update, never a rebuild, and cannot flash. The font family is the one
/// token a switch deliberately leaves alone: it is resolved once at startup from the font
/// probe's answer, and a family change would re-shape every glyph the window draws.
///
/// # Parameters
///
/// - `sink`: the window's theme global, behind [`ThemeSink`].
/// - `tokens`: the resolution to apply. Apply the tokens rather than the raw
///   [`ime_types::ThemeSpec`], so the alpha written is the one the contrast gate approved.
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
