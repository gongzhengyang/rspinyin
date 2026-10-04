//! The two palettes against the specification table, and the frozen fractions the
//! gate and the view share.
//!
//! The palette tests resolve the shipped acrylic request and pin every token byte by
//! byte, which is what keeps the 3.2 table and its Rust copy one table. The fraction
//! tests hold `ui/candidate_grid.slint`'s dimmed bindings and the dark column's
//! separator and secondary fractions to the constants the gate measures with, so
//! neither copy can move alone.

use ime_types::ColorScheme;

use super::{rgba, spec, spec_with, tokens_for};
use crate::theme::{
    ANNOTATION_ALPHA, ANNOTATION_OPACITY, BLACK, BlurNegotiation, DEFAULT_ACCENT_DARK,
    DEFAULT_ACCENT_LIGHT, NUMBER_LABEL_ALPHA, NUMBER_LABEL_OPACITY, OPAQUE_ALPHA,
    SECONDARY_FRACTION_DARK, SEPARATOR_FRACTION_DARK, WHITE, slint_palette::rounded_alpha,
};

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

/// `ui/candidate_grid.slint`, as text: the cells and their dimmed text.
const SLINT_GRID: &str = include_str!("../../../ui/candidate_grid.slint");

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
