//! Executing the effects a session step returns.
//!
//! # Responsibility
//!
//! A step answers with an [`Effects`] list; this module is what turns that list into
//! something the user can see. Text is committed, the application's preedit area is filled
//! or emptied, the candidate window is shown, hidden and repainted, a learned frequency is
//! recorded through the privacy gate, a phrase the user saved is appended to their own
//! document, and a condition the host should know about is reported.
//!
//! The one value it rewrites on the way through is the header's right-hand slot. The
//! status strip the engine wrote before the step carries the standing answer, and a frame
//! is only resolved when it is posted — the point at which the frame carries its own
//! candidates and page state, which is what the badge resolution reads. See the
//! `engine::badge` module for the ordering it applies.
//!
//! # The two effects with a sequel
//!
//! [`Effect::Commit`] and [`Effect::SetClientPreedit`] with `None` both describe work the
//! host does synchronously — `commitString`, and emptying the application's preedit area —
//! and both leave the session waiting for the host to confirm it happened. The session
//! drops every key that reaches it while it waits, so [`Context::run`] tells it the work
//! landed in the same call: a commit is followed by `CommitDone`, and a cleared preedit
//! area by `PreeditCleared`. Without the second one an Escape would wedge the session in
//! its `Cancelling` state and the input method would stop responding to every later key.
//! The word the user chose is recorded in the user's frequencies on the first of those
//! confirmations, so a commit the host never took teaches the dictionary nothing.
//!
//! # Boundary
//!
//! Plain Rust over the [`Host`] trait: nothing here holds a host object of its own, and
//! every call that leaves the process goes through the boundary the caller passed in.

use ime_core::state::paging::MAX_REACHABLE_CANDIDATES;
use ime_core::state::{AnchorHint, Effect, Effects, SessionEvent, step};
use ime_types::{HideReason, ImeError, UiCommand, UiFrame};

use crate::engine::badge;
use crate::engine::host::Host;
use crate::ffi::emit_diagnostic;
use crate::privacy_impl::LearningGate;

use super::phrases::PHRASE_ADDED_CODE;
use super::{Context, StepCtx};

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

impl Context {
    /// Steps the session with one event and executes everything the step produced.
    ///
    /// An event that leaves the session waiting for the host — a commit, a cleared preedit
    /// area — is followed by the event that closes it, in the same call: see the module
    /// documentation for why the session cannot be left waiting. The loop runs at most
    /// three times, because neither closing event produces another of them.
    ///
    /// # Arguments
    ///
    /// * `event` — the event to step the session with.
    /// * `ctx` — the environment, configuration and privacy state the step and its effects
    ///   read.
    /// * `host` — the boundary the effects are executed against.
    ///
    /// # Returns
    ///
    /// Whether anything other than a diagnostic reached the host.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn run(
        &mut self,
        event: SessionEvent,
        ctx: &mut StepCtx<'_, '_>,
        host: &mut dyn Host,
    ) -> bool {
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
    ///
    /// Takes the context mutably because a posted frame consumes the process-local badge
    /// state: the first-run hint is shown once, and showing it is a write.
    ///
    /// # Panics
    ///
    /// Never.
    fn apply_effects(
        &mut self,
        ctx: &mut StepCtx<'_, '_>,
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
                Effect::SendFrame(mut frame) => {
                    // The frame is complete here — the step's candidates and the page
                    // they fill are in it — which is what the header's slot resolution
                    // reads and what the pre-step write into the frame context could
                    // not have known.
                    self.write_badge(ctx, &mut frame);
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
                Effect::AddPhrase { key, text } => {
                    // The one place a phrase reaches the disk: `ime-core` touches no file
                    // (0.4 rule 4), so the row is rendered and appended here. A failure
                    // leaves the key to the application rather than swallowing it, and
                    // neither the key nor the phrase's text reaches a diagnostic.
                    match self.phrases.append(&key, &text) {
                        Ok(()) => {
                            emit_diagnostic(PHRASE_ADDED_CODE);
                            applied.acted = true;
                        }
                        Err(err) => host.diagnose(self.ic, &err),
                    }
                }
                Effect::ForgetUserWord { key } => {
                    // The one path from a forgotten candidate to the user's frequencies.
                    // The session removed the word from the frame it sent; this is what
                    // takes it out of the store, so the next decode cannot bring it back.
                    //
                    // The store answers whether anything was there to remove, and `false`
                    // is reported rather than swallowed: a key the plugin claimed and then
                    // did nothing with is the class the shortcut table forbids. The three
                    // ways `UserFreqSource::forget` answers `false` -- never learned,
                    // pinned, read-only -- are not distinguished here, because only the
                    // first is reachable today: pinning has no UI path yet, and read-only
                    // mode is already reported with `data/readonly-mode` when the store
                    // degrades. A source that gains a pinning path must widen this.
                    if !ctx.env.user_freq.forget(&key) {
                        host.diagnose(self.ic, &ImeError::UserWordNotFound);
                    }
                    applied.acted = true;
                }
                Effect::Diagnose(err) => host.diagnose(self.ic, &err),
            }
        }
        applied
    }

    /// Resolves the header's right-hand slot into a frame about to be posted.
    ///
    /// The frame context the engine wrote before the step carries the standing answer —
    /// the mode name — and the frame the step produced now carries what that write could
    /// not have known: whether it has candidates, and which page they fill. That is what
    /// the badge resolution reads, and the process-local state it consumes travels in the
    /// step context, so the first-run hint is shown exactly once for the whole process,
    /// whatever input context the first frame happens to belong to. The mode label the
    /// resolver falls back to is read from the same mode bits the standing answer was,
    /// so the two can never disagree about what the mode is.
    ///
    /// # Panics
    ///
    /// Never.
    fn write_badge(&mut self, ctx: &mut StepCtx<'_, '_>, frame: &mut UiFrame) {
        let mode = self
            .modes
            .mode_label(self.session.temp_english, ctx.config.scheme_hint);
        let has_candidates = !frame.candidates.is_empty();
        // The overflow is a fact about the whole decoded list, which the frame cannot
        // carry: its own page total is capped at the display limit, so a sixth page
        // reaches the window as a fifth. The session's full list is what the question
        // is asked of.
        let overflow =
            self.session.decoded().candidates.len() > usize::from(MAX_REACHABLE_CANDIDATES);
        let (label, overflow_started) = badge::resolve(
            mode,
            &ctx.config.keys,
            frame.page,
            has_candidates,
            overflow,
            &mut *ctx.badge,
        );
        if overflow_started {
            emit_diagnostic(badge::UI_CANDIDATE_OVERFLOW_CODE);
        }
        frame.status.mode_label = label;
    }

    /// Writes the composing text where the configuration says it goes.
    ///
    /// With `[ui] client_preedit` on, the text goes into the application's own preedit
    /// area. With it off — the shipped default — the area is emptied instead, because the
    /// candidate window's header is what shows the pinyin, and an area left holding an
    /// earlier configuration's text would duplicate it.
    ///
    /// # Panics
    ///
    /// Never.
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
    ///
    /// # Panics
    ///
    /// Never.
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
    ///
    /// # Panics
    ///
    /// Never.
    fn hide(&self, reason: HideReason, host: &mut dyn Host) {
        let revision = self.session.revision.value();
        host.post_ui(self.ic, UiCommand::Hide { revision, reason });
    }
}
