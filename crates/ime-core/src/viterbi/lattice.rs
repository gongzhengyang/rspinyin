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
//! # Borrowing
//!
//! An edge holds the dictionary's own [`WordRef`]: the word text points straight
//! into the mapped string pool and is never copied here.

use ime_types::{CandidateSource, Lexicon, SyllableId, UserFreqSource, WordFlags, WordRef};
use smallvec::SmallVec;

use crate::segment::{DagEdge, MAX_NODES, MAX_SYLLABLE_LEN, SyllableDag};

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
}

/// Every word edge of one input, grouped by the node they leave.
///
/// # Examples
///
/// ```
/// use ime_core::segment::SyllableDag;
/// use ime_core::viterbi::build_lattice;
/// use ime_types::{ImeError, Lexicon, WordFlags, WordIter, WordRef};
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
/// let lattice = build_lattice(&dag, &One, &One, true);
/// let edges = lattice.edges_from(0);
/// assert_eq!(edges.len(), 1);
/// assert_eq!(edges[0].word.text, "你好");
/// assert_eq!(edges[0].syllables, 2);
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
}

impl<'dict> Lattice<'dict> {
    /// Creates an empty lattice with room for the edges of a graph of `nodes` nodes.
    ///
    /// The edge vector is what one decode allocates, so it is sized once rather than grown:
    /// that is the difference between a single allocation and the eight a doubling growth
    /// takes for a twelve-syllable input. The estimate is the average the walk produces --
    /// every node contributes at most [`WORDS_PER_KEY`] edges for its one-syllable spans,
    /// plus the single-character fallbacks when they are on -- so a dictionary that is richer
    /// than that grows the vector once more rather than being refused.
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
}

/// Builds the lattice of `dag`: every word stored under the key of every span of
/// every reading of the input.
///
/// `fallback_single` decides whether a one-syllable span the dictionary has no
/// word for is filled in with single-character candidates. Turning it off leaves
/// the lattice without a path for such an input, which the decoder answers with a
/// pass-through candidate.
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
    fallback_single: bool,
) -> Lattice<'dict> {
    let mut lattice = Lattice::with_capacity(usize::from(dag.len()) + 1, fallback_single);
    build_lattice_into(&mut lattice, dag, lexicon, user, fallback_single);
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
    fallback_single: bool,
) {
    lattice.edges.clear();
    lattice.starts.clear();
    lattice.lookup_failed = false;
    let mut builder = Builder {
        lexicon,
        user,
        fallback_single,
        lattice,
        key: String::with_capacity(MAX_KEY_BYTES),
        singles: SmallVec::new(),
        stack: SmallVec::new(),
    };
    let nodes = usize::from(dag.len()) + 1;
    for node in 0..nodes {
        let first = u32::try_from(builder.lattice.edges.len()).unwrap_or(u32::MAX);
        builder.lattice.starts.push(first);
        builder.node(dag, node);
    }
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
/// lets a caller build into a lattice it keeps.
struct Builder<'a, 'lattice> {
    lexicon: &'a dyn Lexicon,
    user: &'a dyn UserFreqSource,
    fallback_single: bool,
    lattice: &'lattice mut Lattice<'a>,
    /// Key under construction, reused by every span the walk spells.
    key: String,
    /// The one-syllable spans of the node being walked.
    singles: SmallVec<[Single; SINGLES_INLINE]>,
    /// Spans still to be spelled, in the order the graph produced them.
    stack: SmallVec<[Frame; SINGLES_INLINE]>,
}

impl Builder<'_, '_> {
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
                characters: u16::try_from(word.text.chars().count()).unwrap_or(u16::MAX),
                source,
                word,
            });
        }
        covered
    }

    /// Adds the single-character candidates for every one-syllable span the
    /// dictionary had no word of its own for.
    fn fallbacks(&mut self) {
        let lexicon = self.lexicon;
        for single in &self.singles {
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
                            characters: u16::try_from(word.text.chars().count())
                                .unwrap_or(u16::MAX),
                            source,
                            word,
                        });
                    }
                }
                Err(_) => self.lattice.lookup_failed = true,
            }
        }
    }

    /// Answers where one word came from.
    fn source_of(&self, word: WordRef<'_>) -> CandidateSource {
        let coined = word.flags.contains(WordFlags::USER) || self.user.is_user_word(word.text);
        if coined {
            CandidateSource::UserDict
        } else {
            CandidateSource::Dict
        }
    }
}

/// Test doubles shared by the lattice and the decoder tests.
///
/// They live in `lattice/testing.rs` rather than in the decoder's own test module
/// because both suites rank against the same dictionary: one in-memory
/// implementation of the frozen traits keeps the two honest about what a lookup
/// answers.
#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use super::testing::{MockLexicon, NoUser};
    use super::*;

    /// Builds the lattice of `raw` against `lexicon`, with fallbacks on.
    fn lattice_of<'a>(raw: &str, lexicon: &'a MockLexicon) -> Lattice<'a> {
        let mut dag = SyllableDag::new();
        assert!(dag.build(raw).is_ok(), "building {raw:?}");
        build_lattice(&dag, lexicon, &NoUser, true)
    }

    /// Returns the texts of the edges leaving `node`.
    fn texts<'a>(lattice: &'a Lattice<'_>, node: usize) -> Vec<&'a str> {
        lattice
            .edges_from(node)
            .iter()
            .map(|edge| edge.word.text)
            .collect()
    }

    #[test]
    fn test_build_lattice_finds_the_words_of_every_span() {
        let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
        let lattice = lattice_of("nihao", &lexicon);
        assert_eq!(texts(&lattice, 0), vec!["你", "你好"]);
        // `hao` starts at byte 2 of `nihao`, and the terminal node has no edges.
        assert_eq!(texts(&lattice, 2), vec!["好"]);
        assert!(lattice.edges_from(5).is_empty());
        assert!(!lattice.is_empty());
    }

    #[test]
    fn test_build_lattice_reads_every_spelling_of_an_ambiguous_input() {
        // `xian` cuts four ways, and the readings are not interchangeable: the
        // one-syllable reading is 先 and the two-syllable one is 西安. A span is
        // therefore looked up once per spelling. Node indices are byte offsets, so the
        // 西 of `xi` and the 先 of `xian` both leave node 0, while the 安 of `xi'an`
        // starts two bytes in and leaves node 2.
        let lexicon = MockLexicon::with(&[("xian", "先"), ("xi", "西"), ("an", "安")]);
        let lattice = lattice_of("xian", &lexicon);
        assert_eq!(texts(&lattice, 0), vec!["西", "先"]);
        assert_eq!(texts(&lattice, 2), vec!["安"]);
        // Both readings reach the terminal node: 先 in one edge, and 西安 through
        // the `xi` edge followed by the `an` edge out of node 2.
        let direct = lattice
            .edges_from(0)
            .iter()
            .any(|edge| usize::from(edge.end) == 4);
        assert!(direct);
        assert!(!lattice.edges_from(2).is_empty());
    }

    #[test]
    fn test_build_lattice_keeps_only_the_strongest_words_of_a_key() {
        let rows: Vec<(&'static str, &'static str)> = (0..WORDS_PER_KEY + 2)
            .map(|index| {
                (
                    "shi",
                    ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"][index],
                )
            })
            .collect();
        let lexicon = MockLexicon::with(&rows);
        let lattice = lattice_of("shi", &lexicon);
        let kept = lattice.edges_from(0);
        assert_eq!(kept.len(), WORDS_PER_KEY);
        assert_eq!(kept[0].word.text, "一");
        assert_eq!(kept[WORDS_PER_KEY - 1].word.text, "八");
    }

    #[test]
    fn test_build_lattice_stops_spelling_spans_at_the_word_length_limit() {
        // Seven `a`s segment one way only -- a chain of seven syllables -- so the walk
        // can spell a span of any length up to the limit and no longer. The dictionary
        // holds a word for the six-syllable span and one for the seven-syllable span.
        let rows = [("a'a'a'a'a'a", "六"), ("a'a'a'a'a'a'a", "七")];
        let lexicon = MockLexicon::with(&rows);
        let lattice = lattice_of("aaaaaaa", &lexicon);
        // Two edges only: the six-syllable key is spelled from node 0 and from node 1,
        // and the seven-syllable key is never looked up, so no edge carries 七.
        assert_eq!(lattice.len(), 2);
        assert_eq!(texts(&lattice, 0), vec!["六"]);
        assert_eq!(texts(&lattice, 1), vec!["六"]);
        assert_eq!(lattice.edges_from(0)[0].syllables, MAX_WORD_SYLLABLES);
    }

    #[test]
    fn test_build_lattice_falls_back_only_where_no_word_covers_the_syllable() {
        let lexicon = MockLexicon::with(&[("ni", "你")])
            .single("ni", &["伱"])
            .single("hao", &["好", "号", "浩", "郝"]);
        let lattice = lattice_of("nihao", &lexicon);
        // `ni` has a word of its own, so the fallback list is not used for it;
        // `hao` has none, so it is, and only its first three candidates.
        assert_eq!(texts(&lattice, 0), vec!["你"]);
        assert_eq!(texts(&lattice, 2), vec!["好", "号", "浩"]);
    }

    #[test]
    fn test_build_lattice_without_the_fallback_switch_leaves_spans_uncovered() {
        let lexicon = MockLexicon::with(&[("ni", "你")]).single("hao", &["好"]);
        let mut dag = SyllableDag::new();
        assert!(dag.build("nihao").is_ok());
        let lattice = build_lattice(&dag, &lexicon, &NoUser, false);
        assert_eq!(texts(&lattice, 0), vec!["你"]);
        assert!(lattice.edges_from(2).is_empty());
    }

    #[test]
    fn test_build_lattice_labels_a_user_word() {
        // `coined` names the word, not the lookup key: the frozen `UserFreqSource` is
        // queried with the word text (`Scorer::edge_score` passes `uf.freq(word)`), so a
        // mock that recorded the key here would label nothing.
        let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好")]).coined("好");
        let lattice = lattice_of("nihao", &lexicon);
        assert_eq!(lattice.edges_from(0)[0].source, CandidateSource::Dict);
        assert_eq!(lattice.edges_from(2)[0].source, CandidateSource::UserDict);
    }

    #[test]
    fn test_build_lattice_reports_a_refused_lookup() {
        let lexicon = MockLexicon::with(&[("ni", "你")]).failing("hao");
        let lattice = lattice_of("nihao", &lexicon);
        assert!(lattice.lookup_failed());
        // The failing key simply contributes nothing, and the fallback still
        // leaves a path to the end.
        assert_eq!(texts(&lattice, 0), vec!["你"]);
    }

    #[test]
    fn test_build_lattice_gives_every_node_a_range_and_orders_the_edges() {
        let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "好"), ("ni'hao", "你好")]);
        let lattice = lattice_of("nihao", &lexicon);
        assert_eq!(lattice.node_count(), 6);
        let mut seen = 0usize;
        for node in 0..lattice.node_count() {
            let (first, last) = lattice.edge_range(node);
            assert!(first <= last);
            assert_eq!(last - first, lattice.edges_from(node).len());
            for index in first..last {
                assert!(lattice.edge_at(index).is_some());
            }
            seen += last - first;
        }
        assert_eq!(seen, lattice.len());
        // A node past the end, and an edge index past the end, are answered
        // rather than panicking.
        assert!(lattice.edges_from(lattice.node_count()).is_empty());
        assert!(lattice.edge_at(lattice.len()).is_none());
    }

    #[test]
    fn test_build_lattice_of_an_empty_graph_has_no_edges() {
        let lexicon = MockLexicon::with(&[]);
        let dag = SyllableDag::new();
        let lattice = build_lattice(&dag, &lexicon, &NoUser, true);
        assert!(lattice.is_empty());
        assert_eq!(lattice.len(), 0);
        assert_eq!(lattice.node_count(), 1);
        assert!(lattice.edges_from(0).is_empty());
    }

    #[test]
    fn test_build_lattice_counts_characters_and_syllables_of_each_edge() {
        let lexicon = MockLexicon::with(&[("zhong", "中"), ("zhong'guo", "中国")]);
        let lattice = lattice_of("zhongguo", &lexicon);
        let edges = lattice.edges_from(0);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].word.text, "中");
        assert_eq!(edges[0].syllables, 1);
        assert_eq!(edges[0].characters, 1);
        assert_eq!(edges[1].word.text, "中国");
        assert_eq!(edges[1].syllables, 2);
        assert_eq!(edges[1].characters, 2);
    }

    #[test]
    fn test_lattice_with_capacity_sizes_the_edge_vector_for_the_graph() {
        let lattice: Lattice<'_> = Lattice::with_capacity(4, true);
        assert!(lattice.is_empty());
        assert_eq!(lattice.node_count(), 0);
        assert_eq!(
            lattice.edges.capacity(),
            4 * (WORDS_PER_KEY + FALLBACK_SINGLES),
            "the fallbacks are part of the estimate when they are on"
        );
        // A node count past the ceiling is cut to it, so a hand-built configuration cannot
        // make the lattice allocate without bound.
        let bounded: Lattice<'_> = Lattice::with_capacity(usize::MAX, false);
        assert_eq!(bounded.edges.capacity(), MAX_LATTICE_NODES * WORDS_PER_KEY);
    }

    #[test]
    fn test_build_lattice_into_replaces_the_contents_and_keeps_the_buffer() {
        let lexicon = MockLexicon::with(&[
            ("ni", "你"),
            ("hao", "好"),
            ("ni'hao", "你好"),
            ("zhong", "中"),
        ]);
        let mut dag = SyllableDag::new();
        assert!(dag.build("nihao").is_ok());
        let mut lattice = Lattice::with_capacity(usize::from(dag.len()) + 1, true);
        build_lattice_into(&mut lattice, &dag, &lexicon, &NoUser, true);
        assert_eq!(texts(&lattice, 0), vec!["你", "你好"]);
        let grown = lattice.edges.capacity();
        // A second build into the same lattice describes the second input and leaves the
        // buffer the first one grew in place.
        assert!(dag.build("zhongguo").is_ok());
        build_lattice_into(&mut lattice, &dag, &lexicon, &NoUser, true);
        assert_eq!(texts(&lattice, 0), vec!["中"]);
        assert_eq!(lattice.node_count(), usize::from(dag.len()) + 1);
        assert!(
            lattice.edges.capacity() >= grown,
            "the edge vector keeps its allocation"
        );
        assert!(!lattice.lookup_failed());
    }

    #[test]
    fn test_build_lattice_into_clears_a_refusal_of_the_previous_build() {
        let lexicon = MockLexicon::with(&[("ni", "你")]).failing("hao");
        let mut dag = SyllableDag::new();
        assert!(dag.build("nihao").is_ok());
        let mut lattice = Lattice::default();
        build_lattice_into(&mut lattice, &dag, &lexicon, &NoUser, true);
        assert!(lattice.lookup_failed(), "the second syllable is refused");
        let mut whole = SyllableDag::new();
        assert!(whole.build("ni").is_ok());
        build_lattice_into(&mut lattice, &whole, &lexicon, &NoUser, true);
        assert!(
            !lattice.lookup_failed(),
            "a build that reads everything clears the previous refusal"
        );
        assert_eq!(texts(&lattice, 0), vec!["你"]);
    }
}
