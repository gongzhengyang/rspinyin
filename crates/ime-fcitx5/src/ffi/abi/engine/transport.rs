//! The cross-addon frame transport: the engine's half of the wire (ADR-0011).
//!
//! Responsibility: turn each [`UiCommand`] into the `#[repr(C)]` wire shape the user
//! interface addon reads, and hand it to the sink the UI addon registered during the
//! handshake. Before a sink arrives — or after it is cleared on addon unload — the
//! command is dropped with the standing `ui/not-ready` diagnostic, exactly as before
//! this module existed.
//!
//! # Why the wire is transcribed here and not shared
//!
//! The two addon libraries are `dlopen`'d independently and must not gain a link-time
//! coupling through a shared crate (`ADR-0003`/`ADR-0004`, and the same reason three
//! `.cpp` files each carry their own `#[repr(C)]` transcriptions). The field order in
//! this file and in `ime-ui-addon/src/ffi/transport.rs` is therefore a *contract*
//! transcribed twice: the authority is ADR-0011's table, fields are append-only, and
//! both sides carry a size assertion so a transcription that drifts fails to build.
//!
//! # Borrowing rules
//!
//! Every pointer in a wire borrows either the [`UiCommand`] being sent or one of this
//! module's thread-local scratch arrays, and is valid for the duration of the single
//! sink call only. The sink copies what it keeps before returning; nothing here hands
//! out a pointer that outlives the call. The scratch arrays are rebuilt per post in a
//! fixed order — entries first, then the sections that point into them — so a pointer
//! handed to the sink always describes a snapshot no later append can move.
//!
//! # Who may call this
//!
//! `post()` runs on Fcitx5's main loop thread, and so do the registration entry points
//! (both addons initialise on the main thread), which is what makes the plain atomic
//! slot safe: there is no concurrent writer, and an in-flight sink call is serialised
//! against `clear` by the same thread.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use ime_types::ui::Script;
use ime_types::{
    Anchor, Candidate, CandidateSource, ColorScheme, HideReason, OverlayFrame, OverlayKind,
    Placement, Rgba8, SpanKind, ThemeSpec, UiCommand, UiEvent, UiFrame,
};

use crate::ffi::{emit_diagnostic, guard_ffi};

/// Recorded when an event wire arrives that this transcription cannot read (ADR-0011).
const WIRE_MALFORMED_CODE: &str = "ffi/wire-malformed";

// The wire `kind` values. Kept as named constants rather than a Rust enum: the numbers
// are the ABI, and a repr(C) enum here would silently pin Rust's discriminant layout
// into it.
const KIND_FRAME: u32 = 0;
const KIND_SHOW: u32 = 1;
const KIND_HIDE: u32 = 2;
const KIND_THEME: u32 = 3;
const KIND_OVERLAY_OPEN: u32 = 4;
const KIND_OVERLAY_CLOSED: u32 = 5;
const KIND_SHUTDOWN: u32 = 6;

/// Wire bits of the `flags` field, in ADR-0011's order.
const FLAG_FULL_WIDTH: u32 = 1 << 0;
const FLAG_PUNCTUATION_FULL: u32 = 1 << 1;
const FLAG_READONLY: u32 = 1 << 2;
const FLAG_HAS_USER_DICT_HIT: u32 = 1 << 3;

/// A borrowed UTF-8 string on the wire.
///
/// `ptr` is null exactly when `len` is 0: the reader treats the pair as empty then, and
/// as invalid for any other null-or-zero mismatch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinStr {
    pub ptr: *const u8,
    pub len: u32,
}

/// One preedit span: byte range plus the [`SpanKind`] discriminant.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinSpanWire {
    pub start: u16,
    pub end: u16,
    pub kind: u32,
}

/// One candidate. An `annotation` with `len` 0 is "no annotation", not "empty text".
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinCandidateWire {
    pub index: u16,
    pub text: RspinyinStr,
    pub annotation: RspinyinStr,
    pub source: u32,
    pub score: f32,
    pub consumed_syllables: u16,
}

/// A cursor rectangle in screen physical pixels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinRectWire {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// The frame-family wire: one of Frame / Show / Hide / Theme / Shutdown.
///
/// Fields that belong to one kind only keep their place in the layout anyway — the
/// reader decides by `kind` which fields are meaningful, and zero-filling the rest is
/// what the writer does.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinFrameWire {
    pub kind: u32,
    pub revision: u32,
    pub preedit: RspinyinStr,
    pub caret: u32,
    pub spans: *const RspinyinSpanWire,
    pub span_count: u32,
    pub candidates: *const RspinyinCandidateWire,
    pub candidate_count: u32,
    pub page_current: u8,
    pub page_total: u8,
    pub page_size: u8,
    pub mode_label: RspinyinStr,
    pub flags: u32,
    pub script: u32,
    pub cursor: RspinyinRectWire,
    pub screen: i32,
    pub scale: f32,
    pub placement: u32,
    pub max_per_row: u8,
    pub show_annotation: u8,
    pub max_width_dp: u16,
    pub theme_accent: u32,
    pub theme_scheme: u32,
    pub theme_acrylic: u8,
    pub theme_base_alpha: u8,
    pub theme_corner_radius_dp: u16,
    pub theme_scale: f32,
    pub hide_reason: u32,
}

/// One `keys -> label` row of an overlay.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinOverlayEntryWire {
    pub keys: RspinyinStr,
    pub label: RspinyinStr,
}

/// One titled group of overlay rows.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinOverlaySectionWire {
    pub title: RspinyinStr,
    pub entries: *const RspinyinOverlayEntryWire,
    pub entry_count: u32,
}

/// The overlay-family wire: the open panel or the closed slot.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinOverlayWire {
    pub kind: u32,
    pub panel_kind: u32,
    pub title: RspinyinStr,
    pub selected: i32,
    pub query: RspinyinStr,
    pub sections: *const RspinyinOverlaySectionWire,
    pub section_count: u32,
}

/// The sink the user-interface addon registers: one context, two entry points.
///
/// Both function pointers copy out of every borrowed pointer before they return; the
/// context is the UI side's own opaque token this side never dereferences.
#[repr(C)]
pub struct RspinyinUiSink {
    pub ctx: *mut c_void,
    pub frame: extern "C" fn(ctx: *mut c_void, wire: *const RspinyinFrameWire),
    pub overlay: extern "C" fn(ctx: *mut c_void, wire: *const RspinyinOverlayWire),
}

// SAFETY: the sink is written once by the UI addon's initialiser and read by the
// dispatch path, both on the host thread; the context pointer is an opaque token no
// engine-side code dereferences or rewrites, and the entries are plain code pointers.
unsafe impl Sync for RspinyinUiSink {}

/// The one sink slot. `null` is "no channel", the state `post()` degrades in.
static SINK: AtomicPtr<RspinyinUiSink> = AtomicPtr::new(std::ptr::null_mut());

/// Per-thread scratch for the borrowed wire arrays.
///
/// The arrays are rebuilt per post and the pointers into them are only handed to the
/// sink during that post, so clearing and refilling the `Vec`s between posts keeps the
/// assembly allocation-free in steady state.
#[derive(Default)]
struct Scratch {
    spans: Vec<RspinyinSpanWire>,
    candidates: Vec<RspinyinCandidateWire>,
    overlay_entries: Vec<RspinyinOverlayEntryWire>,
    overlay_sections: Vec<RspinyinOverlaySectionWire>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

/// The sink to post through, or `None` while no channel exists.
fn sink() -> Option<&'static RspinyinUiSink> {
    let ptr = SINK.load(Ordering::Acquire);
    // SAFETY: the pointer was stored from a process-lifetime static on the UI side and
    // is only nulled by `clear` on the same thread that posts; no reference derived
    // here outlives the dispatch that read it.
    unsafe { ptr.as_ref() }
}

/// Dispatches one command to the registered sink.
///
/// Returns `false` when no sink is registered, which is the caller's cue to record the
/// standing degradation. Public for the post-path benchmark (`benches/transport.rs`);
/// everything an input-method session posts goes through [`crate::ffi::abi::engine`]'s
/// host boundary, which wraps this.
///
/// # Panics
///
/// Never.
pub fn dispatch(command: &UiCommand) -> bool {
    let Some(sink) = sink() else {
        return false;
    };
    match command {
        UiCommand::Overlay(overlay) => {
            let wire = SCRATCH.with_borrow_mut(|scratch| overlay_wire(overlay.as_deref(), scratch));
            (sink.overlay)(sink.ctx, &wire);
        }
        // The frame family shares one wire; `Show`/`Hide`/`Theme`/`Shutdown` fill only
        // the fields their kind names, and build nothing for the rest.
        command => {
            let wire = SCRATCH.with_borrow_mut(|scratch| frame_wire(command, scratch));
            (sink.frame)(sink.ctx, &wire);
        }
    }
    true
}

/// Builds the frame-family wire for one command over the thread's scratch.
///
/// The reader is kind-gated, so the zeroed remainder of a non-`Frame` wire is
/// meaningless by construction rather than by luck.
fn frame_wire(command: &UiCommand, scratch: &mut Scratch) -> RspinyinFrameWire {
    let mut wire = empty_frame_wire(command_kind(command));
    if let UiCommand::Frame(frame) = command {
        fill_frame(&mut wire, frame, scratch);
        return wire;
    }
    match command {
        UiCommand::Show { revision, anchor } => {
            wire.revision = *revision;
            fill_anchor(&mut wire, anchor);
        }
        UiCommand::Hide { revision, reason } => {
            wire.revision = *revision;
            wire.hide_reason = hide_reason_code(reason);
        }
        UiCommand::Theme(spec) => fill_theme(&mut wire, spec),
        // `Shutdown` and `Overlay` name no payload fields on this wire: the overlay
        // family travels on the overlay sink, and `Shutdown` is its own message.
        _ => {}
    }
    wire
}

/// The zeroed wire for `kind`: every reader is kind-gated, so the untouched fields
/// are meaningless by construction rather than by luck.
fn empty_frame_wire(kind: u32) -> RspinyinFrameWire {
    RspinyinFrameWire {
        kind,
        revision: 0,
        preedit: empty_str(),
        caret: 0,
        spans: std::ptr::null(),
        span_count: 0,
        candidates: std::ptr::null(),
        candidate_count: 0,
        page_current: 0,
        page_total: 0,
        page_size: 0,
        mode_label: empty_str(),
        flags: 0,
        script: 0,
        cursor: RspinyinRectWire {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        screen: 0,
        scale: 0.0,
        placement: 0,
        max_per_row: 0,
        show_annotation: 0,
        max_width_dp: 0,
        theme_accent: 0,
        theme_scheme: 0,
        theme_acrylic: 0,
        theme_base_alpha: 0,
        theme_corner_radius_dp: 0,
        theme_scale: 0.0,
        hide_reason: 0,
    }
}

/// Fills `wire` from a full frame.
fn fill_frame(wire: &mut RspinyinFrameWire, frame: &UiFrame, scratch: &mut Scratch) {
    wire.revision = frame.revision;
    wire.preedit = str_wire(frame.preedit.text.as_bytes());
    wire.caret = frame.preedit.caret;
    scratch.spans.clear();
    scratch
        .spans
        .extend(frame.preedit.spans.iter().map(|span| RspinyinSpanWire {
            start: span.start,
            end: span.end,
            kind: span_kind_code(span.kind),
        }));
    wire.spans = scratch.spans.as_ptr();
    wire.span_count = scratch.spans.len() as u32;
    scratch.candidates.clear();
    scratch
        .candidates
        .extend(frame.candidates.iter().map(candidate_wire));
    wire.candidates = scratch.candidates.as_ptr();
    wire.candidate_count = scratch.candidates.len() as u32;
    wire.page_current = frame.page.current;
    wire.page_total = frame.page.total;
    wire.page_size = frame.page.page_size;
    wire.mode_label = str_wire(frame.status.mode_label.as_bytes());
    let mut flags = 0;
    if frame.status.full_width {
        flags |= FLAG_FULL_WIDTH;
    }
    if frame.status.punctuation_full {
        flags |= FLAG_PUNCTUATION_FULL;
    }
    if frame.status.readonly {
        flags |= FLAG_READONLY;
    }
    if frame.status.has_user_dict_hit {
        flags |= FLAG_HAS_USER_DICT_HIT;
    }
    wire.flags = flags;
    wire.script = script_code(frame.status.script);
    fill_anchor(wire, &frame.anchor);
    wire.max_per_row = frame.layout.max_per_row;
    wire.show_annotation = u8::from(frame.layout.show_annotation);
    wire.max_width_dp = frame.layout.max_width_dp;
}

/// Fills the anchor fields, shared by `Show` and `Frame`.
fn fill_anchor(wire: &mut RspinyinFrameWire, anchor: &Anchor) {
    wire.cursor = RspinyinRectWire {
        x: anchor.cursor.x,
        y: anchor.cursor.y,
        w: anchor.cursor.w,
        h: anchor.cursor.h,
    };
    wire.screen = i32::try_from(anchor.screen.value()).unwrap_or(0);
    wire.scale = anchor.scale;
    wire.placement = placement_code(anchor.placement);
}

/// Fills the theme payload.
fn fill_theme(wire: &mut RspinyinFrameWire, spec: &ThemeSpec) {
    wire.theme_accent = accent_code(&spec.accent);
    wire.theme_scheme = scheme_code(spec.scheme);
    wire.theme_acrylic = u8::from(spec.acrylic);
    wire.theme_base_alpha = spec.base_alpha;
    wire.theme_corner_radius_dp = spec.corner_radius_dp;
    wire.theme_scale = spec.scale;
}

/// Builds the overlay-family wire for one overlay slot value.
///
/// The entry rows are appended first, in section order, and the section rows — whose
/// pointers reach into the entries array — are built only after the last entry landed.
/// Nothing is appended afterwards, so the pointers describe one stable snapshot for
/// the single sink call that follows.
fn overlay_wire(overlay: Option<&OverlayFrame>, scratch: &mut Scratch) -> RspinyinOverlayWire {
    let mut wire = RspinyinOverlayWire {
        kind: if overlay.is_some() {
            KIND_OVERLAY_OPEN
        } else {
            KIND_OVERLAY_CLOSED
        },
        panel_kind: 0,
        title: empty_str(),
        selected: -1,
        query: empty_str(),
        sections: std::ptr::null(),
        section_count: 0,
    };
    let Some(frame) = overlay else {
        return wire;
    };
    wire.panel_kind = overlay_kind_code(frame.kind);
    wire.title = str_wire(frame.title.as_bytes());
    wire.selected = frame.selected.map(i32::from).unwrap_or(-1);
    wire.query = str_wire(frame.query.as_bytes());
    scratch.overlay_entries.clear();
    scratch.overlay_sections.clear();
    for section in &frame.sections {
        for entry in &section.entries {
            scratch.overlay_entries.push(RspinyinOverlayEntryWire {
                keys: str_wire(entry.keys.as_bytes()),
                label: str_wire(entry.label.as_bytes()),
            });
        }
    }
    let mut start = 0usize;
    for section in &frame.sections {
        let count = section.entries.len();
        scratch.overlay_sections.push(RspinyinOverlaySectionWire {
            title: str_wire(section.title.as_bytes()),
            entries: scratch.overlay_entries[start..].as_ptr(),
            entry_count: count as u32,
        });
        start += count;
    }
    wire.sections = scratch.overlay_sections.as_ptr();
    wire.section_count = scratch.overlay_sections.len() as u32;
    wire
}

/// Builds one candidate's wire row.
fn candidate_wire(candidate: &Candidate) -> RspinyinCandidateWire {
    RspinyinCandidateWire {
        index: candidate.index,
        text: str_wire(candidate.text.as_bytes()),
        annotation: match &candidate.annotation {
            Some(annotation) => str_wire(annotation.as_bytes()),
            None => empty_str(),
        },
        source: source_code(candidate.source),
        score: candidate.score,
        consumed_syllables: candidate.consumed_syllables,
    }
}

/// A borrowed byte slice as a wire string.
fn str_wire(bytes: &[u8]) -> RspinyinStr {
    RspinyinStr {
        ptr: bytes.as_ptr(),
        len: bytes.len() as u32,
    }
}

fn empty_str() -> RspinyinStr {
    RspinyinStr {
        ptr: std::ptr::null(),
        len: 0,
    }
}

/// The wire kind of one command.
fn command_kind(command: &UiCommand) -> u32 {
    match command {
        UiCommand::Frame(_) => KIND_FRAME,
        UiCommand::Show { .. } => KIND_SHOW,
        UiCommand::Hide { .. } => KIND_HIDE,
        UiCommand::Theme(_) => KIND_THEME,
        UiCommand::Overlay(_) => KIND_OVERLAY_OPEN,
        UiCommand::Shutdown => KIND_SHUTDOWN,
    }
}

// The wire discriminants. The numbers are the ABI (ADR-0011); each mapping is written
// as an explicit `match` so reordering a contract enum cannot silently renumber the
// wire.

fn span_kind_code(kind: SpanKind) -> u32 {
    match kind {
        SpanKind::Syllable => 0,
        SpanKind::Separator => 1,
        SpanKind::Passthrough => 2,
        SpanKind::Cursor => 3,
    }
}

fn source_code(source: CandidateSource) -> u32 {
    match source {
        CandidateSource::Dict => 0,
        CandidateSource::UserDict => 1,
        CandidateSource::Learned => 2,
        CandidateSource::Passthrough => 3,
        CandidateSource::Symbol => 4,
        CandidateSource::Phrase => 5,
        CandidateSource::Script => 6,
    }
}

fn script_code(script: Script) -> u32 {
    match script {
        Script::Simplified => 0,
        Script::Traditional => 1,
    }
}

fn placement_code(placement: Placement) -> u32 {
    match placement {
        Placement::Below => 0,
        Placement::Above => 1,
        Placement::Auto => 2,
    }
}

fn hide_reason_code(reason: &HideReason) -> u32 {
    match reason {
        HideReason::Committed => 0,
        HideReason::Cancelled => 1,
        HideReason::FocusLost => 2,
        HideReason::EmptyInput => 3,
        HideReason::Shutdown => 4,
    }
}

fn overlay_kind_code(kind: OverlayKind) -> u32 {
    match kind {
        OverlayKind::CheatSheet => 0,
        OverlayKind::CommandPalette => 1,
        OverlayKind::Diagnostics => 2,
    }
}

fn scheme_code(scheme: ColorScheme) -> u32 {
    match scheme {
        ColorScheme::Light => 0,
        ColorScheme::Dark => 1,
    }
}

/// Packs an accent colour as `0xRRGGBBAA`.
fn accent_code(accent: &Rgba8) -> u32 {
    (u32::from(accent.r) << 24)
        | (u32::from(accent.g) << 16)
        | (u32::from(accent.b) << 8)
        | u32::from(accent.a)
}

/// Registers a sink from this crate's own bench target.
///
/// The exported registration symbol answers to `dlsym`, which a bench target cannot
/// use; this is the same store, reached directly. The bench passes a process-lifetime
/// static, which is the same contract the UI glue upholds.
///
/// # Panics
///
/// Never.
#[doc(hidden)]
pub fn register_for_bench(sink: &'static RspinyinUiSink) -> bool {
    SINK.store(std::ptr::from_ref(sink).cast_mut(), Ordering::Release);
    true
}

// The event wire's `kind` values (ADR-0011, P0.01.02). One wire family, growing
// append-only: P0.01.03 appends the anchor kind and its fields.
const EVENT_KIND_SELECT: u32 = 0;
const EVENT_KIND_HOVER: u32 = 1;
const EVENT_KIND_PAGE: u32 = 2;
const EVENT_KIND_DISMISS: u32 = 3;

/// An event the user interface sends back, on the wire.
///
/// The `reason` field is the kind's auxiliary slot: the select trigger for a select,
/// the hover-presence flag for a hover, the page direction for a page, the dismiss
/// reason for a dismissal. One shape, four readings -- the reader is kind-gated, and
/// `P0.01.03`'s anchor variant appends fields rather than reusing these.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinEventWire {
    pub kind: u32,
    pub revision: u32,
    pub index: u16,
    pub reason: u32,
}

// The auxiliary-slot readings, per kind.
const TRIGGER_MOUSE: u32 = 0;
const TRIGGER_NUMBER_KEY: u32 = 1;
const TRIGGER_SPACE: u32 = 2;
const TRIGGER_ENTER: u32 = 3;
const TRIGGER_TAB: u32 = 4;
const HOVER_ABSENT: u32 = 0;
const HOVER_PRESENT: u32 = 1;
const PAGE_NEXT: u32 = 0;
const PAGE_PREV: u32 = 1;
const DISMISS_OUTSIDE_CLICK: u32 = 0;
const DISMISS_ESCAPE: u32 = 1;
const DISMISS_SCROLL_UP_EMPTY: u32 = 2;

/// Reads one event wire back into the [`UiEvent`] it carries, or `None` for a kind or
/// discriminant this version does not know.
fn event_from_wire(wire: &RspinyinEventWire) -> Option<UiEvent> {
    match wire.kind {
        EVENT_KIND_SELECT => Some(UiEvent::Select {
            revision: wire.revision,
            index: wire.index,
            trigger: match wire.reason {
                TRIGGER_MOUSE => ime_types::SelectTrigger::Mouse,
                TRIGGER_NUMBER_KEY => ime_types::SelectTrigger::NumberKey,
                TRIGGER_SPACE => ime_types::SelectTrigger::Space,
                TRIGGER_ENTER => ime_types::SelectTrigger::Enter,
                TRIGGER_TAB => ime_types::SelectTrigger::Tab,
                _ => return None,
            },
        }),
        EVENT_KIND_HOVER => Some(UiEvent::Hover {
            revision: wire.revision,
            index: match wire.reason {
                HOVER_ABSENT => None,
                HOVER_PRESENT => Some(wire.index),
                _ => return None,
            },
        }),
        EVENT_KIND_PAGE => Some(UiEvent::Page {
            revision: wire.revision,
            dir: match wire.reason {
                PAGE_NEXT => ime_types::PageDir::Next,
                PAGE_PREV => ime_types::PageDir::Prev,
                _ => return None,
            },
        }),
        EVENT_KIND_DISMISS => Some(UiEvent::Dismiss {
            revision: wire.revision,
            reason: match wire.reason {
                DISMISS_OUTSIDE_CLICK => ime_types::DismissReason::OutsideClick,
                DISMISS_ESCAPE => ime_types::DismissReason::Escape,
                DISMISS_SCROLL_UP_EMPTY => ime_types::DismissReason::ScrollUpEmpty,
                _ => return None,
            },
        }),
        _ => None,
    }
}

/// The event the user interface sends back, one call per event (ADR-0011, P0.01.02).
///
/// The caller is the engine's own main-loop thread -- the UI addon's drain thread
/// reaches this only through the host's post-event marshalling, which is the constraint
/// that keeps [`crate::session_host`]'s single-threaded contract intact. Returns whether
/// the engine accepted the event for a live session; `false` covers both a context the
/// plugin does not know and a session that rejected it.
///
/// # Panics
///
/// Never.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_event_ingest(
    ic_id: u64,
    wire: *const RspinyinEventWire,
) -> bool {
    guard_ffi(false, || {
        if wire.is_null() {
            emit_diagnostic("ffi/null-event-wire");
            return false;
        }
        // SAFETY: the caller passes a pointer valid for this call; the wire is
        // `repr(C)` with no invalid bit patterns.
        let wire = unsafe { *wire };
        let Some(event) = event_from_wire(&wire) else {
            emit_diagnostic(WIRE_MALFORMED_CODE);
            return false;
        };
        super::with_host(ic_id, |host| crate::session_host::ui_event(ic_id, event, host))
    })
}

/// Registers the UI side's sink. Exported for the UI addon's glue (ADR-0011)./// Registers the UI side's sink. Exported for the UI addon's glue (ADR-0011).
///
/// A `null` `sink` is refused rather than stored: clearing goes through
/// [`rspinyin_engine_clear_ui_sinks`], so the two states "never registered" and
/// "cleared" stay the same state instead of two.
///
/// # Panics
///
/// Never.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_engine_register_ui_sinks(sink: *const RspinyinUiSink) -> bool {
    if sink.is_null() {
        return false;
    }
    SINK.store(sink.cast_mut(), Ordering::Release);
    true
}

/// Clears the sink slot on user-interface addon unload (ADR-0011).
///
/// After this returns, `post()` is back on the `ui/not-ready` degradation until the
/// UI addon registers again.
///
/// # Panics
///
/// Never.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_engine_clear_ui_sinks() {
    SINK.store(std::ptr::null_mut(), Ordering::Release);
}

#[cfg(test)]
mod tests;
