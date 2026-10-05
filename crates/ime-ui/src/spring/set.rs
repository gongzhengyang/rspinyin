//! Every motion the candidate window runs, and the pacing state they add up to.
//!
//! The UI thread owns one [`AnimationSet`] and asks it two questions per frame:
//! "advance by this much", and "is anything still moving". The second question is
//! the important one -- `poll(2)` may wait indefinitely only when *nothing* is in
//! flight, so somebody has to fold the rest conditions of the highlight, the window
//! fade and the page slide into a single answer. That is what keeps an idle window
//! from holding a timer, and it is why the motions live in one type rather than
//! three loose fields.
//!
//! The set also owns the `[ui.animation] enabled` switch. When it is off every
//! motion jumps straight to its end state, which is the low-end-device path and the
//! path a visual regression screenshot needs: the frame must not depend on how many
//! frames came before it.

use ime_types::PageDir;

use super::{
    AppearAnim, HighlightAnim, HighlightRect, HighlightStep, MotionConfig, PressSpring, Spring1D,
    SpringParams,
};

/// What one frame of the whole window's motion produced.
///
/// A single value rather than four out-parameters, because every field is consumed
/// by the same caller in the same breath: the renderer sets its properties from
/// `rect`, `opacity`, `scale` and `page_offset_dp`, and uses `animating` to decide
/// the next `poll` timeout. The damage a frame leaves is not part of the motion:
/// the renderer's own region -- what Slint reports it actually drew, recorded in
/// `crate::renderer`'s `record_damage` -- is the ground truth the copy and the
/// compositor report are built from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMotion {
    /// The highlight box and the region it moved through.
    pub highlight: HighlightStep,
    /// The window opacity for this frame, in `0.0..=1.0`.
    pub opacity: f32,
    /// The window scale for this frame.
    pub scale: f32,
    /// The page content's horizontal offset for this frame, in dp.
    pub page_offset_dp: f32,
    /// The scale the pressed cell draws at this frame: `1.0` at rest, and the pressed
    /// floor of 3.4's Active row once the sink has landed.
    pub press_scale: f32,
    /// Whether any motion is still in flight after this step.
    pub animating: bool,
}

/// The window's motions, driven by one clock.
#[derive(Clone, Debug)]
pub struct AnimationSet {
    highlight: HighlightAnim,
    appear: AppearAnim,
    page_slide: Spring1D,
    /// The pressed cell's sink (3.4's Active row), the set's fifth motion.
    press: PressSpring,
    config: MotionConfig,
}

impl AnimationSet {
    /// Creates the motions for a window that has not been shown yet.
    ///
    /// The highlight starts hidden on an empty rectangle and the page slide at rest:
    /// a window that has never appeared has nothing to draw and nothing to slide.
    pub fn new(config: MotionConfig) -> Self {
        let page = SpringParams::PAGE_SLIDE;
        Self {
            highlight: HighlightAnim::new(config.spring, HighlightRect::ZERO),
            appear: AppearAnim::with_scale_spring(config.scale_spring),
            page_slide: Spring1D::new(page.omega0, page.zeta, page.mass, 0.0),
            press: PressSpring::new(),
            config,
        }
    }

    /// The animated highlight box.
    pub fn highlight(&self) -> &HighlightAnim {
        &self.highlight
    }

    /// The window opacity, in `0.0..=1.0`.
    pub fn opacity(&self) -> f32 {
        self.appear.opacity()
    }

    /// The window scale.
    pub fn scale(&self) -> f32 {
        self.appear.scale()
    }

    /// The page content's horizontal offset, in dp.
    pub fn page_offset_dp(&self) -> f32 {
        self.page_slide.x
    }

    /// The scale the pressed cell draws at, `1.0` when no press is in flight.
    pub fn press_scale(&self) -> f32 {
        self.press.scale()
    }

    /// Starts or ends a press on a candidate cell (3.4's Active row).
    ///
    /// A repeated call with the answer the set already holds retargets the spring onto
    /// where it already travels and arms nothing, which is why the adapter may drive
    /// this from every pointer update. A window with the motion switched off lands the
    /// sink or the rebound at once, like every motion here.
    pub fn set_pressed(&mut self, down: bool) {
        self.press.set_pressed(down);
        if !self.config.enabled {
            self.press.snap();
        }
    }

    /// Whether any motion is still in flight.
    ///
    /// This is the only input the frame-pacing decision needs: when it is false the
    /// UI thread may block indefinitely, and when it is true the thread must come
    /// back for the next frame.
    pub fn is_animating(&self) -> bool {
        !(self.highlight.is_settled()
            && self.appear.is_settled()
            && self.page_slide.is_settled()
            && self.press.is_settled())
    }

    /// Turns the motion on or off, bringing everything to a stop when it goes off.
    ///
    /// Switching back on does not restart anything: the caller's next `retarget` or
    /// `appear` picks up from wherever the window currently is.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if !enabled {
            self.snap_all();
        }
    }

    /// Whether the set runs motions at all.
    ///
    /// This is `[ui.animation]`'s `enabled` as the set was built with it, or as
    /// [`Self::set_enabled`] last left it; the adapter reads it so the colour fades
    /// stay on the same switch as the springs.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Moves the highlight to `rect`, keeping the velocity of a box already in flight.
    pub fn retarget_highlight(&mut self, rect: HighlightRect) {
        if self.config.enabled {
            self.highlight.retarget(rect);
        } else {
            self.highlight.snap_to(rect);
        }
    }

    /// Shows or hides the highlight box.
    pub fn set_highlight_visible(&mut self, visible: bool) {
        self.highlight.set_visible(visible);
    }

    /// Starts the appear motion, continuing from the current opacity.
    pub fn appear(&mut self) {
        self.appear.appear(self.duration(self.config.appear_s));
    }

    /// Starts the disappear motion, continuing from the current opacity.
    ///
    /// A `Show` that arrives before it finishes calls [`AnimationSet::appear`], which
    /// resumes from the current value rather than from zero.
    pub fn disappear(&mut self) {
        self.appear
            .disappear(self.duration(self.config.disappear_s));
    }

    /// Starts the page-content slide for a page turn.
    ///
    /// The content is placed at the side it is coming from and springs home, which is
    /// why this restarts the spring instead of redirecting it: there is no velocity
    /// to carry over from the previous page.
    pub fn turn_page(&mut self, dir: PageDir) {
        self.page_slide = Spring1D::for_page_slide(SpringParams::PAGE_SLIDE, dir);
        if !self.config.enabled {
            self.page_slide.snap();
        }
    }

    /// Advances every motion by `dt` and reports the frame they produce.
    pub fn step(&mut self, dt: f32, scale: f32) -> FrameMotion {
        let highlight = self.highlight.step(dt, scale);
        self.appear.step(dt);
        self.page_slide.step(dt);
        self.press.step(dt);
        FrameMotion {
            highlight,
            opacity: self.appear.opacity(),
            scale: self.appear.scale(),
            page_offset_dp: self.page_slide.x,
            press_scale: self.press.scale(),
            animating: self.is_animating(),
        }
    }

    /// Brings every motion to its end state at once.
    ///
    /// This is the `[ui.animation] enabled = false` path and the screenshot path:
    /// both need a frame that does not depend on how many frames came before it.
    pub fn snap_all(&mut self) {
        self.highlight.snap();
        self.appear.snap_to_end();
        self.page_slide.snap();
        self.press.snap();
    }

    /// Resets the window fade to the hidden baseline of a window never shown.
    ///
    /// This is the reset the real unmap performs once the disappear motion has
    /// settled: that motion rests on the disappear's own end values, whose scale is
    /// not the appear's starting one, so a window re-shown from there would grow
    /// from a size no first show grows from. The highlight and the page slide are
    /// content motions the next frame re-drives, so they are left alone.
    pub fn reset_window_fade(&mut self) {
        self.appear.snap_hidden();
    }

    /// A configured duration, or zero when the window does not animate at all.
    fn duration(&self, configured_s: f32) -> f32 {
        if self.config.enabled {
            configured_s
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spring::PAGE_SLIDE_DP;

    const FRAME_S: f32 = 1.0 / 144.0;

    /// A candidate cell of the size the grid produces: 88dp wide, 30dp tall.
    fn cell(column: f32, row: f32) -> HighlightRect {
        HighlightRect::new(column * 88.0, row * 30.0, 88.0, 30.0)
    }

    /// Runs frames until nothing is moving, with a ceiling so a stuck motion fails
    /// the test rather than hanging it.
    fn run_to_rest(set: &mut AnimationSet) -> FrameMotion {
        let mut motion = set.step(FRAME_S, 1.0);
        let mut frames = 0u32;
        while motion.animating {
            frames += 1;
            assert!(frames < 1_000, "the window must stop animating");
            motion = set.step(FRAME_S, 1.0);
        }
        motion
    }

    #[test]
    fn test_animation_set_reports_idle_once_every_motion_settles() {
        let mut set = AnimationSet::new(MotionConfig::default());
        set.set_highlight_visible(true);
        set.retarget_highlight(cell(0.0, 0.0));
        set.appear();
        assert!(set.is_animating(), "both motions are in flight");

        let motion = run_to_rest(&mut set);
        assert!(!motion.animating);
        assert!(!set.is_animating(), "poll may wait indefinitely again");
        assert_eq!(set.opacity(), 1.0);
        assert_eq!(set.scale(), 1.0);

        // The frame after the last one has nothing left to do, which is what lets
        // the caller drop the timer entirely rather than leave it at zero.
        let idle = set.step(FRAME_S, 1.0);
        assert!(!idle.animating);
        assert!(
            idle.highlight.settled,
            "a resting highlight leaves the step nothing to advance"
        );
        assert_eq!(idle.page_offset_dp, 0.0);
    }

    #[test]
    fn test_animation_set_disabled_snaps_every_motion() {
        let mut set = AnimationSet::new(MotionConfig::instant());
        let rect = cell(2.0, 1.0);
        set.set_highlight_visible(true);
        set.retarget_highlight(rect);
        set.appear();
        set.turn_page(PageDir::Next);

        assert!(!set.is_animating(), "nothing is in flight");
        assert_eq!(set.opacity(), 1.0);
        assert_eq!(set.scale(), 1.0);
        assert_eq!(set.page_offset_dp(), 0.0);
        assert_eq!(set.highlight().rect(), rect, "already on the cell");

        // Turning the switch off mid-flight also lands everything at once.
        let mut animating = AnimationSet::new(MotionConfig::default());
        animating.set_highlight_visible(true);
        animating.retarget_highlight(cell(3.0, 0.0));
        animating.appear();
        animating.step(FRAME_S, 1.0);
        assert!(animating.is_animating());
        animating.set_enabled(false);
        assert!(!animating.is_animating());
        assert_eq!(animating.highlight().rect(), cell(3.0, 0.0));
    }

    #[test]
    fn test_animation_set_interrupted_disappear_continues_from_current_opacity() {
        let mut set = AnimationSet::new(MotionConfig::default());
        set.appear();
        run_to_rest(&mut set);
        assert_eq!(set.opacity(), 1.0);

        set.disappear();
        for _ in 0..6 {
            set.step(FRAME_S, 1.0);
        }
        let mid = set.opacity();
        assert!(
            mid > 0.0 && mid < 1.0,
            "the window is part way through fading out, got {mid}"
        );

        // A Show lands during the disappear. The fade must continue from where it is
        // rather than jump to zero and rise again.
        set.appear();
        assert_eq!(set.opacity(), mid, "no jump on the reversal");
        set.step(FRAME_S, 1.0);
        assert!(
            set.opacity() > mid,
            "the window must be fading back in, got {}",
            set.opacity()
        );
        // The appear runs for its configured duration from the moment of the
        // reversal, so it finishes on schedule with the value already part way up.
        let mut frames = 0u32;
        while set.opacity() < 1.0 {
            set.step(FRAME_S, 1.0);
            frames += 1;
            assert!(frames < 1_000, "the reversal must finish");
        }
        assert_eq!(set.opacity(), 1.0);
        assert!(!set.is_animating());
    }

    #[test]
    fn test_animation_set_page_turn_displaces_then_settles() {
        let mut set = AnimationSet::new(MotionConfig::default());
        set.turn_page(PageDir::Next);
        assert_eq!(set.page_offset_dp(), PAGE_SLIDE_DP);
        set.turn_page(PageDir::Prev);
        assert_eq!(set.page_offset_dp(), -PAGE_SLIDE_DP);

        let motion = run_to_rest(&mut set);
        assert_eq!(set.page_offset_dp(), 0.0, "lands where it belongs");
        assert!(motion.highlight.settled);
        // 3.3.2 derives 139ms for the page-slide spring; the content must be home in
        // well under the 160ms the table allots it.
        assert!(!set.is_animating());
    }

    #[test]
    fn test_animation_set_reset_window_fade_returns_to_the_hidden_baseline() {
        let mut set = AnimationSet::new(MotionConfig::default());
        set.appear();
        run_to_rest(&mut set);
        assert_eq!(set.opacity(), 1.0);
        set.disappear();
        run_to_rest(&mut set);
        assert_eq!(set.opacity(), 0.0);
        let rested_scale = set.scale();
        assert_ne!(
            rested_scale,
            AppearAnim::hidden().scale(),
            "the disappear rests on its own end scale, so the reset has something to move"
        );

        // The reset is the snap the real unmap performs: the fade goes back to the
        // state a never-shown window starts from, and the next appear is identical to
        // the first one instead of continuing from the disappear's end values.
        set.reset_window_fade();
        assert_eq!(set.opacity(), 0.0);
        assert_eq!(set.scale(), AppearAnim::hidden().scale());
        assert!(!set.is_animating(), "a reset is a snap, not a motion");

        // The appear that follows grows from that baseline, exactly as a first
        // appear does.
        set.appear();
        set.step(FRAME_S, 1.0);
        assert!(
            set.opacity() > 0.0,
            "the fade rises again from the baseline, got {}",
            set.opacity()
        );
        assert!(set.scale() > AppearAnim::hidden().scale());
    }

    #[test]
    fn test_animation_set_highlight_step_reports_rect_and_settled_only() {
        let mut set = AnimationSet::new(MotionConfig::default());
        set.set_highlight_visible(true);
        set.retarget_highlight(cell(0.0, 1.0));
        let motion = run_to_rest(&mut set);
        assert_eq!(motion.highlight.rect, cell(0.0, 1.0));
        assert!(motion.highlight.settled);
        // The step carries no damage of its own: what a frame changed is the renderer's
        // region (`crate::renderer`'s `record_damage`), so the set's report ends at the
        // rectangle and the rest condition. The frame after the last moving one has
        // nothing left to do, which is what lets the caller drop its frame timer rather
        // than leave it armed at zero.
        let idle = set.step(FRAME_S, 1.0);
        assert!(idle.highlight.settled);
        assert!(!idle.animating);
    }

    #[test]
    fn test_animation_set_press_sinks_and_releases_as_one_motion_of_the_set() {
        let mut set = AnimationSet::new(MotionConfig::default());
        assert_eq!(
            set.press_scale(),
            1.0,
            "a fresh set draws every cell full size"
        );
        set.set_pressed(true);
        assert!(set.is_animating(), "the sink is in flight");
        run_to_rest(&mut set);
        assert_eq!(set.press_scale(), crate::spring::PRESS_SCALE_TO);
        assert!(!set.is_animating(), "a landed press arms no timer");

        // A press the pointer already holds reaches the set again on the next pointer
        // update; it must answer with rest, not with a motion that never ends.
        set.set_pressed(true);
        assert!(!set.is_animating());

        // The release rebounds, and the set goes idle once it has landed.
        set.set_pressed(false);
        assert!(set.is_animating(), "the rebound is in flight");
        run_to_rest(&mut set);
        assert_eq!(set.press_scale(), 1.0);
        assert!(!set.is_animating());
    }

    #[test]
    fn test_animation_set_disabled_press_lands_at_once() {
        // The disabled path is the screenshot path: the pressed cell draws its pressed
        // scale on the first frame, with nothing left in flight.
        let mut set = AnimationSet::new(MotionConfig::instant());
        set.set_pressed(true);
        assert!(!set.is_animating(), "the disabled path springs nothing");
        assert_eq!(set.press_scale(), crate::spring::PRESS_SCALE_TO);

        // Switching the motion off mid-sink lands the press where it was headed.
        let mut set = AnimationSet::new(MotionConfig::default());
        set.set_pressed(true);
        set.step(FRAME_S, 1.0);
        assert!(set.is_animating());
        set.set_enabled(false);
        assert!(!set.is_animating());
        assert_eq!(set.press_scale(), crate::spring::PRESS_SCALE_TO);
    }

    #[test]
    fn test_animation_set_page_turn_during_a_press_releases_the_sink() {
        // A page turn invalidates the press -- the router forgets it when the new
        // frame is adopted -- and the cell the pointer no longer presses rebounds
        // while the page content slides in: both motions run on the one clock, and
        // the set goes idle only when both have landed.
        let mut set = AnimationSet::new(MotionConfig::default());
        set.set_pressed(true);
        set.step(FRAME_S, 1.0);
        set.turn_page(PageDir::Next);
        set.set_pressed(false);
        assert!(set.is_animating());
        let motion = run_to_rest(&mut set);
        assert_eq!(set.press_scale(), 1.0, "the invalidated press rebounded");
        assert_eq!(motion.page_offset_dp, 0.0, "the page content slid home");
        assert!(!set.is_animating());
    }
}
