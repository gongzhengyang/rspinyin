//! The adapter's surface half: mapping and unmapping the window, the staged exit
//! fade a hide becomes, and the motion tick that completes it.

use ime_types::ImeError;
use slint::ComponentHandle as _;

use super::Adapter;

/// Recorded when the window cannot be mapped or unmapped.
pub const SURFACE_FAILED_CODE: &str = "ui/slint/surface";

impl Adapter {
    /// Maps or unmaps the surface, and starts the motion that goes with it.
    ///
    /// Idempotent: asking for the visibility the window already has writes nothing. Mapping
    /// the surface is what makes the component appear at all -- an unshown component is laid
    /// out but never drawn into a buffer -- and it also starts 3.3.2's appear motion, so the
    /// panel grows out of the caret instead of arriving at full size.
    ///
    /// Unmapping is *staged*, not executed: the window stays mapped while the disappear
    /// motion fades it out, and [`Self::advance`] performs the real unmap once that motion
    /// settles. Unmapping first would run the spring in a window nobody can see -- a stretch
    /// of deadline wakeups that draws nothing -- and would cut the exit fade the appear
    /// motion is the mirror of. A `Show` landing during the staged fade cancels the unmap:
    /// the window never left the screen, so there is nothing to map, and the appear motion
    /// resumes from the opacity the fade had reached.
    ///
    /// # Parameters
    ///
    /// * `visible` -- `true` to map the surface, `false` to stage its unmap.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] carrying [`SURFACE_FAILED_CODE`] when the
    /// platform refuses to map the window, which means the candidate window cannot be shown
    /// at all. Staging an unmap cannot fail; the unmap itself reports through
    /// [`Self::advance`] and [`Self::unmap_now`].
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn set_visible(&mut self, visible: bool) -> Result<(), ImeError> {
        // A second Hide is the request already being served; a Show during the fade keeps
        // the window -- never unmapped -- and fades it back up from where the fade got to.
        if self.pending_hide {
            if !visible {
                return Ok(());
            }
            self.pending_hide = false;
            self.motion.appear();
            return Ok(());
        }
        if self.visible == visible {
            return Ok(());
        }
        if visible {
            self.window.show().map_err(|_| surface_error())?;
            self.visible = true;
            self.motion.appear();
            return Ok(());
        }
        self.pending_hide = true;
        self.motion.disappear();
        Ok(())
    }

    /// Advances the window's motion and colour fades by `dt` and writes the frame
    /// they produced.
    ///
    /// The delta comes from the UI thread's clock, which is the only place an instant is read:
    /// a session's first call passes zero, and a late frame passes a delta large enough that
    /// the integrator clamps it, so a stalled compositor slows the motion down rather than
    /// making it jump. The fades -- the theme's accent and alpha, and the status markers' --
    /// advance on the same delta, and a fade frame equal to the one already written costs
    /// nothing, so a window at rest still writes no property at all.
    ///
    /// A staged unmap ([`Self::set_visible`]) completes here: the first step after the exit
    /// fade has settled performs the real unmap. Running it here rather than where the Hide
    /// landed is what puts the fade on screen, and what lets the caller drop its frame
    /// deadline on the very frame the motion runs out.
    ///
    /// # Parameters
    ///
    /// * `dt` -- seconds since the previous call. Negative, zero and unusable values carry no
    ///   time rather than running the motion backwards.
    ///
    /// # Returns
    ///
    /// Whether anything is still in flight. `false` is what lets the caller block
    /// indefinitely, which is the whole of the idle CPU budget: a window at rest must not keep
    /// a timer armed.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] carrying [`SURFACE_FAILED_CODE`] when the
    /// staged unmap cannot be performed, which would otherwise leave a fully transparent
    /// window mapped for ever.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn advance(&mut self, dt: f32) -> Result<bool, ImeError> {
        let motion = self.motion.step(dt, self.scale);
        let fading = self.step_fades(dt);
        // The visibility of the box is not part of `FrameMotion`, so it is checked against the
        // component's own value rather than against the previous frame. A box that hid without
        // moving -- the page emptied while the highlight was already at rest on a cell -- would
        // otherwise produce a frame equal to the one before it and never reach the component,
        // leaving it drawing a box the engine no longer has a candidate for.
        let visible = self.motion.highlight().is_visible();
        if self.drawn_motion != Some(motion) || self.window.get_highlight_visible() != visible {
            self.write_motion(&motion);
            // The pressed cell's scale travels in the motion frame, so a frame that
            // differs at all writes it too and a settled window still writes nothing.
            self.window.set_press_scale(motion.press_scale);
            self.drawn_motion = Some(motion);
            self.request_repaint();
        }
        if !motion.animating && !fading && self.pending_hide {
            self.unmap_now()?;
            return Ok(false);
        }
        Ok(motion.animating || fading)
    }

    /// Unmaps the window now, ending a staged fade at whatever opacity it reached.
    ///
    /// [`Self::advance`] calls this on the frame a staged unmap's fade has settled; the
    /// surface's shutdown path calls it directly, because the host's stop budget is 200ms,
    /// the loop stops right after the close call, and a fade there has no frame to run on.
    /// The fade is reset to the hidden baseline, so the next appear is identical to the
    /// first one instead of continuing from the disappear's own end values.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] carrying [`SURFACE_FAILED_CODE`] when
    /// the platform refuses to unmap the window.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn unmap_now(&mut self) -> Result<(), ImeError> {
        self.pending_hide = false;
        if !self.visible {
            return Ok(());
        }
        self.window.hide().map_err(|_| surface_error())?;
        self.visible = false;
        self.motion.reset_window_fade();
        // The colour fades have no frame to run on once the window is unmapped, so
        // they land on their end values here rather than keeping a deadline armed
        // over a window nobody can see.
        let glide = self.theme_fade.target();
        self.theme_fade.snap_to(glide);
        self.state.snap_markers();
        Ok(())
    }
}

/// The error a window that cannot be mapped or unmapped reports.
fn surface_error() -> ImeError {
    ImeError::CompositorUnsupported {
        detail: String::from(SURFACE_FAILED_CODE),
    }
}
