//! What the machine said, before any of it is interpreted.
//!
//! Responsibility: read the environment into one record -- `$DISPLAY`, `$WAYLAND_DISPLAY`,
//! the X server, `/proc`, `pkg-config`, `fc-list`, the installed addon descriptors and the
//! plugin's data directory -- and hand it over. Boundaries: it interprets nothing. Every
//! judgement lives in [`super::EnvCapabilities::from_observation`] and the helpers beside
//! it, which take this record as an argument, and that is what lets the whole report be
//! tested without a display server, a compositor, Fcitx5 or a process table.
//!
//! # Nothing here writes, and nothing here fails
//!
//! The probe reads permission bits instead of creating a file, and it inspects `/proc`
//! instead of signalling anything. A reading that cannot be taken -- no display, no
//! `fc-list`, no `/proc` -- stays `None` or empty and becomes a
//! [`ProbeGap`](super::ProbeGap) in the report, because "nobody could look" and "the answer
//! is no" are different answers and only one of them may pass a case.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ime_dict::paths::{BaseDirs, Paths};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, Screen, VisualClass};

use super::addons::{self, AddonDescriptor};
use super::compositor::RegistryFacts;
use super::process::{self, ProcessEntry};

/// `$XDG_RUNTIME_DIR`, where a Wayland socket named by a relative `$WAYLAND_DISPLAY` lives.
const RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";

/// The depth of the visual the transparent candidate window is drawn into.
const ARGB_DEPTH: u8 = 32;

/// The prefix of the selection a compositor owns on an X screen.
const COMPOSITOR_SELECTION: &str = "_NET_WM_CM_S";

/// The tool that answers what the Fcitx5 development package installs.
const PKG_CONFIG: &str = "pkg-config";

/// The `pkg-config` module that package publishes.
const FCITX5_PC_MODULE: &str = "Fcitx5Core";

/// The tool that lists fonts.
const FC_LIST: &str = "fc-list";

/// The fontconfig pattern that selects the fonts carrying Chinese.
const CJK_PATTERN: &str = ":lang=zh";

/// What the X server answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X11Facts {
    /// The server offers a 32-bit TrueColor visual with the `Argb8888` channel order.
    pub argb_visual: bool,
    /// A compositor owns `_NET_WM_CM_S<n>` for the screen the connection landed on.
    pub compositor_present: bool,
}

/// Everything the probe read, and nothing it concluded.
///
/// The fields are the readings themselves, in the shape the readers produce them, so that a
/// test -- or a later reader -- can name the ones it has.
/// [`Default`] is the state of having read nothing: every optional reading absent, the
/// registry unread, no addon descriptor and no process table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// `$DISPLAY`.
    pub display: Option<String>,
    /// `$WAYLAND_DISPLAY`.
    pub wayland_display: Option<String>,
    /// Whether `$WAYLAND_DISPLAY` names a socket that exists.
    pub wayland_socket: bool,
    /// What the X server answered, when one could be reached.
    pub x11: Option<X11Facts>,
    /// The process table, when `/proc` could be read.
    pub processes: Option<Vec<ProcessEntry>>,
    /// The probe's own process id, which the agent count excludes.
    pub own_pid: u32,
    /// The version `pkg-config` reports for the Fcitx5 development package.
    pub fcitx5_version: Option<String>,
    /// The output of `fc-list :lang=zh`.
    pub cjk_fonts: Option<String>,
    /// The installed addon descriptors, as their files were read.
    pub addon_descriptors: Vec<AddonDescriptor>,
    /// A Fcitx5 session log, when the caller brought one.
    pub session_log: Option<String>,
    /// The plugin's data directory.
    pub data_dir: Option<PathBuf>,
    /// Whether the data directory, or its nearest existing ancestor, is writable.
    pub data_dir_writable: bool,
    /// What the `wl_registry` listing said, when it was read at all.
    pub registry: RegistryFacts,
}

impl Observation {
    /// Reads the machine.
    ///
    /// Never fails and never panics: a reading that cannot be taken stays `None` or empty,
    /// and the report turns that into a gap. Both external tools are run without a shell, so
    /// nothing here depends on a command language being present.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn read() -> Self {
        let display = env_value("DISPLAY");
        let wayland_display = env_value("WAYLAND_DISPLAY");
        // The plugin's own base directories: the descriptor of a per-user addon lives under
        // the same XDG data home the plugin writes to.
        let bases = BaseDirs::from_env().ok();
        let data_dir = bases
            .as_ref()
            .and_then(|bases| Paths::from_bases(bases).ok())
            .map(|paths| paths.data_dir);
        Self {
            wayland_socket: wayland_display
                .as_deref()
                .and_then(|name| socket_path(name, runtime_dir().as_deref()))
                .is_some_and(|path| path.exists()),
            x11: x11_facts(display.as_deref()),
            processes: process::table(),
            own_pid: std::process::id(),
            fcitx5_version: pkg_config_version(),
            cjk_fonts: fc_list_zh(),
            addon_descriptors: addons::descriptors(bases.as_ref()),
            data_dir_writable: data_dir.as_deref().is_some_and(writable_dir),
            display,
            wayland_display,
            session_log: None,
            data_dir,
            registry: RegistryFacts::default(),
        }
    }
}

/// Counts the fonts a `fc-list :lang=zh` listing reports.
///
/// One line per font is the measure `features.md` 0.5.5 registers, so this is the same
/// number the baseline quotes. A listing that is empty, or that holds only blank lines, is
/// zero fonts rather than an error.
///
/// # Panics
///
/// Never.
pub fn count_cjk_fonts(listing: &str) -> u32 {
    listing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u32
}

/// Opens the X display and reads the two facts the report needs.
///
/// `None` when the display name cannot be parsed or the connection fails, which is also what
/// an `xtask` run with no display looks like. Nothing is created on the server: the
/// connection is opened, the setup is read, one selection owner is queried, and it is
/// dropped.
fn x11_facts(display: Option<&str>) -> Option<X11Facts> {
    let (connection, screen_number) = x11rb::connect(display).ok()?;
    // The setup is borrowed out of the connection, so the two readings that come from it are
    // taken inside this block and the borrow ends before the selection query below.
    let (argb_visual, selection) = {
        let screen = connection.setup().roots.get(screen_number)?;
        let argb_visual = screen_has_argb_visual(screen);
        let selection = format!("{COMPOSITOR_SELECTION}{screen_number}");
        (argb_visual, selection)
    };
    let atom = connection
        .intern_atom(false, selection.as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    let owner = connection
        .get_selection_owner(atom)
        .ok()?
        .reply()
        .ok()?
        .owner;
    Some(X11Facts {
        argb_visual,
        compositor_present: owner != 0,
    })
}

/// Whether the screen offers a visual an `Argb8888` buffer can be uploaded into.
///
/// This mirrors the renderer's own selection, which `xtask` cannot call because it does not
/// link the renderer; the same rule is written down twice on purpose, and the test below
/// pins the channel order both copies depend on.
fn screen_has_argb_visual(screen: &Screen) -> bool {
    let depth = match screen
        .allowed_depths
        .iter()
        .find(|depth| depth.depth == ARGB_DEPTH)
    {
        Some(depth) => depth,
        None => return false,
    };
    depth.visuals.iter().any(|visual| {
        is_argb_visual(
            depth.depth,
            visual.class,
            visual.red_mask,
            visual.green_mask,
            visual.blue_mask,
        )
    })
}

/// Whether one visual can carry an `Argb8888` buffer.
///
/// The masks are what matter, not the depth alone: a depth-32 visual with another channel
/// order would render blue and red swapped, which is worse than the opaque fallback the
/// caller then chooses.
fn is_argb_visual(depth: u8, class: VisualClass, red: u32, green: u32, blue: u32) -> bool {
    depth == ARGB_DEPTH
        && class == VisualClass::TRUE_COLOR
        && red == 0x00ff_0000
        && green == 0x0000_ff00
        && blue == 0x0000_00ff
}

/// The version `pkg-config` reports for the Fcitx5 development package.
///
/// `None` when `pkg-config` is missing, when it does not know the module, or when it exits
/// non-zero: all three mean the development package is not installed here, which is a
/// reading and not a failure.
fn pkg_config_version() -> Option<String> {
    let output = Command::new(PKG_CONFIG)
        .arg("--modversion")
        .arg(FCITX5_PC_MODULE)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// The output of `fc-list :lang=zh`, one line per font fontconfig matches.
fn fc_list_zh() -> Option<String> {
    let output = Command::new(FC_LIST).arg(CJK_PATTERN).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Where a `$WAYLAND_DISPLAY` name points, or `None` when a relative name has no runtime
/// directory to live in.
///
/// The name is either absolute or relative to `$XDG_RUNTIME_DIR`; a relative name with no
/// runtime directory resolves nowhere, and a session whose socket cannot be found is not a
/// Wayland session.
fn socket_path(wayland_display: &str, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let path = Path::new(wayland_display);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    runtime_dir.map(|dir| dir.join(path))
}

/// `$XDG_RUNTIME_DIR`, where a relative Wayland socket name lives.
fn runtime_dir() -> Option<PathBuf> {
    env_value(RUNTIME_DIR).map(PathBuf::from)
}

/// Whether `path`, or the nearest ancestor of it that exists, may be written to.
///
/// The permission bits are what is read: a probe must not create a file under the operator's
/// real `$XDG_DATA_HOME` to find out, which is the same red line the sandbox draws for every
/// case. A path that does not exist yet is judged by the directory that would hold it,
/// because that is the directory a `mkdir` would have to write to. Ownership, access control
/// lists and a read-only mount are not visible in the mode bits, so this is a reading and
/// not a guarantee.
fn writable_dir(path: &Path) -> bool {
    let mut candidate = Some(path);
    while let Some(current) = candidate {
        match fs::metadata(current) {
            Ok(metadata) if metadata.is_dir() => return !metadata.permissions().readonly(),
            // A file where a directory has to be: nothing below it can be created.
            Ok(_) => return false,
            Err(_) => candidate = current.parent(),
        }
    }
    false
}

/// An environment variable's value, with the empty value treated as unset: the XDG
/// specification gives an empty variable no meaning, and neither does a display name.
fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows. A `#[path]` child module keeps them inside `observation`.
#[cfg(test)]
#[path = "observation/tests.rs"]
mod tests;
