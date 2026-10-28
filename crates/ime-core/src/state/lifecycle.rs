//! Configuration, focus and commit-confirmation life-cycle transitions.
//!
//! These rows live apart from keystroke routing because they are the boundaries where
//! the engine talks back to the session. They remain pure: every outward action is an
//! [`Effect`], and a landed commit may open association only through providers that the
//! environment injected before the step began.

use ime_types::HideReason;

use crate::state::SessionConfig;
use crate::state::machine::{Ctx, Effect, Session, SessionState};

impl Session {
    /// Adopts a reloaded configuration without disturbing the composition.
    ///
    /// The context band is latched for the next decode, and only the page size can
    /// change what the session shows immediately. Either way the input and the candidate
    /// list are left alone; a page-size change moves the highlight onto the same word on
    /// the new page grid. A reload that changes nothing else the session reads produces
    /// no effect at all, so reloading an unchanged file is idempotent.
    pub(super) fn on_config_reloaded(&mut self, next: &SessionConfig, ctx: &mut Ctx<'_>) {
        // The band changes what the next decode re-ranks with, but not the list already
        // on the screen; adopting it here avoids resetting an active composition while
        // still making the reload effective without waiting for an idle latch.
        self.context_band = next.context_band;
        self.set_assoc_history_depth(next.assoc.history_depth);
        if next.max_per_row == self.paging.page_size {
            return;
        }
        self.adopt_page_size(next.max_per_row);
        if self.state == SessionState::Composing {
            self.emit_frame(next, ctx);
        }
    }

    /// Takes the composition back when the focus goes away, committing nothing.
    pub(super) fn on_focus_lost(&mut self, ctx: &mut Ctx<'_>) {
        match self.state {
            SessionState::Composing => self.end_session(HideReason::FocusLost, ctx),
            SessionState::Committing => {
                // The commit has already been handed to the host and is not rolled
                // back; only the composing text goes away with the session. The engine
                // records the `session/commit-on-focus-out` line for this, which is a
                // host-side concern and not something a pure step can emit.
                self.clear_session();
                // The commit landed into a window that is going away, so the context
                // it would have given the next composition goes with it: focus loss
                // is the one event that ends a thread of input without the user
                // finishing it.
                self.clear_context();
                ctx.push(Effect::SetClientPreedit(None));
            }
            // A cancellation has already hidden the window and cleared the preedit,
            // and an idle session holds nothing to clear.
            SessionState::Idle | SessionState::Cancelling => self.clear_session(),
        }
    }

    /// Ends a cancellation once the host confirms the preedit is gone.
    pub(super) fn on_preedit_cleared(&mut self) {
        if self.state == SessionState::Cancelling {
            self.clear_session();
        }
    }

    /// Finishes a commit: the input goes away and the word is recorded.
    ///
    /// The record is written here rather than when the commit was asked for, so that a
    /// commit the host refused -- an invalid one, which the FFI layer reports as
    /// `ffi/invalid-commit` -- never teaches the dictionary a word that was not
    /// committed.
    pub(super) fn on_commit_done(&mut self, ctx: &mut Ctx<'_>) {
        let pending = self.take_pending();
        let landed = self.state == SessionState::Committing;
        self.clear_session();
        let assoc_previous = pending
            .as_ref()
            .filter(|_| landed)
            .map(|pending| pending.key.clone());
        if let Some(pending) = pending.filter(|_| landed) {
            // The context the next compositions decode with is born here, at the
            // commit the host confirmed landed: the word the commit put on the
            // screen. A single-word candidate is exactly the key the model's bigram
            // table continues from; a whole-sentence commit stores its whole text,
            // which no bigram entry is keyed by, so its context is inert rather
            // than wrong. The word is session memory alone -- it reaches no effect
            // and no log line, and it dies with the session.
            self.set_context(pending.key.clone());
            if pending.is_learning_commit {
                ctx.push(Effect::RecordUserFreq {
                    key: pending.key.clone(),
                    weight_hint: pending.weight_hint,
                });
            }
            // The pairs the reading confirmed follow the same landed-only rule as the
            // frequency record: a commit the host refused teaches nothing. Association
            // selections never carry coins, because they did not come from a spelling.
            if !pending.coins.is_empty() {
                ctx.push(Effect::CoinUserWords {
                    pairs: pending.coins,
                });
            }
        }
        ctx.push(Effect::SetClientPreedit(None));
        if let Some(previous) = assoc_previous {
            self.open_assoc_after_commit(&previous, ctx);
        }
    }
}
