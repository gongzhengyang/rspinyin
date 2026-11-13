//! The palette tables: the 3.2 token table's two columns as static data, and the
//! constructor that stamps a [`ThemeTokens`] set out of an accent and a base alpha.

use ime_types::{ColorScheme, Rgba8};

use super::OPAQUE_ALPHA;
use super::color::with_alpha;
use super::tokens::ThemeTokens;

/// One scheme's palette, exactly as `features.md` 3.2 tabulates it.
///
/// The alphas are the table's fractions in the 8 bits the framebuffer takes. A token
/// `ui/theme.slint` builds with `with-alpha()` carries `round(fraction * 255)`; a token
/// it builds with the `rgba()` builtin carries `trunc(fraction * 255)`, which is what
/// that builtin does. Each of the seven tokens the two conventions separate by a step
/// says so where it is declared.
#[derive(Clone, Copy)]
pub(super) struct Palette {
    dark: bool,
    surface_base: Rgba8,
    surface_stroke: Rgba8,
    text_primary: Rgba8,
    text_secondary: Rgba8,
    text_annotation: Rgba8,
    text_separator: Rgba8,
    accent_on: Rgba8,
    state_hover: Rgba8,
    state_pressed: Rgba8,
    separator: Rgba8,
    shadow_inner: Rgba8,
    shadow_outer: Rgba8,
    status_dot_idle: Rgba8,
    /// `state.selected.bg`: the accent at 0.18 in the dark palette, 0.14 in the light.
    selected_bg_alpha: u8,
    /// `state.selected.stroke`: the accent at 0.55 dark, 0.50 light.
    selected_stroke_alpha: u8,
}

/// The dark column of the 3.2 table.
const DARK_PALETTE: Palette = Palette {
    dark: true,
    // 3.2 `surface.base`: #1C1C1E, opaque here; the acrylic alpha is `base-alpha`.
    surface_base: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `surface.stroke`: rgba(255,255,255,0.10), truncated to 25 by Slint's `rgba()`.
    surface_stroke: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: 25,
    },
    // 3.2 `text.primary`: #F2F2F7.
    text_primary: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `text.secondary`: rgba(242,242,247,0.62).
    text_secondary: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 158,
    },
    // 3.2 `text.annotation`: rgba(242,242,247,0.48).
    text_annotation: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 122,
    },
    // 3.2 `text.separator`: rgba(242,242,247,0.40).
    text_separator: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 102,
    },
    // 3.2 `accent.on`: #FFFFFF.
    accent_on: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `state.hover`: rgba(242,242,247,0.08).
    state_hover: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 20,
    },
    // 3.2 `state.pressed`: rgba(242,242,247,0.14), truncated to 35 by Slint's `rgba()`.
    state_pressed: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 35,
    },
    // 3.2 `separator`: rgba(242,242,247,0.10), truncated to 25 by Slint's `rgba()`.
    separator: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 25,
    },
    // 3.2 `shadow.inner`: rgba(0,0,0,0.35).
    shadow_inner: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 89,
    },
    // 3.2 `shadow.outer`: rgba(0,0,0,0.42).
    shadow_outer: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 107,
    },
    // 3.2 `status.dot.idle`: rgba(242,242,247,0.35).
    status_dot_idle: Rgba8 {
        r: 0xF2,
        g: 0xF2,
        b: 0xF7,
        a: 89,
    },
    // 3.2 `state.selected.bg`: accent @ 0.18.
    selected_bg_alpha: 46,
    // 3.2 `state.selected.stroke`: accent @ 0.55.
    selected_stroke_alpha: 140,
};

/// The light column of the 3.2 table.
const LIGHT_PALETTE: Palette = Palette {
    dark: false,
    // 3.2 `surface.base`: #FFFFFF.
    surface_base: Rgba8 {
        r: 0xFF,
        g: 0xFF,
        b: 0xFF,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `surface.stroke`: rgba(0,0,0,0.06).
    surface_stroke: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 15,
    },
    // 3.2 `text.primary`: #1C1C1E.
    text_primary: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `text.secondary`: rgba(28,28,30,0.60).
    text_secondary: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 153,
    },
    // 3.2 `text.annotation`: rgba(28,28,30,0.45), truncated to 114 by Slint's `rgba()`.
    text_annotation: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 114,
    },
    // 3.2 `text.separator`: rgba(28,28,30,0.35).
    text_separator: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 89,
    },
    // 3.2 `accent.on`: #FFFFFF.
    accent_on: Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: OPAQUE_ALPHA,
    },
    // 3.2 `state.hover`: rgba(28,28,30,0.06).
    state_hover: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 15,
    },
    // 3.2 `state.pressed`: rgba(28,28,30,0.12), truncated to 30 by Slint's `rgba()`.
    state_pressed: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 30,
    },
    // 3.2 `separator`: rgba(28,28,30,0.08).
    separator: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 20,
    },
    // 3.2 `shadow.inner`: rgba(0,0,0,0.08).
    shadow_inner: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 20,
    },
    // 3.2 `shadow.outer`: rgba(0,0,0,0.16), truncated to 40 by Slint's `rgba()`.
    shadow_outer: Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 40,
    },
    // 3.2 `status.dot.idle`: rgba(28,28,30,0.30), truncated to 76 by Slint's `rgba()`.
    status_dot_idle: Rgba8 {
        r: 0x1C,
        g: 0x1C,
        b: 0x1E,
        a: 76,
    },
    // 3.2 `state.selected.bg`: accent @ 0.14.
    selected_bg_alpha: 36,
    // 3.2 `state.selected.stroke`: accent @ 0.50.
    selected_stroke_alpha: 128,
};

/// The palette of a scheme.
pub(super) fn palette_for(scheme: ColorScheme) -> Palette {
    match scheme {
        ColorScheme::Dark => DARK_PALETTE,
        ColorScheme::Light => LIGHT_PALETTE,
    }
}

impl Palette {
    /// Builds the token set for an accent and a base alpha.
    ///
    /// The four tokens the table derives from the accent -- the selected cell's
    /// background and ring, and the two status dots -- are computed here, in the one
    /// place, and mirrored by the expressions in `ui/theme.slint`.
    pub(super) fn tokens(self, accent: Rgba8, base_alpha: u8) -> ThemeTokens {
        ThemeTokens {
            dark: self.dark,
            base_alpha,
            surface_base: self.surface_base,
            surface_fill: with_alpha(self.surface_base, base_alpha),
            surface_stroke: self.surface_stroke,
            text_primary: self.text_primary,
            text_secondary: self.text_secondary,
            text_annotation: self.text_annotation,
            text_separator: self.text_separator,
            accent,
            accent_on: self.accent_on,
            state_hover: self.state_hover,
            state_selected_bg: with_alpha(accent, self.selected_bg_alpha),
            state_selected_stroke: with_alpha(accent, self.selected_stroke_alpha),
            state_pressed: self.state_pressed,
            separator: self.separator,
            shadow_inner: self.shadow_inner,
            shadow_outer: self.shadow_outer,
            status_dot_active: accent,
            status_dot_idle: self.status_dot_idle,
        }
    }
}
