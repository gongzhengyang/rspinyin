//! The spring integrator: [`Spring1D`], the semi-implicit Euler stepping of 3.3.1,
//! with the clamps that keep degenerate deltas and coordinates finite.

use ime_types::PageDir;

use super::params::SpringParams;
use super::{clamp_coord, clamp_step};

/// The largest time step the integrator will accept, in seconds.
///
/// A late frame -- a stalled compositor, a debugger break, a suspended laptop -- must
/// slow an animation down rather than inject a large step into an explicit
/// integrator, which is exactly where such an integrator goes unstable. Clamping is
/// the whole stability argument, and it is why a dropped frame degrades to "slower"
/// instead of "diverged".
pub const MAX_STEP_S: f32 = 1.0 / 60.0;

/// Position error below which a spring counts as at rest, in dp.
pub const REST_POSITION_DP: f32 = 0.5;

/// Speed below which a spring counts as at rest, in dp/s.
pub const REST_VELOCITY_DP_PER_S: f32 = 20.0;

/// How far from its resting place the page content starts, in dp.
///
/// A page turn slides the content in from the side it came from; the sign is applied
/// by [`Spring1D::for_page_slide`].
pub const PAGE_SLIDE_DP: f32 = 12.0;

/// A one-dimensional spring integrated with semi-implicit Euler.
///
/// The integrator is the one 3.3.1 fixes: acceleration from Hooke's law plus linear
/// damping, velocity updated first and position second. That ordering is what makes
/// the scheme stable at the step sizes a frame clock produces; updating the position
/// first is not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring1D {
    /// Current position, in dp.
    pub x: f32,
    /// Current velocity, in dp/s.
    pub v: f32,
    target: f32,
    k: f32,
    c: f32,
    m: f32,
}

impl Spring1D {
    /// Creates a spring at rest on `x0` and targeting `x0`.
    ///
    /// `k` and `c` are derived from the natural frequency and the damping ratio as
    /// `k = m * omega0^2` and `c = 2 * zeta * omega0 * m`; an unusable frequency,
    /// damping ratio or mass is replaced, per [`SpringParams::new`]. An unusable `x0`
    /// becomes zero.
    pub fn new(omega0: f32, zeta: f32, mass: f32, x0: f32) -> Self {
        let params = SpringParams::new(omega0, zeta, mass);
        let x0 = clamp_coord(x0);
        Self {
            x: x0,
            v: 0.0,
            target: x0,
            k: params.mass * params.omega0 * params.omega0,
            c: 2.0 * params.zeta * params.omega0 * params.mass,
            m: params.mass,
        }
    }

    /// Redirects the spring to `target`, keeping the current velocity.
    ///
    /// This is the behaviour a bezier cannot reproduce, and the reason the highlight
    /// uses a spring at all: when the user presses an arrow key again while the box
    /// is still flying, the box bends toward the new cell instead of restarting from
    /// a standstill. A non-finite target is refused rather than allowed to poison the
    /// integrator.
    pub fn retarget(&mut self, target: f32) {
        if !target.is_finite() {
            return;
        }
        self.target = clamp_coord(target);
    }

    /// Places the spring at `x` with no velocity and makes it target `target`.
    ///
    /// One-shot motions such as the page-content slide start displaced and travel
    /// home, which is not a redirect: the velocity from the previous motion would be
    /// meaningless. A non-finite target leaves the existing one in place.
    pub fn restart(&mut self, x: f32, target: f32) {
        self.x = clamp_coord(x);
        self.v = 0.0;
        if target.is_finite() {
            self.target = clamp_coord(target);
        }
    }

    /// Jumps to the target and stops.
    ///
    /// Used when `[ui.animation] enabled = false`, where the motion must be
    /// instantaneous, and by the tests that need a deterministic frame.
    pub fn snap(&mut self) {
        self.x = self.target;
        self.v = 0.0;
    }

    /// The position this spring is travelling toward, in dp.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Whether the spring is close enough to its target to stop integrating.
    ///
    /// The answer is computed from the state rather than latched, so it cannot drift
    /// out of step with the position it describes.
    pub fn is_settled(&self) -> bool {
        self.at_rest()
    }

    /// Advances the spring by `dt` and reports whether it is at rest.
    ///
    /// `dt` is clamped into `0.0..=`[`MAX_STEP_S`] first, so a late frame slows the
    /// motion down, a zero or negative delta carries no time at all, and a
    /// non-finite delta is treated as zero. Once the rest condition holds, the
    /// remaining sub-threshold error is snapped away, so a spring that settled and a
    /// spring that was snapped produce the same value.
    pub fn step(&mut self, dt: f32) -> bool {
        let dt = clamp_step(dt);
        let acceleration = (-self.k * (self.x - self.target) - self.c * self.v) / self.m;
        self.v += acceleration * dt;
        self.x += self.v * dt;
        if !(self.x.is_finite() && self.v.is_finite()) {
            // An explicit integrator that has been fed something it cannot represent
            // can only be recovered by returning to rest: a non-finite position would
            // reach the rasterizer and blank the window.
            self.snap();
            return true;
        }
        if self.at_rest() {
            self.snap();
        }
        self.at_rest()
    }

    /// Creates the spring that slides the page content into place.
    ///
    /// The content starts one [`PAGE_SLIDE_DP`] to the side the page came from --
    /// right for the next page, left for the previous one -- and springs home.
    pub fn for_page_slide(params: SpringParams, dir: PageDir) -> Self {
        let displaced = match dir {
            PageDir::Next => PAGE_SLIDE_DP,
            PageDir::Prev => -PAGE_SLIDE_DP,
        };
        let mut spring = Self::new(params.omega0, params.zeta, params.mass, displaced);
        spring.restart(displaced, 0.0);
        spring
    }

    /// The rest condition of 3.3.1: close enough and slow enough.
    fn at_rest(&self) -> bool {
        (self.x - self.target).abs() < REST_POSITION_DP && self.v.abs() < REST_VELOCITY_DP_PER_S
    }
}
