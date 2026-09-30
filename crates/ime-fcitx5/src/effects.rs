//! The host boundary the effect executor runs against.
//!
//! # Responsibility
//!
//! A session decides and the routing layer executes: `crate::engine::router::effects`
//! walks the effect list one step returned and performs each entry in the order the
//! session produced it. This module owns the other half of that pair — the calls
//! themselves. [`HostCtx`] is what one input context can be asked to do, and
//! [`EffectHost`] is the adapter that lets the routing layer's
//! [`Host`](crate::engine::host::Host) speak it.
//!
//! # Why the boundary is a trait
//!
//! The production implementation forwards into the C++ glue and therefore needs the
//! host's own objects, which means raw pointers, which are confined to `crate::ffi`. A
//! trait is what lets everything above it — the executor, the router, the session — run
//! and be tested without Fcitx5 present, and it is what lets a test drive the same code
//! path with a recorder instead of a process.
//!
//! The boundary is per input context rather than per call: the routing layer passes the
//! context id on every call because it routes for all of them, and an implementation
//! here is built for the one context the callback arrived for. That is also what keeps
//! the production implementation free of an id-to-pointer lookup on the hot path — the
//! lookup happens once, where the boundary is built.
//!
//! # What an implementation owes
//!
//! * **Do not block.** Every method runs on the Fcitx5 main loop, inside a host callback
//!   that has 100us to spend (`ASM-04`). Nothing here waits for the candidate window to
//!   draw, takes a lock a decode could hold, or touches a file.
//! * **Never log user content.** Wherever `text` appears it is the user's own input. It
//!   reaches the application and nowhere else: not a log event, not a diagnostic line,
//!   not a crash report (`AGENTS.md` prohibition 21).
//! * **Report what did not happen.** A refused call is the difference between a key the
//!   plugin acted on and a key it swallowed, so it is returned rather than dropped.
//!
//! # Threading
//!
//! The boundary is owned by the Fcitx5 host thread and is never shared: decoding is
//! serial per session (`ASM-11`) and the routing layer is single-threaded. The candidate
//! window is reached through [`HostCtx::post`], which hands a command over and returns.

use ime_types::{ImeError, UiCommand};

use crate::engine::host::Host;
use crate::ffi::emit_diagnostic;

/// The host calls one input context can be asked to make.
///
/// Every method runs on the Fcitx5 main loop inside a host callback; see the module
/// documentation for what that obliges an implementation to do.
pub trait HostCtx {
    /// Inserts `text` into the client at the caret.
    ///
    /// # Arguments
    ///
    /// * `text` — the committed text. It is user content and must not be logged.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the host has taken the text, which is what tells the session the
    /// commit landed.
    ///
    /// # Errors
    ///
    /// [`ImeError::FfiInvalidCommit`] when the host refused the text — it has no such
    /// input context, or the context is gone. The frozen error model has no code for a
    /// refused *outgoing* transfer, and a code no rendering can produce is a code no
    /// diagnostic can match.
    ///
    /// # Panics
    ///
    /// Never.
    fn commit(&mut self, text: &str) -> Result<(), ImeError>;

    /// Writes `text` into the application's own preedit area.
    ///
    /// # Arguments
    ///
    /// * `text` — the composing text, as the preedit builder produced it. User content.
    /// * `caret` — the caret position in bytes, always on a character boundary.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the area holds the text.
    ///
    /// # Errors
    ///
    /// [`ImeError::FfiInvalidCommit`], for the reason given on [`HostCtx::commit`].
    ///
    /// # Panics
    ///
    /// Never.
    fn set_client_preedit(&mut self, text: &str, caret: u32) -> Result<(), ImeError>;

    /// Empties the application's own preedit area.
    ///
    /// Called both when a composition ends and when the client-preedit policy is off, in
    /// which case the area has to stay empty rather than hold what an earlier
    /// configuration put there.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the area is empty.
    ///
    /// # Errors
    ///
    /// [`ImeError::FfiInvalidCommit`], for the reason given on [`HostCtx::commit`].
    ///
    /// # Panics
    ///
    /// Never.
    fn clear_client_preedit(&mut self) -> Result<(), ImeError>;

    /// Whether the host has its input method enabled for this context.
    ///
    /// This is Fcitx5's own state rather than a flag of the plugin's: with the context
    /// disabled the host hands the keyboard back to the application, which is what
    /// switching to English means.
    ///
    /// # Returns
    ///
    /// The host's answer, and the value the status strip shows.
    ///
    /// # Panics
    ///
    /// Never.
    fn is_enabled(&self) -> bool;

    /// Sets the host's input-method state for this context.
    ///
    /// A host that cannot honour the call reports nothing here: the caller reads the
    /// state back through [`HostCtx::is_enabled`], so a call that did not land shows up
    /// as a state that did not change. That is deliberate — the frozen error model has no
    /// code for a refused mode switch, and returning a code that describes something else
    /// would be worse than the read-back.
    ///
    /// # Arguments
    ///
    /// * `enabled` — the state to take.
    ///
    /// # Panics
    ///
    /// Never.
    fn set_enabled(&mut self, enabled: bool);

    /// Hands one command to the candidate window.
    ///
    /// The channel routes by command class, so an implementation must not collapse the
    /// ordered ones: a frame is a full snapshot and may be coalesced, while a show or a
    /// hide must arrive in the order the session produced them (see `ime_types::ui`).
    ///
    /// # Arguments
    ///
    /// * `command` — what the window is told to do.
    ///
    /// # Panics
    ///
    /// Never.
    fn post(&mut self, command: UiCommand);
}

/// The routing layer's host boundary, over one input context's [`HostCtx`].
///
/// The adapter exists because the two boundaries ask the same question in two shapes: the
/// router carries the context id on every call, since one router serves every context,
/// while a [`HostCtx`] is built for one context and does not need to be told which. The
/// translation is the context id, the two preedit calls, and the toggle — which the
/// boundary expresses as a state to set rather than as a flip.
pub struct EffectHost<'a> {
    /// The host's identity for the context these calls belong to.
    ic: u64,
    /// The calls themselves.
    ctx: &'a mut dyn HostCtx,
}

impl<'a> EffectHost<'a> {
    /// Builds the boundary for one input context.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the calls belong to.
    /// * `ctx` — the context's own host calls, borrowed for the life of the adapter.
    ///
    /// # Returns
    ///
    /// An adapter that can be handed to the routing layer as a
    /// [`Host`](crate::engine::host::Host).
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(ic: u64, ctx: &'a mut dyn HostCtx) -> Self {
        Self { ic, ctx }
    }

    /// The input context these calls belong to.
    ///
    /// # Returns
    ///
    /// The id the adapter was built with.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn context(&self) -> u64 {
        self.ic
    }

    /// Reports a refused call on the crash channel.
    ///
    /// The line is the error's own rendering, which is the stable
    /// `domain/action/reason` code; it carries no text the user typed.
    fn report(&self, err: &ImeError) {
        emit_diagnostic(&err.to_string());
    }
}

impl Host for EffectHost<'_> {
    /// Inserts `text` into the client.
    ///
    /// The context id is the adapter's own, so it is not read from the argument.
    fn commit(&mut self, _ic: u64, text: &str) {
        if let Err(err) = self.ctx.commit(text) {
            self.report(&err);
        }
    }

    /// Writes the composing text into the application's preedit area.
    fn set_preedit(&mut self, _ic: u64, text: &str, caret: u32) {
        if let Err(err) = self.ctx.set_client_preedit(text, caret) {
            self.report(&err);
        }
    }

    /// Empties the application's preedit area.
    fn clear_preedit(&mut self, _ic: u64) {
        if let Err(err) = self.ctx.clear_client_preedit() {
            self.report(&err);
        }
    }

    /// Posts one command to the candidate window.
    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        self.ctx.post(command);
    }

    /// Flips the host's input-method state and reports the state it took.
    ///
    /// The flip is a set of the negation followed by a read-back, rather than an
    /// assumption about what the host did: a call that did not land must not be reported
    /// as a mode switch, because the status strip is drawn from this answer.
    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        let next = !self.ctx.is_enabled();
        self.ctx.set_enabled(next);
        self.ctx.is_enabled()
    }

    /// Reports one diagnostic condition.
    ///
    /// The crash channel rather than a host call: a diagnostic is the plugin's own record
    /// of a condition, and Fcitx5 captures the process's stderr into its log, which is
    /// where an operator looks. The rendering is the stable code and never carries the
    /// characters the user typed.
    fn diagnose(&mut self, _ic: u64, err: &ImeError) {
        self.report(err);
    }
}

#[cfg(test)]
mod tests;
