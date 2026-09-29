//! The transitions of the input session state machine.
//!
//! Responsibility: one method per state, one arm per event, so that the design's
//! transition table can be read off this file row by row. Each handler is named
//! after what it does to the session rather than after the state it serves, because
//! several states share a handler: a focus loss and a plugin reset both take the
//! composition back, and Escape, a right click and a scroll past the first page all
//! cancel one.
//!
//! Boundaries: nothing here touches the host. The handlers mutate the session and
//! push [`Effect`] values onto the step context; executing them -- posting to the UI
//! thread, calling `commitString`, writing the user's frequencies -- is the engine's
//! job.
//!
//! # Exhaustiveness
//!
//! The `match` on [`KeyAction`] in `on_key_composing` and the one in `on_key_idle`
//! between them name every variant of the frozen enum without a wildcard, so adding
//! a key action breaks the build instead of silently falling into a catch-all arm.
//! The `match` on [`SessionEvent`] in `on_event` does the same for the events.

use ime_types::{HideReason, ImeError, KeyAction, PageDir, UiEvent};

use crate::input::BackspaceOutcome;
use crate::state::SessionConfig;
use crate::state::machine::{Ctx, Effect, PendingCommit, Session, SessionEvent, SessionState};

impl Session {
    /// Routes one event to the handler for the state the session is in.
    pub(super) fn on_event(&mut self, ev: SessionEvent, ctx: &mut Ctx<'_>) {
        match ev {
            SessionEvent::Key(action) => self.on_key(action, ctx),
            SessionEvent::Ui(event) => self.on_ui(event, ctx),
            SessionEvent::ConfigReloaded(next) => self.on_config_reloaded(&next, ctx),
            SessionEvent::FocusLost => self.on_focus_lost(ctx),
            // Shutting the plugin down has the same session-side effect as losing the
            // focus: nothing is committed and the window goes away. The engine posts
            // `UiCommand::Shutdown` itself, which is not a session concern.
            SessionEvent::Reset => self.on_focus_lost(ctx),
            SessionEvent::PreeditCleared => self.on_preedit_cleared(),
            SessionEvent::CommitDone => self.on_commit_done(ctx),
        }
    }

    /// Routes one key by the state the session is in.
    fn on_key(&mut self, action: KeyAction, ctx: &mut Ctx<'_>) {
        if self.temp_english {
            self.on_key_temp_english(action);
            return;
        }
        match self.state {
            SessionState::Idle => self.on_key_idle(action, ctx),
            SessionState::Composing => self.on_key_composing(action, ctx),
            SessionState::Cancelling | SessionState::Committing => {
                // The session has already decided what happens to the composition; a
                // key that arrives while the host finishes the job is handed straight
                // back rather than acted on twice.
            }
        }
    }

    /// Temporary English mode: every key reaches the application.
    ///
    /// Enter and Escape are the two keys that leave the mode. They change no other
    /// state and produce no effect, so the key that ended the mode is handed back to
    /// the application as well -- which is what "every key passes through" means.
    fn on_key_temp_english(&mut self, action: KeyAction) {
        let leaves = matches!(
            action,
            KeyAction::Escape | KeyAction::CommitHighlighted | KeyAction::CommitRaw
        );
        if leaves {
            self.temp_english = false;
        }
    }

    /// Routes one key with nothing composing.
    fn on_key_idle(&mut self, action: KeyAction, ctx: &mut Ctx<'_>) {
        match action {
            KeyAction::InputChar(ch) => self.start_composing(ch, ctx),
            KeyAction::EnterTempEnglish => self.temp_english = true,
            // Everything else has nothing to act on without a composition, and is
            // handed back to the host. The mode keys are the engine's: it owns the
            // Chinese / English, full-width and punctuation bits, and writes them into
            // the frame context itself.
            KeyAction::Backspace
            | KeyAction::CommitHighlighted
            | KeyAction::CommitRaw
            | KeyAction::SelectIndex(_)
            | KeyAction::PageNext
            | KeyAction::PagePrev
            | KeyAction::MoveHighlight(_)
            | KeyAction::MoveCaret(_)
            | KeyAction::ToggleLang
            | KeyAction::ToggleFullWidth
            | KeyAction::TogglePunct
            | KeyAction::Escape
            | KeyAction::Ignore => {}
        }
    }

    /// Routes one key while a composition is live.
    fn on_key_composing(&mut self, action: KeyAction, ctx: &mut Ctx<'_>) {
        match action {
            KeyAction::InputChar(ch) => {
                if !self.push_char(ch, ctx) {
                    return;
                }
                self.refresh(ctx.env);
                self.emit_update(ctx);
            }
            KeyAction::Backspace => self.on_backspace(ctx),
            KeyAction::CommitHighlighted => self.commit_highlighted(ctx),
            KeyAction::CommitRaw => self.commit_raw(ctx),
            KeyAction::SelectIndex(digit) => self.select_digit(digit, ctx),
            KeyAction::PageNext => self.flip_page(PageDir::Next, ctx),
            KeyAction::PagePrev => self.flip_page(PageDir::Prev, ctx),
            KeyAction::MoveHighlight(delta) => self.move_highlight(delta, ctx),
            KeyAction::MoveCaret(delta) => self.move_caret(delta, ctx),
            KeyAction::Escape => self.cancel(ctx),
            KeyAction::EnterTempEnglish => self.enter_temp_english(ctx),
            KeyAction::ToggleLang | KeyAction::ToggleFullWidth | KeyAction::TogglePunct => {
                // The engine has already written the new mode bits into the frame
                // context; the status strip in the window just has to catch up.
                self.emit_frame(ctx.cfg, ctx);
            }
            // The host decides what `Ignore` means, and the session agrees: nothing.
            KeyAction::Ignore => {}
        }
    }

    /// Removes one syllable, or ends the composition when the input runs out.
    fn on_backspace(&mut self, ctx: &mut Ctx<'_>) {
        // `BufferEmpty` means "the input holds nothing after this key": it covers both
        // the press that emptied the input and a press on an input that was already
        // empty, and both end the composition.
        if self.buf.backspace() == BackspaceOutcome::BufferEmpty {
            self.end_session(HideReason::EmptyInput, ctx);
            return;
        }
        self.refresh(ctx.env);
        self.emit_update(ctx);
    }

    /// Turns to another page, if there is one.
    fn flip_page(&mut self, dir: PageDir, ctx: &mut Ctx<'_>) {
        let total = self.candidate_count();
        if self.paging.flip(dir, total) {
            self.emit_frame(ctx.cfg, ctx);
        }
    }

    /// Moves the highlight, turning the page when the move leaves it.
    fn move_highlight(&mut self, delta: i8, ctx: &mut Ctx<'_>) {
        let total = self.candidate_count();
        if self.paging.move_highlight(delta, total) {
            self.emit_frame(ctx.cfg, ctx);
        }
    }

    /// Moves the caret inside the input, which changes the preedit and nothing else.
    fn move_caret(&mut self, delta: i8, ctx: &mut Ctx<'_>) {
        if !self.buf.move_caret(delta) {
            return;
        }
        self.rebuild_preedit();
        self.emit_update(ctx);
    }

    /// Selects the candidate a digit names and commits it.
    fn select_digit(&mut self, digit: u8, ctx: &mut Ctx<'_>) {
        let total = self.candidate_count();
        let Some(index) = self.paging.digit_target(digit, total) else {
            // The digit names nothing on this page, so it is not ours to consume and
            // goes back to the host, which types the character the user pressed.
            return;
        };
        self.paging.highlight = index;
        self.commit_highlighted(ctx);
    }

    /// Enters temporary English mode, taking back the composition if there is one.
    fn enter_temp_english(&mut self, ctx: &mut Ctx<'_>) {
        self.temp_english = true;
        if self.state == SessionState::Composing {
            // Every key now passes through, so a composition left standing would
            // strand the window on screen with nothing able to finish it.
            self.cancel(ctx);
        }
    }

    /// Commits the highlighted candidate.
    fn commit_highlighted(&mut self, ctx: &mut Ctx<'_>) {
        let Some(candidate) = self.highlighted_candidate() else {
            return;
        };
        let text = String::from(candidate.text.as_str());
        let weight_hint = Session::weight_hint(candidate);
        self.set_pending(Some(PendingCommit {
            key: text.clone(),
            weight_hint,
        }));
        self.state = SessionState::Committing;
        ctx.push(Effect::Commit(text));
    }

    /// Commits the raw input instead of a candidate.
    ///
    /// Nothing is recorded in the user's frequencies: the raw input is pinyin, not
    /// the word the user meant, and learning it would teach the dictionary a word
    /// that does not exist.
    fn commit_raw(&mut self, ctx: &mut Ctx<'_>) {
        if self.buf.raw().is_empty() {
            return;
        }
        let text = String::from(self.buf.raw());
        self.set_pending(None);
        self.state = SessionState::Committing;
        ctx.push(Effect::Commit(text));
    }

    /// Takes the composition back without committing anything.
    fn cancel(&mut self, ctx: &mut Ctx<'_>) {
        self.state = SessionState::Cancelling;
        self.clear_input();
        ctx.push(Effect::SetClientPreedit(None));
        ctx.push(Effect::Hide(HideReason::Cancelled));
    }

    /// Ends the composition and hides the window, committing nothing.
    fn end_session(&mut self, reason: HideReason, ctx: &mut Ctx<'_>) {
        self.clear_session();
        ctx.push(Effect::SetClientPreedit(None));
        ctx.push(Effect::Hide(reason));
    }

    /// Handles one event from the candidate window.
    fn on_ui(&mut self, event: UiEvent, ctx: &mut Ctx<'_>) {
        match event {
            // A render receipt carries no business meaning; the probes read it.
            UiEvent::Rendered { .. } => {}
            UiEvent::Select { revision, index, .. } => {
                if !self.accept_revision(revision, ctx) {
                    return;
                }
                self.on_select(index, ctx);
            }
            UiEvent::Hover { revision, index } => {
                if !self.accept_revision(revision, ctx) {
                    return;
                }
                self.on_hover(index, ctx);
            }
            UiEvent::Page { revision, dir } => {
                if !self.accept_revision(revision, ctx) {
                    return;
                }
                if self.state == SessionState::Composing {
                    self.flip_page(dir, ctx);
                }
            }
            UiEvent::Dismiss { revision, .. } => {
                if !self.accept_revision(revision, ctx) {
                    return;
                }
                if self.state == SessionState::Composing {
                    self.cancel(ctx);
                }
            }
        }
    }

    /// Selects the candidate a click names, once.
    ///
    /// A second click on the same frame is dropped: the session is already committing
    /// that candidate, and a duplicate would commit it twice. That is the idempotence
    /// `ASM-12` requires of a click that races the frame it refers to.
    fn on_select(&mut self, index: u16, ctx: &mut Ctx<'_>) {
        if self.state != SessionState::Composing || index >= self.candidate_count() {
            return;
        }
        self.paging.highlight = index;
        self.commit_highlighted(ctx);
    }

    /// Moves the highlight onto the candidate the pointer is over.
    fn on_hover(&mut self, index: Option<u16>, ctx: &mut Ctx<'_>) {
        if self.state != SessionState::Composing {
            return;
        }
        // `None` means the pointer left the grid, which changes no state: the
        // highlight stays where the keyboard put it.
        let Some(index) = index else {
            return;
        };
        if index >= self.candidate_count() || index == self.paging.highlight {
            return;
        }
        self.paging.highlight = index;
        self.emit_frame(ctx.cfg, ctx);
    }

    /// Adopts a reloaded configuration without disturbing the composition.
    ///
    /// Only the page size can change what the session shows, and even that leaves the
    /// input and the candidate list alone: the highlight is moved onto the same word
    /// on the new page grid, which is what keeps it visible. A reload that changes
    /// nothing the session reads produces no effect at all, so reloading an unchanged
    /// file is idempotent.
    fn on_config_reloaded(&mut self, next: &SessionConfig, ctx: &mut Ctx<'_>) {
        if next.max_per_row == self.paging.page_size {
            return;
        }
        let prev_text = self.highlighted_candidate().map(|held| held.text.clone());
        self.paging.set_page_size(next.max_per_row);
        self.paging
            .reconcile(prev_text.as_deref(), &self.decoded.candidates);
        if self.state == SessionState::Composing {
            self.emit_frame(next, ctx);
        }
    }

    /// Takes the composition back when the focus goes away, committing nothing.
    fn on_focus_lost(&mut self, ctx: &mut Ctx<'_>) {
        match self.state {
            SessionState::Composing => self.end_session(HideReason::FocusLost, ctx),
            SessionState::Committing => {
                // The commit has already been handed to the host and is not rolled
                // back; only the composing text goes away with the session. The engine
                // records the `session/commit-on-focus-out` line for this, which is a
                // host-side concern and not something a pure step can emit.
                self.clear_session();
                ctx.push(Effect::SetClientPreedit(None));
            }
            // A cancellation has already hidden the window and cleared the preedit,
            // and an idle session holds nothing to clear.
            SessionState::Idle | SessionState::Cancelling => self.clear_session(),
        }
    }

    /// Ends a cancellation once the host confirms the preedit is gone.
    fn on_preedit_cleared(&mut self) {
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
    fn on_commit_done(&mut self, ctx: &mut Ctx<'_>) {
        let pending = self.take_pending();
        let landed = self.state == SessionState::Committing;
        self.clear_session();
        if let Some(pending) = pending.filter(|_| landed) {
            ctx.push(Effect::RecordUserFreq {
                key: pending.key,
                weight_hint: pending.weight_hint,
            });
        }
        ctx.push(Effect::SetClientPreedit(None));
    }

    /// Starts a composition from the first typed character.
    fn start_composing(&mut self, ch: char, ctx: &mut Ctx<'_>) {
        if !is_input_char(ch) {
            // The key translator only produces input characters, so this is a defect
            // rather than a user-visible failure; the character is not echoed
            // anywhere.
            ctx.push(Effect::Diagnose(ImeError::DecodeInvalidChar { ch, at: 0 }));
            return;
        }
        self.state = SessionState::Composing;
        self.start_new_id();
        self.clear_input();
        self.paging.set_page_size(ctx.cfg.max_per_row);
        if !self.push_char(ch, ctx) {
            self.state = SessionState::Idle;
            return;
        }
        self.refresh(ctx.env);
        self.emit_window(ctx);
    }

    /// Appends one typed character, or reports why it could not be appended.
    fn push_char(&mut self, ch: char, ctx: &mut Ctx<'_>) -> bool {
        let max = usize::from(ctx.cfg.max_raw_len.max(1));
        let len = self.buf.raw().len().saturating_add(ch.len_utf8());
        if len > max {
            ctx.push(Effect::Diagnose(ImeError::DecodeTooLong { len, max }));
            return false;
        }
        match self.buf.push_char(ch) {
            Ok(()) => true,
            Err(err) => {
                ctx.push(Effect::Diagnose(err));
                false
            }
        }
    }

    /// Accepts or drops an event by the revision it refers to.
    ///
    /// A mismatch means the window acted on a frame the session has already replaced
    /// -- a click that raced a keystroke. The event is dropped, and the diagnostic
    /// records both revisions, which is how a race is told from a bug.
    fn accept_revision(&self, revision: u32, ctx: &mut Ctx<'_>) -> bool {
        if revision == self.revision.value() {
            return true;
        }
        ctx.push(Effect::Diagnose(ImeError::UiStaleSelect {
            got: revision,
            current: self.revision.value(),
        }));
        false
    }

    /// Returns the number of candidates the session is holding.
    fn candidate_count(&self) -> u16 {
        u16::try_from(self.decoded.candidates.len()).unwrap_or(u16::MAX)
    }
}

/// Returns `true` for the characters the input buffer accepts.
///
/// The alphabet is the one `InputBuffer::push_char` accepts: ASCII letters, plus the
/// apostrophe that pins a syllable boundary.
fn is_input_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '\''
}
