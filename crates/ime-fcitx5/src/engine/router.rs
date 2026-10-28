//! Driving a session from the host: the session a context runs, the state a released
//! context parks for its successor, and the executor of the effects a step returns.
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
//! host last reported. Under the shipped `[engine] share_state = "context"` tier that
//! state is private to its context from birth to release; the opt-in `"program"` tier
//! pools it by application, so the next window of the same program starts where the last
//! one left off. The three pieces beside it are leaves:
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
use std::sync::Arc;

use ime_config::ShareState;
use ime_core::assoc::AssocRuntime;
use ime_core::passthrough::{PassthroughDecision, classify};
use ime_core::privacy::InputContextKind;
use ime_core::state::{SessionEnv, SessionEvent, SessionState};
use ime_dict::hot_swap::{LexiconBundle, LexiconSlot};
use ime_dict::script::ScriptIndex;
use ime_dict::user_lexicon::{ChainedLexicon, UserLexiconHandle};
use ime_types::{Anchor, KeyAction, UiEvent};

use crate::engine::badge::BadgeState;
use crate::engine::compose::ComposeFallback;
use crate::engine::host::ComposeOutcome;
use crate::engine::host::Host;
use crate::engine::{
    carries_compose_blocking_modifier, claims_key, is_policy_mark, is_shift_press,
    is_syllable_separator, leaves_temp_english, translate_key,
};
use crate::ffi::{FcitxKeyEvent, emit_diagnostic};
use crate::privacy_impl::{ContextPrivacy, ContextReport, report_suppression};

/// Extracts the client-preedit capability a host report carries.
///
/// A report that names no capability — the fail-closed shape the FFI layer sends
/// when it could not read the flags — answers `true`, which is the behaviour every
/// build before the negotiation had: the negotiation narrows only when the host
/// spoke.
fn client_preedit_of(report: ContextReport<'_>) -> bool {
    match report {
        ContextReport::Reported { preedit, .. } => preedit,
        ContextReport::Unreported => true,
    }
}

// The phrase document the engine saves into: the store the `AddPhrase` arm writes
// through, which the startup sequence fills through `phrases::install`.
pub mod phrases;

mod config;
mod context;
mod effects;
mod lifecycle;
mod modes;

pub use self::config::RoutingConfig;

/// Re-exported for the startup sequence: the session host installs the production
/// clock once, at assembly time, so the quick panel's time domains never read the
/// process environment on their own.
pub(crate) use self::effects::quick::install_clock;

use self::context::{Context, PoolKey, PoolSlot, ScriptState, pool_key};
use self::lifecycle::step_env;
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
    /// The word pairs a commit coined, waiting for the user-lexicon write the step's
    /// dictionary lock forbids. Collected here, drained by the caller after the step.
    coins: Vec<(String, String)>,
    /// The words whose in-memory coin entries a forget must remove -- the store's half
    /// travelled through the frozen trait while the step ran.
    forget_words: Vec<String>,
    /// The words whose pin a toggle flipped, for the lexicon's own eviction-protection
    /// flag.
    pin_changes: Vec<String>,
}

/// The engine's routing state: a session per input context, pooled by the tier in force.
///
/// The host thread owns one of these for the lifetime of the addon. It is created with
/// the data sources the decode reads and the privacy state the commit path asks, and
/// every input context the host activates gets a session of its own -- or, under the
/// program tier, adopts the session the last released context of the same application
/// parked. See [`PoolKey`].
pub struct KeyRouter<'a> {
    /// The decoder, the dictionary, the user's frequencies and the language model.
    ///
    /// The dictionary and the model of a process that installed a hot-swap slot are read
    /// from the slot at every step instead; this value still holds the references the slot
    /// replaces (the degraded stand-ins), which are what a router without a slot decodes
    /// against.
    env: SessionEnv<'a>,
    /// The slot the dictionary and its model are re-read through at every step, when the
    /// sources behind [`KeyRouter::env`] can be replaced live. `Some` only for the process
    /// router the startup sequence builds over a mapped container; a router without one —
    /// a test router, or a router over a degraded start — decodes against [`KeyRouter::env`]
    /// as it stands.
    lexicon_slot: Option<&'a LexiconSlot>,
    /// The user's own words, when the process assembled them: read under a lock held for
    /// the length of a decode-bearing step, written by the drain that runs after it.
    user_lexicon: Option<&'a UserLexiconHandle>,
    /// The per-context privacy decisions and the gate every commit's learning goes
    /// through.
    privacy: ContextPrivacy,
    /// The configuration in force.
    config: RoutingConfig,
    /// The phrase document a saved phrase is appended to, and the table it is read into.
    phrases: PhraseHandle,
    /// The variant table the script mode converts through, or its absence. Shared with
    /// every context this router creates; installed by the data assembly, or missing
    /// until it is.
    script: ScriptState,
    /// The live contexts, keyed by the host's input-context id.
    contexts: HashMap<u64, Context>,
    /// The states a deactivated context of the program tier parked, keyed by the pool
    /// key they belong to.
    ///
    /// A slot exists only between the release of the last live context of its key and
    /// the next activation under the same key: it is taken out whole by the context that
    /// adopts it, and it is emptied again when the tier in force changes, because state
    /// parked under one tier must not answer under another. The map is bounded by the
    /// number of programs seen since the process started, one small slot each.
    pool: HashMap<PoolKey, PoolSlot>,
    /// The base of the next session id range; see [`SESSION_ID_STRIDE`].
    next_session_base: u64,
    /// The built-in compose state used only when the host answers unsupported.
    ///
    /// It is router-level because the unsupported-host condition is router-level; a
    /// context reset clears the pending dead key, while a host that can compose never
    /// touches this state at all.
    compose_fallback: ComposeFallback,
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
            lexicon_slot: None,
            user_lexicon: None,
            privacy,
            config,
            phrases: phrases::handle(),
            script: ScriptState::Missing,
            contexts: HashMap::new(),
            pool: HashMap::new(),
            next_session_base: 1,
            badge: BadgeState::new(),
            compose_fallback: ComposeFallback::new(),
        }
    }

    /// Hands the router the slot the dictionary is re-read through at every step.
    ///
    /// The process router's builder calls this once, right after [`KeyRouter::new`], with
    /// the slot the `lexicon` step installed; a router built without it decodes against its
    /// environment's own dictionary for the whole of its life.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn with_lexicon_slot(mut self, slot: &'a LexiconSlot) -> Self {
        self.lexicon_slot = Some(slot);
        self
    }

    /// Hands the router the user's own words, which every decode-bearing step reads ahead
    /// of the system dictionary.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn with_user_lexicon(mut self, handle: &'a UserLexiconHandle) -> Self {
        self.user_lexicon = Some(handle);
        self
    }

    /// Writes what a step collected for the user's lexicon, after its dictionary lock has
    /// gone.
    ///
    /// The step reads the lexicon under a lock it must hold to the end -- a decode may
    /// not wait behind a writer -- so the writes the step was asked for wait instead, and
    /// this is the drain: coins are enrolled into the memory table and the store's write
    /// queue, a forgotten word leaves the memory table, and a pin toggle moves the flag
    /// eviction honours. An empty set of changes is the ordinary case and costs one
    /// branch.
    ///
    /// # Panics
    ///
    /// Never.
    fn apply_user_lexicon_changes(
        &mut self,
        coins: Vec<(String, String)>,
        forget_words: Vec<String>,
        pin_changes: Vec<String>,
    ) {
        let Some(handle) = self.user_lexicon else {
            return;
        };
        if !coins.is_empty() {
            handle.coin(&coins);
        }
        for word in forget_words {
            handle.forget_word(&word);
        }
        for word in pin_changes {
            handle.toggle_pin(&word);
        }
    }

    /// Installs `index` as the variant table the script mode converts through.
    ///
    /// The contexts created after this point share the handle; the ones alive before it
    /// are re-pointed too, for the same reason [`KeyRouter::use_phrases`] re-points
    /// theirs -- a table installed between two activations of one context must not
    /// leave that context answering from a state the assembly already replaced.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn use_script(&mut self, index: std::sync::Arc<ScriptIndex>) {
        let script = ScriptState::Ready(index);
        for context in self.contexts.values_mut() {
            context.script = script.clone();
        }
        self.script = script;
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

    pub fn key_event(&mut self, ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
        // The compose step runs before anything claims the key: a dead-key
        // sequence the host is composing is the layout's, and a chord with Ctrl
        // or Super held is the application's shortcut, never a letter to
        // compose through. The step asks only for a press on an idle session --
        // a composing session is never asked, which is what keeps the hot path
        // free of the call -- and with the switch off it is never asked at all,
        // so the default configuration routes every key byte for byte as a
        // build before the step did.
        if self.config.compose
            && !key.is_release
            && !carries_compose_blocking_modifier(key)
            && self.compose_may_ask(ic)
        {
            match host.process_compose(ic, key) {
                ComposeOutcome::Unsupported => {
                    if let Some((character, stop)) = self.compose_fallback.process(key) {
                        let mut text = [0u8; 4];
                        host.commit(ic, character.encode_utf8(&mut text));
                        if stop {
                            return true;
                        }
                    }
                }
                ComposeOutcome::Swallowed => {
                    self.compose_fallback.reset();
                    return true;
                }
                ComposeOutcome::Committed(character) => {
                    self.compose_fallback.reset();
                    let mut text = [0u8; 4];
                    host.commit(ic, character.encode_utf8(&mut text));
                    return true;
                }
            }
        }
        let action = translate_key(key, &self.config.keys);
        if !claims_key(action) || is_shift_press(key) {
            return false;
        }
        // The dictionary the step decodes against is assembled here, once: the slot's
        // current container, the user's own words locked ahead of it, and the environment
        // over the two. The guard and the bundle are held to the end of the step -- a
        // decode may not wait behind a writer, and the words a lookup hands back point
        // into both -- which is what makes the writes the step collects wait for the
        // drain at the end instead of running inside it.
        let assoc_sensitive = !self.privacy.should_learn(ic);
        let bundle = self.lexicon_slot.map(LexiconSlot::load);
        let user = self.user_lexicon.map(UserLexiconHandle::lock);
        let chain = match (&user, &bundle) {
            (Some(user), Some(bundle)) => Some(ChainedLexicon::new(user, bundle.lexicon())),
            (Some(user), None) => Some(ChainedLexicon::new(user, self.env.lexicon)),
            _ => None,
        };
        let live_env = SessionEnv {
            decoder: self.env.decoder,
            lexicon: match (&chain, &bundle) {
                (Some(chain), _) => chain,
                (None, Some(bundle)) => bundle.lexicon(),
                (None, None) => self.env.lexicon,
            },
            user_freq: self.env.user_freq,
            lm: match &bundle {
                Some(bundle) => bundle.lm(),
                None => self.env.lm,
            },
            english: self.env.english,
            confusables: self.env.confusables,
            symbols: self.env.symbols,
            assoc: self.env.assoc.map(|runtime| AssocRuntime {
                sensitive: assoc_sensitive,
                ..runtime
            }),
        };
        let Self {
            contexts,
            config,
            privacy,
            badge,
            ..
        } = self;
        let mut step = StepCtx {
            env: &live_env,
            config,
            privacy,
            badge,
            coins: Vec::new(),
            forget_words: Vec::new(),
            pin_changes: Vec::new(),
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
                            let text = ctx.modes.transform_output(&text, ctx.script.index());
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
        let mode_acted = match action {
            // The script toggle reads a table the mode bits cannot hold, so its arm is
            // the context's own: flip the bit through the table, or report the
            // degradation and hand the key back.
            KeyAction::ToggleScript => ctx.toggle_script(),
            _ => ctx.modes.apply(action, ctx.ic, host),
        };
        // A refused script toggle is the one mode key that reaches here without acting:
        // the degradation is reported, the bit never moved, and nothing the session
        // could repaint would differ -- so the step is skipped and the key goes back to
        // the application rather than being swallowed by a switch that answered
        // nothing.
        let refused_toggle = action == KeyAction::ToggleScript && !mode_acted;
        let session_acted = if refused_toggle {
            false
        } else {
            ctx.write_frame_context(config);
            ctx.run(SessionEvent::Key(action), &mut step, host)
        };
        // `EnterTempEnglish` is the one action whose whole effect is a session flag: with
        // nothing composing it emits no effect, and the key would be handed back even
        // though the mode changed.
        let acted = mode_acted || session_acted || ctx.session.temp_english;
        // The dictionary lock outlived the step; the writes it deferred are the drain's
        // now, and every borrow they waited behind ends here, oldest first.
        let coins = std::mem::take(&mut step.coins);
        let forget_words = std::mem::take(&mut step.forget_words);
        let pin_changes = std::mem::take(&mut step.pin_changes);
        drop(step);
        drop(user);
        drop(bundle);
        self.apply_user_lexicon_changes(coins, forget_words, pin_changes);
        acted
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
        // The same assembly a key event runs: the slot's container and the user's words,
        // locked and chained for the step's whole run, the writes drained after it.
        let assoc_sensitive = !self.privacy.should_learn(ic);
        let bundle = self.lexicon_slot.map(LexiconSlot::load);
        let user = self.user_lexicon.map(UserLexiconHandle::lock);
        let chain = match (&user, &bundle) {
            (Some(user), Some(bundle)) => Some(ChainedLexicon::new(user, bundle.lexicon())),
            (Some(user), None) => Some(ChainedLexicon::new(user, self.env.lexicon)),
            _ => None,
        };
        let live_env = SessionEnv {
            decoder: self.env.decoder,
            lexicon: match (&chain, &bundle) {
                (Some(chain), _) => chain,
                (None, Some(bundle)) => bundle.lexicon(),
                (None, None) => self.env.lexicon,
            },
            user_freq: self.env.user_freq,
            lm: match &bundle {
                Some(bundle) => bundle.lm(),
                None => self.env.lm,
            },
            english: self.env.english,
            confusables: self.env.confusables,
            symbols: self.env.symbols,
            assoc: self.env.assoc.map(|runtime| AssocRuntime {
                sensitive: assoc_sensitive,
                ..runtime
            }),
        };
        let mut step = StepCtx {
            env: &live_env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
            coins: Vec::new(),
            forget_words: Vec::new(),
            pin_changes: Vec::new(),
        };
        let acted = match self.contexts.get_mut(&ic) {
            Some(ctx) => {
                ctx.write_frame_context(&self.config);
                ctx.run(SessionEvent::Ui(event), &mut step, host)
            }
            None => {
                emit_diagnostic(STALE_IC_CODE);
                false
            }
        };
        let coins = std::mem::take(&mut step.coins);
        let forget_words = std::mem::take(&mut step.forget_words);
        let pin_changes = std::mem::take(&mut step.pin_changes);
        drop(step);
        drop(user);
        drop(bundle);
        self.apply_user_lexicon_changes(coins, forget_words, pin_changes);
        acted
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
        // A tier switch empties the pool rather than keeping it: a state parked under
        // the old tier can never be adopted under the new one -- the keys are of
        // different shapes -- so keeping it would only be memory that answers nothing.
        // The live contexts stay as they are (the snapshot rule below), and every
        // activation after this point pools under the tier the new configuration names.
        if config.share != self.config.share {
            self.pool.clear();
        }
        self.config = config;
        let bundle = self.lexicon_slot.map(LexiconSlot::load);
        let env = step_env(self.env, bundle.as_ref());
        let mut step = StepCtx {
            env: &env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
            coins: Vec::new(),
            forget_words: Vec::new(),
            pin_changes: Vec::new(),
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
    pub(super) fn session(&self, ic: u64) -> Option<&ime_core::state::Session> {
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
        let bundle = self.lexicon_slot.map(LexiconSlot::load);
        let env = step_env(self.env, bundle.as_ref());
        let mut step = StepCtx {
            env: &env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
            coins: Vec::new(),
            forget_words: Vec::new(),
            pin_changes: Vec::new(),
        };
        match self.contexts.get_mut(&ic) {
            Some(ctx) => ctx.run(event, &mut step, host),
            None => false,
        }
    }
}
