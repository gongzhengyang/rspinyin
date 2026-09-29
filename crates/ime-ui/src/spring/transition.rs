//! The one-shot transitions: the window appearing and disappearing, the status-icon
//! crossfade and the theme crossfade.
//!
//! These are deliberately *not* springs. A one-shot transition has no redirection to
//! absorb -- the user cannot press "appear" again halfway through -- so a
//! cubic-bezier is more predictable and, more to the point, cannot overshoot. An
//! overshooting window would visibly bounce past the screen edge it was placed
//! against, which is the one place a spring is the wrong tool.
//!
//! Interruption is still supported, and is why these are objects rather than a pure
//! function of elapsed time: starting a transition takes its `from` value from
//! wherever the previous one had got to. A `Show` that lands during a disappear
//! therefore continues from the current opacity instead of dropping to zero first,
//! which is the behaviour the window state machine requires.

use super::clamp_step;

/// The appear duration of 3.3.2, in seconds.
pub const APPEAR_S: f32 = 0.110;

/// The disappear duration of 3.3.2, in seconds.
pub const DISAPPEAR_S: f32 = 0.090;

/// The status-icon and theme crossfade duration of 3.3.2, in seconds.
pub const CROSSFADE_S: f32 = 0.120;

/// The scale the window grows from when it appears (3.3.2: `0.96 -> 1.0`).
pub const APPEAR_SCALE_FROM: f32 = 0.96;

/// The scale the window shrinks to when it disappears (3.3.2: `1.0 -> 0.98`).
pub const DISAPPEAR_SCALE_TO: f32 = 0.98;

/// Iterations of the bisection in [`CubicBezier::eval`].
///
/// Twenty-four halvings put the solution inside `6e-8` of the true parameter, which
/// is five orders of magnitude below a pixel.
const SOLVER_ITERATIONS: u32 = 24;

/// A `cubic-bezier(x1, y1, x2, y2)` timing function.
///
/// Evaluating the curve at progress `t` means solving `x(s) = t` for `s` and reading
/// `y(s)`, which is what the CSS definition means by "the value of the curve at `t`";
/// interpolating the parameter directly would give the wrong shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// First control point's x. Must lie in `0.0..=1.0` for the curve to be a
    /// function of `x`.
    pub x1: f32,
    /// First control point's y.
    pub y1: f32,
    /// Second control point's x. Must lie in `0.0..=1.0`.
    pub x2: f32,
    /// Second control point's y.
    pub y2: f32,
}

impl CubicBezier {
    /// `cubic-bezier(0.22, 1.0, 0.36, 1.0)`: the appear curve of 3.3.2.
    pub const APPEAR: Self = Self {
        x1: 0.22,
        y1: 1.0,
        x2: 0.36,
        y2: 1.0,
    };

    /// `cubic-bezier(0.4, 0.0, 1.0, 1.0)`: the disappear curve of 3.3.2.
    pub const DISAPPEAR: Self = Self {
        x1: 0.4,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
    };

    /// `ease-in-out` = `cubic-bezier(0.42, 0.0, 0.58, 1.0)`. 3.3.2 uses it for the
    /// status-icon crossfade and the theme crossfade.
    pub const EASE_IN_OUT: Self = Self {
        x1: 0.42,
        y1: 0.0,
        x2: 0.58,
        y2: 1.0,
    };

    /// Builds a curve, pulling the two x control points into `0.0..=1.0`.
    ///
    /// The y values pass through unchanged: they are what gives a curve its
    /// character, and every curve this module defines keeps them inside the unit
    /// range anyway.
    pub fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x1: unit(x1),
            y1: finite_or_zero(y1),
            x2: unit(x2),
            y2: finite_or_zero(y2),
        }
    }

    /// The progress the curve reports at `t`.
    ///
    /// `t` outside `0.0..=1.0`, or not a number at all, is clamped rather than
    /// extrapolated: the curve's endpoints are the only values the callers can act
    /// on, and extrapolating a cubic past them produces an opacity outside
    /// `0.0..=1.0`.
    pub fn eval(&self, t: f32) -> f32 {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else if t > 0.0 {
            1.0
        } else {
            0.0
        };
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        sample(self.y1, self.y2, solve_for_t(self.x1, self.x2, t))
    }
}

/// A transition between two values, driven by the caller's clock.
///
/// The value is always a normalized quantity -- an opacity, or a scale factor close
/// to one -- which is why it is clamped into `0.0..=1.0` on the way in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedTransition {
    from: f32,
    to: f32,
    value: f32,
    elapsed_s: f32,
    duration_s: f32,
    easing: CubicBezier,
    settled: bool,
}

impl TimedTransition {
    /// Creates a transition at rest on `initial`.
    pub fn new(initial: f32) -> Self {
        let initial = unit(initial);
        Self {
            from: initial,
            to: initial,
            value: initial,
            elapsed_s: 0.0,
            duration_s: 0.0,
            easing: CubicBezier::EASE_IN_OUT,
            settled: true,
        }
    }

    /// Starts a transition to `to`.
    ///
    /// The starting value is the current one, not the previous transition's origin:
    /// that is what makes an interrupted fade continue from where it is rather than
    /// jump. A duration of zero or less completes immediately, which is the path
    /// `[ui.animation] enabled = false` and the screenshot tests take.
    pub fn start(&mut self, to: f32, duration_s: f32, easing: CubicBezier) {
        let to = unit(to);
        self.from = self.value;
        self.to = to;
        self.elapsed_s = 0.0;
        self.duration_s = if duration_s.is_finite() && duration_s > 0.0 {
            duration_s
        } else {
            0.0
        };
        self.easing = easing;
        self.settled = self.duration_s <= 0.0 || self.from == to;
        if self.settled {
            self.value = to;
        }
    }

    /// Advances by `dt` and reports whether the transition has finished.
    ///
    /// The delta is clamped exactly as the spring's is, so a stalled frame stretches
    /// the fade instead of skipping it.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.settled {
            return true;
        }
        self.elapsed_s += clamp_step(dt);
        if self.elapsed_s >= self.duration_s {
            self.elapsed_s = self.duration_s;
            self.value = self.to;
            self.settled = true;
            return true;
        }
        let progress = self.elapsed_s / self.duration_s;
        self.value = self.from + (self.to - self.from) * self.easing.eval(progress);
        false
    }

    /// Jumps to `value` and stops.
    pub fn snap_to(&mut self, value: f32) {
        let value = unit(value);
        self.from = value;
        self.to = value;
        self.value = value;
        self.elapsed_s = 0.0;
        self.duration_s = 0.0;
        self.settled = true;
    }

    /// Jumps to the value this transition was heading for.
    pub fn snap_to_end(&mut self) {
        let to = self.to;
        self.snap_to(to);
    }

    /// The current value.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// The value this transition is heading for.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the transition has finished.
    pub fn is_settled(&self) -> bool {
        self.settled
    }
}

/// The appear and disappear motion: opacity, plus the scale the window grows from.
///
/// Both properties run on the same clock and the same curve, so they are driven
/// together here rather than kept as two independent transitions by every caller.
#[derive(Clone, Copy, Debug)]
pub struct AppearAnim {
    opacity: TimedTransition,
    scale: TimedTransition,
}

impl AppearAnim {
    /// A window that is not on screen: fully transparent, at its smallest.
    pub fn hidden() -> Self {
        Self {
            opacity: TimedTransition::new(0.0),
            scale: TimedTransition::new(APPEAR_SCALE_FROM),
        }
    }

    /// A window that is fully on screen.
    pub fn visible() -> Self {
        Self {
            opacity: TimedTransition::new(1.0),
            scale: TimedTransition::new(1.0),
        }
    }

    /// Starts the appear motion, continuing from the current opacity and scale.
    pub fn appear(&mut self, duration_s: f32) {
        self.opacity.start(1.0, duration_s, CubicBezier::APPEAR);
        self.scale.start(1.0, duration_s, CubicBezier::APPEAR);
    }

    /// Starts the disappear motion, continuing from the current opacity and scale.
    pub fn disappear(&mut self, duration_s: f32) {
        self.opacity.start(0.0, duration_s, CubicBezier::DISAPPEAR);
        self.scale
            .start(DISAPPEAR_SCALE_TO, duration_s, CubicBezier::DISAPPEAR);
    }

    /// Advances both properties by `dt` and reports whether both have finished.
    pub fn step(&mut self, dt: f32) -> bool {
        let opacity_done = self.opacity.step(dt);
        let scale_done = self.scale.step(dt);
        opacity_done && scale_done
    }

    /// The window opacity for this frame, in `0.0..=1.0`.
    pub fn opacity(&self) -> f32 {
        self.opacity.value()
    }

    /// The window scale for this frame.
    pub fn scale(&self) -> f32 {
        self.scale.value()
    }

    /// Whether both properties have finished.
    pub fn is_settled(&self) -> bool {
        self.opacity.is_settled() && self.scale.is_settled()
    }

    /// Jumps to a fully visible window.
    pub fn snap_visible(&mut self) {
        self.opacity.snap_to(1.0);
        self.scale.snap_to(1.0);
    }

    /// Jumps to a fully hidden window.
    pub fn snap_hidden(&mut self) {
        self.opacity.snap_to(0.0);
        self.scale.snap_to(APPEAR_SCALE_FROM);
    }

    /// Jumps both properties to the values they were heading for.
    pub fn snap_to_end(&mut self) {
        self.opacity.snap_to_end();
        self.scale.snap_to_end();
    }
}

/// Evaluates one axis of the cubic with control points `(0, a1, a2, 1)`.
fn sample(a1: f32, a2: f32, s: f32) -> f32 {
    let c = 3.0 * a1;
    let b = 3.0 * (a2 - a1) - c;
    let a = 1.0 - c - b;
    ((a * s + b) * s + c) * s
}

/// Solves `sample(x1, x2, s) == x` for `s`.
///
/// Bisection rather than Newton: the x-curve's slope goes to zero at the ends of the
/// useful range, which is exactly where Newton stalls or shoots outside `0.0..=1.0`,
/// while bisection converges in a fixed number of steps for any curve whose x
/// control points lie in range. The cost is a handful of multiplies on a path that
/// runs a few times per frame.
fn solve_for_t(x1: f32, x2: f32, x: f32) -> f32 {
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    for _ in 0..SOLVER_ITERATIONS {
        let mid = 0.5 * (low + high);
        if sample(x1, x2, mid) < x {
            low = mid;
        } else {
            high = mid;
        }
    }
    0.5 * (low + high)
}

/// Clamps a normalized value, replacing anything unusable with zero.
fn unit(value: f32) -> f32 {
    finite_or_zero(value).clamp(0.0, 1.0)
}

/// Replaces a number the arithmetic cannot carry with zero.
fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
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
}
