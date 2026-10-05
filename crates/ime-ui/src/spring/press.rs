//! The pressed cell's sink: 3.4's Active row as one motion.
//!
//! A candidate cell a pointer is holding sinks to [`PRESS_SCALE_TO`] of its size and
//! rebounds to full size on release, on a single critically damped spring. The spring is
//! deliberately not a timed transition: a release in the middle of the sink reverses a
//! spring that is already moving, and a redirect carries the speed the sink had built up
//! into the rebound, which is what makes the gesture read as physical rather than as a
//! played-back clip.
//!
//! The parameters come from the Active row's own allotment: the sink has 60ms, and the
//! damping ratio is exactly critical so that neither the sink nor the rebound ever passes
//! its target -- a pressed cell that dipped below its pressed size and came back would
//! read as a glitch, not as weight.

use super::Spring1D;

/// The scale a pressed cell sinks to: the visible part of 3.4's 3% press.
pub const PRESS_SCALE_TO: f32 = 0.985;

/// Natural frequency of the press spring, in rad/s.
///
/// The critical-damping rest condition of the shared integrator -- position within its
/// rest error and speed within its rest speed -- is reached at about `6.4 / omega0`; at
/// 130 rad/s that is inside 50ms, which lands the sink and the rebound both inside the
/// 60ms the Active row allots, with a frame to spare at the 144Hz the budget is measured
/// at.
pub const PRESS_OMEGA0: f32 = 130.0;

/// Damping ratio of the press spring.
///
/// Critically damped: the sink is monotone, the release rebounds without ringing, and a
/// rapid press-release-press never overshoots the size it is travelling towards.
pub const PRESS_ZETA: f32 = 1.0;

/// The unit the press spring's position travels in: thousandths of the cell scale.
///
/// The shared [`Spring1D`] rest condition -- half a unit of position, twenty units of
/// speed per second -- is sized for the dp the layout springs travel in, and a press
/// travels fifteen thousandths of a scale factor. Running the spring in these units puts
/// the travel at 15, so the same thresholds mean a resting error of half a thousandth of
/// a percent of scale and a resting speed of two percent of scale per second: on a 36dp
/// cell both sit far below one physical pixel, which is exactly where a press should
/// stop being distinguishable from its end state.
const PRESS_UNITS_PER_SCALE: f32 = 1000.0;

/// The pressed-cell motion of 3.4's Active row.
///
/// The sink to [`PRESS_SCALE_TO`] and the rebound to full size, on one critically damped
/// spring. The value it produces is the cell scale the grid draws the pressed cell's
/// background and corner radius at; the text a cell carries is deliberately not scaled.
#[derive(Clone, Debug)]
pub struct PressSpring {
    /// The integrator, in [`PRESS_UNITS_PER_SCALE`].
    spring: Spring1D,
}

impl Default for PressSpring {
    fn default() -> Self {
        Self::new()
    }
}

impl PressSpring {
    /// Creates the spring at rest and released: every cell draws at full size.
    pub fn new() -> Self {
        Self {
            spring: Spring1D::new(
                PRESS_OMEGA0,
                PRESS_ZETA,
                crate::spring::DEFAULT_MASS,
                PRESS_UNITS_PER_SCALE,
            ),
        }
    }

    /// Starts the sink (`down`) or the release rebound (`!down`).
    ///
    /// A redirect rather than a restart, on purpose: a release in the middle of a sink
    /// continues from the speed the sink had built up, and a repeated call with the same
    /// answer retargets the spring onto where it already travels, which moves nothing.
    /// That is why the adapter can drive this from every pointer update without arming a
    /// motion that is not there.
    pub fn set_pressed(&mut self, down: bool) {
        let target = if down { PRESS_SCALE_TO } else { 1.0 };
        self.spring.retarget(target * PRESS_UNITS_PER_SCALE);
    }

    /// Jumps to the target and stops.
    ///
    /// This is the `[ui.animation] enabled = false` path, where the pressed cell must
    /// draw its pressed scale at once, and the tests that need a deterministic frame.
    pub fn snap(&mut self) {
        self.spring.snap();
    }

    /// Whether the spring is at rest.
    ///
    /// Folding this into the window's idle answer is what keeps a settled press from
    /// holding the UI thread's timer: the frame after the spring lands reports nothing
    /// in flight.
    pub fn is_settled(&self) -> bool {
        self.spring.is_settled()
    }

    /// Advances the spring by `dt` and reports whether it is at rest.
    ///
    /// The delta is clamped exactly as every other motion's is, so a late frame slows
    /// the sink down instead of letting the integrator jump. The step is split into
    /// fixed sub-steps because the shared integrator is an explicit one: at 130 rad/s
    /// a whole 144Hz frame is `omega * dt = 0.9`, which the damping term of a critically
    /// damped spring turns into a visible ring across the target, and a pressed cell
    /// that overshoots its sink reads as a glitch. Four sub-steps put every integration
    /// slice at `omega * dt = 0.23`, where the discrete spring follows the continuous
    /// one closely enough that the sink lands inside its 60ms without crossing it.
    pub fn step(&mut self, dt: f32) -> bool {
        // The shared integrator clamps the step it is given, but a clamp of a huge
        // frame still lands on the one step size this omega is numerically unstable
        // at -- so the frame is clamped here first and the stable slice is a quarter
        // of the clamped step.
        let dt = match dt {
            dt if dt.is_finite() => dt.clamp(0.0, crate::spring::MAX_STEP_S),
            _ => 0.0,
        };
        let slice = dt / 4.0;
        let mut settled = false;
        for _ in 0..4 {
            settled = self.spring.step(slice);
        }
        settled
    }

    /// The cell scale this frame draws, in `PRESS_SCALE_TO..=1.0`.
    pub fn scale(&self) -> f32 {
        self.spring.x / PRESS_UNITS_PER_SCALE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame at the rate the motion budget is measured at.
    const FRAME_S: f32 = 1.0 / 144.0;

    #[test]
    fn test_press_spring_sinks_monotonically_to_the_pressed_scale_within_60ms() {
        let mut spring = PressSpring::new();
        assert_eq!(
            spring.scale(),
            1.0,
            "a fresh spring draws every cell full size"
        );
        assert!(
            spring.is_settled(),
            "and nothing is in flight before a press"
        );
        spring.set_pressed(true);
        assert!(!spring.is_settled(), "the sink is in flight");

        let mut previous = spring.scale();
        let mut frames = 0u32;
        while !spring.step(FRAME_S) {
            frames += 1;
            assert!(
                spring.scale() < previous,
                "the sink is monotone: {} after {}",
                spring.scale(),
                frames
            );
            assert!(
                spring.scale() >= PRESS_SCALE_TO,
                "a critically damped sink never passes its target, got {}",
                spring.scale()
            );
            previous = spring.scale();
        }
        // The last step snapped onto the target, which is the exact end value the
        // table fixes -- not a near-miss the renderer would smear across cells.
        assert!((spring.scale() - PRESS_SCALE_TO).abs() < 1.0e-6);
        assert!(spring.is_settled());
        // The 60ms the Active row allots the sink, rounded up to one 144Hz frame.
        assert!(
            frames <= 9,
            "the sink took {frames} frames, past the 60ms it is allotted"
        );
    }

    #[test]
    fn test_press_spring_release_mid_sink_rebounds_without_ringing() {
        let mut spring = PressSpring::new();
        spring.set_pressed(true);
        for _ in 0..3 {
            spring.step(FRAME_S);
        }
        let mid_sink = spring.scale();
        assert!(
            mid_sink > PRESS_SCALE_TO && mid_sink < 1.0,
            "the release lands while the cell is still sinking, got {mid_sink}"
        );

        spring.set_pressed(false);
        let mut previous = spring.scale();
        let mut frames = 0u32;
        while !spring.step(FRAME_S) {
            frames += 1;
            assert!(
                spring.scale() > previous,
                "the rebound is monotone: {} after {}",
                spring.scale(),
                frames
            );
            assert!(
                spring.scale() <= 1.0,
                "a critically damped rebound never rings past full size, got {}",
                spring.scale()
            );
            previous = spring.scale();
        }
        assert!((spring.scale() - 1.0).abs() < 1.0e-6);
        assert!(
            frames <= 9,
            "the rebound took {frames} frames, past the 60ms it is allotted"
        );
    }

    #[test]
    fn test_press_spring_of_a_redundant_or_degenerate_press_changes_nothing() {
        let mut spring = PressSpring::new();
        // A press the pointer already holds is reported on every pointer update; the
        // spring must answer one that changes nothing with "nothing is in flight",
        // which is what keeps a stationary press from holding a timer.
        spring.set_pressed(false);
        assert!(spring.is_settled());
        assert!(spring.step(FRAME_S), "a settled spring stays settled");
        assert_eq!(spring.scale(), 1.0);

        // A frame clamped to the shared 1/60s ceiling and sliced four ways still
        // carries 16ms of integration -- not enough to finish the 49ms sink, so the
        // cell is part way down and strictly between the two ends. A frame far past
        // the ceiling carries no more time than the clamped one did, so the state
        // stays finite and where it was; and a non-finite or negative frame carries
        // no time at all, so the honest answer is the one the state gives: still in
        // flight, still where it was.
        spring.set_pressed(true);
        spring.step(crate::spring::MAX_STEP_S);
        assert!(spring.scale() > PRESS_SCALE_TO && spring.scale() < 1.0);
        spring.step(5.0);
        assert!(spring.scale() > PRESS_SCALE_TO && spring.scale() < 1.0);
        let held = spring.scale();
        assert!(!spring.step(f32::NAN));
        assert!(!spring.step(-FRAME_S));
        assert_eq!(spring.scale(), held);

        // The disabled-motion path snaps straight onto the pressed scale.
        let mut snapped = PressSpring::new();
        snapped.set_pressed(true);
        snapped.snap();
        assert_eq!(snapped.scale(), PRESS_SCALE_TO);
        assert!(snapped.is_settled());
    }
}
