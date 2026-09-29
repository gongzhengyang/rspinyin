//! The host boundary: what the engine does to Fcitx5 and to the candidate window.
//!
//! # Responsibility
//!
//! A session decides; this trait is how the decision becomes visible. Every action the
//! routing layer takes outside its own state goes through [`Host`]: text reaches the
//! application, the composing text reaches the application's preedit area, a command
//! reaches the candidate window's channel, a mode switch reaches Fcitx5's input-method
//! state, and a diagnostic reaches the sink.
//!
//! # Why the engine takes this as a parameter
//!
//! The engine never holds an `InputContext`. Reaching one means a raw pointer, and raw
//! pointers are confined to `crate::ffi` — the input-context table the id-to-pointer
//! lookup needs lives in the C++ glue and can only be read through it. Passing the
//! boundary in as a parameter is what keeps the whole routing layer — the table, the
//! sessions, the effect executor — plain Rust that runs and is tested without Fcitx5
//! present.
//!
//! # What an implementation owes
//!
//! * **Do not block.** Every method runs on the Fcitx5 main loop, inside a host
//!   callback. [`Host::post_ui`] posts to the candidate window's channel and returns;
//!   it never waits for the window to draw.
//! * **Do not report success that did not happen.** The engine claims a key only when
//!   the plugin acted on it, and what it acted on is what these methods did. An
//!   implementation that silently drops a commit makes the engine consume a key whose
//!   text never reached the application.
//! * **Never log user content.** The text handed to [`Host::commit`] and
//!   [`Host::set_preedit`] is the user's input; it goes to the application and nowhere
//!   else. Diagnostics carry the stable `domain/action/reason` code of the error they
//!   report, never a word the user typed.
//!
//! # The production implementation
//!
//! It belongs beside the C ABI, in `crate::ffi`, because it needs the host's own
//! objects: the input-context table behind the id the callbacks receive, the
//! `InputContext::commitString` and `setPreedit` calls, and the user-interface channel
//! of the design's `UiCommand` queue. The C ABI carries the host-to-engine direction
//! today, so that implementation is the piece the host wiring adds; the routing layer
//! is complete against this trait without it.

use ime_types::{ImeError, UiCommand};

/// The host actions the routing layer performs.
///
/// One implementation drives a real Fcitx5 session and one drives the tests; see the
/// module documentation for what every implementation owes.
pub trait Host {
    /// Hands committed text to the application.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the commit happened in.
    /// * `text` — the text to insert. It is user content: it goes to the application
    ///   and must not reach a log event, a diagnostic line or a crash report.
    fn commit(&mut self, ic: u64, text: &str);

    /// Writes the composing text into the application's own preedit area.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `text` — the composing text, as the preedit builder produced it.
    /// * `caret` — the caret position in bytes, always on a character boundary.
    fn set_preedit(&mut self, ic: u64, text: &str, caret: u32);

    /// Empties the application's preedit area.
    ///
    /// Called both when a composition ends and when the client-preedit policy is off,
    /// in which case the area has to stay empty rather than hold what an earlier
    /// configuration put there.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    fn clear_preedit(&mut self, ic: u64);

    /// Posts one command to the candidate window.
    ///
    /// The command carries no input-context identity of its own, so `ic` is what tells
    /// an implementation which context it came from.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the command belongs to.
    /// * `command` — the command, which the channel routes by its own class: a frame is
    ///   latest-wins, a show or a hide is ordered and never dropped.
    fn post_ui(&mut self, ic: u64, command: UiCommand);

    /// Flips the input context's enabled state, and reports the state it took.
    ///
    /// This is Fcitx5's own input-method state, not a flag of the plugin's: with the
    /// context disabled the host hands the keyboard back to the application, which is
    /// what switching to English means.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// Whether the context is enabled after the call, which is what the status strip
    /// shows. An implementation that cannot read the state back reports the state it
    /// set.
    fn toggle_enabled(&mut self, ic: u64) -> bool;

    /// Reports one diagnostic condition.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the condition belongs to.
    /// * `err` — the condition, whose rendering is the stable
    ///   `domain/action/reason` code. It never carries the characters the user typed.
    fn diagnose(&mut self, ic: u64, err: &ImeError);
}
