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
use crate::state::effects::ModeBit;
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
    /// The mode is left by `Return` and by `Escape`, and by nothing else. Leaving changes no
    /// other state and produces no effect, so the key that ended the mode is handed back to
    /// the application as well -- which is what "every key passes through" means.
    ///
    /// This handler is given the action and never the key, and two of the keys that reach
    /// the mode share one action: the space bar and the `Return` key are both
    /// [`KeyAction::CommitHighlighted`] unless `[keys] enter_commit_raw` moves the latter to
    /// [`KeyAction::CommitRaw`]. Reading the mode's exit out of the action therefore ended
    /// the mode on a space bar press, which is the one key the mode exists to pass through.
    /// Only the two actions that exactly one key produces are honoured here: `Escape` comes
    /// from the `Escape` key alone and `CommitRaw` from `Return` alone, so a session driven
    /// by nothing but actions still leaves the mode on the keys that mean to leave it. The
    /// `Return` that arrives as `CommitHighlighted` cannot be told from the space bar here;
    /// the layer that holds the key answers it through [`Session::leave_temp_english`],
    /// which is the same transition reached the other way round.
    fn on_key_temp_english(&mut self, action: KeyAction) {
        if matches!(action, KeyAction::Escape | KeyAction::CommitRaw) {
            self.temp_english = false;
        }
    }

    /// Leaves temporary English mode, reporting whether the mode was on.
    ///
    /// The other half of `Session::on_key_temp_english`, for the layer that holds the key
    /// rather than the action. The two keys that leave the mode are `Return` and `Escape`,
    /// and one of them is translated to the same action as the space bar, so the action
    /// cannot carry the intent: that layer names the key and takes the mode off through
    /// here, clearing the same flag the handler above clears. One transition with two ways
    /// to reach it, rather than two answers to which key leaves the mode.
    ///
    /// Nothing else is touched. The composition a temporary-English session held is already
    /// gone -- entering the mode takes it back -- so there is no input to clear, no frame to
    /// re-send and no effect to produce, and the key that left the mode travels on to the
    /// application like every other key of the mode.
    ///
    /// # Returns
    ///
    /// Whether the mode was on and has now been left. `false` for a session that was not in
    /// the mode, which makes a second call idempotent rather than a second transition.
    ///
    /// # Errors
    ///
    /// None: leaving the mode cannot fail.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn leave_temp_english(&mut self) -> bool {
        core::mem::take(&mut self.temp_english)
    }

    /// Routes one key with nothing composing.
    fn on_key_idle(&mut self, action: KeyAction, ctx: &mut Ctx<'_>) {
        match action {
            KeyAction::InputChar(ch) => self.start_composing(ch, ctx),
            KeyAction::EnterTempEnglish => self.temp_english = true,
            // The full-width and punctuation switches are the engine's bits: the session
            // does not hold them, so all it contributes is the announcement that one
            // moved, which is the feedback a switch has when there is no window to
            // repaint. The engine applied the bit before stepping the session, so the
            // executor reads the new value from its own state.
            KeyAction::ToggleFullWidth => {
                ctx.push(Effect::ModeFlash {
                    bit: ModeBit::FullWidth,
                });
            }
            KeyAction::TogglePunct => {
                ctx.push(Effect::ModeFlash {
                    bit: ModeBit::PunctFull,
                });
            }
            // Everything else has nothing to act on without a composition, and is
            // handed back to the host. The language and script switches are the
            // engine's too, and the plugin's routing table no longer claims a chord
            // for the language switch at all — switching it is the host's own hotkey —
            // so neither action reaches this machine from a key today.
            //
            // The user-word actions have no highlight to act on here for the same reason
            // `CommitHighlighted` does not: there is no candidate list and no session to
            // change.
            KeyAction::Backspace
            | KeyAction::CommitHighlighted
            | KeyAction::CommitRaw
            | KeyAction::SelectIndex(_)
            | KeyAction::PageNext
            | KeyAction::PagePrev
            | KeyAction::MoveHighlight(_)
            | KeyAction::MoveCaret(_)
            | KeyAction::ToggleLang
            | KeyAction::ToggleScript
            | KeyAction::ForgetHighlighted
            | KeyAction::PinHighlighted
            | KeyAction::AddPhrase
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
            KeyAction::ToggleScript => {
                // Switching script keeps the session exactly as it is: the composing
                // input is what the user typed and a display toggle must not discard
                // it (0.4 rule 10). Only the frame is re-sent, which bumps the
                // revision so the window knows the older one is stale.
                self.emit_frame(ctx.cfg, ctx);
            }
            // The pin set does not exist yet, and reporting that through
            // `dict/unsupported` is what the contract reserves that code for; the work that
            // adds the pin set replaces this arm. A silent no-op here would look to the
            // user like a key that does nothing, with no diagnostic to explain it.
            KeyAction::PinHighlighted => {
                ctx.push(Effect::Diagnose(ImeError::Unsupported));
            }
            KeyAction::ForgetHighlighted => self.forget_highlighted(ctx),
            KeyAction::AddPhrase => self.add_phrase(ctx),
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

    /// Drops the highlighted word from the user's learned frequencies.
    ///
    /// The word leaves the candidate list here, and the store is told to forget it through an
    /// effect: the session touches no file (0.4 rule 4), and the store's write path belongs to
    /// the layer that owns it. The composition is left exactly as it is -- the input the user
    /// typed is still what they typed, and the session stays [`SessionState::Composing`] -- so
    /// only the frame is re-sent, under a new revision, which is what takes the word off the
    /// screen.
    ///
    /// A highlight on a candidate the store never learned is still worth asking about: the
    /// store answers whether anything was there, and the host reports `dict/user-word-not-found`
    /// from that answer. Nothing here can tell, because the session does not read the user
    /// database directly.
    fn forget_highlighted(&mut self, ctx: &mut Ctx<'_>) {
        let Some(candidate) = self.highlighted_candidate() else {
            return;
        };
        let key = String::from(candidate.text.as_str());
        if self.drop_candidate_at(self.paging.highlight).is_none() {
            return;
        }
        ctx.push(Effect::ForgetUserWord { key });
        self.emit_frame(ctx.cfg, ctx);
    }

    /// Saves the highlighted candidate as a phrase the user defined.
    ///
    /// The session cannot write the phrase document -- it touches no file (0.4 rule 4)
    /// -- so the input and the candidate's text leave as an effect, and the host layer
    /// appends the row for the next load of that document to pick up. The composition
    /// is left exactly as it is: the user may go on typing, and no frame is re-sent
    /// because nothing the window shows has changed.
    ///
    /// The key is the raw input folded to lower case, which is the alphabet a phrase
    /// key is written and matched in. The input buffer accepts nothing outside that
    /// alphabet, so no character can be lost by folding here.
    fn add_phrase(&mut self, ctx: &mut Ctx<'_>) {
        let Some(candidate) = self.highlighted_candidate() else {
            return;
        };
        let text = String::from(candidate.text.as_str());
        let key = self.buf.raw().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        ctx.push(Effect::AddPhrase { key, text });
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
            UiEvent::Select {
                revision, index, ..
            } => {
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
        self.adopt_page_size(next.max_per_row);
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
        u16::try_from(self.decoded().candidates.len()).unwrap_or(u16::MAX)
    }
}

/// Returns `true` for the characters the input buffer accepts.
///
/// The alphabet is the one `InputBuffer::push_char` accepts: ASCII letters, plus the
/// apostrophe that pins a syllable boundary.
fn is_input_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '\''
}

/// Tests for the arms this file gained on their own.
///
/// The rest of the transition table is covered by the table-driven suite next door;
/// this module holds the phrase a user saves from a highlighted candidate, which is
/// the one action of this file that hands work to a subsystem that exists today, and
/// the reload property that action's card requires of a live session.
#[cfg(test)]
mod tests {
    use ime_types::KeyAction;

    use crate::state::SessionConfig;
    use crate::state::machine::{Effect, Session, SessionEvent, SessionState, step};
    use crate::state::tests::Fixture;

    /// A session with `raw` typed into it, and the sources it was decoded against.
    fn composing(raw: &str) -> (Session, Fixture) {
        let fixture = Fixture::new();
        let mut session = Session::new();
        let cfg = SessionConfig::default();
        for ch in raw.chars() {
            let effects = session.handle_key(KeyAction::InputChar(ch), &cfg, &fixture.env());
            assert!(!effects.is_empty(), "typing {ch:?} composes");
        }
        assert_eq!(session.state, SessionState::Composing);
        (session, fixture)
    }

    /// The text of the candidate the highlight is on.
    fn highlighted(session: &Session) -> String {
        session
            .highlighted_candidate()
            .map(|held| held.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn test_forget_highlighted_preserves_highlight() {
        let (mut session, fixture) = composing("ni");
        let cfg = SessionConfig::default();
        // Off the first candidate, so that "the highlight followed the words" and "the
        // highlight was reset" are different outcomes rather than the same one.
        session.handle_key(KeyAction::MoveHighlight(2), &cfg, &fixture.env());
        assert_eq!(
            session.paging.highlight, 2,
            "the fixture pages enough to move"
        );
        let dropped = highlighted(&session);
        let next = session
            .decoded()
            .candidates
            .get(3)
            .map(|held| held.text.clone())
            .expect("a candidate behind the highlighted one");

        let effects = session.handle_key(KeyAction::ForgetHighlighted, &cfg, &fixture.env());

        assert_eq!(
            session.state,
            SessionState::Composing,
            "the composition survives the removal"
        );
        assert_eq!(session.buf.raw(), "ni", "and so does the input");
        assert_eq!(
            effects.len(),
            2,
            "the store is told and the window is re-sent"
        );
        match effects.first() {
            Some(Effect::ForgetUserWord { key }) => {
                assert_eq!(key.as_str(), dropped.as_str());
            }
            other => panic!("expected a forget effect first, got {other:?}"),
        }
        assert!(
            matches!(effects.last(), Some(Effect::SendFrame(_))),
            "the window draws the list without the word"
        );
        assert!(
            !session
                .decoded()
                .candidates
                .iter()
                .any(|held| held.text == dropped),
            "the word is gone from the candidate list"
        );
        assert_eq!(
            session.paging.highlight, 2,
            "the highlight stayed where the user was looking"
        );
        assert_eq!(
            highlighted(&session),
            next,
            "on the word that took the removed one's place"
        );
        for (position, candidate) in session.decoded().candidates.iter().enumerate() {
            assert_eq!(
                usize::from(candidate.index),
                position + 1,
                "the display numbers still match the positions"
            );
        }
    }

    #[test]
    fn test_forget_highlighted_at_the_end_keeps_a_neighbour() {
        let (mut session, fixture) = composing("ni");
        let cfg = SessionConfig::default();
        let last = session.candidate_count().saturating_sub(1);
        assert!(last > 1, "the fixture offers a list to stand at the end of");
        session.paging.highlight = last;
        let before = session
            .decoded()
            .candidates
            .get(usize::from(last) - 1)
            .map(|held| held.text.clone())
            .expect("a candidate in front of the last one");

        let effects = session.handle_key(KeyAction::ForgetHighlighted, &cfg, &fixture.env());

        assert!(matches!(
            effects.first(),
            Some(Effect::ForgetUserWord { .. })
        ));
        assert_eq!(
            highlighted(&session),
            before,
            "the highlight falls back onto the word in front of the one removed"
        );
        assert!(
            session.paging.highlight > 0,
            "and not back to the top of the list"
        );
    }

    #[test]
    fn test_forget_highlighted_arm_does_nothing_without_a_composition() {
        let fixture = Fixture::new();
        let mut session = Session::new();
        let effects = session.handle_key(
            KeyAction::ForgetHighlighted,
            &SessionConfig::default(),
            &fixture.env(),
        );
        assert!(effects.is_empty(), "there is no highlight to forget");
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_add_phrase_arm_records_the_highlighted_candidate() {
        let (mut session, fixture) = composing("ni");
        let expected = highlighted(&session);
        assert!(
            !expected.is_empty(),
            "a composition has a candidate to save"
        );

        let effects = session.handle_key(
            KeyAction::AddPhrase,
            &SessionConfig::default(),
            &fixture.env(),
        );
        assert_eq!(effects.len(), 1, "the arm emits exactly one effect");
        assert!(
            matches!(effects.first(), Some(Effect::AddPhrase { .. })),
            "the arm saves the phrase rather than reporting it unsupported"
        );
        if let Some(Effect::AddPhrase { key, text }) = effects.first() {
            assert_eq!(key.as_str(), "ni", "the key is the input the user typed");
            assert_eq!(text.as_str(), expected);
        }
    }

    #[test]
    fn test_add_phrase_arm_leaves_the_composition_alone() {
        let (mut session, fixture) = composing("ni");
        let before = session.decoded().clone();
        let raw = String::from(session.buf.raw());
        let id = session.id;

        let effects = session.handle_key(
            KeyAction::AddPhrase,
            &SessionConfig::default(),
            &fixture.env(),
        );
        assert_eq!(session.state, SessionState::Composing);
        assert_eq!(session.id, id, "the composition is the same one");
        assert_eq!(session.buf.raw(), raw.as_str());
        assert_eq!(
            session.decoded(),
            &before,
            "the candidate list is untouched"
        );
        assert_eq!(effects.len(), 1, "nothing is re-sent to the window");
    }

    #[test]
    fn test_add_phrase_arm_does_nothing_without_a_composition() {
        let fixture = Fixture::new();
        let mut session = Session::new();
        let effects = session.handle_key(
            KeyAction::AddPhrase,
            &SessionConfig::default(),
            &fixture.env(),
        );
        assert!(effects.is_empty(), "there is no highlight to save");
        assert_eq!(session.state, SessionState::Idle);
    }

    #[test]
    fn test_reload_preserves_active_session() {
        // A reload of the configuration -- the phrase table included, which is swapped
        // in the layer that owns it -- never resets a composition in progress (0.4
        // rule 10): the input, the candidates and the session's own identity all
        // survive it, and only the frame is re-sent under the new values.
        let (mut session, fixture) = composing("ni");
        let cfg = SessionConfig::default();
        let before = session.decoded().clone();
        let raw = String::from(session.buf.raw());
        let id = session.id;

        let next = SessionConfig {
            max_per_row: 7,
            ..SessionConfig::default()
        };
        let effects = step(
            &mut session,
            SessionEvent::ConfigReloaded(next),
            &cfg,
            &fixture.env(),
        );
        assert_eq!(session.state, SessionState::Composing);
        assert_eq!(session.id, id, "the composition is not restarted");
        assert_eq!(session.buf.raw(), raw.as_str());
        assert_eq!(session.decoded(), &before);
        assert_eq!(effects.len(), 1, "only the frame is re-sent");
        assert!(matches!(effects.first(), Some(Effect::SendFrame(_))));
    }
}
