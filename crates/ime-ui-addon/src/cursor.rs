//! Caret resolution: client coordinates to screen physical pixels.
//!
//! Fcitx5 hands the engine a caret rectangle relative to the client window, while the candidate
//! window is placed in the physical pixel space of the virtual desktop. This module normalises
//! that rectangle, decides which output it belongs to, and returns the [`ime_types::Anchor`] the UI
//! thread positions the window with.
//!
//! # The source ladder
//!
//! | Tier | Source | When it answers |
//! |---|---|---|
//! | 1 | the frontend's rectangle, if it is already screen-absolute | when the heuristic below holds |
//! | 2 | the client rectangle plus the focused window's origin | X11 (`_NET_ACTIVE_WINDOW` + `xcb_translate_coordinates`); Wayland through its tier's coordinate space |
//! | 3 | the fallback: the screen's horizontal centre, 60% down | always |
//!
//! Tier 1 is a **heuristic** -- a rectangle whose centre falls inside an output is taken to be
//! absolute -- and it lives in one function ([`resolver::looks_absolute`]) so that
//! `docs/dev/spikes/cursor-probe.md`, which records what each frontend really reports, can revise
//! it in one place. Tiers 1 and 3 cost nothing; tier 2 costs one platform round trip.
//!
//! The rules the ladder applies -- physical pixels, centre-point hit testing, degenerate
//! rectangles, scale substitution -- and the arithmetic implementing them are in [`crate::screen`].
//! What lives here is the ladder, the cache that keeps the window still, and the diagnostics.
//!
//! # Module map
//!
//! This file carries the identity type the ladder is keyed by ([`IcId`]) and the module's public
//! surface. `resolver` holds the ladder, its cache and its diagnostics; `sources` the platform seam
//! tier 2 reads the focused window's origin through; and `tests` the suite. Nothing outside this
//! module names the submodules directly.

mod resolver;
mod sources;

#[cfg(test)]
mod tests;

pub use self::resolver::{
    CURSOR_UNRESOLVED_CODE, CursorResolver, CursorTier, Resolution, SCALE_INVALID_CODE,
};
pub use self::sources::{FixedGeometry, UnknownWindowGeometry, WindowGeometrySource, WindowOrigin};

/// Identifies one Fcitx5 input context.
///
/// The host identifies a context by its `ICUUID`; the ABI carries a 64-bit digest of that uuid
/// instead, because a C ABI has no place for a 16-byte value. The resolver keys its cache by this
/// id, so a caret remembered for one client never positions another's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IcId(u64);

impl IcId {
    /// Wraps a raw context id as the ABI carries it.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw id.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl From<IcId> for u64 {
    fn from(id: IcId) -> Self {
        id.0
    }
}
