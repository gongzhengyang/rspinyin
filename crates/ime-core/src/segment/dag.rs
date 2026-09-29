//! The syllable segmentation graph of one input string.
//!
//! Responsibility: hold every legal way to cut one raw input string into syllables,
//! and answer the three questions the rest of the engine asks of it: is there any
//! cut at all ([`SyllableDag::has_path`]), which cuts leave node `i`
//! ([`SyllableDag::edges_from`]), and what is the default cut
//! ([`SyllableDag::best_segmentation_hint`], defined in the parent module).
//!
//! Boundaries: the graph is built from the syllable alphabet alone. It assigns no
//! scores and knows no dictionary; ranking the cuts is the Viterbi decoder's job.
//! It is a pure value: no file, no clock, no global state.
//!
//! # Nodes and edges
//!
//! Node `i` is byte offset `i` of the normalized string, so the graph has
//! `len + 1` nodes and a path from node `0` to node `len` is one complete
//! segmentation. An edge covers `[start, edge.end)`, where `start` is the node it
//! was read from and `edge.end` is the node it reaches, which is also the first
//! byte of the next syllable.
//!
//! # Forced boundaries
//!
//! A `'` the user typed pins a syllable boundary, and the marker itself belongs to
//! no syllable. The edge of the syllable that ends on the marker therefore reaches
//! past it and is marked [`EdgeKind::Forced`], and the next syllable starts on the
//! node beyond the marker. [`SyllableDag::syllable_text`] returns the syllable text
//! with the marker stripped; callers should use it rather than slicing
//! [`SyllableDag::normalized`] themselves.
//!
//! # Allocation
//!
//! [`SyllableDag::build`] reuses the capacity of everything it holds: it clears the
//! node list and the per-node edge lists instead of dropping them, rewrites the
//! normalized string in place, and resizes the reachability and diagnostic buffers.
//! A DAG that has already built a string of a given length builds again without
//! touching the allocator, which is what keeps a decode free of allocations.

use ime_types::{DecodeError, SyllableId};
use smallvec::SmallVec;

use crate::segment::syllable::{self, DroppedChars, MAX_NODES, MAX_RAW_LEN, MAX_SYLLABLE_LEN};

/// How many edges one node holds before its edge list spills to the heap.
///
/// The widest node is an `a`-initial one, which can start at most five syllables
/// (`a`, `ai`, `an`, `ang`, `ao`), so the inline capacity covers every node of every
/// legal input.
const EDGE_INLINE: usize = 8;

/// One edge of the segmentation graph.
///
/// The edge covers `[start, edge.end)`, where `start` is the node it was read from;
/// see the module documentation for how a forced boundary marker sits in that range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DagEdge {
    /// Node this edge reaches: one past the last byte the syllable covers.
    pub end: u16,
    /// The syllable that was matched.
    pub syllable: SyllableId,
    /// Whether the boundary after this syllable was pinned by the user.
    pub kind: EdgeKind,
}

impl DagEdge {
    /// Returns `true` when the user pinned this boundary with `'`.
    pub fn is_forced(self) -> bool {
        self.kind == EdgeKind::Forced
    }
}

/// Why an edge ends where it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// The boundary was inferred from the syllable table.
    Normal,
    /// The user typed `'` after this syllable, so the boundary is pinned.
    Forced,
}

/// Every legal segmentation of one input string.
///
/// Build one with [`SyllableDag::new`] and reuse it across keystrokes: the buffers
/// it holds are sized for the first input and reused by every later build.
#[derive(Clone, Debug)]
pub struct SyllableDag {
    /// `edges[i]` holds the edges leaving node `i`, ordered by `(end, syllable)`.
    edges: Vec<SmallVec<[DagEdge; EDGE_INLINE]>>,
    /// Byte length of the normalized string, and index of the terminal node.
    len: u16,
    /// The normalized input the node indices refer to.
    normalized: String,
    /// `reachable_to_end[i]` is `true` when node `i` can still reach node `len`.
    reachable_to_end: Vec<bool>,
    /// Characters normalization discarded, for `decode/invalid-char` diagnostics.
    dropped: DroppedChars,
}

impl SyllableDag {
    /// Creates an empty graph: no input, no edges, no path.
    pub fn new() -> Self {
        Self {
            edges: Vec::new(),
            len: 0,
            normalized: String::new(),
            reachable_to_end: vec![false],
            dropped: DroppedChars::default(),
        }
    }

    /// Rebuilds the graph for `raw`, reusing every buffer the graph already holds.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::EmptyInput`] when `raw` is empty.
    /// - [`DecodeError::TooLong`] when `raw` is longer than [`MAX_RAW_LEN`]. This is
    ///   judged before normalization, so normalization can never enlarge an
    ///   oversized input.
    /// - [`DecodeError::NoPath`] when the normalized input has no legal
    ///   segmentation at all.
    ///
    /// On every error the graph is still left describing the input: the normalized
    /// string is set, [`SyllableDag::has_path`] returns `false`, and the dropped
    /// character report is filled in. Callers that degrade instead of failing (the
    /// decoder answers with a pass-through candidate) can therefore keep reading the
    /// segmentation data after an error.
    pub fn build(&mut self, raw: &str) -> Result<(), DecodeError> {
        if raw.is_empty() {
            self.reset();
            return Err(DecodeError::EmptyInput);
        }
        if raw.len() > MAX_RAW_LEN {
            self.reset();
            return Err(DecodeError::TooLong {
                len: raw.len(),
                max: MAX_RAW_LEN,
            });
        }

        let mut normalized = std::mem::take(&mut self.normalized);
        syllable::normalize_into(raw, &mut normalized, &mut self.dropped);

        let len = normalized.len();
        // Normalization grows at most one byte per byte (`v` into `ü`), so a legal
        // request can never produce more than `MAX_NORMALIZED_LEN` bytes.
        self.len = u16::try_from(len).unwrap_or(u16::MAX);
        self.prepare_nodes(len);
        self.build_edges(&normalized);
        self.compute_reachability(len);
        self.normalized = normalized;

        if self.has_path() {
            Ok(())
        } else {
            let raw = raw.to_owned();
            Err(DecodeError::NoPath { raw })
        }
    }

    /// Returns `true` when at least one segmentation covers the whole input.
    pub fn has_path(&self) -> bool {
        self.len > 0 && self.reachable_to_end.first().copied().unwrap_or(false)
    }

    /// Returns the byte length of the normalized string, which is also the index of
    /// the terminal node.
    pub fn len(&self) -> u16 {
        self.len
    }

    /// Returns `true` when the graph describes no input: before the first build, or
    /// after an input that normalized to nothing.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the canonical spelling of the input the graph was built from.
    ///
    /// This is the string node indices refer to, and the string the preedit renders.
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Returns the characters normalization discarded, for `decode/invalid-char`
    /// diagnostics.
    pub fn dropped_chars(&self) -> &DroppedChars {
        &self.dropped
    }

    /// Returns the edges leaving node `at`, ordered by `(end, syllable)`.
    ///
    /// Returns an empty slice for a node index outside the graph rather than
    /// panicking, so that a walker cannot be tripped up by a degenerate input.
    pub fn edges_from(&self, at: usize) -> &[DagEdge] {
        match self.edges.get(at) {
            Some(edges) => edges.as_slice(),
            None => &[],
        }
    }

    /// Returns the text of the syllable an edge covers, without any boundary marker.
    ///
    /// `start` is the node the edge was read from. Callers that build dictionary keys
    /// out of a path must use this instead of slicing [`SyllableDag::normalized`]:
    /// a [`EdgeKind::Forced`] edge reaches past the `'` the user typed, so a plain
    /// slice would carry that marker into the key. Returns an empty string when the
    /// range does not describe a syllable.
    pub fn syllable_text(&self, start: u16, edge: DagEdge) -> &str {
        let start = usize::from(start);
        let end = usize::from(edge.end);
        let text_end = if edge.kind == EdgeKind::Forced {
            end.saturating_sub(1)
        } else {
            end
        };
        self.normalized.get(start..text_end).unwrap_or("")
    }

    /// Drops every trace of the previous input, freeing no capacity.
    fn reset(&mut self) {
        self.edges.clear();
        self.len = 0;
        self.normalized.clear();
        self.dropped.clear();
        self.reachable_to_end.clear();
        self.reachable_to_end.push(false);
    }

    /// Grows the node grid to `len + 1` nodes, reusing the existing capacity.
    fn prepare_nodes(&mut self, len: usize) {
        let nodes = len + 1;
        self.edges.clear();
        self.edges.reserve(nodes);
        for _ in 0..nodes {
            self.edges.push(SmallVec::new());
        }
    }

    /// Fills the edge list of every node from the syllable table.
    fn build_edges(&mut self, normalized: &str) {
        let bytes = normalized.as_bytes();
        let len = bytes.len();
        // A syllable never spans a boundary marker, so node `i` may only reach as far
        // as the next marker. One reverse scan fills the whole limit table.
        let mut limits = [0u16; MAX_NODES];
        let mut next_marker = u16::try_from(len).unwrap_or(u16::MAX);
        for i in (0..len).rev() {
            limits[i] = next_marker;
            if bytes[i] == b'\'' {
                next_marker = u16::try_from(i).unwrap_or(u16::MAX);
            }
        }

        for (i, &limit_end) in limits.iter().enumerate().take(len) {
            // Only an ASCII letter, or the non-ASCII syllable `ê`, can start a
            // syllable. This also rejects a byte in the middle of a multi-byte
            // character, which keeps the slicing below on character boundaries.
            let starts_syllable = match normalized.get(i..) {
                Some(rest) => rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == 'ê'),
                None => false,
            };
            if !starts_syllable {
                continue;
            }

            let limit = usize::from(limit_end);
            for length in 1..=MAX_SYLLABLE_LEN {
                let end = i + length;
                if end > limit {
                    break;
                }
                let Some(text) = normalized.get(i..end) else {
                    continue;
                };
                let Some(syllable) = syllable::lookup(text) else {
                    continue;
                };
                let forced = end < len && bytes[end] == b'\'';
                // The marker belongs to no syllable, so the edge reaches past it and
                // the next syllable starts on the node beyond the marker.
                let edge_end = if forced { end + 1 } else { end };
                let kind = if forced {
                    EdgeKind::Forced
                } else {
                    EdgeKind::Normal
                };
                self.edges[i].push(DagEdge {
                    end: u16::try_from(edge_end).unwrap_or(u16::MAX),
                    syllable,
                    kind,
                });
            }
        }
    }

    /// Marks which nodes can still reach the terminal node, by a reverse sweep.
    fn compute_reachability(&mut self, len: usize) {
        self.reachable_to_end.clear();
        self.reachable_to_end.resize(len + 1, false);
        self.reachable_to_end[len] = true;
        for i in (0..len).rev() {
            let reaches_end = self.edges[i]
                .iter()
                .any(|edge| self.reachable_to_end[usize::from(edge.end)]);
            self.reachable_to_end[i] = reaches_end;
        }
    }
}

impl Default for SyllableDag {
    /// Creates an empty graph, identical to [`SyllableDag::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest raw input a request may carry, as a legal, segmentable string.
    fn sixty_four_bytes() -> String {
        // 12 x 5 bytes plus a 4-byte tail: exactly the hard limit.
        let raw = "nihao".repeat(12) + "niha";
        assert_eq!(raw.len(), MAX_RAW_LEN);
        raw
    }

    #[test]
    fn test_new_dag_is_empty_and_has_no_path() {
        let dag = SyllableDag::new();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
        assert!(!dag.has_path());
        assert_eq!(dag.normalized(), "");
        assert!(dag.edges_from(0).is_empty());
        assert_eq!(dag.path_count(), 0);
    }

    #[test]
    fn test_build_empty_input_returns_empty_input_error() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build(""), Err(DecodeError::EmptyInput));
        assert!(dag.is_empty());
        assert!(!dag.has_path());
    }

    #[test]
    fn test_build_too_long_input_returns_too_long_error() {
        let mut dag = SyllableDag::new();
        let raw = "nihao".repeat(13);
        assert_eq!(raw.len(), MAX_RAW_LEN + 1);
        let expected = DecodeError::TooLong {
            len: MAX_RAW_LEN + 1,
            max: MAX_RAW_LEN,
        };
        assert_eq!(dag.build(&raw), Err(expected));
        assert!(dag.is_empty());
        assert!(!dag.has_path());
        assert_eq!(dag.edges_from(0).len(), 0);
    }

    #[test]
    fn test_build_accepts_input_at_the_length_limit() {
        let mut dag = SyllableDag::new();
        let raw = sixty_four_bytes();
        assert_eq!(dag.build(&raw), Ok(()));
        assert_eq!(usize::from(dag.len()), MAX_RAW_LEN);
        assert!(dag.has_path());
    }

    #[test]
    fn test_build_rejects_input_that_cannot_be_segmented() {
        let mut dag = SyllableDag::new();
        for raw in ["zzz", "qqq", "x", "vv", "ngz", "io"] {
            let built = dag.build(raw);
            assert!(
                matches!(built, Err(DecodeError::NoPath { .. })),
                "{raw:?} must not be segmentable"
            );
            assert!(!dag.has_path(), "{raw:?} must not have a path");
            assert_eq!(dag.path_count(), 0);
        }
    }

    #[test]
    fn test_build_all_non_letter_input_returns_no_path() {
        let mut dag = SyllableDag::new();
        for raw in ["###", "123", " ", "你好"] {
            assert!(
                matches!(dag.build(raw), Err(DecodeError::NoPath { .. })),
                "{raw:?} must be reported as having no path"
            );
            assert!(!dag.has_path());
        }
    }

    #[test]
    fn test_build_drops_stray_characters_but_still_finds_the_path() {
        // Characters outside the alphabet are dropped, not fatal: the remaining
        // syllables are segmented normally.
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("ni hao!"), Ok(()));
        assert_eq!(dag.normalized(), "nihao");
        assert_eq!(dag.dropped_chars().total(), 2);
        assert!(dag.has_path());
    }

    #[test]
    fn test_build_apostrophe_only_input_returns_no_path() {
        let mut dag = SyllableDag::new();
        for raw in ["'", "''", "'''", "''''"] {
            assert!(
                matches!(dag.build(raw), Err(DecodeError::NoPath { .. })),
                "{raw:?} folds away to nothing and has no path"
            );
            assert_eq!(dag.normalized(), "");
        }
    }

    #[test]
    fn test_build_finds_a_path_for_every_table_syllable() {
        let mut dag = SyllableDag::new();
        for entry in syllable::SYLLABLES {
            assert_eq!(dag.build(entry), Ok(()), "building {entry:?}");
            assert!(dag.has_path(), "{entry:?} must have a path");
            assert!(
                dag.path_count() >= 1,
                "{entry:?} must have at least one cut"
            );
        }
    }

    #[test]
    fn test_build_nihao_yields_two_paths() {
        // "nihao" cuts as ni|hao and as ni|ha|o, and those are the only two cuts.
        // The leading `n` is a syllable of its own, but no syllable starts with `i`,
        // so that branch dead-ends and contributes no path.
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("nihao"), Ok(()));
        assert_eq!(dag.path_count(), 2);
    }

    #[test]
    fn test_build_marks_forced_boundaries_and_folds_the_marker() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("ni'hao"), Ok(()));
        assert_eq!(dag.normalized(), "ni'hao");
        assert_eq!(usize::from(dag.len()), 6);

        let edges = dag.edges_from(0);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].end, 1);
        assert!(!edges[0].is_forced());
        // The `ni` edge ends on the marker and reaches past it, so the next syllable
        // starts on node 3 rather than on node 2.
        assert_eq!(edges[1].end, 3);
        assert!(edges[1].is_forced());
        assert_eq!(dag.syllable_text(0, edges[1]), "ni");
        assert_eq!(dag.path_count(), 2);
    }

    #[test]
    fn test_build_keeps_apostrophe_input_from_crossing_the_marker() {
        // Without the marker, "nihao" also cuts as ni|ha|o; with it, the cut after
        // `ni` is pinned but the tail is still free, so two cuts survive.
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("ni'hao"), Ok(()));
        let pinned = dag
            .edges_from(0)
            .iter()
            .filter(|edge| edge.is_forced())
            .count();
        assert_eq!(pinned, 1);
        assert_eq!(dag.path_count(), 2);
    }

    #[test]
    fn test_syllable_text_returns_the_text_an_edge_covers() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("zhongguo"), Ok(()));
        let edges = dag.edges_from(0);
        assert_eq!(edges.len(), 1);
        assert_eq!(dag.syllable_text(0, edges[0]), "zhong");
        assert_eq!(dag.syllable_text(5, edges[0]), "");
    }

    #[test]
    fn test_edges_from_is_sorted_by_end_and_syllable() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("xianzaihaoa"), Ok(()));
        for at in 0..=usize::from(dag.len()) {
            let edges = dag.edges_from(at);
            for pair in edges.windows(2) {
                let left = (pair[0].end, pair[0].syllable);
                let right = (pair[1].end, pair[1].syllable);
                assert!(
                    left < right,
                    "node {at} is out of order: {left:?} {right:?}"
                );
            }
        }
    }

    #[test]
    fn test_edges_from_rejects_a_node_outside_the_graph() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("ni"), Ok(()));
        assert!(dag.edges_from(usize::from(dag.len()) + 1).is_empty());
        assert!(dag.edges_from(usize::MAX).is_empty());
    }

    #[test]
    fn test_build_reuses_the_graph_without_leaving_stale_nodes() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("zhongguoxiangqi"), Ok(()));
        assert_eq!(dag.build("a"), Ok(()));
        assert_eq!(dag.normalized(), "a");
        assert_eq!(dag.len(), 1);
        assert!(dag.has_path());
        // Node 1 is the terminal node of "a" and node 2 must not exist any more.
        assert!(dag.edges_from(1).is_empty());
        assert!(dag.edges_from(2).is_empty());
        assert_eq!(dag.path_count(), 1);
    }

    #[test]
    fn test_build_reports_dropped_characters_it_discarded() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("ni3hao"), Ok(()));
        assert_eq!(dag.normalized(), "nihao");
        assert_eq!(dag.dropped_chars().total(), 1);
        assert_eq!(dag.dropped_chars().entries(), &[('3', 2)]);
        let expected = DecodeError::InvalidChar { ch: '3', at: 2 };
        assert_eq!(dag.dropped_chars().first_error(), Some(expected));
    }

    #[test]
    fn test_build_normalizes_umlaut_input_forms_onto_the_table_spelling() {
        let mut dag = SyllableDag::new();
        let cases = [("lv", "lü"), ("nv", "nü"), ("jv", "ju"), ("xve", "xue")];
        for (raw, expected) in cases {
            assert_eq!(dag.build(raw), Ok(()), "building {raw:?}");
            assert_eq!(dag.normalized(), expected);
            assert!(dag.has_path());
        }
    }

    #[test]
    fn test_build_treats_a_folded_apostrophe_as_no_boundary() {
        let mut dag = SyllableDag::new();
        assert_eq!(dag.build("'ni''hao'"), Ok(()));
        assert_eq!(dag.normalized(), "ni'hao");
        assert_eq!(dag.dropped_chars().total(), 3);
        assert_eq!(dag.path_count(), 2);
    }
}
