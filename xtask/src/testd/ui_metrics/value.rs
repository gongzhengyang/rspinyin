//! The values a specification, a declaration and a running window can state.
//!
//! Responsibility: name the units a size is stated in and give one value type that all three
//! sources can be reduced to, so that a comparison between any two of them is one function.
//! Nothing here reads a document; the parsers that produce these values live beside this file.

use ime_types::Rgba8;

/// The unit a row of `features.md` 3.1.1 states a value in.
///
/// The unit is part of the identity of a value, not a label on it: `候选单元内边距` states a
/// horizontal padding and a vertical one, and `Header 字号/字重` states three type sizes, so
/// "the first value of this row" is only meaningful once the unit is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unit {
    /// Device-independent pixels, the unit 3.1 states every length in.
    Dp,
    /// Scaled points, the unit the type scale of 3.1.4 is stated in.
    Sp,
    /// A whole number with no unit: a per-row maximum, a page count, an icon count.
    Count,
}

impl Unit {
    /// The suffix the document writes after the number.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Dp => "dp",
            Self::Sp => "sp",
            Self::Count => "",
        }
    }
}

/// A value one of the three sources states.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricValue {
    /// A length in device-independent pixels.
    LengthDp(u16),
    /// A font size in scaled points.
    FontSp(u16),
    /// A whole number with no unit.
    Count(u16),
    /// An alpha channel, as the byte the renderer writes.
    Alpha8(u8),
    /// A colour, as its four channels.
    Colour(Rgba8),
    /// A declaration this harness cannot read as a number, kept verbatim.
    ///
    /// The alternative is to drop it, and a dropped declaration is one that a comparison can
    /// never report -- a `.slint` constant that was changed to an expression would silently stop
    /// being checked. The text is a source file's own, so carrying it is safe.
    Unreadable(String),
}

impl MetricValue {
    /// The whole number the value is, when it is one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn number(&self) -> Option<u16> {
        match self {
            Self::LengthDp(value) | Self::FontSp(value) | Self::Count(value) => Some(*value),
            Self::Alpha8(value) => Some(u16::from(*value)),
            Self::Colour(_) | Self::Unreadable(_) => None,
        }
    }

    /// The value as a report prints it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::LengthDp(value) => format!("{value}dp"),
            Self::FontSp(value) => format!("{value}sp"),
            Self::Count(value) => value.to_string(),
            Self::Alpha8(value) => format!("alpha {value}"),
            Self::Colour(colour) => {
                format!(
                    "#{:02X}{:02X}{:02X}{:02X}",
                    colour.r, colour.g, colour.b, colour.a
                )
            }
            Self::Unreadable(text) => text.clone(),
        }
    }
}

/// The byte a fraction is written as, rounded the way the Slint runtime rounds it.
///
/// `with-alpha()` rounds where the `rgba()` builtin truncates, and this is the rounding one: it
/// is the conversion `theme.rs` holds `DEFAULT_BASE_ALPHA` in, and the one
/// `scripts/check-ui-spec.sh` asserts the 3.2 base token with. A fraction that is not a number
/// answers zero rather than panicking, because the value came from a document.
///
/// # Panics
///
/// Never.
pub fn alpha_byte(fraction: f32) -> u8 {
    let scaled = fraction * 255.0;
    if !scaled.is_finite() {
        return 0;
    }
    scaled.round().clamp(0.0, 255.0) as u8
}
