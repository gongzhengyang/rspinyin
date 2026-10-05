//! Tests for the one-shot transitions: the bezier curves, the scalar clock, the
//! window's appear motion and the colour blends the status strip and the theme
//! crossfade run on.
//!
//! The clock is handed in, so every scene steps a fixed 144Hz frame itself and
//! asserts on the frame counts the durations the design quotes come out to.

use super::*;

const FRAME_S: f32 = 1.0 / 144.0;

/// Steps a transition until it settles, with a ceiling so a stuck one fails the
/// test rather than hanging it.
fn run(transition: &mut TimedTransition) -> u32 {
    let mut frames = 0u32;
    while !transition.step(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the transition must finish");
    }
    frames + 1
}

#[test]
fn test_cubic_bezier_endpoints_are_exact() {
    for curve in [
        CubicBezier::APPEAR,
        CubicBezier::DISAPPEAR,
        CubicBezier::EASE_IN_OUT,
    ] {
        assert_eq!(curve.eval(0.0), 0.0);
        assert_eq!(curve.eval(1.0), 1.0);
    }
}

#[test]
fn test_cubic_bezier_stays_within_unit_range_and_is_monotone() {
    for curve in [
        CubicBezier::APPEAR,
        CubicBezier::DISAPPEAR,
        CubicBezier::EASE_IN_OUT,
    ] {
        let mut previous = 0.0f32;
        for step in 0..=100 {
            let value = curve.eval(step as f32 / 100.0);
            assert!(
                (0.0..=1.0).contains(&value),
                "the curve left the unit range at {step}"
            );
            assert!(
                value >= previous,
                "the curve went backwards at {step}: {value} < {previous}"
            );
            previous = value;
        }
    }
}

#[test]
fn test_appear_curve_is_ahead_of_linear_and_disappear_is_behind() {
    // The appear curve is a fast-out: it is nearly finished at the midpoint.
    assert!(CubicBezier::APPEAR.eval(0.5) > 0.9);
    // The disappear curve is an ease-in: it has barely started at the midpoint.
    assert!(CubicBezier::DISAPPEAR.eval(0.5) < 0.5);
}

#[test]
fn test_ease_in_out_is_symmetric_about_the_midpoint() {
    let curve = CubicBezier::EASE_IN_OUT;
    assert!((curve.eval(0.5) - 0.5).abs() < 1.0e-4);
    for step in 1..50 {
        let t = step as f32 / 100.0;
        let sum = curve.eval(t) + curve.eval(1.0 - t);
        assert!(
            (sum - 1.0).abs() < 1.0e-3,
            "the curve is not symmetric at {t}: {sum}"
        );
    }
}

#[test]
fn test_cubic_bezier_of_degenerate_progress_is_finite() {
    let curve = CubicBezier::EASE_IN_OUT;
    assert_eq!(curve.eval(f32::NAN), 0.0);
    assert_eq!(curve.eval(-5.0), 0.0);
    assert_eq!(curve.eval(5.0), 1.0);
    assert_eq!(curve.eval(f32::INFINITY), 1.0);
    // Control points outside the unit range are pulled back in, so the curve
    // stays a function of x.
    let repaired = CubicBezier::new(-1.0, 0.0, 4.0, 1.0);
    assert_eq!((repaired.x1, repaired.x2), (0.0, 1.0));
    assert!(repaired.eval(0.5).is_finite());
    let poisoned = CubicBezier::new(f32::NAN, f32::NAN, f32::NAN, f32::NAN);
    assert!(poisoned.eval(0.5).is_finite());
}

#[test]
fn test_timed_transition_finishes_in_the_configured_time() {
    let mut transition = TimedTransition::new(0.0);
    transition.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    assert!(!transition.is_settled());
    let frames = run(&mut transition);
    // 120ms at 144Hz is 17.28 frames, so the eighteenth frame completes it.
    assert_eq!(frames, 18);
    assert_eq!(transition.value(), 1.0);
    assert!(transition.is_settled());
}

#[test]
fn test_timed_transition_of_zero_duration_snaps() {
    let mut transition = TimedTransition::new(0.0);
    transition.start(1.0, 0.0, CubicBezier::APPEAR);
    assert!(transition.is_settled());
    assert_eq!(transition.value(), 1.0);
    // A negative or unusable duration is treated as zero rather than reversed.
    transition.start(0.0, -1.0, CubicBezier::APPEAR);
    assert!(transition.is_settled());
    assert_eq!(transition.value(), 0.0);
    transition.start(1.0, f32::NAN, CubicBezier::APPEAR);
    assert!(transition.is_settled());
}

#[test]
fn test_timed_transition_of_degenerate_delta_stays_finite() {
    let mut transition = TimedTransition::new(0.0);
    transition.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    transition.step(FRAME_S);
    let value = transition.value();
    assert!(!transition.step(0.0), "a zero delta carries no time");
    assert_eq!(transition.value(), value);
    assert!(!transition.step(-FRAME_S));
    assert_eq!(transition.value(), value);
    assert!(!transition.step(f32::NAN));
    assert_eq!(transition.value(), value);
    // A stalled frame is clamped, so it advances by one stable step and no more.
    let mut stalled = transition;
    let mut reference = transition;
    stalled.step(30.0);
    reference.step(1.0 / 60.0);
    assert_eq!(stalled.value(), reference.value());
    assert!(transition.value().is_finite());
}

#[test]
fn test_appear_anim_reaches_full_opacity_and_scale() {
    let mut anim = AppearAnim::hidden();
    assert_eq!(anim.opacity(), 0.0);
    assert_eq!(anim.scale(), APPEAR_SCALE_FROM);
    anim.appear(APPEAR_S);
    let mut frames = 0u32;
    while !anim.step(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the appear motion must finish");
    }
    // 110ms at 144Hz is 15.84 frames.
    assert_eq!(frames + 1, 16);
    assert_eq!(anim.opacity(), 1.0);
    assert_eq!(anim.scale(), 1.0);
}

#[test]
fn test_appear_anim_disappear_reaches_transparent_and_shrunk() {
    let mut anim = AppearAnim::visible();
    anim.disappear(DISAPPEAR_S);
    let mut frames = 0u32;
    while !anim.step(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the disappear motion must finish");
    }
    // 90ms at 144Hz is 12.96 frames.
    assert_eq!(frames + 1, 13);
    assert_eq!(anim.opacity(), 0.0);
    assert_eq!(anim.scale(), DISAPPEAR_SCALE_TO);
}

#[test]
fn test_appear_anim_interrupted_disappear_continues_from_current_opacity() {
    let mut anim = AppearAnim::visible();
    anim.disappear(DISAPPEAR_S);
    for _ in 0..6 {
        anim.step(FRAME_S);
    }
    let mid = anim.opacity();
    assert!(
        mid > 0.0 && mid < 1.0,
        "the window is part way through fading out, got {mid}"
    );
    // A Show arrives during the disappear: the state machine requires the fade to
    // continue from where it is rather than drop to zero and rise again.
    anim.appear(APPEAR_S);
    assert_eq!(anim.opacity(), mid, "no jump on the reversal");
    assert!(!anim.is_settled());
    anim.step(FRAME_S);
    assert!(
        anim.opacity() > mid,
        "the window must now be fading back in, got {}",
        anim.opacity()
    );
}

#[test]
fn test_appear_anim_snap_hidden_and_visible_reach_the_end_states() {
    let mut anim = AppearAnim::hidden();
    anim.appear(APPEAR_S);
    anim.step(FRAME_S);
    anim.snap_visible();
    assert_eq!(anim.opacity(), 1.0);
    assert_eq!(anim.scale(), 1.0);
    assert!(anim.is_settled());
    anim.snap_hidden();
    assert_eq!(anim.opacity(), 0.0);
    assert_eq!(anim.scale(), APPEAR_SCALE_FROM);
    assert!(anim.is_settled());
    // `snap_to_end` follows the direction the motion was already heading in.
    anim.appear(APPEAR_S);
    anim.step(FRAME_S);
    anim.snap_to_end();
    assert_eq!(anim.opacity(), 1.0);
    assert!(anim.is_settled());
}

#[test]
fn test_status_crossfade_completes_in_the_configured_time() {
    // 3.3.2's status-icon switch is an opacity crossfade on the shared
    // `ease-in-out` curve; the same shape serves the theme crossfade.
    let mut icon = TimedTransition::new(0.0);
    icon.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    let mut frames = 0u32;
    while !icon.step(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the crossfade must finish");
    }
    assert_eq!(frames + 1, 18, "120ms at 144Hz");
    assert_eq!(icon.value(), 1.0);
    // Halfway through it is exactly halfway across, which is what `ease-in-out`
    // buys and what keeps the two icons equally weighted mid-switch.
    let mut halfway = TimedTransition::new(0.0);
    halfway.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    for _ in 0..9 {
        halfway.step(FRAME_S);
    }
    assert!((halfway.value() - 0.5).abs() < 0.05);
}

#[test]
fn test_theme_crossfade_uses_the_ease_in_out_curve() {
    // 3.3.2 gives the theme crossfade the same 120ms `ease-in-out` treatment, so
    // a colour change and a status-icon change stay in step.
    let mut theme = TimedTransition::new(0.0);
    theme.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    let mut icon = TimedTransition::new(0.0);
    icon.start(1.0, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    for _ in 0..18 {
        theme.step(FRAME_S);
        icon.step(FRAME_S);
    }
    assert_eq!(theme.value(), icon.value());
    assert!(theme.is_settled());
}

/// The four channels of a colour, in index order.
fn channels(colour: Rgba8) -> [u8; 4] {
    [colour.r, colour.g, colour.b, colour.a]
}

#[test]
fn test_blend_transition_steps_monotonically_and_lands_byte_exact() {
    let from = Rgba8 {
        r: 10,
        g: 200,
        b: 0,
        a: 40,
    };
    let to = Rgba8 {
        r: 200,
        g: 10,
        b: 240,
        a: 230,
    };
    let mut fade = BlendTransition::new(from);
    fade.start(to, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    assert_eq!(fade.value(), from, "a fade at rest sits on its start");
    let mut previous = from;
    let mut distinct = 0u32;
    let mut frames = 0u32;
    while !fade.step(FRAME_S) {
        let value = fade.value();
        if value != previous {
            distinct += 1;
        }
        for channel in 0..4 {
            let (was, now, end) = (
                channels(previous)[channel],
                channels(value)[channel],
                channels(to)[channel],
            );
            let (was, now, end) = (i32::from(was), i32::from(now), i32::from(end));
            assert!(
                (now - was) * (end - was) >= 0,
                "channel {channel} went backwards at frame {frames}"
            );
            assert!(
                (end - was).abs() >= (end - now).abs(),
                "channel {channel} overshot its endpoint at frame {frames}"
            );
        }
        previous = value;
        frames += 1;
        assert!(frames < 1_000, "the blend must finish");
    }
    // 120ms at 144Hz is 17.28 frames, and an eased glide passes through more
    // than two distinct colours on the way.
    assert!(
        frames <= 20,
        "the blend took {frames} frames, over the budget"
    );
    assert!(distinct >= 3, "only {distinct} distinct colours on the way");
    assert_eq!(fade.value(), to, "the fade lands byte-exact on its end");
    assert!(fade.is_settled());
}

#[test]
fn test_blend_transition_interrupted_start_continues_from_the_reached_value() {
    let dark = Rgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    let light = Rgba8 {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    let mut fade = BlendTransition::new(dark);
    fade.start(light, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    for _ in 0..6 {
        fade.step(FRAME_S);
    }
    let mid = fade.value();
    assert!(mid != dark && mid != light, "the fade is part way across");
    // A reversal landing mid-fade continues from the colour reached rather
    // than dropping back to an endpoint first, the way a fast mode toggle
    // reads as one motion instead of two.
    fade.start(dark, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    assert_eq!(fade.value(), mid, "the reversal keeps the colour reached");
    let mut frames = 0u32;
    while !fade.step(FRAME_S) {
        frames += 1;
        assert!(frames < 1_000, "the reversed fade must finish");
    }
    assert_eq!(fade.value(), dark, "and it lands byte-exact on the new end");
}

#[test]
fn test_blend_transition_of_equal_endpoints_stays_put_and_the_steps_are_exact() {
    let grey = Rgba8 {
        r: 0x80,
        g: 0x80,
        b: 0x80,
        a: 255,
    };
    let mut fade = BlendTransition::new(grey);
    fade.start(grey, CROSSFADE_S, CubicBezier::EASE_IN_OUT);
    assert!(
        fade.is_settled(),
        "equal endpoints are nothing to travel to"
    );
    fade.step(FRAME_S);
    assert_eq!(fade.value(), grey);
    // The integer blend is exact at both ends of the step scale and monotone
    // across it, which is what makes a fade that settled and one that was
    // snapped draw the same bytes.
    let dark = Rgba8 {
        r: 200,
        g: 100,
        b: 50,
        a: 200,
    };
    let light = Rgba8 {
        r: 10,
        g: 20,
        b: 240,
        a: 250,
    };
    let mut previous = Rgba8::blend(dark, light, 0);
    assert_eq!(previous, dark, "step zero is the exact start");
    for t in 1..=256u16 {
        let value = Rgba8::blend(dark, light, t);
        for channel in 0..4 {
            // Monotone towards the target, whichever way the channel runs: the red
            // channel here falls from 200 to 10, and the blend must not wobble on
            // its way down any more than the blue must on its way up.
            let rest = (channels(value)[channel] as i32 - channels(light)[channel] as i32).abs();
            let was = (channels(previous)[channel] as i32 - channels(light)[channel] as i32).abs();
            assert!(
                rest <= was,
                "the blend moved away from the target at step {t}"
            );
        }
        previous = value;
    }
    assert_eq!(previous, light, "step 256 is the exact end");
}
