//! The `ui/theme.slint` copy of the palette, and the tests that keep it in step with
//! the one in `src/theme.rs`.
//!
//! The palette lives twice: in the Slint global's defaults, so the window is right
//! before the first `apply` runs, and in `src/theme.rs`, so the contrast gate can be
//! evaluated without a renderer. Two copies of one table drift apart unless something
//! compares them, and the comparison cannot go through Slint -- building a global needs
//! a window and a display server, which a test must not require -- so the file is read
//! as text and its expressions are evaluated with the conversions Slint itself applies:
//! `i-slint-compiler` emits `(255. * a) as u8` for the `rgba()` builtin, which
//! truncates, and `i-slint-core`'s `Color::with_alpha` rounds.
//!
//! This module holds test code only; it is compiled under `cfg(test)`.

use super::tests::{rgba, spec, spec_with, tokens_for};
use super::*;

/// `ui/theme.slint`, as text.
const SLINT_THEME: &str = include_str!("../../ui/theme.slint");

/// One property declaration of a `.slint` file.
struct Declaration<'a> {
    /// The property's name.
    name: &'a str,
    /// Whether the property is an input, which the Rust side writes.
    input: bool,
    /// The declared type, without its angle brackets.
    type_name: &'a str,
    /// The default expression, without its trailing semicolon.
    expression: &'a str,
}

/// Parses the property declarations out of a `.slint` source.
fn declarations(source: &str) -> Vec<Declaration<'_>> {
    source.lines().filter_map(declaration).collect()
}

/// Parses one line, when it declares a property.
fn declaration(line: &str) -> Option<Declaration<'_>> {
    let line = line.trim();
    let (input, rest) = line
        .strip_prefix("in property ")
        .map(|rest| (true, rest))
        .or_else(|| line.strip_prefix("out property ").map(|rest| (false, rest)))?;
    let (type_name, rest) = rest.strip_prefix('<')?.split_once('>')?;
    let (name, expression) = rest.split_once(':')?;
    Some(Declaration {
        name: name.trim(),
        input,
        type_name: type_name.trim(),
        expression: expression.trim().strip_suffix(';')?,
    })
}

/// The values `theme.slint` takes from Rust, which its expressions read.
struct Inputs {
    dark: bool,
    accent: Rgba8,
    base_alpha: u8,
}

/// The default expression `theme.slint` declares for a property.
fn declared_expression<'src>(properties: &[Declaration<'src>], name: &str) -> Option<&'src str> {
    properties
        .iter()
        .find(|declaration| declaration.name == name)
        .map(|declaration| declaration.expression)
}

/// Evaluates a colour expression the way the Slint runtime would.
fn evaluate_color(
    expression: &str,
    inputs: &Inputs,
    properties: &[Declaration<'_>],
) -> Option<Rgba8> {
    let expression = expression.trim();
    if let Some((yes, no)) = conditional(expression) {
        return evaluate_color(if inputs.dark { yes } else { no }, inputs, properties);
    }
    if let Some(hex) = expression.strip_prefix('#') {
        return hex_color(hex);
    }
    if let Some(arguments) = call(expression, "rgba") {
        return rgba_builtin(arguments);
    }
    if let Some(arguments) = call(expression, "accent.with-alpha") {
        return Some(with_alpha(
            inputs.accent,
            alpha(arguments, inputs, properties)?,
        ));
    }
    if let Some(arguments) = call(expression, "surface-base.with-alpha") {
        let base = named("surface-base", inputs, properties)?;
        return Some(with_alpha(base, alpha(arguments, inputs, properties)?));
    }
    if expression == "accent" {
        // The accent is an input: the Rust side overwrites the default the file ships,
        // and every token derived from it has to follow the property.
        return Some(inputs.accent);
    }
    named(expression, inputs, properties)
}

/// Splits the `dark ? A : B` expression `theme.slint` uses into its two branches.
///
/// Every branch of every conditional in the file is a literal or a single call, so the
/// last spaced colon is the separator.
fn conditional(expression: &str) -> Option<(&str, &str)> {
    expression.strip_prefix("dark ? ")?.rsplit_once(" : ")
}

/// The arguments of the `name(...)` call, when the expression is that call.
fn call<'a>(expression: &'a str, name: &str) -> Option<&'a str> {
    expression
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// Reads a `#RRGGBB` literal. Slint's six-digit form is opaque.
fn hex_color(hex: &str) -> Option<Rgba8> {
    if hex.len() != 6 {
        return None;
    }
    Some(Rgba8 {
        r: u8::from_str_radix(hex.get(0..2)?, 16).ok()?,
        g: u8::from_str_radix(hex.get(2..4)?, 16).ok()?,
        b: u8::from_str_radix(hex.get(4..6)?, 16).ok()?,
        a: OPAQUE_ALPHA,
    })
}

/// Reads the arguments of Slint's `rgba(r, g, b, a)` builtin.
fn rgba_builtin(arguments: &str) -> Option<Rgba8> {
    let parts: Vec<&str> = arguments.split(',').map(str::trim).collect();
    let [r, g, b, a] = parts.as_slice() else {
        return None;
    };
    Some(Rgba8 {
        r: r.parse::<u8>().ok()?,
        g: g.parse::<u8>().ok()?,
        b: b.parse::<u8>().ok()?,
        a: truncated_alpha(a.parse::<f32>().ok()?),
    })
}

/// Evaluates an alpha argument: a fraction, a `dark ? a : b` choice of two, or the
/// name of a declaration that holds one.
fn alpha(expression: &str, inputs: &Inputs, properties: &[Declaration<'_>]) -> Option<u8> {
    let expression = expression.trim();
    if let Some((yes, no)) = conditional(expression) {
        return alpha(if inputs.dark { yes } else { no }, inputs, properties);
    }
    if expression == "base-alpha" {
        // Like the accent, the base alpha is an input: the degradation ladder writes it.
        return Some(inputs.base_alpha);
    }
    if let Some(declaration) = properties.iter().find(|entry| entry.name == expression) {
        return alpha(declaration.expression, inputs, properties);
    }
    Some(rounded_alpha(expression.parse::<f32>().ok()?))
}

/// The byte Slint's `Color::with_alpha` produces for a fraction.
fn rounded_alpha(fraction: f32) -> u8 {
    (fraction.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The byte Slint's `rgba()` builtin produces for a fraction, which is a truncation.
fn truncated_alpha(fraction: f32) -> u8 {
    (255.0 * fraction).clamp(0.0, 255.0) as u8
}

/// Evaluates the declaration called `name`.
fn named(name: &str, inputs: &Inputs, properties: &[Declaration<'_>]) -> Option<Rgba8> {
    let declaration = properties.iter().find(|entry| entry.name == name)?;
    evaluate_color(declaration.expression, inputs, properties)
}

/// Every colour token `features.md` 3.2 tabulates, in the order the table lists them,
/// each with the value the Rust palette resolves for it.
///
/// The table has seventeen rows; the eighteenth token is `surface-fill`, the base at
/// `base-alpha`, which is what the panel is filled with and which the global exposes
/// beside `surface-base`.
fn specification_tokens(tokens: &ThemeTokens) -> [(&'static str, Rgba8); 18] {
    [
        ("surface-base", tokens.surface_base),
        ("surface-fill", tokens.surface_fill),
        ("surface-stroke", tokens.surface_stroke),
        ("text-primary", tokens.text_primary),
        ("text-secondary", tokens.text_secondary),
        ("text-annotation", tokens.text_annotation),
        ("text-separator", tokens.text_separator),
        ("accent", tokens.accent),
        ("accent-on", tokens.accent_on),
        ("state-hover", tokens.state_hover),
        ("state-selected-bg", tokens.state_selected_bg),
        ("state-selected-stroke", tokens.state_selected_stroke),
        ("state-pressed", tokens.state_pressed),
        ("separator", tokens.separator),
        ("shadow-inner", tokens.shadow_inner),
        ("shadow-outer", tokens.shadow_outer),
        ("status-dot-active", tokens.status_dot_active),
        ("status-dot-idle", tokens.status_dot_idle),
    ]
}

/// Asserts that every colour `theme.slint` declares evaluates to the token the Rust
/// palette resolves for the same request.
fn assert_slint_palette_matches(scheme: ColorScheme) {
    let tokens = tokens_for(&spec(scheme), BlurNegotiation::Applied);
    let properties = declarations(SLINT_THEME);
    let inputs = Inputs {
        dark: tokens.dark,
        accent: tokens.accent,
        base_alpha: tokens.base_alpha,
    };

    for (name, expected) in specification_tokens(&tokens) {
        let declared = declared_expression(&properties, name);
        let actual = declared.and_then(|text| evaluate_color(text, &inputs, &properties));
        assert_eq!(
            actual,
            Some(expected),
            "theme.slint's {name} is {declared:?}"
        );
    }
}

#[test]
fn test_slint_theme_matches_the_rust_palette_dark() {
    assert_slint_palette_matches(ColorScheme::Dark);
}

#[test]
fn test_slint_theme_matches_the_rust_palette_light() {
    assert_slint_palette_matches(ColorScheme::Light);
}

#[test]
fn test_slint_theme_declares_every_specification_token() {
    // Both directions matter: a token the table names and the file drops would leave a
    // visual undefined, and one the file adds would be a colour no test covers.
    let tokens = tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied);
    let mut declared: Vec<&str> = declarations(SLINT_THEME)
        .into_iter()
        .filter(|entry| entry.type_name == "color")
        .map(|entry| entry.name)
        .collect();
    let mut expected: Vec<&str> = specification_tokens(&tokens)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    declared.sort_unstable();
    expected.sort_unstable();

    assert_eq!(
        declared, expected,
        "theme.slint's tokens are not the table's"
    );
}

#[test]
fn test_slint_theme_takes_only_three_inputs() {
    // The window derives every token from three properties, which is what makes a
    // theme switch a property update rather than a rebuild. A fourth input would be a
    // fourth write, and the claim that a switch cannot flash would stop holding.
    let inputs: Vec<&str> = declarations(SLINT_THEME)
        .into_iter()
        .filter(|entry| entry.input)
        .map(|entry| entry.name)
        .collect();

    assert_eq!(inputs.as_slice(), ["dark", "accent", "base-alpha"]);
}

#[test]
fn test_slint_theme_defaults_to_the_dark_palette() {
    // A system that reports no preference is drawn dark before the first `apply` runs,
    // which is the scheme `resolve_scheme` returns for the same silence.
    let properties = declarations(SLINT_THEME);

    assert_eq!(declared_expression(&properties, "dark"), Some("true"));
}

#[test]
fn test_slint_theme_default_base_alpha_is_the_shipped_default() {
    // `base-alpha` is the one value the degradation ladder writes, so the fraction the
    // file ships has to be 3.2's 0.85 -- the byte `DEFAULT_BASE_ALPHA` holds.
    let properties = declarations(SLINT_THEME);
    let inputs = Inputs {
        dark: true,
        accent: DEFAULT_ACCENT_DARK,
        base_alpha: DEFAULT_BASE_ALPHA,
    };
    let declared = declared_expression(&properties, "base-alpha");

    let actual = declared.and_then(|text| alpha(text, &inputs, &properties));
    assert_eq!(actual, Some(DEFAULT_BASE_ALPHA));
}

#[test]
fn test_slint_theme_derives_the_selected_tokens_from_the_accent() {
    // The portal's accent reaches the window through one property, so the tokens the
    // table derives from the accent have to be expressions over that property rather
    // than constants that happen to match the shipped default.
    let accent = rgba(0xE0, 0x1B, 0x24, OPAQUE_ALPHA);
    let custom = spec_with(ColorScheme::Dark, accent, true, DEFAULT_BASE_ALPHA);
    let tokens = tokens_for(&custom, BlurNegotiation::Applied);
    let properties = declarations(SLINT_THEME);
    let inputs = Inputs {
        dark: true,
        accent,
        base_alpha: DEFAULT_BASE_ALPHA,
    };

    assert_ne!(
        tokens.state_selected_bg,
        tokens_for(&spec(ColorScheme::Dark), BlurNegotiation::Applied).state_selected_bg,
        "the accent under test has to change the selected cell"
    );
    for (name, expected) in [
        ("state-selected-bg", tokens.state_selected_bg),
        ("state-selected-stroke", tokens.state_selected_stroke),
        ("status-dot-active", tokens.status_dot_active),
    ] {
        let declared = declared_expression(&properties, name);
        let actual = declared.and_then(|text| evaluate_color(text, &inputs, &properties));
        assert_eq!(
            actual,
            Some(expected),
            "theme.slint's {name} ignores the accent"
        );
    }
}
