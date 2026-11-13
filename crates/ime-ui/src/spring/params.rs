//! Spring parameters: the physical presets of `features.md` 3.3 and the derived
//! metrics (settling time, overshoot) the tests measure the integrator against.

/// Natural frequency of the highlight-slide spring, in rad/s.
pub const HIGHLIGHT_OMEGA0: f32 = 26.0;

/// Damping ratio of the highlight-slide spring.
pub const HIGHLIGHT_ZETA: f32 = 0.85;

/// Natural frequency of the page-slide spring, in rad/s.
pub const PAGE_OMEGA0: f32 = 32.0;

/// Damping ratio of the page-slide spring.
pub const PAGE_ZETA: f32 = 0.90;

/// Natural frequency of the window-resize spring, in rad/s.
pub const RESIZE_OMEGA0: f32 = 30.0;

/// Damping ratio of the window-resize spring.
pub const RESIZE_ZETA: f32 = 0.92;

/// Natural frequency of the scale spring the appear and disappear fades run on, in
/// rad/s. It is the built-in default of `[ui.animation]`'s `omega0`.
pub const SCALE_OMEGA0: f32 = 26.0;

/// Damping ratio of the scale spring the appear and disappear fades run on. It is the
/// built-in default of `[ui.animation]`'s `zeta`.
pub const SCALE_ZETA: f32 = 0.85;

/// The mass every spring in the window runs with.
pub const DEFAULT_MASS: f32 = 1.0;

/// The physical parameters of one spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// Undamped angular frequency, in rad/s.
    pub omega0: f32,
    /// Damping ratio: below `1.0` overshoots, at `1.0` is critically damped.
    pub zeta: f32,
    /// Mass. Only the ratios `k / m` and `c / m` reach the integrator, so this is a
    /// scale rather than a physical quantity.
    pub mass: f32,
}

impl SpringParams {
    /// The highlight-slide spring of 3.3.1.
    pub const HIGHLIGHT: Self = Self {
        omega0: HIGHLIGHT_OMEGA0,
        zeta: HIGHLIGHT_ZETA,
        mass: DEFAULT_MASS,
    };

    /// The page-content slide of 3.3.2.
    pub const PAGE_SLIDE: Self = Self {
        omega0: PAGE_OMEGA0,
        zeta: PAGE_ZETA,
        mass: DEFAULT_MASS,
    };

    /// The window-resize spring of 3.3.2.
    pub const RESIZE: Self = Self {
        omega0: RESIZE_OMEGA0,
        zeta: RESIZE_ZETA,
        mass: DEFAULT_MASS,
    };

    /// The spring that carries the window's scale through the appear and disappear
    /// fades. `[ui.animation]`'s `omega0` and `zeta` replace it in a
    /// configuration-assembled [`super::MotionConfig`].
    pub const SCALE: Self = Self {
        omega0: SCALE_OMEGA0,
        zeta: SCALE_ZETA,
        mass: DEFAULT_MASS,
    };

    /// Builds parameters, replacing values the integrator cannot use.
    ///
    /// A frequency, damping ratio or mass that is zero, negative or not a number is
    /// answered with the highlight-slide default rather than propagated: a window
    /// that moves at slightly the wrong speed is usable, while a window whose
    /// integrator produces `NaN` is not. The configuration layer rejects these
    /// values before they get here; this is the second line.
    pub fn new(omega0: f32, zeta: f32, mass: f32) -> Self {
        Self {
            omega0: if omega0.is_finite() && omega0 > 0.0 {
                omega0
            } else {
                HIGHLIGHT_OMEGA0
            },
            zeta: if zeta.is_finite() && zeta >= 0.0 {
                zeta
            } else {
                HIGHLIGHT_ZETA
            },
            mass: if mass.is_finite() && mass > 0.0 {
                mass
            } else {
                DEFAULT_MASS
            },
        }
    }

    /// Builds the highlight-slide parameters from the tunable configuration keys.
    ///
    /// Only the frequency and the damping ratio are user-configurable, so the mass
    /// comes from [`DEFAULT_MASS`].
    pub fn from_config(omega0: f32, zeta: f32) -> Self {
        Self::new(omega0, zeta, DEFAULT_MASS)
    }

    /// The time the spring needs to stay inside a two-percent band, in seconds.
    ///
    /// `4 / (zeta * omega0)`, the standard first-order envelope estimate. It is the
    /// figure 3.3.1 derives from the highlight parameters, and the value the
    /// implementation's measured settling time is checked against.
    pub fn settling_time_s(self) -> f32 {
        let decay = self.zeta * self.omega0;
        if decay > 0.0 { 4.0 / decay } else { 0.0 }
    }

    /// The peak overshoot of a step response, as a fraction of the step.
    ///
    /// `exp(-pi * zeta / sqrt(1 - zeta^2))` while the spring is underdamped, and zero
    /// once it is critically damped or over-damped.
    pub fn overshoot(self) -> f32 {
        if self.zeta >= 1.0 {
            return 0.0;
        }
        let root = 1.0 - self.zeta * self.zeta;
        if root <= 0.0 {
            return 0.0;
        }
        let exponent = core::f32::consts::PI * self.zeta / root.sqrt();
        (-exponent).exp()
    }
}
