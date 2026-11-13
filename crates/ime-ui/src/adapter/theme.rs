//! The adapter's theme half: the crossfade of the global's accent and base alpha,
//! the status markers' fades beside it, and the writes that keep a settled window
//! free of property stores.

use ime_types::{Rgba8, ThemeSpec};
use slint::Color;

use crate::spring::{Blend, CROSSFADE_S, CubicBezier};
use crate::theme::{ThemeSink, ThemeTokens};

use super::Adapter;
use super::write::WindowTheme;

/// The two theme properties a crossfade can carry, as one blendable value.
///
/// The accent and the base alpha are the pair a resolved theme writes into the
/// component's global; a theme switch glides them together on one clock, so the
/// accent-derived tokens and the acrylic base move as one motion instead of two.
/// The scheme flag itself is not interpolable and is written directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ThemeGlide {
    /// 3.2 `accent.default`, opaque.
    pub(super) accent: Rgba8,
    /// The base alpha the contrast gate approved, `0..=255`.
    pub(super) base_alpha: u8,
}

impl ThemeGlide {
    /// The glide endpoints of a resolved theme.
    fn of(tokens: &ThemeTokens) -> Self {
        Self {
            accent: tokens.accent,
            base_alpha: tokens.base_alpha,
        }
    }
}

impl Blend for ThemeGlide {
    fn blend(from: Self, to: Self, t: u16) -> Self {
        Self {
            accent: Rgba8::blend(from.accent, to.accent, t),
            base_alpha: u8::blend(from.base_alpha, to.base_alpha, t),
        }
    }
}

/// A token colour as the component's colour type.
fn color_of(colour: Rgba8) -> Color {
    Color::from_argb_u8(colour.a, colour.r, colour.g, colour.b)
}

impl Adapter {
    /// Writes a resolved theme into the component, as a crossfade.
    ///
    /// The scheme flag lands at once: it is not interpolable, and it gates the
    /// panel, the grid and every dark-conditional token, so half a scheme would
    /// be a window in no scheme at all. Everything that *can* blend then glides
    /// over 3.3.2's 120ms: the global's accent and base alpha move on the theme
    /// fade, and the status cluster's markers re-target at the new tokens and
    /// crossfade with it. A first application, and a window with the motion
    /// switched off, land everything at once instead -- there is no previous
    /// palette to glide away from, and a screenshot needs a frame that does not
    /// depend on how many frames came before it.
    ///
    /// The container radius is the one value that lives outside the global,
    /// because the component owns its geometry.
    ///
    /// # Parameters
    ///
    /// * `spec` -- the host's theme request, which carries the configured corner radius.
    /// * `tokens` -- the resolution to apply. Apply the tokens rather than the raw request, so
    ///   the alpha written is the one the contrast gate approved.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn apply_theme(&mut self, spec: &ThemeSpec, tokens: &ThemeTokens) {
        let glide = ThemeGlide::of(tokens);
        let instant = !self.themed || !self.motion.is_enabled();
        {
            let mut sink = WindowTheme {
                window: &self.window,
            };
            sink.set_dark(tokens.dark);
            if instant {
                sink.set_accent(glide.accent);
                sink.set_base_alpha(f32::from(glide.base_alpha) / 255.0);
            }
        }
        if instant {
            self.theme_fade.snap_to(glide);
            self.state.set_status_tokens(tokens, false);
        } else {
            self.theme_fade
                .start(glide, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
            self.state.set_status_tokens(tokens, true);
        }
        self.themed = true;
        self.window
            .set_container_radius(f32::from(spec.corner_radius_dp));
    }

    /// Advances the theme crossfade and the markers' fades by `dt`.
    ///
    /// Whatever the step changed is written once -- the global's accent and base
    /// alpha, and the cluster's four marker colours -- and compared against the
    /// frame already written, so a fade that settled costs one final write and no
    /// wake-up after it.
    pub(super) fn step_fades(&mut self, dt: f32) -> bool {
        self.theme_fade.step(dt);
        let markers = self.state.markers.step(dt);
        let glide = self.theme_fade.value();
        let colors = self.state.markers.values();
        let frame = (glide.accent, glide.base_alpha, colors);
        if frame != self.drawn_fade {
            self.write_fade(glide, colors);
            self.drawn_fade = frame;
        }
        !self.theme_fade.is_settled() || !markers
    }

    /// Writes one fade frame: the global's accent and alpha, and the cluster's
    /// four marker colours. Followed by an explicit repaint, for the same reason
    /// every other write path here asks for one.
    fn write_fade(&self, glide: ThemeGlide, colors: [Rgba8; 4]) {
        let mut sink = WindowTheme {
            window: &self.window,
        };
        sink.set_accent(glide.accent);
        sink.set_base_alpha(f32::from(glide.base_alpha) / 255.0);
        self.window.set_mode_dot_color(color_of(colors[0]));
        self.window.set_full_width_color(color_of(colors[1]));
        self.window.set_punctuation_color(color_of(colors[2]));
        self.window.set_lock_color(color_of(colors[3]));
        self.request_repaint();
    }
}
