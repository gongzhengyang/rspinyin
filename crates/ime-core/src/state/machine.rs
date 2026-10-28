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

use crate::confusable::ConfusableTable;
use crate::input::InputBuffer;
use crate::preedit::build_preedit_into;
use crate::segment::SyllableDag;
use crate::state::SessionConfig;
use crate::viterbi::context::ContextBand;
use crate::state::outcome::{clear_result, empty_preedit, renumber, write_boundaries};
use crate::state::paging::Paging;
use crate::state::scheme::SchemeSession;
use crate::symbols::SymbolTable;
use crate::viterbi::{DecodeScratch, DecodeSources, Decoder};

pub use crate::state::effects::{AnchorHint, Effect, Effects, MAX_EFFECTS};
pub use crate::state::frame::FrameContext;

/// How many key events a context word serves before it goes stale.
///
/// The re-rank it drives is a statement about the sentence the user just finished;
/// five keystrokes into a new composition that statement has worn thin, and the
/// sixth retires it. A handful, not a ceiling: the number exists so the context
/// cannot outlive its usefulness, not to meter it precisely.
pub const CONTEXT_KEY_BUDGET: u8 = 5;

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
    pub(super) scratch: DecodeScratch,
    /// The candidate list of the refresh before the last one, parked outside the
    /// workspace.
    ///
    /// [`Session::refresh`] reads the text the highlight follows out of it as a borrow,
    /// while the decode below needs the workspace mutably; holding one answer here is
    /// what lets that borrow and the decode exist at the same time without copying the
    /// text. The buffers it carries go back into the workspace before the next decode,
    /// so the decode writes into storage that is already sized.
    pub(super) previous: DecodeResult,
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
    pub(super) preedit: Preedit,
    /// Where the window goes and what its status strip says. The engine writes this
    /// whenever the host tells it the cursor moved or a mode bit changed.
    ctx: FrameContext,
    /// The word and weight a commit in flight should record once it lands.
    pending: Option<PendingCommit>,
    /// The fuzzy-equivalence classes this session decodes with, latched from the
    /// configuration beside the scheme.
    ///
    /// The latch runs while the session is idle, like the scheme's: a reload mid
    /// composition changes the reading from the next composition on, for the same
    /// reason the scheme's does -- re-reading the input under the caret would move
    /// the candidate list the user is looking at. The value is already the projected
    /// mask (master bit consistent with the class bits), so the decode request
    /// assembles from it without consulting anything else.
    fuzzy: ime_types::DecodeFlags,
    /// Whether the session expands initial-letter abbreviations, latched beside the
    /// fuzzy classes for the same reason.
    abbrev: bool,
    /// Whether the session corrects one-step typing slips, latched beside the
    /// abbreviation switch for the same reason.
    correction: bool,
    /// Whether the session offers an English candidate for an input no pinyin
    /// reading cuts, latched beside the correction switch for the same reason.
    english: bool,
    /// Whether the session attaches the hint annotations -- the standard reading of
    /// a widened edge and the correct form of a confusable hit -- to its candidates,
    /// latched from `ui.show_annotation` for the same reason its siblings latch: a
    /// reload mid composition must not change what the user is looking at, and with
    /// the switch off the whole hint chain short-circuits at the decode.
    hints: bool,
    /// The context re-rank band in effect now, latched like the other decode switches.
    pub(super) context_band: ContextBand,
    /// Whether the composition on screen is the symbol panel's: the input starts
    /// with `v` and names a category of the symbol table, so the candidate list
    /// was assembled from the table rather than decoded. The one reader is the
    /// commit path, which records nothing for a panel candidate -- a symbol is
    /// chosen, not spelled, and no frequency is learned from choosing it.
    pub(super) symbol_mode: bool,
    /// How much of the decoder's deterministic candidate supply is materialized now.
    ///
    /// This is the session's position on the fixed ladder, not user configuration. It
    /// resets with every refresh and advances only when paging asks past what is visible.
    pub(super) candidate_supply: u16,
    /// The word the user's last landed commit ended with, and the one piece of
    /// context that crosses compositions.
    ///
    /// It is read once per refresh, to re-order the near-tie head of a fresh
    /// candidate list, and it is session memory and nothing else: it never reaches
    /// an effect, a log line or the frame (the privacy rule the candidate and commit
    /// texts already live under), and it dies with the session.
    context_word: Option<String>,
    /// How many key events have passed since [`Session::context_word`] was set.
    ///
    /// Context grows stale as the user types on: past a handful of keystrokes the
    /// composition it describes is no longer the sentence the user is in, and the
    /// re-rank it would drive is a guess. The count is what retires it, in the one
    /// currency a pure session is paid in -- keystrokes, not seconds.
    context_keys: u8,
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
    /// The word pairs of the committed reading that the commit can coin, in `(key, text)`
    /// form; empty when there is nothing to coin.
    ///
    /// Captured when the commit was asked for, from the workspace's own record of the
    /// reading, and reported only once the host confirms the commit landed -- a commit the
    /// host refused must not teach the dictionary a word that was not on the screen, which
    /// is the same rule [`PendingCommit::key`] already lives under.
    pub(super) coins: Vec<(String, String)>,
}

/// Everything the session reads that it does not own.
///
/// Grouped into one struct rather than threaded through [`step`] as four parameters,
/// which also gives the tests a single place to swap in their in-memory doubles.
///
/// `Copy` because every field is a shared reference: a host that rebuilds the environment
/// per key -- the dictionary side of a hot swap, say -- copies the parts it keeps and
/// fills the parts it re-reads, without the borrow of the old environment surviving.
#[derive(Clone, Copy)]
pub struct SessionEnv<'a> {
    /// The decoder, built once and shared by every session.
    pub decoder: &'a Decoder,
    /// The dictionary the decode reads.
    pub lexicon: &'a dyn Lexicon,
    /// The user's own frequencies, read while ranking and written after a commit.
    pub user_freq: &'a dyn UserFreqSource,
    /// The language model the ranking is scored with.
    pub lm: &'a dyn LanguageModel,
    /// The English lexicon the mixed-English switch reads, when the process
    /// assembled one. `None` — a build without the container, or a degraded
    /// start — decodes exactly as a build before the switch existed.
    pub english: Option<&'a dyn Lexicon>,
    /// The confusable table the wrong-word hint reads, when the process loaded
    /// one. `None` — no data file, or a request with the annotations switch off —
    /// leaves every candidate's annotation exactly as a build before the hint
    /// channel produced it.
    pub confusables: Option<&'a ConfusableTable>,
    /// The symbol table the v-mode panel serves its categories from, when the
    /// process loaded one. `None` — no data file — leaves an input that starts
    /// with `v` decoding exactly as a build before the panel existed.
    pub symbols: Option<&'a SymbolTable>,
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
    /// The host applied a pin toggle; the session re-decodes the unchanged input.
    PinApplied,
}

mod rebuild;

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
            fuzzy: ime_types::DecodeFlags::empty(),
            abbrev: false,
            correction: false,
            english: false,
            hints: false,
            context_band: ContextBand::default(),
            symbol_mode: false,
            candidate_supply: SessionConfig::INITIAL_CANDIDATE_QUOTA,
            context_word: None,
            context_keys: 0,
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
    /// Returns the workspace's answer, mutably, for the one post-pass that
    /// assembles a candidate list without decoding: the symbol panel's.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn decoded_mut(&mut self) -> &mut DecodeResult {
        self.scratch.result_mut()
    }

    /// Empties the workspace's segmentation graph, keeping its buffers.
    ///
    /// The symbol panel's refresh runs no decode, so no graph may describe the
    /// input: the preedit builder answers an empty graph with the raw, which is
    /// exactly what the panel's composition shows.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn clear_decode_graph(&mut self) {
        self.scratch.clear_dag();
    }

    /// Returns whether the composition on screen is the symbol panel's.
    ///
    /// The host's arbiter reads it to route the digit keys: inside the panel a
    /// digit keys the code, and outside it selects, which is what the keymap has
    /// always meant by a digit.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_symbol_mode(&self) -> bool {
        self.symbol_mode
    }

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

    /// The word pairs of the candidate at `index` that a commit could coin.
    ///
    /// The pairs live in the workspace, which holds the reading of the candidate list the
    /// highlight moves over; this is the read side the commit path captures them through,
    /// so that a keystroke after the capture -- which would decode a new input into the
    /// same buffers -- cannot change what the commit reports.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn coin_pairs_for(&self, index: usize) -> Vec<(String, String)> {
        self.scratch.coin_pairs(index)
    }

    /// Remembers `word` as the context the next compositions decode with.
    ///
    /// This is the one place context is born: a commit the host confirmed landed.
    /// The keystroke count beside it restarts, so the whole budget of stale-ness is
    /// the new composition's, not the finished one's.
    pub(super) fn set_context(&mut self, word: String) {
        self.context_word = Some(word);
        self.context_keys = 0;
    }

    /// Forgets the context word.
    ///
    /// Every path that ends a composition without a commit runs through here: a
    /// composition the user took back says nothing about what they mean next, which
    /// is exactly what the context claims to know.
    pub(super) fn clear_context(&mut self) {
        self.context_word = None;
        self.context_keys = 0;
    }

    /// Retires the context word once the keystrokes it was allowed have passed.
    ///
    /// The budget is five key events. A pure session has no clock -- touching one
    /// would break the determinism the tests replay -- so staleness is counted in
    /// keystrokes, which is the currency the input arrives in anyway; the wall-clock
    /// expiry the design sketches is host-side business, reachable through
    /// [`SessionEvent::Reset`] when a host wants it.
    pub(super) fn age_context(&mut self) {
        if self.context_word.is_some() {
            self.context_keys = self.context_keys.saturating_add(1);
            if self.context_keys > CONTEXT_KEY_BUDGET {
                self.clear_context();
            }
        }
    }

    /// Reports whether a page request can still widen the deterministic supply.
    ///
    /// The host asks this before a key that would otherwise look like the materialized
    /// list's last page; the session performs the actual ladder expansion while handling
    /// the event.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn has_page_supply(&self) -> bool {
        self.can_expand_next_page()
    }

    /// Returns the context word, for the tests that pin its life cycle.
    ///
    /// The field is session-private by design; the tests read it through this door.
    #[cfg(test)]
    pub(super) fn context(&self) -> Option<&str> {
        self.context_word.as_deref()
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
        self.candidate_supply = SessionConfig::INITIAL_CANDIDATE_QUOTA;
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
        // The fuzzy classes latch beside the scheme, and are clamped to the mask the
        // fuzzy namespace owns: a caller that hands over bits from somewhere else
        // cannot leave them set in a request the segmentation layer reads.
        sess.fuzzy = cfg.fuzzy & crate::fuzzy::FLAG_MASK;
        sess.abbrev = cfg.abbrev;
        sess.correction = cfg.correction;
        sess.english = cfg.english;
        sess.hints = cfg.show_annotation;
        sess.context_band = cfg.context_band;
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
