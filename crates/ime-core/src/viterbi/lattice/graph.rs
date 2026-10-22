//! The lattice graph: the word edges of one input, grouped by the node they leave.
//!
//! Responsibility: hold the edges a build produced and answer the reads the sweep and
//! the sweep's readers make -- the edges of one node, one edge by index, and the whole
//! key of a widened word. The graph is plain data: nothing here scores an edge or
//! decides an order.
//!
//! Boundaries: the type is built by the build in the parent module and walked by the
//! sweep; it owns no buffers of its own beyond the edge vector, and the key arena its
//! ranges name belongs to the caller's workspace.

use ime_types::{CandidateSource, WordRef};
use smallvec::SmallVec;

use super::{
    FALLBACK_SINGLES, MAX_LATTICE_NODES, MAX_LATTICE_STARTS, MAX_WORD_SYLLABLES, WORDS_PER_KEY,
};

/// One word of the lattice.
///
/// The edge covers the byte range `[start, end)` of the normalized input, where
/// `start` is the node the edge leaves; see [`Lattice::edges_from`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatticeEdge<'dict> {
    /// Node the edge reaches: the byte offset just past the span's last syllable.
    pub end: u16,
    /// Syllables the span covers, counted on the graph rather than taken from the
    /// dictionary entry's own claim, so that a candidate's syllable accounting
    /// always adds up to the path it came from.
    pub syllables: u8,
    /// Characters in the word, for the length bonus. Counted once here because
    /// the sweep would otherwise recount it for every path that uses the edge.
    pub characters: u16,
    /// Where the word came from.
    pub source: CandidateSource,
    /// The word, borrowed from the dictionary's mapping.
    pub word: WordRef<'dict>,
    /// The key the word was looked up under, as a byte range into the lattice's key
    /// arena: the canonical spelling the walk built -- syllable texts joined with the
    /// separator -- which is what a later lookup of the same word will spell. An edge the
    /// extension walks added names no key in this arena: its range is empty, and the one
    /// reader of the ranges treats that as nothing to coin from.
    pub key: (u32, u16),
    /// Score penalty the edge carries, in the Q16.16 unit the sweep ranks with.
    ///
    /// Zero for an edge the exact walk produced, and the heaviest penalty of the
    /// spans the word covers for an extended one -- [`ABBREV_PENALTY_Q8`](crate::segment::abbrev::ABBREV_PENALTY_Q8)
    /// for an abbreviated one, the fuzzy penalty for a fuzzy variant, the correction
    /// penalty for a corrected span at any position of the word -- so that a full
    /// spelling outranks a widened reading at equal dictionary weight, wherever in
    /// the word the widening sits. The lattice records the penalty rather than
    /// applying it: the score of an edge is the sweep's to compute, and this is the
    /// one term of that score the lattice knows and the sweep cannot derive from the
    /// word and its predecessor. The order the edges sit in is a second, weaker
    /// guarantee -- the exact walk's edges come before the abbreviation walk's, so a
    /// tie falls the right way even on a sweep that ignores this field -- and the
    /// penalty is what makes the outcome a matter of the score rather than of the
    /// order.
    pub penalty_q8: i32,
}

/// Every word edge of one input, grouped by the node they leave.
///
/// # Examples
///
/// ```
/// use ime_core::segment::{Readings, SyllableDag};
/// use ime_core::viterbi::build_lattice;
/// use ime_core::viterbi::lattice::LatticeOptions;
/// use ime_types::{DecodeFlags, ImeError, Lexicon, WordFlags, WordIter, WordRef};
///
/// struct One;
/// impl Lexicon for One {
///     fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
///         let words = if key == "ni'hao" {
///             vec![WordRef { text: "你好", weight: 9, syl_count: 2, flags: WordFlags::empty() }]
///         } else {
///             Vec::new()
///         };
///         Ok(WordIter::from_vec(words))
///     }
///     fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
///         Err(ImeError::Unsupported)
///     }
///     fn fallback_single(&self, _syl: ime_types::SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
///         Ok(WordIter::from_vec(Vec::new()))
///     }
/// }
/// impl ime_types::UserFreqSource for One {
///     fn freq(&self, _key: &str) -> u32 { 0 }
///     fn record(&self, _key: &str, _weight_hint: u16) {}
///     fn is_user_word(&self, _key: &str) -> bool { false }
/// }
///
/// let mut dag = SyllableDag::new();
/// assert!(dag.build("nihao").is_ok());
/// let mut readings = Readings::new();
/// let mut keys = Vec::new();
/// let lattice = build_lattice(&dag, &One, &One, LatticeOptions {
///     fallback_single: true,
///     flags: DecodeFlags::empty(),
///     readings: &mut readings,
///     keys: &mut keys,
///     english: None,
/// });
/// let edges = lattice.edges_from(0);
/// assert_eq!(edges.len(), 1);
/// assert_eq!(edges[0].word.text, "你好");
/// assert_eq!(edges[0].syllables, 2);
/// // An edge the exact walk found carries no penalty.
/// assert_eq!(edges[0].penalty_q8, 0);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Lattice<'dict> {
    /// Every edge, ordered by the node they leave.
    pub(super) edges: Vec<LatticeEdge<'dict>>,
    /// `starts[i]` is the first edge of node `i`; the entry after it is the end,
    /// so the table has one entry per node plus a sentinel.
    pub(super) starts: SmallVec<[u32; MAX_LATTICE_STARTS]>,
    /// Whether the dictionary refused a lookup, which makes the lattice an
    /// incomplete picture of what it holds.
    pub(super) lookup_failed: bool,
    /// Whether the abbreviation enumeration was cut short by its own cap, which makes
    /// the lattice miss the vaguest readings of the input.
    pub(super) abbrev_truncated: bool,
    /// The whole keys of the widened words, as `(edge index, arena start, arena len)`.
    ///
    /// The span-local range an edge's own `key` field names is what the coining
    /// pairs read, and it is only the word's own slice of the key -- the spans a
    /// lookup missed never entered the arena at all. The hint channel needs the
    /// whole spelling the word was looked up under, so a widened lookup records it
    /// here, once per edge the lookup produced, and the sweep reads it by the edge
    /// it walks. A build with no widened word leaves the vector empty, which is no
    /// allocation.
    pub(super) widened_keys: Vec<(u32, u32, u16)>,
}

impl<'dict> Lattice<'dict> {
    /// Creates an empty lattice with room for the edges of a graph of `nodes` nodes.
    ///
    /// The edge vector is what one decode allocates, so it is sized once rather than grown:
    /// that is the difference between a single allocation and the several a doubling growth
    /// takes for a twelve-syllable input. The estimate is what the walk produces per node --
    /// [`WORDS_PER_KEY`] for the node's one-syllable spans, [`FALLBACK_SINGLES`] when the
    /// fallback is on, and [`MAX_WORD_SYLLABLES`]` - 1` more keys for the longer spans a
    /// chain of syllables spells out of the same node. A dictionary richer than that, or a
    /// graph that branches harder than the margin, grows the vector once more rather than
    /// being refused.
    ///
    /// # Panics
    ///
    /// Never: a node count past [`MAX_LATTICE_NODES`] is cut to it.
    pub fn with_capacity(nodes: usize, fallback_single: bool) -> Self {
        let mut per_node = WORDS_PER_KEY + usize::from(MAX_WORD_SYLLABLES) - 1;
        if fallback_single {
            per_node += FALLBACK_SINGLES;
        }
        let nodes = nodes.min(MAX_LATTICE_NODES);
        Self {
            edges: Vec::with_capacity(nodes.saturating_mul(per_node)),
            starts: SmallVec::new(),
            lookup_failed: false,
            abbrev_truncated: false,
            widened_keys: Vec::new(),
        }
    }

    /// Returns the edges leaving `node`, in the order the walk built them.
    ///
    /// Answers with an empty slice for a node index outside the graph rather than
    /// panicking, so a walker cannot be tripped up by a degenerate lattice.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn edges_from(&self, node: usize) -> &[LatticeEdge<'dict>] {
        let (first, last) = self.edge_range(node);
        self.edges.get(first..last).unwrap_or(&[])
    }

    /// Returns the half-open range of edge indices `node` owns.
    ///
    /// # Panics
    ///
    /// Never: a node outside the table answers with an empty range.
    pub fn edge_range(&self, node: usize) -> (usize, usize) {
        let total = u32::try_from(self.edges.len()).unwrap_or(u32::MAX);
        let first = self.starts.get(node).copied().unwrap_or(total);
        let last = self.starts.get(node + 1).copied().unwrap_or(total);
        let first = usize::try_from(first.min(total)).unwrap_or(0);
        let last = usize::try_from(last.min(total)).unwrap_or(0);
        (first.min(last), last)
    }

    /// Returns one edge by its index in the lattice.
    ///
    /// # Panics
    ///
    /// Never: an index past the end answers `None`.
    pub fn edge_at(&self, index: usize) -> Option<&LatticeEdge<'dict>> {
        self.edges.get(index)
    }

    /// Returns how many edges the lattice holds.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Returns whether the lattice holds no word at all, which is the state an
    /// empty or unusable dictionary leaves it in.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Returns how many nodes the lattice describes, the terminal node included.
    pub fn node_count(&self) -> usize {
        self.starts.len()
    }

    /// Returns whether the dictionary refused a lookup while the lattice was
    /// built, so the caller can report a degraded result instead of an
    /// inexplicably short candidate list.
    pub fn lookup_failed(&self) -> bool {
        self.lookup_failed
    }

    /// Returns whether the abbreviation walk left readings of the input out of the
    /// lattice.
    ///
    /// The signal behind the `decode/abbrev-truncated` diagnostic: an input with more
    /// readings than the enumeration cap holds is still decoded, with the most specific
    /// readings the cap left room for, and this is what tells a caller the answer is
    /// built from a subset. It is a fact about the input rather than about the flag set,
    /// so a caller that logs it once has to remember that it already did. Always `false`
    /// while [`DecodeFlags::ABBREV`] is clear, because the walk is never entered.
    pub fn abbrev_truncated(&self) -> bool {
        self.abbrev_truncated
    }

    /// Returns the arena range of the whole key `edge`'s word was looked up under,
    /// when that word is one the exact walk did not produce.
    ///
    /// The answer is a range into the build's key arena, which the caller holds and
    /// the sweep reads the spelling out of. An edge the exact walk produced -- the
    /// steady state -- answers `None`, and so does an index the lattice does not
    /// hold.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn full_key(&self, edge: u32) -> Option<(u32, u16)> {
        let found = self
            .widened_keys
            .iter()
            .find(|(held, _, _)| *held == edge)?;
        Some((found.1, found.2))
    }
}
