//! The wire shape of one frame snapshot: the JSON the mirror file holds.
//!
//! Responsibility: define the file format, and convert between it and the frozen contract
//! types. Nothing here touches the filesystem, reads a clock or looks at the environment.
//!
//! # Why the contract types are restated instead of serialised
//!
//! `crates/ime-types` is frozen, and its `serde` support sits behind a feature this harness
//! does not turn on; deriving `Serialize` for `UiFrame` there is a contract change and needs
//! an ADR. Restating the frame field by field costs one type per field group and buys two
//! things: the file format is written down in one place instead of being implied by another
//! crate's derives, and a field that changes shape in the contract becomes a compile error
//! in the conversions below rather than a snapshot that silently reads back different.
//!
//! Every conversion is total and lossless in both directions: a frame that goes through
//! [`FrameSnapshot::of`](super::FrameSnapshot::of) and back comes out equal to itself.
//!
//! # Naming
//!
//! Each type here is the view of one contract type, and each enum spells the contract's
//! variants out in `snake_case`. A variant appended to the contract makes the conversions
//! below fail to compile until this format names it, which is deliberate: a new candidate
//! source has to enter the file format on purpose rather than by accident.

use ime_types::ui::Script;
use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, PreeditSpan,
    RectI, ScreenId, SpanKind, StatusStrip,
};

/// The preedit line, as a snapshot carries it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreeditView {
    /// Text the header line shows.
    pub text: String,
    /// Caret position, as a byte offset into `text`.
    pub caret: u32,
    /// Highlighted spans of `text`.
    pub spans: Vec<SpanView>,
}

impl PreeditView {
    /// Restates `preedit` as the snapshot's view of it.
    pub(super) fn of(preedit: &Preedit) -> Self {
        Self {
            text: preedit.text.clone(),
            caret: preedit.caret,
            spans: preedit.spans.iter().map(SpanView::of).collect(),
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_preedit(&self) -> Preedit {
        Preedit {
            text: self.text.clone(),
            caret: self.caret,
            // A closure rather than `map(SpanView::to_span)`: the conversion now takes
            // `self` by value, because `SpanView` is `Copy`, and the iterator yields
            // `&SpanView`. The closure lets the method call copy through the reference.
            spans: self.spans.iter().map(|view| view.to_span()).collect(),
        }
    }
}

/// One highlighted span of the preedit line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpanView {
    /// Start offset, as a byte offset into the preedit text.
    pub start: u16,
    /// End offset, exclusive.
    pub end: u16,
    /// What the span represents.
    pub kind: SpanKindName,
}

impl SpanView {
    /// Restates `span` as the snapshot's view of it.
    pub(super) fn of(span: &PreeditSpan) -> Self {
        Self {
            start: span.start,
            end: span.end,
            kind: SpanKindName::of(&span.kind),
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_span(self) -> PreeditSpan {
        PreeditSpan {
            start: self.start,
            end: self.end,
            kind: self.kind.to_span_kind(),
        }
    }
}

/// What one preedit span represents, in the snapshot's own spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKindName {
    /// One syllable of the input.
    Syllable,
    /// A separator the user typed.
    Separator,
    /// A character passed through unchanged.
    Passthrough,
    /// The caret itself.
    Cursor,
}

impl SpanKindName {
    /// Names `kind` in the snapshot's spelling.
    pub(super) fn of(kind: &SpanKind) -> Self {
        match kind {
            SpanKind::Syllable => Self::Syllable,
            SpanKind::Separator => Self::Separator,
            SpanKind::Passthrough => Self::Passthrough,
            SpanKind::Cursor => Self::Cursor,
        }
    }

    /// Rebuilds the contract value this name stands for.
    pub(super) fn to_span_kind(self) -> SpanKind {
        match self {
            Self::Syllable => SpanKind::Syllable,
            Self::Separator => SpanKind::Separator,
            Self::Passthrough => SpanKind::Passthrough,
            Self::Cursor => SpanKind::Cursor,
        }
    }
}

/// One candidate of the current page, as a snapshot carries it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateView {
    /// Display number, 1-based; it is the number key that selects the candidate.
    pub index: u16,
    /// Full text of the candidate, which is what a selection commits.
    pub text: String,
    /// Grey right-hand annotation, when there is one.
    pub annotation: Option<String>,
    /// Where the candidate came from.
    pub source: SourceName,
    /// Display-only score. Ordering is decided by the decoder's integer scores, never by
    /// this float.
    pub score: f32,
    /// Raw syllables this candidate consumes.
    pub consumed_syllables: u16,
}

impl CandidateView {
    /// Restates `candidate` as the snapshot's view of it.
    pub(super) fn of(candidate: &Candidate) -> Self {
        Self {
            index: candidate.index,
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: SourceName::of(&candidate.source),
            score: candidate.score,
            consumed_syllables: candidate.consumed_syllables,
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_candidate(&self) -> Candidate {
        Candidate {
            index: self.index,
            text: self.text.clone(),
            annotation: self.annotation.clone(),
            source: self.source.to_source(),
            score: self.score,
            consumed_syllables: self.consumed_syllables,
        }
    }
}

/// Where a candidate came from, in the snapshot's own spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceName {
    /// The shipped dictionary.
    Dict,
    /// The user's own dictionary.
    UserDict,
    /// A word the plugin learned from what the user committed.
    Learned,
    /// The raw input, passed through unchanged.
    Passthrough,
    /// A symbol lookup.
    Symbol,
    /// The user's own phrase table.
    Phrase,
    /// A script conversion of another candidate.
    Script,
}

impl SourceName {
    /// Names `source` in the snapshot's spelling.
    pub(super) fn of(source: &CandidateSource) -> Self {
        match source {
            CandidateSource::Dict => Self::Dict,
            CandidateSource::UserDict => Self::UserDict,
            CandidateSource::Learned => Self::Learned,
            CandidateSource::Passthrough => Self::Passthrough,
            CandidateSource::Symbol => Self::Symbol,
            CandidateSource::Phrase => Self::Phrase,
            CandidateSource::Script => Self::Script,
        }
    }

    /// Rebuilds the contract value this name stands for.
    pub(super) fn to_source(self) -> CandidateSource {
        match self {
            Self::Dict => CandidateSource::Dict,
            Self::UserDict => CandidateSource::UserDict,
            Self::Learned => CandidateSource::Learned,
            Self::Passthrough => CandidateSource::Passthrough,
            Self::Symbol => CandidateSource::Symbol,
            Self::Phrase => CandidateSource::Phrase,
            Self::Script => CandidateSource::Script,
        }
    }
}

/// The paging state of the candidate grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageView {
    /// Current page, 1-based.
    pub current: u8,
    /// How many pages the candidate list fills.
    pub total: u8,
    /// How many candidates one page holds.
    pub page_size: u8,
}

impl PageView {
    /// Restates `page` as the snapshot's view of it.
    pub(super) fn of(page: &PageState) -> Self {
        Self {
            current: page.current,
            total: page.total,
            page_size: page.page_size,
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_page(self) -> PageState {
        PageState {
            current: self.current,
            total: self.total,
            page_size: self.page_size,
        }
    }
}

/// The mode strip, as a snapshot carries it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusView {
    /// Mode label the strip shows; Chinese in Chinese mode, holding the active scheme.
    pub mode_label: String,
    /// Whether full-width characters are on.
    pub full_width: bool,
    /// Whether full-width punctuation is on.
    pub punctuation_full: bool,
    /// Whether this page holds a hit from the user's own dictionary.
    pub has_user_dict_hit: bool,
    /// Whether the plugin is in read-only mode, which the strip shows as a lock.
    pub readonly: bool,
    /// Whether the strip's mode dot reads Chinese — the engine's own bit, never an
    /// inference from the label's text.
    ///
    /// The serde default keeps an older document a *format* refusal (the version check)
    /// rather than a shape error, the same convention `highlight` follows.
    #[serde(default)]
    pub chinese: bool,
    /// Which script the strip shows.
    pub script: ScriptName,
}

impl StatusView {
    /// Restates `status` as the snapshot's view of it.
    pub(super) fn of(status: &StatusStrip) -> Self {
        Self {
            mode_label: status.mode_label.clone(),
            full_width: status.full_width,
            punctuation_full: status.punctuation_full,
            has_user_dict_hit: status.has_user_dict_hit,
            readonly: status.readonly,
            chinese: status.chinese,
            script: ScriptName::of(&status.script),
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_status(&self) -> StatusStrip {
        StatusStrip {
            mode_label: self.mode_label.clone(),
            full_width: self.full_width,
            punctuation_full: self.punctuation_full,
            has_user_dict_hit: self.has_user_dict_hit,
            readonly: self.readonly,
            chinese: self.chinese,
            script: self.script.to_script(),
        }
    }
}

/// Which Chinese script the plugin commits in, in the snapshot's own spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptName {
    /// Simplified Chinese.
    Simplified,
    /// Traditional Chinese.
    Traditional,
}

impl ScriptName {
    /// Names `script` in the snapshot's spelling.
    pub(super) fn of(script: &Script) -> Self {
        match script {
            Script::Simplified => Self::Simplified,
            Script::Traditional => Self::Traditional,
        }
    }

    /// Rebuilds the contract value this name stands for.
    pub(super) fn to_script(self) -> Script {
        match self {
            Self::Simplified => Script::Simplified,
            Self::Traditional => Script::Traditional,
        }
    }
}

/// Where the candidate window would appear, as a snapshot carries it.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorView {
    /// Cursor rectangle, in screen physical pixels.
    pub cursor: RectView,
    /// Screen the cursor is on.
    pub screen: u32,
    /// Device pixel ratio the window is rasterised with.
    pub scale: f32,
    /// Requested side of the cursor.
    pub placement: PlacementName,
}

impl AnchorView {
    /// Restates `anchor` as the snapshot's view of it.
    pub(super) fn of(anchor: &Anchor) -> Self {
        Self {
            cursor: RectView::of(&anchor.cursor),
            screen: anchor.screen.value(),
            scale: anchor.scale,
            placement: PlacementName::of(&anchor.placement),
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_anchor(self) -> Anchor {
        Anchor {
            cursor: self.cursor.to_rect(),
            screen: ScreenId::new(self.screen),
            scale: self.scale,
            placement: self.placement.to_placement(),
        }
    }
}

/// A rectangle in screen physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RectView {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl RectView {
    /// Restates `rect` as the snapshot's view of it.
    pub(super) fn of(rect: &RectI) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_rect(self) -> RectI {
        RectI {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// Requested side of the cursor, in the snapshot's own spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementName {
    /// Below the cursor.
    Below,
    /// Above the cursor.
    Above,
    /// Whichever side has room.
    Auto,
}

impl PlacementName {
    /// Names `placement` in the snapshot's spelling.
    pub(super) fn of(placement: &Placement) -> Self {
        match placement {
            Placement::Below => Self::Below,
            Placement::Above => Self::Above,
            Placement::Auto => Self::Auto,
        }
    }

    /// Rebuilds the contract value this name stands for.
    pub(super) fn to_placement(self) -> Placement {
        match self {
            Self::Below => Placement::Below,
            Self::Above => Placement::Above,
            Self::Auto => Placement::Auto,
        }
    }
}

/// Layout constraints the window was built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutView {
    /// Maximum candidates per row.
    pub max_per_row: u8,
    /// Whether annotations are drawn.
    pub show_annotation: bool,
    /// Maximum width of the window, in dp.
    pub max_width_dp: u16,
}

impl LayoutView {
    /// Restates `layout` as the snapshot's view of it.
    pub(super) fn of(layout: &LayoutHint) -> Self {
        Self {
            max_per_row: layout.max_per_row,
            show_annotation: layout.show_annotation,
            max_width_dp: layout.max_width_dp,
        }
    }

    /// Rebuilds the contract type this view restates.
    pub(super) fn to_layout(self) -> LayoutHint {
        LayoutHint {
            max_per_row: self.max_per_row,
            show_annotation: self.show_annotation,
            max_width_dp: self.max_width_dp,
        }
    }
}
