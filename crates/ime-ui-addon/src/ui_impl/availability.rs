//! Whether the host may route input-panel updates to this plugin.
//!
//! Responsibility: answer the host's `UserInterface::available()` query and follow its
//! `suspend()` and `resume()` calls. The answer combines two process-wide flags -- the
//! candidate window exists, and a platform backend can host it -- and nothing else.
//!
//! Boundaries: this module owns those flags and the rule over them. It never asks the host
//! for anything, never touches the panel and never resolves geometry; the takeover
//! decision that reads the same flags is `super::takeover`'s, and the panel capture is
//! `super::panel`'s.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::addon::candidate_window_ready;

use super::takeover::TAKEOVER_DECLINED;

/// Whether the candidate window can be drawn into right now.
///
/// Answers the host's `UserInterface::available()` query, which the host asks every
/// time it re-evaluates which user interface is active — so this reads two atomics and
/// records nothing. Reporting `true` without the window or without a backend would
/// suppress ClassicUI while nothing draws in its place, which is a worse failure than
/// the default candidate window.
///
/// Suspension is deliberately **not** part of the answer. The host consults
/// `available()` while choosing, and calls `resume()` on the one it chose, so a
/// suspended user interface that answered `false` could never be chosen again and
/// would stay off for the rest of the session. [`is_host_ui_suspended`] is what the
/// window's visibility follows instead.
pub fn is_available() -> bool {
    plan_availability(candidate_window_ready(), window_backend_available())
}

/// The availability rule, taken over its inputs rather than the process-wide flags so
/// that every combination is reachable from a test.
pub(super) fn plan_availability(is_window_ready: bool, is_backend_available: bool) -> bool {
    is_window_ready && is_backend_available
}

/// Whether a platform backend that can host the self-drawn window exists.
///
/// Set by the platform probe once it lands: `true` is the `layer-shell` / `popup` /
/// `subsurface` tiers, `false` is the fallback tier where no backend can place the
/// window at all. It starts `false`, because a session nothing has probed is a session
/// that must keep the host's own candidate window.
pub fn window_backend_available() -> bool {
    WINDOW_BACKEND_AVAILABLE.load(Ordering::Acquire)
}

/// Records the platform probe's answer.
///
/// Called by the probe when it finishes, and by a test that drives the takeover
/// policy; nothing else changes it.
pub fn set_window_backend_available(is_available: bool) {
    WINDOW_BACKEND_AVAILABLE.store(is_available, Ordering::Release);
}

/// The platform probe's answer, which starts at "no backend".
static WINDOW_BACKEND_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether the host suspended this user interface.
///
/// The host calls `suspend()` when another user interface becomes active and
/// `resume()` when this one does, so the flag is the honest answer to "is anything
/// drawing into our window right now". The candidate window's visibility follows from
/// it; [`is_available`] deliberately does not, because the host re-evaluates
/// availability before it resumes a user interface.
pub fn is_host_ui_suspended() -> bool {
    HOST_UI_SUSPENDED.load(Ordering::Acquire)
}

/// Handles `UserInterface::suspend()`.
///
/// The candidate window's visibility follows from this flag; hiding the surface itself
/// belongs to the UI thread, which reads the flag through [`is_host_ui_suspended`].
pub fn on_host_suspend() {
    HOST_UI_SUSPENDED.store(true, Ordering::Release);
}

/// Handles `UserInterface::resume()`.
///
/// A resumed user interface is the active one again, so a refusal recorded earlier in
/// the session no longer describes the host's state and is cleared.
pub fn on_host_resume() {
    HOST_UI_SUSPENDED.store(false, Ordering::Release);
    TAKEOVER_DECLINED.store(false, Ordering::Release);
}

/// The suspension flag, which starts cleared.
static HOST_UI_SUSPENDED: AtomicBool = AtomicBool::new(false);
