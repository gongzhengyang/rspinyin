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
//! the user pressed instead: one-based, and counted within the page on show.
//!
//! # Concurrency
//!
//! A session is owned by the host thread and is never shared: decoding is serial per
//! session (`ASM-11`), and the UI thread reads only the immutable [`UiFrame`]
//! snapshots that leave as effects. Nothing here is `Sync`, and nothing here needs
//! to be.

use ime_types::{
    Anchor, Candidate, DecodeRequest, DecodeResult, ImeError, KeyAction, LanguageModel, LayoutHint,
    Lexicon, Placement, Preedit, RectI, Revision, ScreenId, SessionId, StatusStrip, UiEvent,
    UiFrame, UserFreqSource,
};
use smallvec::SmallVec;

use crate::input::InputBuffer;
use crate::preedit::build_preedit_into;
use crate::segment::{HINT_INLINE_BOUNDARIES, SyllableDag};
use crate::state::SessionConfig;
use crate::state::boundaries::map_to_raw;
use crate::state::paging::Paging;
use crate::viterbi::{DecodeScratch, Decoder};

/// Most effects one [`step`] produces.
///
/// The returned `SmallVec` holds this many inline, so a step never allocates for its
/// effects. A step that puts a session on screen emits three -- the preedit, the
/// show and the frame -- and no path in this module emits a fourth.
pub const MAX_EFFECTS: usize = 4;

/// The set of effects one step produces.
pub type Effects = SmallVec<[Effect; MAX_EFFECTS]>;

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
    /// Segmentation graph of the input in [`Session::buf`]. It describes the input
    /// the last refresh segmented, and is emptied when a session ends.
    dag: SyllableDag,
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

/// The parts of a frame the session cannot derive from its own state.
///
/// The cursor rectangle and the mode bits belong to the host, not to the session, so
/// the engine copies them here before stepping and the session writes them into
/// every frame it emits. They are held rather than passed to [`step`] because the
/// anchor has to survive between steps: a frame emitted on a keystroke the host did
/// not tell us about must still place the window where the caret is.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameContext {
    /// Where the window should appear, as far as the host has reported it.
    pub anchor: Anchor,
    /// Mode label, full-width and punctuation flags, and the read-only marker.
    pub status: StatusStrip,
}

impl Default for FrameContext {
    /// The context of a session that has not been told anything: an empty cursor
    /// rectangle on the first screen, and a blank status strip.
    fn default() -> Self {
        Self {
            anchor: Anchor {
                cursor: RectI {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
                screen: ScreenId::new(0),
                scale: 1.0,
                placement: Placement::Auto,
            },
            status: StatusStrip::default(),
        }
    }
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

/// What the host must do after a step.
///
/// The machine never does any of this itself: it describes the work so that the
/// caller can execute it on the thread that owns the channel, and so that a test can
/// assert on it without a host.
#[derive(Debug)]
pub enum Effect {
    /// Replace the composing text. The executor applies the client-preedit policy:
    /// with it on, the text goes to the application's preedit area, and with it off
    /// the area is cleared.
    UpdatePreedit(Preedit),
    /// Show the window with a new frame.
    SendFrame(Box<UiFrame>),
    /// Show the window, with the anchor the host resolves from [`AnchorHint`].
    Show(AnchorHint),
    /// Hide the window and say why.
    Hide(ime_types::HideReason),
    /// Commit this text to the application.
    Commit(String),
    /// Record one commit in the user's frequencies.
    RecordUserFreq {
        /// The word the user chose.
        key: String,
        /// How strongly to weigh it.
        weight_hint: u16,
    },
    /// Save the highlighted candidate as a phrase the user defined on purpose.
    ///
    /// The session holds no phrase table and writes no file, so the pair leaves as an
    /// effect and the host layer appends it to the user's phrase document, where the
    /// next load of that document picks it up. The composition is not touched: the
    /// candidate list and the preedit stay exactly as they were, and nothing is
    /// re-sent to the window.
    AddPhrase {
        /// The input the user typed, folded to the lower-case key alphabet of a phrase.
        key: String,
        /// The text of the candidate the highlight was on, which becomes the phrase.
        text: String,
    },
    /// Report a diagnostic.
    ///
    /// The payload is built from the stable `domain/action/reason` codes and from
    /// lengths and counts; it never carries the characters the user typed, which is
    /// what keeps a diagnostic log out of the user's input.
    Diagnose(ImeError),
    /// Set or clear the application's preedit area directly.
    ///
    /// This module emits it only to clear the area when a composition ends; the
    /// engine also uses it when a mode change outside a composition invalidates what
    /// the area shows.
    SetClientPreedit(Option<(String, u32)>),
}

/// Where the window should appear, as far as the session can say.
///
/// The session does not know where the caret is -- that is the host's -- so it asks
/// for a side of the cursor and leaves the rectangle to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorHint {
    /// Revision of the frame this window shows, so that a show and the frame it
    /// belongs to can be matched up.
    pub revision: Revision,
    /// Requested side of the cursor.
    pub placement: Placement,
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
            dag: SyllableDag::new(),
            scratch: DecodeScratch::new(),
            previous: empty_result(),
            request: DecodeRequest::new(""),
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

    /// Returns the segmentation graph of the input in [`Session::buf`].
    ///
    /// The graph describes the input the last refresh segmented, and is empty while the
    /// session is idle.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn dag(&self) -> &SyllableDag {
        &self.dag
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
        let _ = self.dag.build("");
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
    pub(super) fn refresh(&mut self, env: &SessionEnv<'_>) {
        self.resegment();
        let parked = core::mem::replace(&mut self.previous, self.scratch.take_result());
        self.scratch.recycle(parked);
        let prev = self
            .previous
            .candidates
            .get(usize::from(self.paging.highlight))
            .map(|held| held.text.as_str());
        self.request.raw.clear();
        self.request.raw.push_str(self.buf.raw());
        env.decoder.decode_into(
            &mut self.scratch,
            &self.request,
            env.lexicon,
            env.user_freq,
            env.lm,
        );
        self.paging
            .reconcile(prev, &self.scratch.result().candidates);
        build_preedit_into(&self.buf, &self.dag, &mut self.preedit);
    }

    /// Rebuilds the segmentation graph and writes its syllable grid into the buffer.
    fn resegment(&mut self) {
        if self.dag.build(self.buf.raw()).is_err() {
            return;
        }
        write_boundaries(&mut self.buf, &self.dag);
    }

    /// Rebuilds the preedit from the current input and graph.
    pub(super) fn rebuild_preedit(&mut self) {
        build_preedit_into(&self.buf, &self.dag, &mut self.preedit);
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
    let mut ctx = Ctx::new(cfg, env);
    sess.on_event(ev, &mut ctx);
    let out = ctx.into_effects();
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

/// Empties a result in place, keeping the buffers it holds.
///
/// The same value [`empty_result`] builds from nothing, for a caller that already has a
/// result: the lists lose their entries, and the capacity they were sized to stays for
/// the next decode to write into.
fn clear_result(result: &mut DecodeResult) {
    result.candidates.clear();
    result.segments.clear();
    result.degraded = false;
}

/// Writes the syllable grid of `dag` into `buf`, so that a Backspace removes a syllable.
///
/// The grid is written back only when the graph has a segmentation at all. A graph
/// without one reports the whole input as a single pass-through range, and adopting that
/// would make one Backspace delete everything the user typed; the buffer keeps the grid
/// it already had instead.
///
/// The buffer and the graph are separate arguments rather than two fields of a session, so
/// that the caller decides which graph the grid is derived from.
fn write_boundaries(buf: &mut InputBuffer, dag: &SyllableDag) {
    let mut hint = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !dag.best_segmentation_hint(&mut hint) {
        return;
    }
    let raw = buf.raw();
    let mut grid = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !map_to_raw(raw, &hint, &mut grid) {
        return;
    }
    buf.set_boundaries(&grid);
}

/// Returns the preedit of a session that has composed nothing.
pub(super) fn empty_preedit() -> Preedit {
    Preedit {
        text: String::new(),
        caret: 0,
        spans: Vec::new(),
    }
}
