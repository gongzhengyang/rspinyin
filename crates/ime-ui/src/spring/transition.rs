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

use ime_types::Rgba8;

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

/// Fully transparent black: the resting colour of a marker nothing has themed yet.
pub const TRANSPARENT: Rgba8 = Rgba8 {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// One 8-bit channel of a deterministic blend, `t` in `0..=256`.
///
/// The arithmetic is integer end to end: `from` plus the channel delta carried `t`
/// 256ths of the way, with the `+ 128` before the shift as the round-half step.
/// Integer division floors, which keeps the result monotone in `t` and inside the
/// two endpoints either way, and a given pair of endpoints and a given `t` always
/// produce the same byte.
const fn blend_channel(from: u8, to: u8, t: u16) -> u8 {
    let delta = to as i32 - from as i32;
    let carried = (delta * (t as i32) + 128) >> 8;
    let value = from as i32 + carried;
    // `clamp` is not const-callable yet, so the bounds are the two comparisons it
    // would have made.
    if value < 0 {
        0
    } else if value > 255 {
        255
    } else {
        value as u8
    }
}

/// A value that can be carried from one endpoint to another in 256 deterministic
/// steps.
///
/// The step count is what keeps a blend exact at both ends and reproducible frame
/// for frame: the value is a function of the endpoints and an integer alone,
/// never of the rounding a floating-point pipeline would apply on the way.
pub trait Blend: Copy {
    /// The value `t` 256ths of the way from `from` to `to`. `t` of `0` and `256`
    /// return `from` and `to` unchanged.
    fn blend(from: Self, to: Self, t: u16) -> Self;
}

impl Blend for u8 {
    fn blend(from: Self, to: Self, t: u16) -> Self {
        blend_channel(from, to, t)
    }
}

impl Blend for Rgba8 {
    fn blend(from: Self, to: Self, t: u16) -> Self {
        Rgba8 {
            r: blend_channel(from.r, to.r, t),
            g: blend_channel(from.g, to.g, t),
            b: blend_channel(from.b, to.b, t),
            a: blend_channel(from.a, to.a, t),
        }
    }
}

/// A one-shot transition between two blendable values.
///
/// [`TimedTransition`] underneath carries the eased progress from `0.0` to `1.0`
/// and the endpoints are blended in integer arithmetic, so the value a frame
/// paints is a function of the two endpoints and the elapsed time alone. Starting
/// a new transition takes its `from` from wherever the previous one had got to,
/// which is what makes an interrupted fade continue instead of jump -- the same
/// contract the scalar transitions and the window's springs follow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendTransition<C: Blend> {
    /// The eased progress from `0.0` to `1.0`, on the shared one-shot clock.
    clock: TimedTransition,
    /// The value the transition continues from.
    from: C,
    /// The value the transition is heading for.
    to: C,
}

impl<C: Blend + PartialEq> BlendTransition<C> {
    /// Creates a transition at rest on `initial`.
    pub fn new(initial: C) -> Self {
        Self {
            clock: TimedTransition::new(0.0),
            from: initial,
            to: initial,
        }
    }

    /// Starts a transition to `to`, continuing from the current value.
    ///
    /// A duration of zero or less lands on `to` at once, which is the path
    /// `[ui.animation] enabled = false` and a first theme application take.
    pub fn start(&mut self, to: C, duration_s: f32, easing: CubicBezier) {
        if to == self.value() {
            // Equal endpoints are nothing to travel to: settle the clock at once
            // rather than spend the duration redrawing the same bytes every frame.
            self.from = to;
            self.to = to;
            self.clock.start(1.0, 0.0, easing);
            return;
        }
        self.from = self.value();
        self.to = to;
        self.clock.snap_to(0.0);
        self.clock.start(1.0, duration_s, easing);
    }

    /// Advances by `dt` and reports whether the transition has finished.
    pub fn step(&mut self, dt: f32) -> bool {
        self.clock.step(dt)
    }

    /// The current value: the endpoints carried the eased progress of the way.
    pub fn value(&self) -> C {
        // `TimedTransition` keeps its value inside `0.0..=1.0`, so the quantized
        // step is inside `0..=256` and the blend is exact at both ends.
        let t = (self.clock.value() * 256.0).round() as u16;
        C::blend(self.from, self.to, t)
    }

    /// Whether the transition has finished.
    pub fn is_settled(&self) -> bool {
        self.clock.is_settled()
    }

    /// The value this transition is heading for.
    pub fn target(&self) -> C {
        self.to
    }

    /// Jumps to `value` and stops.
    pub fn snap_to(&mut self, value: C) {
        self.from = value;
        self.to = value;
        self.clock.snap_to(1.0);
    }
}

impl Default for BlendTransition<Rgba8> {
    fn default() -> Self {
        Self::new(TRANSPARENT)
    }
}

/// The four markers of the header's status cluster, in the order it draws them.
///
/// Each marker sits between two colours of 3.2: the mode dot between
/// `status.dot.active` and `status.dot.idle`, the two secondary markers between
/// the active token and the idle one, and the lock between `text.annotation` and
/// nothing at all. Which side of the pair is drawn belongs to the frame's flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMarker {
    /// The mode dot, driven by the strip's Chinese bit.
    Mode,
    /// The full-width marker, driven by the full-width bit.
    FullWidth,
    /// The Chinese-punctuation marker, driven by the punctuation bit.
    Punctuation,
    /// The read-only lock, driven by the read-only bit.
    Lock,
}

impl StatusMarker {
    /// The markers in the order the cluster draws them.
    pub const ALL: [Self; 4] = [Self::Mode, Self::FullWidth, Self::Punctuation, Self::Lock];
}

/// The colours each marker sits between, of the theme in force (3.2).
///
/// Copied out of a resolved [`crate::theme::ThemeTokens`] when one is applied, so
/// a theme switch re-targets the fades without the frame path knowing the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusTokens {
    /// The colour a marker draws while its state is on.
    pub active: Rgba8,
    /// The colour a marker rests at while its state is off.
    pub idle: Rgba8,
    /// The read-only lock's colour.
    pub lock: Rgba8,
}

impl StatusTokens {
    /// The status tokens of a resolved theme.
    pub fn of(tokens: &crate::theme::ThemeTokens) -> Self {
        Self {
            active: tokens.status_dot_active,
            idle: tokens.status_dot_idle,
            lock: tokens.text_annotation,
        }
    }
}

impl Default for StatusTokens {
    fn default() -> Self {
        Self {
            active: TRANSPARENT,
            idle: TRANSPARENT,
            lock: TRANSPARENT,
        }
    }
}

/// The status strip's crossfade: one colour transition per marker.
///
/// One transition per marker rather than one shared clock, because the markers
/// flip independently and an interrupted fade continues from the colour it had
/// reached, which is a per-marker property. The colours blend in integer steps,
/// so a fade that settled and one that was snapped draw the same bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerFades([BlendTransition<Rgba8>; 4]);

impl Default for MarkerFades {
    fn default() -> Self {
        Self([BlendTransition::new(TRANSPARENT); 4])
    }
}

impl MarkerFades {
    /// Starts, or with `fade` off lands, the marker's fade toward `to`.
    ///
    /// The landing form is the `[ui.animation] enabled = false` path and a first
    /// theme application: both need a frame that does not depend on how many
    /// frames came before it.
    pub(crate) fn set(&mut self, marker: StatusMarker, to: Rgba8, fade: bool) {
        let slot = &mut self.0[marker as usize];
        let duration_s = if fade { CROSSFADE_S } else { 0.0 };
        slot.start(to, duration_s, CubicBezier::EASE_IN_OUT);
    }

    /// Advances every marker by `dt` and reports whether all have finished.
    ///
    /// A plain loop rather than `all`: every marker has to take the step, so the
    /// short-circuit `all` would offer is the wrong semantics.
    pub(crate) fn step(&mut self, dt: f32) -> bool {
        let mut settled = true;
        for marker in &mut self.0 {
            settled &= marker.step(dt);
        }
        settled
    }

    /// The four colours this frame paints, in [`StatusMarker`] order.
    pub(crate) fn values(&self) -> [Rgba8; 4] {
        let [mode, full_width, punctuation, lock] = &self.0;
        [
            mode.value(),
            full_width.value(),
            punctuation.value(),
            lock.value(),
        ]
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
mod tests;
