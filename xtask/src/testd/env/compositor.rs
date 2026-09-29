//! Which compositor this session runs, and which of the four Wayland tiers it can serve.
//!
//! Responsibility: name the compositor a session is running from the process names that are
//! live, and decide from that name and the `wl_registry` listing which of the four
//! positioning tiers applies. Boundaries: it reads no `/proc` and no protocol itself. The
//! process names arrive as strings and the registry arrives as data, so every rule here is a
//! pure function and is tested without a compositor present.
//!
//! # Why the process table names the compositor
//!
//! Wayland has no request that answers "who are you": a client can learn which *interfaces*
//! a compositor offers and nothing more, so the family is inferred from what the registry
//! announces -- `zwlr_layer_shell_v1` exists only in the wlroots family. That inference is
//! what the tier ladder starts from, and it is also what `ASM-13` asks to be reported when
//! the answer is a compositor outside the four tiers. The process name is a second, coarser
//! reading of the same fact, and it is the one the report quotes.
//!
//! # The table is the registered baseline
//!
//! The names below are the compositors `features.md` 0.5.5 registers and 2.5.2 maps to tiers,
//! plus the X11 compositors that `features.md` 0.5.2 names as the source of the acrylic
//! background. A compositor that is not in the table is not guessed at: the report says the
//! compositor is unknown and records a gap.

use super::{DisplayServer, WaylandTier};

/// The compositor families the four-tier table names, plus the ones it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorFamily {
    /// A wlroots-family compositor, whose row in the table is T1.
    Wlroots,
    /// KWin, whose row in the table is T2.
    KWin,
    /// Mutter, whose row in the table is T3.
    Mutter,
    /// A compositor outside the four registered tiers, which `ASM-13` names as the case to
    /// detect and report.
    Other,
}

/// A compositor the probe recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compositor {
    /// The name the report carries, e.g. `weston`.
    pub name: &'static str,
    /// The family that decides its tier.
    pub family: CompositorFamily,
}

/// One compositor the table recognises.
struct Known {
    /// The process name the kernel reports for it.
    process: &'static str,
    /// The name the report carries.
    name: &'static str,
    /// The family that decides its tier.
    family: CompositorFamily,
    /// The display server it is a compositor for.
    server: DisplayServer,
}

/// The compositors the probe recognises, in the order it checks them.
///
/// The shell and the compositor library it is built on share a name here (`gnome-shell` and
/// `mutter` are one tier), because the table maps a session to a tier and not a process to
/// one. The two X11 entries for KWin and its fellow compositors exist so that an X11 session
/// is named by the compositor it actually runs rather than by the Wayland one on the same
/// machine.
const KNOWN: [Known; 14] = [
    Known {
        process: "sway",
        name: "sway",
        family: CompositorFamily::Wlroots,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "hyprland",
        name: "hyprland",
        family: CompositorFamily::Wlroots,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "labwc",
        name: "labwc",
        family: CompositorFamily::Wlroots,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "river",
        name: "river",
        family: CompositorFamily::Wlroots,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "niri",
        name: "niri",
        family: CompositorFamily::Wlroots,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "kwin_wayland",
        name: "kwin",
        family: CompositorFamily::KWin,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "gnome-shell",
        name: "mutter",
        family: CompositorFamily::Mutter,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "mutter",
        name: "mutter",
        family: CompositorFamily::Mutter,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "weston",
        name: "weston",
        family: CompositorFamily::Other,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "cosmic-comp",
        name: "cosmic-comp",
        family: CompositorFamily::Other,
        server: DisplayServer::Wayland,
    },
    Known {
        process: "kwin_x11",
        name: "kwin",
        family: CompositorFamily::KWin,
        server: DisplayServer::X11,
    },
    Known {
        process: "picom",
        name: "picom",
        family: CompositorFamily::Other,
        server: DisplayServer::X11,
    },
    Known {
        process: "xcompmgr",
        name: "xcompmgr",
        family: CompositorFamily::Other,
        server: DisplayServer::X11,
    },
    Known {
        process: "compton",
        name: "compton",
        family: CompositorFamily::Other,
        server: DisplayServer::X11,
    },
];

/// The compositor of this session, from the process names that are running.
///
/// Only compositors of the session's own display server are considered: a machine running a
/// Wayland compositor and an X11 session is two sessions, and the report is about the one the
/// plugin would run in. `None` means no compositor the table knows is running, which is a
/// real answer on X11 -- WSLg provides no X11 compositor -- and a gap on Wayland.
///
/// # Panics
///
/// Never.
pub fn identify(processes: &[&str], server: DisplayServer) -> Option<Compositor> {
    KNOWN
        .iter()
        .find(|known| known.server == server && processes.contains(&known.process))
        .map(|known| Compositor {
            name: known.name,
            family: known.family,
        })
}

/// What the `wl_registry` listing said about the interfaces the tiers need.
///
/// `xtask` links no Wayland client, so nothing fills this in today: the reader that would is
/// a later task's, and the fields exist so that the tier decision is a function of the
/// listing rather than of a guess. A `None` field means the listing was not read, which is
/// not the same as "the interface is absent".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegistryFacts {
    /// Whether `zwlr_layer_shell_v1` is among the globals, which is what T1 needs.
    pub layer_shell: Option<bool>,
    /// Whether `xdg_wm_base` is among them, which is what the two popup tiers need.
    pub xdg_wm_base: Option<bool>,
}

impl RegistryFacts {
    /// Whether the listing was read at all.
    ///
    /// A half-read listing counts as unread: the tier decision needs both interfaces, and a
    /// single field is not enough to choose between the tiers.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn was_read(&self) -> bool {
        self.layer_shell.is_some() && self.xdg_wm_base.is_some()
    }
}

/// Which tier the ladder should start at.
///
/// The listing is direct evidence and the table is inference, so the listing wins wherever it
/// was read. Where it was not, the table decides for KWin and Mutter -- the four-tier table
/// maps a compositor to a tier, and those two rows do not depend on the listing -- but not
/// for wlroots, where T1 and T2 are separated by a global that only the listing names.
///
/// # Panics
///
/// Never.
pub fn tier_for(family: CompositorFamily, registry: RegistryFacts) -> WaylandTier {
    // A half-read listing is treated as unread rather than as a negative answer.
    let (layer_shell, xdg_wm_base) = if registry.was_read() {
        (registry.layer_shell, registry.xdg_wm_base)
    } else {
        (None, None)
    };
    match (family, layer_shell, xdg_wm_base) {
        // A layer-shell global outranks the table: it is the only pixel-exact tier, and the
        // table's own wlroots row says the same thing.
        (_, Some(true), _) => WaylandTier::LayerShell,
        // Neither interface is on offer, so no surface of any tier can be built.
        (_, Some(false), Some(false)) => WaylandTier::Fallback,
        (CompositorFamily::Wlroots, Some(false), _) => WaylandTier::Popup,
        (CompositorFamily::KWin, _, _) => WaylandTier::Popup,
        (CompositorFamily::Mutter, _, _) => WaylandTier::CanvasPopup,
        // Without the listing, one wlroots compositor cannot be told from another: T1 needs
        // the layer-shell global and T2 needs xdg-shell, and nothing else says which is there.
        (CompositorFamily::Wlroots, None, _) => WaylandTier::NotApplicable,
        // `ASM-13`'s case: a compositor outside the four registered tiers.
        (CompositorFamily::Other, _, _) => WaylandTier::NotApplicable,
    }
}

/// Whether `zwlr_layer_shell_v1` is available.
///
/// The listing answers it when it was read. When it was not, the family is the evidence: the
/// interface is a wlroots one, so a wlroots-family compositor offers it and no other
/// compositor does -- which is what `features.md` 0.5.5 registers for the development machine
/// (`wlr_layer_shell = false`, under Weston).
///
/// # Panics
///
/// Never.
pub fn layer_shell_for(family: CompositorFamily, registry: RegistryFacts) -> bool {
    registry
        .layer_shell
        .unwrap_or(matches!(family, CompositorFamily::Wlroots))
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows. A `#[path]` child module keeps them inside `compositor`, which
// is where the table's private shape is reachable.
#[cfg(test)]
#[path = "compositor/tests.rs"]
mod tests;
