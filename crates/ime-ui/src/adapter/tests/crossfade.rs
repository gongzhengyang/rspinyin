//! The two crossfades of 3.3.2: the status markers' colour fades and the theme's
//! accent-and-alpha fade.
//!
//! Every scene drives a real component on a fresh thread and reads the colours back
//! through the properties the component holds, so the assertions cover the write
//! path and not only the arithmetic. The byte-exact endings are the contract the
//! colour gate cares about: a fade is a change in time only, and a settled one draws
//! exactly the tokens.

use ime_types::{ColorScheme, Rgba8, ThemeSpec, UiFrame};
use slint::{Color, ComponentHandle as _};

use crate::spring::transition::TRANSPARENT;
use crate::theme::{BlurNegotiation, ThemeResolution, ThemeTokens};
use crate::ui_generated::Theme;

use super::motion::FRAME_S;
use super::{Adapter, frame_with, with_adapter};

/// The accent the light-theme scenes use.
const LIGHT_ACCENT: Rgba8 = Rgba8 {
    r: 0x0A,
    g: 0x6C,
    b: 0xFF,
    a: 255,
};

/// The accent the dark-theme scenes use, far enough from the light one that a
/// glide and a snap are tellable apart.
const DARK_ACCENT: Rgba8 = Rgba8 {
    r: 0x4C,
    g: 0x9A,
    b: 0xFF,
    a: 255,
};

/// A resolved theme of `scheme`, exactly as the surface resolves one: the tokens
/// are what the contrast gate approved, which is what the adapter is handed.
fn resolved(scheme: ColorScheme, accent: Rgba8) -> (ThemeSpec, ThemeTokens) {
    let spec = ThemeSpec {
        scheme,
        accent,
        acrylic: false,
        base_alpha: 217,
        corner_radius_dp: 14,
        scale: 1.0,
    };
    let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Disabled);
    (spec, resolution.tokens)
}

/// Applies a theme through the adapter's own path and returns its tokens.
fn theme(adapter: &mut Adapter, scheme: ColorScheme, accent: Rgba8) -> ThemeTokens {
    let (spec, tokens) = resolved(scheme, accent);
    adapter.apply_theme(&spec, &tokens);
    tokens
}

/// A token colour as the component holds it, for byte-exact comparisons.
fn token(colour: Rgba8) -> Color {
    Color::from_argb_u8(colour.a, colour.r, colour.g, colour.b)
}

/// A component colour as the four bytes it paints with.
fn bytes(colour: Color) -> [u8; 4] {
    [colour.red(), colour.green(), colour.blue(), colour.alpha()]
}

/// A frame whose status strip reports `full_width` and nothing else.
fn frame_with_full_width(revision: u32, full_width: bool) -> UiFrame {
    let mut frame = frame_with(revision, "ni", 1);
    frame.status.full_width = full_width;
    frame
}

#[test]
fn test_adapter_full_width_fade_interpolates_monotonically_and_lands_byte_exact() {
    with_adapter(|adapter| {
        let tokens = theme(adapter, ColorScheme::Light, LIGHT_ACCENT);
        assert!(adapter.apply_frame(&frame_with(1, "ni", 1)));
        assert!(
            !adapter.advance(FRAME_S).expect("the motion advances"),
            "a settled window reports no deadline"
        );
        let idle = adapter.window().get_full_width_color();
        assert_eq!(
            idle,
            token(tokens.status_dot_idle),
            "the marker rests at the idle token before anything toggles"
        );

        // Toggling full-width starts the fade: its first frame is the colour it
        // starts from, every frame moves towards the active token without
        // passing it, and the last frame is that token byte for byte.
        assert!(adapter.apply_frame(&frame_with_full_width(2, true)));
        adapter.advance(0.0).expect("the motion advances");
        let start = adapter.window().get_full_width_color();
        assert_eq!(start, idle, "no jump on the first frame of the fade");

        let end = bytes(token(tokens.status_dot_active));
        let mut previous = bytes(start);
        let mut distinct = 0u32;
        let mut frames = 0u32;
        while adapter.advance(FRAME_S).expect("the motion advances") {
            let now = bytes(adapter.window().get_full_width_color());
            for channel in 0..4 {
                let (was, now, to) = (
                    i32::from(previous[channel]),
                    i32::from(now[channel]),
                    i32::from(end[channel]),
                );
                assert!(
                    (now - was) * (to - was) >= 0,
                    "channel {channel} moved backwards at frame {frames}"
                );
                assert!(
                    (to - was).abs() >= (to - now).abs(),
                    "channel {channel} overshot its endpoint at frame {frames}"
                );
            }
            if now != previous {
                distinct += 1;
            }
            previous = now;
            frames += 1;
            assert!(frames < 1_000, "the fade must settle");
        }
        // 120ms at 144Hz is 17.28 frames, and an eased glide passes through more
        // than two distinct colours on the way.
        assert!(
            frames <= 20,
            "the fade took {frames} frames, over the budget"
        );
        assert!(distinct >= 3, "only {distinct} distinct colours on the way");
        assert_eq!(
            adapter.window().get_full_width_color(),
            token(tokens.status_dot_active),
            "the fade lands byte-exact on the active token"
        );
        assert!(
            !adapter.advance(FRAME_S).expect("the motion advances")
                && !adapter.advance(FRAME_S).expect("the motion advances"),
            "a settled fade reports no deadline, which is what lets the loop block"
        );
    });
}

#[test]
fn test_adapter_theme_switch_fades_stay_between_the_endpoints_and_land_byte_exact() {
    with_adapter(|adapter| {
        let light = theme(adapter, ColorScheme::Light, LIGHT_ACCENT);
        assert!(adapter.apply_frame(&frame_with(1, "ni", 1)));
        adapter.advance(0.0).expect("the motion advances");

        // A light-to-dark switch: the scheme flag lands at once and everything
        // that blends glides, never leaving the hull the two palettes span --
        // which is what "no flash frame" means colour by colour.
        let dark = theme(adapter, ColorScheme::Dark, DARK_ACCENT);
        let mode_from = bytes(token(light.status_dot_idle));
        let mode_to = bytes(token(dark.status_dot_idle));
        let accent_from = bytes(token(light.accent));
        let accent_to = bytes(token(dark.accent));
        let mut moved_mid_fade = false;
        let mut frames = 0u32;
        loop {
            let in_flight = adapter.advance(FRAME_S).expect("the motion advances");
            let mode = bytes(adapter.window().get_mode_dot_color());
            let accent = bytes(adapter.window().global::<Theme>().get_accent());
            for channel in 0..4 {
                let mode_hull = (
                    mode_from[channel].min(mode_to[channel]),
                    mode_from[channel].max(mode_to[channel]),
                );
                assert!(
                    (mode_hull.0..=mode_hull.1).contains(&mode[channel]),
                    "the mode dot left the two palettes' hull at frame {frames}"
                );
                let accent_hull = (
                    accent_from[channel].min(accent_to[channel]),
                    accent_from[channel].max(accent_to[channel]),
                );
                assert!(
                    (accent_hull.0..=accent_hull.1).contains(&accent[channel]),
                    "the global's accent left the two accents' hull at frame {frames}"
                );
            }
            if mode != mode_from && mode != mode_to {
                moved_mid_fade = true;
            }
            frames += 1;
            assert!(frames < 1_000, "the theme fade must settle");
            if !in_flight {
                break;
            }
        }
        assert!(
            moved_mid_fade,
            "the switch genuinely glided instead of snapping between the palettes"
        );
        assert!(
            frames <= 20,
            "the theme fade took {frames} frames, over its 120ms budget"
        );
        assert_eq!(
            adapter.window().get_mode_dot_color(),
            token(dark.status_dot_idle),
            "the mode dot lands byte-exact on the new palette"
        );
        assert_eq!(
            adapter.window().global::<Theme>().get_accent(),
            token(dark.accent),
            "and so does the global's accent"
        );
        assert!(
            !adapter.advance(FRAME_S).expect("the motion advances"),
            "a settled theme fade reports no deadline"
        );
    });
}

#[test]
fn test_adapter_rapid_flag_flips_continue_from_the_reached_colour() {
    with_adapter(|adapter| {
        let tokens = theme(adapter, ColorScheme::Light, LIGHT_ACCENT);
        assert!(adapter.apply_frame(&frame_with(1, "ni", 1)));
        adapter.advance(0.0).expect("the motion advances");

        // Full-width flips on and the fade is caught part way across.
        assert!(adapter.apply_frame(&frame_with_full_width(2, true)));
        for _ in 0..7 {
            adapter.advance(FRAME_S).expect("the motion advances");
        }
        let mid = adapter.window().get_full_width_color();
        assert!(
            mid != token(tokens.status_dot_idle) && mid != token(tokens.status_dot_active),
            "the fade is part way across after seven frames"
        );

        // The toggle flips back before the fade ran out: it continues from the
        // colour reached rather than restarting from an endpoint, which is what
        // makes a fast toggle read as one motion instead of two.
        assert!(adapter.apply_frame(&frame_with_full_width(3, false)));
        adapter.advance(0.0).expect("the motion advances");
        assert_eq!(
            adapter.window().get_full_width_color(),
            mid,
            "the reversal keeps the colour reached"
        );
        while adapter.advance(FRAME_S).expect("the motion advances") {
            // The ceiling guards the loop; the landing is asserted below.
        }
        assert_eq!(
            adapter.window().get_full_width_color(),
            token(tokens.status_dot_idle),
            "and the fade lands byte-exact back on the idle token"
        );
        assert!(
            !adapter.advance(FRAME_S).expect("the motion advances"),
            "and the window is at rest afterwards"
        );
    });
}

#[test]
fn test_adapter_theme_switch_with_motion_disabled_lands_at_once() {
    with_adapter(|adapter| {
        theme(adapter, ColorScheme::Light, LIGHT_ACCENT);
        assert!(adapter.apply_frame(&frame_with(1, "ni", 1)));
        adapter.advance(0.0).expect("the motion advances");
        adapter.set_motion_enabled(false);

        // With the switch off the second theme is a direct write: the global's
        // accent lands inside the apply call itself, with no fade to wait for.
        let dark = theme(adapter, ColorScheme::Dark, DARK_ACCENT);
        assert_eq!(
            adapter.window().global::<Theme>().get_accent(),
            token(dark.accent),
            "the accent lands in the apply call, with no fade in between"
        );
        // The markers land on the first advance, which is also the last: nothing
        // is left in flight to schedule a frame for, so the snapshot is exactly
        // the end state no matter how many frames came before it.
        assert!(
            !adapter.advance(0.0).expect("the motion advances"),
            "a disabled motion has nothing in flight"
        );
        assert_eq!(
            adapter.window().get_mode_dot_color(),
            token(dark.status_dot_idle),
            "the mode dot lands byte-exact on the new palette"
        );
        assert_eq!(
            adapter.window().get_full_width_color(),
            token(dark.status_dot_idle),
            "and so does the full-width marker"
        );
        assert!(!adapter.advance(FRAME_S).expect("the motion advances"));

        // And a flag that flips while the switch is off lands on its first
        // advance too, which is what keeps the disabled path snapshot-exact.
        assert!(adapter.apply_frame(&frame_with_full_width(3, true)));
        assert!(
            !adapter.advance(0.0).expect("the motion advances"),
            "a flag that flips with the motion off has nothing in flight"
        );
        assert_eq!(
            adapter.window().get_full_width_color(),
            token(dark.status_dot_active),
            "the marker lands on its new colour in one step"
        );
    });
}

#[test]
fn test_adapter_first_theme_lands_on_the_tokens_without_a_fade() {
    with_adapter(|adapter| {
        // There is no previous palette to glide away from, so the first theme
        // snaps: the first advance writes the tokens and reports nothing left to
        // schedule a frame for.
        let tokens = theme(adapter, ColorScheme::Dark, DARK_ACCENT);
        assert!(adapter.apply_frame(&frame_with(1, "ni", 1)));
        assert!(!adapter.advance(0.0).expect("the motion advances"));
        let window = adapter.window();
        assert_eq!(
            window.get_mode_dot_color(),
            token(tokens.status_dot_idle),
            "the mode dot is the idle token, not a fade's first step"
        );
        assert_eq!(
            window.get_full_width_color(),
            token(tokens.status_dot_idle),
            "the full-width marker is the idle token"
        );
        assert_eq!(
            window.get_punctuation_color(),
            token(tokens.status_dot_idle),
            "the punctuation marker is the idle token"
        );
        assert_eq!(
            window.get_lock_color(),
            token(TRANSPARENT),
            "the lock draws nothing while the store is writable"
        );
        assert_eq!(
            window.global::<Theme>().get_accent(),
            token(tokens.accent),
            "the global's accent is the theme's own"
        );
        assert!(!adapter.advance(FRAME_S).expect("the motion advances"));
    });
}
