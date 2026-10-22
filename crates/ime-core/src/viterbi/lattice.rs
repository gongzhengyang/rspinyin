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

use crate::segment::syllable;
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

/// The lattice graph the build produces: the word edges of one input, grouped by
/// the node they leave.
///
/// The type lives in `lattice/graph.rs` because it is the data the build fills and
/// the sweep walks, while this file carries the walk that fills it.
mod graph;

pub use graph::{Lattice, LatticeEdge};

/// The switches one lattice build reads and the buffer it reuses.
///
/// The type lives in `lattice/options.rs` because it is plain data -- the knobs and the
/// reusable buffers a build takes -- while this file carries the build itself.
mod options;

pub use options::LatticeOptions;

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
    options: LatticeOptions<'_, 'dict>,
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
    options: LatticeOptions<'_, 'dict>,
) {
    lattice.edges.clear();
    lattice.starts.clear();
    lattice.lookup_failed = false;
    lattice.abbrev_truncated = false;
    options.keys.clear();
    let LatticeOptions {
        fallback_single,
        flags,
        readings,
        keys,
        english,
    } = options;
    let mut builder = Builder {
        lexicon,
        user,
        fallback_single,
        english,
        lattice,
        key: [0; MAX_KEY_BYTES],
        key_len: 0,
        singles: SmallVec::new(),
        stack: SmallVec::new(),
        flags,
        readings,
        keys,
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
    if builder.english.is_some() {
        builder.english_span(dag);
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
    /// The heaviest penalty any edge of the partial path carries.
    ///
    /// A word that covers several spans is one lattice edge, and its penalty has
    /// to be the reading's, not its last syllable's: a corrected or fuzzy reading
    /// whose widened span sits inside the word -- `wan` in `wan'ba` -- would
    /// otherwise lose the penalty the graph charged it and rank as if the input
    /// spelled it. The maximum is the fold, because the penalties are a ladder:
    /// an exact span carries zero, and every widened span carries more than it.
    penalty_q8: i32,
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
    /// The span's own penalty, which the fallback edge carries: zero for an exact
    /// span, the fuzzy penalty for a variant.
    penalty_q8: i32,
}

/// The walk's state: the dictionary, the buffers it reuses for every span, and
/// the lattice being filled.
///
/// The key and the two stacks are held across the whole build rather than
/// allocated per span. The key is a fixed buffer rather than a `String` on
/// purpose: the walk runs on the decode path, and the allocation budget
/// (`BUDGET-MEM-04`) counts what that path asks the allocator for, so the scratch
/// the key needs lives in this struct on the stack. The lattice is borrowed rather
/// than owned, which is what lets a caller build into a lattice it keeps. The
/// abbreviation walk's own state -- the switches, its enumeration buffer and the
/// reachability of the nodes -- is held here as well, because it runs node by node
/// inside the same pass.
struct Builder<'a, 'lattice, 'buf> {
    lexicon: &'a dyn Lexicon,
    user: &'a dyn UserFreqSource,
    fallback_single: bool,
    /// The English lexicon the mixed-English pass reads, or `None` when the
    /// request did not ask for it. Never queried for an input a pinyin reading
    /// cuts.
    english: Option<&'a dyn Lexicon>,
    lattice: &'lattice mut Lattice<'a>,
    /// Key under construction, reused by every span the walk spells.
    ///
    /// Only `[..key_len]` is meaningful; the bytes past it are stale and are
    /// overwritten before they are ever read. The abbreviation query is not spelled
    /// here: it lives in the caller's readings buffer, which keeps its allocation the
    /// way this array keeps its bytes.
    key: [u8; MAX_KEY_BYTES],
    /// How much of `key` the spans spelled so far. Tracked beside the buffer because
    /// a fixed buffer cannot answer `len` the way a `String` does.
    key_len: usize,
    /// The one-syllable spans of the node being walked.
    singles: SmallVec<[Single; SINGLES_INLINE]>,
    /// Spans still to be spelled, in the order the graph produced them.
    stack: SmallVec<[Frame; SINGLES_INLINE]>,
    /// Switches of the request being decoded; `ABBREV` gates the abbreviation walk.
    flags: DecodeFlags,
    /// The enumeration buffer the abbreviation walk hands to `readings_into`.
    readings: &'buf mut Readings,
    /// The key arena the span and fallback edges name ranges into; lent by the options
    /// for the same reason the readings buffer is.
    keys: &'buf mut Vec<u8>,
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
            penalty_q8: 0,
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
        // The key is spelled from the edge's syllable, not from the input's bytes: a
        // fuzzy variant covers the bytes the input spells but names the syllable the
        // dictionary stores its words under, and the key is what the dictionary is
        // asked for. For an exact edge the two are the same text, because the graph
        // admits a syllable only when its spelling is the input's.
        let text = syllable::syllable_at(edge.syllable)
            .unwrap_or_else(|| dag.syllable_text(frame.node, edge));
        if text.is_empty() {
            return;
        }
        // The word this span may complete carries the heaviest penalty of the path
        // it covers, not the last span's: the reading widened in the middle is as
        // widened as one widened at the end.
        let penalty_q8 = frame.penalty_q8.max(edge.penalty_q8);
        self.key_len = frame.key_len;
        if frame.syllables > 0 {
            self.key[self.key_len] = b'\'';
            self.key_len += 1;
        }
        let text = text.as_bytes();
        debug_assert!(
            self.key_len + text.len() <= MAX_KEY_BYTES,
            "a key longer than the bound the buffer is sized for: one of the walk's own \
             bounds is broken"
        );
        self.key[self.key_len..self.key_len + text.len()].copy_from_slice(text);
        self.key_len += text.len();
        let key_len = self.key_len;
        let syllables = frame.syllables.saturating_add(1);
        let covered = self.words(edge, penalty_q8, frame.key_len, key_len, syllables);
        if frame.syllables == 0 {
            self.singles.push(Single {
                end: edge.end,
                syllable: edge.syllable,
                covered,
                penalty_q8: edge.penalty_q8,
            });
        }
        // A longer span is still worth spelling even when this one has no word:
        // `zhong` may be missing from the dictionary while `zhong'guo` is in it.
        if syllables < MAX_WORD_SYLLABLES {
            self.stack.push(Frame {
                node: edge.end,
                syllables,
                key_len,
                penalty_q8,
            });
        }
    }

    /// Records the words the key of the last span spells, and answers whether the
    /// dictionary had any.
    ///
    /// `key_base` is where the span's own key starts in the walk's buffer: the bytes
    /// `[key_base..key_len]` are the canonical spelling of this span alone, separators
    /// included, and they are copied into the lattice's arena once per span so a path
    /// reader after the build can spell the same key again.
    fn words(
        &mut self,
        edge: DagEdge,
        penalty_q8: i32,
        key_base: usize,
        key_len: usize,
        syllables: u8,
    ) -> bool {
        // The reference is copied out first: the dictionary outlives this builder,
        // so the words it hands back stay valid while the lattice is filled.
        let lexicon = self.lexicon;
        // The key is spelled from syllable text, which is UTF-8 by construction, so
        // the conversion cannot fail; the branch exists because the standard library
        // cannot know that.
        let Ok(key) = std::str::from_utf8(&self.key[..key_len]) else {
            self.lattice.lookup_failed = true;
            return false;
        };
        let Ok(words) = lexicon.lookup(key) else {
            self.lattice.lookup_failed = true;
            return false;
        };
        // The span's own key enters the arena once, before the edges that name it: the
        // edges carry ranges, and the bytes are the walk's own spelling of the key --
        // the one a later decode will look the same word up under.
        let key_start = self.keys.len();
        self.keys.extend_from_slice(&self.key[key_base..key_len]);
        let key_range = (
            u32::try_from(key_start).unwrap_or(u32::MAX),
            u16::try_from(key_len.saturating_sub(key_base)).unwrap_or(u16::MAX),
        );
        let mut covered = false;
        let first_edge = self.lattice.edges.len();
        for word in words.take(WORDS_PER_KEY) {
            covered = true;
            let source = self.source_of(word);
            self.lattice.edges.push(LatticeEdge {
                end: edge.end,
                syllables,
                characters: chars::count_characters(word.text),
                source,
                word,
                key: key_range,
                // The heaviest penalty of the spans the word covers, which the
                // walk folded into `penalty_q8`: zero for a reading the input
                // spells, the widened span's penalty for one it does not -- at
                // whatever position of the word the widened span sits.
                penalty_q8,
            });
        }
        if covered {
            self.reach(edge.end);
            // A word the exact walk did not produce is annotated with the standard
            // reading it was found under, and the standard reading is the whole key:
            // the span-local range above carries only this span's slice of it, and
            // the spans a lookup missed never entered the arena at all. The full
            // spelling is still in the walk's own buffer, so it is recorded here,
            // once per edge the lookup produced, where the sweep can read it by the
            // edge it walks.
            if crate::fuzzy::is_widened(penalty_q8) {
                let start = u32::try_from(self.keys.len()).unwrap_or(u32::MAX);
                self.keys.extend_from_slice(&self.key[..key_len]);
                let range = (start, u16::try_from(key_len).unwrap_or(u16::MAX));
                for index in first_edge..self.lattice.edges.len() {
                    if let Ok(index) = u32::try_from(index) {
                        self.lattice.widened_keys.push((index, range.0, range.1));
                    }
                }
            }
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
            // The fallback edge's key is the one syllable's own spelling, which enters the
            // arena once per fallback span the same way a span key does.
            let spelling = syllable::syllable_at(single.syllable).unwrap_or_default();
            let key_start = self.keys.len();
            self.keys.extend_from_slice(spelling.as_bytes());
            let key_range = (
                u32::try_from(key_start).unwrap_or(u32::MAX),
                u16::try_from(spelling.len()).unwrap_or(u16::MAX),
            );
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
                            key: key_range,
                            penalty_q8: single.penalty_q8,
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

/// The mixed-English pass: the whole-input edge an input no pinyin walk cuts is
/// offered, when the mixed-English switch is on.
///
/// It lives in `lattice/english.rs` beside the abbreviation walk's own file: both are
/// passes over the finished span walk rather than part of it, and both are gated so
/// that a request that does not ask for them never pays for them.
mod english;

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
