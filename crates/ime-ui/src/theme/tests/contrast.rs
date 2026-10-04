//! The contrast gate: the pairs 3.2 requires at every tier, the five dimmed pairs the
//! view draws while the base is translucent, and the property sweeps that hold the
//! gate to its guarantee whatever the accent or the base alpha is.

use ime_types::ColorScheme;

use super::{rgba, spec, spec_with, tokens_for};
use crate::theme::{
    BLACK, BlurNegotiation, CONTRAST_BODY, CONTRAST_MINIMUM, ContrastReport, DEFAULT_BASE_ALPHA,
    OPAQUE_ALPHA, ThemeResolution, WHITE, composite_over, contrast_ratio, default_accent,
};

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
