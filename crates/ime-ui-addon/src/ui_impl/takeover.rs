//! The candidate-window takeover: the policy, the request and its outcome.
//!
//! Responsibility: decide whether the host should be asked to route input-panel updates
//! to this plugin, ask it once, and report what happened. The decision is a pure function
//! over three flags so that every branch is reachable from a test; the request itself is
//! the only part that touches the host.
//!
//! Boundaries: this module owns the policy and the refusal it remembers, and it never
//! draws, decodes or captures anything. The flags it reads belong to
//! `super::availability`; the name of the previous user interface is reported rather than
//! logged, because it is the value a restore needs.
//!
//! # Threading
//!
//! [`register_takeover`] reaches the host, so it belongs on the Fcitx5 host thread.
//! Asking is `UserInterfaceManager::updateAvailability()`, which walks every
//! user-interface addon, calls `available()` on each one and suspends or resumes the one
//! it picks; none of that may run from a worker thread while the main loop is inside the
//! same manager. A caller whose state changed on another thread -- the UI start-up
//! reporting the window ready, a platform probe answering -- has to get back onto the
//! host loop before calling this.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::addon::{UI_NOT_READY_CODE, candidate_window_ready};
use crate::ffi::{UiActivation, activate_ui, current_ui};

use super::availability::window_backend_available;

/// Diagnostic code recorded when the host routes input-panel updates to this plugin.
pub const TAKEOVER_ACTIVE_CODE: &str = "ui/takeover/active";

/// Diagnostic code recorded when the host has no user interface of this plugin to
/// switch to.
pub const TAKEOVER_NOT_REGISTERED_CODE: &str = "ui/takeover/not-registered";

/// Diagnostic code recorded when another user interface stayed active after the request.
///
/// This is the code that says the plugin will not fight the user's own choice of user
/// interface, so it is the one to grep for when the self-drawn window does not appear on
/// a system where the addon is installed and available.
pub const TAKEOVER_DECLINED_CODE: &str = "ui/takeover/declined";

/// Diagnostic code recorded when no platform backend can host the candidate window.
///
/// The platform layer spells the same code for the same condition, so a session without
/// a backend is greppable under one code whether the tier ladder or the takeover reports
/// it.
pub const NO_BACKEND_CODE: &str = "platform/compositor/unsupported";

/// Diagnostic code recorded when this build has no host ABI to ask.
pub const NO_HOST_CODE: &str = "ffi/host-not-linked";

/// Whether the host kept another user interface active after being asked to switch.
///
/// Once this is set the plugin stops asking: the user's choice of user interface is
/// not this plugin's to override, and re-asking on every lifecycle event would fight
/// it. A `resume()` clears it, because the host just made this plugin active.
pub(super) static TAKEOVER_DECLINED: AtomicBool = AtomicBool::new(false);

/// What the takeover policy decided, before anything is asked of the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TakeoverPlan {
    /// The self-drawn window does not exist yet.
    NotReady,
    /// No platform backend can host the window.
    Unsupported,
    /// The host already kept another user interface active.
    Declined,
    /// Ask the host to make this plugin the active user interface.
    Ask,
}

/// The takeover decision, taken over its inputs rather than the process-wide flags so
/// that every branch is reachable from a test.
///
/// The order matters only when two conditions hold at once: the missing backend wins,
/// because it is the permanent one — a session without a backend can never host the
/// window, while a window that is merely late becomes ready on its own.
pub(super) fn plan_takeover(
    is_window_ready: bool,
    is_backend_available: bool,
    is_declined: bool,
) -> TakeoverPlan {
    if !is_backend_available {
        return TakeoverPlan::Unsupported;
    }
    if !is_window_ready {
        return TakeoverPlan::NotReady;
    }
    if is_declined {
        return TakeoverPlan::Declined;
    }
    TakeoverPlan::Ask
}

/// What one takeover attempt did.
///
/// The previous user interface's name is the value a restore needs, so it is reported
/// rather than only logged. `Eq` is derived so that a test can compare whole outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TakeoverOutcome {
    /// The host routes input-panel updates to this plugin now. `previous_ui` is the
    /// user interface that was active before the request that switched, and `None` when
    /// the host named none — the value a restore needs, and therefore only meaningful
    /// for the attempt that performed the switch.
    Active {
        /// The user interface that was active before this plugin took over.
        previous_ui: Option<String>,
    },
    /// The candidate window is not up yet, so the host was not asked.
    NotReady,
    /// No platform backend can host the window, so the host keeps its own candidates.
    Unsupported,
    /// The host has no user interface of this plugin to switch to: the addon instance
    /// is not one, or the addon is not registered under a user-interface category.
    NotRegistered,
    /// Another user interface stayed active after the request. Not retried.
    Declined {
        /// The user interface that stayed active, as the host named it.
        active_ui: Option<String>,
    },
    /// This build has no host ABI to ask.
    Unavailable,
}

impl TakeoverOutcome {
    /// The diagnostic line this outcome records.
    ///
    /// Each line starts with the stable `domain/action/reason` code an operator greps
    /// for and then names the condition. The code is the leading field and is built from
    /// the constant that spells it, so a code has exactly one spelling in this crate and
    /// a grep for it matches every line it produced. `ui/not-ready` and
    /// `platform/compositor/unsupported` are shared with the lifecycle and the platform
    /// layer respectively; this module's own codes are [`TAKEOVER_ACTIVE_CODE`],
    /// [`TAKEOVER_NOT_REGISTERED_CODE`] and [`TAKEOVER_DECLINED_CODE`].
    pub fn diagnostic(&self) -> String {
        match self {
            Self::Active { previous_ui } => {
                let previous = previous_ui.as_deref().unwrap_or("none");
                format!("{TAKEOVER_ACTIVE_CODE}: previous={previous}")
            }
            Self::NotReady => format!("{UI_NOT_READY_CODE}: {NOT_READY_DETAIL}"),
            Self::Unsupported => format!("{NO_BACKEND_CODE}: {NO_BACKEND_DETAIL}"),
            Self::NotRegistered => {
                format!("{TAKEOVER_NOT_REGISTERED_CODE}: {NOT_REGISTERED_DETAIL}")
            }
            Self::Declined { active_ui } => {
                let active = active_ui.as_deref().unwrap_or("another user interface");
                format!(
                    "{TAKEOVER_DECLINED_CODE}: {active} stays active; the takeover is not retried"
                )
            }
            Self::Unavailable => format!("{NO_HOST_CODE}: {NO_HOST_DETAIL}"),
        }
    }

    /// Whether the host routes input-panel updates to this plugin.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// What the line recorded while the candidate window does not exist yet says.
const NOT_READY_DETAIL: &str = "the takeover waits for the candidate window";

/// What the line recorded when no platform backend can host the candidate window says.
///
/// The fallback tier is a degraded look, not a broken input method, so the line names
/// both what keeps working and what the session is missing: an operator who finds this
/// in the log is looking for a way out of the tier, not only for the reason. The literal
/// is built with `concat!` so that no source line carries an unbreakable 120-column
/// string.
const NO_BACKEND_DETAIL: &str = concat!(
    "no backend can host the window; ClassicUI keeps drawing, and a compositor offering ",
    "a layer-shell, popup or subsurface surface is what enables the self-drawn window"
);

/// What the line recorded when the host has no user interface of this plugin says.
const NOT_REGISTERED_DETAIL: &str =
    "the host has no user interface of ours; ClassicUI keeps drawing";

/// What the line recorded when there is no host ABI to ask at all says.
const NO_HOST_DETAIL: &str = "user-interface takeover requested";

/// Attempts the takeover and reports what happened.
///
/// Idempotent and cheap, but **not thread-agnostic**: it reaches the host, so it runs on
/// the Fcitx5 host thread — see the module documentation. A caller re-runs it when the
/// state it depends on changes — the UI thread reporting ready, the platform probe
/// answering — and every branch either does nothing or asks the host once. A repeat
/// after a successful takeover reports [`TakeoverOutcome::Active`] again, naming this
/// plugin as its own predecessor, so the value worth recording is the one from the
/// attempt that switched. The caller records [`TakeoverOutcome::diagnostic`]; this
/// function records nothing.
pub fn register_takeover() -> TakeoverOutcome {
    apply_takeover(plan_takeover(
        candidate_window_ready(),
        window_backend_available(),
        TAKEOVER_DECLINED.load(Ordering::Acquire),
    ))
}

/// Applies a decision, split out from [`register_takeover`] so the branch that talks
/// to the host is reachable from a test.
pub(super) fn apply_takeover(plan: TakeoverPlan) -> TakeoverOutcome {
    match plan {
        TakeoverPlan::NotReady => TakeoverOutcome::NotReady,
        TakeoverPlan::Unsupported => TakeoverOutcome::Unsupported,
        TakeoverPlan::Declined => TakeoverOutcome::Declined {
            active_ui: current_ui(),
        },
        TakeoverPlan::Ask => ask_host(),
    }
}

/// Asks the host to switch, and interprets its answer.
fn ask_host() -> TakeoverOutcome {
    // Read before the request: after it, the active user interface is this plugin's.
    let previous_ui = current_ui();
    match activate_ui() {
        UiActivation::Active => TakeoverOutcome::Active { previous_ui },
        UiActivation::NotRegistered => TakeoverOutcome::NotRegistered,
        // Read after the request: for a refusal this is the user interface that stayed
        // active, which is the one the user chose.
        UiActivation::OtherUiActive => record_refusal(current_ui()),
        UiActivation::Unavailable => TakeoverOutcome::Unavailable,
    }
}

/// Records that the host kept another user interface active, and reports it.
///
/// The latch is the whole point of this function, and it is split out of [`ask_host`] so
/// that the branch is reachable from a test: a host that refuses cannot be arranged in
/// one. What it stores is the promise the card makes to the user — a choice of user
/// interface is not this plugin's to override — and the promise only holds if the refusal
/// survives the lifecycle events that follow it.
pub(super) fn record_refusal(active_ui: Option<String>) -> TakeoverOutcome {
    TAKEOVER_DECLINED.store(true, Ordering::Release);
    TakeoverOutcome::Declined { active_ui }
}
