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

use std::sync::atomic::{AtomicBool, Ordering};

use crate::addon::candidate_window_ready;
use crate::ffi::{UiActivation, activate_ui, current_ui};

use super::availability::window_backend_available;

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
    /// for and then names the condition. `ui/not-ready` is the code the lifecycle
    /// already uses for "the window is not up yet", and
    /// `platform/compositor/unsupported` the one the platform layer uses for "no
    /// backend can host a window"; the takeover's own codes are `ui/takeover/active`,
    /// `ui/takeover/not-registered` and `ui/takeover/declined`.
    pub fn diagnostic(&self) -> String {
        match self {
            Self::Active { previous_ui } => {
                let previous = previous_ui.as_deref().unwrap_or("none");
                format!("ui/takeover/active: previous={previous}")
            }
            Self::NotReady => String::from(NOT_READY_LINE),
            Self::Unsupported => String::from(NO_BACKEND_LINE),
            Self::NotRegistered => String::from(NOT_REGISTERED_LINE),
            Self::Declined { active_ui } => {
                let active = active_ui.as_deref().unwrap_or("another user interface");
                format!("ui/takeover/declined: {active} stays active; the takeover is not retried")
            }
            Self::Unavailable => String::from(NO_HOST_LINE),
        }
    }

    /// Whether the host routes input-panel updates to this plugin.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// The line recorded while the candidate window does not exist yet.
const NOT_READY_LINE: &str = "ui/not-ready: the takeover waits for the candidate window";

/// The line recorded when no platform backend can host the candidate window.
///
/// The fallback tier is a degraded look, not a broken input method, which is what the
/// trailing clause tells an operator who finds this in the log. The literal is built
/// with `concat!` so that no source line carries an unbreakable 120-column string.
const NO_BACKEND_LINE: &str = concat!(
    "platform/compositor/unsupported: no backend can host the window; ",
    "ClassicUI keeps drawing"
);

/// The line recorded when the host has no user interface of this plugin to switch to.
const NOT_REGISTERED_LINE: &str = concat!(
    "ui/takeover/not-registered: the host has no user interface of ours; ",
    "ClassicUI keeps drawing"
);

/// The line recorded when there is no host ABI to ask at all.
const NO_HOST_LINE: &str = "ffi/host-not-linked: user-interface takeover requested";

/// Attempts the takeover and reports what happened.
///
/// Idempotent and cheap: a caller re-runs it when the state it depends on changes —
/// the UI thread reporting ready, the platform probe answering — and every branch
/// either does nothing or asks the host once. A repeat after a successful takeover
/// reports [`TakeoverOutcome::Active`] again, naming this plugin as its own
/// predecessor, so the value worth recording is the one from the attempt that switched.
/// The caller records [`TakeoverOutcome::diagnostic`]; this function records nothing.
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
        UiActivation::OtherUiActive => {
            // The host kept another user interface. Stop asking.
            TAKEOVER_DECLINED.store(true, Ordering::Release);
            TakeoverOutcome::Declined {
                active_ui: current_ui(),
            }
        }
        UiActivation::Unavailable => TakeoverOutcome::Unavailable,
    }
}
