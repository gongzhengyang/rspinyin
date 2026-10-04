//! Driving a session from the host: one session per input context, and the executor of
//! the effects a step returns.
//!
//! # Responsibility
//!
//! [`KeyRouter`] is what turns the routing table into input. A key event becomes a
//! [`KeyAction`](ime_types::KeyAction), the action is stepped through the session state
//! machine of `ime-core`, and every [`Effect`](ime_core::state::Effect) that comes back is
//! executed against the [`Host`] boundary: text is committed, the application's preedit
//! area is filled or emptied, the candidate window is shown, hidden and repainted, a
//! learned frequency is recorded, a phrase the user saved is appended to their own
//! document, and a condition the host should know about is reported.
//!
//! # The one decision this layer adds
//!
//! The table says what a key *means*; the session says what that meaning does to the
//! composition. Neither answers the question the host actually asks — *may I keep this
//! key?* — because an action the session has nothing to act on must reach the application
//! instead of disappearing. [`KeyRouter::key_event`] answers it from what the step
//! produced: the key is claimed when the plugin did something about it, and handed back
//! otherwise. "Did something" is either an effect other than a diagnostic, or one of the
//! mode switches the engine owns (the language, full-width and punctuation bits), which
//! the session never sees. A key release is never claimed, and neither is a key the table
//! does not name.
//!
//! One action is claimed only inside a composition, and the reason is a property of the
//! session rather than of the step: the syllable separator pins a boundary in the composing
//! input, and `ime-core`'s input alphabet accepts it, so an idle session would take it as
//! the first character of a new composition. [`is_syllable_separator`] names that action
//! and the guard in [`KeyRouter::key_event`] hands it back while nothing is composing.
//!
//! # Temporary English
//!
//! One mode is answered before any of that. Temporary English hands every key to the
//! application, so the branch in [`KeyRouter::key_event`] claims none of them — the two
//! keys that leave the mode included, because leaving it changes nothing else and the key
//! the user pressed to leave is still a key they pressed for the application. Leaving goes
//! through the session, which is where the mode's flag lives: [`leaves_temp_english`] names
//! the two keys and [`Session::leave_temp_english`] takes the mode off. Which keys they are
//! cannot be read out of the routing table, because the space bar and the `Return` of the
//! shipped configuration are the same action; see [`leaves_temp_english`] for why that
//! makes the table the wrong place to ask.
//!
//! # Layout
//!
//! This file holds [`KeyRouter`] itself and the per-context state it keeps: one
//! [`Session`] per input context, the mode bits that context carries and the anchor the
//! host last reported. The three pieces beside it are leaves:
//!
//! * [`config`] is the routing layer's view of the configuration document, projected from
//!   the document by [`RoutingConfig::from_config`].
//! * [`effects`] is the executor: what a step returns becomes a call on [`Host`] there,
//!   including the commit and preedit confirmations that close the two effects with a
//!   sequel.
//! * [`modes`] is the status strip the engine's own mode bits paint, the label included.
//! * [`phrases`] is the phrase document a saved phrase is appended to.
//!
//! # Threading
//!
//! The router lives on the Fcitx5 host thread and is never shared: a session is stepped
//! serially per input context (`ASM-11`), the decode is pure and bounded, and nothing here
//! blocks. The candidate window is reached through [`Host::post_ui`], which posts a
//! command and returns.

use std::collections::HashMap;

use ime_core::passthrough::{PassthroughDecision, classify};
use ime_core::privacy::InputContextKind;
use ime_core::state::{FrameContext, Session, SessionEnv, SessionEvent, SessionState};
use ime_types::{Anchor, KeyAction, UiEvent};

use crate::engine::badge::BadgeState;
use crate::engine::host::Host;
use crate::engine::{
    claims_key, is_policy_mark, is_shift_press, is_syllable_separator, leaves_temp_english,
    translate_key,
};
use crate::ffi::{FcitxKeyEvent, emit_diagnostic};
use crate::privacy_impl::{ContextPrivacy, ContextReport, report_suppression};

// The phrase document the engine saves into: the store the `AddPhrase` arm writes
// through, which the startup sequence fills through `phrases::install`.
pub mod phrases;

mod config;
mod effects;
mod modes;

pub use self::config::RoutingConfig;

use self::modes::Modes;
use self::phrases::PhraseHandle;

#[cfg(test)]
mod tests;

/// Recorded when a key arrives for an input context the engine has no session for.
///
/// Stable code for a wiring defect: the host delivered a key for a context it never
/// activated, or the activation was lost. It is never a user-visible condition, which is
/// why it is reported on the diagnostic channel rather than as an effect.
pub const STALE_IC_CODE: &str = "ffi/stale-ic";

/// How far apart two input contexts' session id ranges start.
///
/// A session id identifies a composition across the whole process, not only within one
/// input context, so each context counts in a range of its own. Four billion compositions
/// per context is far past what a session can reach, and the counter saturates rather
/// than wrapping into another context's range.
const SESSION_ID_STRIDE: u64 = 1 << 32;

/// What the engine holds for one input context.
struct Context {
    /// The host's stable identity for this context, for the calls that go back to it.
    ic: u64,
    /// The composing session: the input, the candidates and the window's state.
    session: Session,
    /// The mode bits the engine owns; see [`Modes`].
    modes: Modes,
    /// Where the host last reported the caret. Written into every frame, so a frame
    /// emitted on a key the host said nothing about still lands in the right place.
    anchor: Anchor,
    /// The phrase document a saved phrase is appended to.
    ///
    /// A clone of the router's handle rather than a reference to the router: the effect
    /// that writes a phrase is applied here, where the session's own state is, and the
    /// handle is shared precisely so that both can reach one store.
    phrases: PhraseHandle,
}

impl Context {
    /// A context that has just been activated.
    fn new(ic: u64, session: Session, phrases: PhraseHandle) -> Self {
        Self {
            ic,
            session,
            modes: Modes::default(),
            phrases,
            // Until the host reports a cursor rectangle, the window is anchored at the
            // origin of the first screen with no scaling; the cursor work replaces this
            // through `KeyRouter::set_anchor`.
            anchor: FrameContext::default().anchor,
        }
    }

    /// Writes the cursor position and the mode bits into the session.
    ///
    /// Called before every step that may emit a frame, so the frame carries where the
    /// window goes and what its status strip says. The strip written here is the standing
    /// answer — the mode name — because at this point the step has not run and the frame
    /// it will produce does not exist; the header's slot is resolved again when a frame
    /// is posted, where the frame's own candidates and page state are known. See the
    /// `engine::badge` module.
    ///
    /// # Arguments
    ///
    /// * `config` — the configuration in force, whose scheme hint the label shows.
    ///
    /// # Panics
    ///
    /// Never.
    fn write_frame_context(&mut self, config: &RoutingConfig) {
        let status = self
            .modes
            .status(self.session.temp_english, config.scheme_hint);
        self.session.set_frame_context(self.anchor, status);
    }
}

/// Everything a step and its effect executor read besides the session itself.
///
/// Grouped so that a step, its effects and the commit path that closes them take the
/// session, this context and the host, which keeps every handler inside the parameter
/// budget. The badge state is the one mutable piece: the process-local hint bit the
/// frame executor consumes, which the router owns beside its other process state.
struct StepCtx<'a, 'b> {
    /// The decoder, the dictionary, the user's frequencies and the language model.
    env: &'b SessionEnv<'a>,
    /// The configuration in force for this step.
    config: &'b RoutingConfig,
    /// The per-context decisions the learning path asks before it records anything.
    privacy: &'b ContextPrivacy,
    /// The process-local state the header's slot is resolved through when a frame is
    /// posted; the `engine::badge` module owns the resolution.
    badge: &'b mut BadgeState,
}

/// The engine's routing state: one session per input context.
///
/// The host thread owns one of these for the lifetime of the addon. It is created with
/// the data sources the decode reads and the privacy state the commit path asks, and
/// every input context the host activates gets a session of its own.
pub struct KeyRouter<'a> {
    /// The decoder, the dictionary, the user's frequencies and the language model.
    env: SessionEnv<'a>,
    /// The per-context privacy decisions and the gate every commit's learning goes
    /// through.
    privacy: ContextPrivacy,
    /// The configuration in force.
    config: RoutingConfig,
    /// The phrase document a saved phrase is appended to, and the table it is read into.
    phrases: PhraseHandle,
    /// The live contexts, keyed by the host's input-context id.
    contexts: HashMap<u64, Context>,
    /// The base of the next session id range; see [`SESSION_ID_STRIDE`].
    next_session_base: u64,
    /// Whether the header's first-run key hint is still owed: the one badge bit that
    /// outlives every context, which is why it lives on the router rather than on a
    /// session. The `engine::badge` module owns the resolution it feeds.
    badge: BadgeState,
}

impl<'a> KeyRouter<'a> {
    /// Builds a router with no context activated.
    ///
    /// # Arguments
    ///
    /// * `env` — the data sources every session decodes against.
    /// * `privacy` — the per-context decisions the commit path asks before it learns;
    ///   the router owns it, because it is the state the activation callbacks update.
    /// * `config` — the configuration in force.
    ///
    /// # Returns
    ///
    /// A router that routes nothing until [`KeyRouter::activate`] is called for an input
    /// context. Its phrase document is the one the startup sequence installed, which is
    /// why the addon runs that sequence before it builds anything that routes keys.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(env: SessionEnv<'a>, privacy: ContextPrivacy, config: RoutingConfig) -> Self {
        Self {
            env,
            privacy,
            config,
            phrases: phrases::handle(),
            contexts: HashMap::new(),
            next_session_base: 1,
            badge: BadgeState::new(),
        }
    }

    /// Adopts `phrases` as the document this router saves into, for the tests: a router
    /// built by [`KeyRouter::new`] reaches the store the startup sequence installed, and
    /// this is how a test hands over a document of its own instead.
    ///
    /// # Panics
    ///
    /// Never.
    #[cfg(test)]
    pub(super) fn use_phrases(&mut self, phrases: PhraseHandle) {
        // The live contexts hold a clone of the old handle, so they are re-pointed too:
        // a test that hands over a document after activating a context would otherwise
        // write into the store the startup sequence installed.
        for context in self.contexts.values_mut() {
            context.phrases = phrases.clone();
        }
        self.phrases = phrases;
    }

    /// Creates the session of an input context the host has activated.
    ///
    /// The context is observed as [`ContextReport::Unreported`], which every policy treats
    /// as sensitive: the capability flags the host knows about the context do not reach
    /// Rust through the C ABI yet, and an unreported context fails closed — the plugin
    /// learns nothing rather than learning from a password box. Use
    /// [`KeyRouter::activate_reported`] once the flags travel.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// What the privacy policy knows about the context. A context that must not be learned
    /// from is also reported once, here, rather than from the commit path.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn activate(&mut self, ic: u64) -> InputContextKind {
        self.activate_reported(ic, ContextReport::Unreported)
    }

    /// Creates the session of an input context the host has described.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `report` — what the host read off the context: the password and sensitive flags
    ///   and the application name, or [`ContextReport::Unreported`].
    ///
    /// # Returns
    ///
    /// What the privacy policy knows about the context, which is what decides whether the
    /// commits made in it are recorded.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn activate_reported(&mut self, ic: u64, report: ContextReport<'_>) -> InputContextKind {
        let kind = self.privacy.observe(ic, report);
        let session = Session::new_with_counter(self.next_session_base);
        self.next_session_base = self.next_session_base.saturating_add(SESSION_ID_STRIDE);
        self.contexts
            .insert(ic, Context::new(ic, session, self.phrases.clone()));
        if !self.privacy.should_learn(ic) {
            report_suppression(ic, &kind);
        }
        kind
    }

    /// Drops the session of an input context the host switched away from.
    ///
    /// The composition goes with it: nothing is committed, the window is hidden and the
    /// application's preedit area is emptied. What the privacy state knew about the
    /// context is forgotten too, so a context that is never activated again falls back to
    /// the fail-closed answer.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `host` — the boundary the effects of ending the session are executed against.
    ///
    /// # Errors
    ///
    /// None: deactivating a context that was never activated does nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn deactivate(&mut self, ic: u64, host: &mut dyn Host) {
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        if let Some(mut ctx) = self.contexts.remove(&ic) {
            ctx.run(SessionEvent::Reset, &mut step, host);
        }
        self.privacy.forget(ic);
    }

    /// Takes back the composition of an input context that stays active.
    ///
    /// The host calls this when the input method is reset — a focus change, a mode switch
    /// — without the context going away, so the session survives and only its composition
    /// ends.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `host` — the boundary the effects are executed against.
    ///
    /// # Errors
    ///
    /// None: resetting a context that was never activated does nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reset(&mut self, ic: u64, host: &mut dyn Host) {
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        if let Some(ctx) = self.contexts.get_mut(&ic) {
            ctx.run(SessionEvent::Reset, &mut step, host);
        }
    }

    /// Ensures the session of an input context that gained focus.
    ///
    /// A context that is already here is reused as it stands: the host reusing an id, or
    /// a focus coming back to a context a focus loss left behind, must not rebuild the
    /// session under it — the mode bits and the reset state the user last saw are what
    /// refocusing is entitled to find. A context that is not here yet is created exactly
    /// as [`KeyRouter::activate`] would create it, observed as
    /// [`ContextReport::Unreported`] for the same fail-closed reason.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn focus_in(&mut self, ic: u64) {
        // The reuse is the point of the call: routing focus through `activate` would
        // replace the session wholesale, and a second activation of a live id would
        // silently end the composition it was in the middle of.
        if self.contexts.contains_key(&ic) {
            return;
        }
        self.activate_reported(ic, ContextReport::Unreported);
    }

    /// Takes the composition of an input context that lost focus, and keeps the session.
    ///
    /// The session is stepped with `SessionEvent::FocusLost`, whose answer per the
    /// transition table is the application's preedit area emptied and the window hidden
    /// under the focus-lost reason, committing nothing. Unlike [`KeyRouter::deactivate`]
    /// the session, its mode bits and the privacy state stay behind, so a focus that
    /// comes back finds the context it left; unlike [`KeyRouter::reset`] the event names
    /// the focus, which the two share a handler with rather than a meaning.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `host` — the boundary the effects are executed against.
    ///
    /// # Errors
    ///
    /// None: a focus loss for a context that was never activated does nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn focus_out(&mut self, ic: u64, host: &mut dyn Host) {
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        if let Some(ctx) = self.contexts.get_mut(&ic) {
            ctx.run(SessionEvent::FocusLost, &mut step, host);
        }
    }

    /// Routes one key event and answers whether the key was consumed.
    ///
    /// A `true` answer is what makes the engine's `keyEvent` call `filterAndAccept`: the
    /// key stops here and the application never sees it. Every other answer keeps the key
    /// travelling — a key the table does not name, a key release, a key for a context with
    /// no session, a key the session had nothing to act on — which is what "the plugin did
    /// nothing" has to mean. See the module documentation for what "did something" covers.
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
    /// # Errors
    ///
    /// None: a key that cannot be routed is handed back, and a condition the host should
    /// know about leaves as an effect on `host`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn key_event(&mut self, ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
        let action = translate_key(key, &self.config.keys);
        if !claims_key(action) || is_shift_press(key) {
            return false;
        }
        let Self {
            contexts,
            env,
            config,
            privacy,
            badge,
            ..
        } = self;
        let mut step = StepCtx {
            env,
            config,
            privacy,
            badge,
        };
        let Some(ctx) = contexts.get_mut(&ic) else {
            emit_diagnostic(STALE_IC_CODE);
            return false;
        };
        if ctx.session.temp_english {
            // Temporary English hands every key back to the application, the two that leave
            // the mode included: what ends the mode changes nothing else, so taking the key
            // would take a keystroke the user typed for the application.
            //
            // The session is not stepped here, because the mode's only transition is the one
            // that leaves it and which key leaves it is a fact about the key rather than
            // about the action the table made of it. The space bar and the `Return` key are
            // both `CommitHighlighted` unless the document moves `Return` to `CommitRaw`, so
            // an action that ended the mode ended it on a space bar press. Asking
            // `leaves_temp_english` and taking the mode off through the session keeps the
            // two answers from being able to disagree.
            if leaves_temp_english(key) {
                ctx.session.leave_temp_english();
            }
            return false;
        }
        // The apostrophe pins a syllable boundary inside a composition and is an ordinary
        // character outside one. The session would take it as the first character of a new
        // composition -- the input alphabet accepts it, which is what makes `ni'hao`
        // typeable -- and opening a candidate window on it would take a key the user typed
        // for the application. The table is a function of the key and the modifiers and
        // cannot see the session, so the guard lives here, where the session is.
        if is_syllable_separator(action) && ctx.session.state != SessionState::Composing {
            return false;
        }
        // A text-producing key that lands on an idle session is the passthrough policy's
        // to answer, not the session's: the uppercase letter, the URL keystroke and the
        // bare punctuation mark are decisions about text the user typed, and the decoder
        // must not see them — a character outside the pinyin alphabet would only be
        // swallowed with a diagnostic, which is the defect this answers. While a
        // composition is live the session keeps every key it has always had: the
        // policy's composition half — a mark that carries the pending candidates out —
        // is session-machine work and stays there.
        if let KeyAction::InputChar(ch) = action {
            // While a composition is live a mark belongs to the application, exactly as
            // it did when the table claimed no punctuation: the policy's composition
            // half — a mark that carries the pending candidates out — is session-machine
            // work that has not landed. A letter never satisfies this, so the composing
            // input is untouched.
            if ctx.session.state != SessionState::Idle && is_policy_mark(ch) {
                return false;
            }
            if ctx.session.state == SessionState::Idle {
                // The typed character as the keyboard produced it, read off the raw
                // keysym: the routing table folds a shifted letter before its rows see
                // it, and the uppercase signal is the user's rather than the fold's.
                // The ASCII filter keeps the non-Latin keysym ranges — a frontend's
                // function and modifier keys — out of the policy entirely; they are not
                // `InputChar` actions anyway, so this is the guard that keeps the two
                // tables from being able to disagree.
                let typed = char::from_u32(key.sym).filter(char::is_ascii);
                if let Some(ch) = typed {
                    // The typed character is the whole raw text the policy sees; the
                    // stack buffer keeps the key path allocation-free. The surrounding
                    // text stays `None` because the `Host` boundary carries no accessor
                    // for it yet, so the URL rule sees only the evidence the keystroke
                    // itself provides.
                    let flags = ctx.modes.passthrough_flags(
                        config.auto_english_on_uppercase,
                        config.passthrough_url,
                    );
                    let mut raw = [0u8; 4];
                    match classify(ch.encode_utf8(&mut raw), flags, None) {
                        PassthroughDecision::HostHandles => return false,
                        PassthroughDecision::CommitDirectly(text) => {
                            // The same door a session commit leaves through, minus the
                            // hide: an idle session has no window to take down.
                            // Learning is untouched — typed English is not a candidate
                            // choice.
                            let text = ctx.modes.transform_output(&text);
                            host.commit(ic, &text);
                            return true;
                        }
                        // The shortcut is the router's own chord: `classify` never
                        // answers it, so the arm is unreachable, and a key the policy
                        // has no answer for belongs to the session below.
                        PassthroughDecision::EnterTempEnglish | PassthroughDecision::Decode => {}
                    }
                }
            }
        }
        // The mode bits go into the session before the step, so the frame a mode key
        // repaints carries the switch that key just made.
        let mode_acted = ctx.modes.apply(action, ctx.ic, host);
        ctx.write_frame_context(config);
        let session_acted = ctx.run(SessionEvent::Key(action), &mut step, host);
        // `EnterTempEnglish` is the one action whose whole effect is a session flag: with
        // nothing composing it emits no effect, and the key would be handed back even
        // though the mode changed.
        mode_acted || session_acted || ctx.session.temp_english
    }

    /// Routes one event from the candidate window.
    ///
    /// The window is a reader of frames and a writer of events; a click, a hover, a page
    /// request or a dismissal all come back through here. An event whose revision is not
    /// the one the session is holding is dropped by the session as stale, and a context
    /// with no session is answered the way a stale key is.
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
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn ui_event(&mut self, ic: u64, event: UiEvent, host: &mut dyn Host) -> bool {
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        match self.contexts.get_mut(&ic) {
            Some(ctx) => {
                ctx.write_frame_context(&self.config);
                ctx.run(SessionEvent::Ui(event), &mut step, host)
            }
            None => {
                emit_diagnostic(STALE_IC_CODE);
                false
            }
        }
    }

    /// Records where the host reports the caret, for the frames that follow.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `anchor` — the cursor rectangle in screen physical pixels, the screen it is on
    ///   and the scale factor.
    ///
    /// # Errors
    ///
    /// None: an anchor for a context with no session is dropped.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_anchor(&mut self, ic: u64, anchor: Anchor) {
        if let Some(ctx) = self.contexts.get_mut(&ic) {
            ctx.anchor = anchor;
        }
    }

    /// Adopts a reloaded configuration without disturbing a composition in progress.
    ///
    /// Every live session is told the new values, so the page size and the layout hint
    /// follow the file while the input buffer, the candidate list and a commit in flight
    /// survive it (0.4 rule 10). A reload that changes nothing a session reads produces no
    /// effect, which is what makes reloading an unchanged file idempotent.
    ///
    /// # Arguments
    ///
    /// * `config` — the configuration that replaces the one in force.
    /// * `host` — the boundary the resulting effects are executed against.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reload(&mut self, config: RoutingConfig, host: &mut dyn Host) {
        self.config = config;
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        for ctx in self.contexts.values_mut() {
            ctx.write_frame_context(&self.config);
            ctx.run(
                SessionEvent::ConfigReloaded(config.session),
                &mut step,
                host,
            );
        }
    }

    /// The session of one input context, for the tests.
    ///
    /// Read-only: every mutation a session undergoes goes through an event this type
    /// routes.
    #[cfg(test)]
    pub(super) fn session(&self, ic: u64) -> Option<&Session> {
        self.contexts.get(&ic).map(|ctx| &ctx.session)
    }

    /// How many contexts the privacy state is holding, for the tests.
    ///
    /// The privacy table is the router's and must stay that way; the count is what a
    /// lifecycle test reads to assert that ending a context took its privacy state with
    /// it, and that a focus loss alone did not.
    #[cfg(test)]
    pub(crate) fn observed_privacy_contexts(&self) -> usize {
        self.privacy.observed_count()
    }

    /// Steps the session of one input context with one event, for the tests.
    ///
    /// The routing table binds no key to every action a session understands, and
    /// `AddPhrase` is one of the unbound ones, so a test that has to reach the effect it
    /// produces drives the session through here. The event takes the same path a key
    /// takes: it is stepped, and every effect it returns is executed.
    ///
    /// # Panics
    ///
    /// Never.
    #[cfg(test)]
    pub(super) fn step_session(
        &mut self,
        ic: u64,
        event: SessionEvent,
        host: &mut dyn Host,
    ) -> bool {
        let mut step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
        };
        match self.contexts.get_mut(&ic) {
            Some(ctx) => ctx.run(event, &mut step, host),
            None => false,
        }
    }
}
