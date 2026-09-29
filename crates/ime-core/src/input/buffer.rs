//! The mutable input buffer of one composing session.
//!
//! Responsibility: hold the raw input string, the caret and the syllable
//! boundaries the last segmentation reported, and define what Backspace removes
//! and how far the caret may travel. The parent module documents those two rules;
//! this file implements them and keeps the invariants they rest on.
//!
//! Boundaries: the buffer is a plain value. It never normalizes, never touches a
//! dictionary and never reads a clock, so a test can replay a whole session by
//! calling the same methods the state machine calls.

use ime_types::ImeError;
use smallvec::SmallVec;

use crate::segment::{HINT_INLINE_BOUNDARIES, MAX_RAW_LEN};

/// What one Backspace removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackspaceOutcome {
    /// A whole syllable was removed and the input still holds text.
    RemovedSyllable,
    /// One character was removed and the input still holds text.
    RemovedChar,
    /// The input holds nothing: either there was nothing to remove, or the removal
    /// emptied the input. The caller leaves the composing state and hands the key
    /// back to the host.
    BufferEmpty,
}

/// One composing session's input string, caret and syllable grid.
///
/// # Invariants
///
/// - `raw` holds input-alphabet characters only: ASCII letters and `'`. Every byte
///   offset in `0..=raw.len()` is therefore a character boundary, and no slice of
///   `raw` can split a character.
/// - `caret <= raw.len()`: the caret always addresses a position the user can see.
/// - `last_boundaries` is strictly increasing, starts at `0` and ends at
///   `raw.len()`. It is the syllable grid Backspace deletes by and the set of
///   positions the caret may take. Every method below preserves that shape:
///   [`InputBuffer::set_boundaries`] adopts a grid only when it already has it, and
///   the mutators repair the grid after they change the input.
///
/// # Examples
///
/// ```
/// use ime_core::input::{BackspaceOutcome, InputBuffer};
///
/// let mut buf = InputBuffer::new();
/// for ch in "nihaoa".chars() {
///     assert!(buf.push_char(ch).is_ok());
/// }
/// // The segmentation reported the cut ni|hao|a.
/// buf.set_boundaries(&[0, 2, 5, 6]);
///
/// assert_eq!(buf.backspace(), BackspaceOutcome::RemovedSyllable);
/// assert_eq!(buf.raw(), "nihao");
/// assert_eq!(buf.backspace(), BackspaceOutcome::RemovedSyllable);
/// assert_eq!(buf.raw(), "ni");
/// assert_eq!(buf.backspace(), BackspaceOutcome::BufferEmpty);
/// assert_eq!(buf.raw(), "");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputBuffer {
    /// Raw input as typed, never normalized.
    raw: String,
    /// Caret position, as a byte offset into `raw`.
    caret: u32,
    /// Syllable boundaries of the last segmentation, as byte offsets.
    last_boundaries: SmallVec<[u16; HINT_INLINE_BOUNDARIES]>,
    /// Wall-clock stamp of the session start, in milliseconds since the Unix epoch.
    started_at_unix_ms: u64,
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBuffer {
    /// Creates an empty buffer: no input, caret at the origin, no session stamp.
    ///
    /// The syllable grid starts as the single empty unit `[0]`, which is the grid
    /// the segmentation reports for empty input, so the buffer is usable before the
    /// caller has segmented anything.
    pub fn new() -> Self {
        Self {
            raw: String::new(),
            caret: 0,
            last_boundaries: SmallVec::from_slice(&[0u16]),
            started_at_unix_ms: 0,
        }
    }

    /// Appends one typed character and leaves the caret after it.
    ///
    /// A character is always appended at the end of the input: the caret only ever
    /// sits on syllable boundaries in this phase and character-level editing is not
    /// implemented yet, so a caret that had been moved back into the input is
    /// carried along to the end instead of turning into an insertion point.
    ///
    /// # Errors
    ///
    /// - [`ImeError::DecodeInvalidChar`] when `ch` is outside the input alphabet
    ///   (ASCII letters and `'`). The key translator filters those out, so this is
    ///   an internal-defect guard rather than a user-facing failure; the reported
    ///   offset is the end of the input, where the character would have gone.
    /// - [`ImeError::DecodeTooLong`] when appending would push the input past
    ///   [`MAX_RAW_LEN`]. The buffer is left untouched, so the caller can drop the
    ///   keystroke and report that the input is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::input::InputBuffer;
    ///
    /// let mut buf = InputBuffer::new();
    /// for _ in 0..64 {
    ///     assert!(buf.push_char('a').is_ok());
    /// }
    /// // The 65th byte does not fit, and the input stays as it was.
    /// assert!(buf.push_char('a').is_err());
    /// assert_eq!(buf.raw().len(), 64);
    /// ```
    pub fn push_char(&mut self, ch: char) -> Result<(), ImeError> {
        if !is_input_char(ch) {
            return Err(ImeError::DecodeInvalidChar {
                ch,
                at: self.raw.len(),
            });
        }
        let len = self.raw.len().saturating_add(ch.len_utf8());
        if len > MAX_RAW_LEN {
            return Err(ImeError::DecodeTooLong {
                len,
                max: MAX_RAW_LEN,
            });
        }
        self.raw.push(ch);
        self.repair_boundaries();
        self.caret = u32::try_from(self.raw.len()).unwrap_or(u32::MAX);
        Ok(())
    }

    /// Removes one unit of input in front of the caret.
    ///
    /// The unit is the trailing syllable while the caret sits at the end of the
    /// input, and one character otherwise: inside the input the grid no longer says
    /// what the caret is next to, and removing a whole syllable there would delete
    /// text the user did not point at. A caret at the start of the input has no
    /// character in front of it, so the character it points at goes instead: the key
    /// always removes something while the input is not empty, because
    /// [`BackspaceOutcome::BufferEmpty`] makes the caller end the composing session
    /// and the user asked for a deletion, not for a cancel.
    ///
    /// # Panics
    ///
    /// Never panics: the input alphabet is ASCII, so the caret is always a character
    /// boundary and the removal range is always a whole character.
    pub fn backspace(&mut self) -> BackspaceOutcome {
        if self.raw.is_empty() {
            return BackspaceOutcome::BufferEmpty;
        }
        let at_end = self.caret_offset() == self.raw.len();
        let (start, end) = self.removal_range();
        self.raw.replace_range(start..end, "");
        self.caret = u32::try_from(start).unwrap_or(u32::MAX);
        self.repair_boundaries();
        if self.raw.is_empty() {
            return BackspaceOutcome::BufferEmpty;
        }
        if at_end {
            BackspaceOutcome::RemovedSyllable
        } else {
            BackspaceOutcome::RemovedChar
        }
    }

    /// Moves the caret by `delta` syllable boundaries.
    ///
    /// A move lands on a boundary of the current grid only: from a position inside a
    /// syllable, moving right goes to the boundary that follows it and moving left to
    /// the boundary before it. The move stops at the ends of the input and never
    /// wraps, so holding a key down cannot bounce the caret between the two ends.
    ///
    /// Returns `true` when the caret moved, and `false` when it could not: an empty
    /// input, a `delta` of `0`, or a `delta` that would leave the input.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn move_caret(&mut self, delta: i8) -> bool {
        if delta == 0 {
            return false;
        }
        let caret = self.caret_offset();
        let target = self.stepped_boundary(caret, delta);
        if target == caret {
            return false;
        }
        self.caret = u32::try_from(target).unwrap_or(u32::MAX);
        true
    }

    /// Clears the input, the caret and the syllable grid, and drops the session
    /// stamp: the buffer is left exactly as [`InputBuffer::new`] leaves it.
    ///
    /// The stamp goes with the input, because it belongs to the session that just
    /// ended; a diagnostic that read a stale stamp would report the length of a
    /// session that is over. The caller stamps the next session with
    /// [`InputBuffer::mark_session_start`]. The input buffer's capacity is kept, so
    /// the next session does not reallocate on its first keystroke.
    pub fn clear(&mut self) {
        self.raw.clear();
        self.caret = 0;
        self.last_boundaries.clear();
        self.last_boundaries.push(0);
        self.started_at_unix_ms = 0;
    }

    /// Returns the raw input, exactly as typed and not yet normalized.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the caret position, as a byte offset into [`InputBuffer::raw`].
    ///
    /// The offset is always a character boundary, so the caller can slice the input
    /// at it and the preedit can place its cursor there.
    pub fn caret(&self) -> u32 {
        self.caret
    }

    /// Records the syllable boundaries the latest segmentation reported.
    ///
    /// The grid is what Backspace deletes by and what the caret moves between, so the
    /// caller writes back the hint it received after every keystroke. Pass the hint
    /// only when the input was segmentable: the pass-through range a failed
    /// segmentation reports is one unit, and Backspace would then remove the whole
    /// input in a single keystroke.
    ///
    /// A grid that is not strictly increasing, does not start at `0`, does not end at
    /// the end of the input, or falls inside a character cannot come from the
    /// segmentation and is an internal defect: it trips a debug assertion, and a
    /// release build ignores it and keeps the previous grid.
    ///
    /// # Panics
    ///
    /// Panics in debug builds only, when `boundaries` is not a usable grid for the
    /// current input; release builds ignore such a grid.
    pub fn set_boundaries(&mut self, boundaries: &[u16]) {
        let is_valid = is_valid_grid(boundaries, &self.raw);
        debug_assert!(
            is_valid,
            "input buffer received a grid that does not describe the input"
        );
        if !is_valid {
            return;
        }
        self.last_boundaries.clear();
        self.last_boundaries.extend_from_slice(boundaries);
    }

    /// Stamps the wall-clock start of the composing session, in milliseconds since
    /// the Unix epoch.
    ///
    /// The buffer never reads a clock itself -- the decode path stays a pure function
    /// -- so the caller that observes the clock stamps it here when the session
    /// starts, and session-duration diagnostics subtract the stamp from the current
    /// time.
    pub fn mark_session_start(&mut self, at_unix_ms: u64) {
        self.started_at_unix_ms = at_unix_ms;
    }

    /// Returns the stamp written by [`InputBuffer::mark_session_start`], or `0` when
    /// no session has been stamped.
    pub fn started_at_unix_ms(&self) -> u64 {
        self.started_at_unix_ms
    }

    /// Returns the caret as a byte offset that is guaranteed to be inside the input.
    ///
    /// The caret is kept inside by construction; the clamp only makes the arithmetic
    /// total, so a broken invariant degrades instead of panicking.
    fn caret_offset(&self) -> usize {
        (self.caret as usize).min(self.raw.len())
    }

    /// Returns the byte range `(start, end)` the next Backspace removes.
    ///
    /// At the end of the input this is the trailing syllable, which starts at the
    /// second-to-last boundary of the grid. Inside the input it is the character in
    /// front of the caret, or the one the caret points at when it sits at the start.
    fn removal_range(&self) -> (usize, usize) {
        let caret = self.caret_offset();
        if caret == self.raw.len() {
            let index = self.last_boundaries.len().saturating_sub(2);
            let previous = self.last_boundaries.get(index).copied();
            return (previous.map_or(0, usize::from), caret);
        }
        if caret > 0 {
            let prefix = &self.raw[..caret];
            let front = prefix.char_indices().next_back();
            return (front.map_or(caret, |(at, _)| at), caret);
        }
        let first = self.raw.chars().next();
        (0, first.map_or(0, char::len_utf8))
    }

    /// Returns the byte offset of the boundary `delta` syllables away from `caret`,
    /// clamped to the ends of the input.
    fn stepped_boundary(&self, caret: usize, delta: i8) -> usize {
        let grid = &self.last_boundaries;
        let last = grid.len().saturating_sub(1);
        // A move starts from the boundary at or before the caret, so a caret inside a
        // syllable travels relative to the syllable it sits in.
        let at_or_below = |boundary: &u16| usize::from(*boundary) <= caret;
        let index = grid.iter().rposition(at_or_below).unwrap_or(0);
        let target = if delta > 0 {
            index.saturating_add(delta as usize).min(last)
        } else {
            index.saturating_sub(delta.unsigned_abs() as usize)
        };
        grid.get(target).copied().map_or(caret, usize::from)
    }

    /// Restores the boundary invariant after the input changed.
    ///
    /// Boundaries the edit pushed past the end of the input are dropped and the end
    /// of the input is appended as the last boundary, which keeps the grid strictly
    /// increasing and ending where the input does. A character removed from the
    /// middle of the input leaves a coarser grid than the real segmentation; the next
    /// segmentation replaces it.
    fn repair_boundaries(&mut self) {
        let len = u16::try_from(self.raw.len()).unwrap_or(u16::MAX);
        self.last_boundaries.retain(|boundary| *boundary < len);
        self.last_boundaries.push(len);
    }
}

/// Returns `true` for the characters the input alphabet accepts.
///
/// The alphabet is the ASCII letters and the `'` separator. Uppercase is kept as
/// typed, because the buffer stores what the user pressed; normalization folds the
/// case later, when the segmentation builds its graph.
fn is_input_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '\''
}

/// Returns `true` when `boundaries` is a usable syllable grid for `raw`.
///
/// A usable grid starts at `0`, ends at the end of `raw`, is strictly increasing and
/// only names character boundaries. Empty input has the single-entry grid `[0]`,
/// which is what the segmentation reports for it.
fn is_valid_grid(boundaries: &[u16], raw: &str) -> bool {
    let len = u16::try_from(raw.len()).unwrap_or(u16::MAX);
    let Some((&first, rest)) = boundaries.split_first() else {
        return false;
    };
    if first != 0 || rest.last().copied().unwrap_or(0) != len {
        return false;
    }
    let increasing = boundaries.windows(2).all(|pair| pair[0] < pair[1]);
    let inside = |at: &u16| raw.is_char_boundary(usize::from(*at));
    increasing && boundaries.iter().all(inside)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Builds a buffer holding `raw` and carrying `boundaries` as its syllable grid.
    fn buffer_with(raw: &str, boundaries: &[u16]) -> InputBuffer {
        let mut buf = InputBuffer::new();
        for ch in raw.chars() {
            assert!(buf.push_char(ch).is_ok(), "pushing {ch:?}");
        }
        buf.set_boundaries(boundaries);
        buf
    }

    /// Returns the character and offset a rejected keystroke reported, or `None` when
    /// the keystroke was accepted.
    fn invalid_char(outcome: Result<(), ImeError>) -> Option<(char, usize)> {
        match outcome {
            Err(ImeError::DecodeInvalidChar { ch, at }) => Some((ch, at)),
            _ => None,
        }
    }

    /// Returns the length and limit a rejected keystroke reported, or `None` when the
    /// keystroke was accepted.
    fn too_long(outcome: Result<(), ImeError>) -> Option<(usize, usize)> {
        match outcome {
            Err(ImeError::DecodeTooLong { len, max }) => Some((len, max)),
            _ => None,
        }
    }

    #[test]
    fn test_new_buffer_is_empty_and_unstamped() {
        let mut buf = InputBuffer::new();
        assert_eq!(buf.raw(), "");
        assert_eq!(buf.caret(), 0);
        assert_eq!(buf.started_at_unix_ms(), 0);
        // Nothing to remove and nowhere to move: both answer "nothing changed".
        assert_eq!(buf.backspace(), BackspaceOutcome::BufferEmpty);
        assert!(!buf.move_caret(-1));
        assert_eq!(buf, InputBuffer::default());
    }

    #[test]
    fn test_push_char_appends_and_moves_the_caret_to_the_end() {
        let mut buf = InputBuffer::new();
        for ch in "ni'".chars() {
            assert!(buf.push_char(ch).is_ok());
        }
        assert_eq!(buf.raw(), "ni'");
        assert_eq!(buf.caret(), 3);
    }

    #[test]
    fn test_push_char_rejects_a_character_outside_the_input_alphabet() {
        let mut buf = buffer_with("ni", &[0, 2]);
        for rejected in ['1', ' ', '中'] {
            assert_eq!(invalid_char(buf.push_char(rejected)), Some((rejected, 2)));
        }
        assert_eq!(buf.raw(), "ni");
        assert_eq!(buf.caret(), 2);
    }

    #[test]
    fn test_push_char_at_the_length_cap_reports_too_long_and_keeps_the_input() {
        let mut buf = InputBuffer::new();
        for _ in 0..MAX_RAW_LEN {
            assert!(buf.push_char('a').is_ok());
        }
        let expected = Some((MAX_RAW_LEN + 1, MAX_RAW_LEN));
        assert_eq!(too_long(buf.push_char('b')), expected);
        assert_eq!(buf.raw().len(), MAX_RAW_LEN);
        assert_eq!(buf.caret(), MAX_RAW_LEN as u32);
    }

    #[test]
    fn test_push_char_after_moving_the_caret_appends_and_carries_the_caret() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        assert!(buf.move_caret(-1));
        assert_eq!(buf.caret(), 5);
        // Character-level editing is not implemented, so the key extends the input and
        // the caret follows it to the end instead of inserting at offset 5.
        assert!(buf.push_char('n').is_ok());
        assert_eq!(buf.raw(), "nihaoan");
        assert_eq!(buf.caret(), 7);
    }

    #[test]
    fn test_backspace_removes_the_trailing_syllable_step_by_step() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        let expected = [
            (BackspaceOutcome::RemovedSyllable, "nihao"),
            (BackspaceOutcome::RemovedSyllable, "ni"),
            (BackspaceOutcome::BufferEmpty, ""),
        ];
        for (outcome, raw) in expected {
            assert_eq!(buf.backspace(), outcome);
            assert_eq!(buf.raw(), raw);
            assert_eq!(buf.caret(), raw.len() as u32);
        }
        // The input is empty now: the key has nothing left to remove.
        assert_eq!(buf.backspace(), BackspaceOutcome::BufferEmpty);
        assert_eq!(buf.raw(), "");
    }

    #[test]
    fn test_backspace_with_the_caret_inside_removes_one_character() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        assert!(buf.move_caret(-1));
        assert_eq!(buf.caret(), 5);
        assert_eq!(buf.backspace(), BackspaceOutcome::RemovedChar);
        // The 'o' in front of the caret went and the caret stayed where it was.
        assert_eq!(buf.raw(), "nihaa");
        assert_eq!(buf.caret(), 4);
    }

    #[test]
    fn test_backspace_at_the_start_removes_the_character_the_caret_points_at() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        for _ in 0..3 {
            assert!(buf.move_caret(-1));
        }
        assert_eq!(buf.caret(), 0);
        assert_eq!(buf.backspace(), BackspaceOutcome::RemovedChar);
        assert_eq!(buf.raw(), "ihaoa");
        assert_eq!(buf.caret(), 0);
    }

    #[test]
    fn test_backspace_without_a_segmentation_removes_one_character_at_a_time() {
        let mut buf = InputBuffer::new();
        for ch in "nihaoa".chars() {
            assert!(buf.push_char(ch).is_ok());
        }
        // No segmentation was written back, so every keystroke repaired the grid to
        // per-character boundaries: the last unit is one character, and the outcome
        // names the unit the grid describes.
        assert_eq!(buf.backspace(), BackspaceOutcome::RemovedSyllable);
        assert_eq!(buf.raw(), "nihao");
        assert_eq!(buf.caret(), 5);
    }

    #[test]
    fn test_move_caret_steps_between_syllable_boundaries() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        for expected in [5u32, 2, 0] {
            assert!(buf.move_caret(-1));
            assert_eq!(buf.caret(), expected);
        }
        // Already at the start: the caret stays put instead of wrapping.
        assert!(!buf.move_caret(-1));
        assert_eq!(buf.caret(), 0);
        for expected in [2u32, 5, 6] {
            assert!(buf.move_caret(1));
            assert_eq!(buf.caret(), expected);
        }
        assert!(!buf.move_caret(1));
        assert_eq!(buf.raw(), "nihaoa");
    }

    #[test]
    fn test_move_caret_ignores_a_zero_delta_and_an_empty_input() {
        let mut buf = buffer_with("ni", &[0, 2]);
        assert!(!buf.move_caret(0));
        assert_eq!(buf.caret(), 2);
        buf.clear();
        assert!(!buf.move_caret(-1));
        assert!(!buf.move_caret(1));
        assert_eq!(buf.caret(), 0);
    }

    #[test]
    fn test_move_caret_from_inside_a_syllable_snaps_to_a_boundary() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        assert!(buf.move_caret(-1));
        assert_eq!(buf.backspace(), BackspaceOutcome::RemovedChar);
        assert_eq!(buf.caret(), 4);
        // The caret is inside a syllable of the repaired grid [0, 2, 5]: one step
        // right reaches the boundary after it, one step left the boundary before the
        // syllable it sits in.
        assert!(buf.move_caret(1));
        assert_eq!(buf.caret(), 5);
        assert!(buf.move_caret(-1));
        assert_eq!(buf.caret(), 2);
    }

    #[test]
    fn test_clear_returns_the_buffer_to_its_initial_state() {
        let mut buf = buffer_with("nihaoa", &[0, 2, 5, 6]);
        buf.mark_session_start(1_700_000_000_000);
        assert!(buf.move_caret(-1));
        buf.clear();
        assert_eq!(buf, InputBuffer::new());
    }

    #[test]
    fn test_set_boundaries_adopts_a_grid_that_describes_the_input() {
        let mut buf = buffer_with("zhongguo", &[0, 5, 8]);
        assert_eq!(buf.backspace(), BackspaceOutcome::RemovedSyllable);
        assert_eq!(buf.raw(), "zhong");

        // Without the grid the same keystroke would have removed one character only.
        let mut per_char = InputBuffer::new();
        for ch in "zhongguo".chars() {
            assert!(per_char.push_char(ch).is_ok());
        }
        assert_eq!(per_char.backspace(), BackspaceOutcome::RemovedSyllable);
        assert_eq!(per_char.raw(), "zhonggu");
    }

    /// An unusable grid cannot come from the segmentation, so the buffer treats it as
    /// an internal defect: the debug assertion fires, and a release build ignores the
    /// grid and keeps the previous one.
    #[test]
    #[should_panic(expected = "grid")]
    fn test_set_boundaries_trips_the_debug_guard_on_an_invalid_grid() {
        let mut buf = buffer_with("ni", &[0, 2]);
        buf.set_boundaries(&[0, 1]);
    }

    #[test]
    fn test_is_valid_grid_accepts_only_grids_that_describe_the_input() {
        assert!(is_valid_grid(&[0], ""));
        assert!(is_valid_grid(&[0, 2], "ni"));
        assert!(is_valid_grid(&[0, 2, 5, 6], "nihaoa"));
        // Empty, not anchored at 0, not ending at the end of the input, not strictly
        // increasing, and past the end of the input.
        assert!(!is_valid_grid(&[], ""));
        assert!(!is_valid_grid(&[1, 2], "ni"));
        assert!(!is_valid_grid(&[0, 1], "ni"));
        assert!(!is_valid_grid(&[0, 2, 2], "ni"));
        assert!(!is_valid_grid(&[0, 3], "ni"));
        assert!(!is_valid_grid(&[0, 5], "nihaoa"));
    }

    #[test]
    fn test_mark_session_start_is_reported_until_the_buffer_is_cleared() {
        let mut buf = InputBuffer::new();
        assert_eq!(buf.started_at_unix_ms(), 0);
        buf.mark_session_start(1_700_000_000_000);
        assert_eq!(buf.started_at_unix_ms(), 1_700_000_000_000);
        buf.clear();
        assert_eq!(buf.started_at_unix_ms(), 0);
    }

    /// One operation of the property test's random sequence.
    #[derive(Clone, Debug)]
    enum Op {
        Push(char),
        Backspace,
        MoveCaret(i8),
        Clear,
    }

    /// Builds one random operation, mixing characters the input alphabet accepts with
    /// ones it rejects so that the property covers both the appending and the
    /// rejecting path.
    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (b'a'..=b'z').prop_map(|byte| Op::Push(char::from(byte))),
            2 => (b'A'..=b'Z').prop_map(|byte| Op::Push(char::from(byte))),
            1 => Just(Op::Push('\'')),
            1 => any::<char>().prop_map(Op::Push),
            3 => Just(Op::Backspace),
            2 => (-2i8..=2).prop_map(Op::MoveCaret),
            1 => Just(Op::Clear),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        #[test]
        fn test_input_buffer_ops_preserve_the_length_caret_and_grid_invariants(
            ops in prop::collection::vec(op_strategy(), 0..128)
        ) {
            let mut buf = InputBuffer::new();
            for op in ops {
                match op {
                    // The input alphabet and the length cap are the only rejections.
                    Op::Push(ch) => {
                        let _ = buf.push_char(ch);
                    }
                    Op::Backspace => {
                        let _ = buf.backspace();
                    }
                    Op::MoveCaret(delta) => {
                        let _ = buf.move_caret(delta);
                    }
                    Op::Clear => buf.clear(),
                }
                prop_assert!(buf.raw().len() <= MAX_RAW_LEN);
                prop_assert!(buf.caret() as usize <= buf.raw().len());
                prop_assert!(buf.raw().is_char_boundary(buf.caret() as usize));
                prop_assert!(is_valid_grid(&buf.last_boundaries, buf.raw()));
            }
        }
    }
}
