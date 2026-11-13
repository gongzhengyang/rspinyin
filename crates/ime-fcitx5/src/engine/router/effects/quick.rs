//! The quick-input panel: the routing layer's own composition for a matched trigger.
//!
//! # Responsibility
//!
//! When the input buffer names a quick-input trigger — a configured date-family
//! spelling, the calculator, the amount trigger or the codepoint trigger — this
//! module takes the keys over: it renders the domain's candidates into the candidate
//! window, pages them, commits a selection through the same door an ordinary commit
//! uses, and hands keys it cannot use back to the session so the composition
//! underneath continues as if the panel had never opened.
//!
//! # Why the panel is here and not in the session
//!
//! The session's candidate list is the decoder's; the panel's is rendered from pure
//! translators. Rather than teaching the state machine a second candidate source, the
//! panel keeps its own small state on the context and answers before the session
//! steps: a key the panel claims never reaches the machine, so the two candidate
//! lists can never be confused. The session underneath keeps composing the trigger
//! letters it was given; when the panel closes without committing — a backspace out
//! of the trigger, a key the payload cannot use — the event falls through and the
//! session continues exactly where its own buffer stands.
//!
//! # The clock
//!
//! The date family renders from an injected [`CivilTime`], converted from the host's
//! [`ClockSource`] snapshot. The source is installed once by the startup sequence; a
//! process that never installed one renders no time-domain candidates at all, which
//! is the documented degradation — the panel simply never opens for those triggers —
//! rather than an error on every key.
//!
//! # Threading
//!
//! Everything here runs on the host thread inside one `key_event` call. The panel
//! state is context state; nothing is shared, nothing blocks, and the only host calls
//! are the same commit, preedit and `post_ui` calls the effect executor makes.

use std::sync::Arc;
use std::sync::OnceLock;

use ime_core::quick::{self, CivilTime, QuickCandidate, QuickDomain, QuickMatch};
use ime_core::state::SessionEvent;
use ime_types::{KeyAction, PageDir, UiEvent};

use crate::engine::host::{ClockSource, Host};

use super::super::StepCtx;
use super::super::context::Context;
use super::panel::{PanelFrame, PanelPaging, post_frame};

/// The most date-family triggers the trigger table walks.
///
/// The same bound `ime-config` documents and the projection stores; the array here is
/// the stack copy the recognition call reads.
const MAX_DATE_PREFIXES: usize = ime_config::schema::MAX_QUICK_DATE_PREFIXES;

/// The wall clock the time domains render from, installed once by the startup
/// sequence.
///
/// A process that never installed a source renders no time-domain candidates: the
/// panel refuses to open for a trigger that would render nothing, so the triggers
/// keep composing as ordinary (dead) input exactly as they did before the panel
/// existed.
static CLOCK: OnceLock<Arc<dyn ClockSource + Send + Sync>> = OnceLock::new();

/// Installs the wall clock the time domains render from.
///
/// # Arguments
///
/// * `clock` - the source, shared; the first installation wins and later calls
///   answer `false`.
///
/// # Returns
///
/// Whether this call installed the clock.
///
/// # Panics
///
/// Never.
// The production caller is the startup sequence, which assembles the router
// outside this task's file set; until that wiring lands, the tests are this
// function's only callers and the dead-code analysis says so.
#[allow(dead_code)]
pub(crate) fn install_clock(clock: Arc<dyn ClockSource + Send + Sync>) -> bool {
    CLOCK.set(clock).is_ok()
}

/// Reads one civil-time snapshot, when a clock is installed.
fn snapshot() -> Option<CivilTime> {
    let clock = CLOCK.get()?;
    let snap = clock.now();
    Some(CivilTime {
        year: snap.year,
        month: snap.month,
        day: snap.day,
        hour: snap.hour,
        minute: snap.minute,
        second: snap.second,
        weekday: snap.weekday,
    })
}

/// The state of one open quick-input panel.
///
/// The visibility reaches the whole router module tree because the context — the
/// panel's owner — is a sibling of this module, not a descendant.
#[derive(Debug)]
pub(in crate::engine::router) struct QuickPanel {
    /// The domain the trigger matched, which decides what digits and letters mean.
    domain: QuickDomain,
    /// The full quick buffer the panel answers, trigger and payload: `rq`, `R12`,
    /// `cC1+2`, `U4f60`.
    buffer: String,
    /// The candidates, in display order — the whole list, not the page on show.
    candidates: Vec<QuickCandidate>,
    /// The revision of the last frame posted for this panel; UI events carrying any
    /// other revision are stale and dropped.
    revision: u32,
    /// The page on show and the keyboard highlight, shared with the reverse panel.
    paging: PanelPaging,
}

/// What one routed event became in the panel's hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The panel kept the key; the bool records whether anything reached the host,
    /// which is what makes the key the plugin's to claim.
    Claimed(bool),
    /// The panel committed a candidate; the composition underneath must now end, so
    /// the caller steps `Escape` next and lets the ordinary cancellation machinery
    /// hide the window and empty the preedit area.
    Committed,
    /// The panel has no answer; the event continues to the session.
    Pass,
}

/// Answers one routed event for the quick-input layer.
///
/// The caller asks only when quick input is enabled or a panel is open; with the
/// feature off and no panel this is never reached, which is what keeps the shipped
/// configuration's key path free of the call.
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
    // The panel is ephemeral routing-layer state: every boundary event closes it,
    // whether or not the feature is still enabled. A reload that switched quick
    // input off must not leave a panel that the next enable would resurrect.
    match event {
        SessionEvent::Key(action) => key(ctx, *action, step, host),
        SessionEvent::Ui(ui_event) => ui(ctx, ui_event, step, host),
        SessionEvent::ConfigReloaded(_) | SessionEvent::FocusLost | SessionEvent::Reset => {
            ctx.quick = None;
            Verdict::Pass
        }
        SessionEvent::PreeditCleared | SessionEvent::CommitDone | SessionEvent::PinApplied => {
            Verdict::Pass
        }
    }
}

/// Checks, after a composing key landed in the session buffer, whether the buffer now
/// names a trigger the panel should open for.
///
/// This is the letter-trigger path: the trigger's own characters reach the session
/// first — `rq` composes as ordinary (dead) input while it is being typed — and the
/// panel takes over only once the whole spelling stands. A trigger that renders no
/// candidates, the date family without a clock among them, opens nothing and the
/// session's own frame stays up.
///
/// # Panics
///
/// Never.
pub(super) fn open_from_buffer(ctx: &mut Context, step: &mut StepCtx<'_, '_>, host: &mut dyn Host) {
    if ctx.quick.is_some() {
        return;
    }
    let raw = ctx.session.buf.raw();
    let Some(matched) = match_recognize(raw, step) else {
        return;
    };
    open(matched, String::from(raw), ctx, step, host);
}

/// Opens a panel for `buffer`, which `matched` vouched for, unless it renders
/// nothing.
fn open(
    matched: QuickMatch,
    buffer: String,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) {
    let candidates = quick::candidates(matched, &buffer, snapshot().as_ref());
    if candidates.is_empty() {
        return;
    }
    let mut panel = QuickPanel {
        domain: matched.domain,
        buffer,
        candidates,
        revision: 0,
        paging: PanelPaging::new(),
    };
    show(&mut panel, ctx, step, host);
    ctx.quick = Some(panel);
}

/// Runs the trigger table over one buffer with the configuration's spellings.
fn match_recognize(raw: &str, step: &StepCtx<'_, '_>) -> Option<QuickMatch> {
    let mut spellings = [""; MAX_DATE_PREFIXES];
    for (slot, out) in step
        .config
        .quick
        .date_prefixes
        .iter()
        .zip(spellings.iter_mut())
    {
        *out = slot.as_str();
    }
    quick::recognize(true, raw, &spellings, step.config.quick.calc_enabled)
}

/// Answers one key for the panel or for the admission gate.
fn key(
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let Some(mut panel) = ctx.quick.take() else {
        return admit(ctx, action, step, host);
    };
    let (verdict, keep) = panel_key(&mut panel, ctx, action, step, host);
    if keep {
        ctx.quick = Some(panel);
    }
    verdict
}

/// Answers one key for an open panel. The bool answers whether the panel stays open.
fn panel_key(
    panel: &mut QuickPanel,
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let page_size = usize::from(step.config.session.max_per_row);
    match action {
        // The number keys commit on a date panel — the formats are the candidates —
        // and type into the payload everywhere else, where the digits are content
        // and Space is the selection gesture.
        KeyAction::SelectIndex(digit) => match panel.domain {
            QuickDomain::Date => {
                if digit == 0 {
                    return (Verdict::Claimed(false), true);
                }
                let index = usize::from(u16::from(digit) - 1);
                if index >= panel.candidates.len() {
                    return (Verdict::Claimed(false), true);
                }
                (commit(panel, index, ctx, host), false)
            }
            _ => {
                // A selection digit is payload content here, as its digit character.
                // The routing table names digits 1..=9; anything wider is not a
                // digit and is clamped like an over-long payload.
                if digit > 9 {
                    return (Verdict::Claimed(false), true);
                }
                grow(panel, char::from(b'0' + digit), ctx, step, host)
            }
        },
        KeyAction::CommitHighlighted => {
            let index = usize::from(panel.paging.highlight);
            if index >= panel.candidates.len() {
                return (Verdict::Claimed(false), true);
            }
            (commit(panel, index, ctx, host), false)
        }
        // Enter is the raw-text way out: the panel closes and the session commits
        // the letters it holds, which is what a user who wanted the spelling rather
        // than a rendering asked for.
        KeyAction::CommitRaw | KeyAction::Escape => (Verdict::Pass, false),
        // One keystroke removes one character of the quick buffer. While the buffer
        // still names an active trigger the panel re-renders; the moment it does
        // not, the panel closes and the session's own backspace takes over, with
        // the two buffers back in step.
        KeyAction::Backspace => {
            panel.pop_char();
            shrink(panel, ctx, step, host)
        }
        KeyAction::PageNext | KeyAction::PagePrev => {
            let dir = if action == KeyAction::PageNext {
                PageDir::Next
            } else {
                PageDir::Prev
            };
            panel.flip_page(dir, page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::PageFirst | KeyAction::PageLast => {
            panel.paging.page = if action == KeyAction::PageFirst {
                0
            } else {
                panel.last_page(page_size)
            };
            panel.paging.highlight = panel.page_start(page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::MoveHighlight(delta) => {
            panel.move_highlight(delta, page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::InputChar(ch) => {
            if panel.accepts_payload(ch) {
                grow(panel, ch, ctx, step, host)
            } else {
                // A key the payload cannot use closes the panel and falls through:
                // the session's buffer holds the trigger letters, so the
                // composition continues exactly where the user's typing left it.
                (Verdict::Pass, false)
            }
        }
        // A mode key already flipped its bit in the routing layer; the panel is the
        // thing on screen, so the repaint it owes is the panel's own frame.
        KeyAction::ToggleLang
        | KeyAction::ToggleFullWidth
        | KeyAction::TogglePunct
        | KeyAction::ToggleScript => {
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        // Chords that act on the session's own highlight — pin, forget, phrase
        // saving — have nothing to act on while the panel covers the list, and a
        // stale highlight is nobody's to act on. The panel closes and the session
        // answers, so the chord does what it means rather than what a hidden list
        // would make it.
        _ => (Verdict::Pass, false),
    }
}

/// Grows the panel's payload by one character, when the longer buffer still names an
/// active trigger.
///
/// A payload past its domain's bound is clamped rather than fatal: the panel keeps
/// its last good rendering and the key stays claimed, because a digit that leaked to
/// the session would act on a candidate list the user cannot see. The character is
/// the payload value a digit key names or the typed character itself; only the
/// codepoint payload takes letters.
fn grow(
    panel: &mut QuickPanel,
    ch: char,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let mut prospective = panel.buffer.clone();
    prospective.push(ch.to_ascii_lowercase());
    let Some(matched) = match_recognize(&prospective, step) else {
        return (Verdict::Claimed(false), true);
    };
    let candidates = quick::candidates(matched, &prospective, snapshot().as_ref());
    if candidates.is_empty() {
        return (Verdict::Claimed(false), true);
    }
    panel.buffer = prospective;
    panel.candidates = candidates;
    panel.paging = PanelPaging::new();
    show(panel, ctx, step, host);
    (Verdict::Claimed(true), true)
}

/// Shrinks the panel after a backspace, or closes it when the buffer stopped naming
/// an active trigger.
fn shrink(
    panel: &mut QuickPanel,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let Some(matched) = match_recognize(&panel.buffer, step) else {
        return (Verdict::Pass, false);
    };
    let candidates = quick::candidates(matched, &panel.buffer, snapshot().as_ref());
    if candidates.is_empty() {
        return (Verdict::Pass, false);
    }
    panel.candidates = candidates;
    panel.paging = PanelPaging::new();
    show(panel, ctx, step, host);
    (Verdict::Claimed(true), true)
}

/// Answers one key for the admission gate: the first digit of a payload domain.
///
/// Digits reach this layer as selection actions, because the routing table names
/// them so; a digit typed after a bare trigger is content, not a selection, and this
/// is where the two readings are told apart. Only a prospective buffer the trigger
/// table names opens the panel, so a digit after ordinary spelling — selecting a
/// pinyin candidate, the gesture it always was — passes through untouched.
fn admit(
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let KeyAction::SelectIndex(digit) = action else {
        return Verdict::Pass;
    };
    if digit > 9 {
        return Verdict::Pass;
    }
    let raw = ctx.session.buf.raw();
    let mut prospective = String::with_capacity(raw.len() + 1);
    prospective.push_str(raw);
    prospective.push(char::from(b'0' + digit));
    let Some(matched) = match_recognize(&prospective, step) else {
        return Verdict::Pass;
    };
    open(matched, prospective, ctx, step, host);
    if ctx.quick.is_some() {
        Verdict::Claimed(true)
    } else {
        Verdict::Pass
    }
}

/// Answers one candidate-window event for an open panel.
fn ui(
    ctx: &mut Context,
    event: &UiEvent,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let Some(mut panel) = ctx.quick.take() else {
        return Verdict::Pass;
    };
    match event {
        UiEvent::Select {
            revision, index, ..
        } => {
            if *revision != panel.revision {
                // A stale click is the session's to drop, as it always was, and it
                // says nothing about the panel the user is looking at.
                ctx.quick = Some(panel);
                return Verdict::Pass;
            }
            let index = usize::from(*index);
            if index >= panel.candidates.len() {
                ctx.quick = Some(panel);
                return Verdict::Claimed(false);
            }
            commit(&panel, index, ctx, host)
        }
        UiEvent::Page { revision, dir } => {
            if *revision != panel.revision {
                ctx.quick = Some(panel);
                return Verdict::Pass;
            }
            panel.flip_page(*dir, usize::from(step.config.session.max_per_row));
            show(&mut panel, ctx, step, host);
            ctx.quick = Some(panel);
            Verdict::Claimed(true)
        }
        UiEvent::Dismiss { revision, .. } => {
            if *revision != panel.revision {
                ctx.quick = Some(panel);
                return Verdict::Pass;
            }
            // The window dismissed the panel; the session cancels the composition
            // the ordinary way, which hides the window under its own revision.
            Verdict::Pass
        }
        UiEvent::Hover { .. } | UiEvent::Rendered { .. } => {
            ctx.quick = Some(panel);
            Verdict::Pass
        }
    }
}

/// Commits one panel candidate and answers [`Verdict::Committed`].
///
/// The text leaves through the same door an ordinary commit uses — the output
/// switches rewrite it, the host hands it to the application — and learning is
/// untouched: a rendered date or amount is a choice, not a spelling, and the panel
/// records nothing.
fn commit(panel: &QuickPanel, index: usize, ctx: &mut Context, host: &mut dyn Host) -> Verdict {
    let Some(candidate) = panel.candidates.get(index) else {
        return Verdict::Claimed(false);
    };
    let text = candidate.text.clone();
    let text = ctx.modes.transform_output(&text, ctx.script.index());
    host.commit(ctx.ic, &text);
    Verdict::Committed
}

/// Posts the panel's frame: the preedit, the show and the frame, in the order the
/// window drops nothing of.
///
/// The frame itself is the machinery the rendered panels share
/// (`post_frame`); this wrapper hands it the panel's own state and takes the
/// clamped page back.
fn show(
    panel: &mut QuickPanel,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) {
    let revision = ctx.session.revision.next();
    panel.revision = revision.value();
    panel.paging.page = u8::try_from(post_frame(
        PanelFrame {
            buffer: &panel.buffer,
            candidates: &panel.candidates,
            revision: panel.revision,
            highlight: panel.paging.highlight,
            requested_page: usize::from(panel.paging.page),
        },
        ctx,
        step,
        host,
    ))
    .unwrap_or(0);
}

impl QuickPanel {
    /// Whether a typed character is content this panel's payload can take: the
    /// hex letters of a codepoint payload, nothing else.
    fn accepts_payload(&self, ch: char) -> bool {
        self.domain == QuickDomain::Codepoint && ch.is_ascii_hexdigit()
    }

    /// Removes the last character of the quick buffer.
    fn pop_char(&mut self) {
        self.buffer.pop();
    }

    /// The zero-based index the page on show starts at.
    fn page_start(&self, page_size: usize) -> u16 {
        self.paging.page_start(page_size)
    }

    /// The zero-based index of the last page the list fills.
    fn last_page(&self, page_size: usize) -> u8 {
        self.paging.last_page(self.candidates.len(), page_size)
    }

    /// Flips one page, following the highlight to the page it lands on. A flip past
    /// either end is inert, and the key stays claimed: a page key is punctuation to
    /// the application and must not leak through the panel.
    fn flip_page(&mut self, dir: PageDir, page_size: usize) {
        self.paging.flip_page(dir, self.candidates.len(), page_size);
    }

    /// Moves the keyboard highlight, paging when it crosses an edge.
    fn move_highlight(&mut self, delta: i8, page_size: usize) {
        self.paging
            .move_highlight(delta, self.candidates.len(), page_size);
    }
}

#[cfg(test)]
mod tests {
    use super::QuickPanel;
    use ime_core::quick::{QuickCandidate, QuickDomain};
    use ime_types::PageDir;

    /// A panel of `count` candidates in the codepoint domain, the domain that takes
    /// letters as content.
    fn panel(count: usize) -> QuickPanel {
        QuickPanel {
            domain: QuickDomain::Codepoint,
            buffer: String::from("U4f60"),
            candidates: (0..count)
                .map(|_| QuickCandidate {
                    text: String::from("x"),
                    annotation: None,
                })
                .collect(),
            revision: 0,
            paging: super::super::panel::PanelPaging::new(),
        }
    }

    #[test]
    fn test_quick_panel_pop_char_empties_the_buffer_byte_by_byte() {
        // The buffer holds trigger spellings and payload characters only, which the
        // trigger table bounds to ASCII, so one pop is one character.
        let mut panel = panel(3);
        panel.pop_char();
        assert_eq!(panel.buffer, "U4f6");
        panel.pop_char();
        panel.pop_char();
        panel.pop_char();
        assert_eq!(panel.buffer, "U");
        panel.pop_char();
        assert_eq!(panel.buffer, "");
        panel.pop_char();
        assert_eq!(panel.buffer, "");
    }

    #[test]
    fn test_quick_panel_flip_page_stays_inside_the_list() {
        let mut panel = panel(17);
        panel.flip_page(PageDir::Prev, 5);
        assert_eq!(panel.paging.page, 0, "a flip past the first page is inert");
        assert_eq!(panel.paging.highlight, 0);
        panel.paging.page = 3;
        panel.flip_page(PageDir::Next, 5);
        assert_eq!(panel.paging.page, 3, "a flip past the last page is inert");
        panel.flip_page(PageDir::Prev, 5);
        assert_eq!(panel.paging.page, 2);
        assert_eq!(panel.paging.highlight, 10, "the highlight follows the page");
    }

    #[test]
    fn test_quick_panel_move_highlight_clamps_at_both_ends() {
        let mut panel = panel(17);
        panel.move_highlight(50, 5);
        assert_eq!(panel.paging.highlight, 16);
        assert_eq!(panel.paging.page, 3);
        panel.move_highlight(-50, 5);
        assert_eq!(panel.paging.highlight, 0);
        assert_eq!(panel.paging.page, 0);
        panel.move_highlight(7, 5);
        assert_eq!(panel.paging.highlight, 7);
        assert_eq!(panel.paging.page, 1);
    }

    #[test]
    fn test_quick_panel_accepts_hex_letters_only_in_the_codepoint_domain() {
        let mut panel = panel(3);
        assert!(panel.accepts_payload('F'));
        assert!(!panel.accepts_payload('g'));
        assert!(!panel.accepts_payload('+'));
        panel.domain = QuickDomain::Number;
        assert!(
            !panel.accepts_payload('a'),
            "an amount payload takes no letters"
        );
    }
}
