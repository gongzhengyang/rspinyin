//! The session host: the host's callbacks onto the routing layer's sessions.
//!
//! # Responsibility
//!
//! Fcitx5 delivers a key, an activation, a reset or a window event, each naming an input
//! context; this module is what turns that into work on the session of that context and
//! into calls back on the host. It owns the one thing the routing layer cannot: the
//! process-wide slot the sessions live in, and the sweep that empties it at unload.
//!
//! The routing itself is [`KeyRouter`]'s: it holds one session per input context, steps
//! it, executes the effects a step returns, and answers whether a key may be kept. This
//! module adds no decision of its own — a second place deciding whether a key is the
//! plugin's would be a second answer to the question the shortcut table forbids
//! answering twice. What it adds is the slot, the lifecycle, and the degradation for a
//! callback that arrives before anything is installed.
//!
//! # The slot
//!
//! One session host per process, behind a `Mutex` over an `Option` rather than a
//! `OnceLock`, because the slot is emptied as well as filled: the shutdown sweep takes
//! the host out, which is what makes a second sweep a no-op instead of a second teardown.
//! The first install wins, like the user store's: the router already there is the one a
//! decode may be running through.
//!
//! The lock is held for the length of one callback. It is taken in one order only —
//! before the crash channel's throttle, which is the sole other lock reachable from here
//! and is never taken the other way round — so no deadlock is reachable. Every callback
//! runs on the Fcitx5 main loop and none of them blocks, allocates without bound or
//! waits for the candidate window (`ASM-04`).
//!
//! # Not installed yet
//!
//! Nothing installs the host at load: the environment a session decodes against needs the
//! memory-mapped dictionary and the user-frequency store, and the startup step that
//! assembles them has not landed. Until it does, every callback answers its documented
//! safe default — no key is claimed, so nothing is swallowed — and records
//! [`NO_SESSION_HOST_CODE`] once per window. That is the same behaviour the plugin has
//! today, which is why the gap is safe to leave open; [`install`] is the seam the step
//! plugs into.
//!
//! # Threading
//!
//! Everything here runs on the Fcitx5 host thread. Sessions are stepped serially
//! (`ASM-11`) and the UI thread never touches one: it reads the frames the router posts.

use std::sync::{Mutex, MutexGuard};

use ime_core::privacy::InputContextKind;
use ime_core::state::SessionEnv;
use ime_types::{Anchor, UiEvent};

use crate::engine::host::Host;
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::ffi::{FcitxKeyEvent, emit_diagnostic};
use crate::privacy_impl::ContextPrivacy;

/// Recorded when a callback arrives with no session host installed.
///
/// Stable code for a wiring or start-up condition rather than a user-visible one, in the
/// same family as `ffi/stale-ic`: it names the one piece the plugin is waiting for. It is
/// throttled by the crash channel, so a keystroke that reaches it costs one line per
/// window rather than one per frame.
pub const NO_SESSION_HOST_CODE: &str = "ffi/no-session-host";

/// The sessions this plugin holds, keyed by the host's input-context id.
///
/// One entry per input context: v1 is single-user single-session (`ASM-12`), but Fcitx5
/// hands out a fresh id per application window, and a session that outlived its context
/// would keep a window on screen with no application behind it. The map is bounded by the
/// host's context count and an entry is dropped when the host deactivates it.
pub struct SessionHost {
    /// The sessions, and everything that routes a key into them.
    router: KeyRouter<'static>,
    /// The input contexts the host has activated, in activation order.
    ///
    /// Kept here rather than read from the router because the routing layer does not
    /// enumerate its contexts and the shutdown sweep has to reach every one of them. The
    /// two are written together in [`SessionHost::activate`] and
    /// [`SessionHost::deactivate`], so the list can only drift if a context left the
    /// router without this type hearing about it — which would be a defect here, not
    /// there.
    live: Vec<u64>,
}

impl SessionHost {
    /// Wraps a router in the host's slot type.
    ///
    /// # Arguments
    ///
    /// * `router` — the router every callback of this process goes through.
    ///
    /// # Returns
    ///
    /// A host with no context activated, which is the state a freshly loaded addon is in.
    ///
    /// # Panics
    ///
    /// Never.
    fn new(router: KeyRouter<'static>) -> Self {
        Self {
            router,
            live: Vec::new(),
        }
    }

    /// Creates the session of an input context the host switched to this input method.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// What the privacy policy knows about the context. The capability flags the host
    /// holds do not reach Rust through the C ABI yet, so the context is observed as
    /// unreported — which every policy treats as sensitive, and which is why the plugin
    /// learns nothing rather than learning from a password box.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn activate(&mut self, ic: u64) -> InputContextKind {
        if !self.live.contains(&ic) {
            self.live.push(ic);
        }
        self.router.activate(ic)
    }

    /// Drops the session of an input context the host switched away from.
    ///
    /// The composition goes with it: nothing is committed, the window is hidden and the
    /// application's preedit area is emptied. What the privacy state knew about the
    /// context is forgotten too.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `host` — the boundary the effects of ending the session are executed against.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn deactivate(&mut self, ic: u64, host: &mut dyn Host) {
        self.router.deactivate(ic, host);
        self.live.retain(|live| *live != ic);
    }

    /// Takes back the composition of an input context that stays active.
    ///
    /// The host calls this when the input method is reset — a focus change, a mode switch
    /// — without the context going away, so the session survives and only its composition
    /// ends. The host-side call sequence is the one
    /// [`SessionHost::deactivate`] produces; what differs is that the session stays.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `host` — the boundary the effects are executed against.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reset(&mut self, ic: u64, host: &mut dyn Host) {
        self.router.reset(ic, host);
    }

    /// Routes one key event and answers whether the key was consumed.
    ///
    /// A `true` answer is what makes the engine's `keyEvent` call `filterAndAccept`. Every
    /// other answer keeps the key travelling: a key the routing table does not name, a key
    /// release, a key for a context with no session, a key the session had nothing to act
    /// on.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the key arrived in.
    /// * `key` — the key as the host delivered it.
    /// * `host` — the boundary the resulting effects are executed against.
    ///
    /// # Returns
    ///
    /// Whether the plugin acted on the key.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn key_event(&mut self, ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
        self.router.key_event(ic, key, host)
    }

    /// Routes one event from the candidate window.
    ///
    /// The window is a reader of frames and a writer of events: a click, a hover, a page
    /// request or a dismissal all come back through here. An event whose revision is not
    /// the one the session holds is dropped by the session as stale.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the event belongs to.
    /// * `event` — what the user did to the window.
    /// * `host` — the boundary the resulting effects are executed against.
    ///
    /// # Returns
    ///
    /// Whether anything other than a diagnostic reached the host.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn ui_event(&mut self, ic: u64, event: UiEvent, host: &mut dyn Host) -> bool {
        self.router.ui_event(ic, event, host)
    }

    /// Records where the host reports the caret, for the frames that follow.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `anchor` — the cursor rectangle, the screen it is on and the scale factor.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_anchor(&mut self, ic: u64, anchor: Anchor) {
        self.router.set_anchor(ic, anchor);
    }

    /// Adopts a reloaded configuration without disturbing a composition in progress.
    ///
    /// Every live session is told the new values, so the page size and the layout hint
    /// follow the file while the input buffer, the candidate list and a commit in flight
    /// survive it (`AGENTS.md` prohibition 23). A reload that changes nothing a session
    /// reads produces no effect, which is what makes reloading an unchanged file
    /// idempotent.
    ///
    /// # Arguments
    ///
    /// * `config` — the configuration that replaces the one in force.
    /// * `host` — the boundary the resulting effects are executed against.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reload(&mut self, config: RoutingConfig, host: &mut dyn Host) {
        self.router.reload(config, host);
    }

    /// How many input contexts are live.
    ///
    /// # Returns
    ///
    /// The number of sessions the host holds. Zero after a sweep, which is what the
    /// unload criterion asserts.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn live_contexts(&self) -> usize {
        self.live.len()
    }

    /// Ends every session and empties the table.
    ///
    /// # Arguments
    ///
    /// * `host` — the boundary the effects of ending a session are executed against.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn shutdown(&mut self, host: &mut dyn Host) {
        let live = std::mem::take(&mut self.live);
        for ic in live {
            self.router.deactivate(ic, host);
        }
    }
}

/// The process's session host: installed at load, taken by the shutdown sweep.
static SESSIONS: Mutex<Option<SessionHost>> = Mutex::new(None);

/// Borrows the session slot, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. What the slot holds — a router and a list of ids —
/// stays usable, and refusing to serve it would leave the plugin unable to route a single
/// key for the rest of the process's life.
fn lock_sessions() -> MutexGuard<'static, Option<SessionHost>> {
    match SESSIONS.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Runs `read` against the installed session host, or reports the missing one.
///
/// # Arguments
///
/// * `fallback` — the value to answer when nothing is installed.
/// * `read` — the work, which runs with the slot held.
///
/// # Returns
///
/// What `read` answered, or `fallback`.
///
/// # Panics
///
/// Never.
fn with_sessions<T>(fallback: T, read: impl FnOnce(&mut SessionHost) -> T) -> T {
    let mut slot = lock_sessions();
    match slot.as_mut() {
        Some(sessions) => read(sessions),
        None => {
            // Reported with the slot still held. The crash channel's throttle is the only
            // other lock reachable from here, it is taken in this order and never the
            // other way round, and it is released before its line is written; see the
            // module documentation.
            emit_diagnostic(NO_SESSION_HOST_CODE);
            fallback
        }
    }
}

/// Installs the process's session host; the first install wins.
///
/// The seam the startup sequence plugs into once the sources a decode reads exist. A
/// second install is refused rather than replacing the first: the router already there is
/// the one a key may be running through, and swapping it would drop the composition the
/// user is in the middle of.
///
/// # Arguments
///
/// * `env` — the decoder, the dictionary, the user's frequencies and the language model.
///   The references are `'static` because the host outlives every callback that reaches
///   it: the sources are process-wide, like the user store.
/// * `privacy` — the per-context decisions the commit path asks before it learns.
/// * `config` — the configuration in force, as the routing layer reads it.
///
/// # Returns
///
/// Whether the host was installed. `false` means one was already there.
///
/// # Panics
///
/// Never.
pub fn install(env: SessionEnv<'static>, privacy: ContextPrivacy, config: RoutingConfig) -> bool {
    let mut slot = lock_sessions();
    if slot.is_some() {
        return false;
    }
    *slot = Some(SessionHost::new(KeyRouter::new(env, privacy, config)));
    true
}

/// Whether a session host is installed.
///
/// # Returns
///
/// `true` once [`install`] has run and before [`shutdown`] takes it away again.
///
/// # Panics
///
/// Never.
pub fn is_installed() -> bool {
    lock_sessions().is_some()
}

/// Creates the session of an input context the host switched to this input method.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
///
/// # Returns
///
/// Whether a session is live for the context afterwards, which is what the C ABI's
/// activation callback reports. `false` means nothing is installed, so the host was told
/// nothing was set up — it never means a session was dropped.
///
/// # Panics
///
/// Never.
pub fn activate(ic: u64) -> bool {
    with_sessions(false, |sessions| {
        sessions.activate(ic);
        true
    })
}

/// Drops the session of an input context the host switched away from.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `host` — the boundary the effects of ending the session are executed against.
///
/// # Panics
///
/// Never.
pub fn deactivate(ic: u64, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.deactivate(ic, host));
}

/// Takes back the composition of an input context that stays active.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `host` — the boundary the effects are executed against.
///
/// # Panics
///
/// Never.
pub fn reset(ic: u64, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.reset(ic, host));
}

/// Routes one key event and answers whether the key was consumed.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context the key arrived in.
/// * `key` — the key as the host delivered it.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Returns
///
/// Whether the plugin acted on the key. `false` for a key that must keep travelling, and
/// `false` when no session host is installed.
///
/// # Panics
///
/// Never.
pub fn key_event(ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
    with_sessions(false, |sessions| sessions.key_event(ic, key, host))
}

/// Routes one event from the candidate window.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context the event belongs to.
/// * `event` — what the user did to the window.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Returns
///
/// Whether anything other than a diagnostic reached the host.
///
/// # Panics
///
/// Never.
pub fn ui_event(ic: u64, event: UiEvent, host: &mut dyn Host) -> bool {
    with_sessions(false, |sessions| sessions.ui_event(ic, event, host))
}

/// Records where the host reports the caret, for the frames that follow.
///
/// # Arguments
///
/// * `ic` — the host's identity for the input context.
/// * `anchor` — the cursor rectangle, the screen it is on and the scale factor.
///
/// # Panics
///
/// Never.
pub fn set_anchor(ic: u64, anchor: Anchor) {
    with_sessions((), |sessions| sessions.set_anchor(ic, anchor));
}

/// Adopts a reloaded configuration without disturbing a composition in progress.
///
/// # Arguments
///
/// * `config` — the configuration that replaces the one in force.
/// * `host` — the boundary the resulting effects are executed against.
///
/// # Panics
///
/// Never.
pub fn reload(config: RoutingConfig, host: &mut dyn Host) {
    with_sessions((), |sessions| sessions.reload(config, host));
}

/// Takes the session host out of the process and reports how many sessions were live.
///
/// The sweep the addon runs at unload. It drops the sessions rather than stepping each
/// one with `SessionEvent::Reset`, and the difference is the host: a reset's effects — a
/// hide posted to the candidate window — have nowhere to go at unload, because the
/// lifecycle module reaches neither the C ABI nor the window's addon, and the window's
/// addon is being torn down with this one. Nothing is committed either way, which is what
/// the unload criterion asks for: no session is left behind and no candidate can still be
/// taken. [`SessionHost::shutdown`] is the same sweep for a caller that does have a host.
///
/// # Returns
///
/// The number of sessions that were live, for the line the caller reports.
///
/// # Panics
///
/// Never.
pub fn shutdown() -> usize {
    match lock_sessions().take() {
        Some(sessions) => sessions.live_contexts(),
        None => 0,
    }
}

#[cfg(test)]
mod tests;
