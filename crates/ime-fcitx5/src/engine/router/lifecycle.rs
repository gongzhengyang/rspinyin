//! The input-context lifecycle: sessions created, parked, reset, focused and released.
//!
//! Split out of `router.rs` to keep that file inside the line limit. This is the half of
//! [`KeyRouter`] that answers the host's context events -- activate, deactivate, focus,
//! reset -- and the environment rebinding a hot-swapped dictionary asks of every step.
//! What is deliberately not here is the key path: a key that arrives for a context goes
//! through the same [`KeyRouter`] it always did, and this module reads none of it.

use super::*;

/// The environment one step runs against, over a possibly swapped-in dictionary.
///
/// `bundle` is the slot's current container, fetched once by the caller and held bound for
/// the step's whole run: the environment's dictionary and model read the mapping that
/// `Arc` keeps alive, so the words a lookup in this step hands back stay readable however
/// many swaps land afterwards. `None` — a router with no slot — leaves `env` as it stands,
/// which is the whole story for a router over a degraded start or a test's own sources.
///
/// The fetch is the entire cost the hot swap asks of the key path: one atomic read per
/// step, and nothing per lookup.
///
/// # Panics
///
/// Never.
pub(super) fn step_env<'a, 'b>(
    env: SessionEnv<'a>,
    bundle: Option<&'b Arc<LexiconBundle>>,
) -> SessionEnv<'b>
where
    'a: 'b,
{
    match bundle {
        Some(bundle) => SessionEnv {
            decoder: env.decoder,
            lexicon: bundle.lexicon(),
            user_freq: env.user_freq,
            lm: bundle.lm(),
            english: env.english,
            confusables: env.confusables,
            symbols: env.symbols,
            assoc: env.assoc,
        },
        None => env,
    }
}

impl<'a> KeyRouter<'a> {
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
        let key = pool_key(self.config.share, ic, kind.app_id);
        // The program tier's whole point: the state a released context of this key
        // parked is the state this context starts from. Under the context tier the pool
        // never holds the key, so the lookup misses and the branch costs one hash probe.
        let context = match self.pool.remove(&key) {
            Some(slot) => Context::adopted(
                ic,
                key,
                slot,
                self.phrases.clone(),
                self.script.clone(),
                client_preedit_of(report),
            ),
            None => {
                let base = self.next_session_base;
                self.next_session_base = self.next_session_base.saturating_add(SESSION_ID_STRIDE);
                Context::fresh(
                    ic,
                    key,
                    base,
                    self.phrases.clone(),
                    self.script.clone(),
                    self.config.output,
                    client_preedit_of(report),
                )
            }
        };
        self.contexts.insert(ic, context);
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
        if let Some(mut ctx) = self.contexts.remove(&ic) {
            ctx.run(SessionEvent::Reset, &mut step, host);
            self.park(ctx);
        }
        self.compose_fallback.reset();
        self.privacy.forget(ic);
    }

    /// Parks the state of a deactivated context, when it has a successor to hand it to.
    ///
    /// Three conditions, each its own refusal. Only the program tier parks, and only
    /// program-key state: the context tier's contract is today's behaviour, where a
    /// deactivated context ends as a whole and its next activation starts fresh, so a
    /// context-tier key never enters the pool under any tier -- which is also what keeps
    /// the unknown-application contexts of the program tier from pooling. A key another
    /// live context still holds does not park: the live context's state is the
    /// authoritative one, and what the releasing window did since focus left is nobody's
    /// to adopt. And a context created under a tier that is no longer in force does not
    /// park: a policy switch closes the states of the old tier rather than letting them
    /// resurface under the new one -- the same snapshot semantics that lets the live
    /// contexts finish what they started.
    fn park(&mut self, ctx: Context) {
        let key = ctx.key;
        if !matches!(key, PoolKey::Program(_)) || self.config.share != ShareState::Program {
            return;
        }
        if self.contexts.values().any(|live| live.key == key) {
            return;
        }
        self.pool.insert(
            key,
            PoolSlot {
                session: ctx.session,
                modes: ctx.modes,
            },
        );
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
        if let Some(ctx) = self.contexts.get_mut(&ic) {
            ctx.run(SessionEvent::Reset, &mut step, host);
        }
        self.compose_fallback.reset();
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
    /// Answers whether the compose step may ask the host about a key of this
    /// context: only an idle session that is not passing keys through is
    /// composing nothing of its own, and those are the keys a dead-key sequence
    /// belongs to.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn compose_may_ask(&self, ic: u64) -> bool {
        match self.contexts.get(&ic) {
            Some(context) => {
                context.session.state == SessionState::Idle && !context.session.temp_english
            }
            None => false,
        }
    }
}
