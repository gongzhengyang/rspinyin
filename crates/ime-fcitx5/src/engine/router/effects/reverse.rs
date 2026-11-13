//! The decomposition reverse-lookup panel: the routing layer's face of the
//! `u`-led domain.
//!
//! # Responsibility
//!
//! When the input buffer names a reverse query — a lowercase `u` followed by one to
//! four lowercase letters, per the recognizer frozen in `ime_core::reverse` — this
//! module takes the keys over: it renders the matching characters into the candidate
//! window, each annotated with its full decomposition code, narrows the list as the
//! payload grows, pages it, commits a selection through the same door an ordinary
//! commit uses, and hands keys it cannot use back to the pipeline so the composition
//! underneath continues as if the panel had never opened.
//!
//! # The claim rule and its collisions
//!
//! Lowercase `u` is a legitimate start of ordinary pinyin input, so the domain
//! claims as little as the recognizer it serves already does: a bare `u` never opens
//! anything (it keeps composing as the syllable it is), a payload holding a digit or
//! any non-letter never selects this domain (the lowercase codepoint spelling
//! `u4f60` keeps working), and past four letters the buffer is ordinary input again.
//! What remains is the deliberate overlap the frozen module documentation names: a
//! pure-letter payload such as `uab` matches both this domain and the codepoint
//! spelling, and the effect executor resolves it by consulting this domain *before*
//! the quick trigger table. An absent index, a switch the configuration turned off,
//! another domain owning the input, or a query with no hits each hand the buffer
//! back untouched — the last of these is what keeps a `u`-initial composition whose
//! payload matches nothing (a miss, not yet a hit) composing as ordinary pinyin, so
//! the panel can only ever take a buffer the index actually answers. With the
//! feature off — the shipped default — the executor never asks, and a `u`-led
//! buffer routes byte for byte as it did before the domain existed.
//!
//! # Why the panel is here and not in the session
//!
//! The session's candidate list is the decoder's; the panel's is rendered from the
//! reverse index. Rather than teaching the state machine a second candidate source,
//! the panel keeps its own small state on the context and answers before the session
//! steps: a key the panel claims never reaches the machine, so the two candidate
//! lists can never be confused. The session underneath keeps composing the letters
//! it was given up to the trigger; when the panel closes without committing — a
//! backspace out of the payload, a key the payload cannot use — the event falls
//! through and the session continues exactly where its own buffer stands.
//!
//! # Threading
//!
//! Everything here runs on the host thread inside one `key_event` call. The panel
//! state is context state; nothing is shared, nothing blocks, and the only host
//! calls are the same commit, preedit and `post_ui` calls the frame machinery makes.
//! The index reads are bounded binary searches over an immutable mapped image, and
//! neither the payload nor the hit text reaches a diagnostic.

use ime_core::quick::QuickCandidate;
use ime_core::reverse::{self, ABSENT_INDEX_CODE, ReverseMatch};
use ime_core::state::SessionEvent;
use ime_types::{KeyAction, PageDir, UiEvent};

use crate::engine::host::Host;
use crate::ffi::emit_diagnostic;

use super::super::StepCtx;
use super::super::context::Context;
use super::panel::{PanelFrame, PanelPaging, post_frame};

/// The stable code for a query the index cannot answer out of a container that has
/// already passed structural verification (`domain/action/reason` form; the router
/// owns the emission, this module owns the code). The
/// [`ReverseSource`] contract calls the condition impossible for a verified
/// container, so the arm is defensive and the error is never treated as a miss.
pub(in crate::engine::router) const ROW_UNREADABLE_CODE: &str = "reverse/row-unreadable";

/// The state of one open reverse-lookup panel.
///
/// The visibility reaches the whole router module tree because the context — the
/// panel's owner — is a sibling of this module, not a descendant.
#[derive(Debug)]
pub(in crate::engine::router) struct ReversePanel {
    /// The whole reverse buffer the panel answers, trigger and payload: `uhf`.
    buffer: String,
    /// The candidates, in container order — the whole list, not the page on show.
    /// Capped by `ime_core::reverse::MAX_CANDIDATES` at the render, which is the
    /// bound the render contract states.
    candidates: Vec<QuickCandidate>,
    /// The revision of the last frame posted for this panel; UI events carrying any
    /// other revision are stale and dropped.
    revision: u32,
    /// The page on show and the keyboard highlight, shared with the quick panel.
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
    /// The panel has no answer; the event continues to the pipeline.
    Pass,
}

/// Answers one routed event for the reverse-lookup layer.
///
/// The caller asks only when the feature is enabled or a panel is open; with the
/// switch off and no panel this is never reached, which is what keeps the shipped
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
    // whether or not the feature is still enabled. A reload that switched the domain
    // off must not leave a panel that the next enable would resurrect.
    match event {
        SessionEvent::Key(action) => key(ctx, *action, step, host),
        SessionEvent::Ui(ui_event) => ui(ctx, ui_event, step, host),
        SessionEvent::ConfigReloaded(_) | SessionEvent::FocusLost | SessionEvent::Reset => {
            ctx.reverse = None;
            Verdict::Pass
        }
        SessionEvent::PreeditCleared | SessionEvent::CommitDone | SessionEvent::PinApplied => {
            Verdict::Pass
        }
    }
}

/// Checks, after a composing key landed in the session buffer, whether the buffer now
/// names a reverse query the panel should open for.
///
/// This is the letter-trigger path: the trigger and the payload letters reach the
/// session first — `uh` composes as ordinary (dead) input while it is being typed —
/// and the panel takes over only once the whole spelling stands and every gate
/// passes. The gates, in order: no panel of this domain open yet, no other domain
/// owning the input, the dictionary actually carrying the reverse index (the absent
/// one is the once-per-process degradation report), the index answering the query,
/// and the query rendering at least one candidate. A gate that fails leaves the
/// session's own frame up, which is the ordinary decode the buffer composes into.
///
/// # Panics
///
/// Never.
pub(super) fn open_from_buffer(ctx: &mut Context, step: &mut StepCtx<'_, '_>, host: &mut dyn Host) {
    if ctx.reverse.is_some() {
        return;
    }
    let raw = ctx.session.buf.raw();
    let Some(matched) = reverse::recognize(raw) else {
        return;
    };
    // The domains that own the input whole: a view or panel already on screen,
    // temporary English, and the symbol panel. A `u` prefix typed while one of them
    // owns the window is that domain's input, not this domain's.
    let other_domain = ctx.aux.is_some()
        || ctx.quick.is_some()
        || ctx.session.temp_english
        || ctx.session.is_symbol_mode();
    if other_domain {
        return;
    }
    let Some(source) = step.reverse.filter(|source| source.has_reverse_index()) else {
        report_absent_index(step.reverse_absent_reported);
        return;
    };
    let hits = match source.reverse_lookup(payload_of(raw, matched)) {
        Ok(hits) => hits,
        Err(_) => {
            // The condition the contract calls impossible for a verified container,
            // surfaced rather than treated as a miss; the buffer keeps composing.
            emit_diagnostic(ROW_UNREADABLE_CODE);
            return;
        }
    };
    let candidates = reverse::candidates(&hits);
    if candidates.is_empty() {
        // A miss is an answer, not an error: no character decomposes this way yet,
        // so the buffer keeps composing as ordinary input.
        return;
    }
    let mut panel = ReversePanel {
        buffer: String::from(raw),
        candidates,
        revision: 0,
        paging: PanelPaging::new(),
    };
    show(&mut panel, ctx, step, host);
    ctx.reverse = Some(panel);
}

/// Reports the absent-index degradation, once per process.
///
/// The code is a once-per-process report the [`ReverseSource`] contract names:
/// the first `u`-payload typed against a container without the index spends the
/// router's flag, and every later one stays silent.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn report_absent_index(reported: &mut bool) {
    if !*reported {
        emit_diagnostic(ABSENT_INDEX_CODE);
        *reported = true;
    }
}

/// Answers one key for the panel.
fn key(
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    // The domain has no digit-first admission: the payload is letters, and a letter
    // opens through `open_from_buffer` after the session composed it. A key arriving
    // while no panel is open is therefore never this layer's.
    let Some(mut panel) = ctx.reverse.take() else {
        return Verdict::Pass;
    };
    let (verdict, keep) = panel_key(&mut panel, ctx, action, step, host);
    if keep {
        ctx.reverse = Some(panel);
    }
    verdict
}

/// Answers one key for an open panel. The bool answers whether the panel stays open.
fn panel_key(
    panel: &mut ReversePanel,
    ctx: &mut Context,
    action: KeyAction,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let page_size = usize::from(step.config.session.max_per_row);
    match action {
        // A selection digit is payload-shaped content the payload cannot take: the
        // fragment grows only by letters, and a digit that leaked to the session
        // would act on a candidate list the user cannot see. The key stays claimed
        // and the rendering does not move, the way an over-long payload is clamped.
        KeyAction::SelectIndex(_) => (Verdict::Claimed(false), true),
        KeyAction::CommitHighlighted => {
            let index = usize::from(panel.paging.highlight);
            if index >= panel.candidates.len() {
                return (Verdict::Claimed(false), true);
            }
            (commit(panel, index, ctx, host), false)
        }
        // Enter is the raw-text way out: the panel closes and the session commits
        // the letters it holds, which is what a user who wanted the spelling rather
        // than a decomposition asked for.
        KeyAction::CommitRaw | KeyAction::Escape => (Verdict::Pass, false),
        // One keystroke removes one letter of the payload. While what is left still
        // names a query the panel re-renders; the moment it does not — the bare
        // trigger — the panel closes and the session's own backspace takes over,
        // with the two buffers back in step.
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
            panel
                .paging
                .flip_page(dir, panel.candidates.len(), page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::PageFirst | KeyAction::PageLast => {
            panel.paging.page = if action == KeyAction::PageFirst {
                0
            } else {
                panel.paging.last_page(panel.candidates.len(), page_size)
            };
            panel.paging.highlight = panel.paging.page_start(page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::MoveHighlight(delta) => {
            panel
                .paging
                .move_highlight(delta, panel.candidates.len(), page_size);
            show(panel, ctx, step, host);
            (Verdict::Claimed(true), true)
        }
        KeyAction::InputChar(ch) => {
            if panel.accepts_payload(ch) {
                grow(panel, ch, ctx, step, host)
            } else {
                // A key the payload cannot use closes the panel and falls through:
                // the session's buffer holds the letters, so the composition
                // continues exactly where the user's typing left it.
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
        // saving — have nothing to act on while the panel covers the list. The
        // panel closes and the session answers, so the chord does what it means
        // rather than what a hidden list would make it.
        _ => (Verdict::Pass, false),
    }
}

/// Grows the panel's payload by one letter, when the longer buffer still names a
/// query the index answers.
///
/// A payload past the recognizer's bound, and a fragment the index holds nothing
/// for, are clamped rather than fatal: the panel keeps its last good rendering and
/// the key stays claimed, because a letter that leaked to the session would act on
/// a candidate list the user cannot see. Extensions of a fragment can only ever
/// shrink the answer, so a clamped miss stays a miss until the user steps back.
fn grow(
    panel: &mut ReversePanel,
    ch: char,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let mut prospective = panel.buffer.clone();
    // The gate above admitted a lowercase letter; the fold is the same normalization
    // the quick panel applies, and a no-op on what actually arrives.
    prospective.push(ch.to_ascii_lowercase());
    let Some(matched) = reverse::recognize(&prospective) else {
        // Past the recognizer's bound the query is finished: the letter is the
        // panel's to eat — the application must not receive an `n` behind a
        // list that still says the query is `uhfgj`.
        return (Verdict::Claimed(true), true);
    };
    let Some(source) = step.reverse else {
        // Unreachable through the gate that opened the panel -- a panel cannot be
        // up without an index -- but a defensive arm, not a second judgment: the
        // panel closes and the session answers, the way the open path aborts.
        return (Verdict::Pass, false);
    };
    let hits = match source.reverse_lookup(payload_of(&prospective, matched)) {
        Ok(hits) => hits,
        Err(_) => {
            emit_diagnostic(ROW_UNREADABLE_CODE);
            return (Verdict::Pass, false);
        }
    };
    let candidates = reverse::candidates(&hits);
    if candidates.is_empty() {
        // A fragment the index holds nothing for clamps like an over-long one:
        // the last good rendering stays up and the extension letter is eaten
        // until the user steps back.
        return (Verdict::Claimed(true), true);
    }
    panel.buffer = prospective;
    panel.candidates = candidates;
    panel.paging = PanelPaging::new();
    show(panel, ctx, step, host);
    (Verdict::Claimed(true), true)
}

/// Shrinks the panel after a backspace, or closes it when the buffer stopped naming
/// a query.
fn shrink(
    panel: &mut ReversePanel,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> (Verdict, bool) {
    let Some(matched) = reverse::recognize(&panel.buffer) else {
        return (Verdict::Pass, false);
    };
    let Some(source) = step.reverse else {
        return (Verdict::Pass, false);
    };
    let hits = match source.reverse_lookup(payload_of(&panel.buffer, matched)) {
        Ok(hits) => hits,
        Err(_) => {
            emit_diagnostic(ROW_UNREADABLE_CODE);
            return (Verdict::Claimed(true), true);
        }
    };
    let candidates = reverse::candidates(&hits);
    if candidates.is_empty() {
        // Stepping back can only ever grow the answer, so a miss here means the
        // container changed under the panel; the session's own backspace takes the
        // key and the panel stands down rather than showing a stale list.
        return (Verdict::Pass, false);
    }
    panel.candidates = candidates;
    panel.paging = PanelPaging::new();
    show(panel, ctx, step, host);
    (Verdict::Claimed(true), true)
}

/// Answers one candidate-window event for an open panel.
fn ui(
    ctx: &mut Context,
    event: &UiEvent,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> Verdict {
    let Some(mut panel) = ctx.reverse.take() else {
        return Verdict::Pass;
    };
    match event {
        UiEvent::Select {
            revision, index, ..
        } => {
            if *revision != panel.revision {
                // A stale click is the session's to drop, as it always was, and it
                // says nothing about the panel the user is looking at.
                ctx.reverse = Some(panel);
                return Verdict::Pass;
            }
            let index = usize::from(*index);
            if index >= panel.candidates.len() {
                ctx.reverse = Some(panel);
                return Verdict::Claimed(false);
            }
            commit(&panel, index, ctx, host)
        }
        UiEvent::Page { revision, dir } => {
            if *revision != panel.revision {
                ctx.reverse = Some(panel);
                return Verdict::Pass;
            }
            panel.paging.flip_page(
                *dir,
                panel.candidates.len(),
                usize::from(step.config.session.max_per_row),
            );
            show(&mut panel, ctx, step, host);
            ctx.reverse = Some(panel);
            Verdict::Claimed(true)
        }
        UiEvent::Dismiss { revision, .. } => {
            if *revision != panel.revision {
                ctx.reverse = Some(panel);
                return Verdict::Pass;
            }
            // The window dismissed the panel; the session cancels the composition
            // the ordinary way, which hides the window under its own revision.
            Verdict::Pass
        }
        UiEvent::Hover { .. } | UiEvent::Rendered { .. } => {
            ctx.reverse = Some(panel);
            Verdict::Pass
        }
    }
}

/// Commits one panel candidate and answers [`Verdict::Committed`].
///
/// The character leaves through the same door an ordinary commit uses — the output
/// switches rewrite it, the host hands it to the application — and learning is
/// untouched: a character found by its shape is a choice, not a spelling, and the
/// panel records nothing.
fn commit(panel: &ReversePanel, index: usize, ctx: &mut Context, host: &mut dyn Host) -> Verdict {
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
/// The frame itself is the machinery the rendered panels share (`post_frame`);
/// this wrapper hands it the panel's own state and takes the clamped page back.
fn show(
    panel: &mut ReversePanel,
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

/// Splits a reverse buffer into the payload its match names.
///
/// `recognize` validated the byte structure of this exact buffer — a one-byte
/// trigger followed by `payload_len` ASCII letters — so the range is inside the
/// slice and on character boundaries by construction, and the default answers only
/// a caller that ignored its own match.
fn payload_of(buffer: &str, matched: ReverseMatch) -> &str {
    buffer
        .get(matched.trigger_len..matched.trigger_len + matched.payload_len)
        .unwrap_or_default()
}

impl ReversePanel {
    /// Whether a typed character is content this panel's payload can take: a
    /// lowercase letter, the only thing a decomposition fragment is made of.
    ///
    /// # Panics
    ///
    /// Never.
    fn accepts_payload(&self, ch: char) -> bool {
        ch.is_ascii_lowercase()
    }

    /// Removes the last letter of the reverse buffer.
    ///
    /// # Panics
    ///
    /// Never.
    fn pop_char(&mut self) {
        self.buffer.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::ReversePanel;
    use ime_core::quick::QuickCandidate;

    /// A panel of `count` candidates over the fixture buffer `uhf`.
    fn panel(count: usize) -> ReversePanel {
        ReversePanel {
            buffer: String::from("uhf"),
            candidates: (0..count)
                .map(|index| QuickCandidate {
                    text: char::from_u32(0x4E00 + index as u32)
                        .unwrap_or('一')
                        .to_string(),
                    annotation: Some(String::from("hf")),
                })
                .collect(),
            revision: 0,
            paging: super::super::panel::PanelPaging::new(),
        }
    }

    #[test]
    fn test_reverse_panel_accepts_lowercase_letters_only() {
        let panel = panel(1);
        assert!(panel.accepts_payload('h'));
        assert!(
            !panel.accepts_payload('4'),
            "a digit is the codepoint spelling"
        );
        assert!(
            !panel.accepts_payload('H'),
            "the uppercase spelling is not ours"
        );
        assert!(!panel.accepts_payload('+'));
    }

    #[test]
    fn test_reverse_panel_pop_char_steps_back_to_the_bare_trigger() {
        // The buffer holds the trigger and payload letters only, which the
        // recognizer bounds to ASCII, so one pop is one character.
        let mut panel = panel(1);
        panel.pop_char();
        assert_eq!(panel.buffer, "uh");
        panel.pop_char();
        assert_eq!(panel.buffer, "u");
        panel.pop_char();
        assert_eq!(panel.buffer, "");
        panel.pop_char();
        assert_eq!(panel.buffer, "");
    }

    #[test]
    fn test_reverse_report_absent_index_spends_the_once_flag_once() {
        // The absent-index report is a once-per-process diagnostic: the first
        // refusal spends the router's flag, and the flag is what keeps every later
        // one silent. The report travels the process diagnostic channel, which the
        // test host cannot read, so the flag is what the test pins.
        let mut reported = false;
        super::report_absent_index(&mut reported);
        assert!(reported, "the first refusal spends the process flag");

        super::report_absent_index(&mut reported);
        assert!(
            reported,
            "the flag stays spent: the guard kept the second report silent"
        );
    }
}
