//! The `UiFrame` snapshot channel: what the candidate window would have drawn, as data.
//!
//! # Why a snapshot stands in for the accessibility tree
//!
//! The candidate window is drawn by the plugin itself, pixel by pixel, into a buffer it
//! shares with the display server. There is no toolkit, no widget tree, and therefore no
//! accessibility tree to read: a harness that claimed to walk one would be describing a
//! structure that does not exist. What does exist is the frame the engine posts to the UI
//! thread -- the frozen [`UiFrame`] contract -- and that is what this module mirrors into a
//! file a case can read.
//!
//! The substitution is exact in what it claims and silent about what it does not: a snapshot
//! says what the window was *asked* to draw, never what appeared on the screen. Pixels are
//! the screenshot channel's business. A case that reported one as the other would be
//! asserting on a frame no user ever saw, so the two channels are kept apart and neither
//! stands in for the other.
//!
//! # The file
//!
//! One JSON object, at `<mirror dir>/ui_frame.json` (the name is
//! [`FRAME_FILE`]), holding the newest frame the engine posted. The mirror directory is the
//! one the sandbox hands out (`$XDG_RUNTIME_DIR/rspinyin-test` for a sandboxed session);
//! this module never resolves that path from the environment itself, because a harness that
//! did could write into the operator's real runtime directory.
//!
//! ```json
//! {
//!   "format": 2,
//!   "revision": 7,
//!   "preedit": { "text": "ni", "caret": 2, "spans": [] },
//!   "candidates": [
//!     { "index": 1, "text": "你", "annotation": null, "source": "dict",
//!       "score": 1.5, "consumed_syllables": 1 }
//!   ],
//!   "page": { "current": 1, "total": 1, "page_size": 9 },
//!   "status": { "mode_label": "拼音", "full_width": false, "punctuation_full": false,
//!               "has_user_dict_hit": false, "readonly": false, "script": "simplified" },
//!   "anchor": { "cursor": { "x": 100, "y": 200, "w": 2, "h": 20 }, "screen": 0,
//!               "scale": 1.0, "placement": "below" },
//!   "layout": { "max_per_row": 5, "show_annotation": true, "max_width_dp": 720 },
//!   "highlight": 0
//! }
//! ```
//!
//! The file is written to `<name>.tmp` and renamed over the named file, so a reader finds
//! either the whole previous snapshot or the whole new one and never a file in between.
//!
//! # The revision rule
//!
//! A frame carries a monotonic revision, and the contract drops a frame older than the one
//! already held. Both sides of the file apply that rule: [`UiFrameMirror::publish`] refuses
//! to overwrite a newer snapshot with an older frame, and [`FrameWatch`] refuses to hand a
//! case a frame older than the one it has already read, counting the event instead. A case
//! therefore asserts on the newest frame rather than on whichever frame a replay happened to
//! leave in the file.
//!
//! # What this module never does
//!
//! It never logs the content of a frame: a snapshot holds what the user typed, and the
//! project's logging rules keep that out of every log, so the frame's text reaches the file
//! and nothing else. It never resolves a path from the environment, and it opens no socket:
//! the whole channel is one file inside the sandbox. It also never touches the decoder or
//! the display server, which is what lets its tests run with no display present.
//!
//! # Modules
//!
//! `mirror` holds the file side -- the writer, the reader and the revision rules -- `schema`
//! holds the wire shape and the conversions to and from the contract types, and `error`
//! holds the refusals.

mod error;
mod mirror;
mod schema;

#[cfg(test)]
mod tests;

pub use self::error::FrameError;
pub use self::mirror::{FRAME_FILE, FrameWatch, Publish, ReadRetry, Reading, UiFrameMirror};
pub use self::schema::{
    AnchorView, CandidateView, LayoutView, PageView, PlacementName, PreeditView, RectView,
    ScriptName, SourceName, SpanKindName, SpanView, StatusView,
};

use ime_types::UiFrame;

/// Version of the snapshot file format this build writes and reads.
///
/// A frame has no version of its own; the file does. A reader that finds a version it does
/// not know refuses the file rather than reading fields that may have changed meaning, which
/// is the one failure a format without a version cannot report.
///
/// Version 2 appended `highlight` (ADR-0005's incremental `UiFrame` extension). The field is
/// declared with a serde default, so a version-1 document still *parses* and is then refused
/// by the version check below as a format refusal -- the one gate the rule names -- rather
/// than as a shape error.
///
/// Version 3 appended `chinese` to the status view (ADR-0005's `StatusStrip` extension):
/// the mode dot's bit, which a reader must not re-derive from the label's text.
pub const FRAME_FORMAT_VERSION: u32 = 3;

/// The structured view of one frame: what the candidate window would have drawn.
///
/// This is the substitute for the accessibility tree the project does not have, and it is
/// also the file format: the fields below are the JSON object the mirror holds, in this
/// order. It is comparable and cloneable, so a case can assert a snapshot against a snapshot
/// it built by hand, and it is total in both directions -- [`FrameSnapshot::of`] followed by
/// [`FrameSnapshot::to_frame`] returns the frame it started from, field for field.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameSnapshot {
    /// Version of the snapshot file format this snapshot is written in.
    pub format: u32,
    /// Revision of the frame this snapshot was taken from.
    pub revision: u32,
    /// The preedit line shown in the header.
    pub preedit: PreeditView,
    /// Candidates of the current page, best first.
    pub candidates: Vec<CandidateView>,
    /// Paging state of the candidate grid.
    pub page: PageView,
    /// Mode strip contents.
    pub status: StatusView,
    /// Where the window would appear.
    pub anchor: AnchorView,
    /// Layout constraints the window was built with.
    pub layout: LayoutView,
    /// The keyboard highlight, as a zero-based position within the page's candidates,
    /// or `None` when the page holds no candidate for it.
    ///
    /// Appended by the format's second version, mirroring the contract's own tail
    /// append (ADR-0005). The default keeps a document written by the first version
    /// parseable, so the version check is what refuses it.
    #[serde(default)]
    pub highlight: Option<u16>,
}

impl FrameSnapshot {
    /// Takes the snapshot of `frame`.
    ///
    /// # Return value
    ///
    /// A view of every field of `frame`, stamped with the format version this build writes.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn of(frame: &UiFrame) -> Self {
        Self {
            format: FRAME_FORMAT_VERSION,
            revision: frame.revision,
            preedit: PreeditView::of(&frame.preedit),
            candidates: frame.candidates.iter().map(CandidateView::of).collect(),
            page: PageView::of(&frame.page),
            status: StatusView::of(&frame.status),
            anchor: AnchorView::of(&frame.anchor),
            layout: LayoutView::of(&frame.layout),
            highlight: frame.highlight,
        }
    }

    /// Rebuilds the frame this snapshot was taken from.
    ///
    /// # Return value
    ///
    /// The frame, with every field the contract defines restored from the view.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn to_frame(&self) -> UiFrame {
        UiFrame {
            revision: self.revision,
            preedit: self.preedit.to_preedit(),
            candidates: self
                .candidates
                .iter()
                .map(CandidateView::to_candidate)
                .collect(),
            page: self.page.to_page(),
            status: self.status.to_status(),
            anchor: self.anchor.to_anchor(),
            layout: self.layout.to_layout(),
            highlight: self.highlight,
        }
    }

    /// Renders the snapshot as the JSON text the mirror file holds.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::Encode`] when a field cannot be written as JSON. The only
    /// fields that can refuse are the two floats -- a candidate's display score and the
    /// anchor's device pixel ratio -- because a not-a-number or infinite float is written as
    /// `null` and would read back as a type error rather than as the value that went in.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn to_json(&self) -> Result<String, FrameError> {
        self.check_writable()?;
        serde_json::to_string(self).map_err(|error| FrameError::Encode {
            detail: format!(
                "{:?} at line {} column {}",
                error.classify(),
                error.line(),
                error.column()
            ),
        })
    }

    /// How many candidates the page holds.
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// The candidates' texts, in display order.
    ///
    /// A case asserts on this rather than on the views when what it is checking is the order
    /// the engine chose, which is the one property the window may not change while drawing.
    pub fn texts(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect()
    }

    /// Returns `true` when the window would draw a header and no candidates.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// How much of the current page the frame fills.
    pub fn page_fill(&self) -> PageFill {
        PageFill {
            occupied: u8::try_from(self.candidates.len()).unwrap_or(u8::MAX),
            capacity: self.page.page_size,
        }
    }

    /// Returns `true` when the page holds at least the candidates it is sized for.
    pub fn is_page_full(&self) -> bool {
        self.page_fill().is_full()
    }

    /// Refuses a snapshot whose floating-point fields cannot survive the file.
    ///
    /// Both floats are display-only -- the candidate score and the device pixel ratio -- so
    /// refusing them loses no ordering information, and writing them would leave a file that
    /// every reader refuses in place of the last good snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::Encode`] naming the candidate whose score is not a finite
    /// number, or the anchor whose ratio is not.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn check_writable(&self) -> Result<(), FrameError> {
        if let Some(candidate) = self.candidates.iter().find(|view| !view.score.is_finite()) {
            return Err(FrameError::Encode {
                detail: format!(
                    "candidate {} has a display score that is not a finite number",
                    candidate.index
                ),
            });
        }
        if !self.anchor.scale.is_finite() {
            return Err(FrameError::Encode {
                detail: String::from("the anchor's device pixel ratio is not a finite number"),
            });
        }
        Ok(())
    }
}

/// How much of the current page a frame fills.
///
/// The three states a case asserts on are the frame with no candidates, the partly filled
/// page and the full page. Naming them here keeps the comparison out of the cases, and keeps
/// the empty page from also counting as full when the page state says it can hold nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFill {
    /// Candidates the page holds.
    pub occupied: u8,
    /// Candidates the page is sized for.
    pub capacity: u8,
}

impl PageFill {
    /// Returns `true` when the page holds no candidates.
    pub fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    /// Returns `true` when the page holds at least the candidates it is sized for.
    ///
    /// A capacity of zero is never full: a frame whose page state claims it can draw nothing
    /// would otherwise be both empty and full at once.
    pub fn is_full(&self) -> bool {
        self.capacity > 0 && self.occupied >= self.capacity
    }

    /// Returns `true` when the page holds some candidates and has room for more.
    pub fn is_partial(&self) -> bool {
        self.occupied > 0 && self.occupied < self.capacity
    }
}
