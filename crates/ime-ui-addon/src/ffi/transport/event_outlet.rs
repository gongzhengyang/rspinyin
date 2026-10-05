//! The event return channel: the wire, the encoders, and the glue's outlet.
//!
//! The candidate window's events are produced on the drain thread (`crate::events`),
//! but the engine's session layer runs on the Fcitx5 main loop and only there. This
//! module is the drain's side of that crossing: it encodes each event onto the wire
//! below and hands it to the glue's outbox (`rspinyin_ui_event_outlet_*` in
//! `src/ffi/cpp/ui_addon_glue.cpp`), whose queue the main-loop callback entries drain
//! into the engine's ingest. The wire structs are a transcription of the engine's
//! reader (`ime-fcitx5/src/ffi/abi/engine/transport.rs`) — field order is the ABI,
//! fields are append-only, and both sides assert the sizes their transcription
//! produces, so a drift fails to build.
//!
//! The caret anchor rides the same channel under its own kind: it is engine state
//! rather than a window event, which is why it is not a `UiEvent` and why the
//! engine's event reader refuses the kind.

use ime_types::ids::ScreenId;
use ime_types::{DismissReason, PageDir, Placement, RectI, SelectTrigger, UiEvent};

/// Recorded when the engine refused, or cannot take, an event the drain handed it.
///
/// The drain thread keeps running either way -- the window it serves does -- so the
/// count is what tells an operator the return channel is one-way at the moment. The
/// spelling is stable and matched by tests. Public for the same reason the wire
/// family is: the module is the crate's seam for the return channel, and its codes
/// are part of what a reader of that seam matches on.
pub const ENGINE_GONE_CODE: &str = "ui/event/engine-gone";

/// Recorded once when the event outlet could not be armed.
///
/// Without an outlet there is nowhere for the drain thread to hand events, so the
/// drain starts no thread rather than growing one that only manufactures this line.
pub const EVENT_OUTLET_UNAVAILABLE_CODE: &str = "ui/event/outlet-unavailable";

/// The wire `kind` of a caret anchor uplink. The event kinds 0-3 (select, hover,
/// page, dismiss) live with [`event_wire`]; the anchor is the one kind this module
/// itself builds.
const EVENT_KIND_ANCHOR: u32 = 4;

/// An event the candidate window sends back to the engine, on the wire.
///
/// The transcription of the engine's reader; field order is the ABI and only tail
/// appends are allowed. The `reason` field is the kind's auxiliary slot -- the
/// select trigger for a select, the hover-presence flag for a hover, the page
/// direction for a page, the dismiss reason for a dismissal -- and the anchor fields
/// belong to the anchor kind alone: the reader is kind-gated, so they are zero for
/// every other kind.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspinyinEventWire {
    /// Which event this is; the encoders below hold the kind numbers.
    pub kind: u32,
    /// The frame revision the event was made against.
    pub revision: u32,
    /// The candidate position, for the kinds that carry one.
    pub index: u16,
    /// The kind's auxiliary slot.
    pub reason: u32,
    /// The caret anchor's rectangle, its screen, scale and placement: meaningful for
    /// the anchor kind only.
    pub anchor_x: i32,
    pub anchor_y: i32,
    pub anchor_w: u32,
    pub anchor_h: u32,
    pub anchor_screen: i32,
    pub anchor_scale: f32,
    pub anchor_placement: u32,
}

// The anchor append grew the event wire from 16 to 44 bytes. The engine's
// transcription carries the same number; a drift between the two fails this build.
const _: () = assert!(size_of::<RspinyinEventWire>() == 44);

// The event wire's `kind` values, transcribed from the engine's reader. Numbers are
// the ABI; the round-trip tests in `crate::events` fail when either side drifts.
const EVENT_KIND_SELECT: u32 = 0;
const EVENT_KIND_HOVER: u32 = 1;
const EVENT_KIND_PAGE: u32 = 2;
const EVENT_KIND_DISMISS: u32 = 3;

// The auxiliary-slot readings, per kind: the select trigger for a select, the hover
// presence flag for a hover, the page direction for a page, the dismiss reason for a
// dismissal.
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

/// Builds the event wire one UI event travels on.
///
/// `None` for a [`UiEvent::Rendered`] receipt: it is the latency probe's sample and
/// names no engine state, so there is no wire kind for it to become. Public beside
/// the rest of the return-channel seam, so the drain's round-trip tests encode with
/// the same function production uses.
pub fn event_wire(event: &UiEvent) -> Option<RspinyinEventWire> {
    // SAFETY: the wire crosses FFI as a raw copy, so it is zeroed first — defined
    // padding keeps that copy free of uninitialised bytes. Every field of the wire
    // is an integer, so zero is a valid state for all of them, and each field is
    // written below before the wire leaves this function.
    let mut wire: RspinyinEventWire = unsafe { std::mem::zeroed() };
    match *event {
        UiEvent::Select {
            revision,
            index,
            trigger,
        } => {
            wire.kind = EVENT_KIND_SELECT;
            wire.revision = revision;
            wire.index = index;
            wire.reason = match trigger {
                SelectTrigger::Mouse => TRIGGER_MOUSE,
                SelectTrigger::NumberKey => TRIGGER_NUMBER_KEY,
                SelectTrigger::Space => TRIGGER_SPACE,
                SelectTrigger::Enter => TRIGGER_ENTER,
                SelectTrigger::Tab => TRIGGER_TAB,
            };
        }
        UiEvent::Hover { revision, index } => {
            wire.kind = EVENT_KIND_HOVER;
            wire.revision = revision;
            // The presence flag is the auxiliary slot; the position rides in
            // `index` and the reader ignores it when the flag says absent.
            wire.index = index.unwrap_or(0);
            wire.reason = if index.is_some() {
                HOVER_PRESENT
            } else {
                HOVER_ABSENT
            };
        }
        UiEvent::Page { revision, dir } => {
            wire.kind = EVENT_KIND_PAGE;
            wire.revision = revision;
            wire.reason = match dir {
                PageDir::Next => PAGE_NEXT,
                PageDir::Prev => PAGE_PREV,
            };
        }
        UiEvent::Dismiss { revision, reason } => {
            wire.kind = EVENT_KIND_DISMISS;
            wire.revision = revision;
            wire.reason = match reason {
                DismissReason::OutsideClick => DISMISS_OUTSIDE_CLICK,
                DismissReason::Escape => DISMISS_ESCAPE,
                DismissReason::ScrollUpEmpty => DISMISS_SCROLL_UP_EMPTY,
            };
        }
        UiEvent::Rendered { .. } => return None,
    }
    Some(wire)
}

/// Builds the wire a caret anchor uplink travels on.
///
/// The anchor is engine state rather than a window event, so it is not a `UiEvent`:
/// it rides the same channel under its own kind, which is what keeps the engine's
/// reader from mistaking a caret report for a click.
fn anchor_wire(
    rect: &RectI,
    screen: ScreenId,
    scale: f32,
    placement: Placement,
) -> Option<RspinyinEventWire> {
    // The wire carries the screen as an `i32`; an id it cannot carry refuses the send
    // rather than wrapping into a negative one the reader would refuse anyway.
    let screen = i32::try_from(screen.value()).ok()?;
    // SAFETY: `RspinyinEventWire` is a plain-data `#[repr(C)]` wire mirror, so an
    // all-zero bit pattern is a valid value; every field is written below before the
    // wire leaves this function.
    let mut wire: RspinyinEventWire = unsafe { std::mem::zeroed() };
    wire.kind = EVENT_KIND_ANCHOR;
    wire.anchor_x = rect.x;
    wire.anchor_y = rect.y;
    wire.anchor_w = rect.w;
    wire.anchor_h = rect.h;
    wire.anchor_screen = screen;
    wire.anchor_scale = scale;
    wire.anchor_placement = match placement {
        Placement::Below => 0,
        Placement::Above => 1,
        Placement::Auto => 2,
    };
    Some(wire)
}

/// Hands one event to the engine addon through the glue's outlet.
///
/// `false` when the event carries no wire (a render receipt) or when the outlet
/// refused it -- the outlet is full, or the engine is gone. The drain reports the
/// refusals under [`ENGINE_GONE_CODE`]; this side does not log per event, because a
/// drain that outlives its engine would otherwise write one line per queued event.
///
/// # Panics
///
/// Never.
pub fn post_event(ic: u64, event: &UiEvent) -> bool {
    let Some(wire) = event_wire(event) else {
        return false;
    };
    post_wire(ic, &wire)
}

/// Hands one caret anchor uplink to the engine addon through the glue's outlet.
///
/// The producer is the caret path: the rectangle the host reported, resolved into a
/// screen anchor and sent so the next frame the engine builds carries it. An anchor
/// the wire cannot carry (a screen id past what an `i32` holds) refuses the send;
/// the next report replaces it, and a dropped anchor costs one stale frame placed at
/// the previous caret, never user text.
///
/// # Panics
///
/// Never.
pub fn post_anchor(
    ic: u64,
    rect: &RectI,
    screen: ScreenId,
    scale: f32,
    placement: Placement,
) -> bool {
    let Some(wire) = anchor_wire(rect, screen, scale, placement) else {
        return false;
    };
    post_wire(ic, &wire)
}

/// The glue's outbox: the one marshal point between the drain thread and the engine's
/// main-loop thread.
fn post_wire(ic: u64, wire: &RspinyinEventWire) -> bool {
    #[cfg(fcitx5_host)]
    {
        // SAFETY: the glue copies the wire into its own outbox before returning; the
        // borrow ends with the call, and the callee blocks on nothing but its own
        // bounded queue lock.
        unsafe { rspinyin_ui_event_outlet_post(ic, wire) == 1 }
    }
    #[cfg(not(fcitx5_host))]
    {
        // No glue is linked, so there is no engine to hand the event to.
        let _ = (ic, wire);
        false
    }
}

/// Arms the event outlet: asks the glue to resolve the engine's ingest and open its
/// outbox.
///
/// `false` when the engine is not reachable, which the caller records once and the
/// drain honours by not starting -- a thread whose every post would be refused is a
/// thread that only manufactures diagnostics.
///
/// # Panics
///
/// Never.
pub fn arm_event_outlet() -> bool {
    #[cfg(fcitx5_host)]
    {
        // SAFETY: the glue reads no memory this side owns, blocks on nothing, and
        // answers whether the engine's ingest was resolved.
        unsafe { rspinyin_ui_event_outlet_arm() == 1 }
    }
    #[cfg(not(fcitx5_host))]
    {
        false
    }
}

/// Drops the engine's ingest and empties the outbox, beside the transport's own
/// unregister.
///
/// An ingest pointer into an engine that unloaded first must never be called, and
/// events queued behind a gone engine are dropped rather than delivered into a
/// teardown.
///
/// # Panics
///
/// Never.
pub(super) fn disarm() {
    #[cfg(fcitx5_host)]
    {
        // SAFETY: the glue clears its own state and blocks on nothing.
        unsafe {
            rspinyin_ui_event_outlet_disarm();
        }
    }
}

/// Hands the outbox's queued events to the engine, from a main-loop thread.
///
/// The glue calls the engine's ingest on this thread, which is the thread the
/// engine's session layer runs on; the drain thread never does. Callers are the
/// loop-thread entry points of the callback table, which is where queued events
/// drain today; the wake that also drains them with no dispatch in flight is
/// recorded in ADR-0011 as the outlet's remaining piece.
///
/// # Panics
///
/// Never.
pub fn flush_event_outlet() {
    #[cfg(fcitx5_host)]
    {
        // SAFETY: the callee drains its own queue on the calling thread and blocks
        // on nothing beyond it.
        unsafe {
            rspinyin_ui_event_outlet_flush();
        }
    }
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// Resolves the engine's `rspinyin_event_ingest` and opens the event outbox.
    ///
    /// Provided by `src/ffi/cpp/ui_addon_glue.cpp`. Answers 1 when the ingest was
    /// resolved and 0 when no mechanism found a live engine.
    ///
    /// # Safety
    ///
    /// Called from the host thread; the callee blocks on nothing and owns nothing
    /// the caller has to release.
    fn rspinyin_ui_event_outlet_arm() -> u32;

    /// Drops the engine's ingest and empties the outbox. Provided by the same glue.
    ///
    /// # Safety
    ///
    /// As `rspinyin_ui_event_outlet_arm`.
    fn rspinyin_ui_event_outlet_disarm();

    /// Queues one event wire for the engine's main loop. Answers 1 when queued.
    ///
    /// # Safety
    ///
    /// `wire` must point to an initialised `RspinyinEventWire`; the callee copies it
    /// before returning.
    fn rspinyin_ui_event_outlet_post(ic: u64, wire: *const RspinyinEventWire) -> u32;

    /// Hands the outbox's queued events to the engine, on the calling thread.
    ///
    /// # Safety
    ///
    /// Must be called from the Fcitx5 main-loop thread: the callee invokes the
    /// engine's ingest, whose session layer runs on that thread alone.
    fn rspinyin_ui_event_outlet_flush() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anchor_wire_encodes_every_anchor_field() {
        // The caret anchor rides the event channel under its own kind; the encoder
        // is what the caret path hands its answer to, and the assertion is field for
        // field against the engine reader's numbers.
        let rect = RectI {
            x: -40,
            y: 300,
            w: 12,
            h: 30,
        };
        let wire = anchor_wire(&rect, ScreenId::new(1), 1.25, Placement::Above)
            .expect("the anchor encodes");
        assert_eq!(wire.kind, EVENT_KIND_ANCHOR);
        assert_eq!(wire.anchor_x, -40);
        assert_eq!(wire.anchor_y, 300);
        assert_eq!(wire.anchor_w, 12);
        assert_eq!(wire.anchor_h, 30);
        assert_eq!(wire.anchor_screen, 1);
        assert_eq!(wire.anchor_scale, 1.25);
        assert_eq!(wire.anchor_placement, 1);
        // Every other slot stays zero: the anchor kind's fields are the anchor's
        // alone.
        assert_eq!((wire.revision, wire.index, wire.reason), (0, 0, 0));
    }

    #[test]
    fn test_anchor_wire_refuses_a_screen_the_wire_cannot_carry() {
        // The wire carries the screen as an `i32`; an id past that wraps into a
        // negative the engine's reader refuses anyway, so the send is refused on
        // this side first.
        let rect = RectI {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        let screen = ScreenId::new((i32::MAX as u32) + 1);
        assert!(
            anchor_wire(&rect, screen, 1.0, Placement::Below).is_none(),
            "a screen id the wire cannot carry refuses the send"
        );
    }
}
