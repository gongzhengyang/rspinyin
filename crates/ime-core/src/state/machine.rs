//! The input session state machine: the single source of truth between the engine
//! and the candidate window.
//!
//! Responsibility: decide, for one `(state, event)` pair, what the session becomes
//! and what the host must do about it. This file owns the session, its states, its
//! events and the effects; the transitions themselves live next door in the
//! `transitions` module, and the design's transition table is implemented row by row
//! there, with a case in the test suite for every row.
//!
//! Boundaries: [`step`] is a pure function of its arguments. It touches no file, no
//! clock, no environment and no global state; the dictionary, the user's frequencies
//! and the language model arrive through the trait objects of [`SessionEnv`], and
//! everything the host must do -- post a frame, commit a string, record a frequency
//! -- leaves as an [`Effect`] the caller executes. That is what lets a whole session
//! be replayed in a test from literals, and what keeps the host thread free of
//! anything but executing effects.
//!
//! # Effects
//!
//! The effects of one step are ordered, and the order is part of the contract:
//!
//! - [`Effect::Show`] precedes [`Effect::SendFrame`] because the window's own state
//!   machine drops a frame that arrives while it is hidden.
//! - [`Effect::UpdatePreedit`] is emitted while a composition is alive and carries
//!   the preedit the window header and the application's preedit area show.
//! - [`Effect::SetClientPreedit`] with `None` is emitted when a composition ends, so
//!   the application's preedit area is cleared even though no frame follows.
//! - [`Effect::Commit`] is emitted once, on the transition into
//!   [`SessionState::Committing`]. The hide that follows a commit is posted by the
//!   executor of that effect, so emitting a [`Effect::Hide`] here as well would post
//!   it twice on an ordered, non-droppable queue.
//!
//! At most [`MAX_EFFECTS`] effects leave one step, which is what the inline capacity
//! of the returned `SmallVec` covers: a step that changes what the window shows emits
//! three, and no path emits more.
//!
//! # Indexing
//!
//! [`UiEvent::Select`] and [`UiEvent::Hover`] carry a zero-based *global* candidate
//! index -- the same numbering [`Paging::highlight`] uses -- so that a click and a
//! number key name a candidate the same way. [`KeyAction::SelectIndex`] is the digit
//! the user pressed instead: one-based, and counted within the page on show. The frame
//! the window draws carries the highlight already converted to that page's own
//! numbering ([`UiFrame::highlight`]), so the view never repeats the conversion.
//!
//! # Concurrency
//!
//! A session is owned by the host thread and is never shared: decoding is serial per
//! session (`ASM-11`), and the UI thread reads only the immutable [`UiFrame`]
//! snapshots that leave as effects. Nothing here is `Sync`, and nothing here needs
//! to be.

use ime_types::{
    Anchor, Candidate, DecodeFlags, DecodeRequest, DecodeResult, ImeError, KeyAction,
    LanguageModel, LayoutHint, Lexicon, Placement, Preedit, Revision, SchemeId, SessionId,
    StatusStrip, UiEvent, UiFrame, UserFreqSource,
};

use crate::input::InputBuffer;
use crate::preedit::build_preedit_into;
use crate::segment::SyllableDag;
use crate::state::SessionConfig;
use crate::state::outcome::{clear_result, empty_preedit, renumber, write_boundaries};
use crate::state::paging::Paging;
use crate::state::scheme::SchemeSession;
use crate::viterbi::{DecodeScratch, Decoder};

pub use crate::state::effects::{AnchorHint, Effect, Effects, MAX_EFFECTS};
pub use crate::state::frame::FrameContext;

/// Where a session is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// No composition: the plugin holds no input and the window is hidden.
    Idle,
    /// The user is typing; the input buffer, the candidates and the window are live.
    Composing,
    /// The composition is being taken back: the preedit is cleared, the window is
    /// hidden, and nothing is committed. The session leaves this state when the host
    /// confirms the preedit is gone ([`SessionEvent::PreeditCleared`]).
    Cancelling,
    /// A commit has been handed to the host and the session waits for it to land
    /// ([`SessionEvent::CommitDone`]) before it returns to [`SessionState::Idle`].
    Committing,
}

/// One composing session and everything the machine needs to decide the next state.
///
/// The fields are public because the engine reads them: it builds a host-side
/// preedit from the buffer, decides what `keyEvent` returns from the state, and
/// reports session duration from the buffer's stamp. Every mutation goes through
/// [`step`], which keeps the fields consistent with one another.
#[derive(Debug)]
pub struct Session {
    /// Identity of the current composition, for diagnostics only. A fresh id is
    /// allocated on every transition out of [`SessionState::Idle`].
    pub id: SessionId,
    /// Where the session is in its life.
    pub state: SessionState,
    /// What the user has typed, and where the caret sits in it.
    pub buf: InputBuffer,
    /// The decode workspace of this session: the graph a decode builds, its beams and
    /// drafts, and the candidate list it answers with, all kept across keystrokes.
    ///
    /// It is the one set of decode buffers a session owns; [`Session::decoded`] is the
    /// read side of it. The workspace is borrowed mutably for the length of a decode,
    /// which is exactly the concurrency the decoder promises -- decoding is serial per
    /// session.
    scratch: DecodeScratch,
    /// The candidate list of the refresh before the last one, parked outside the
    /// workspace.
    ///
    /// [`Session::refresh`] reads the text the highlight follows out of it as a borrow,
    /// while the decode below needs the workspace mutably; holding one answer here is
    /// what lets that borrow and the decode exist at the same time without copying the
    /// text. The buffers it carries go back into the workspace before the next decode,
    /// so the decode writes into storage that is already sized.
    previous: DecodeResult,
    /// The request the last decode was built from, refilled per keystroke rather than
    /// built again, which is what keeps a keystroke from allocating the input string.
    request: DecodeRequest,
    /// The scheme side of the session: the layout the keystrokes follow, the rewrite
    /// that turns them into the spelling the segmentation layer reads, and the
    /// syllable grid that rewrite reports in the keystrokes' own offsets.
    scheme: SchemeSession,
    /// A diagnostic a rewrite raised, waiting for the step that reports it.
    ///
    /// The rewrite runs inside [`Session::refresh`], which the transitions call with
    /// the environment alone -- there is no context there to push an effect onto --
    /// so the error is parked and [`step`], the one entry point every event goes
    /// through, flushes it into that step's effect list.
    scheme_error: Option<ImeError>,
    /// Which page is shown and which candidate is highlighted.
    pub paging: Paging,
    /// Revision of the frame the window holds. It advances with every frame, and
    /// every UI event that carries a different revision is dropped as stale.
    pub revision: Revision,
    /// Temporary English mode: every key reaches the application until Enter or
    /// Escape leaves the mode. The engine reads this to decide its own return value,
    /// which is why the flag lives here rather than in the engine.
    pub temp_english: bool,
    /// The preedit of the current input, rebuilt on every change and reused as a
    /// buffer so that a keystroke does not reallocate it.
    preedit: Preedit,
    /// Where the window goes and what its status strip says. The engine writes this
    /// whenever the host tells it the cursor moved or a mode bit changed.
    ctx: FrameContext,
    /// The word and weight a commit in flight should record once it lands.
    pending: Option<PendingCommit>,
    /// Id the next composition will take.
    next_session_id: u64,
}

/// A commit that has been handed to the host but has not landed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingCommit {
    /// The word the user chose, which is what the frequency is recorded against.
    pub(super) key: String,
    /// How strongly to weigh the record; see `Session::weight_hint`.
    pub(super) weight_hint: u16,
}

/// Everything the session reads that it does not own.
///
/// Grouped into one struct rather than threaded through [`step`] as four parameters,
/// which also gives the tests a single place to swap in their in-memory doubles.
pub struct SessionEnv<'a> {
    /// The decoder, built once and shared by every session.
    pub decoder: &'a Decoder,
    /// The dictionary the decode reads.
    pub lexicon: &'a dyn Lexicon,
    /// The user's own frequencies, read while ranking and written after a commit.
    pub user_freq: &'a dyn UserFreqSource,
    /// The language model the ranking is scored with.
    pub lm: &'a dyn LanguageModel,
}

/// What the session is reacting to.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    /// A key, already translated into a semantic action by the host layer.
    Key(KeyAction),
    /// Something the user did to the candidate window.
    Ui(UiEvent),
    /// The configuration changed. The session keeps its input and its candidates and
    /// adopts only the behaviour the new values change.
    ConfigReloaded(SessionConfig),
    /// The application or the host took the focus away.
    FocusLost,
    /// The session is being dropped: the plugin is shutting down, or the input
    /// context is going away. Nothing is committed.
    Reset,
    /// The host confirmed that the preedit is cleared, which ends a cancellation.
    ///
    /// The design's transition table lists this as the event that takes a session out
    /// of [`SessionState::Cancelling`]; the task card's sketch of the event enum
    /// leaves it out, and the table cannot be implemented without it.
    PreeditCleared,
    /// The host finished committing the text a [`Effect::Commit`] asked for.
    ///
    /// Like [`SessionEvent::PreeditCleared`], this is a row of the design's table
    /// rather than a variant of the card's sketch.
    CommitDone,
}

impl Default for Session {
    /// An idle session with no composition, no candidates and no revision.
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Creates an idle session whose first composition takes the id `1`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new() -> Self {
        Self::new_with_counter(1)
    }

    /// Creates an idle session whose first composition takes the id `next`.
    ///
    /// The engine holds one session per input context, and gives each of them a
    /// disjoint range so that a session id identifies a composition across the whole
    /// process and not only within one context.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new_with_counter(next: u64) -> Self {
        Self {
            id: SessionId::new(0),
            state: SessionState::Idle,
            buf: InputBuffer::new(),
            scratch: DecodeScratch::new(),
            previous: empty_result(),
            request: DecodeRequest::new(""),
            scheme: SchemeSession::default(),
            scheme_error: None,
            paging: Paging::new(),
            revision: Revision::new(0),
            temp_english: false,
            preedit: empty_preedit(),
            ctx: FrameContext::default(),
            pending: None,
            next_session_id: next,
        }
    }

    /// Records where the window goes and what its status strip says.
    ///
    /// The engine calls this whenever the host reports a new cursor rectangle or a
    /// mode bit changes, before stepping the event that follows. It emits no effect:
    /// the frame that carries the values is the one the next step produces.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn set_frame_context(&mut self, anchor: Anchor, status: StatusStrip) {
        self.ctx = FrameContext { anchor, status };
    }

    /// Returns the frame context the engine last wrote.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn frame_context(&self) -> &FrameContext {
        &self.ctx
    }

    /// Returns the preedit of the current input.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn preedit(&self) -> &Preedit {
        &self.preedit
    }

    /// Returns the request the last decode was built from.
    ///
    /// The engine reads it to see which switches, and which double-pinyin layout, the
    /// answer it is about to show was produced under -- without re-deriving them from
    /// a configuration the session may have adopted later than the caller read it.
    /// `raw` is the spelling the decode actually read: for a scheme session that is
    /// the layout's full-pinyin spelling, not the keystrokes in [`Session::buf`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn request(&self) -> &DecodeRequest {
        &self.request
    }

    /// Returns the segmentation graph of the input in [`Session::buf`].
    ///
    /// The graph describes the input the last refresh segmented, and is empty while the
    /// session is idle.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn dag(&self) -> &SyllableDag {
        self.scratch.dag()
    }

    /// Returns the candidate list and the cut of the winning path, as the last decode
    /// left them.
    ///
    /// The list is empty while the session is idle, which is what a session that has
    /// composed nothing holds.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn decoded(&self) -> &DecodeResult {
        self.scratch.result()
    }

    /// Returns the candidate the highlight is on, or `None` when there is none.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn highlighted_candidate(&self) -> Option<&Candidate> {
        self.decoded()
            .candidates
            .get(usize::from(self.paging.highlight))
    }

    /// Handles one key action.
    ///
    /// Shorthand for stepping [`SessionEvent::Key`], for the host layer that has
    /// nothing else to feed the session.
    ///
    /// # Errors
    ///
    /// None. A key the session cannot use is answered with an empty effect list, and
    /// the caller hands it back to the host.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn handle_key(
        &mut self,
        action: KeyAction,
        cfg: &SessionConfig,
        env: &SessionEnv<'_>,
    ) -> Effects {
        step(self, SessionEvent::Key(action), cfg, env)
    }

    /// Takes the commit in flight, if there is one.
    pub(super) fn take_pending(&mut self) -> Option<PendingCommit> {
        self.pending.take()
    }

    /// Puts a commit in flight, to be recorded once the host confirms it landed.
    pub(super) fn set_pending(&mut self, pending: Option<PendingCommit>) {
        self.pending = pending;
    }

    /// Allocates the id of a new composition and advances the counter behind it.
    pub(super) fn start_new_id(&mut self) {
        self.id = SessionId::new(self.next_session_id);
        self.next_session_id = self.next_session_id.saturating_add(1);
    }

    /// Empties the input, the candidates, the paging and the preedit.
    ///
    /// The state the session is in is left alone, because the callers that end a
    /// composition and the callers that take one back set it themselves.
    pub(super) fn clear_input(&mut self) {
        self.buf.clear();
        // The candidate list goes with the input: taking the answer out of the workspace
        // leaves it holding an empty one, which is what `Session::decoded` reports
        // between compositions. The answer parked in `previous` is emptied the same way,
        // so that no list of a finished composition stays in the session -- its buffers
        // stay, and the next composition's first decode writes into them.
        let _ = self.scratch.take_result();
        clear_result(&mut self.previous);
        self.paging.reset();
        self.preedit = empty_preedit();
        self.pending = None;
        // An empty input has no graph, which is exactly what the session should hold
        // between compositions; the buffers keep their capacity, so the next build
        // does not reallocate.
        self.scratch.clear_dag();
    }

    /// Returns how strongly a commit of `candidate` should weigh in the user's
    /// frequencies.
    ///
    /// The only per-candidate signal the session has is how many syllables the word
    /// spans, and a longer word is the stronger statement about what the user meant;
    /// a commit that spans nothing, such as the pass-through candidate, still counts
    /// as one.
    pub(super) fn weight_hint(candidate: &Candidate) -> u16 {
        candidate.consumed_syllables.max(1)
    }

    /// Ends the composition without committing, emptying the buffer and the graph.
    pub(super) fn clear_session(&mut self) {
        self.state = SessionState::Idle;
        self.clear_input();
    }

    /// Re-segments and re-decodes the current input, then repairs everything derived.
    ///
    /// The candidate text the highlight is on is read before the decode and handed to
    /// [`Paging::reconcile`], so a keystroke moves the highlight with the word the
    /// user had chosen rather than resetting it to the first candidate.
    ///
    /// That text is read as a borrow rather than copied: the copy this used to take
    /// existed only to outlive the decode below, which needs the workspace mutably, and
    /// the borrow ends at the `reconcile` call. The list is parked in
    /// [`Session::previous`] for the length of the decode, and the buffers of the answer
    /// before it go back into the workspace, so that the decode writes into storage that
    /// is already sized. The two fields are named directly instead of through
    /// [`Session::highlighted_candidate`], because that accessor borrows the whole
    /// session and the mutable borrow `reconcile` takes of the paging state would
    /// collide with it.
    ///
    /// A double-pinyin layout is applied here, before segmentation: the request carries
    /// the layout's full-pinyin spelling of the keystrokes rather than the keystrokes
    /// themselves, which is what leaves the graph, the lattice, the Viterbi pass and
    /// the preedit builder untouched. The buffer keeps what the user typed, and its
    /// syllable grid is written from the rewrite's own alignment, so Backspace and the
    /// caret stay on the units the user pressed.
    pub(super) fn refresh(&mut self, env: &SessionEnv<'_>) {
        let parked = core::mem::replace(&mut self.previous, self.scratch.take_result());
        self.scratch.recycle(parked);
        self.request.raw.clear();
        self.request.raw.push_str(self.buf.raw());
        let rewritten = self.rewrite_input();
        // Read after the rewrite rather than before: `rewrite_input` takes the whole
        // session mutably, so a borrow of the previous candidate taken first would have to
        // be released before it runs. The rewrite touches the request and the scheme and
        // neither the previous result nor the paging state, so the word the highlight was
        // on is the same either way.
        let prev = self
            .previous
            .candidates
            .get(usize::from(self.paging.highlight))
            .map(|held| held.text.as_str());
        self.request.scheme = if rewritten {
            self.scheme.id()
        } else {
            SchemeId::FULL
        };
        self.request.flags.set(DecodeFlags::SHUANGPIN, rewritten);
        env.decoder.decode_into(
            &mut self.scratch,
            &self.request,
            env.lexicon,
            env.user_freq,
            env.lm,
        );
        self.paging
            .reconcile(prev, &self.scratch.result().candidates);
        if rewritten {
            // The rewrite walked the keystrokes in order, so it is the only pass that
            // can say where a scheme syllable started; the graph describes the
            // rewritten spelling and its boundaries are the layout's, not the user's.
            self.buf.set_boundaries(self.scheme.grid());
        } else {
            // The graph comes out of the workspace the decode just wrote, so the input
            // is segmented once per keystroke rather than twice: `decode_into` builds
            // it, and this only maps its syllable boundaries back onto the raw input.
            write_boundaries(&mut self.buf, self.scratch.dag());
        }
        build_preedit_into(&self.buf, self.scratch.dag(), &mut self.preedit);
    }

    /// Rewrites the request's input through the active layout.
    ///
    /// # Returns
    ///
    /// `true` when [`Session::request`] now carries the layout's full-pinyin spelling
    /// of the keystrokes. `false` when the input is decoded exactly as typed, which is
    /// full pinyin and a layout this build does not implement.
    fn rewrite_input(&mut self) -> bool {
        if !self.scheme.is_active() {
            return false;
        }
        // The scheme key alphabet is lower case while the buffer keeps what the user
        // pressed, so the copy the rewrite reads is folded here. Folding rewrites byte
        // values and moves no offset, so the grid that comes back still names
        // positions in the keystrokes in `Session::buf`.
        self.request.raw.make_ascii_lowercase();
        if let Err(err) = self.scheme.rewrite(self.request.raw.as_str()) {
            // A layout a newer build wrote. The input is read as typed and the host is
            // told why, which is what the contract asks of a scheme number this build
            // cannot honour: the user keeps typing rather than losing the composition
            // to a configuration key.
            self.scheme_error = Some(err);
            self.request.raw.clear();
            self.request.raw.push_str(self.buf.raw());
            return false;
        }
        self.request.raw.clear();
        self.request.raw.push_str(self.scheme.text());
        if self.request.raw.is_empty() {
            // A stream the layout has nothing to say about rewrites to nothing: a lone
            // `v` -- the key every `zh` syllable starts on -- is dropped rather than
            // carried, because the normalizer would fold it onto `ü`. Reading the
            // keystrokes as typed instead keeps the window from showing a candidate
            // with no text, which looks to the user like an input method that has
            // stopped answering.
            self.request.raw.push_str(self.buf.raw());
            return false;
        }
        true
    }

    /// Rebuilds the preedit from the current input and graph.
    pub(super) fn rebuild_preedit(&mut self) {
        build_preedit_into(&self.buf, self.scratch.dag(), &mut self.preedit);
    }

    /// Adopts a new page size and puts the highlight back on the word it was on.
    ///
    /// The candidate list is read from the workspace and the highlight is written into
    /// the paging state, which are different fields of the session; the list stays
    /// borrowed while the paging state is written, which is why this is one call here
    /// rather than two calls from the transition. A transition reaches the list only
    /// through [`Session::decoded`], and that accessor borrows the whole session, so it
    /// cannot be held across a mutable borrow of the paging state.
    pub(super) fn adopt_page_size(&mut self, size: u8) {
        let prev = self
            .scratch
            .result()
            .candidates
            .get(usize::from(self.paging.highlight))
            .map(|held| held.text.as_str());
        self.paging.set_page_size(size);
        self.paging
            .reconcile(prev, &self.scratch.result().candidates);
    }

    /// Drops the candidate at `index` and moves the highlight onto the word that takes its
    /// place, returning the text of the word that was dropped.
    ///
    /// The list is the session's own: the store cannot answer until the effect that forgets
    /// the word has run, and the user has to see the candidate go on the keystroke, so the
    /// removal happens here as well as there. Renumbering keeps the invariant every decode
    /// establishes -- a candidate's display number is its position plus one -- because the
    /// number keys and the click indices are read against it.
    ///
    /// [`Paging::reconcile`] is reused rather than the highlight being moved by hand: it
    /// already looks a word up by its text and turns to the page that holds it, which is
    /// what keeps the highlight on a word the user was looking at instead of sending it back
    /// to the first candidate. The anchor is the word that took the removed one's place, or
    /// the word before it when the last candidate went -- there is no list position left to
    /// stand on at the end.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn drop_candidate_at(&mut self, index: u16) -> Option<String> {
        let mut result = self.scratch.take_result();
        let position = usize::from(index);
        let dropped = result
            .candidates
            .get(position)
            .map(|held| held.text.clone());
        let anchor = match dropped {
            Some(_) => {
                result.candidates.remove(position);
                renumber(&mut result.candidates);
                result
                    .candidates
                    .get(position)
                    .or_else(|| {
                        position
                            .checked_sub(1)
                            .and_then(|before| result.candidates.get(before))
                    })
                    .map(|held| held.text.clone())
            }
            None => None,
        };
        self.scratch.recycle(result);
        if dropped.is_some() {
            self.paging
                .reconcile(anchor.as_deref(), &self.scratch.result().candidates);
        }
        dropped
    }

    /// Builds the frame the window draws from the session's current state.
    ///
    /// The frame leaves the session for the UI thread, so everything in it is owned and
    /// this call allocates: the preedit text and its span list, the page's candidates,
    /// and the mode strip. None of those copies can be dropped without changing
    /// [`UiFrame`], which is frozen -- a frame built out of borrows would outlive the
    /// session state it points at. What the build does *not* do is copy anything twice:
    /// the preedit it snapshots is the buffer [`build_preedit_into`] refills in place
    /// across keystrokes, and the candidates are one clone of the page the decoder
    /// produced, taken with an exact-sized allocation.
    fn build_frame(&self, cfg: &SessionConfig, revision: Revision) -> UiFrame {
        let candidates = &self.decoded().candidates;
        let total = u16::try_from(candidates.len()).unwrap_or(u16::MAX);
        let start = usize::from(self.paging.page_start()).min(candidates.len());
        let end = usize::from(self.paging.page_end(total)).min(candidates.len());
        // Page-local position of the keyboard highlight, or `None` when the page holds
        // no candidate for it. The view treats `None` as "hide the ring", never as
        // "keep the previous one": a ring that outlives its candidate is the defect
        // this field exists to prevent. The conversion runs here, on the engine side of
        // the boundary, so the frame and the candidate slice it carries share one basis
        // and the view never re-derives the position.
        let highlight = self.paging.highlight_position_in_page(total);
        UiFrame {
            revision: revision.value(),
            preedit: self.preedit.clone(),
            candidates: candidates[start..end].to_vec(),
            page: self.paging.page_state(total),
            status: self.ctx.status.clone(),
            anchor: self.ctx.anchor,
            layout: LayoutHint {
                max_per_row: self.paging.page_size,
                show_annotation: cfg.show_annotation,
                max_width_dp: cfg.max_width_dp,
            },
            highlight,
        }
    }

    /// Emits the preedit, the show and the frame that put a composition on screen.
    ///
    /// The show precedes the frame because the window drops a frame that arrives
    /// while it is hidden, so the two cannot be reordered.
    ///
    /// The preedit the application's preedit area is told to show is taken from the
    /// frame rather than read from the session a second time, so the two carriers of
    /// the composing text are the same value: the window header and the application
    /// cannot disagree about it, which two independent reads could only guarantee by
    /// construction.
    pub(super) fn emit_window(&mut self, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(ctx.cfg, revision));
        ctx.push(Effect::UpdatePreedit(frame.preedit.clone()));
        ctx.push(Effect::Show(AnchorHint {
            revision,
            placement: Placement::Auto,
        }));
        ctx.push(Effect::SendFrame(frame));
    }

    /// Emits the preedit and the frame after a change to the composing text.
    ///
    /// The application's preedit comes from the frame, as in [`Session::emit_window`].
    pub(super) fn emit_update(&mut self, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(ctx.cfg, revision));
        ctx.push(Effect::UpdatePreedit(frame.preedit.clone()));
        ctx.push(Effect::SendFrame(frame));
    }

    /// Emits the frame alone, after a change that leaves the composing text as it was.
    ///
    /// `cfg` is passed explicitly rather than read from the context because a
    /// configuration reload repaints the window under the *new* values.
    pub(super) fn emit_frame(&mut self, cfg: &SessionConfig, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(cfg, revision));
        ctx.push(Effect::SendFrame(frame));
    }
}

/// Everything one step reads, and the effect list it fills.
///
/// Grouped so that a transition takes the session and this context and nothing else:
/// the configuration, the injected data sources and the accumulating effects are all
/// reachable from here, which keeps every handler inside the parameter budget and
/// gives the transitions one place to hand their results back.
pub(super) struct Ctx<'a> {
    /// The configuration in force for this step.
    pub(super) cfg: &'a SessionConfig,
    /// The decoder, the dictionary, the user's frequencies and the model.
    pub(super) env: &'a SessionEnv<'a>,
    /// The effects the transition has produced so far, in the order they must run.
    out: Effects,
}

impl<'a> Ctx<'a> {
    /// Builds a context for one step.
    fn new(cfg: &'a SessionConfig, env: &'a SessionEnv<'a>) -> Self {
        Self {
            cfg,
            env,
            out: Effects::new(),
        }
    }

    /// Adds one effect to the end of the list.
    pub(super) fn push(&mut self, effect: Effect) {
        self.out.push(effect);
    }

    /// Returns the effects the step produced, in order.
    fn into_effects(self) -> Effects {
        self.out
    }
}

/// Advances a session by one event, and returns what the host must do about it.
///
/// `cfg` is the configuration in force *before* this event; the one a
/// [`SessionEvent::ConfigReloaded`] carries is the one after it. The caller passes
/// the current configuration on every step, because the engine reads it from a
/// shared slot rather than holding a copy.
///
/// The scheme keys of `cfg` are adopted while the session is idle and frozen for as
/// long as a composition is live. A reload that switches the layout, or turns the
/// mixed-input reading on or off, therefore changes nothing about the input the user
/// has already typed -- re-reading it would move the candidate list under the caret
/// -- and answers the next composition instead (0.4 rule 10).
///
/// # Errors
///
/// None. Anything the session cannot use -- a stale click, a key with nothing to act
/// on -- is answered with an empty effect list, or with a [`Effect::Diagnose`] when
/// the host should know about it.
///
/// # Panics
///
/// Never panics. The debug assertion at the end fires only if a transition emitted
/// more effects than the frame budget allows, which is an internal defect and not a
/// reachable state.
pub fn step(
    sess: &mut Session,
    ev: SessionEvent,
    cfg: &SessionConfig,
    env: &SessionEnv<'_>,
) -> Effects {
    if sess.state == SessionState::Idle {
        sess.scheme.latch(cfg.scheme, cfg.keep_full_pinyin);
    }
    let mut ctx = Ctx::new(cfg, env);
    sess.on_event(ev, &mut ctx);
    let mut out = ctx.into_effects();
    // A rewrite that raised a diagnostic ran inside `Session::refresh`, which the
    // transitions call with the environment alone; it is reported here, with the
    // effects of the same step, so that a configuration the session cannot honour is
    // never silent.
    if let Some(err) = sess.scheme_error.take() {
        out.push(Effect::Diagnose(err));
    }
    debug_assert!(
        out.len() <= MAX_EFFECTS,
        "one step produced more effects than the frame budget allows"
    );
    out
}

/// Returns the decode result of a session that has decoded nothing.
pub(super) fn empty_result() -> DecodeResult {
    DecodeResult {
        candidates: Vec::new(),
        segments: Vec::new(),
        degraded: false,
    }
}
