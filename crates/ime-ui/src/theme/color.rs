//! Colour arithmetic for the theme pipeline: alpha compositing and the WCAG
//! contrast ratio.
//!
//! Compositing is integer arithmetic on purpose. A token that reaches the
//! framebuffer must not depend on how a floating-point library rounds, and the
//! integer form reproduces the worked examples in `features.md` 3.2 exactly:
//! `#1C1C1E` at alpha 0.85 over white is `#3E3E40`, and `#FFFFFF` at 0.85 over black
//! is `#D9D9D9`.
//!
//! The contrast ratio is the one place floating point is allowed, because the WCAG
//! formula is defined on linearised luminance. It only ever feeds a threshold
//! comparison, and the closest pair either palette can produce clears its threshold
//! by more than 20%, so no rounding difference can move the decision.

use ime_types::Rgba8;

/// Opaque white: the backdrop the dark palette is measured against, because a white
/// desktop washes a translucent dark base out the most.
pub const WHITE: Rgba8 = Rgba8 {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// Opaque black: the backdrop the light palette is measured against.
pub const BLACK: Rgba8 = Rgba8 {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

/// Replaces a colour's alpha channel, keeping its three colour channels.
///
/// # Parameters
///
/// - `colour`: the colour to re-alpha.
/// - `alpha`: the new alpha, `0..=255`.
///
/// # Returns
///
/// The same colour at the given alpha.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub const fn with_alpha(colour: Rgba8, alpha: u8) -> Rgba8 {
    Rgba8 {
        r: colour.r,
        g: colour.g,
        b: colour.b,
        a: alpha,
    }
}

/// Flattens `foreground` over an opaque `backdrop`, source-over.
///
/// # Parameters
///
/// - `foreground`: the layer being painted, at any alpha.
/// - `backdrop`: what is already there. It is treated as opaque, so a translucent
///   backdrop has to be flattened onto its own backdrop first.
///
/// # Returns
///
/// The opaque colour the two layers add up to, each channel rounded half up. The
/// rounding is what makes the acrylic base composite to exactly the values the
/// design document computes by hand.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub const fn composite_over(foreground: Rgba8, backdrop: Rgba8) -> Rgba8 {
    let alpha = foreground.a as u32;
    let inverse = 255 - alpha;
    Rgba8 {
        r: blend_channel(foreground.r, backdrop.r, alpha, inverse),
        g: blend_channel(foreground.g, backdrop.g, alpha, inverse),
        b: blend_channel(foreground.b, backdrop.b, alpha, inverse),
        a: 255,
    }
}

/// One channel of a source-over blend.
///
/// The `+ 127` before the division is round-half-up for integer inputs: the exact
/// halfway point is a fractional remainder of 127.5, which an integer numerator
/// cannot reach, so adding 127 rounds at the same place as adding 127.5 would.
const fn blend_channel(foreground: u8, backdrop: u8, alpha: u32, inverse: u32) -> u8 {
    ((foreground as u32 * alpha + backdrop as u32 * inverse + 127) / 255) as u8
}

/// The WCAG 2.x relative luminance of an opaque colour.
///
/// # Parameters
///
/// - `colour`: an opaque colour. Its alpha channel is ignored, because a luminance
///   is only defined for a colour that has been flattened onto its backdrop.
///
/// # Returns
///
/// The relative luminance: `0.0` for black and `1.0` for white.
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn relative_luminance(colour: Rgba8) -> f32 {
    const RED: f32 = 0.2126;
    const GREEN: f32 = 0.7152;
    const BLUE: f32 = 0.0722;
    RED * linearise(colour.r) + GREEN * linearise(colour.g) + BLUE * linearise(colour.b)
}

/// The WCAG 2.x contrast ratio between two colours.
///
/// `foreground` is flattened over `background` first, so a translucent text colour
/// is measured as it is actually seen rather than at its own alpha.
///
/// # Parameters
///
/// - `foreground`: the text or symbol colour.
/// - `background`: what it is drawn on, treated as opaque.
///
/// # Returns
///
/// The ratio, between `1.0` (the two colours are indistinguishable) and `21.0`
/// (black on white).
///
/// # Errors
///
/// This function is infallible: it returns no `Result`.
///
/// # Panics
///
/// Never panics.
pub fn contrast_ratio(foreground: Rgba8, background: Rgba8) -> f32 {
    let flattened = relative_luminance(composite_over(foreground, background));
    let backdrop = relative_luminance(background);
    let (lighter, darker) = if flattened >= backdrop {
        (flattened, backdrop)
    } else {
        (backdrop, flattened)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// Linearises one sRGB channel, the first step of the WCAG luminance formula.
fn linearise(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba8 {
        Rgba8 { r, g, b, a }
    }

    #[test]
    fn test_with_alpha_replaces_only_the_alpha() {
        let colour = rgba(0x4C, 0x9A, 0xFF, 255);
        let faded = with_alpha(colour, 46);
        assert_eq!(faded, rgba(0x4C, 0x9A, 0xFF, 46));
        assert_eq!(faded.r, colour.r);
        assert_eq!(faded.g, colour.g);
        assert_eq!(faded.b, colour.b);
    }

    #[test]
    fn test_composite_over_matches_the_specified_worked_examples() {
        // features.md 3.2 computes these two by hand; a change in the rounding would
        // make the acrylic base drift away from the document.
        let dark_base = rgba(0x1C, 0x1C, 0x1E, 217);
        assert_eq!(
            composite_over(dark_base, WHITE),
            rgba(0x3E, 0x3E, 0x40, 255)
        );

        let light_base = rgba(0xFF, 0xFF, 0xFF, 217);
        assert_eq!(
            composite_over(light_base, BLACK),
            rgba(0xD9, 0xD9, 0xD9, 255)
        );
    }

    #[test]
    fn test_composite_over_of_an_opaque_foreground_is_the_foreground() {
        let foreground = rgba(0x0A, 0x6C, 0xFF, 255);
        let blended = composite_over(foreground, rgba(0x12, 0x34, 0x56, 255));
        assert_eq!(blended, foreground);
    }

    #[test]
    fn test_composite_over_of_a_transparent_foreground_is_the_backdrop() {
        let backdrop = rgba(0x12, 0x34, 0x56, 255);
        let blended = composite_over(rgba(0xFF, 0xFF, 0xFF, 0), backdrop);
        assert_eq!(blended, backdrop);
    }

    #[test]
    fn test_composite_over_flattens_a_half_transparent_layer_symmetrically() {
        let blended = composite_over(rgba(0, 0, 0, 128), WHITE);
        assert_eq!(blended, rgba(127, 127, 127, 255));
    }

    #[test]
    fn test_relative_luminance_orders_black_below_white() {
        let black = relative_luminance(BLACK);
        let white = relative_luminance(WHITE);
        assert!(black.abs() < f32::EPSILON);
        assert!((white - 1.0).abs() < 1.0e-6);
        assert!(black < white);
    }

    #[test]
    fn test_contrast_ratio_of_black_on_white_is_the_maximum() {
        let ratio = contrast_ratio(BLACK, WHITE);
        assert!((ratio - 21.0).abs() < 0.01, "black on white was {ratio}");
    }

    #[test]
    fn test_contrast_ratio_of_a_colour_against_itself_is_one() {
        let grey = rgba(0x80, 0x80, 0x80, 255);
        assert!((contrast_ratio(grey, grey) - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn test_contrast_ratio_is_symmetric_in_its_arguments() {
        let text = rgba(0xF2, 0xF2, 0xF7, 255);
        let surface = rgba(0x1C, 0x1C, 0x1E, 255);
        assert!((contrast_ratio(text, surface) - contrast_ratio(surface, text)).abs() < 1.0e-6);
    }

    #[test]
    fn test_contrast_ratio_flattens_a_semi_transparent_foreground() {
        // Fading the text towards the surface can only reduce the ratio, which is why
        // the theme measures the flattened colour rather than the raw one.
        let surface = rgba(0x1C, 0x1C, 0x1E, 255);
        let opaque = contrast_ratio(rgba(0xF2, 0xF2, 0xF7, 255), surface);
        let faded = contrast_ratio(rgba(0xF2, 0xF2, 0xF7, 122), surface);
        assert!(
            faded < opaque,
            "faded {faded} was not below opaque {opaque}"
        );
        assert!(faded > 1.0);
    }
}
