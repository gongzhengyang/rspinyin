//! The association view's post-commit life cycle.
//!
//! Responsibility: turn one landed commit into a bounded, explicitly provided
//! association view, route only the keys that can act on that view, and break it
//! immediately on any boundary. The view reuses the ordinary candidate result and
//! paging machinery, so the window needs no second presentation channel.
//!
//! Boundaries: providers arrive through [`SessionEnv`], are non-blocking by contract
//! and are never discovered here. History is the session's in-memory ring alone. No
//! association text is logged, persisted or retained across a focus boundary.

use ime_types::{Candidate, CandidateSource, HideReason, ImeError, KeyAction, PageDir};

use crate::assoc::{AssocHit, AssocSource, merge_assoc};
use crate::state::assoc_view::AssocView;
use crate::state::machine::{Ctx, Effect, PendingCommit, Session, SessionEvent, SessionState};
use crate::state::transitions::is_input_char;

const ASSOC_ANNOTATION: &str = "联想";

impl Session {
    /// Returns how many committed words association history currently retains.
    ///
    /// The field is session-private by design; the tests read it through this door.
    #[cfg(test)]
    pub(super) fn assoc_history_len(&self) -> usize {
        AssocSource::hits(&self.assoc_history).len()
    }

    /// Reports whether the association view is open.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn is_assoc_active(&self) -> bool {
        self.assoc_view.is_some()
    }

    /// Drops the ephemeral association view without touching session history.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn clear_assoc_view(&mut self) {
        self.assoc_view = None;
    }

    /// Drops association view and session-only history after a focus or reset boundary.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn clear_assoc_memory(&mut self) {
        self.assoc_view = None;
        self.assoc_history.clear();
    }

    /// Re-bounds session-only association history from a configuration reload.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn set_assoc_history_depth(&mut self, depth: u8) {
        self.assoc_history.set_depth(usize::from(depth));
    }
}

impl Session {
    /// Builds an association view after the host confirms a commit landed.
    ///
    /// The provider query is bounded by the configured candidate quota. A disabled
    /// feature, a sensitive context, an empty predecessor, provider absence or a
    /// miss leaves the ordinary post-commit idle behaviour untouched.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn open_assoc_after_commit(&mut self, previous: &str, ctx: &mut Ctx<'_>) {
        let Some(runtime) = ctx.env.assoc else {
            return;
        };
        if !ctx.cfg.assoc.enabled || runtime.sensitive || previous.is_empty() {
            return;
        }

        // Snapshot history before using this predecessor: the commit that opens a view
        // is the context of the next composition, not one of its own candidates.
        let history = self.newest_history_hits(usize::from(ctx.cfg.assoc.max_candidates));
        self.assoc_history.push(previous, previous);
        self.set_context(previous.to_owned());
        let quota = ctx.cfg.assoc.max_candidates;
        let bigram = runtime
            .providers
            .bigram
            .map_or_else(Vec::new, |provider| provider.hits_after(previous, quota));
        let prefix = runtime
            .providers
            .prefix
            .map_or_else(Vec::new, |provider| provider.hits_after(previous, quota));
        let hits = merge_assoc(quota, &bigram, &history, &prefix);
        if hits.is_empty() {
            // A miss is the ordinary post-commit path; no ephemeral state may remain.
            return;
        }
        self.state = SessionState::Composing;
        self.start_new_id();
        self.assoc_view = Some(AssocView {
            previous: String::from(previous),
            hits,
        });
        self.render_assoc_view(ctx);
    }

    /// Copies the newest history entries in newest-first order.
    ///
    /// The ring stores insertion order, while provider ranking expects the strongest
    /// entry first. Copying at most the render quota keeps the merge allocation
    /// proportional to the view even when the configured history is deeper.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn newest_history_hits(&self, quota: usize) -> Vec<AssocHit> {
        let hits = self.assoc_history.hits();
        let start = hits.len().saturating_sub(quota);
        hits[start..].iter().rev().cloned().collect()
    }

    /// Projects owned association hits into the ordinary candidate workspace.
    ///
    /// The display text and commit text can differ, so the owned view remains the
    /// authority for selection even though the frame only sees candidates.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn render_assoc_view(&mut self, ctx: &mut Ctx<'_>) {
        let Some(view) = self.assoc_view.as_ref() else {
            return;
        };
        let hits = view.hits.clone();
        self.paging.set_page_size(ctx.cfg.max_per_row);
        {
            let result = self.decoded_mut();
            result.candidates.clear();
            result
                .candidates
                .extend(hits.iter().enumerate().map(|(position, hit)| Candidate {
                    index: u16::try_from(position.saturating_add(1)).unwrap_or(u16::MAX),
                    text: hit.text.clone(),
                    annotation: Some(String::from(ASSOC_ANNOTATION)),
                    source: CandidateSource::Assoc,
                    score: 0.0,
                    consumed_syllables: 0,
                }));
            result.segments.clear();
            result.degraded = false;
        }
        self.emit_window(ctx);
    }

    /// Routes one key while an association view is live.
    ///
    /// Selection and navigation reuse the ordinary candidate semantics without
    /// querying a provider or decoder. Every other action breaks the view first;
    /// printable input then starts the normal composition on that character.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn on_key_assoc(&mut self, action: KeyAction, ctx: &mut Ctx<'_>) {
        match action {
            KeyAction::InputChar(ch) => {
                self.clear_assoc_view();
                self.clear_input();
                self.state = SessionState::Idle;
                if !is_input_char(ch) {
                    ctx.push(Effect::Diagnose(ImeError::DecodeInvalidChar { ch, at: 0 }));
                    return;
                }
                self.start_composing(ch, ctx);
            }
            KeyAction::Backspace => self.end_assoc(HideReason::EmptyInput, ctx),
            KeyAction::CommitHighlighted => self.commit_assoc(ctx),
            KeyAction::SelectIndex(digit) => self.select_assoc_digit(digit, ctx),
            KeyAction::PageNext => self.flip_assoc_page(PageDir::Next, ctx),
            KeyAction::PagePrev => self.flip_assoc_page(PageDir::Prev, ctx),
            KeyAction::PageFirst => self.flip_assoc_edge(false, ctx),
            KeyAction::PageLast => self.flip_assoc_edge(true, ctx),
            KeyAction::MoveHighlight(delta) => {
                let total = self.candidate_count();
                if self.paging.move_highlight(delta, total) {
                    self.emit_frame(ctx.cfg, ctx);
                }
            }
            KeyAction::MoveCaret(_) => self.end_assoc(HideReason::Cancelled, ctx),
            KeyAction::Escape => self.cancel(ctx),
            KeyAction::EnterTempEnglish => {
                self.temp_english = true;
                self.end_assoc(HideReason::Cancelled, ctx);
            }
            // A boundary is preferable to silently interpreting an action against a
            // view it was not designed for. The key that broke the view is still
            // consumed by the plugin, matching a normal composition's ownership.
            KeyAction::CommitRaw
            | KeyAction::ToggleLang
            | KeyAction::ToggleFullWidth
            | KeyAction::TogglePunct
            | KeyAction::ToggleScript
            | KeyAction::ForgetHighlighted
            | KeyAction::PinHighlighted
            | KeyAction::AddPhrase
            | KeyAction::SelectFirstChar
            | KeyAction::SelectLastChar
            | KeyAction::ReduceFrequency
            | KeyAction::HideCandidate
            | KeyAction::Ignore => self.end_assoc(HideReason::Cancelled, ctx),
        }
    }

    /// Hides an association view and returns to idle after a boundary.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn end_assoc(&mut self, reason: HideReason, ctx: &mut Ctx<'_>) {
        self.clear_assoc_memory();
        self.clear_session();
        self.clear_context();
        ctx.push(Effect::SetClientPreedit(None));
        ctx.push(Effect::Hide(reason));
    }

    /// Commits the highlighted association hit's insert text.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn commit_assoc(&mut self, ctx: &mut Ctx<'_>) {
        let index = usize::from(self.paging.highlight);
        let Some(hit) = self
            .assoc_view
            .as_ref()
            .and_then(|view| view.hits.get(index))
        else {
            return;
        };
        let commit = String::from(hit.commit.as_str());
        self.set_pending(Some(PendingCommit {
            is_learning_commit: false,
            key: commit.clone(),
            weight_hint: 0,
            coins: Vec::new(),
        }));
        self.clear_assoc_view();
        self.clear_input();
        self.state = SessionState::Committing;
        ctx.push(Effect::Commit(commit));
    }

    /// Selects the association candidate a digit names.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn select_assoc_digit(&mut self, digit: u8, ctx: &mut Ctx<'_>) {
        let total = self.candidate_count();
        let Some(index) = self.paging.digit_target(digit, total) else {
            return;
        };
        self.paging.highlight = index;
        self.commit_assoc(ctx);
    }

    /// Turns association pages without re-querying either provider.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn flip_assoc_page(&mut self, dir: PageDir, ctx: &mut Ctx<'_>) {
        if self.paging.flip(dir, self.candidate_count()) {
            self.emit_frame(ctx.cfg, ctx);
        }
    }

    /// Jumps to an association page edge without re-querying either provider.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn flip_assoc_edge(&mut self, to_last: bool, ctx: &mut Ctx<'_>) {
        let total = self.candidate_count();
        let moved = if to_last {
            self.paging.flip_to_last(total)
        } else {
            self.paging.flip_to_first(total)
        };
        if moved {
            self.emit_frame(ctx.cfg, ctx);
        }
    }

    /// Reports whether an event must break the association view before ordinary routing.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn assoc_breaks_on_event(&self, event: &SessionEvent) -> bool {
        matches!(
            event,
            SessionEvent::FocusLost
                | SessionEvent::Reset
                | SessionEvent::ConfigReloaded(_)
                | SessionEvent::PinApplied
        )
    }
}
