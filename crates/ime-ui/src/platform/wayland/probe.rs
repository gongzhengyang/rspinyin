//! Tier probing: which of the four window backends this compositor can serve, and what to do
//! when the answer is no.
//!
//! Responsibility: read the registry's global list, decide which tier to try first, and run
//! the ladder that confirms it -- a configure must arrive within [`CONFIGURE_TIMEOUT`], and a
//! tier that does not produce one, or that is refused, hands over to the next. Boundaries:
//! nothing here owns a connection or a clock. The globals arrive as plain data and the
//! passage of time arrives as a `Duration` the caller measured, so the ladder is a function
//! of its inputs and the UI thread's `timerfd` stays the only timer in the process.
//!
//! # What can be inferred about a compositor, and what cannot
//!
//! Wayland has no request that names the compositor. The family is therefore inferred from
//! which interfaces the registry announces -- `zwlr_layer_shell_v1` exists only in the
//! wlroots family -- and the diagnostics report that inference rather than a name. `ASM-13`
//! treats a compositor outside the four tiers as a case to detect and report, which is what
//! [`UNSUPPORTED_CODE`] is for: the ladder falls through to T4 and the caller records it.

use std::time::Duration;

use super::Tier;
use super::events::WireEvent;

/// The wlroots layer-shell interface; the only global that selects the first tier.
pub const LAYER_SHELL_INTERFACE: &str = "zwlr_layer_shell_v1";

/// The xdg-shell interface the popup tiers need.
pub const XDG_WM_BASE_INTERFACE: &str = "xdg_wm_base";

/// The compositor interface, whose version decides whether the input region can be shaped.
pub const COMPOSITOR_INTERFACE: &str = "wl_compositor";

/// The shared-memory interface every tier draws through.
pub const SHM_INTERFACE: &str = "wl_shm";

/// The lowest `wl_compositor` version with `set_input_region`.
pub const MIN_INPUT_REGION_VERSION: u32 = 4;

/// How long one tier attempt is given to produce a configure before it is judged unusable.
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_millis(300);

/// Recorded when the ladder ends at T4 and the host's own candidate list takes over.
///
/// `ASM-13` names this code for a compositor outside the four tiers; it is recorded once, at
/// the transition, rather than once per frame.
pub const UNSUPPORTED_CODE: &str = "platform/compositor/unsupported";

/// One global object the registry announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Global {
    /// The registry's numeric name for the object.
    pub name: u32,
    /// The interface name, e.g. `zwlr_layer_shell_v1`.
    pub interface: String,
    /// The version the compositor offers.
    pub version: u32,
}

impl Global {
    /// Builds one registry entry.
    pub fn new(name: u32, interface: impl Into<String>, version: u32) -> Self {
        Self {
            name,
            interface: interface.into(),
            version,
        }
    }
}

/// The protocol capabilities this backend needs, reduced from the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// `zwlr_layer_shell_v1` is present, so T1 can be tried.
    pub layer_shell: bool,
    /// `xdg_wm_base` is present, so the popup tiers can be tried.
    pub xdg_wm_base: bool,
    /// `wl_compositor` is at least version [`MIN_INPUT_REGION_VERSION`], so the interactive
    /// region can be shaped. Without it the whole surface takes input, which is a degradation
    /// rather than a failure: the window is small, so the worst case is its shadow reserve
    /// swallowing clicks near the edge.
    pub input_region: bool,
    /// `wl_shm` is present, without which there is nothing to draw into.
    pub shm: bool,
}

impl Capabilities {
    /// Reduces a registry listing to the capabilities that matter.
    pub fn from_globals(globals: &[Global]) -> Self {
        let version_of = |interface: &str| {
            globals
                .iter()
                .find(|global| global.interface == interface)
                .map(|global| global.version)
        };
        Self {
            layer_shell: version_of(LAYER_SHELL_INTERFACE).is_some(),
            xdg_wm_base: version_of(XDG_WM_BASE_INTERFACE).is_some(),
            input_region: version_of(COMPOSITOR_INTERFACE)
                .is_some_and(|version| version >= MIN_INPUT_REGION_VERSION),
            shm: version_of(SHM_INTERFACE).is_some(),
        }
    }

    /// Whether any tier can run at all.
    pub fn usable(&self) -> bool {
        self.shm && (self.layer_shell || self.xdg_wm_base)
    }
}

/// The compositor family, inferred from the registry.
///
/// The inference is coarse on purpose: `zwlr_layer_shell_v1` marks the wlroots family, and
/// every other compositor that speaks xdg-shell lands in one bucket. That is enough to report
/// `ASM-13`'s case -- a compositor outside the four tiers -- without pretending to know more
/// than the registry says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorKind {
    /// A wlroots-family compositor: Sway, Hyprland, labwc, river, niri.
    WlrootsFamily,
    /// A compositor that speaks plain xdg-shell: KWin, Mutter, Weston, COSMIC.
    XdgShellOnly,
    /// Neither interface is present, so this is not a usable Wayland session for this backend.
    Unsupported,
}

impl CompositorKind {
    /// Infers the family from what the registry announced.
    pub fn from_capabilities(capabilities: &Capabilities) -> Self {
        if capabilities.layer_shell {
            Self::WlrootsFamily
        } else if capabilities.xdg_wm_base {
            Self::XdgShellOnly
        } else {
            Self::Unsupported
        }
    }

    /// The label the diagnostics carry.
    pub fn label(self) -> &'static str {
        match self {
            Self::WlrootsFamily => "wlroots",
            Self::XdgShellOnly => "xdg-shell",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Why a tier was given up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierFailure {
    /// The interface the tier needs is not in the registry.
    ProtocolMissing,
    /// No configure arrived within [`CONFIGURE_TIMEOUT`].
    ConfigureTimeout,
    /// The compositor dismissed the popup before configuring it.
    PopupDone,
    /// The compositor closed the layer surface.
    Closed,
    /// The compositor raised a protocol error.
    ProtocolError,
    /// This surface was given the keyboard, which a candidate window must never hold.
    FocusTaken,
}

impl TierFailure {
    /// The diagnostic code recorded for this failure.
    ///
    /// Each is recorded once, at the transition, by the caller that owns the diagnostics sink.
    pub fn code(self) -> &'static str {
        match self {
            Self::ProtocolMissing => "platform/wayland/protocol-missing",
            Self::ConfigureTimeout => "platform/wayland/configure-timeout",
            Self::PopupDone => "platform/wayland/popup-done",
            Self::Closed => "platform/wayland/layer-closed",
            Self::ProtocolError => "platform/wayland/protocol-error",
            Self::FocusTaken => "platform/wayland/focus-taken",
        }
    }
}

/// One tier that was tried and given up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierAttempt {
    /// The tier that failed.
    pub tier: Tier,
    /// Why it was abandoned.
    pub failure: TierFailure,
}

/// What the ladder did with one input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LadderStep {
    /// The current tier is still being tried, or is confirmed and running.
    Pending,
    /// The tier produced a configure and is in use.
    Confirmed(Tier),
    /// The tier failed; the caller must rebuild the surface for `next`.
    Promote {
        /// The tier that failed.
        failed: Tier,
        /// Why it failed.
        reason: TierFailure,
        /// The tier to try next.
        next: Tier,
    },
    /// Every tier failed; the caller must not register a user interface at all.
    Fallback {
        /// Why the last tier failed.
        reason: TierFailure,
    },
}

/// Picks the tier to try first, in the order the design fixes.
///
/// T1 when the compositor speaks layer-shell, T2 when it speaks xdg-shell, and the fallback
/// when neither is on offer. T2 is a starting point rather than a conclusion: an xdg-shell
/// popup is a request a compositor may refuse, so the ladder confirms it with a configure
/// before the tier counts as adopted.
pub fn probe_tier(capabilities: &Capabilities) -> Tier {
    if !capabilities.usable() {
        return Tier::Fallback;
    }
    if capabilities.layer_shell {
        Tier::LayerShell
    } else if capabilities.xdg_wm_base {
        Tier::Popup
    } else {
        Tier::Fallback
    }
}

/// The descending ladder: one tier at a time, each on a [`CONFIGURE_TIMEOUT`] budget.
///
/// The ladder takes the passage of time as an argument rather than reading a clock, so a test
/// drives it in microseconds and the UI thread drives it from the elapsed time it already
/// measures for its `timerfd`.
#[derive(Clone, Debug)]
pub struct TierLadder {
    capabilities: Capabilities,
    tier: Tier,
    confirmed: bool,
    /// When the current attempt expires; `None` until the surface has been created and the
    /// budget starts running.
    deadline: Option<Duration>,
    failures: Vec<TierAttempt>,
}

impl TierLadder {
    /// Starts the ladder at the tier [`probe_tier`] picks.
    ///
    /// The budget does not start until [`TierLadder::arm`] is called, because until the
    /// surface exists there is nothing for a configure to configure.
    pub fn begin(capabilities: Capabilities) -> Self {
        Self {
            capabilities,
            tier: probe_tier(&capabilities),
            confirmed: false,
            deadline: None,
            failures: Vec::new(),
        }
    }

    /// Starts the budget for the current attempt, `now` being the caller's elapsed time.
    pub fn arm(&mut self, now: Duration) {
        if self.tier == Tier::Fallback {
            return;
        }
        self.deadline = Some(now.saturating_add(CONFIGURE_TIMEOUT));
    }

    /// The tier currently in use, or [`Tier::Fallback`] when none is.
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Whether the current tier has produced a configure.
    pub fn is_confirmed(&self) -> bool {
        self.confirmed
    }

    /// The tiers tried and abandoned, in order.
    pub fn attempts(&self) -> &[TierAttempt] {
        &self.failures
    }

    /// When the current attempt expires, as the caller's elapsed time.
    ///
    /// This is what the UI thread arms its `timerfd` with; there is no other timer.
    pub fn deadline(&self) -> Option<Duration> {
        if self.confirmed {
            None
        } else {
            self.deadline
        }
    }

    /// Feeds one protocol event.
    ///
    /// `now` is the caller's elapsed time, used when the event ends the current attempt.
    pub fn on_event(&mut self, event: &WireEvent, now: Duration) -> LadderStep {
        if self.tier == Tier::Fallback {
            return LadderStep::Pending;
        }
        // Read once, so the arms below need no borrow of `self` in a guard.
        let confirmed = self.confirmed;
        match event {
            WireEvent::LayerConfigure { .. } | WireEvent::PopupConfigure { .. } => {
                self.confirmed = true;
                LadderStep::Confirmed(self.tier)
            }
            // A dismissal after the popup was confirmed is the normal end of a popup's life --
            // the user clicked somewhere else -- and the caller hides the window. Before the
            // configure, it is the compositor refusing the tier.
            WireEvent::PopupDone if !confirmed => self.fail(TierFailure::PopupDone, now),
            // Likewise, a closed layer surface before its first configure is a refusal; after
            // it, the output it was on has gone and the caller relocates.
            WireEvent::LayerClosed if !confirmed => self.fail(TierFailure::Closed, now),
            WireEvent::DisplayError => self.fail(TierFailure::ProtocolError, now),
            // Never acceptable, confirmed or not: a candidate window that holds the keyboard
            // swallows what the user is typing. Failing the tier costs the user a different
            // window backend; keeping it costs them their input.
            WireEvent::KeyboardEnter => self.fail(TierFailure::FocusTaken, now),
            _ => LadderStep::Pending,
        }
    }

    /// Feeds the passage of time.
    ///
    /// Returns [`LadderStep::Pending`] while the budget is unarmed, already spent, or the
    /// tier is confirmed.
    pub fn on_timeout(&mut self, now: Duration) -> LadderStep {
        if self.confirmed || self.tier == Tier::Fallback {
            return LadderStep::Pending;
        }
        let Some(deadline) = self.deadline else {
            return LadderStep::Pending;
        };
        if now < deadline {
            return LadderStep::Pending;
        }
        self.fail(TierFailure::ConfigureTimeout, now)
    }

    /// Records the failure, moves to the next tier, and reports what the caller must do.
    fn fail(&mut self, reason: TierFailure, now: Duration) -> LadderStep {
        let failed = self.tier;
        self.failures.push(TierAttempt {
            tier: failed,
            failure: reason,
        });
        let next = self.next_tier();
        self.tier = next;
        self.confirmed = false;
        self.deadline = None;
        // The next tier's surface does not exist yet, so its budget is armed here and the
        // configure is expected within the same 300 ms. `arm` is a no-op on the fallback.
        self.arm(now);
        if next == Tier::Fallback {
            LadderStep::Fallback { reason }
        } else {
            LadderStep::Promote {
                failed,
                reason,
                next,
            }
        }
    }

    /// The tier after the current one, skipping the popup tiers when xdg-shell is absent.
    fn next_tier(&self) -> Tier {
        match self.tier {
            Tier::LayerShell if self.capabilities.xdg_wm_base => Tier::Popup,
            Tier::Popup => Tier::CanvasPopup,
            _ => Tier::Fallback,
        }
    }
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows. A `#[path]` child module keeps them inside `probe`, which is
// where the ladder's private state is reachable.
#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
