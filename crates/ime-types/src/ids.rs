//! Cross-boundary identifier newtypes.
//!
//! Every identifier that crosses the engine / UI boundary is its own newtype so
//! that same-shaped primitives cannot be swapped at a call site: `SessionId`
//! wraps a `u64`, `WordId` a `u32`, and neither can be passed where the other is
//! expected. The inner values stay private; construct with `new`, read back with
//! `value`.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

/// Identifies one composing session.
///
/// A fresh id is allocated on every `Idle -> Composing` transition. Sessions are
/// referenced only from diagnostics, which is why the id is a plain counter: it
/// lets a log reader tell two sessions apart without recording any input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SessionId(u64);

impl SessionId {
    /// Wraps a raw session counter value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter value.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl From<SessionId> for u64 {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

/// Identifies one output (monitor) in the virtual desktop.
///
/// The id comes from the platform's output enumeration. `Anchor` carries it so
/// that the candidate window is placed on the screen the cursor is on and uses
/// that screen's scale factor rather than a mixed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScreenId(u32);

impl ScreenId {
    /// Wraps a raw output index.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw output index.
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl From<ScreenId> for u32 {
    fn from(id: ScreenId) -> Self {
        id.0
    }
}

/// Identifies one entry in the compiled dictionary.
///
/// The id is the index into the dictionary's entry table. It is stable for the
/// lifetime of a compiled dictionary file and is what the string pool and the
/// per-key word list are addressed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WordId(u32);

impl WordId {
    /// Wraps a raw entry-table index.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw entry-table index.
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl From<WordId> for u32 {
    fn from(id: WordId) -> Self {
        id.0
    }
}

/// Monotonic revision counter carried by every frame and UI event.
///
/// `0` is reserved for "no revision yet": `next` never produces it. The UI drops
/// a frame or an event whose revision is older than the one it already holds,
/// which is what makes the host / UI thread split safe against reordering and
/// replays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Revision(u32);

impl Revision {
    /// Wraps a raw revision value, for example when reading a frame's revision
    /// back out of `UiFrame`.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw value that travels in `UiFrame` and `UiEvent`.
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Advances to the next revision and returns it.
    ///
    /// The counter wraps from `u32::MAX` back to `1`, skipping `0` so that `0`
    /// keeps its "unset" meaning. The wrap is unreachable in practice (at twenty
    /// keys per second it is more than six years of continuous typing); it exists
    /// so that a long-lived process can never hand the UI a revision it treats as
    /// stale forever.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_types::ids::Revision;
    ///
    /// let mut revision = Revision::new(0);
    /// revision.next();
    /// assert_eq!(revision.value(), 1);
    /// ```
    // `next` is the name the contract fixes for this operation, and the counter
    // is not a sequence: implementing `Iterator` for it would be meaningless.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Revision {
        self.0 = self.0.wrapping_add(1);
        if self.0 == 0 {
            self.0 = 1;
        }
        *self
    }
}

impl From<Revision> for u32 {
    fn from(revision: Revision) -> Self {
        revision.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revision_next_from_zero_returns_one() {
        let mut revision = Revision::new(0);
        let next = revision.next();
        assert_eq!(next, Revision::new(1));
        assert_eq!(revision.value(), 1);
    }

    #[test]
    fn test_revision_next_from_max_wraps_to_one_skipping_zero() {
        let mut revision = Revision::new(u32::MAX);
        let next = revision.next();
        assert_eq!(next, Revision::new(1));
        assert_ne!(revision.value(), 0);
    }

    #[test]
    fn test_revision_next_increments_without_wrap() {
        let mut revision = Revision::new(41);
        assert_eq!(revision.next(), Revision::new(42));
        assert_eq!(revision.next(), Revision::new(43));
    }

    #[test]
    fn test_revision_into_u32_round_trips() {
        let revision = Revision::new(7);
        assert_eq!(revision.value(), 7);
        assert_eq!(u32::from(revision), 7);
    }

    #[test]
    fn test_session_id_value_round_trips() {
        let id = SessionId::new(u64::MAX);
        assert_eq!(id.value(), u64::MAX);
        assert_eq!(u64::from(id), u64::MAX);
    }

    #[test]
    fn test_screen_id_value_round_trips() {
        let id = ScreenId::new(2);
        assert_eq!(id.value(), 2);
        assert_eq!(u32::from(id), 2);
    }

    #[test]
    fn test_word_id_value_round_trips() {
        let id = WordId::new(400_000);
        assert_eq!(id.value(), 400_000);
        assert_eq!(u32::from(id), 400_000);
    }
}
