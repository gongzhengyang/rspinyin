//! The window backend: what the session offers, and the surface the candidate window is
//! drawn into.
//!
//! Responsibility: answer whether this session can host a self-drawn candidate window,
//! construct the backend that hosts it, and hold it until the UI thread takes it. That
//! answer is the one the host reads through `UserInterface::available()`, so this module is
//! what makes the answer `true` possible at all — without it the plugin can only ever
//! decline, and ClassicUI keeps the candidates for the rest of the session.
//!
//! Boundaries: no pixel is written here and no window is placed here. The backend owns the
//! window and its buffers; the renderer fills them and the geometry pass moves them. This
//! module decides only *whether* there is a surface and *which* one.
//!
//! # The tier ladder
//!
//! `ASM-13` makes X11 and wlroots first-class tiers and accepts the popup tiers with a
//! bounded placement deviation. The ladder is walked once, in this order:
//!
//! | Rung | Needs | This build |
//! |---|---|---|
//! | X11 | `$DISPLAY` and a reachable X server | constructed here, from `ime_ui::platform::x11` |
//! | Wayland T1–T3 | a Wayland connection and the tier's protocol | **not constructible** |
//! | T4 fallback | nothing | no window; ClassicUI keeps the candidates |
//!
//! # What this build can and cannot construct
//!
//! X11 is the only rung reachable from here. `ime_ui::platform::wayland` is not part of that
//! crate's module tree in this build — `crates/ime-ui/src/platform/mod.rs` declares
//! `pub mod x11;` and nothing else, and no `wayland-client` appears in the dependency
//! closure — so a session that offers only Wayland is answered with T4 rather than with a
//! surface. That is the honest answer and the documented fallback: the host draws the
//! candidates itself and typing is unaffected. The probe reports it as
//! `platform/compositor/unsupported` with `tier=wayland`, so a Wayland session is
//! distinguishable in the log from a session with no display server at all.
//!
//! # Degradation
//!
//! A probe that finds nothing is not a failure of the addon: it clears the availability flag
//! — so the host keeps drawing its own candidate window, which is a worse look and not a
//! worse input method — and records the reason under one stable code. Nothing here returns
//! an error to the lifecycle, because "no backend" is an answer to the question the probe
//! asks, not a step that failed.
//!
//! # Threading
//!
//! The probe runs on the Fcitx5 host thread, inside the addon's load sequence, and the
//! surface it constructs is used by the UI thread alone. That hand-over is safe for the X11
//! rung because an X11 connection has no owning thread — `x11rb`'s `RustConnection` is
//! `Send` and serialises its own requests — which is not true of a Wayland connection: a
//! `wl_display` may only be used from the thread that created it, so the day the Wayland
//! rung lands, its connection has to be created on the UI thread rather than here, and this
//! module will hand over the *decision* instead of the surface.

mod environment;
mod probe;

// The display-free backend the tests drive the window with, and the tests that use it.
// Visible to the crate so that the lifecycle tests can drive the same surface the platform
// layer hands over, and compiled only for tests: no build of the plugin contains it.
#[cfg(test)]
pub(crate) mod mock;
#[cfg(test)]
mod tests;

pub use self::environment::{Environment, SessionTier};
pub use self::probe::{ProbeOutcome, initial_window_size, probe};

use std::sync::{Mutex, MutexGuard, OnceLock};

use ime_types::SurfaceBackend;

use crate::ffi::emit_diagnostic;

/// Probes the process environment.
///
/// The one entry point the lifecycle calls: it reads `$DISPLAY` and `$WAYLAND_DISPLAY` and
/// hands them to [`probe`], which is the part a test can drive with an environment of its
/// own.
///
/// # Panics
///
/// Never panics.
pub fn probe_from_process() -> ProbeOutcome {
    probe(&Environment::from_process())
}

/// Installs a probe result: stores the backend and records the answer.
///
/// Returns whether a backend can host the candidate window. The same answer is written to
/// the flag `ui_impl::availability` reports to the host, and this is the only place it is
/// written from a probe — a session nothing has probed must keep the host's own candidate
/// window.
///
/// The diagnostic is recorded here rather than by the caller so that the code has one
/// producer and one spelling. A probe that succeeded records nothing; the lifecycle summary
/// names the backend it found.
///
/// # Parameters
///
/// * `outcome` -- what the probe answered, including the surface when it found one.
///
/// # Returns
///
/// Whether the session can host the self-drawn window.
///
/// # Panics
///
/// Never panics.
pub fn install(outcome: ProbeOutcome) -> bool {
    if let Some(line) = outcome.diagnostic() {
        emit_diagnostic(&line);
    }
    let id = outcome.backend_id();
    if let Some(id) = id {
        // Set once: the identifier describes the surface this process was built around, and
        // a later probe cannot make the first window have been a different backend.
        let _ = BACKEND_ID.set(id);
    }
    let is_available = id.is_some();
    *lock_backend() = outcome.into_backend();
    // Written after the backend, so a reader that observes the flag also observes the
    // surface it promises.
    crate::ui_impl::set_window_backend_available(is_available);
    is_available
}

/// Takes the backend the probe constructed.
///
/// Returns `None` when the probe found none, and when the UI start-up has already taken the
/// one it did: a backend owns one window, and a second caller must not be handed a second
/// owner.
///
/// # Panics
///
/// Never panics.
pub fn take_backend() -> Option<Box<dyn SurfaceBackend>> {
    lock_backend().take()
}

/// The identifier of the backend the probe constructed, or `None` when it found none.
///
/// Kept after the backend itself is taken, because the lifecycle summary is written at the
/// end of the load sequence and the UI thread has the surface by then.
///
/// # Panics
///
/// Never panics.
pub fn backend_id() -> Option<&'static str> {
    BACKEND_ID.get().copied()
}

/// The backend the probe constructed, until the UI start-up takes it.
///
/// A process-wide slot rather than a return value because the initialisation sequence is a
/// table of `fn() -> Result<(), ImeError>`: two steps run in order and cannot pass a value
/// between them. The lock is uncontended — written once at load, taken once by the start-up
/// — and is never held across a call into another subsystem.
static BACKEND: Mutex<Option<Box<dyn SurfaceBackend>>> = Mutex::new(None);

/// The identifier of the backend, which outlives the backend itself.
static BACKEND_ID: OnceLock<&'static str> = OnceLock::new();

/// Borrows the backend slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The slot holds a surface, and treating the candidate
/// window as permanently unstartable would cost the user the window for the rest of the
/// process; the worst a recovered slot can do is hand out a surface that was already taken,
/// which the `Option` already prevents.
fn lock_backend() -> MutexGuard<'static, Option<Box<dyn SurfaceBackend>>> {
    match BACKEND.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}
