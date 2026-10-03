//! The engine / UI boundary.
//!
//! One type family per direction: the host thread posts [`UiCommand`] values and
//! the UI thread posts [`UiEvent`] values back. The UI thread only ever reads an
//! immutable [`UiFrame`] snapshot and never touches session state, which is what
//! keeps the two threads physically separable.
//!
//! Delivery semantics are part of the contract, not an implementation detail:
//!
//! | Channel | Shape | Capacity | Overflow behaviour |
//! |---|---|---|---|
//! | `UiCommand::Frame` | latest-wins single slot | 1 | older frame overwritten, counted as `ui.frame.coalesced` |
//! | `UiCommand::Show` / `Hide` | ordered ring queue | 8 | host thread spins up to 200us, then collapses to the newest and counts `ui.control.dropped` |
//! | `UiCommand::Theme` | latest-wins single slot | 1 | older value overwritten |
//! | `UiCommand::Overlay` | latest-wins single slot | 1 | older value overwritten |
//! | `UiEvent::Select` | SPSC bounded queue | 64 | never dropped: the UI thread spins up to 500us, then abandons the click and reports `ui/select/timeout` |
//! | `UiEvent::Hover` | single slot plus 16ms throttle | 1 | posted only when the hovered index changes |
//! | `UiEvent::Page` | ordered ring queue | 16 | as `Show` / `Hide` |
//!
//! `Frame` and `Theme` may be coalesced because a frame is a full snapshot rather
//! than a delta; `Show` and `Hide` may not, because collapsing an ordered pair
//! would leave the candidate window in the wrong visibility state. `Overlay`
//! coalesces for the same reason as `Theme`: an overlay is a mode, and only the
//! newest one describes what the user is looking at.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use core::time::Duration;

use crate::ids::ScreenId;

/// Commands the host thread posts to the UI thread.
///
/// Delivery comes in two tiers: `Frame` / `Theme` / `Overlay` / `Shutdown` are
/// latest-wins and may be coalesced, while `Show` / `Hide` are ordered and must
/// never be dropped. `PartialEq` is derived on top of the frozen definition so
/// that the ordered-queue tests can compare posted commands.
#[derive(Clone, Debug, PartialEq)]
pub enum UiCommand {
    Frame(Box<UiFrame>),
    Show {
        revision: u32,
        anchor: Anchor,
    },
    Hide {
        revision: u32,
        reason: HideReason,
    },
    Theme(ThemeSpec),
    /// The modal overlay drawn in the candidate window's surface, or `None` when
    /// no overlay is open.
    ///
    /// Appended by ADR-0006. A latest-wins slot rather than an ordered queue, for
    /// the same reason as `Theme`: an overlay is a *mode*, so only the newest
    /// value is meaningful, and replaying a queued one would show a panel the user
    /// has already dismissed.
    Overlay(Option<Box<OverlayFrame>>),
    Shutdown,
}

/// Which panel an [`OverlayFrame`] describes.
///
/// Appended to the contract by ADR-0006. The kind selects the layout the UI
/// thread uses; it never carries behaviour, because the UI thread derives
/// nothing from the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    /// The read-only key reference built from the active bindings.
    CheatSheet,
    /// The fuzzy-searchable command list.
    CommandPalette,
    /// The diagnostics readout.
    Diagnostics,
}

/// One panel's worth of content, rendered in the candidate window's surface.
///
/// Appended to the contract by ADR-0006. This carries data only: the engine
/// builds it from the active bindings and posts it; the UI thread draws it
/// verbatim. A `slint::` type must never appear in these fields (ADR-0006
/// decision 4, `OB-4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayFrame {
    /// Which panel this is.
    pub kind: OverlayKind,
    /// Panel title, shown in the header. User-visible text, so Chinese.
    pub title: String,
    /// Grouped rows, in display order.
    pub sections: Vec<OverlaySection>,
    /// Keyboard highlight, or `None` for no highlight. The cheat sheet is always
    /// `None`; the command palette moves it as the user walks the list.
    pub selected: Option<u16>,
    /// Fuzzy-search string. Always empty for the cheat sheet, which has no
    /// search field.
    pub query: String,
}

/// A titled group of [`OverlayEntry`] rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlaySection {
    /// Group heading, e.g. `组字`. User-visible text, so Chinese.
    pub title: String,
    /// Rows in display order.
    pub entries: Vec<OverlayEntry>,
}

/// One `keys -> label` row of an overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayEntry {
    /// The key chord as the user should press it, e.g. `Tab` or `Ctrl+Shift+/`.
    pub keys: String,
    /// What the chord does, e.g. `高亮下一项`. User-visible text, so Chinese.
    pub label: String,
}

/// A complete candidate-window state. The UI thread only reads it and derives
/// nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct UiFrame {
    /// Monotonic; the UI drops a frame whose revision is older than the one it
    /// already holds, which makes out-of-order delivery and replays safe.
    pub revision: u32,
    /// Preedit line shown in the header.
    pub preedit: Preedit,
    /// Candidates for the current page, best first.
    pub candidates: Vec<Candidate>,
    /// Paging state of the candidate grid.
    pub page: PageState,
    /// Mode strip contents.
    pub status: StatusStrip,
    /// Where the window should appear.
    pub anchor: Anchor,
    /// Layout constraints derived from configuration.
    pub layout: LayoutHint,
    /// The candidate the keyboard highlight sits on, as a zero-based position within
    /// this frame's `candidates`, or `None` when the page holds no candidate for it.
    ///
    /// Appended to the frozen struct by ADR-0005. The engine's `Paging` numbers
    /// candidates globally across pages; the engine converts that global highlight to
    /// this page-local position when it builds the frame, so the view never re-derives
    /// it. `None` means "hide the ring", never "keep the previous one": a ring that
    /// outlives its candidate is the defect this field exists to prevent.
    pub highlight: Option<u16>,
}

/// Where the candidate window goes.
///
/// `Eq` is deliberately absent from the derives: `scale` is an `f32`, which has
/// no `Eq` implementation, and `PartialEq` is all the UI needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// Cursor rectangle in screen physical pixels, already rounded by `scale`;
    /// the origin is the top-left corner of the screen.
    pub cursor: RectI,
    /// Screen the cursor is on.
    pub screen: ScreenId,
    /// Device pixel ratio: 1.0, 1.25, 1.5, 2.0 or 3.0.
    pub scale: f32,
    /// Requested side of the cursor.
    pub placement: Placement,
}

/// Requested side of the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Below,
    Above,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preedit {
    pub text: String,
    /// Byte offset; always on a UTF-8 character boundary, guaranteed by the
    /// preedit builder.
    pub caret: u32,
    pub spans: Vec<PreeditSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreeditSpan {
    pub start: u16,
    pub end: u16,
    pub kind: SpanKind,
}

/// What one preedit span represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Syllable,
    Separator,
    Passthrough,
    Cursor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Display number, 1-based; matches the number keys 1..=9.
    pub index: u16,
    /// Full text. The window may truncate what it draws, but this is what gets
    /// committed, so a rendering truncation can never lose text.
    pub text: String,
    /// Grey right-hand annotation: a reading hint or a source label.
    pub annotation: Option<String>,
    pub source: CandidateSource,
    /// Display-only score. Ordering is decided by integer scores inside the
    /// decoder, never by this float.
    pub score: f32,
    /// Number of raw syllables this candidate consumes, used to compute the
    /// remaining preedit after a selection.
    pub consumed_syllables: u16,
}

/// Where a candidate came from; drives the source label and the diagnostics.
///
/// `Phrase` and `Script` were appended by ADR-0005. Appending a variant makes every
/// `match` on this enum non-exhaustive, which is deliberate: the compiler lists the
/// places that have to decide what the new source looks like. `Symbol` is the
/// precedent -- it was defined ahead of its implementation for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    Dict,
    UserDict,
    Learned,
    Passthrough,
    Symbol,
    /// The user's own phrase table.
    Phrase,
    /// A script conversion of another candidate, rather than a reading of the
    /// input.
    Script,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageState {
    pub current: u8,
    pub total: u8,
    pub page_size: u8,
}

/// Which Chinese script the plugin commits in.
///
/// Appended to the contract by ADR-0005. `Default` is [`Script::Simplified`], so a
/// configuration that predates the setting keeps the behaviour it had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Script {
    /// Simplified Chinese.
    #[default]
    Simplified,
    /// Traditional Chinese.
    Traditional,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StatusStrip {
    /// Chinese or English; in Chinese mode it shows the active scheme.
    pub mode_label: String,
    pub full_width: bool,
    pub punctuation_full: bool,
    pub has_user_dict_hit: bool,
    /// Read-only mode: the data directory is not writable, so learning and log
    /// writing are off and the strip shows a lock.
    ///
    /// Appended to the frozen struct by ADR-0001 because the user-data task
    /// requires the UI to surface that degraded mode.
    pub readonly: bool,
    /// Which script the strip should show.
    ///
    /// Appended by ADR-0005, following the `readonly` precedent above: the field is
    /// additive, and the struct already derives `Default`, so the new field's
    /// default is the value every existing construction site gets.
    pub script: Script,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutHint {
    /// Maximum candidates per row; defaults to 5, configurable in 3..=9.
    pub max_per_row: u8,
    pub show_annotation: bool,
    pub max_width_dp: u16,
}

/// Why the candidate window was hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideReason {
    Committed,
    Cancelled,
    FocusLost,
    EmptyInput,
    Shutdown,
}

/// Theme request. The UI thread resolves it into concrete colours; the host
/// thread only ever sends semantic tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSpec {
    pub scheme: ColorScheme,
    pub accent: Rgba8,
    /// Whether compositor blur is requested.
    pub acrylic: bool,
    /// Base surface alpha; defaults to 217 (0.85 x 255).
    pub base_alpha: u8,
    /// Corner radius in dp; defaults to 12.
    pub corner_radius_dp: u16,
    pub scale: f32,
}

/// Light or dark theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Events the UI thread posts back to the host thread.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Mouse click on a candidate. `revision` must match the current frame or the
    /// engine drops the event, which makes a click both idempotent and safe
    /// against races.
    Select {
        revision: u32,
        index: u16,
        trigger: SelectTrigger,
    },
    /// Pointer moved onto a candidate, or off the grid (`None`).
    Hover { revision: u32, index: Option<u16> },
    /// Page forward or backward.
    Page { revision: u32, dir: PageDir },
    /// The window should be dismissed.
    Dismiss {
        revision: u32,
        reason: DismissReason,
    },
    /// Render receipt from the UI thread, used for end-to-end latency probes; it
    /// carries no business meaning.
    Rendered {
        revision: u32,
        raster: Duration,
        presented_at_unix_nanos: u64,
    },
}

/// What the user did to select a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectTrigger {
    Mouse,
    NumberKey,
    Space,
    Enter,
    Tab,
}

/// Page forward or backward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageDir {
    Next,
    Prev,
}

/// Why the candidate window should be dismissed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DismissReason {
    OutsideClick,
    Escape,
    ScrollUpEmpty,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_anchor() -> Anchor {
        Anchor {
            cursor: RectI {
                x: 100,
                y: 200,
                w: 2,
                h: 20,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Below,
        }
    }

    fn sample_frame() -> UiFrame {
        UiFrame {
            revision: 1,
            preedit: Preedit {
                text: String::from("ni'hao"),
                caret: 6,
                spans: Vec::new(),
            },
            candidates: Vec::new(),
            page: PageState {
                current: 1,
                total: 1,
                page_size: 9,
            },
            status: StatusStrip::default(),
            anchor: sample_anchor(),
            layout: LayoutHint {
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
            },
            highlight: Some(0),
        }
    }

    #[test]
    fn test_ui_frame_size_stays_within_budget() {
        assert!(core::mem::size_of::<UiFrame>() <= 256);
    }

    #[test]
    fn test_ui_frame_highlight_is_page_local_or_hidden() {
        let frame = sample_frame();
        assert_eq!(
            frame.highlight,
            Some(0),
            "a fresh page highlights its first candidate, in the page's own numbering"
        );
        // `None` is a state of its own: the view hides the ring rather than keeping the
        // previous one, so the two values must never compare equal.
        let mut hidden = frame.clone();
        hidden.highlight = None;
        assert_ne!(hidden, frame);
    }

    #[test]
    fn test_status_strip_default_is_idle_and_writable() {
        let status = StatusStrip::default();
        assert!(status.mode_label.is_empty());
        assert!(!status.full_width);
        assert!(!status.punctuation_full);
        assert!(!status.has_user_dict_hit);
        assert!(!status.readonly);
    }

    #[test]
    fn test_ui_frame_equality_detects_content_change() {
        let frame = sample_frame();
        let mut same = frame.clone();
        assert_eq!(same, frame);
        same.revision = 2;
        assert_ne!(same, frame);
    }

    #[test]
    fn test_ui_command_equality_distinguishes_control_commands() {
        let anchor = sample_anchor();
        let show = UiCommand::Show {
            revision: 1,
            anchor,
        };
        assert_eq!(show, show.clone());
        assert_ne!(show, UiCommand::Shutdown);

        let reason = HideReason::Committed;
        let hide = UiCommand::Hide {
            revision: 1,
            reason,
        };
        assert_ne!(show, hide);
    }

    #[test]
    fn test_ui_event_select_round_trips_through_clone() {
        let select = UiEvent::Select {
            revision: 3,
            index: 2,
            trigger: SelectTrigger::NumberKey,
        };
        let cloned = select.clone();
        assert_eq!(select, cloned);

        let dismiss = UiEvent::Dismiss {
            revision: 3,
            reason: DismissReason::Escape,
        };
        assert_ne!(select, dismiss);
    }

    #[test]
    fn test_ui_frame_size_stays_within_budget_with_the_overlay_variant_present() {
        // ADR-0006 added a `UiCommand` variant and deliberately left `UiFrame`
        // untouched, so the frame budget above must not have moved. This test
        // exists to fail loudly if someone later folds the overlay back into the
        // frame "to simplify the channel".
        assert!(core::mem::size_of::<UiFrame>() <= 256);
        assert!(
            core::mem::size_of::<OverlayFrame>() > core::mem::size_of::<Box<OverlayFrame>>(),
            "boxing the overlay is what keeps `UiCommand` from growing by the \
             overlay's inline size on the hot Show/Hide queue"
        );
    }

    #[test]
    fn test_ui_command_overlay_distinguishes_open_from_closed() {
        // The `None` arm is how the engine closes a panel. Without it a
        // latest-wins slot could never go back to "no overlay open", and the
        // cheat sheet would be undismissable.
        let open = UiCommand::Overlay(Some(Box::new(sample_overlay())));
        let closed = UiCommand::Overlay(None);
        assert_ne!(open, closed);
        assert_eq!(closed, UiCommand::Overlay(None));
    }

    #[test]
    fn test_ui_command_overlay_equality_detects_a_content_change() {
        let frame = sample_overlay();
        let same = frame.clone();
        assert_eq!(frame, same);

        let mut moved = frame.clone();
        moved.selected = Some(1);
        assert_ne!(frame, moved);

        let mut retitled = frame;
        retitled.title = String::from("命令面板");
        assert_ne!(same, retitled);
    }

    fn sample_overlay() -> OverlayFrame {
        OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: String::from("按键速查"),
            sections: vec![OverlaySection {
                title: String::from("组字"),
                entries: vec![OverlayEntry {
                    keys: String::from("Tab"),
                    label: String::from("高亮下一项"),
                }],
            }],
            selected: None,
            query: String::new(),
        }
    }
}
