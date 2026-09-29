//! The live X11 session: connection, XTEST probe, and the requests the channel sends.
//!
//! Responsibility: hold one connection, prove the server can inject at all, and expose the
//! handful of requests the injection path needs. The tables it reads -- keysyms, the
//! keyboard mapping, coordinates -- live in [`super::keys`] and [`super::coords`], which is
//! what lets their tests run without a display server.
//!
//! Boundaries: this module never decides whether a test passed. It reports what the server
//! said, as a [`TestError`] the caller turns into a verdict.

use std::fmt::Display;
use std::os::fd::{AsRawFd, RawFd};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as XprotoExt, InputFocus};
use x11rb::protocol::xtest::ConnectionExt as XtestExt;
use x11rb::rust_connection::RustConnection;

pub use x11rb::protocol::xproto::Window;

use super::TestError;
use super::coords::{WindowPlacement, dp_to_px};
use super::keys::{Keymap, pressed_modifiers};

/// The XTEST `FakeInput` event types, from the extension's specification.
const KEY_PRESS: u8 = 2;
/// See [`KEY_PRESS`].
const KEY_RELEASE: u8 = 3;
/// See [`KEY_PRESS`].
const BUTTON_PRESS: u8 = 4;
/// See [`KEY_PRESS`].
const BUTTON_RELEASE: u8 = 5;
/// See [`KEY_PRESS`].
const MOTION_NOTIFY: u8 = 6;

/// The `FakeInput` detail that asks for an absolute pointer position.
const MOTION_ABSOLUTE: u8 = 0;

/// The core keyboard and pointer, the only device this channel drives.
const CORE_DEVICE: u8 = 0;

/// The X11 `None` id, used where a request field is ignored.
const NO_WINDOW: Window = 0;

/// The timestamp that tells the server to use its own current time.
const CURRENT_TIME: u32 = 0;

/// Bytes of `WM_CLASS` the harness is willing to read.
const WM_CLASS_BYTES: u32 = 256;

/// The XTEST major version the channel needs.
const XTEST_MAJOR: u8 = 2;

/// The XTEST minor version the channel asks for.
const XTEST_MINOR: u8 = 2;

/// The extension name the probe asks the server for.
const XTEST_EXTENSION: &str = "XTEST";

/// The XTEST version a server reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XtestVersion {
    /// Major version; the channel needs 2.
    pub major: u8,
    /// Minor version.
    pub minor: u8,
}

/// One live X session, opened for injection.
pub struct X11Session {
    /// The connection every request goes out on.
    conn: RustConnection,
    /// The root window: the origin of screen coordinates.
    root: Window,
    /// The screen size in physical pixels, which the coordinate assertions use.
    screen: (u16, u16),
    /// The keyboard mapping, read once at connect time.
    keymap: Keymap,
    /// What the XTEST probe answered.
    xtest: XtestVersion,
    /// The display name, kept for diagnostics.
    display: String,
}

impl X11Session {
    /// Opens a connection, probes XTEST, and reads the keyboard mapping.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::NoDisplay`] when the display name cannot be parsed or the
    /// connection fails, and [`TestError::NoXtest`] when the server does not offer a usable
    /// XTEST extension. The XTEST failure is a refusal, never a silent fallback to another
    /// injection channel: the caller decides whether to use one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn connect(display: Option<&str>) -> Result<Self, TestError> {
        let name = display
            .map(str::to_owned)
            .or_else(|| std::env::var("DISPLAY").ok())
            .unwrap_or_else(|| "$DISPLAY".to_owned());
        let (conn, screen_num) = x11rb::connect(display).map_err(|error| TestError::NoDisplay {
            display: name.clone(),
            detail: error.to_string(),
        })?;
        // The screen is read out of the setup before the connection is moved into the
        // session: the borrow of the setup ends here, and the session owns the connection.
        let (root, screen) = {
            let screen = conn
                .setup()
                .roots
                .get(screen_num)
                .ok_or_else(|| TestError::NoDisplay {
                    display: name.clone(),
                    detail: format!("the server lists no screen {screen_num}"),
                })?;
            (screen.root, (screen.width_in_pixels, screen.height_in_pixels))
        };
        let xtest = probe_xtest(&conn, &name)?;
        let keymap = Keymap::load(&conn)?;
        Ok(Self {
            conn,
            root,
            screen,
            keymap,
            xtest,
            display: name,
        })
    }

    /// The connection, for the requests this channel does not wrap itself.
    ///
    /// The screenshot and probe work of the later test-platform tasks reads the same
    /// connection, and opening a second one would put the two out of step about what the
    /// server has already processed.
    pub fn connection(&self) -> &RustConnection {
        &self.conn
    }

    /// The root window.
    pub fn root(&self) -> Window {
        self.root
    }

    /// The file descriptor of the connection, for a `poll(2)` loop.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn connection_fd(&self) -> RawFd {
        self.connection().stream().as_raw_fd()
    }

    /// The screen size in physical pixels.
    pub fn screen(&self) -> (u16, u16) {
        self.screen
    }

    /// The XTEST version the server reported.
    pub fn xtest(&self) -> XtestVersion {
        self.xtest
    }

    /// The display name, for diagnostics.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The keycode that produces `keysym`.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::NoKeycode`] when no row of the keyboard mapping carries the
    /// keysym, which means the key does not exist on this server's layout.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn keycode(&self, keysym: u32) -> Result<u8, TestError> {
        self.keymap
            .keycode(keysym)
            .ok_or(TestError::NoKeycode { keysym })
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
        let reply = self
            .conn
            .get_input_focus()
            .map_err(request)?
            .reply()
            .map_err(request)?;
        Ok(reply.focus)
    }

    /// Moves the input focus to `window` and waits for the server's verdict.
    ///
    /// The request is checked rather than merely queued, so a window that no longer exists
    /// is reported here instead of leaving the focus where it was and letting the following
    /// keys travel to whatever held it before.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the server rejects the request or the connection
    /// fails.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_input_focus(&self, window: Window) -> Result<(), TestError> {
        self.conn
            .set_input_focus(InputFocus::Parent, window, CURRENT_TIME)
            .map_err(request)?
            .check()
            .map_err(request)
    }

    /// The modifier keycodes the server reports as physically held.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the query cannot be answered.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn held_modifiers(&self) -> Result<Vec<u8>, TestError> {
        let reply = self
            .conn
            .query_keymap()
            .map_err(request)?
            .reply()
            .map_err(request)?;
        Ok(pressed_modifiers(&reply.keys, &self.keymap))
    }

    /// Sends one key press or release for `keycode`.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the request cannot be delivered.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn fake_key(&self, keycode: u8, press: bool) -> Result<(), TestError> {
        let kind = if press { KEY_PRESS } else { KEY_RELEASE };
        self.fake(kind, keycode, NO_WINDOW, (0, 0))
    }

    /// Sends one button press or release at the pointer's current position.
    ///
    /// X11 has no wheel and no button coordinates: a button event happens wherever the
    /// pointer is, so a caller that cares about the position moves it first.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the request cannot be delivered.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn fake_button(&self, button: u8, press: bool) -> Result<(), TestError> {
        let kind = if press { BUTTON_PRESS } else { BUTTON_RELEASE };
        self.fake(kind, button, NO_WINDOW, (0, 0))
    }

    /// Moves the pointer to an absolute screen position.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the request cannot be delivered.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn fake_motion(&self, point: (i16, i16)) -> Result<(), TestError> {
        self.fake(MOTION_NOTIFY, MOTION_ABSOLUTE, self.root, point)
    }

    /// Sends one `FakeInput` request and pushes it to the server.
    ///
    /// The request is flushed rather than checked: a round trip per event would pace the
    /// injection by the harness instead of by the test's own delay, and the only error a
    /// well-formed `FakeInput` can raise is one the next request would report anyway.
    fn fake(&self, kind: u8, detail: u8, root: Window, point: (i16, i16)) -> Result<(), TestError> {
        self.conn
            .xtest_fake_input(kind, detail, CURRENT_TIME, root, point.0, point.1, CORE_DEVICE)
            .map_err(request)?;
        self.conn.flush().map_err(request)
    }

    /// Reads where a window sits on the screen.
    ///
    /// The position comes from the server rather than from a recorded `move_to` call,
    /// because the candidate window lives in the host's process and this channel cannot see
    /// that call. The container offset is the shadow reserve, which is a design constant; a
    /// caller that has the probe's exported geometry should build a [`WindowPlacement`]
    /// from it instead and get the exact offset.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the geometry or the translation cannot be
    /// answered, which is also what a window that has gone away reports.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn window_placement(
        &self,
        window: Window,
        shadow_dp: u16,
        scale: f32,
    ) -> Result<WindowPlacement, TestError> {
        let geometry = self
            .conn
            .get_geometry(window)
            .map_err(request)?
            .reply()
            .map_err(request)?;
        let translated = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .map_err(request)?
            .reply()
            .map_err(request)?;
        let shadow = dp_to_px(i32::from(shadow_dp), scale);
        Ok(WindowPlacement {
            origin: (i32::from(translated.dst_x), i32::from(translated.dst_y)),
            size: (u32::from(geometry.width), u32::from(geometry.height)),
            container_offset: (shadow, shadow),
            scale,
        })
    }

    /// The `WM_CLASS` of a window, or `None` when it has none or cannot be read.
    ///
    /// The answer is only ever used to sharpen a failure message, so a window that has gone
    /// away between the focus check and the question must not become an error.
    pub fn window_class(&self, window: Window) -> Option<Vec<u8>> {
        let reply = self
            .conn
            .get_property(
                false,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                0,
                WM_CLASS_BYTES / 4,
            )
            .ok()?
            .reply()
            .ok()?;
        if reply.value.is_empty() {
            None
        } else {
            Some(reply.value)
        }
    }
}

/// Probes the XTEST extension.
///
/// # Errors
///
/// Returns [`TestError::NoXtest`] when the server has no XTEST extension, or one older than
/// the version the channel needs, and [`TestError::Request`] for any other failure.
///
/// # Panics
///
/// Never.
fn probe_xtest(conn: &RustConnection, display: &str) -> Result<XtestVersion, TestError> {
    let refused = || TestError::NoXtest {
        display: display.to_owned(),
    };
    // The extension table is asked first: this is what names the reason directly, instead
    // of leaving the caller to read an unsupported-extension error out of a request.
    let known = conn
        .extension_information(XTEST_EXTENSION)
        .map_err(request)?
        .is_some();
    if !known {
        return Err(refused());
    }
    let reply = match conn.xtest_get_version(XTEST_MAJOR, XTEST_MINOR) {
        Ok(cookie) => cookie.reply().map_err(request)?,
        Err(error) => return Err(request(error)),
    };
    if reply.major_version < XTEST_MAJOR {
        return Err(refused());
    }
    Ok(XtestVersion {
        major: reply.major_version,
        minor: reply.minor_version,
    })
}

/// Maps an X11 failure onto the channel's error vocabulary.
pub(super) fn request<E: Display>(error: E) -> TestError {
    TestError::Request {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::keys::{KS_SHIFT_L, KS_TAB};
    use super::*;

    #[test]
    #[ignore = "needs a live X server with XTEST; the lab job runs it with DISPLAY set"]
    fn test_session_connects_probes_xtest_and_reads_the_keymap() {
        let session = X11Session::connect(None).expect("a live X server with XTEST on $DISPLAY");
        let (width, height) = session.screen();
        assert!(width > 0 && height > 0, "a screen has a size");
        assert_eq!(session.xtest().major, XTEST_MAJOR, "the probe answered");
        assert!(session.keycode(KS_SHIFT_L).is_ok(), "the layout has a shift key");
        assert!(session.keycode(KS_TAB).is_ok(), "the layout has a tab key");
        assert!(
            session.input_focus().is_ok(),
            "the server answers a focus query"
        );
    }
}
