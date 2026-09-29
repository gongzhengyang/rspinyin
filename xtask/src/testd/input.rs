//! The injection channel: focusing the client under test, and sending keys and pointer
//! events through XTEST.
//!
//! Responsibility: this module decides *when* something is injected and *what is checked
//! around it*. The protocol, the tables and the coordinate arithmetic live beside it in
//! [`super::x11`], [`super::keys`] and [`super::coords`].
//!
//! # Why events go through XTEST
//!
//! An injected key travels the same path a human's key does: the X server, the `xcb`
//! frontend, Fcitx5, the addon. Driving the engine directly would be faster and would prove
//! nothing about that path -- that is a different channel with a different contract -- so
//! nothing here shortcuts the server.
//!
//! # Why every injection takes a guard
//!
//! Losing the keyboard focus is this project's highest-severity defect, and a harness that
//! *can* forget to check it eventually will. So the check is not a step a test may skip:
//! [`X11Injector::focus`] hands back a [`FocusGuard`], and every injection requires one.
//! There is no way to send a key without naming the window that must still hold the focus
//! afterwards, and no way to send one without the server being asked before and after.

use std::os::fd::RawFd;
use std::thread;
use std::time::Duration;

use super::TestError;
use super::coords::{DEFAULT_SHADOW_DP, WindowPlacement, container_to_screen, on_screen};
use super::keys::{HOLDABLE_MASKS, HOLDABLE_ORDER, modifier_keysym, strokes};
use super::x11::{Window, X11Session, XtestVersion};

/// The wheel button one notch backwards is.
const WHEEL_UP: u8 = 4;
/// The wheel button one notch forwards is.
const WHEEL_DOWN: u8 = 5;

/// Most wheel notches one `scroll` call will send.
///
/// A test scrolls one to three notches; a request far beyond that is a bug, and the cap is
/// what keeps such a bug from spinning out a thousand events.
const MAX_NOTCHES: u32 = 32;

/// The `WM_CLASS` the candidate window carries.
///
/// It is `ime-ui`'s own class string, and it is what lets a focus failure say whether the
/// candidate window is the culprit rather than leaving the reader to decode a window id.
const CANDIDATE_CLASS: &[u8] = b"rspinyin";

/// The default delay between two injected keys.
///
/// Below roughly this value the host's event loop starts dropping keys, which is why the
/// channel paces the injection at all instead of sending a word as fast as it can.
pub const DEFAULT_KEY_DELAY: Duration = Duration::from_millis(8);

/// One synthetic input channel bound to a live X11 session.
pub struct X11Injector {
    /// The session the events go out on.
    session: X11Session,
    /// Device pixel ratio of the candidate window, for the logical-to-physical step of the
    /// coordinate conversion.
    scale: f32,
}

impl X11Injector {
    /// Opens a session and binds a channel to it.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::NoDisplay`] when the display cannot be opened, and
    /// [`TestError::NoXtest`] when it has no usable XTEST extension.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn connect(display: Option<&str>, scale: f32) -> Result<Self, TestError> {
        Ok(Self {
            session: X11Session::connect(display)?,
            scale,
        })
    }

    /// The session behind the channel.
    ///
    /// Only this module's own live test reaches for it today. The later test-platform tasks
    /// need the raw connection for screenshots and window lookups, and this is where they
    /// will take it from; until then the accessor exists for the test target alone, which
    /// is what keeps it from being dead code in the tool's own build.
    #[cfg(test)]
    pub fn session(&self) -> &X11Session {
        &self.session
    }

    /// The root window, the origin of screen coordinates.
    pub fn root(&self) -> Window {
        self.session.root()
    }

    /// The device pixel ratio the channel converts logical geometry with.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The file descriptor of the X connection, for a `poll(2)` loop.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn connection_fd(&self) -> RawFd {
        self.session.connection_fd()
    }

    /// The screen size in physical pixels.
    pub fn screen(&self) -> (u16, u16) {
        self.session.screen()
    }

    /// The XTEST version the server reported.
    pub fn xtest(&self) -> XtestVersion {
        self.session.xtest()
    }

    /// The display name, for diagnostics.
    pub fn display(&self) -> &str {
        self.session.display()
    }

    /// The window the server currently considers focused.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the query cannot be answered.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn input_focus(&self) -> Result<Window, TestError> {
        self.session.input_focus()
    }

    /// Moves the input focus to `window` and returns the guard every injection needs.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::FocusRefused`] when the server reports a different window
    /// after the request -- a window manager that immediately moves the focus back, a
    /// window that cannot take it, or one that is no longer there.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn focus(&self, window: Window) -> Result<FocusGuard, TestError> {
        self.session.set_input_focus(window)?;
        let actual = self.session.input_focus()?;
        focused(window, actual)?;
        Ok(FocusGuard { window })
    }

    /// Types one keysym with the given modifier mask, press then release.
    ///
    /// The modifiers are pressed around the key and released after it, whatever the key
    /// did, so a failure cannot leave one down for the rest of the run. Any modifier the
    /// server reports as already held is released first, which is the `--clearmodifiers`
    /// behaviour: a test that runs while the user holds Shift must not have its keys arrive
    /// as chords.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::UnsupportedModifiers`] for a mask the channel cannot hold,
    /// [`TestError::NoKeycode`] when the layout has no key for the keysym, and
    /// [`TestError::FocusStolen`] when the focus moved while the key was being sent.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn key(&self, guard: &FocusGuard, keysym: u32, state: u16) -> Result<(), TestError> {
        guard.verify(self)?;
        self.stroke(keysym, state)?;
        guard.verify(self)
    }

    /// Types one stroke, with its modifiers held around it.
    fn stroke(&self, keysym: u32, state: u16) -> Result<(), TestError> {
        if state & !HOLDABLE_MASKS != 0 {
            return Err(TestError::UnsupportedModifiers { state });
        }
        self.clear_modifiers()?;
        // Every keycode is resolved before anything is pressed, so a keysym the layout
        // lacks cannot leave a modifier held down.
        let keycode = self.session.keycode(keysym)?;
        let modifiers = self.modifier_keycodes(state)?;
        self.press(&modifiers)?;
        let typed = self
            .session
            .fake_key(keycode, true)
            .and_then(|()| self.session.fake_key(keycode, false));
        self.release(&modifiers)?;
        typed
    }

    /// Types `text` one character at a time, waiting `delay` between two keys.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::UnsupportedChar`] when a character has no key on the layout,
    /// and whatever [`X11Injector::key`] returns for each stroke. A string is refused as a
    /// whole before the first key goes out, so a run never leaves half of it behind.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn type_text(
        &self,
        guard: &FocusGuard,
        text: &str,
        delay: Duration,
    ) -> Result<(), TestError> {
        let planned = strokes(text)?;
        let last = planned.len().saturating_sub(1);
        for (index, stroke) in planned.iter().enumerate() {
            self.key(guard, stroke.keysym, stroke.state)?;
            if index < last && !delay.is_zero() {
                thread::sleep(delay);
            }
        }
        Ok(())
    }

    /// Moves the pointer to a screen point and clicks one button there.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the request cannot be delivered, and
    /// [`TestError::FocusStolen`] when the click moved the focus away from the client under
    /// test -- which is what a click on a candidate cell would do if the window ever took
    /// the keyboard.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn click(
        &self,
        guard: &FocusGuard,
        point: (i16, i16),
        button: u8,
    ) -> Result<(), TestError> {
        guard.verify(self)?;
        self.session.fake_motion(point)?;
        self.session.fake_button(button, true)?;
        self.session.fake_button(button, false)?;
        guard.verify(self)
    }

    /// Moves the pointer to a screen point and turns the wheel `delta` notches there.
    ///
    /// The sign follows the axis event the UI thread consumes: a negative delta pages back
    /// and a positive one pages forward, which are buttons 4 and 5. A request for more
    /// notches than the channel's cap is clamped rather than sent.
    ///
    /// # Errors
    ///
    /// As [`X11Injector::click`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn scroll(
        &self,
        guard: &FocusGuard,
        point: (i16, i16),
        delta: i32,
    ) -> Result<(), TestError> {
        guard.verify(self)?;
        self.session.fake_motion(point)?;
        let button = wheel_button(delta);
        for _ in 0..notches(delta) {
            self.session.fake_button(button, true)?;
            self.session.fake_button(button, false)?;
        }
        guard.verify(self)
    }

    /// Checks a screen-absolute point against the live screen.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::OffScreen`] for a point that is not on the screen.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn on_screen(&self, point: (i32, i32)) -> Result<(i16, i16), TestError> {
        on_screen(
            i64::from(point.0),
            i64::from(point.1),
            self.session.screen(),
        )
    }

    /// Screen-absolute pixels of a point in the candidate container.
    ///
    /// This is the conversion a test uses to aim at a cell: the hit map's rectangle is
    /// container-relative, so its centre goes through here before it can be clicked.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::OffScreen`] when the result is not on the screen.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn hit_point(
        &self,
        placement: &WindowPlacement,
        point: (i32, i32),
    ) -> Result<(i16, i16), TestError> {
        container_to_screen(placement, point, self.session.screen())
    }

    /// Reads where a window sits on the screen.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the server cannot answer.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn placement_of(&self, window: Window) -> Result<WindowPlacement, TestError> {
        self.session
            .window_placement(window, DEFAULT_SHADOW_DP, self.scale)
    }

    /// The keycodes of the modifiers a mask asks for, in press order.
    fn modifier_keycodes(&self, state: u16) -> Result<Vec<u8>, TestError> {
        let mut keycodes = Vec::new();
        for mask in HOLDABLE_ORDER {
            if state & mask == 0 {
                continue;
            }
            let keysym = modifier_keysym(mask).ok_or(TestError::UnsupportedModifiers { state })?;
            keycodes.push(self.session.keycode(keysym)?);
        }
        Ok(keycodes)
    }

    /// Presses `keycodes` in order, releasing what was already pressed if one fails.
    fn press(&self, keycodes: &[u8]) -> Result<(), TestError> {
        for (index, keycode) in keycodes.iter().enumerate() {
            if let Err(error) = self.session.fake_key(*keycode, true) {
                self.release(&keycodes[..index])?;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Releases `keycodes` in reverse order.
    fn release(&self, keycodes: &[u8]) -> Result<(), TestError> {
        for keycode in keycodes.iter().rev() {
            self.session.fake_key(*keycode, false)?;
        }
        Ok(())
    }

    /// Releases every modifier the server reports as physically held.
    fn clear_modifiers(&self) -> Result<(), TestError> {
        let held = self.session.held_modifiers()?;
        self.release(&held)
    }

    /// Whether `window` is the candidate window, judged by its `WM_CLASS`.
    fn is_candidate_window(&self, window: Window) -> bool {
        self.session
            .window_class(window)
            .is_some_and(|class| class.starts_with(CANDIDATE_CLASS))
    }
}

impl Drop for X11Injector {
    /// Releases whatever the run left held, so a failure cannot leave the server modified.
    ///
    /// [`X11Injector::stroke`] already releases its own modifiers whatever the key did, so
    /// this is the second line: it catches the case where the run ended between a
    /// modifier's press and the release that was supposed to follow it -- an aborted
    /// assertion, a panicking test, a returned error the caller stopped at. A modifier left
    /// down would make every later key of the session arrive as a chord.
    ///
    /// The result is deliberately discarded: a drop has nowhere to report a failure to, and
    /// the only failure left at this point is a connection that is already gone.
    fn drop(&mut self) {
        let _ = self.clear_modifiers();
    }
}

/// A recorded focus expectation: the window that must still hold the keyboard.
///
/// Every injection takes one, which is what makes the focus rule impossible to skip by
/// accident. It is created by [`X11Injector::focus`] and checked by the injector before and
/// after each event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusGuard {
    /// The client window that must hold the input focus.
    window: Window,
}

impl FocusGuard {
    /// The window this guard expects the focus on.
    pub fn window(&self) -> Window {
        self.window
    }

    /// Reads the server's focus and fails when it is no longer on the guarded window.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::FocusStolen`] when another window holds the focus, and
    /// [`TestError::Request`] when the server cannot answer. The refusal carries whether
    /// the window that took the focus is the candidate window, because that is the defect
    /// this project treats as its highest-severity one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn verify(&self, injector: &X11Injector) -> Result<(), TestError> {
        let actual = injector.session.input_focus()?;
        focus_held(self.window, actual, injector.is_candidate_window(actual))
    }
}

/// The verdict on the focus the server reported after a focus request.
///
/// The comparison is the contract and the request around it is not, so the decision is a
/// function of its own: the refusal path is covered without a display server, and the
/// caller that only has a server to offer still gets exactly one place where "the focus is
/// not where the test put it" is turned into a failure.
///
/// # Errors
///
/// Returns [`TestError::FocusRefused`] when the server kept a different window focused.
fn focused(expected: Window, actual: Window) -> Result<(), TestError> {
    if actual == expected {
        return Ok(());
    }
    Err(TestError::FocusRefused { expected, actual })
}

/// The verdict on the focus the server reported while events were being injected.
///
/// `by_candidate_window` says whether the window that took the focus carries the candidate
/// window's class. It is the one detail that separates the project's highest-severity
/// defect from a harness aimed at the wrong window, so it is carried into the refusal
/// rather than looked up by whoever reads the failure.
///
/// # Errors
///
/// Returns [`TestError::FocusStolen`] when another window holds the focus.
fn focus_held(
    expected: Window,
    actual: Window,
    by_candidate_window: bool,
) -> Result<(), TestError> {
    if actual == expected {
        return Ok(());
    }
    Err(TestError::FocusStolen {
        expected,
        actual,
        by_candidate_window,
    })
}

/// The wheel button one notch stands for.
///
/// X11 has no wheel: buttons 4 and 5 are the vertical pair, and the sign matches the axis
/// event the UI thread consumes -- forward is positive and is button 5. A delta of zero
/// still answers with the forward button; the caller sends nothing in that case.
fn wheel_button(delta: i32) -> u8 {
    if delta < 0 { WHEEL_UP } else { WHEEL_DOWN }
}

/// How many press/release pairs a wheel request stands for, clamped to the cap.
///
/// A test scrolls one to three notches. The clamp is what keeps a mistyped delta from
/// spinning out an unbounded run of events, and it is applied to the magnitude only, so
/// the direction a request asked for is never reversed.
fn notches(delta: i32) -> u32 {
    delta.unsigned_abs().min(MAX_NOTCHES)
}

#[cfg(test)]
mod tests;
