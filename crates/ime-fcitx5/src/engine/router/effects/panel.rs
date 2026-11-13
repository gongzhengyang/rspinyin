//! The machinery the rendered panels share: paging state and the frame a panel
//! posts.
//!
//! # Responsibility
//!
//! Both routing-layer panels — the quick-input panel and the decomposition
//! reverse-lookup panel — carry the same paging state (one page, one keyboard
//! highlight) and post the same kind of frame (the panel buffer as the preedit, one
//! page of `QuickCandidate`s, the badge resolved at post time). This module holds
//! that machinery once: [`PanelPaging`] is the state and its transitions, and
//! [`post_frame`] is the one place a panel's candidates reach the window. The two
//! panels differ in what they admit into their payloads and where their candidates
//! come from; how they page and what they post is identical, and identical here.
//!
//! # Boundary
//!
//! Pure state transitions and one effect-executor call per frame. Nothing reads a
//! clock, a file or the session's candidate list; the frame's anchor travels from
//! the frame context the step wrote, with the placement the panel convention fixes.

use ime_core::quick::QuickCandidate;
use ime_types::{
    Candidate, CandidateSource, LayoutHint, PageDir, PageState, Placement, Preedit, UiCommand,
    UiFrame,
};

use crate::engine::badge;
use crate::engine::host::Host;
use crate::engine::router::context::{Context, resolve_client_preedit};
use crate::ffi::emit_diagnostic;

use super::super::StepCtx;

/// The paging state one rendered panel carries: the page on show and the keyboard
/// highlight, both zero-based and global (the highlight names a candidate of the
/// whole list, not of the page).
///
/// The transitions are the quick-input panel's own, extracted verbatim when the
/// reverse-lookup panel needed the same ones: a flip past either end is inert and
/// the key stays claimed, and the highlight follows the page it lands on.
#[derive(Debug)]
pub(in crate::engine::router) struct PanelPaging {
    /// Zero-based index of the page on show.
    pub(in crate::engine::router) page: u8,
    /// Zero-based global position of the keyboard highlight.
    pub(in crate::engine::router) highlight: u16,
}

impl PanelPaging {
    /// The entry state: the first page, the first candidate.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn new() -> Self {
        Self {
            page: 0,
            highlight: 0,
        }
    }

    /// The zero-based index the page on show starts at.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn page_start(&self, page_size: usize) -> u16 {
        u16::try_from(usize::from(self.page) * page_size.max(1)).unwrap_or(u16::MAX)
    }

    /// The zero-based index of the last page a list of `count` candidates fills.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn last_page(&self, count: usize, page_size: usize) -> u8 {
        let page_size = page_size.max(1);
        u8::try_from(count.saturating_sub(1) / page_size).unwrap_or(0)
    }

    /// Flips one page, following the highlight to the page it lands on. A flip past
    /// either end is inert, and the key stays claimed: a page key is punctuation to
    /// the application and must not leak through the panel.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn flip_page(
        &mut self,
        dir: PageDir,
        count: usize,
        page_size: usize,
    ) {
        let last = self.last_page(count, page_size);
        match dir {
            PageDir::Next => {
                if self.page < last {
                    self.page += 1;
                }
            }
            PageDir::Prev => {
                if self.page > 0 {
                    self.page -= 1;
                }
            }
        }
        self.highlight = self.page_start(page_size);
    }

    /// Moves the keyboard highlight, paging when it crosses an edge.
    ///
    /// # Panics
    ///
    /// Never.
    pub(in crate::engine::router) fn move_highlight(
        &mut self,
        delta: i8,
        count: usize,
        page_size: usize,
    ) {
        if count == 0 {
            return;
        }
        let distance = usize::from(delta.unsigned_abs());
        let next = if delta >= 0 {
            (usize::from(self.highlight) + distance).min(count - 1)
        } else {
            usize::from(self.highlight).saturating_sub(distance)
        };
        self.highlight = u16::try_from(next).unwrap_or(0);
        self.page = u8::try_from(next / page_size.max(1)).unwrap_or(0);
    }
}

/// Posts one panel's frame: the preedit, the show and the frame, in the order the
/// window drops nothing of.
///
/// What a panel frame draws besides the context it posts through: the preedit
/// buffer, the candidate list, the frame identity and the paging head.
pub(in crate::engine::router) struct PanelFrame<'a> {
    /// The preedit buffer the panel's composition shows.
    pub(in crate::engine::router) buffer: &'a str,
    /// The candidates on display, already in final order.
    pub(in crate::engine::router) candidates: &'a [QuickCandidate],
    /// The frame revision that deduplicates stale window events.
    pub(in crate::engine::router) revision: u32,
    /// The highlighted row inside the shown page.
    pub(in crate::engine::router) highlight: u16,
    /// The page the caller asked for; the clamped page comes back.
    pub(in crate::engine::router) requested_page: usize,
}

/// This is the frame path the quick-input panel has always taken, extracted whole
/// when the reverse-lookup panel needed to post the same shape. The page shown is
/// the requested one clamped to the list, and the clamped page is returned so the
/// caller can store it back on its own state.
///
/// # Panics
///
/// Never.
pub(in crate::engine::router) fn post_frame(
    frame: PanelFrame<'_>,
    ctx: &mut Context,
    step: &mut StepCtx<'_, '_>,
    host: &mut dyn Host,
) -> usize {
    let PanelFrame {
        buffer,
        candidates,
        revision,
        highlight,
        requested_page,
    } = frame;
    // The application's preedit area follows the same negotiation an ordinary
    // frame's preedit does: the configuration is the upper bound, the application's
    // capability the floor.
    if resolve_client_preedit(step.config.client_preedit, ctx.preedit_supported) {
        host.set_preedit(ctx.ic, buffer, buffer_caret(buffer));
    } else {
        host.clear_preedit(ctx.ic);
    }
    let mut anchor = ctx.session.frame_context().anchor;
    anchor.placement = Placement::Auto;
    host.post_ui(ctx.ic, UiCommand::Show { revision, anchor });
    let total = candidates.len();
    let page_size = usize::from(step.config.session.max_per_row).max(1);
    let last_page = total.saturating_sub(1) / page_size;
    let page = requested_page.min(last_page);
    let start = page * page_size;
    let end = total.min(start + page_size);
    let page_candidates: Vec<Candidate> = candidates[start..end]
        .iter()
        .enumerate()
        .map(|(position, quick)| Candidate {
            index: u16::try_from(start + position + 1).unwrap_or(u16::MAX),
            text: quick.text.clone(),
            annotation: quick.annotation.clone(),
            source: CandidateSource::Symbol,
            score: 0.0,
            consumed_syllables: 0,
        })
        .collect();
    let highlight = usize::from(highlight);
    let mut frame = UiFrame {
        revision,
        preedit: Preedit {
            text: String::from(buffer),
            caret: buffer_caret(buffer),
            spans: Vec::new(),
        },
        candidates: page_candidates,
        page: PageState {
            current: u8::try_from(page + 1).unwrap_or(u8::MAX),
            total: u8::try_from(last_page + 1).unwrap_or(u8::MAX),
            page_size: u8::try_from(page_size).unwrap_or(9),
        },
        status: ctx.session.frame_context().status.clone(),
        anchor,
        layout: LayoutHint {
            max_per_row: step.config.session.max_per_row,
            show_annotation: step.config.session.show_annotation,
            max_width_dp: step.config.session.max_width_dp,
        },
        highlight: (highlight >= start && highlight < end)
            .then(|| u16::try_from(highlight - start).unwrap_or(0)),
    };
    // The header's slot resolves where every frame resolves: here, with the frame's
    // own page and candidates known and the process-local hint state at hand. The
    // panels' lists are far under the display limit, so the overflow bit is always
    // the honest `false`.
    let has_candidates = !frame.candidates.is_empty();
    let mode = ctx
        .modes
        .mode_label(ctx.session.temp_english, step.config.scheme_hint);
    let (label, overflow_started) = badge::resolve(
        mode,
        &step.config.keys,
        frame.page,
        has_candidates,
        false,
        step.badge,
    );
    if overflow_started {
        emit_diagnostic(badge::UI_CANDIDATE_OVERFLOW_CODE);
    }
    frame.status.mode_label = label;
    host.post_ui(ctx.ic, UiCommand::Frame(Box::new(frame)));
    page
}

/// The caret a panel's preedit carries: the end of the buffer, which is ASCII by
/// the panels' construction and therefore always on a character boundary.
fn buffer_caret(buffer: &str) -> u32 {
    u32::try_from(buffer.len()).unwrap_or(u32::MAX)
}
