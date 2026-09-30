//! The word lattice: every word the dictionary offers for every span of one
//! input's syllable graph.
//!
//! Responsibility: turn the segmentation graph and the dictionary into the graph
//! the K-best sweep walks. Its nodes are byte offsets into the normalized input,
//! and an edge is one word covering one span of syllables. Two rules shape it: a
//! span is spelled by joining its syllables with `'`, which is the form the
//! dictionary is keyed by, and every one-syllable span the dictionary has no word
//! for gets a single-character fallback, so that any input with a legal
//! segmentation still has a path to the end.
//!
//! Boundaries: this module reads the dictionary and produces no candidate. It
//! never scores anything -- the score of an edge depends on the word in front of
//! it, which only the sweep knows -- and it never decides how many candidates
//! survive. It is pure: the graph comes in, the lattice comes out, and no file,
//! clock or global state is touched.
//!
//! # Spans, not one fixed cut
//!
//! An input can be segmentable in more than one way and the readings are not
//! interchangeable: `xian` is both `xian` (先) and `xi'an` (西安). The lattice is
//! therefore built by walking the graph rather than by cutting the input once, and
//! a span is looked up once per way of spelling it. The walk is bounded on both
//! axes -- a word covers at most [`MAX_WORD_SYLLABLES`] syllables, and a node
//! looks up a bounded number of keys -- because the number of readings of a
//! repetitive input such as `anananan...` grows exponentially otherwise. Which
//! keys a bounded walk drops follows from the graph's own edge order, so it is
//! the same set on every run.
//!
//! # The abbreviation walk
//!
//! Beside the span walk, an input whose syllables are typed as initials reaches words
//! no key spells out: `nh` means `ni'hao` with letters left out, and the dictionary is
//! asked for it through [`Lexicon::prefix`]. That walk lives in `abbrev.rs`, runs only
//! for a node a path reaches and only while [`DecodeFlags::ABBREV`] is set, and marks
//! every edge it adds with [`LatticeEdge::penalty_q8`] so that a full spelling outranks
//! an abbreviation at equal dictionary weight. Its cost is bounded on four axes -- the
//! readings of one node, the syllables of one reading, the queries of one node, and the
//! edges every extension walk may add together ([`MAX_TOTAL_EDGES`]) -- which is what
//! keeps the lattice of an ambiguous input inside the decode budget.
//!
//! # Borrowing
//!
//! An edge holds the dictionary's own [`WordRef`]: the word text points straight
//! into the mapped string pool and is never copied here.

use ime_types::{
    CandidateSource, DecodeFlags, Lexicon, SyllableId, UserFreqSource, WordFlags, WordRef,
};
use smallvec::SmallVec;

use crate::segment::{DagEdge, MAX_NODES, MAX_SYLLABLE_LEN, Readings, SyllableDag};

/// Longest word span, in syllables, the lattice considers.
///
/// Six covers the four-character idioms and the longer proper names a dictionary
/// carries, and it is the point past which the walk's cost grows faster than the
/// chance of a hit: every extra syllable multiplies the number of readings a span
/// has to be spelled out in.
pub const MAX_WORD_SYLLABLES: u8 = 6;

/// Words kept per key: the strongest ones the dictionary returns.
///
/// A key such as `shi` holds dozens of words, and every one of them costs an edge
/// in every merge that reaches the node. Eight keeps the head of the list, which
/// is where the words a user means live.
pub const WORDS_PER_KEY: usize = 8;

/// Single-character candidates a syllable with no word of its own falls back to.
pub const FALLBACK_SINGLES: usize = 3;

/// Keys one node looks up before the walk stops.
///
/// The bound is what makes the walk's cost independent of how ambiguous the input
/// is; it is far above what a real input needs, where a span usually has one
/// reading and a second one only where the graph branches.
const MAX_KEYS_PER_NODE: usize = 24;

/// Most edges the extension walks may add to one lattice.
///
/// The exact walk is never cut -- a lattice missing a word the dictionary holds is a
/// wrong answer, while a slow one is only slow -- so this is the ceiling the walks
/// beside it fill up to, and they share it in the order the design fixes: the
/// abbreviation walk takes what it needs first and the fuzzy walk takes what is left,
/// so turning both on cannot make a lattice grow without bound. The exact walk's own
/// bound is [`MAX_KEYS_PER_NODE`] keys of [`WORDS_PER_KEY`] words per node, which is
/// far past what a real dictionary offers; this ceiling is the same "strongest eight"
/// budget spent once per node of the widest graph a request can produce, and the sweep
/// scores every one of those edges once per path that reaches its node, so it is a few
/// hundred microseconds at the widest beam.
pub const MAX_TOTAL_EDGES: usize = MAX_LATTICE_NODES * WORDS_PER_KEY;

/// Most nodes one lattice has: one per byte of the normalized input plus the
/// terminal node.
pub const MAX_LATTICE_NODES: usize = MAX_NODES;

/// Entries in the table of per-node edge ranges: one start per node plus the
/// sentinel end of the last one.
const MAX_LATTICE_STARTS: usize = MAX_LATTICE_NODES + 1;

/// Bytes the key of the longest word can occupy: one separator before every
/// syllable but the first, and the widest syllable spelling.
const MAX_KEY_BYTES: usize = MAX_WORD_SYLLABLES as usize * (MAX_SYLLABLE_LEN + 1);

/// Inline capacity of the per-node span list, matching the widest node of the
/// graph (an `a`-initial one, which can start at most five syllables).
const SINGLES_INLINE: usize = 8;

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
    /// Score penalty the edge carries, in the Q16.16 unit the sweep ranks with.
    ///
    /// Zero for an edge the exact walk produced, and the penalty of the extension that
    /// found it -- [`ABBREV_PENALTY_Q8`](crate::segment::abbrev::ABBREV_PENALTY_Q8) for an
    /// abbreviated one -- so that a full spelling outranks an abbreviation at equal
    /// dictionary weight. The lattice records the penalty rather than applying it: the
    /// score of an edge is the sweep's to compute, and this is the one term of that score
    /// the lattice knows and the sweep cannot derive from the word and its predecessor.
    /// The order the edges sit in is a second, weaker guarantee -- the exact walk's edges
    /// come before the abbreviation walk's, so a tie falls the right way even on a sweep
    /// that ignores this field -- and the penalty is what makes the outcome a matter of
    /// the score rather than of the order.
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
/// let lattice = build_lattice(&dag, &One, &One, LatticeOptions {
///     fallback_single: true,
///     flags: DecodeFlags::empty(),
///     readings: &mut readings,
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
    edges: Vec<LatticeEdge<'dict>>,
    /// `starts[i]` is the first edge of node `i`; the entry after it is the end,
    /// so the table has one entry per node plus a sentinel.
    starts: SmallVec<[u32; MAX_LATTICE_STARTS]>,
    /// Whether the dictionary refused a lookup, which makes the lattice an
    /// incomplete picture of what it holds.
    lookup_failed: bool,
    /// Whether the abbreviation enumeration was cut short by its own cap, which makes
    /// the lattice miss the vaguest readings of the input.
    abbrev_truncated: bool,
}

impl<'dict> Lattice<'dict> {
    /// Creates an empty lattice with room for the edges of a graph of `nodes` nodes.
    ///
    /// The edge vector is what one decode allocates, so it is sized once rather than grown:
    /// that is the difference between a single allocation and the eight a doubling growth
    /// takes for a twelve-syllable input. The estimate is the average the walk produces --
    /// every node contributes at most [`WORDS_PER_KEY`] edges for its one-syllable spans,
    /// plus the single-character fallbacks when they are on -- so a dictionary that is richer
    /// than that, or a build the abbreviation walk adds edges to, grows the vector once more
    /// rather than being refused.
    ///
    /// # Panics
    ///
    /// Never: a node count past [`MAX_LATTICE_NODES`] is cut to it.
    pub fn with_capacity(nodes: usize, fallback_single: bool) -> Self {
        let per_node = WORDS_PER_KEY + if fallback_single { FALLBACK_SINGLES } else { 0 };
        let nodes = nodes.min(MAX_LATTICE_NODES);
        Self {
            edges: Vec::with_capacity(nodes.saturating_mul(per_node)),
            starts: SmallVec::new(),
            lookup_failed: false,
            abbrev_truncated: false,
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
}

/// The switches one lattice build reads and the buffer it reuses.
///
/// Grouped rather than passed one by one because the three travel together: a build is
/// asked for the same thing every time and differs only in these, and the buffer is the
/// caller's, so that a decode that runs once per keystroke hands the same allocation
/// back instead of building one per node.
#[derive(Debug)]
pub struct LatticeOptions<'a> {
    /// Whether a one-syllable span the dictionary has no word for is filled in with
    /// single-character candidates. Turning it off leaves the lattice without a path
    /// for such an input, which the decoder answers with a pass-through candidate.
    pub fallback_single: bool,
    /// The switches of the request being decoded.
    ///
    /// [`DecodeFlags::ABBREV`] is the whole abbreviation gate: with the bit clear the
    /// abbreviation walk is never entered, no reading is enumerated and no prefix query
    /// is made, so a build costs what it cost before abbreviation existed.
    pub flags: DecodeFlags,
    /// The buffer the abbreviation enumeration reuses.
    ///
    /// It is cleared and filled once per node the walk reaches, and the syllable vectors
    /// it holds are handed back to the next call, so a decode that keeps one of these
    /// stops allocating after the first input of a given shape.
    pub readings: &'a mut Readings,
}

/// Builds the lattice of `dag`: every word stored under the key of every span of
/// every reading of the input.
///
/// The dictionary's user-word flags and `user`'s own record of coined words are
/// both read, because the two can disagree: a word can sit in the base dictionary
/// and have been coined by the user as well.
///
/// # Panics
///
/// Never: the walk is bounded, and every index into the graph and the lattice is
/// taken with a checked lookup.
pub fn build_lattice<'dict>(
    dag: &SyllableDag,
    lexicon: &'dict dyn Lexicon,
    user: &'dict (dyn UserFreqSource + 'dict),
    options: LatticeOptions<'_>,
) -> Lattice<'dict> {
    let mut lattice = Lattice::with_capacity(usize::from(dag.len()) + 1, options.fallback_single);
    build_lattice_into(&mut lattice, dag, lexicon, user, options);
    lattice
}

/// Builds the lattice of `dag` into `lattice`, reusing the buffers it already holds.
///
/// Same walk as [`build_lattice`], which is this function wrapped around a lattice of its
/// own. A caller that builds one lattice per decode hands in the one it holds, so the edge
/// vector and the per-node table keep their allocation; the previous contents are cleared
/// first, and a failure leaves nothing of them behind either.
///
/// # Panics
///
/// Never: the walk is bounded, and every index into the graph and the lattice is
/// taken with a checked lookup.
pub fn build_lattice_into<'dict>(
    lattice: &mut Lattice<'dict>,
    dag: &SyllableDag,
    lexicon: &'dict dyn Lexicon,
    user: &'dict (dyn UserFreqSource + 'dict),
    options: LatticeOptions<'_>,
) {
    lattice.edges.clear();
    lattice.starts.clear();
    lattice.lookup_failed = false;
    lattice.abbrev_truncated = false;
    let LatticeOptions {
        fallback_single,
        flags,
        readings,
    } = options;
    let mut builder = Builder {
        lexicon,
        user,
        fallback_single,
        lattice,
        key: String::with_capacity(MAX_KEY_BYTES),
        singles: SmallVec::new(),
        stack: SmallVec::new(),
        flags,
        readings,
        live_node: false,
        live: [false; MAX_LATTICE_NODES],
        extension_edges: 0,
        extensions_full: false,
        abbrev_truncated: false,
    };
    let nodes = usize::from(dag.len()) + 1;
    for node in 0..nodes {
        let first = u32::try_from(builder.lattice.edges.len()).unwrap_or(u32::MAX);
        builder.lattice.starts.push(first);
        builder.node(dag, node);
    }
    builder.lattice.abbrev_truncated = builder.abbrev_truncated;
}

/// One key the walk has spelled out, and where it ends.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// Node the partial path ends at.
    node: u16,
    /// Syllables the partial path covers.
    syllables: u8,
    /// Length of the key that spells the partial path.
    key_len: usize,
}

/// One one-syllable span of the node being walked, and whether the dictionary had
/// a word of its own for it.
#[derive(Clone, Copy, Debug)]
struct Single {
    /// Node the syllable reaches.
    end: u16,
    /// The syllable itself, which names the fallback list.
    syllable: SyllableId,
    /// Whether a word edge already covers this span.
    covered: bool,
}

/// The walk's state: the dictionary, the buffers it reuses for every span, and
/// the lattice being filled.
///
/// The key and the two stacks are held across the whole build rather than
/// allocated per span, and the key is sized for the longest word, so the walk
/// never grows a buffer. The lattice is borrowed rather than owned, which is what
/// lets a caller build into a lattice it keeps. The abbreviation walk's own state
/// -- the switches, its enumeration buffer and the reachability of the nodes -- is
/// held here as well, because it runs node by node inside the same pass.
struct Builder<'a, 'lattice, 'buf> {
    lexicon: &'a dyn Lexicon,
    user: &'a dyn UserFreqSource,
    fallback_single: bool,
    lattice: &'lattice mut Lattice<'a>,
    /// Key under construction, reused by every span the walk spells and, once the spans
    /// of a node are done, by the abbreviation query spelled for the same node.
    key: String,
    /// The one-syllable spans of the node being walked.
    singles: SmallVec<[Single; SINGLES_INLINE]>,
    /// Spans still to be spelled, in the order the graph produced them.
    stack: SmallVec<[Frame; SINGLES_INLINE]>,
    /// Switches of the request being decoded; `ABBREV` gates the abbreviation walk.
    flags: DecodeFlags,
    /// The enumeration buffer the abbreviation walk hands to `readings_into`.
    readings: &'buf mut Readings,
    /// Whether the node being walked is one a path reaches.
    live_node: bool,
    /// `live[i]` is set once an edge leaving a reachable node reaches node `i`.
    live: [bool; MAX_LATTICE_NODES],
    /// Edges the extension walks have added, measured against [`MAX_TOTAL_EDGES`].
    extension_edges: usize,
    /// Whether the extension walks have reached their ceiling.
    extensions_full: bool,
    /// Whether the abbreviation enumeration was cut short by its own cap.
    abbrev_truncated: bool,
}

impl Builder<'_, '_, '_> {
    /// Enumerates every word edge that leaves `node`.
    ///
    /// The walk is explicit -- a stack of spans, never recursion -- so a long
    /// input cannot overflow the stack, and it stops after a fixed number of
    /// lookups per node so that an ambiguous input cannot blow up the decode.
    fn node(&mut self, dag: &SyllableDag, node: usize) {
        self.singles.clear();
        self.stack.clear();
        let Ok(node) = u16::try_from(node) else {
            return;
        };
        // Every path starts at node 0, and every other node is walked whether or not a
        // path reaches it -- the exact walk's spans are cheap there, because the graph
        // has no edge to spell. The abbreviation walk is the exception: it reads the
        // input rather than the graph, so a node nothing reaches would make it enumerate
        // readings and query the dictionary for words no candidate can use.
        self.live_node = node == 0 || self.live.get(usize::from(node)).copied().unwrap_or(false);
        self.stack.push(Frame {
            node,
            syllables: 0,
            key_len: 0,
        });
        let mut keys = 0usize;
        'walk: while let Some(frame) = self.stack.pop() {
            if frame.syllables >= MAX_WORD_SYLLABLES {
                continue;
            }
            for edge in dag.edges_from(usize::from(frame.node)) {
                if keys >= MAX_KEYS_PER_NODE {
                    break 'walk;
                }
                keys += 1;
                self.span(dag, frame, *edge);
            }
        }
        if self.fallback_single {
            self.fallbacks();
        }
        self.abbrev(dag, node);
    }

    /// Spells one span and records the words stored under its key.
    fn span(&mut self, dag: &SyllableDag, frame: Frame, edge: DagEdge) {
        let text = dag.syllable_text(frame.node, edge);
        if text.is_empty() {
            return;
        }
        self.key.truncate(frame.key_len);
        if frame.syllables > 0 {
            self.key.push('\'');
        }
        self.key.push_str(text);
        let key_len = self.key.len();
        let syllables = frame.syllables.saturating_add(1);
        let covered = self.words(edge, key_len, syllables);
        if frame.syllables == 0 {
            self.singles.push(Single {
                end: edge.end,
                syllable: edge.syllable,
                covered,
            });
        }
        // A longer span is still worth spelling even when this one has no word:
        // `zhong` may be missing from the dictionary while `zhong'guo` is in it.
        if syllables < MAX_WORD_SYLLABLES {
            self.stack.push(Frame {
                node: edge.end,
                syllables,
                key_len,
            });
        }
    }

    /// Records the words the key of the last span spells, and answers whether the
    /// dictionary had any.
    fn words(&mut self, edge: DagEdge, key_len: usize, syllables: u8) -> bool {
        // The reference is copied out first: the dictionary outlives this builder,
        // so the words it hands back stay valid while the lattice is filled.
        let lexicon = self.lexicon;
        let key = &self.key[..key_len];
        let Ok(words) = lexicon.lookup(key) else {
            self.lattice.lookup_failed = true;
            return false;
        };
        let mut covered = false;
        for word in words.take(WORDS_PER_KEY) {
            covered = true;
            let source = self.source_of(word);
            self.lattice.edges.push(LatticeEdge {
                end: edge.end,
                syllables,
                characters: chars::count_characters(word.text),
                source,
                word,
                penalty_q8: 0,
            });
        }
        if covered {
            self.reach(edge.end);
        }
        covered
    }

    /// Adds the single-character candidates for every one-syllable span the
    /// dictionary had no word of its own for.
    fn fallbacks(&mut self) {
        let lexicon = self.lexicon;
        // The spans are read by index rather than by iterator so that each one is copied
        // out before the edge is pushed: the marking below borrows the builder, and an
        // iterator over `singles` would hold it for the whole loop.
        for index in 0..self.singles.len() {
            let Some(single) = self.singles.get(index).copied() else {
                continue;
            };
            if single.covered {
                continue;
            }
            match lexicon.fallback_single(single.syllable, FALLBACK_SINGLES) {
                Ok(words) => {
                    for word in words.take(FALLBACK_SINGLES) {
                        let source = self.source_of(word);
                        self.lattice.edges.push(LatticeEdge {
                            end: single.end,
                            syllables: 1,
                            characters: chars::count_characters(word.text),
                            source,
                            word,
                            penalty_q8: 0,
                        });
                        self.reach(single.end);
                    }
                }
                Err(_) => self.lattice.lookup_failed = true,
            }
        }
    }

    /// Marks the node an edge reaches as one a path can reach.
    ///
    /// The marking follows the walk: an edge of a node nothing reaches leads nowhere a
    /// candidate could be built from, so it marks nothing either. Node 0 is the root
    /// every path starts from and is reachable by definition.
    fn reach(&mut self, end: u16) {
        if !self.live_node {
            return;
        }
        if let Some(slot) = self.live.get_mut(usize::from(end)) {
            *slot = true;
        }
    }

    /// Answers where one word came from.
    ///
    /// `is_user_word` runs once per edge. It is free today -- the shipped implementation
    /// answers `false` for every word -- but it is a query the user database owns, and
    /// the phase that lets a user coin a word turns it into a store read on the decode
    /// path. Whoever lands that has to keep this call as cheap as the rest of the edge
    /// construction: read a flag the entry already carries, or resolve the answer once
    /// per lookup, rather than let a storage read appear here once per span.
    fn source_of(&self, word: WordRef<'_>) -> CandidateSource {
        let coined = word.flags.contains(WordFlags::USER) || self.user.is_user_word(word.text);
        if coined {
            CandidateSource::UserDict
        } else {
            CandidateSource::Dict
        }
    }
}

/// The character count an edge carries, and why it is a byte scan rather than a decode.
mod chars;

/// The abbreviation walk: the extra edges an input reaches when a syllable may be
/// typed as its initial alone.
///
/// It lives in `lattice/abbrev.rs` because it is the one part of the build that reads
/// the input rather than the graph, and because the walk's own file is at its line
/// budget. The module is private: what a caller sees of it is the edges it adds and
/// the penalty they carry.
mod abbrev;

/// Test doubles shared by the lattice and the decoder tests.
///
/// They live in `lattice/testing.rs` rather than in the decoder's own test module
/// because both suites rank against the same dictionary: one in-memory
/// implementation of the frozen traits keeps the two honest about what a lookup
/// answers.
#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;
