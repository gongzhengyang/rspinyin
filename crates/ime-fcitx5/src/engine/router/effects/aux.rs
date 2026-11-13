//! The auxiliary-code refinement view: the lead key's gate report and the
//! snapshot-based session the router keeps per input context.
//!
//! # Responsibility
//!
//! When the lead key opens a session, this module holds its state and answers its
//! keys: it renders the kept candidates into the candidate window, refines them by
//! the typed code, steps the code back, commits a selection through the same door
//! an ordinary commit uses, and hands every key it cannot use back to the pipeline
//! so the composition underneath continues as if the view had never opened.
//!
//! # Why the view is here and not in the session
//!
//! The state machine's candidate list is the decoder's; the view's is a
//! subsequence of one frame of it. Rather than teaching the machine a second
//! candidate source, the view keeps the owned snapshot on the context and answers
//! before the session steps: a key the view claims never reaches the machine, so
//! the two lists can never be confused. The refinement logic itself is not
//! duplicated — every kept list this module renders comes out of
//! [`AuxSession`], the frozen pure session in `ime-core`, which is what keeps the
//! subsequence invariant a property of the contract rather than of this file.
//!
//! # Why the session is rebuilt per keystroke
//!
//! [`AuxSession`] borrows the per-candidate codes for as long as it lives, and
//! the codes it borrows are views over the mapped dictionary image — an image a
//! dictionary swap can replace between two keystrokes. A view stored on the
//! context therefore cannot hold the borrowed session across steps without
//! pinning an image the swap may want to free. The overlay instead owns the
//! snapshot and the codes as strings, and each key rebuilds the pure session over
//! them inside one call: the rebuild is a clone of a bounded snapshot plus a
//! prefix replay, well inside the refinement budget, and the codes the session
//! borrows never outlive the dictionary read they came from.
//!
//! # Threading
//!
//! Everything here runs on the host thread inside one `key_event` or `ui_event`
//! call. The overlay is context state; nothing is shared, nothing blocks, and the
//! only host calls are the same commit, preedit and `post_ui` calls the effect
//! executor makes.

use ime_config::keymap::KeyBindings;
use ime_core::aux::{ABSENT_COLUMN_CODE, AuxPassReason, AuxSession, AuxStep};
use ime_core::state::{Paging, SessionEvent, SessionState};
use ime_types::{
    Candidate, ImeError, KeyAction, LayoutHint, PageDir, PageState, Preedit, UiCommand, UiEvent,
    UiFrame,
};

use crate::engine::badge::{self, BadgeState};
use crate::engine::host::Host;
use crate::engine::router::RoutingConfig;
use crate::engine::router::context::{Context, resolve_client_preedit};
use crate::engine::router::effects::StepCtx;
use crate::ffi::emit_diagnostic;

/// The stable code for a candidate whose aux code cannot be resolved out of a
/// container that has already passed structural verification (`domain/action/reason`
/// form; the router owns the emission, this module owns the code). The
/// [`AuxSource`](ime_core::aux::AuxSource) contract calls the condition impossible
/// for a verified container, so the arm is defensive and the key is never a miss.
pub(in crate::engine::router) const ROW_UNREADABLE_CODE: &str = "aux/row-unreadable";

/// The configuration key the occupied-lead-key conflict is reported under.
///
/// The conflict names the refinement feature's own switch, and the reason names
/// the binding that holds the mark — the same shape every projection warning
/// travels in.
const LEAD_CONFLICT_KEY: &str = "engine.aux_refinement";

/// Why the occupied-lead-key conflict keeps the session closed.
const LEAD_CONFLICT_REASON: &str = "the aux lead key (backtick) is also bound to a character-selection mark; the refinement session stays closed";

/// The character the view appends to the composing preedit while it is open.
///
/// It is the lead key itself, which is what tells the user the list on screen is
/// a refinement of their composition rather than the decode's own answer.
const LEAD_MARK: char = '`';

/// Whether the backtick key is free to lead a session under `keys`.
///
/// The lead key is occupied when a configurable binding names the same character.
/// The binding table's only character-valued entries are the two character-
/// selection marks — the page and highlight lists bind named keys, not marks — so
/// those are the fields this reads. With the shipped schema the answer is always
/// free, which is the fact that makes the backtick usable as a lead key at all;
/// the gate exists so that a binding the schema grows into the grave key cannot
/// silently take letters away from the refinement session.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn lead_key_free(keys: &KeyBindings) -> bool {
    let lead = Some(LEAD_MARK);
    keys.select_first_char.mark() != lead && keys.select_last_char.mark() != lead
}

/// Reports what a refused gate owes, and nothing beside it.
///
/// The occupied lead key is a configuration conflict, so it travels the host's
/// diagnostic channel as one. The absent column is the once-per-process report
/// the [`AuxSource`](ime_core::aux::AuxSource) contract names; `reported` is the
/// router's process-wide flag, and the first refusal spends it. The other three
/// reasons are silent: the disabled switch is the shipped configuration, a
/// foreign domain is the user's own choice, and an idle backtick is every
/// backtick ever typed into an empty application.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn report_refusal(
    reason: AuxPassReason,
    ic: u64,
    reported: &mut bool,
    host: &mut dyn Host,
) {
    match reason {
        AuxPassReason::LeadKeyOccupied => {
            host.diagnose(
                ic,
                &ImeError::ConfigInvalid {
                    key: String::from(LEAD_CONFLICT_KEY),
                    reason: String::from(LEAD_CONFLICT_REASON),
                },
            );
        }
        AuxPassReason::ColumnAbsent => {
            if !*reported {
                emit_diagnostic(ABSENT_COLUMN_CODE);
                *reported = true;
            }
        }
        AuxPassReason::Disabled | AuxPassReason::OtherDomain | AuxPassReason::NoCandidates => {}
    }
}

/// The state of one open refinement view.
///
/// The snapshot and the codes are owned because the session that consumes them
/// borrows for one call only; the typed code and the view's paging survive
/// between keys, which is the whole of what a keystroke changes.
#[derive(Debug)]
pub(in crate::engine::router) struct AuxOverlay {
    /// The candidate list the session had on screen at entry, in its own order.
    /// Every kept list is an ordered subsequence of this, by construction of
    /// [`AuxSession`].
    snapshot: Vec<Candidate>,
    /// Each snapshot candidate's aux code, resolved once at entry and owned.
    /// A `None` pairs with a candidate whose first character carries no code.
    codes: Vec<Option<String>>,
    /// The letters accepted so far, each one a key `AuxSession::refine` took.
    typed: String,
    /// The view's own paging over the kept list, sized from the configuration.
    paging: Paging,
    /// The revision of the last frame posted for this view; UI events carrying
    /// any other revision are stale and dropped.
    revision: u32,
}

impl AuxOverlay {
    /// Opens a view over `snapshot` with the router-resolved `codes`.
    ///
    /// `codes` pairs by index with `snapshot`; the page size is the
    /// configuration's own, the view starts at its entry state, and nothing is
    /// typed, so the first frame shows the snapshot verbatim.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn new(
        snapshot: Vec<Candidate>,
        codes: Vec<Option<String>>,
        page_size: u8,
    ) -> Self {
        Self {
            snapshot,
            codes,
            typed: String::new(),
            paging: Paging::with_page_size(page_size),
            revision: 0,
        }
    }

    /// Rebuilds the pure refinement session over the owned state, as of the
    /// typed code this overlay holds.
    ///
    /// Every letter in `typed` was accepted by [`AuxSession::refine`] when it
    /// was typed, so the replay's answers are all `Some` and are dropped on
    /// purpose: only the final kept list is read.
    fn rebuilt(&self) -> AuxSession<'_> {
        let codes: Vec<Option<&str>> = self.codes.iter().map(Option::as_deref).collect();
        let mut session = AuxSession::enter(self.snapshot.clone(), codes);
        for letter in self.typed.chars() {
            let _ = session.refine(letter);
        }
        session
    }

    /// Returns the view to its entry state after the kept list changed shape:
    /// the list starts over from its first page, like the panel's own lists do.
    fn reset_view(&mut self) {
        self.paging.page = 0;
        self.paging.highlight = 0;
    }

    /// The number of candidates the typed code keeps.
    ///
    /// The count is read out of the pure session rather than recomputed here:
    /// the filter is the contract's, not this module's.
    fn kept_total(&self) -> u16 {
        u16::try_from(self.rebuilt().kept().len()).unwrap_or(u16::MAX)
    }
}

/// What one routed event became in the view's hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The view kept the key; the bool records whether anything reached the
    /// host, which is what makes the key the plugin's to claim.
    Claimed(bool),
    /// The view committed a candidate; the composition underneath must now end,
    /// so the caller steps `Escape` next and lets the ordinary cancellation
    /// machinery hide the window and empty the preedit area.
    Committed,
    /// The view has no answer; the event continues to the pipeline. With an
    /// overlay taken out of the context, `Pass` is also the close: the pipeline
    /// answers with the session's own list, which is the snapshot's content.
    Pass,
}

/// Answers one routed event for the refinement view.
///
/// The caller asks only when the feature is enabled or a view is open; with the
/// switch off and none open this is never reached, which is what keeps the
/// shipped configuration's key path free of the call. Opening happens on the
/// raw-key path in the router, not here: the lead key has no row of its own, so
/// it never arrives as a routed action.
///
/// # Panics
///
/// Never.
pub(super) fn step(
    ctx: &mut Context,
    event: &SessionEvent,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    // The view is ephemeral routing-layer state: every boundary event closes it,
    // whether or not the feature is still enabled. A reload that switched the
    // refinement off must not leave a view that the next enable would resurrect;
    // the composition the view refined survives all three events untouched.
    match event {
        SessionEvent::Key(action) => key(ctx, *action, step, host),
        SessionEvent::Ui(ui_event) => ui(ctx, ui_event, step, host),
        // A reload closes the view and repaints the session's own list: the
        // core only repaints itself when the page size changed, which says
        // nothing about an overlay it does not know about, and a closed view
        // must not leave the refinement's preedit on a window nothing owns.
        SessionEvent::ConfigReloaded(_) => {
            ctx.aux = None;
            if ctx.session.state == SessionState::Composing {
                render_session_view(ctx, step, host);
            }
            Verdict::Pass
        }
        SessionEvent::FocusLost | SessionEvent::Reset => {
            ctx.aux = None;
            Verdict::Pass
        }
        SessionEvent::PreeditCleared | SessionEvent::CommitDone | SessionEvent::PinApplied => {
            Verdict::Pass
        }
    }
}

/// Answers one key for an open view.
///
/// A key the view does not own leaves with the view closed and answers `Pass`:
/// the pipeline's own answer then repaints the session's list, which is the
/// content the view was filtering, so nothing on screen can go stale behind an
/// overlay that stayed open. The taken-out overlay is what closes it — `Pass`
/// arms simply do not put it back.
fn key(
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let Some(mut overlay) = ctx.aux.take() else {
        return Verdict::Pass;
    };
    match action {
        KeyAction::InputChar(ch) => refine_key(ctx, overlay, ch, step, host),
        KeyAction::Backspace => backspace(ctx, overlay, step, host),
        // Escape is the way out that keeps the composition: the snapshot is
        // restored verbatim and the next Escape, unanswered by the view, is the
        // cancellation it always was.
        KeyAction::Escape => {
            render_session_view(ctx, step, host);
            Verdict::Claimed(true)
        }
        KeyAction::CommitHighlighted => {
            let index = overlay.paging.highlight;
            select_at(ctx, overlay, usize::from(index), host)
        }
        KeyAction::SelectIndex(digit) => select_digit(ctx, overlay, digit, host),
        KeyAction::PageNext
        | KeyAction::PagePrev
        | KeyAction::PageFirst
        | KeyAction::PageLast
        | KeyAction::MoveHighlight(_) => view_key(ctx, overlay, action, step, host),
        // A mode key already flipped its bit in the routing layer; the view is
        // the thing on screen, so the repaint it owes is the view's own frame.
        KeyAction::ToggleLang
        | KeyAction::ToggleFullWidth
        | KeyAction::TogglePunct
        | KeyAction::ToggleScript => {
            render_overlay(ctx, &mut overlay, step, host);
            ctx.aux = Some(overlay);
            Verdict::Claimed(true)
        }
        _ => Verdict::Pass,
    }
}

/// Feeds one typed character to the refinement.
///
/// A character outside the aux alphabet is not consumed: [`AuxSession::refine`]
/// answers `None` for it, the view closes, and the pipeline answers the key
/// exactly as it would have without the view.
fn refine_key(
    ctx: &mut Context,
    mut overlay: AuxOverlay,
    ch: char,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let accepted = overlay.rebuilt().refine(ch).is_some();
    if !accepted {
        return Verdict::Pass;
    }
    overlay.typed.push(ch);
    overlay.reset_view();
    render_overlay(ctx, &mut overlay, step, host);
    ctx.aux = Some(overlay);
    Verdict::Claimed(true)
}

/// Steps the typed code back one letter.
///
/// At the entry point [`AuxSession::restore`] answers [`AuxStep::Close`]: the
/// backspace is consumed — so the pinyin underneath keeps its letter — and the
/// snapshot goes back on screen as the session's own frame.
fn backspace(
    ctx: &mut Context,
    mut overlay: AuxOverlay,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let closed = overlay.rebuilt().restore() == AuxStep::Close;
    if closed {
        render_session_view(ctx, step, host);
        return Verdict::Claimed(true);
    }
    overlay.typed.pop();
    overlay.reset_view();
    render_overlay(ctx, &mut overlay, step, host);
    ctx.aux = Some(overlay);
    Verdict::Claimed(true)
}

/// The view's own paging keys: flips and highlight moves over the kept list.
///
/// Every one is claimed whether or not it moved anything — a page key is
/// punctuation to the application and must not leak from behind the view — and
/// an inert one repaints the unchanged view rather than claiming silently.
fn view_key(
    ctx: &mut Context,
    mut overlay: AuxOverlay,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let total = overlay.kept_total();
    match action {
        KeyAction::PageNext => {
            let _moved = overlay.paging.flip(PageDir::Next, total);
        }
        KeyAction::PagePrev => {
            let _moved = overlay.paging.flip(PageDir::Prev, total);
        }
        KeyAction::PageFirst => {
            let _moved = overlay.paging.flip_to_first(total);
        }
        KeyAction::PageLast => {
            let _moved = overlay.paging.flip_to_last(total);
        }
        KeyAction::MoveHighlight(delta) => {
            let _moved = overlay.paging.move_highlight(delta, total);
        }
        // The match in `key` admits no other action here; the view stays as it
        // is rather than closing on a key no caller can send.
        _ => {
            ctx.aux = Some(overlay);
            return Verdict::Claimed(false);
        }
    }
    render_overlay(ctx, &mut overlay, step, host);
    ctx.aux = Some(overlay);
    Verdict::Claimed(true)
}

/// Commits the kept candidate `index` names, or leaves the list as it is.
///
/// The commit leaves through the same door an ordinary one does — the output
/// switches rewrite it, the host hands it to the application — and the
/// composition is then taken back by the caller's `Escape`, because the word the
/// user meant is out and the letters that spelled it have nothing left to keep.
/// An index past the kept list is claimed without action, the way a digit past a
/// page is: committing a candidate the user cannot see is the defect the guard
/// exists for.
fn select_at(ctx: &mut Context, overlay: AuxOverlay, index: usize, host: &mut dyn Host) -> Verdict {
    let text = overlay
        .rebuilt()
        .kept()
        .get(index)
        .map(|kept| kept.text.clone());
    let Some(text) = text else {
        ctx.aux = Some(overlay);
        return Verdict::Claimed(false);
    };
    let text = ctx.modes.transform_output(&text, ctx.script.index());
    host.commit(ctx.ic, &text);
    Verdict::Committed
}

/// Commits the kept candidate the digit on show names.
///
/// The digit is page-relative, exactly as it is on a decoded list, and `0`
/// names nothing — the contract answers `None` for it and the key is claimed
/// without action rather than leaking into the application from behind the view.
fn select_digit(ctx: &mut Context, overlay: AuxOverlay, digit: u8, host: &mut dyn Host) -> Verdict {
    let total = overlay.kept_total();
    match overlay.paging.digit_target(digit, total) {
        Some(index) => select_at(ctx, overlay, usize::from(index), host),
        None => {
            ctx.aux = Some(overlay);
            Verdict::Claimed(false)
        }
    }
}

/// Answers one candidate-window event for an open view.
fn ui(
    ctx: &mut Context,
    event: &UiEvent,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let Some(mut overlay) = ctx.aux.take() else {
        return Verdict::Pass;
    };
    match event {
        UiEvent::Select {
            revision, index, ..
        } => {
            if *revision != overlay.revision {
                // A stale click is the session's to drop, as it always was, and
                // it says nothing about the view the user is looking at.
                ctx.aux = Some(overlay);
                return Verdict::Pass;
            }
            select_at(ctx, overlay, usize::from(*index), host)
        }
        UiEvent::Page { revision, dir } => {
            if *revision != overlay.revision {
                ctx.aux = Some(overlay);
                return Verdict::Pass;
            }
            let total = overlay.kept_total();
            let _moved = overlay.paging.flip(*dir, total);
            render_overlay(ctx, &mut overlay, step, host);
            ctx.aux = Some(overlay);
            Verdict::Claimed(true)
        }
        UiEvent::Dismiss { revision, .. } => {
            if *revision != overlay.revision {
                ctx.aux = Some(overlay);
                return Verdict::Pass;
            }
            // The window dismissed the view; the composition ends the ordinary
            // way, which hides the window under its own revision.
            Verdict::Pass
        }
        UiEvent::Hover { .. } | UiEvent::Rendered { .. } => {
            ctx.aux = Some(overlay);
            Verdict::Pass
        }
    }
}

/// Posts the first frame of a view the router just stored on the context.
///
/// The router calls this right after [`AuxOverlay::new`]; the entry frame shows
/// the snapshot verbatim with the lead mark appended to the composing preedit,
/// which is what tells the user the list is theirs to narrow now.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn post_open_frame(
    ctx: &mut Context,
    config: &RoutingConfig,
    badge: &mut BadgeState,
    host: &mut dyn Host,
) {
    let view = {
        let Some(overlay) = ctx.aux.as_ref() else {
            return;
        };
        aux_view(ctx, overlay)
    };
    let paint = Painter { config, badge };
    let revision = post_frame(ctx, view, paint, host);
    if let Some(overlay) = ctx.aux.as_mut() {
        overlay.revision = revision;
    }
}

/// Repaints an overlay the caller took out of the context, then stores it back.
fn render_overlay(
    ctx: &mut Context,
    overlay: &mut AuxOverlay,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) {
    let view = aux_view(ctx, overlay);
    let paint = Painter {
        config: step.config,
        // A reborrow, not a move: the step context outlives this frame, and
        // every later effect of the same step reads its badge through it.
        badge: &mut *step.badge,
    };
    let revision = post_frame(ctx, view, paint, host);
    overlay.revision = revision;
}

/// Renders the session's own frame, after the view closed without a commit.
///
/// The session's list is the snapshot's content — the view changed no state the
/// session reads — so this is the "restore the snapshot verbatim" the contract
/// names, built from the session's own paging, highlight and preedit the same
/// way the session builds its frames. The revision is the session's own; the
/// view records nothing because it is already gone.
fn render_session_view(ctx: &mut Context, step: &mut StepCtx<'_, '_>, host: &mut dyn Host) {
    let view = session_view(ctx);
    let paint = Painter {
        config: step.config,
        // A reborrow, not a move: the step context outlives this frame.
        badge: &mut *step.badge,
    };
    let _revision = post_frame(ctx, view, paint, host);
}

/// What every frame this module posts reads besides the view itself: the
/// configuration in force, and the process-local badge state a posted frame
/// consumes.
struct Painter<'a> {
    /// The configuration in force, whose layout hints and client-preedit
    /// switch the frame carries.
    config: &'a RoutingConfig,
    /// The process-local state the header's slot resolves through.
    badge: &'a mut BadgeState,
}

/// The one frame this module posts, from the view it is given.
///
/// The shape is the session's own: the show precedes the frame because the
/// window drops a frame that arrives while it is hidden, the preedit follows the
/// same client negotiation an ordinary frame's does, and the header's slot
/// resolves here, where the frame's own page and candidates are known. The
/// overlay's lists are bounded by the snapshot the session decoded, so the
/// overflow bit is always the honest `false`.
fn post_frame(ctx: &mut Context, view: FrameView, paint: Painter<'_>, host: &mut dyn Host) -> u32 {
    let revision = ctx.session.revision.next().value();
    if resolve_client_preedit(paint.config.client_preedit, ctx.preedit_supported) {
        host.set_preedit(ctx.ic, &view.preedit.text, view.preedit.caret);
    } else {
        host.clear_preedit(ctx.ic);
    }
    let anchor = ctx.session.frame_context().anchor;
    host.post_ui(ctx.ic, UiCommand::Show { revision, anchor });
    let has_candidates = !view.candidates.is_empty();
    let mode = ctx
        .modes
        .mode_label(ctx.session.temp_english, paint.config.scheme_hint);
    let (label, overflow_started) = badge::resolve(
        mode,
        &paint.config.keys,
        view.page,
        has_candidates,
        false,
        paint.badge,
    );
    if overflow_started {
        emit_diagnostic(badge::UI_CANDIDATE_OVERFLOW_CODE);
    }
    let mut frame = UiFrame {
        revision,
        preedit: view.preedit,
        candidates: view.candidates,
        page: view.page,
        status: ctx.session.frame_context().status.clone(),
        anchor,
        layout: LayoutHint {
            max_per_row: paint.config.session.max_per_row,
            show_annotation: paint.config.session.show_annotation,
            max_width_dp: paint.config.session.max_width_dp,
        },
        highlight: view.highlight,
    };
    frame.status.mode_label = label;
    host.post_ui(ctx.ic, UiCommand::Frame(Box::new(frame)));
    revision
}

/// What one frame carries besides the session's standing context.
struct FrameView {
    /// The page's candidates, display-numbered from one.
    candidates: Vec<Candidate>,
    /// The paging part of the frame.
    page: PageState,
    /// The page-local position of the highlight, or `None` to hide the ring.
    highlight: Option<u16>,
    /// The composing text the application's preedit area is told to show.
    preedit: Preedit,
}

/// Builds the frame of an open view: the kept list, paged and numbered.
///
/// The kept list is read out of the pure session, so the filter is never this
/// module's to restate. Candidates keep their snapshot identity — text,
/// annotation, source and score are the ones the decoder produced — and carry
/// page-local display numbers, which is what makes the digit keys name what the
/// window shows.
fn aux_view(ctx: &Context, overlay: &AuxOverlay) -> FrameView {
    let kept = overlay.rebuilt();
    let kept_slice = kept.kept();
    let total = u16::try_from(kept_slice.len()).unwrap_or(u16::MAX);
    let start = usize::from(overlay.paging.page_start()).min(kept_slice.len());
    let end = usize::from(overlay.paging.page_end(total)).min(kept_slice.len());
    let candidates = kept_slice[start..end]
        .iter()
        .enumerate()
        .map(|(position, candidate)| Candidate {
            index: u16::try_from(start + position + 1).unwrap_or(u16::MAX),
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: candidate.source,
            score: candidate.score,
            consumed_syllables: candidate.consumed_syllables,
        })
        .collect();
    FrameView {
        candidates,
        page: overlay.paging.page_state(total),
        highlight: overlay.paging.highlight_position_in_page(total),
        preedit: aux_preedit(ctx, overlay),
    }
}

/// The composing preedit an open view shows: the session's own text, the lead
/// mark, and the typed code.
///
/// The session's spans keep pointing at the pinyin they describe — the appended
/// text starts where they end — and the caret sits at the end, which is on a
/// character boundary because the mark and the code are ASCII.
fn aux_preedit(ctx: &Context, overlay: &AuxOverlay) -> Preedit {
    let mut preedit = ctx.session.preedit().clone();
    preedit.text.push(LEAD_MARK);
    preedit.text.push_str(&overlay.typed);
    preedit.caret = u32::try_from(preedit.text.len()).unwrap_or(u32::MAX);
    preedit
}

/// Builds the frame the session itself would draw, for the view's close.
///
/// Every piece is the session's own — its list, its paging, its highlight and
/// its preedit — so what lands on screen after the close is exactly the frame
/// the session's next step would have painted, without asking the machine to
/// paint it.
fn session_view(ctx: &Context) -> FrameView {
    let session = &ctx.session;
    let candidates = session.decoded().candidates.as_slice();
    let total = u16::try_from(candidates.len()).unwrap_or(u16::MAX);
    let start = usize::from(session.paging.page_start()).min(candidates.len());
    let end = usize::from(session.paging.page_end(total)).min(candidates.len());
    FrameView {
        candidates: candidates[start..end].to_vec(),
        page: session.paging.page_state(total),
        highlight: session.paging.highlight_position_in_page(total),
        preedit: session.preedit().clone(),
    }
}
