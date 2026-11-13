//! The candidate window's inbound interface: [`KeyRouter::ui_event`], which routes
//! the window's events back into the session they belong to, and
//! [`KeyRouter::set_anchor`], which records where the host reports the caret for
//! the frames that follow.

use ime_core::assoc::AssocRuntime;
use ime_core::state::{SessionEnv, SessionEvent};
use ime_dict::hot_swap::LexiconSlot;
use ime_dict::symbols::SymbolSlot;
use ime_dict::user_lexicon::{ChainedLexicon, UserLexiconHandle};
use ime_types::{Anchor, UiEvent};

use crate::engine::host::Host;
use crate::ffi::emit_diagnostic;

use super::KeyRouter;
use super::STALE_IC_CODE;
use super::step::StepCtx;

impl<'a> KeyRouter<'a> {
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
        let symbols = self.symbol_slot.and_then(SymbolSlot::load);
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
            symbols: symbols.as_deref().or(self.env.symbols),
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
            reverse: self.reverse_source,
            reverse_absent_reported: &mut self.reverse_absent_reported,
            user_lexicon: self.user_lexicon,
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
}
