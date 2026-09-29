//! Driving a session from the host: one session per input context, and the executor of
//! the effects a step returns.
//!
//! # Responsibility
//!
//! [`KeyRouter`] is what turns the routing table into input. A key event becomes a
//! [`KeyAction`], the action is stepped through the session state machine of `ime-core`,
//! and every [`Effect`] that comes back is executed against the [`Host`] boundary: text
//! is committed, the application's preedit area is filled or emptied, the candidate
//! window is shown, hidden and repainted, a learned frequency is recorded, and a
//! condition the host should know about is reported.
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
//! # The two effects with a sequel
//!
//! [`Effect::Commit`] and [`Effect::SetClientPreedit`] with `None` both describe work the
//! host does synchronously — `commitString`, and emptying the application's preedit area —
//! and both leave the session waiting for the host to confirm it happened. The session
//! drops every key that reaches it while it waits, so the router tells it the work landed
//! in the same call: a commit is followed by `CommitDone`, and a cleared preedit area by
//! `PreeditCleared`. Without the second one an Escape would wedge the session in its
//! `Cancelling` state and the input method would stop responding to every later key. The
//! word the user chose is recorded in the user's frequencies on the first of those
//! confirmations, so a commit the host never took teaches the dictionary nothing.
//!
//! # Threading
//!
//! The router lives on the Fcitx5 host thread and is never shared: a session is stepped
//! serially per input context (`ASM-11`), the decode is pure and bounded, and nothing here
//! blocks. The candidate window is reached through [`Host::post_ui`], which posts a
//! command and returns.

use std::collections::HashMap;

use ime_core::privacy::InputContextKind;
use ime_core::state::{
    AnchorHint, Effect, Effects, FrameContext, Session, SessionConfig, SessionEnv, SessionEvent,
    step,
};
use ime_types::{Anchor, HideReason, ImeError, KeyAction, StatusStrip, UiCommand, UiEvent};

use crate::engine::host::Host;
use crate::engine::{KeyBindings, claims_key, is_shift_press, translate_key};
use crate::ffi::{FcitxKeyEvent, emit_diagnostic};
use crate::privacy_impl::{ContextPrivacy, ContextReport, LearningGate, report_suppression};

/// Recorded when a key arrives for an input context the engine has no session for.
///
/// Stable code for a wiring defect: the host delivered a key for a context it never
/// activated, or the activation was lost. It is never a user-visible condition, which is
/// why it is reported on the diagnostic channel rather than as an effect.
pub const STALE_IC_CODE: &str = "ffi/stale-ic";

/// What the status strip shows in Chinese mode.
const MODE_LABEL_CHINESE: &str = "中";

/// What the status strip shows in English mode, temporary English included.
const MODE_LABEL_ENGLISH: &str = "英";

/// How far apart two input contexts' session id ranges start.
///
/// A session id identifies a composition across the whole process, not only within one
/// input context, so each context counts in a range of its own. Four billion compositions
/// per context is far past what a session can reach, and the counter saturates rather
/// than wrapping into another context's range.
const SESSION_ID_STRIDE: u64 = 1 << 32;

/// The configuration values the routing layer acts on.
///
/// A view rather than the user's document: the routing table reads the `[keys]` rows
/// through [`KeyBindings`], the session reads its own values through [`SessionConfig`],
/// and the effect executor reads the client-preedit policy directly. Grouping them means
/// a caller hands one value to [`KeyRouter::new`] and a reload hands one value to
/// [`KeyRouter::reload`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutingConfig {
    /// The `[keys]` settings the translation table branches on.
    pub keys: KeyBindings,
    /// The configuration the session state machine reads.
    pub session: SessionConfig,
    /// `[ui] client_preedit`: whether the composing text is written into the
    /// application's own preedit area instead of only into the candidate window.
    pub client_preedit: bool,
}

impl Default for RoutingConfig {
    /// The shipped defaults: the values `config/default.toml` declares for `[keys]` and
    /// `[ui]`, so a caller with no configuration yet routes keys and fills the preedit
    /// area exactly as a fresh installation does.
    fn default() -> Self {
        Self {
            keys: KeyBindings::default(),
            session: SessionConfig::default(),
            client_preedit: false,
        }
    }
}

/// The mode bits the engine owns.
///
/// The session does not hold them: the language switch *is* Fcitx5's input-method state,
/// and the full-width and punctuation switches are the plugin's own output choices. They
/// reach the window through the status strip the engine writes into every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Modes {
    /// Whether the host has the input method enabled for this context, which is the
    /// Chinese mode. The engine mirrors the host's answer rather than keeping a flag of
    /// its own.
    is_chinese: bool,
    /// Whether the punctuation the plugin commits is written full width.
    is_full_width: bool,
    /// Whether the punctuation the plugin commits is Chinese.
    is_punct_full: bool,
}

impl Default for Modes {
    /// A freshly activated context: Chinese, half width, Chinese punctuation — the three
    /// values the shipped configuration declares.
    fn default() -> Self {
        Self {
            is_chinese: true,
            is_full_width: false,
            is_punct_full: true,
        }
    }
}

/// What one round of effect execution did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Applied {
    /// Whether anything other than a diagnostic reached the host, which is what makes a
    /// key the plugin's to keep.
    acted: bool,
    /// Whether a commit was handed over, which the session has to be told about before
    /// the next key arrives.
    committed: bool,
    /// Whether the application's preedit area was emptied, which is the other thing the
    /// session waits for the host to confirm.
    preedit_cleared: bool,
}

/// Everything a step and its effect executor read besides the session itself.
///
/// Grouped so that a step, its effects and the commit path that closes them take the
/// session, this context and the host, which keeps every handler inside the parameter
/// budget.
struct StepCtx<'a, 'b> {
    /// The decoder, the dictionary, the user's frequencies and the language model.
    env: &'b SessionEnv<'a>,
    /// The configuration in force for this step.
    config: &'b RoutingConfig,
    /// The per-context decisions the learning path asks before it records anything.
    privacy: &'b ContextPrivacy,
}

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
}

impl Context {
    /// A context that has just been activated.
    fn new(ic: u64, session: Session) -> Self {
        Self {
            ic,
            session,
            modes: Modes::default(),
            // Until the host reports a cursor rectangle, the window is anchored at the
            // origin of the first screen with no scaling; the cursor work replaces this
            // through `KeyRouter::set_anchor`.
            anchor: FrameContext::default().anchor,
        }
    }

    /// Writes the cursor position and the mode bits into the session.
    ///
    /// Called before every step that may emit a frame, so the frame carries where the
    /// window goes and what its status strip says.
    fn write_frame_context(&mut self) {
        let status = self.status();
        self.session.set_frame_context(self.anchor, status);
    }

    /// The status strip the window draws.
    ///
    /// `has_user_dict_hit` and `readonly` are facts about the decode and the data
    /// directory: the layers that own them write them into the strip when they land, and
    /// the routing layer has nothing to say about either.
    fn status(&self) -> StatusStrip {
        StatusStrip {
            mode_label: String::from(self.mode_label()),
            full_width: self.modes.is_full_width,
            punctuation_full: self.modes.is_punct_full,
            ..StatusStrip::default()
        }
    }

    /// The label the status strip shows.
    ///
    /// Temporary English shows as English whatever the persistent state is, because that
    /// is the mode the keys are in.
    fn mode_label(&self) -> &'static str {
        if self.modes.is_chinese && !self.session.temp_english {
            MODE_LABEL_CHINESE
        } else {
            MODE_LABEL_ENGLISH
        }
    }

    /// Applies the keys the engine owns rather than the session.
    ///
    /// The three mode keys change bits the engine holds: the persistent language switch
    /// goes to the host, because switching to English means Fcitx5 hands the keyboard
    /// back to the application, and the full-width and punctuation switches are the
    /// plugin's own output choices. The session repaints from the frame context the
    /// caller wrote, so nothing here emits an effect.
    ///
    /// # Returns
    ///
    /// Whether `action` was one of them.
    fn apply_mode_action(&mut self, action: KeyAction, host: &mut dyn Host) -> bool {
        match action {
            KeyAction::ToggleLang => {
                self.modes.is_chinese = host.toggle_enabled(self.ic);
                true
            }
            KeyAction::ToggleFullWidth => {
                self.modes.is_full_width = !self.modes.is_full_width;
                true
            }
            KeyAction::TogglePunct => {
                self.modes.is_punct_full = !self.modes.is_punct_full;
                true
            }
            _ => false,
        }
    }

    /// Steps the session with one event and executes everything the step produced.
    ///
    /// An event that leaves the session waiting for the host — a commit, a cleared preedit
    /// area — is followed by the event that closes it, in the same call: see the module
    /// documentation for why the session cannot be left waiting. The loop runs at most
    /// three times, because neither closing event produces another of them.
    ///
    /// # Returns
    ///
    /// Whether anything other than a diagnostic reached the host.
    fn run(&mut self, event: SessionEvent, ctx: &StepCtx<'_, '_>, host: &mut dyn Host) -> bool {
        let mut acted = false;
        let mut pending = Some(event);
        while let Some(event) = pending.take() {
            let effects = step(&mut self.session, event, &ctx.config.session, ctx.env);
            let applied = self.apply_effects(ctx, effects, host);
            acted |= applied.acted;
            if applied.committed {
                pending = Some(SessionEvent::CommitDone);
            } else if applied.preedit_cleared {
                pending = Some(SessionEvent::PreeditCleared);
            }
        }
        acted
    }

    /// Executes one effect list against the host.
    fn apply_effects(
        &self,
        ctx: &StepCtx<'_, '_>,
        effects: Effects,
        host: &mut dyn Host,
    ) -> Applied {
        let mut applied = Applied::default();
        for effect in effects {
            match effect {
                Effect::UpdatePreedit(preedit) => {
                    self.write_preedit(ctx, &preedit.text, preedit.caret, host);
                    applied.acted = true;
                }
                Effect::SetClientPreedit(Some((text, caret))) => {
                    self.write_preedit(ctx, &text, caret, host);
                    applied.acted = true;
                }
                Effect::SetClientPreedit(None) => {
                    host.clear_preedit(self.ic);
                    applied.acted = true;
                    applied.preedit_cleared = true;
                }
                Effect::SendFrame(frame) => {
                    host.post_ui(self.ic, UiCommand::Frame(frame));
                    applied.acted = true;
                }
                Effect::Show(hint) => {
                    self.show(hint, host);
                    applied.acted = true;
                }
                Effect::Hide(reason) => {
                    self.hide(reason, host);
                    applied.acted = true;
                }
                Effect::Commit(text) => {
                    host.commit(self.ic, &text);
                    self.hide(HideReason::Committed, host);
                    applied.acted = true;
                    applied.committed = true;
                }
                Effect::RecordUserFreq { key, weight_hint } => {
                    // The one path from a commit to the user's frequencies. It goes
                    // through the gate rather than the store so that a context which
                    // must not be learned from -- a password box -- records nothing;
                    // the gate reports that per activation, never per commit.
                    let gate = LearningGate::new(ctx.privacy, ctx.env.user_freq);
                    gate.record_commit(self.ic, &key, weight_hint);
                    applied.acted = true;
                }
                Effect::AddPhrase { .. } => {
                    // The session has decided *what* to save; saving it belongs here, and
                    // "here" is not written yet. `ime-core`'s `PhraseTable` reads a phrase
                    // document and has no writer, and nothing loads the user's
                    // `phrases.tsv` into the host in the first place, so an arm that tried
                    // to append would be inventing the writer's interface before its owner
                    // has defined it.
                    //
                    // `dict/unsupported` is the contract's code for exactly this -- a
                    // capability that is deliberately not available in this build phase --
                    // and it is emitted rather than swallowed so the key is visibly inert
                    // instead of silently doing nothing.
                    host.diagnose(self.ic, &ImeError::Unsupported);
                }
                Effect::Diagnose(err) => host.diagnose(self.ic, &err),
            }
        }
        applied
    }

    /// Writes the composing text where the configuration says it goes.
    ///
    /// With `[ui] client_preedit` on, the text goes into the application's own preedit
    /// area. With it off — the shipped default — the area is emptied instead, because the
    /// candidate window's header is what shows the pinyin, and an area left holding an
    /// earlier configuration's text would duplicate it.
    fn write_preedit(&self, ctx: &StepCtx<'_, '_>, text: &str, caret: u32, host: &mut dyn Host) {
        if ctx.config.client_preedit {
            host.set_preedit(self.ic, text, caret);
        } else {
            host.clear_preedit(self.ic);
        }
    }

    /// Shows the window, resolving the anchor the session asked for.
    ///
    /// The session knows which side of the cursor it wants the window on but not where
    /// the cursor is; the rectangle is the host's, and the engine holds it.
    fn show(&self, hint: AnchorHint, host: &mut dyn Host) {
        let mut anchor = self.session.frame_context().anchor;
        anchor.placement = hint.placement;
        host.post_ui(
            self.ic,
            UiCommand::Show {
                revision: hint.revision.value(),
                anchor,
            },
        );
    }

    /// Hides the window under the revision the session is holding.
    fn hide(&self, reason: HideReason, host: &mut dyn Host) {
        let revision = self.session.revision.value();
        host.post_ui(self.ic, UiCommand::Hide { revision, reason });
    }
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
    /// The live contexts, keyed by the host's input-context id.
    contexts: HashMap<u64, Context>,
    /// The base of the next session id range; see [`SESSION_ID_STRIDE`].
    next_session_base: u64,
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
    /// context.
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
            contexts: HashMap::new(),
            next_session_base: 1,
        }
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
        self.contexts.insert(ic, Context::new(ic, session));
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
        let step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
        };
        if let Some(mut ctx) = self.contexts.remove(&ic) {
            ctx.run(SessionEvent::Reset, &step, host);
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
        let step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
        };
        if let Some(ctx) = self.contexts.get_mut(&ic) {
            ctx.run(SessionEvent::Reset, &step, host);
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
            ..
        } = self;
        let step = StepCtx {
            env,
            config,
            privacy,
        };
        let Some(ctx) = contexts.get_mut(&ic) else {
            emit_diagnostic(STALE_IC_CODE);
            return false;
        };
        if ctx.session.temp_english {
            // Temporary English hands every key back to the application. The session is
            // still stepped, because Enter and Escape are what leaves the mode.
            ctx.run(SessionEvent::Key(action), &step, host);
            return false;
        }
        // The mode bits go into the session before the step, so the frame a mode key
        // repaints carries the switch that key just made.
        let mode_acted = ctx.apply_mode_action(action, host);
        ctx.write_frame_context();
        let session_acted = ctx.run(SessionEvent::Key(action), &step, host);
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
        let step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
        };
        match self.contexts.get_mut(&ic) {
            Some(ctx) => {
                ctx.write_frame_context();
                ctx.run(SessionEvent::Ui(event), &step, host)
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
        let step = StepCtx {
            env: &self.env,
            config: &self.config,
            privacy: &self.privacy,
        };
        for ctx in self.contexts.values_mut() {
            ctx.write_frame_context();
            ctx.run(SessionEvent::ConfigReloaded(config.session), &step, host);
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
}
