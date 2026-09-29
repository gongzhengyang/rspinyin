//! The `ui/theme.slint` copy of the palette, and the tests that keep it in step with
//! the one in `src/theme.rs`.
//!
//! The same file declares the window's two typographic tokens -- the font family the probe
//! resolves and the optical nudge every text run takes -- and neither of them has a copy in
//! `src/theme.rs`, so the tests below are the only thing that pins them. A token no text
//! element draws with is a token that does nothing, so those tests read the two view files
//! as well: every run of text takes the family, both text containers take the nudge, and the
//! header starts its content on the candidate grid's left edge.
//!
//! It declares the two shadow layers' band colours for the same reason, and those have no
//! copy in Rust at all: 3.1.2 fixes the layers and `ui/theme.slint` computes the falloff the
//! rasterizer needs, so the tests below are what keeps that falloff the one 3.1.2 describes
//! rather than a table that merely looks plausible.
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
///
/// `pub(super)` rather than private: the sibling `tests` module pins the view's shadow
/// material against these two files as well, and reading one copy of them twice is what keeps
/// the two halves of that contract from drifting apart.
pub(super) const SLINT_THEME: &str = include_str!("../../ui/theme.slint");

/// `ui/candidate.slint`, as text: the window, its shadow layers and its header strip.
pub(super) const SLINT_CANDIDATE: &str = include_str!("../../ui/candidate.slint");

/// `ui/candidate_grid.slint`, as text: the candidate area and its cells.
const SLINT_CANDIDATE_GRID: &str = include_str!("../../ui/candidate_grid.slint");

/// Which side of the Slint boundary may write a property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    /// Written by the Rust side, read by the window: the three a theme switch sets.
    In,
    /// Computed inside the window, read by the Rust side.
    Out,
    /// Carries a value of its own that the Rust side may overwrite.
    InOut,
}

/// One property declaration of a `.slint` file.
struct Declaration<'a> {
    /// The property's name.
    name: &'a str,
    /// Which side may write the property.
    direction: Direction,
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
    let (direction, rest) = if let Some(rest) = line.strip_prefix("in-out property ") {
        (Direction::InOut, rest)
    } else if let Some(rest) = line.strip_prefix("in property ") {
        (Direction::In, rest)
    } else {
        (Direction::Out, line.strip_prefix("out property ")?)
    };
    let (type_name, rest) = rest.strip_prefix('<')?.split_once('>')?;
    let (name, expression) = rest.split_once(':')?;
    Some(Declaration {
        name: name.trim(),
        direction,
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
pub(super) fn rounded_alpha(fraction: f32) -> u8 {
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
    // fourth write, and the claim that a switch cannot flash would stop holding. The
    // font family is `in-out` and so is not one of these three: it carries a value the
    // window draws with before the probe answers, and it is written once at startup
    // rather than on a switch.
    let inputs: Vec<&str> = declarations(SLINT_THEME)
        .into_iter()
        .filter(|entry| entry.direction == Direction::In)
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

/// The declaration `ui/theme.slint` carries for `name`.
fn declared<'src>(
    properties: &'src [Declaration<'src>],
    name: &str,
) -> Option<&'src Declaration<'src>> {
    properties.iter().find(|entry| entry.name == name)
}

/// How often `needle` occurs in `haystack`.
fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn test_slint_theme_declares_the_probed_font_family() {
    // The window's text has to be drawn with a family the machine can shape CJK with, so the
    // token carries a named family as its default and is one the Rust side may overwrite. The
    // default is the family every Linux baseline ships, which is also the head of the probe's
    // fallback list: a run drawn before the probe answers is drawn with the best candidate
    // rather than with `sans-serif`, the family `fontdb` resolves to when nothing matched and
    // the one the probe reads as "no CJK family installed".
    let properties = declarations(SLINT_THEME);
    let family = declared(&properties, "font-family");

    assert_eq!(
        family.map(|entry| (entry.type_name, entry.direction)),
        Some(("string", Direction::InOut)),
        "the window draws with a family the probe overwrites"
    );
    let default = family.map(|entry| entry.expression).unwrap_or_default();
    assert_eq!(
        default, "\"Noto Sans CJK SC\"",
        "the default is a named CJK family, not a generic fallback"
    );
}

#[test]
fn test_slint_theme_declares_the_optical_calibration() {
    // The nudge is a sub-pixel calibration rather than a grid size: 3.1.4's 4dp grid governs
    // the window's geometry, and this sits on top of it. A value of a whole pixel or more
    // would mean the text containers were being moved by a layout decision instead, which is
    // what the grid is for.
    let properties = declarations(SLINT_THEME);
    let nudge = declared(&properties, "optical-nudge");

    assert_eq!(
        nudge.map(|entry| (entry.type_name, entry.direction)),
        Some(("length", Direction::Out)),
        "the nudge is a length the window computes"
    );
    let expression = nudge.map(|entry| entry.expression).unwrap_or_default();
    let digits = expression.trim_end_matches("px");
    let value: f32 = digits.parse().unwrap_or_default();
    assert!(
        value > 0.0 && value < 1.0,
        "the nudge is a sub-pixel calibration, got {expression}"
    );
}

#[test]
fn test_view_files_draw_every_text_run_with_the_theme_tokens() {
    // The family has to reach every run of text and not only the candidate cells: a run left
    // on the font stack's own default would be shaped with a family the probe never measured,
    // which is the window full of boxes the probe exists to prevent. The optical nudge has to
    // reach the header strip and the cells alike, or the two lines stop sharing one baseline.
    // Counting the elements against the bindings is what a test can check without a display
    // server; which family they resolve to, and where the glyphs land, needs a rendered frame.
    for (name, source) in [
        ("ui/candidate.slint", SLINT_CANDIDATE),
        ("ui/candidate_grid.slint", SLINT_CANDIDATE_GRID),
    ] {
        let texts = count(source, "Text {");
        assert!(texts > 0, "{name} draws text");
        assert_eq!(
            count(source, "font-family: Theme.font-family;"),
            texts,
            "every text element of {name} draws with the probed family"
        );
        assert!(
            count(source, "Theme.optical-nudge") > 0,
            "{name} places its text on the window's optical baseline"
        );
    }
}

#[test]
fn test_view_header_starts_on_the_candidate_grids_left_edge() {
    // The preedit and the first candidate cell have to share one left edge, and the two sides
    // pad differently: 3.1.1 gives the header 10dp and the candidate area 8dp. The strip spans
    // the panel's full width, so its own left inset *is* the grid's padding -- the number that
    // would put the two edges 2dp apart is `header-padding-h`, and the expression is pinned
    // here because a later edit that put the strip's own padding back on the left would move
    // the preedit off the grid, which is invisible until the two lines are compared on screen.
    let inset = "padding-left: CandidateMetrics.container-padding";

    assert_eq!(
        count(SLINT_CANDIDATE, inset),
        1,
        "the header starts its content on the candidate grid's left edge"
    );
    assert_eq!(
        count(
            SLINT_CANDIDATE,
            "padding-left: CandidateMetrics.header-padding-h"
        ),
        0,
        "the strip's own padding is the right side's, not the left's"
    );
}

// ---------------------------------------------------------------------------
// The two shadow layers, as bands (3.1.2)
// ---------------------------------------------------------------------------

/// How many bands `ui/theme.slint` tiles the 28dp outer blur with.
///
/// The count is part of the material, not of the geometry: the bands have to be narrow
/// enough that the step between two of them is below what an eye reads as an edge, and
/// eight bands of 3.5dp is the fewest that clears that at the 1.0 scale factor this
/// window supports.
const OUTER_BANDS: usize = 8;

/// How many bands `ui/theme.slint` draws the 2dp inner shadow as.
const INNER_BANDS: usize = 2;

/// The alpha of the band hugging the panel, which the outer ramp scales down from.
const OUTER_PEAK: f32 = 0.18;

/// The alpha fraction the outer ramp puts in one band.
///
/// A Gaussian blur of a straight edge leaves a tail that falls off with the square of the
/// distance from the edge, so band `index` -- counted from the outermost, which is band 0
/// -- carries the square of its position in the ramp. The outermost band is therefore
/// `1/64` of the peak rather than nothing: the ramp reaches zero at the outside of the
/// blur radius, which is where the banding stops.
fn outer_band_fraction(index: usize, bands: usize) -> f32 {
    let position = (index + 1) as f32 / bands as f32;
    OUTER_PEAK * position * position
}

/// The elements of a `[color]` array literal, in declaration order.
///
/// The split is on the top-level commas only, because a band is a `with-alpha()` call and
/// a call's own commas are argument separators rather than element separators. An empty
/// literal yields an empty list rather than an error: a table that declares no band is
/// still a table the caller can measure.
fn array_elements(expression: &str) -> Option<Vec<&str>> {
    let inner = expression.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut elements = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in inner.char_indices() {
        match character {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                elements.push(inner.get(start..index)?.trim());
                start = index.saturating_add(1);
            }
            _ => {}
        }
    }
    elements.push(inner.get(start..)?.trim());
    if elements.iter().any(|element| element.is_empty()) {
        return None;
    }
    Some(elements)
}

/// Evaluates one element of a shadow-band table: `<token>.with-alpha(<fraction>)`.
///
/// The token supplies the three channels and the fraction supplies the alpha. That is the
/// whole point of the table: the renderer this project ships does not apply `opacity` to
/// the frame it produces, so a falloff written as an opacity would draw every band at full
/// strength. Baking it into the alpha is the same picture on a renderer that does honour
/// `opacity`, and the only one that is drawn here.
fn evaluate_band(
    expression: &str,
    inputs: &Inputs,
    properties: &[Declaration<'_>],
) -> Option<Rgba8> {
    let (token, argument) = expression.trim().split_once(".with-alpha(")?;
    let colour = named(token.trim(), inputs, properties)?;
    let alpha = band_alpha(argument.strip_suffix(')')?, inputs, properties)?;
    Some(with_alpha(colour, alpha))
}

/// The alpha byte one band paints with, from the fraction its declaration spells.
fn band_alpha(expression: &str, inputs: &Inputs, properties: &[Declaration<'_>]) -> Option<u8> {
    let expression = expression.trim();
    if let Some((left, right)) = expression.split_once(" * ") {
        let product =
            band_fraction(left, inputs, properties)? * band_fraction(right, inputs, properties)?;
        return Some(rounded_alpha(product));
    }
    Some(rounded_alpha(band_fraction(
        expression, inputs, properties,
    )?))
}

/// The alpha fraction one band spells, before it is rounded into a byte.
///
/// A band is a literal, a `dark ? a : b` choice of two, or the name of a declaration that
/// holds one; the inner layer's bands are the token's own alpha times a weight, which is
/// why the two sides of the product are resolved separately.
fn band_fraction(expression: &str, inputs: &Inputs, properties: &[Declaration<'_>]) -> Option<f32> {
    let expression = expression.trim();
    let expression = expression
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(expression);
    if let Some((yes, no)) = conditional(expression) {
        return band_fraction(if inputs.dark { yes } else { no }, inputs, properties);
    }
    if let Some(declaration) = properties.iter().find(|entry| entry.name == expression) {
        return band_fraction(declaration.expression, inputs, properties);
    }
    expression.parse::<f32>().ok()
}

/// Evaluates one of the theme's `[color]` array declarations for a scheme.
///
/// `pub(super)` for the same reason the two sources above are: the sibling `tests` module
/// measures the view's band counts and peaks against what this returns.
pub(super) fn evaluate_bands(source: &str, name: &str, scheme: ColorScheme) -> Vec<Rgba8> {
    let tokens = tokens_for(&spec(scheme), BlurNegotiation::Applied);
    let properties = declarations(source);
    let inputs = Inputs {
        dark: tokens.dark,
        accent: tokens.accent,
        base_alpha: tokens.base_alpha,
    };
    let Some(expression) = declared_expression(&properties, name) else {
        panic!("theme.slint declares {name}");
    };
    let Some(elements) = array_elements(expression) else {
        panic!("{name} is an array literal");
    };
    let mut bands = Vec::new();
    for element in elements {
        let Some(band) = evaluate_band(element, &inputs, &properties) else {
            panic!("{name} holds a band colour, got {element:?}");
        };
        bands.push(band);
    }
    bands
}

#[test]
fn test_array_elements_splits_only_on_top_level_commas() {
    // The band tables are the only array literals the theme declares, and their elements
    // are calls. Splitting on a call's own commas would tear a band in half and the table
    // would measure as more bands than the material has.
    assert_eq!(array_elements("[]"), Some(Vec::new()));
    assert_eq!(array_elements("  [ ]  "), Some(Vec::new()));
    assert_eq!(
        array_elements("[a.with-alpha(1, 2), b.with-alpha(3)]"),
        Some(vec!["a.with-alpha(1, 2)", "b.with-alpha(3)"])
    );
    assert_eq!(array_elements("[a, b, c]"), Some(vec!["a", "b", "c"]));
    assert_eq!(array_elements("a, b"), None, "a table is bracketed");
    assert_eq!(array_elements("[a,, b]"), None, "a band is not empty");
}

#[test]
fn test_slint_theme_outer_shadow_bands_follow_the_quadratic_ramp() {
    // 3.1.2's L3 is `0 8dp 28dp`. The bands tile the blur and the falloff is the square of
    // the band's position in the ramp, and the table has to obey that law rather than
    // merely look plausible: because the bands tile without overlapping, the composite
    // alpha at any point is the alpha of the single band covering it, so a table that is
    // flat -- or linear -- rasterizes to one wash with a hard outer edge.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let bands = evaluate_bands(SLINT_THEME, "shadow-outer-bands", scheme);
        assert_eq!(
            bands.len(),
            OUTER_BANDS,
            "the ramp tiles the 28dp blur in {OUTER_BANDS} bands"
        );
        for (index, band) in bands.iter().enumerate() {
            let expected = rounded_alpha(outer_band_fraction(index, OUTER_BANDS));
            assert_eq!(
                band.a, expected,
                "{scheme:?} band {index} paints at {} of 255, not {expected}",
                band.a
            );
        }
    }
}

#[test]
fn test_slint_theme_outer_shadow_bands_rise_from_the_outside_in() {
    // Band 0 is the widest and faintest ring and the last band hugs the panel, so the
    // alphas have to rise strictly. An out-of-order table would draw a bright line inside
    // the falloff, which is the one artefact a soft shadow must not have.
    let bands = evaluate_bands(SLINT_THEME, "shadow-outer-bands", ColorScheme::Dark);
    assert_eq!(bands.len(), OUTER_BANDS);
    for pair in bands.windows(2) {
        assert!(
            pair[1].a > pair[0].a,
            "the ramp is not rising: {} then {}",
            pair[0].a,
            pair[1].a
        );
    }
    // The boundary of the ramp: the outermost band is one step above nothing rather than
    // zero, because a band that paints nothing is a band the blur does not need, and the
    // innermost carries the peak 3.1.2 names.
    assert!(bands[0].a > 0, "the outermost band still paints");
    assert_eq!(
        bands.last().map(|band| band.a),
        Some(rounded_alpha(OUTER_PEAK)),
        "the innermost band carries the peak"
    );
}

#[test]
fn test_slint_theme_outer_shadow_ramp_is_quadratic_not_linear() {
    // The reason 3.1.2 asks for the square rather than a straight line: a linear ramp
    // reads visibly too bright in the middle of the falloff. Asserting the midpoint sits
    // under the straight line between the two ends is what pins the exponent, so a later
    // edit to a linear table fails here rather than on someone's screen.
    let bands = evaluate_bands(SLINT_THEME, "shadow-outer-bands", ColorScheme::Dark);
    assert_eq!(bands.len(), OUTER_BANDS);
    let first = f32::from(bands[0].a);
    let last = f32::from(bands[OUTER_BANDS - 1].a);
    let middle = f32::from(bands[OUTER_BANDS / 2].a);
    let straight = first + (last - first) / 2.0;

    assert!(
        middle < straight,
        "the ramp's midpoint {middle} is not under the straight line {straight}"
    );
    assert!(middle > first, "the ramp rises through its middle");
}

#[test]
fn test_slint_theme_inner_shadow_bands_scale_with_the_inner_token() {
    // 3.1.2's L2 is `0 1dp 2dp shadow.inner`, drawn as two 1dp bands: the one against the
    // panel at three quarters of the token's alpha and the one outside it at a third. Both
    // are fractions of `shadow-inner`, which is what makes the layer follow the scheme --
    // a pinned alpha would draw the light scheme's edge as dark as the dark scheme's.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let token = tokens_for(&spec(scheme), BlurNegotiation::Applied).shadow_inner;
        let fraction = f32::from(token.a) / 255.0;
        let bands = evaluate_bands(SLINT_THEME, "shadow-inner-bands", scheme);

        assert_eq!(bands.len(), INNER_BANDS, "3.1.2 asks for two 1dp bands");
        assert_eq!(
            bands[0].a,
            rounded_alpha(fraction * 0.35),
            "{scheme:?}: the outer inner band carries a third of the token"
        );
        assert_eq!(
            bands[1].a,
            rounded_alpha(fraction * 0.75),
            "{scheme:?}: the inner inner band carries three quarters of the token"
        );
        assert!(
            bands[1].a > bands[0].a,
            "{scheme:?}: the band against the panel is the darker one"
        );
    }

    let dark = evaluate_bands(SLINT_THEME, "shadow-inner-bands", ColorScheme::Dark);
    let light = evaluate_bands(SLINT_THEME, "shadow-inner-bands", ColorScheme::Light);
    assert!(
        dark[INNER_BANDS - 1].a > light[INNER_BANDS - 1].a,
        "the inner layer is lighter in the light scheme"
    );
}

#[test]
fn test_slint_theme_shadow_bands_are_painted_in_the_layer_token() {
    // The bands are the layer's own colour at the ramp's alpha and not a second black the
    // material would have to keep in step by hand, so the three channels have to be the
    // token's. Alpha is the one channel the ramp is allowed to move.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let tokens = tokens_for(&spec(scheme), BlurNegotiation::Applied);
        for (name, token) in [
            ("shadow-outer-bands", tokens.shadow_outer),
            ("shadow-inner-bands", tokens.shadow_inner),
        ] {
            let bands = evaluate_bands(SLINT_THEME, name, scheme);
            assert!(!bands.is_empty(), "{name} declares the layer's bands");
            for band in bands {
                assert_eq!(
                    (band.r, band.g, band.b),
                    (token.r, token.g, token.b),
                    "{scheme:?}: {name} paints in a colour that is not its token's"
                );
            }
        }
    }
}

#[test]
fn test_slint_theme_shadow_bands_never_reach_the_base_alpha_of_the_panel() {
    // A shadow band is a hole in the desktop, not a hole in the window: the strongest band
    // of either layer has to stay well under opaque, or the reserve would read as a frame
    // drawn around the panel instead of a shadow cast by it. The bound is the panel's own
    // acrylic alpha, which is the most opaque thing either layer may approach.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let base_alpha = tokens_for(&spec(scheme), BlurNegotiation::Applied).base_alpha;
        for name in ["shadow-outer-bands", "shadow-inner-bands"] {
            for band in evaluate_bands(SLINT_THEME, name, scheme) {
                assert!(
                    band.a < base_alpha,
                    "{scheme:?}: {name} paints a band at {} of 255, over the panel's {base_alpha}",
                    band.a
                );
            }
        }
    }
}
