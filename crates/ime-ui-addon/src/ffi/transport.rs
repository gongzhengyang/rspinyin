//! The user-interface addon's half of the cross-addon wire (ADR-0011).
//!
//! Responsibility: expose the sink the engine calls per [`UiCommand`], turn each wire
//! back into the command it carries, and hand it to the UI thread. The wire structs are
//! a transcription of ADR-0011's table — the same table
//! `ime-fcitx5/src/ffi/abi/engine/transport.rs` transcribes on the producing side — and
//! their field order is the ABI: fields are append-only, and both sides assert the
//! sizes their transcription produces so a drift fails to build.
//!
//! # Borrowing rules
//!
//! Every pointer in a wire is valid for the duration of the single sink call that
//! carries it. The parsers below copy what they keep into an owned [`UiCommand`] before
//! returning; nothing borrowed escapes the call, which is the one invariant that makes
//! the engine's scratch reuse safe.
//!
//! # Who calls whom
//!
//! * The engine calls the two sink entry points on Fcitx5's main loop thread, one per
//!   `UiCommand::post`.
//! * The handshake runs the other way: [`register_transport`] asks the C++ glue to find
//!   the engine library and call its `rspinyin_engine_register_ui_sinks` with
//!   [`rspinyin_ui_frame_sink`]'s pointer. The glue owns the probe order
//!   (`RTLD_DEFAULT` first, then `RTLD_NOLOAD` under the sibling path and the bare
//!   soname) and answers which mechanism fired, so the diagnostic can name it.
//!
//! # Panic safety
//!
//! Both sink entry points run under the panic guard and answer with the documented
//! fallback instead of unwinding into C++.

use std::ffi::c_void;
use std::str;

use ime_types::ids::ScreenId;
use ime_types::ui::Script;
use ime_types::{
    Anchor, Candidate, CandidateSource, ColorScheme, HideReason, LayoutHint, OverlayFrame,
    OverlayKind, OverlaySection, PageState, Placement, Preedit, PreeditSpan, RectI, Rgba8,
    SpanKind, StatusStrip, ThemeSpec, UiCommand, UiFrame,
};

use super::emit_diagnostic;
use super::guard_ffi;

// The wire `kind` values, transcribed from ADR-0011. Numbers are the ABI.
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

/// Recorded when a wire arrived that this transcription cannot read.
///
/// The engine and this addon are built from one ADR table; a wire that fails to parse
/// means the two transcriptions drifted, which is a build defect and not a runtime
/// condition a retry could fix.
const WIRE_MALFORMED_CODE: &str = "ffi/wire-malformed";

/// Recorded once when the handshake found no engine to register with.
///
/// The engine keeps its own `ui/not-ready` for the same state, so this line exists for
/// the operator reading the UI addon's side of the log.
const HANDSHAKE_UNAVAILABLE_CODE: &str = "ui/transport/handshake-unavailable";

/// A borrowed UTF-8 string on the wire. `ptr` is null exactly when `len` is 0.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinStr {
    pub ptr: *const u8,
    pub len: u32,
}

/// One preedit span.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RspinyinSpanWire {
    pub start: u16,
    pub end: u16,
    pub kind: u32,
}

/// One candidate row.
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

/// The frame-family wire. Field order is ADR-0011's; only appends are allowed.
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

// Layout assertions for the leaf shapes. The totals were derived once from the ADR
// table; an append that changes them has to change both transcriptions and this
// number, which is the drift detection the ADR promises.
const _: () = assert!(size_of::<RspinyinStr>() == 16);
const _: () = assert!(size_of::<RspinyinSpanWire>() == 8);
const _: () = assert!(size_of::<RspinyinRectWire>() == 16);
const _: () = assert!(size_of::<RspinyinCandidateWire>() == 56);
const _: () = assert!(size_of::<RspinyinFrameWire>() == 144);
const _: () = assert!(size_of::<RspinyinOverlayEntryWire>() == 32);
const _: () = assert!(size_of::<RspinyinOverlaySectionWire>() == 32);
const _: () = assert!(size_of::<RspinyinOverlayWire>() == 64);

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

/// The overlay-family wire.
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

/// The sink the engine holds. The `ctx` is this addon's identity token; both entry
/// points copy out of every borrowed pointer before they return.
#[repr(C)]
pub struct RspinyinUiSink {
    pub ctx: *mut c_void,
    pub frame: extern "C" fn(ctx: *mut c_void, wire: *const RspinyinFrameWire),
    pub overlay: extern "C" fn(ctx: *mut c_void, wire: *const RspinyinOverlayWire),
}

/// The process-lifetime sink the engine registers once and never frees.
///
/// The context is this module's own token: an address that is stable for the process
/// lifetime and that the engine never dereferences, exactly like the vtable context.
static TRANSPORT_CONTEXT: u8 = 0;

// SAFETY: the sink is immutable after construction -- the context pointer is an address
// token that no one dereferences or rewrites, and the two entries are plain code
// pointers. Sharing it across threads is the entire point of the transport: the engine's
// main loop thread reads it while this addon's initialiser wrote it, both on the host
// thread.
unsafe impl Sync for RspinyinUiSink {}

static UI_SINK: RspinyinUiSink = RspinyinUiSink {
    ctx: &TRANSPORT_CONTEXT as *const u8 as *mut c_void,
    frame: frame_sink,
    overlay: overlay_sink,
};

/// Hands the engine's handshake the sink pointer (ADR-0011).
///
/// The address is a `static`'s, so it stays put for the process lifetime and the
/// engine's atomic slot can hold it across addon reloads; `clear` nuls the slot, not
/// this struct.
///
/// # Panics
///
/// Never.
#[unsafe(no_mangle)]
pub extern "C" fn rspinyin_ui_frame_sink() -> *const RspinyinUiSink {
    &UI_SINK
}

/// The engine's `Frame`/`Show`/`Hide`/`Theme`/`Shutdown` entry point.
extern "C" fn frame_sink(_ctx: *mut c_void, wire: *const RspinyinFrameWire) {
    guard_ffi((), || {
        let Some(command) = command_from_wire(wire) else {
            emit_diagnostic(WIRE_MALFORMED_CODE);
            return;
        };
        #[cfg(feature = "test-mirror")]
        if let UiCommand::Frame(frame) = &command {
            crate::mirror::publish(frame);
        }
        // A closed channel is the engine addon's own bookkeeping: the thread it would
        // be posted to is the one this addon owns, so the answer is recorded rather
        // than acted on.
        if !crate::addon::post_command(command) {
            emit_diagnostic("ui/engine-gone");
        }
    });
}

/// The engine's `Overlay` slot entry point.
extern "C" fn overlay_sink(_ctx: *mut c_void, wire: *const RspinyinOverlayWire) {
    guard_ffi((), || {
        let Some(command) = command_from_overlay(wire) else {
            emit_diagnostic(WIRE_MALFORMED_CODE);
            return;
        };
        if !crate::addon::post_command(command) {
            emit_diagnostic("ui/engine-gone");
        }
    });
}

/// Reads a frame-family wire back into the command it carries.
///
/// `None` for a wire this transcription cannot read: a null-or-zero mismatched string,
/// a negative screen id, or a kind this version does not know. The caller records the
/// refusal; nothing partial is ever posted.
fn command_from_wire(wire: *const RspinyinFrameWire) -> Option<UiCommand> {
    if wire.is_null() {
        return None;
    }
    // SAFETY: the engine passes a pointer valid for this call, and `RspinyinFrameWire`
    // is `repr(C)` with no invalid bit patterns.
    let wire = unsafe { *wire };
    match wire.kind {
        KIND_FRAME => {
            let text = read_str(wire.preedit)?;
            // SAFETY: the engine passes a pointer valid for this call; the count is
            // the row count the writer wrote alongside it.
            let spans = unsafe { read_slice(wire.spans, wire.span_count) }?;
            let mut parsed = Vec::with_capacity(spans.len());
            for span in spans {
                parsed.push(PreeditSpan {
                    start: span.start,
                    end: span.end,
                    kind: span_kind(span.kind)?,
                });
            }
            // SAFETY: as the span read, for the candidate array.
            let candidate_rows = unsafe { read_slice(wire.candidates, wire.candidate_count) }?;
            let mut candidates = Vec::with_capacity(candidate_rows.len());
            for row in candidate_rows {
                candidates.push(read_candidate(row)?);
            }
            let status = StatusStrip {
                mode_label: read_str(wire.mode_label)?.to_owned(),
                full_width: wire.flags & FLAG_FULL_WIDTH != 0,
                punctuation_full: wire.flags & FLAG_PUNCTUATION_FULL != 0,
                readonly: wire.flags & FLAG_READONLY != 0,
                has_user_dict_hit: wire.flags & FLAG_HAS_USER_DICT_HIT != 0,
                script: script(wire.script)?,
            };
            Some(UiCommand::Frame(Box::new(UiFrame {
                revision: wire.revision,
                preedit: Preedit {
                    text: text.to_owned(),
                    caret: wire.caret,
                    spans: parsed,
                },
                candidates,
                page: PageState {
                    current: wire.page_current,
                    total: wire.page_total,
                    page_size: wire.page_size,
                },
                status,
                anchor: read_anchor(&wire.cursor, wire.screen, wire.scale, wire.placement)?,
                layout: LayoutHint {
                    max_per_row: wire.max_per_row,
                    show_annotation: wire.show_annotation != 0,
                    max_width_dp: wire.max_width_dp,
                },
            })))
        }
        KIND_SHOW => Some(UiCommand::Show {
            revision: wire.revision,
            anchor: read_anchor(&wire.cursor, wire.screen, wire.scale, wire.placement)?,
        }),
        KIND_HIDE => Some(UiCommand::Hide {
            revision: wire.revision,
            reason: hide_reason(wire.hide_reason)?,
        }),
        KIND_THEME => Some(UiCommand::Theme(read_theme(&wire)?)),
        KIND_SHUTDOWN => Some(UiCommand::Shutdown),
        _ => None,
    }
}

/// Reads an overlay-family wire back into the command it carries.
fn command_from_overlay(wire: *const RspinyinOverlayWire) -> Option<UiCommand> {
    if wire.is_null() {
        return None;
    }
    // SAFETY: as `command_from_wire`.
    let wire = unsafe { *wire };
    match wire.kind {
        KIND_OVERLAY_CLOSED => Some(UiCommand::Overlay(None)),
        KIND_OVERLAY_OPEN => {
            // SAFETY: the engine passes a pointer valid for this call.
            let sections = unsafe { read_slice(wire.sections, wire.section_count) }?;
            let mut parsed = Vec::with_capacity(sections.len());
            for section in sections {
                let title = read_str(section.title)?.to_owned();
                // SAFETY: as the section read, for this section's rows.
                let rows = unsafe { read_slice(section.entries, section.entry_count) }?;
                let mut entries = Vec::with_capacity(rows.len());
                for row in rows {
                    entries.push(ime_types::OverlayEntry {
                        keys: read_str(row.keys)?.to_owned(),
                        label: read_str(row.label)?.to_owned(),
                    });
                }
                parsed.push(OverlaySection { title, entries });
            }
            Some(UiCommand::Overlay(Some(Box::new(OverlayFrame {
                kind: overlay_kind(wire.panel_kind)?,
                title: read_str(wire.title)?.to_owned(),
                sections: parsed,
                selected: if wire.selected < 0 {
                    None
                } else {
                    u16::try_from(wire.selected).ok()
                },
                query: read_str(wire.query)?.to_owned(),
            }))))
        }
        _ => None,
    }
}

/// Reads one borrowed string, or `None` when the pair is malformed.
///
/// A null pointer with a zero length is the empty string; anything else with a null
/// pointer, or bytes that are not UTF-8, is a drifted wire.
fn read_str<'a>(raw: RspinyinStr) -> Option<&'a str> {
    // SAFETY: the engine passes a pointer valid for this call; a null pointer with a
    // zero length is the empty string, which read_slice answers directly.
    let bytes = unsafe { read_slice(raw.ptr, raw.len) }?;
    str::from_utf8(bytes).ok()
}

/// Reads a borrowed wire array, tolerating the empty shape.
///
/// # Safety
///
/// When `count` is non-zero, `ptr` must point to `count` initialised elements that stay
/// valid and unaliased for reads for the whole of `'a`.
unsafe fn read_slice<'a, T>(ptr: *const T, count: u32) -> Option<&'a [T]> {
    if count == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller upholds the validity contract documented above.
    Some(unsafe { std::slice::from_raw_parts(ptr, count as usize) })
}

/// Reads one candidate row.
fn read_candidate(row: &RspinyinCandidateWire) -> Option<Candidate> {
    // "No annotation" is the null-and-zero shape the writer sends; an empty annotation
    // travels as a live pointer with a zero length, so the two stay distinct on the
    // wire even though both read as empty text.
    let annotation = if row.annotation.ptr.is_null() && row.annotation.len == 0 {
        None
    } else {
        Some(read_str(row.annotation)?.to_owned())
    };
    Some(Candidate {
        index: row.index,
        text: read_str(row.text)?.to_owned(),
        annotation,
        source: source(row.source)?,
        score: row.score,
        consumed_syllables: row.consumed_syllables,
    })
}

/// Reads the anchor fields.
fn read_anchor(
    cursor: &RspinyinRectWire,
    screen: i32,
    scale: f32,
    placement: u32,
) -> Option<Anchor> {
    Some(Anchor {
        cursor: RectI {
            x: cursor.x,
            y: cursor.y,
            w: cursor.w,
            h: cursor.h,
        },
        screen: ScreenId::new(u32::try_from(screen).ok()?),
        scale,
        placement: placement_enum(placement)?,
    })
}

/// Reads the theme payload.
fn read_theme(wire: &RspinyinFrameWire) -> Option<ThemeSpec> {
    Some(ThemeSpec {
        scheme: scheme(wire.theme_scheme)?,
        accent: Rgba8 {
            r: ((wire.theme_accent >> 24) & 0xff) as u8,
            g: ((wire.theme_accent >> 16) & 0xff) as u8,
            b: ((wire.theme_accent >> 8) & 0xff) as u8,
            a: (wire.theme_accent & 0xff) as u8,
        },
        acrylic: wire.theme_acrylic != 0,
        base_alpha: wire.theme_base_alpha,
        corner_radius_dp: wire.theme_corner_radius_dp,
        scale: wire.theme_scale,
    })
}

// The reverse discriminant maps. Like the writer's, each is an explicit `match` over
// the ADR-0011 numbers, so reordering a contract enum cannot silently renumber it.

fn span_kind(code: u32) -> Option<SpanKind> {
    match code {
        0 => Some(SpanKind::Syllable),
        1 => Some(SpanKind::Separator),
        2 => Some(SpanKind::Passthrough),
        3 => Some(SpanKind::Cursor),
        _ => None,
    }
}

fn source(code: u32) -> Option<CandidateSource> {
    match code {
        0 => Some(CandidateSource::Dict),
        1 => Some(CandidateSource::UserDict),
        2 => Some(CandidateSource::Learned),
        3 => Some(CandidateSource::Passthrough),
        4 => Some(CandidateSource::Symbol),
        5 => Some(CandidateSource::Phrase),
        6 => Some(CandidateSource::Script),
        _ => None,
    }
}

fn script(code: u32) -> Option<Script> {
    match code {
        0 => Some(Script::Simplified),
        1 => Some(Script::Traditional),
        _ => None,
    }
}

fn placement_enum(code: u32) -> Option<Placement> {
    match code {
        0 => Some(Placement::Below),
        1 => Some(Placement::Above),
        2 => Some(Placement::Auto),
        _ => None,
    }
}

fn hide_reason(code: u32) -> Option<HideReason> {
    match code {
        0 => Some(HideReason::Committed),
        1 => Some(HideReason::Cancelled),
        2 => Some(HideReason::FocusLost),
        3 => Some(HideReason::EmptyInput),
        4 => Some(HideReason::Shutdown),
        _ => None,
    }
}

fn overlay_kind(code: u32) -> Option<OverlayKind> {
    match code {
        0 => Some(OverlayKind::CheatSheet),
        1 => Some(OverlayKind::CommandPalette),
        2 => Some(OverlayKind::Diagnostics),
        _ => None,
    }
}

fn scheme(code: u32) -> Option<ColorScheme> {
    match code {
        0 => Some(ColorScheme::Light),
        1 => Some(ColorScheme::Dark),
        _ => None,
    }
}

/// Runs the handshake: asks the glue to find the engine and register [`UI_SINK`].
///
/// Returns whether a sink slot now exists. The glue answers with the probe mechanism
/// that fired, which the success diagnostic carries; a zero answer is the one failure,
/// and it is recorded once rather than retried (ADR-0011).
///
/// # Panics
///
/// Never.
pub(crate) fn register_transport() -> bool {
    // SAFETY: the glue reads no memory this side owns, blocks on nothing, and answers
    // with a code naming the probe mechanism that fired. The pure-Rust build's stand-in
    // reads nothing at all, so the call site is the same on both configurations.
    let mechanism = unsafe { rspinyin_ui_transport_register() };
    match mechanism {
        1 => {
            emit_diagnostic("ui/transport/registered: probe=rtld-default");
            true
        }
        2 => {
            emit_diagnostic("ui/transport/registered: probe=sibling-noload");
            true
        }
        3 => {
            emit_diagnostic("ui/transport/registered: probe=soname-noload");
            true
        }
        _ => {
            emit_diagnostic(HANDSHAKE_UNAVAILABLE_CODE);
            false
        }
    }
}

/// Clears the engine's sink slot on addon unload (ADR-0011).
///
/// # Panics
///
/// Never.
pub(crate) fn unregister_transport() {
    // SAFETY: as `register_transport`.
    unsafe {
        rspinyin_ui_transport_unregister();
    }
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Finds the engine library and registers the sink with it.
    ///
    /// Provided by `src/ffi/cpp/ui_addon_glue.cpp`. Answers 1-3 for the probe mechanism
    /// that fired and 0 when none did.
    ///
    /// # Safety
    ///
    /// Called from the host thread; the callee blocks on nothing and owns nothing the
    /// caller has to release.
    fn rspinyin_ui_transport_register() -> u32;

    /// Clears the engine's sink slot. Provided by the same glue.
    ///
    /// # Safety
    ///
    /// As `rspinyin_ui_transport_register`.
    fn rspinyin_ui_transport_unregister();
}

/// Stand-in for the pure-Rust build, where no glue is linked.
///
/// # Safety
///
/// Nothing is read or written; the caller has no obligations beyond the ones the real
/// function carries.
#[cfg(not(fcitx5_host))]
unsafe fn rspinyin_ui_transport_register() -> u32 {
    0
}

/// Stand-in for the pure-Rust build, where no glue is linked.
///
/// # Safety
///
/// Nothing is read or written.
#[cfg(not(fcitx5_host))]
unsafe fn rspinyin_ui_transport_unregister() {}

#[cfg(test)]
mod tests;
