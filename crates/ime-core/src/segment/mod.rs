//! Syllable segmentation: the syllable alphabet, the input normalizer and the
//! segmentation graph.
//!
//! Responsibility: turn one raw input string into the graph of *every* legal
//! syllable segmentation of it, and report a deterministic error for input that
//! cannot be segmented at all.
//!
//! Boundaries: this layer is pure and owns no dictionary, no clock and no global
//! state. It knows the syllable alphabet and the shape of the graph, but it does not
//! score paths -- ranking the cuts is the Viterbi decoder's job -- and it never turns
//! a syllable into a candidate. Everything it hands out is a plain value the caller
//! can hold across keystrokes.
//!
//! # Entry points
//!
//! - [`SyllableDag::build`] builds the graph for one input, reusing its buffers.
//! - [`SyllableDag::has_path`] says whether the input is segmentable at all.
//! - [`SyllableDag::edges_from`] walks the graph, and [`SyllableDag::syllable_text`]
//!   reads a syllable back out of an edge.
//! - [`SyllableDag::path_count`] counts the segmentations, for diagnostics and tests.
//! - [`SyllableDag::best_segmentation_hint`] gives the default cut the preedit shows
//!   before any scoring has run.
//! - [`lookup`] and [`normalize`] expose the syllable alphabet on its own; the
//!   dictionary compiler uses them to validate the keys it writes.

pub mod dag;
pub mod syllable;

pub use crate::segment::dag::{DagEdge, EdgeKind, SyllableDag};
pub use crate::segment::syllable::{
    DROPPED_CAP, DroppedChars, MAX_NODES, MAX_NORMALIZED_LEN, MAX_RAW_LEN, MAX_SYLLABLE_LEN,
    NormalizedResult, SYLLABLE_COUNT, SYLLABLES, lookup, normalize, syllable_at,
};

use smallvec::SmallVec;

/// Inline capacity of the boundary buffer [`SyllableDag::best_segmentation_hint`]
/// fills, matching the input buffer's boundary vector.
pub const HINT_INLINE_BOUNDARIES: usize = 16;

impl SyllableDag {
    /// Counts the segmentations that cover the whole input.
    ///
    /// The count saturates instead of overflowing, so a long repetitive input such
    /// as `xianxianxian...` answers with `u32::MAX` rather than wrapping. This is a
    /// diagnostic and test helper, not a decode-path call: it is exact, but a caller
    /// that only needs to know whether a path exists should ask
    /// [`SyllableDag::has_path`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::segment::SyllableDag;
    ///
    /// let mut dag = SyllableDag::new();
    /// assert!(dag.build("nihao").is_ok());
    /// // "nihao" cuts as ni|hao and ni|ha|o.
    /// assert_eq!(dag.path_count(), 2);
    /// ```
    pub fn path_count(&self) -> u32 {
        let len = usize::from(self.len());
        if len == 0 {
            return 0;
        }
        // A fixed-size stack table keeps this allocation-free; the grid is at most
        // `MAX_NODES` wide, and the reverse sweep is a plain count of the paths from
        // each node to the terminal node.
        let mut counts = [0u32; MAX_NODES];
        counts[len] = 1;
        for i in (0..len).rev() {
            let mut total = 0u32;
            for edge in self.edges_from(i) {
                total = total.saturating_add(counts[usize::from(edge.end)]);
            }
            counts[i] = total;
        }
        counts[0]
    }

    /// Fills `out` with the default segmentation of the input, as node boundaries.
    ///
    /// The returned boundaries always start at node `0`; the last one is the terminal
    /// node when the input is segmentable. The hint is the cut with the fewest
    /// syllables, and among equally short cuts it prefers the longer syllable at each
    /// step, which is the cut a reader expects: `nihaoa` is hinted as `ni|hao|a` and
    /// not as `ni|ha|o|a`. It is what the preedit shows before any scoring has run.
    ///
    /// Returns `true` when the input is segmentable. When it is not, `out` holds the
    /// single pass-through range `[0, len]` and the answer is `false`, so the caller
    /// can still render the whole input as one span.
    ///
    /// `out` is cleared first and then filled, so a caller that keeps one buffer
    /// across keystrokes does not allocate; the inline capacity covers the common
    /// case of a short input, and only a very long input spills to the heap.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::segment::SyllableDag;
    /// use smallvec::SmallVec;
    ///
    /// let mut dag = SyllableDag::new();
    /// assert!(dag.build("nihaoa").is_ok());
    /// let mut boundaries = SmallVec::<[u16; 16]>::new();
    /// assert!(dag.best_segmentation_hint(&mut boundaries));
    /// assert_eq!(boundaries.as_slice(), &[0, 2, 5, 6]);
    /// ```
    pub fn best_segmentation_hint(
        &self,
        out: &mut SmallVec<[u16; HINT_INLINE_BOUNDARIES]>,
    ) -> bool {
        out.clear();
        let len = usize::from(self.len());
        if !self.has_path() {
            out.push(0);
            if len > 0 {
                out.push(self.len());
            }
            return false;
        }

        // Fewest syllables wins. The grid is acyclic, so one forward sweep settles
        // every node. Only strict improvements are taken, so a tie keeps the first
        // predecessor seen -- the one with the smallest index, that is the longest
        // trailing syllable.
        let mut cost = [u16::MAX; MAX_NODES];
        let mut previous = [0u16; MAX_NODES];
        cost[0] = 0;
        for i in 0..len {
            let base = cost[i];
            if base == u16::MAX {
                continue;
            }
            for edge in self.edges_from(i) {
                let end = usize::from(edge.end);
                let candidate = base.saturating_add(1);
                if candidate < cost[end] {
                    cost[end] = candidate;
                    previous[end] = u16::try_from(i).unwrap_or(u16::MAX);
                }
            }
        }

        // Walk back from the terminal node. Every node on the way was reached from a
        // strictly smaller node, so the walk ends at node 0.
        let mut reversed = [0u16; MAX_NODES];
        let mut count = 0usize;
        let mut node = self.len();
        while count < MAX_NODES {
            reversed[count] = node;
            count += 1;
            if node == 0 {
                break;
            }
            node = previous[usize::from(node)];
        }
        for index in (0..count).rev() {
            out.push(reversed[index]);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use ime_types::DecodeError;

    use super::*;

    /// Builds a graph for `raw` and returns it, failing the test if the input is not
    /// segmentable.
    fn dag_for(raw: &str) -> SyllableDag {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build(raw), Ok(()), "building {raw:?}");
        dag
    }

    #[test]
    fn test_path_count_counts_every_segmentation() {
        let cases = [
            ("a", 1),
            ("ni", 1),
            ("nihao", 2),
            ("nihaoa", 2),
            // `xian` cuts four ways: xian | xi'an | xia'n | xi'a'n. The last two exist
            // because `n` is a syllable in its own right (the interjection), which is
            // easy to miss when counting by hand.
            ("xian", 4),
            ("zhongguo", 2),
        ];
        for (raw, expected) in cases {
            let dag = dag_for(raw);
            assert_eq!(dag.path_count(), expected, "path count of {raw:?}");
        }
    }

    #[test]
    fn test_path_count_is_zero_without_a_path() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.path_count(), 0);
        assert!(matches!(dag.build("zzz"), Err(DecodeError::NoPath { .. })));
        assert_eq!(dag.path_count(), 0);
    }

    #[test]
    fn test_best_segmentation_hint_prefers_the_fewest_syllables() {
        let cases: [(&str, &[u16]); 3] = [
            ("nihao", &[0, 2, 5]),
            ("nihaoa", &[0, 2, 5, 6]),
            ("xian", &[0, 4]),
        ];
        for (raw, expected) in cases {
            let dag = dag_for(raw);
            let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
            assert!(dag.best_segmentation_hint(&mut boundaries));
            assert_eq!(boundaries.as_slice(), expected, "hint for {raw:?}");
        }
    }

    #[test]
    fn test_best_segmentation_hint_breaks_ties_toward_the_longer_syllable() {
        // "anan" cuts as a|nan and as an|an; both are two syllables. The hint keeps
        // the longer syllable, so it answers a|nan.
        let dag = dag_for("anan");
        let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(dag.best_segmentation_hint(&mut boundaries));
        assert_eq!(boundaries.as_slice(), &[0, 1, 4]);
    }

    #[test]
    fn test_best_segmentation_hint_keeps_the_whole_range_when_there_is_no_path() {
        let mut dag = SyllableDag::new();
        assert!(matches!(dag.build("zzz"), Err(DecodeError::NoPath { .. })));
        let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(!dag.best_segmentation_hint(&mut boundaries));
        assert_eq!(boundaries.as_slice(), &[0, 3]);
    }

    #[test]
    fn test_best_segmentation_hint_handles_a_forced_boundary() {
        let dag = dag_for("ni'hao");
        let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert!(dag.best_segmentation_hint(&mut boundaries));
        // Node 3 is the node after the marker: the marker belongs to no syllable.
        assert_eq!(boundaries.as_slice(), &[0, 3, 6]);
    }

    #[test]
    fn test_best_segmentation_hint_reuses_the_callers_buffer() {
        let mut dag = SyllableDag::new();
        let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        assert_eq!(dag.build("xian"), Ok(()));
        assert!(dag.best_segmentation_hint(&mut boundaries));
        assert_eq!(boundaries.as_slice(), &[0, 4]);
        assert_eq!(dag.build("nihaoa"), Ok(()));
        assert!(dag.best_segmentation_hint(&mut boundaries));
        assert_eq!(boundaries.as_slice(), &[0, 2, 5, 6]);
    }
}
