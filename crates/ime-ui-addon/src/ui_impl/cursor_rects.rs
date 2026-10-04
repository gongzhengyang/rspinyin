//! The caret rectangles the host has reported, one per input context.
//!
//! Responsibility: remember the rectangle each input context's caret was last reported at,
//! answer for the context that asks, and resolve each report into the anchor the engine
//! positions the window with. The store is a bounded ring: a long-running session reports
//! for context after context, and a fixed number of slots is what keeps that from growing
//! without limit.
//!
//! Boundaries: a rectangle is stored exactly as the host reported it, in client
//! coordinates -- normalising it is the caret ladder's job (`crate::cursor`), which runs
//! here, right after the store takes the report. A rectangle that belongs to another input
//! context is never returned: a caret remembered for one client must never position
//! another client's window.

use std::sync::{Mutex, MutexGuard};

use ime_types::ScreenId;

use crate::cursor::{CursorResolver, IcId};
use crate::ffi::FcitxCursorRect;
use crate::ffi::transport::post_anchor;
use crate::screen::{FALLBACK_SCALE, ScreenInfo, ScreenLayout};

/// One caret rectangle with the input context it belongs to.
#[derive(Clone, Copy, Debug)]
struct CursorEntry {
    /// The input context whose caret the rectangle describes.
    ic: u64,
    /// The rectangle as the host reported it, in client coordinates.
    rect: FcitxCursorRect,
}

/// Handles a caret-rectangle change for `ic`.
///
/// Stores the rectangle exactly as the host reported it. Normalising it — the device
/// pixel ratio, the plausibility of the coordinates, the output it lands on — is the
/// caret ladder's job, and it runs right here: the resolved anchor is what the engine's
/// placement chain is keyed on, and uplinking it the moment the host reports keeps the
/// window's next frame on the caret the user is looking at. The uplink rides the event
/// outlet (ADR-0011), which queues when the engine is momentarily busy, so a failed post
/// costs nothing: the ring below still holds the rectangle, and the next report resends.
pub fn on_cursor_rect(ic: u64, rect: FcitxCursorRect) {
    lock_cursor().insert(CursorEntry { ic, rect });
    let anchor = {
        let mut resolver = lock_resolver();
        resolver
            .as_mut()
            .and_then(|resolver| resolver.resolve_cached(IcId::new(ic), rect).ok())
    };
    if let Some(anchor) = anchor {
        post_anchor(
            ic,
            &anchor.cursor,
            anchor.screen,
            anchor.scale,
            anchor.placement,
        );
    }
}

/// The caret ladder the uplink resolves through, built on first use.
///
/// The platform cannot hand this thread a screen enumeration yet: the real backends own
/// their connections on the UI thread (`ASM-B-06`), so the resolver runs on the fixed
/// single-output fallback layout, which routes every caret through the ladder's fallback
/// rung — the same screen-centre rule the window placement already ends with. The
/// produced anchor is still a real anchor on a real output, and when a backend starts
/// enumerating, its [`ScreenLayout`] replaces this one through the same static, without
/// a new call site.
static RESOLVER: Mutex<Option<CursorResolver>> = Mutex::new(None);

/// The fallback output the ladder resolves against until a backend enumerates.
///
/// One nominal primary at the origin, because a desktop whose facts are unknown is
/// modelled, not invented: the size only feeds the fallback caret's screen-centre
/// fractions, and the scale is the ladder's own [`FALLBACK_SCALE`].
fn fallback_layout() -> ScreenLayout {
    ScreenLayout::new(
        vec![ScreenInfo::new(
            ScreenId::new(0),
            (0, 0),
            (1920, 1080),
            FALLBACK_SCALE,
            "fallback",
        )],
        ScreenId::new(0),
    )
}

/// Borrows the resolver, building it on first use.
///
/// Poisoning cannot corrupt what the lock guards — the resolver is self-healing state —
/// so the poisoned slot is rebuilt rather than left dead.
fn lock_resolver() -> MutexGuard<'static, Option<CursorResolver>> {
    let mut guard = match RESOLVER.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        let mut resolver = CursorResolver::without_platform();
        resolver.adopt_layout(fallback_layout());
        *guard = Some(resolver);
    }
    guard
}

/// The caret rectangle last reported for `ic`, or `None` when the host has reported
/// none for it.
///
/// A rectangle that belongs to another input context is not returned: a caret
/// remembered for one client must never position another client's window.
pub fn latest_cursor_rect(ic: u64) -> Option<FcitxCursorRect> {
    lock_cursor().get(ic)
}

/// How many input contexts' caret rectangles are remembered at once.
///
/// Only the focused context's rectangle is ever read, but a context that loses focus and
/// regains it must not have to wait for the host to report again before its window can be
/// placed — the host reports on change, not on demand. A fixed bound is what keeps a
/// long-running session from growing this store without limit.
const CURSOR_SLOTS: usize = 8;

/// The caret rectangles the host has reported, one per input context.
struct CursorRects {
    /// The remembered entries; an unused slot holds `None`.
    entries: [Option<CursorEntry>; CURSOR_SLOTS],
    /// The slot the next new context writes, which is also the oldest entry's slot.
    next: usize,
}

impl CursorRects {
    /// An empty store.
    const fn new() -> Self {
        Self {
            entries: [None; CURSOR_SLOTS],
            next: 0,
        }
    }

    /// Remembers `entry`, evicting the least recently added context when all slots are
    /// taken.
    ///
    /// An entry for a context already remembered is replaced in place, so a context that
    /// reports repeatedly never evicts anyone else.
    fn insert(&mut self, entry: CursorEntry) {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|existing| existing.ic == entry.ic)
        {
            *slot = entry;
            return;
        }
        self.entries[self.next] = Some(entry);
        self.next = (self.next + 1) % CURSOR_SLOTS;
    }

    /// The rectangle remembered for `ic`, if the host has reported one.
    fn get(&self, ic: u64) -> Option<FcitxCursorRect> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.ic == ic)
            .map(|entry| entry.rect)
    }
}

/// The caret rectangles the host has reported.
static CURSOR_RECTS: Mutex<CursorRects> = Mutex::new(CursorRects::new());

/// Borrows the caret store, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked; the entries it guards are plain values, and
/// refusing to look at them would leave the window without a position.
fn lock_cursor() -> MutexGuard<'static, CursorRects> {
    match CURSOR_RECTS.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}
