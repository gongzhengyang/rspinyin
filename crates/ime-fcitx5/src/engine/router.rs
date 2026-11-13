//! Driving a session from the host: the session a context runs, the state a released
//! context parks for its successor, and the executor of the effects a step returns.
//!
//! # Responsibility
//!
//! [`KeyRouter`] is what turns the routing table into input. A key event becomes a
//! [`KeyAction`], the action is stepped through the session state
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
//! the two keys and `Session::leave_temp_english` takes the mode off. Which keys they are
//! cannot be read out of the routing table, because the space bar and the `Return` of the
//! shipped configuration are the same action; see [`leaves_temp_english`] for why that
//! makes the table the wrong place to ask.
//!
//! # Layout
//!
//! This file holds [`KeyRouter`] itself and the per-context state it keeps: one
//! `Session` per input context, the mode bits that context carries and the anchor the
//! host last reported. Under the shipped `[engine] share_state = "context"` tier that
//! state is private to its context from birth to release; the opt-in `"program"` tier
//! pools it by application, so the next window of the same program starts where the last
//! one left off. The three pieces beside it are leaves:
//!
//! * `config` is the routing layer's view of the configuration document, projected from
//!   the document by [`RoutingConfig::from_config`].
//! * `effects` is the executor: what a step returns becomes a call on [`Host`] there,
//!   including the commit and preedit confirmations that close the two effects with a
//!   sequel.
//! * `modes` is the status strip the engine's own mode bits paint, the label included.
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
use ime_core::aux::AuxSource;
use ime_core::privacy::InputContextKind;
use ime_core::reverse::ReverseSource;
use ime_core::state::{SessionEnv, SessionEvent, SessionState};
use ime_dict::hot_swap::{LexiconBundle, LexiconSlot};
use ime_dict::script::ScriptIndex;
use ime_dict::symbols::SymbolSlot;
use ime_dict::user_lexicon::UserLexiconHandle;

use crate::engine::badge::BadgeState;
use crate::engine::host::Host;
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
mod key;
mod lifecycle;
mod modes;
mod step;
mod window;

pub use self::config::RoutingConfig;

/// Re-exported for the startup sequence: the session host installs the production
/// clock once, at assembly time, so the quick panel's time domains never read the
/// process environment on their own.
pub(crate) use self::effects::quick::install_clock;

use self::context::{Context, FreshEnv, PoolKey, PoolSlot, ScriptState, pool_key};
use self::lifecycle::step_env;
use self::phrases::PhraseHandle;
use self::step::StepCtx;

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

/// The engine's routing state: a session per input context, pooled by the tier in force.
///
/// The host thread owns one of these for the lifetime of the addon. It is created with
/// the data sources the decode reads and the privacy state the commit path asks, and
/// every input context the host activates gets a session of its own -- or, under the
/// program tier, adopts the session the last released context of the same application
/// parked. See `PoolKey`.
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
    /// The slot the v-mode panel's symbol table is re-read through at every step,
    /// when the process installed one; a router without one reads
    /// [`KeyRouter::env`]'s table, which is what a test router and a degraded
    /// start do. The `Arc` a step's load hands back keeps the table alive to the
    /// end of the step, so a reload cannot tear a frame being built.
    symbol_slot: Option<&'a SymbolSlot>,
    /// The user's own words, when the process assembled them: read under a lock held for
    /// the length of a decode-bearing step, written by the drain that runs after it.
    user_lexicon: Option<&'a UserLexiconHandle>,
    /// The aux-column view of the active container, when the process assembled one: the
    /// reads the lead key's gate and a session's entry resolve their codes through. A
    /// router without one degrades the same way a container without the column does --
    /// the lead key passes through and the absence is reported once -- which is the
    /// shipped configuration's shape, since the switch itself defaults off.
    aux_source: Option<&'a dyn AuxSource>,
    /// Whether the absent-column diagnostic is already owed nothing: the code is a
    /// once-per-process report, so the flag lives on the process router beside the
    /// other state that outlives every context.
    aux_absent_reported: bool,
    /// The reverse-lookup view of the active container, when the process assembled
    /// one: the reads a `u`-payload's gate and query resolve through. A router
    /// without one degrades the same way a container without the index does -- the
    /// payload passes through to the ordinary decode and the absence is reported
    /// once -- which is the shipped configuration's shape, since the switch itself
    /// defaults off.
    reverse_source: Option<&'a dyn ReverseSource>,
    /// Whether the absent-reverse-index diagnostic is already owed nothing: the code
    /// is a once-per-process report, so the flag lives on the process router beside
    /// the other state that outlives every context.
    reverse_absent_reported: bool,
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
            symbol_slot: None,
            user_lexicon: None,
            aux_source: None,
            aux_absent_reported: false,
            reverse_source: None,
            reverse_absent_reported: false,
            privacy,
            config,
            phrases: phrases::handle(),
            script: ScriptState::Missing,
            contexts: HashMap::new(),
            pool: HashMap::new(),
            next_session_base: 1,
            badge: BadgeState::new(),
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

    /// Hands the router the aux-column view of the active container, which the lead
    /// key's gate and a session's entry resolve their reads and codes through.
    ///
    /// The process router's builder calls this once, right after [`KeyRouter::new`],
    /// with the reader the data assembly holds over the mapped container; a router
    /// built without it treats the column as absent, which is the degradation the
    /// [`ABSENT_COLUMN_CODE`](ime_core::aux::ABSENT_COLUMN_CODE) diagnostic names and
    /// the whole story for a router over a container that carries no column.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn with_aux_source(mut self, source: &'a dyn AuxSource) -> Self {
        self.aux_source = Some(source);
        self
    }

    /// Hands the router the reverse-lookup view of the active container, which a
    /// `u`-payload's gate and query resolve their reads through.
    ///
    /// The process router's builder calls this once, right after [`KeyRouter::new`],
    /// with the reader the data assembly holds over the mapped container; a router
    /// built without it treats the index as absent, which is the degradation the
    /// [`ABSENT_INDEX_CODE`](ime_core::reverse::ABSENT_INDEX_CODE) diagnostic names
    /// and the whole story for a router over a container that carries no index.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn with_reverse_source(mut self, source: &'a dyn ReverseSource) -> Self {
        self.reverse_source = Some(source);
        self
    }

    /// Installs `index` as the variant table the script mode converts through.
    ///
    /// The contexts created after this point share the handle; the ones alive before it
    /// are re-pointed too, for the same reason `KeyRouter::use_phrases` re-points
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
        let symbols = self.symbol_slot.and_then(SymbolSlot::load);
        let env = step_env(self.env, bundle.as_ref(), symbols.as_deref());
        let mut step = StepCtx {
            env: &env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
            reverse: self.reverse_source,
            reverse_absent_reported: &mut self.reverse_absent_reported,
            user_lexicon: self.user_lexicon,
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
        let symbols = self.symbol_slot.and_then(SymbolSlot::load);
        let env = step_env(self.env, bundle.as_ref(), symbols.as_deref());
        let mut step = StepCtx {
            env: &env,
            config: &self.config,
            privacy: &self.privacy,
            badge: &mut self.badge,
            reverse: self.reverse_source,
            reverse_absent_reported: &mut self.reverse_absent_reported,
            user_lexicon: self.user_lexicon,
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
