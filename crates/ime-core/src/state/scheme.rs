//! The scheme side of one composing session: the layout a composition's keystrokes
//! follow, and the rewrite that turns them into the spelling the segmentation layer
//! reads.
//!
//! Responsibility: hold the layout and the mixed-input switch a composition runs
//! under, rewrite the raw input through them before segmentation, and report where
//! every scheme syllable starts in the keystrokes the user typed.
//!
//! Boundaries: a pure function of the keystrokes, the layout and the switch -- no
//! file, no clock, no environment and no global state (0.4 rule 4). What it produces
//! is a full-pinyin spelling, so the segmentation layer never learns that schemes
//! exist: the DAG, the lattice, the Viterbi pass and the preedit builder are
//! untouched, and a scheme session decodes through exactly the path a full-pinyin one
//! does.
//!
//! # Why the rewrite runs here and not in the decoder
//!
//! The session owns the input buffer, and the buffer's syllable grid has to be
//! written in the offsets of the keystrokes the user pressed: Backspace removes one
//! scheme syllable, and the caret moves between them. The decoder is handed the
//! rewritten spelling and never sees a keystroke, so it cannot report where a scheme
//! syllable started; the walk below can, because it is the pass that reads the
//! keystrokes in order.
//!
//! # The two readings of a keystroke stream
//!
//! - The layout's own reading, which [`SchemeMap`] performs and which the shipped
//!   tables are validated against. It answers the common case, and its text is the
//!   one the session decodes.
//! - The mixed-input ladder of [`fallback`](crate::shuangpin::fallback), which reads
//!   a position the layout cannot spell as full pinyin before falling back to the
//!   single keystroke. Its reading can cover more keys than the layout's -- Xiaohe
//!   spells `hng` as `h` + `neng`, while the full-pinyin reading takes all three
//!   keys as the one syllable -- so where the ladder leaves the layout's reading, its
//!   own text is the one the switch asked for.
//!
//! The walk runs the ladder once over the whole stream, which is what produces the
//! grid and the origins in the same pass. It builds the ladder's text as it goes,
//! because a position the layout cannot spell can be the last one and the text has to
//! be complete by the time the walk ends; where the ladder never left the layout's
//! reading, that text is the map's text and the map's is the one used.
//!
//! # Syllable numbering
//!
//! One scheme syllable rewrites to exactly one full-pinyin syllable, so the syllable
//! indices the segmentation graph reports are scheme syllable indices as long as the
//! graph cuts the spelling the way the layout read it. Where it re-cuts -- Xiaohe
//! reads `nihao` as `ni` + `ha` + `o`, while the graph may cut the same letters as
//! `ni` + `hao` -- the indices follow the graph, which is the cut the window's preedit
//! draws. The grid this module reports is the layout's own cut either way, because it
//! is the numbering Backspace and the caret work in: the units are the keystroke
//! groups the user pressed.
//!
//! # Latched per composition
//!
//! The layout is adopted while the session is idle and is not re-read once a
//! composition is live. A reload therefore cannot change the reading of the input the
//! user has already typed -- which would make the candidate list jump under the caret
//! -- and the new layout answers from the next composition on (0.4 rule 10).

use ime_types::{ImeError, SchemeId};
use smallvec::SmallVec;

use crate::segment::{HINT_INLINE_BOUNDARIES, syllable_at};
use crate::shuangpin::fallback::{FallbackPolicy, SyllableOrigin, is_carriable, resolve};
use crate::shuangpin::{SchemeMap, SchemeTable, table_for};

/// The scheme side of one composing session.
///
/// One of these lives in every [`Session`](crate::state::Session). It holds the
/// buffers a rewrite fills, so a keystroke rewrites the input without allocating
/// after the first one of a given length.
#[derive(Debug)]
pub(crate) struct SchemeSession {
    /// The layout the current composition's keystrokes follow.
    scheme: SchemeId,
    /// Whether a keystroke the layout cannot read is read as full pinyin.
    keep_full_pinyin: bool,
    /// The layout's own reading of the keystrokes, and the syllable alignment it
    /// carries. Empty while full pinyin is active.
    map: SchemeMap,
    /// The ladder's reading of the keystrokes, used where it left the layout's own.
    ladder: String,
    /// Whether the last rewrite answered from the ladder's text rather than from the
    /// map's.
    used_ladder: bool,
    /// `grid[i]` is where scheme syllable `i` starts in the raw keystrokes, and the
    /// last entry is the end of the input.
    grid: SmallVec<[u16; HINT_INLINE_BOUNDARIES]>,
    /// One buffer reused across positions, so composing a syllable allocates nothing.
    scratch: String,
}

impl Default for SchemeSession {
    /// The state of a session that has not been configured yet: full pinyin, the
    /// mixed-input reading on, and empty buffers.
    ///
    /// The session latches the configuration while it is idle, before the first
    /// composition starts, so these values are what a session that has never been
    /// stepped reads.
    fn default() -> Self {
        Self {
            scheme: SchemeId::FULL,
            keep_full_pinyin: true,
            map: SchemeMap::default(),
            ladder: String::new(),
            used_ladder: false,
            grid: SmallVec::new(),
            scratch: String::new(),
        }
    }
}

impl SchemeSession {
    /// Adopts the layout and the mixed-input switch a composition runs under.
    ///
    /// The caller latches while the session is idle, so a reload that changes either
    /// value takes effect on the next composition rather than under the caret of the
    /// one in progress.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn latch(&mut self, scheme: SchemeId, keep_full_pinyin: bool) {
        self.scheme = scheme;
        self.keep_full_pinyin = keep_full_pinyin;
    }

    /// Returns the layout the current composition's keystrokes follow.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn id(&self) -> SchemeId {
        self.scheme
    }

    /// Returns whether a double-pinyin layout is answering the keystrokes.
    ///
    /// Full pinyin is the identity and needs no rewrite: the input is already the
    /// spelling the segmentation layer reads.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn is_active(&self) -> bool {
        self.scheme != SchemeId::FULL
    }

    /// Rewrites `raw` through the layout, filling the text and the syllable grid.
    ///
    /// `raw` is read in the scheme key alphabet, which is lower case: the caller folds
    /// the keystrokes before handing them over. The map folds case itself, but the
    /// mixed-input ladder's key table is indexed by lower-case keys, so an unfolded
    /// stream would be read as a run of single keystrokes and the grid would not
    /// describe the syllables the text holds. Folding rewrites byte values only and
    /// moves no offset, so the grid still names positions in the keystrokes the user
    /// pressed.
    ///
    /// # Returns
    ///
    /// `Ok(())` once [`SchemeSession::text`] and [`SchemeSession::grid`] describe the
    /// keystrokes. Full pinyin is the identity and leaves both empty, because there is
    /// nothing to rewrite and no alignment to keep.
    ///
    /// # Errors
    ///
    /// [`ImeError::SchemeUnsupported`] when the latched layout names a scheme this
    /// build does not implement. The caller reports `decode/scheme-unsupported` and
    /// reads the input as full pinyin rather than refusing to compose, which is what
    /// the contract asks of a number a newer build wrote.
    ///
    /// # Panics
    ///
    /// Never: every offset is the walk's own cursor, which advances by at least one
    /// byte per position and never past the end of the input.
    pub(crate) fn rewrite(&mut self, raw: &str) -> Result<(), ImeError> {
        let Some(table) = table_for(self.scheme)? else {
            self.clear();
            return Ok(());
        };
        // The layout's own reading, with the alignment it carries. It is the reading
        // the shipped tables are validated against, so the text comes from here
        // wherever the ladder below stays inside it.
        self.map = SchemeMap::build(raw, self.scheme)?.unwrap_or_default();
        self.ladder.clear();
        self.grid.clear();
        self.grid.push(0);
        self.used_ladder = false;
        let policy = FallbackPolicy::from_keep_full_pinyin(self.keep_full_pinyin);
        self.walk(raw, table, policy)?;
        self.close_grid(raw.len());
        Ok(())
    }

    /// Returns the full-pinyin spelling the segmentation layer reads.
    ///
    /// Empty while full pinyin is active, where the input needs no rewrite.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn text(&self) -> &str {
        if self.used_ladder {
            &self.ladder
        } else {
            self.map.text()
        }
    }

    /// Returns the byte offsets of the scheme syllables in the raw keystrokes.
    ///
    /// The list starts at `0` and ends at the end of the input, so it is a usable
    /// grid for [`InputBuffer::set_boundaries`](crate::input::InputBuffer::set_boundaries):
    /// one unit per scheme syllable, which is what Backspace removes and what the
    /// caret moves between. Empty while full pinyin is active.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn grid(&self) -> &[u16] {
        &self.grid
    }

    /// Empties the rewritten text, the grid and the flags, keeping the buffers.
    fn clear(&mut self) {
        self.map = SchemeMap::default();
        self.ladder.clear();
        self.grid.clear();
        self.grid.push(0);
        self.used_ladder = false;
    }

    /// Walks the keystrokes, filling the grid and the ladder's own reading.
    ///
    /// The walk reads one position at a time. A forced boundary is not a position: it
    /// is folded into the text exactly as the rewrite folds it, so that the text this
    /// produces is a fixed point of the segmentation layer's normalizer and its
    /// offsets are the graph's own.
    fn walk(
        &mut self,
        raw: &str,
        table: &SchemeTable,
        policy: FallbackPolicy,
    ) -> Result<(), ImeError> {
        let mut at = 0usize;
        let mut pending_marker = false;
        while at < raw.len() {
            let rest = raw.get(at..).unwrap_or("");
            let Some(ch) = rest.chars().next() else {
                break;
            };
            if ch == '\'' {
                pending_marker = true;
                at = at.saturating_add(ch.len_utf8());
                continue;
            }
            let Some(resolved) = resolve(table, rest.as_bytes(), policy, &mut self.scratch)? else {
                // Unreachable for a non-empty slice; the walk stops rather than
                // inventing a position the cursor could not advance over.
                break;
            };
            if resolved.origin() == SyllableOrigin::FullPinyinFallback {
                self.used_ladder = true;
            }
            let consumed = usize::from(resolved.consumed()).max(1);
            let carried = resolved.full().is_none() && is_carriable(ch);
            let spelled = resolved.full().and_then(syllable_at);
            if spelled.is_some() || carried {
                // A marker is folded in only where the position produces something: a
                // keystroke the layout has no entry for carries the marker on to the
                // syllable after it, exactly as the rewrite folds it, and a marker that
                // never reaches a syllable is folded away with the end of the input.
                self.flush_marker(&mut pending_marker);
                match spelled {
                    Some(spelling) => self.ladder.push_str(spelling),
                    None => self.ladder.push(ch.to_ascii_lowercase()),
                }
                let end = u16::try_from(at.saturating_add(consumed)).unwrap_or(u16::MAX);
                self.grid.push(end);
            }
            at = at.saturating_add(consumed);
        }
        Ok(())
    }

    /// Emits a pending forced-boundary marker when the text can carry one.
    ///
    /// A leading marker, a doubled one and a trailing one carry no information, and
    /// the segmentation layer's normalizer folds them away. Folding them here too is
    /// what keeps this text a fixed point of that normalizer.
    fn flush_marker(&mut self, pending: &mut bool) {
        if core::mem::take(pending) && !self.ladder.is_empty() && !self.ladder.ends_with('\'') {
            self.ladder.push('\'');
        }
    }

    /// Closes the grid at the end of the input.
    ///
    /// The walk pushes a boundary after every syllable it keeps, and the bytes it
    /// discarded -- a marker that leads the input, a key the layout does not define --
    /// belong to no unit. The end of the input is appended when the walk did not reach
    /// it, so that the grid always describes the whole input: a grid that stopped
    /// short would be refused by the buffer and leave Backspace deleting by whatever
    /// grid the last keystroke left behind.
    fn close_grid(&mut self, len: usize) {
        let end = u16::try_from(len).unwrap_or(u16::MAX);
        if self.grid.last().copied() != Some(end) {
            self.grid.push(end);
        }
    }
}
