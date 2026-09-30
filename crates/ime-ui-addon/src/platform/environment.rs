//! The display-server names a session offers, and the tier they imply.
//!
//! Responsibility: read the two variables that name a display server and answer which tier
//! the ladder starts at. The answer is a function of a value the caller supplies rather
//! than of the process, so every combination is reachable from a test and no test depends
//! on the environment it happens to run in.
//!
//! Boundaries: nothing here knows about backends, windows or pixels, and nothing here
//! connects to anything. Constructing the surface is `super::probe`'s job.

use std::env;

/// The tier the platform ladder starts at.
///
/// The order is the ladder's rather than a preference. X11 comes first because it is a
/// first-class tier (`ASM-13`) and because an X server is exactly what an XWayland session
/// offers alongside Wayland; a session that names only Wayland is a Wayland session; one
/// that names neither has no window system at all, which is the fallback tier `T4` where
/// the host keeps drawing the candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTier {
    /// `$DISPLAY` names an X server.
    X11,
    /// Only `$WAYLAND_DISPLAY` names a display server.
    Wayland,
    /// Neither variable names one.
    None,
}

impl SessionTier {
    /// The tier's name, as the `tier=` field of a diagnostic.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn name(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Wayland => "wayland",
            Self::None => "none",
        }
    }
}

/// The display-server names the session offers.
///
/// The two names are held as the strings they arrived as, so a diagnostic can report the
/// session's own spelling and the probe can hand `$DISPLAY` to the X client unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// `$DISPLAY`, when it names a server.
    display: Option<String>,
    /// `$WAYLAND_DISPLAY`, when it names one.
    wayland_display: Option<String>,
}

impl Environment {
    /// Reads `$DISPLAY` and `$WAYLAND_DISPLAY` from the process environment.
    ///
    /// A variable set to an empty string counts as unset: an empty name reaches no server,
    /// and reporting it as a name would make the probe fail with the wrong reason.
    ///
    /// # Panics
    ///
    /// Never panics. A variable that is not valid UTF-8 counts as unset, which is the same
    /// answer as a name no server can be reached through.
    pub fn from_process() -> Self {
        Self::new(
            env::var("DISPLAY").ok().as_deref(),
            env::var("WAYLAND_DISPLAY").ok().as_deref(),
        )
    }

    /// Builds an environment from the two names.
    ///
    /// An empty name is treated exactly as `None`, which is what [`Self::from_process`]
    /// does with an empty variable.
    ///
    /// # Parameters
    ///
    /// * `display` -- the X server name, or `None` when the session names none.
    /// * `wayland_display` -- the Wayland display name, or `None`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(display: Option<&str>, wayland_display: Option<&str>) -> Self {
        Self {
            display: named(display),
            wayland_display: named(wayland_display),
        }
    }

    /// The X server name, or `None` when the session names none.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn display(&self) -> Option<&str> {
        self.display.as_deref()
    }

    /// The Wayland display name, or `None` when the session names none.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn wayland_display(&self) -> Option<&str> {
        self.wayland_display.as_deref()
    }

    /// The tier the ladder starts at.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn tier(&self) -> SessionTier {
        if self.display.is_some() {
            SessionTier::X11
        } else if self.wayland_display.is_some() {
            SessionTier::Wayland
        } else {
            SessionTier::None
        }
    }
}

/// Keeps a name a server could be reached through, and drops one it could not.
fn named(name: Option<&str>) -> Option<String> {
    name.filter(|name| !name.is_empty()).map(String::from)
}
