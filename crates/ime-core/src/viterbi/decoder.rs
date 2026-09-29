//! The K-best sweep over the lattice, and the candidates it produces.
//!
//! Responsibility: rank the readings of one input and hand back the ordered
//! candidates the window draws. The sweep walks the lattice's nodes in byte order,
//! keeps the best few paths at every node, and reads the paths that reach the last
//! node back into candidate text. Keeping the best `K` at every node -- not only the
//! single best -- is what makes the second and third readings available at all: the
//! second-best reading of the whole input can share a prefix with the best one and
//! diverge only at the last word, and a node that kept one path would have thrown it
//! away before the divergence was reached.
//!
//! Boundaries: the decoder looks nothing up on its own -- the lattice it walks was
//! built by [`build_lattice`] -- and it never decides how a candidate looks on
//! screen. It fills in the frozen [`DecodeResult`] and stops there. It is a pure
//! function of its arguments: the request, the dictionary, the user's frequencies
//! and the language model, and nothing else. No file, no clock, no environment, no
//! global state, which is what makes the same input rank the same way on every run
//! and in every test.
//!
//! # Determinism
//!
//! The candidate order is part of the contract, so every step that could make it
//! depend on anything but the input is pinned down:
//!
//! - Scores are Q8.8 integers ([`Scorer`]) and the ordering is decided on those
//!   integers; the `f32` on a [`Candidate`] is written last and read by nobody.
//! - Every path carries a monotone generation counter, so two paths that score the
//!   same are ordered by when they were generated rather than by which one a heap
//!   happened to compare first.
//! - A node's beam is sorted as the sweep reaches it, so the order its paths are
//!   expanded in follows from the scores, not from the heap's internal layout.
//! - No `HashMap` is on the path; drafts are deduplicated by a linear scan.
//!
//! # The degraded answer
//!
//! A decode never answers with an empty candidate list. When there is no reading at
//! all -- the input has no segmentation, or no word of the dictionary covers it and
//! the single-character fallback is off -- the answer is one
//! [`CandidateSource::Passthrough`] candidate holding the raw input, with `degraded`
//! set. A window that renders nothing looks to the user like an input method that has
//! stopped responding, which is a worse failure than showing the text they typed.

use core::cmp::Ordering;

use ime_types::{
    Candidate, CandidateSource, DecodeFlags, DecodeRequest, DecodeResult, ImeError, LanguageModel,
    Lexicon, Segment, UserFreqSource,
};

use crate::lm::{Q, ScoreWeights, Scorer};
use crate::segment::{MAX_NODES, SyllableDag};
use crate::viterbi::kbest::TopK;
use crate::viterbi::lattice::{Lattice, build_lattice};

/// Widest beam a configuration may ask for.
pub const MAX_BEAM_K: u16 = 32;

/// Longest candidate list a configuration may ask for.
pub const MAX_CANDIDATES: u16 = 64;

/// Beam width the shipped configuration uses.
pub const DEFAULT_BEAM_K: u16 = 16;

/// Candidate limit the shipped configuration uses: five pages of nine.
pub const DEFAULT_MAX_CANDIDATES: u16 = 45;

/// The Q16.16 unit as a float, for the display-only candidate score.
const Q16_ONE: f32 = (Q * Q) as f32;

/// Marks a path that no edge reached: the empty path every other one starts from.
const NO_EDGE: u32 = u32::MAX;

/// Marks a path with no predecessor.
const NO_SLOT: u16 = u16::MAX;

/// Bytes one word adds to a candidate, used to size its text before it is built.
///
/// A word is at least one CJK character, three bytes in UTF-8, so this is a lower
/// bound and a good enough hint: sizing exactly would need a second pass over the
/// path for the sake of a few bytes.
const BYTES_PER_WORD: usize = 3;

/// The decoder's tunable knobs.
///
/// The five scoring weights are the [`ScoreWeights`] tuple rather than five fields of
/// their own: a second copy of `lambda_uni` and its siblings would be a second place
/// to keep in step with the offline tuner, and the two copies would drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeConfig {
    /// Paths kept at every lattice node; default [`DEFAULT_BEAM_K`].
    ///
    /// Widening it keeps more readings alive and costs time in every merge;
    /// [`MAX_BEAM_K`] is the ceiling. Zero is legal and keeps nothing, which makes
    /// the decode answer with the pass-through candidate.
    pub beam_k: u16,
    /// Most candidates one decode returns; default [`DEFAULT_MAX_CANDIDATES`], the
    /// five pages of nine the window can show; [`MAX_CANDIDATES`] is the ceiling.
    pub max_candidates: u16,
    /// Whether a syllable the dictionary has no word for is covered by
    /// single-character candidates; default `true`. With it off, an input no word of
    /// the dictionary covers has no reading at all and the decode degrades.
    pub fallback_single: bool,
    /// The five weights the edge and path scores are built from.
    pub weights: ScoreWeights,
}

impl Default for DecodeConfig {
    /// The shipped knobs: the default beam and candidate limit, the single-character
    /// fallback on, and the weight tuple the offline tuner walks outwards from.
    fn default() -> Self {
        Self {
            beam_k: DEFAULT_BEAM_K,
            max_candidates: DEFAULT_MAX_CANDIDATES,
            fallback_single: true,
            weights: ScoreWeights::default(),
        }
    }
}

impl DecodeConfig {
    /// Rejects a configuration the decoder cannot honour.
    ///
    /// Both ceilings bound the work one decode does, so a configuration past either of
    /// them is refused rather than silently clamped: a decoder that quietly ignores
    /// half of what it was configured with is worse than one that says so.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming `decode.beam_k` when the beam is wider than
    /// [`MAX_BEAM_K`], and naming `decode.max_candidates` when the list is longer than
    /// [`MAX_CANDIDATES`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn validate(&self) -> Result<(), ImeError> {
        if self.beam_k > MAX_BEAM_K {
            return Err(ImeError::ConfigInvalid {
                key: String::from("decode.beam_k"),
                reason: format!("must not exceed {MAX_BEAM_K}"),
            });
        }
        if self.max_candidates > MAX_CANDIDATES {
            return Err(ImeError::ConfigInvalid {
                key: String::from("decode.max_candidates"),
                reason: format!("must not exceed {MAX_CANDIDATES}"),
            });
        }
        Ok(())
    }
}

/// The K-best decoder.
///
/// Holds the configuration it was built with and the scorer derived from it, so a
/// decode rebuilds neither. One decoder serves every session: nothing in it is
/// per-session state. Changing a knob means building another decoder, which is what
/// keeps the scorer from ever disagreeing with the weights it was built from.
///
/// [`Decoder::default`] is the shipped configuration, with the scorer built from the
/// same default weights; a test pins that pairing, so the two cannot drift apart
/// unnoticed.
///
/// # Concurrency
///
/// `Send + Sync` and reentrant: a decode borrows the decoder immutably and keeps its
/// working storage in its own frame, so two threads could decode at once.
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    /// The configuration, already validated.
    cfg: DecodeConfig,
    /// The scorer built from `cfg.weights`.
    scorer: Scorer,
}

impl Decoder {
    /// Builds a decoder from `cfg`.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] when [`DecodeConfig::validate`] refuses the
    /// configuration, and when one of the scoring weights is negative, which
    /// [`Scorer::new`] refuses.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(cfg: DecodeConfig) -> Result<Self, ImeError> {
        cfg.validate()?;
        let scorer = Scorer::new(cfg.weights)?;
        Ok(Self { cfg, scorer })
    }

    /// Returns the configuration the decoder was built with.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn config(&self) -> &DecodeConfig {
        &self.cfg
    }

    /// Decodes one request into the ordered candidate list the window draws.
    ///
    /// The segmentation graph is built here, from `req.raw`. With
    /// [`DecodeFlags::USER_DICT`] clear, the user's own words and counts take no part
    /// in the ranking or in the labels.
    ///
    /// # Returns
    ///
    /// The candidates, best first and never empty; the cut of the winning path in
    /// `segments`; and `degraded` set when the answer is not the whole picture, either
    /// because the input has no reading or because the dictionary refused a lookup
    /// while the lattice was built.
    ///
    /// # Errors
    ///
    /// None. A request that cannot be decoded is answered with a degraded result
    /// rather than an error, because the caller has to show the user something either
    /// way.
    ///
    /// # Panics
    ///
    /// Never panics: every index is taken with a checked lookup and every arithmetic
    /// step saturates.
    pub fn decode(
        &self,
        req: &DecodeRequest,
        lx: &dyn Lexicon,
        uf: &dyn UserFreqSource,
        lm: &dyn LanguageModel,
    ) -> DecodeResult {
        let mut dag = SyllableDag::new();
        // Every way the input can be refused -- empty, past the length limit, or
        // impossible to cut into syllables -- leaves the graph without a path, and
        // that is the condition the degraded answer keys on. The error is not
        // propagated because the contract answers a request it cannot decode with a
        // candidate rather than with an error the caller has to handle.
        if dag.build(&req.raw).is_err() || !dag.has_path() {
            return passthrough(&req.raw);
        }
        // The switch is honoured by hiding the user's history from both readers of
        // it, rather than by threading a flag into the lattice and the scorer, which
        // would put the same condition in two more places.
        let silent = Silent;
        let user: &dyn UserFreqSource = if req.flags.contains(DecodeFlags::USER_DICT) {
            uf
        } else {
            &silent
        };
        let lattice = build_lattice(&dag, lx, user, self.cfg.fallback_single);
        let sources = Sources {
            lattice: &lattice,
            scorer: &self.scorer,
            lm,
            uf: user,
        };
        let mut sweep = Sweep::new(sources, usize::from(self.cfg.beam_k), lattice.node_count());
        sweep.run();
        let terminal = usize::from(dag.len());
        let mut drafts = sweep.collect(terminal);
        finish(&mut drafts, self.cfg.max_candidates);
        if drafts.is_empty() {
            // Nothing reached the end of the input: no word of the dictionary covers
            // any reading of it and the fallback is off, or the beam is too narrow to
            // keep a path at all.
            return passthrough(&req.raw);
        }
        let segments = sweep.segments_of(terminal);
        result_of(drafts, segments, lattice.lookup_failed())
    }
}

/// A user-frequency source that has recorded nothing.
///
/// Stands in for the real one when a request clears [`DecodeFlags::USER_DICT`], so that
/// neither the lattice nor the scorer has to know the switch exists.
struct Silent;

impl UserFreqSource for Silent {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// The inputs one sweep ranks against, grouped so the sweep's methods stay inside the
/// parameter budget.
struct Sources<'a, 'dict> {
    /// The lattice being walked.
    lattice: &'a Lattice<'dict>,
    /// The fixed-point scorer the edge scores come from.
    scorer: &'a Scorer,
    /// The language model the scorer queries.
    lm: &'a dyn LanguageModel,
    /// The user's frequencies the scorer queries.
    uf: &'a dyn UserFreqSource,
}

impl<'a, 'dict> Sources<'a, 'dict> {
    /// Returns the word that ends the path whose last edge is `edge`, which is what
    /// the next edge's bigram term conditions on. Answers `None` for the empty path at
    /// node 0, and for an edge index the lattice does not hold.
    fn word_before(&self, edge: u32) -> Option<&'dict str> {
        self.lattice
            .edge_at(edge as usize)
            .map(|found| found.word.text)
    }
}

/// One partial path: a way of reading the input up to one node.
///
/// A path is stored where it lives -- in its node's beam -- and remembers where it came
/// from, so a whole reading is recovered by following those back-pointers rather than
/// by copying text as the sweep runs. Nothing on the hot path owns a `String`: the text
/// of a candidate is built once, from the dictionary's own borrows, after the ranking
/// is settled.
#[derive(Clone, Copy, Debug)]
struct PathState {
    /// Accumulated Q16.16 score, every word's edge score and segment penalty in it.
    score: i32,
    /// When this path was created; what makes the ordering total.
    sequence: u32,
    /// Index of the lattice edge that reached the node, or [`NO_EDGE`].
    edge: u32,
    /// Node that edge leaves.
    from: u16,
    /// Slot the predecessor path occupies in `from`'s beam, or [`NO_SLOT`].
    previous: u16,
    /// Syllables the path covers.
    syllables: u16,
}

impl PathState {
    /// The empty path every other one starts from: no edge, no score.
    const ROOT: Self = Self {
        score: 0,
        sequence: 0,
        edge: NO_EDGE,
        from: 0,
        previous: NO_SLOT,
        syllables: 0,
    };

    /// The contents of a slot no live path occupies. It scores below every real path
    /// and is never read: a beam only looks at the slots below its own length.
    const DEAD: Self = Self {
        score: i32::MIN,
        sequence: u32::MAX,
        edge: NO_EDGE,
        from: 0,
        previous: NO_SLOT,
        syllables: 0,
    };
}

impl Ord for PathState {
    /// Orders paths best first: the higher score wins, and two paths that score the same
    /// are ordered by the moment they were generated.
    ///
    /// The counter is what makes the order total. [`TopK`] answers "which of these is the
    /// weakest" through `Ord`, so without it two paths that scored the same would be
    /// interchangeable, and which of them survived a full beam would depend on the order
    /// they were offered in -- a dependence on the merge's internals that the candidate
    /// order must not have.
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for PathState {
    /// Answers [`Ord::cmp`], which is a total order over the two ranking fields.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for PathState {
    /// Defined from [`Ord`] rather than derived: two paths that rank equal are equal here,
    /// and the fields describing where a path came from -- which the ordering deliberately
    /// ignores -- take no part in it. A derived `Eq` would compare them and so disagree
    /// with `cmp`, which is a contract violation, not a shortcut.
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PathState {}

/// The sweep's working storage.
///
/// Every node owns a window of `cap` slots in one flat buffer, and the live length of
/// each window sits beside it. That is what lets one node's beam be merged into from
/// several predecessors in turn: the window is handed to [`TopK`] again and again, and
/// each merge continues where the previous one stopped.
struct Sweep<'a, 'dict> {
    /// Everything the sweep reads: the lattice and the three injected sources.
    sources: Sources<'a, 'dict>,
    /// Slots per node.
    cap: usize,
    /// Nodes the lattice describes.
    nodes: usize,
    /// `slots[node * cap .. (node + 1) * cap]` is that node's beam.
    slots: Vec<PathState>,
    /// Live length of every node's beam.
    lens: [usize; MAX_NODES],
    /// Generation counter to hand out next.
    sequence: u32,
}

impl<'a, 'dict> Sweep<'a, 'dict> {
    /// Prepares the storage for a lattice of `nodes` nodes and a beam of `cap`.
    ///
    /// A beam wider than [`MAX_BEAM_K`] is lowered to it and a node count past
    /// [`MAX_NODES`] is cut to it, so a hand-built configuration cannot make this
    /// allocate without bound or index past its own arrays.
    fn new(sources: Sources<'a, 'dict>, cap: usize, nodes: usize) -> Self {
        let cap = cap.min(usize::from(MAX_BEAM_K));
        let nodes = nodes.min(MAX_NODES);
        Self {
            sources,
            cap,
            nodes,
            slots: vec![PathState::DEAD; nodes * cap],
            lens: [0usize; MAX_NODES],
            sequence: 1,
        }
    }

    /// Runs the sweep: every node in byte order, its beam sorted, its edges expanded
    /// into the nodes they reach.
    ///
    /// An edge always ends at a later node than it starts from, so a single forward pass
    /// settles the whole lattice: by the time the sweep reaches node `i`, every edge into
    /// it has been merged and its beam is final. The pass is an explicit loop -- never
    /// recursion -- so even the longest legal input cannot overflow the stack.
    fn run(&mut self) {
        self.push(0, PathState::ROOT);
        for node in 0..self.nodes {
            // Sorting a node as the sweep reaches it -- after every edge into it has
            // been merged, before any edge out of it is expanded -- is what keeps the
            // expansion order, and with it the generation counters ties are broken by,
            // a function of the scores rather than of the heap's internal layout.
            if let Some((slots, cap, len)) = self.beam_parts(node) {
                TopK::new(slots, cap, len).sort_desc();
            }
            self.expand_node(node);
        }
    }

    /// Expands every path living at `node` over every edge leaving it.
    fn expand_node(&mut self, node: usize) {
        let live = self.lens.get(node).copied().unwrap_or(0);
        let (first, last) = self.sources.lattice.edge_range(node);
        for slot in 0..live {
            let state = self.slot(node, slot);
            let prev_word = self.sources.word_before(state.edge);
            for index in first..last {
                let Some(edge) = self.sources.lattice.edge_at(index).copied() else {
                    continue;
                };
                let Ok(index) = u32::try_from(index) else {
                    continue;
                };
                let edge_score = self.sources.scorer.edge_score(
                    self.sources.lm,
                    self.sources.uf,
                    prev_word,
                    edge.word.text,
                    edge.characters,
                );
                // One segment is charged per edge rather than the whole path at the
                // end. The penalty is linear in the segment count, so the two add up
                // to the same total -- and this way no path carries its own count.
                let next = PathState {
                    score: state
                        .score
                        .saturating_add(edge_score)
                        .saturating_add(self.sources.scorer.path_penalty(1)),
                    sequence: self.sequence,
                    edge: index,
                    from: u16::try_from(node).unwrap_or(u16::MAX),
                    previous: u16::try_from(slot).unwrap_or(u16::MAX),
                    syllables: state.syllables.saturating_add(u16::from(edge.syllables)),
                };
                self.sequence = self.sequence.saturating_add(1);
                self.push(usize::from(edge.end), next);
            }
        }
    }

    /// Offers one path to the beam of `node`, keeping it only if it belongs to that
    /// node's best few. A node outside the lattice changes nothing.
    fn push(&mut self, node: usize, state: PathState) {
        let Some((slots, cap, len)) = self.beam_parts(node) else {
            return;
        };
        TopK::new(slots, cap, len).push(state);
    }

    /// Splits one node's storage into its slot window, its capacity and the length of the
    /// beam living in it.
    fn beam_parts(&mut self, node: usize) -> Option<(&mut [PathState], usize, &mut usize)> {
        if node >= self.nodes {
            return None;
        }
        let cap = self.cap;
        let window = node.saturating_mul(cap);
        let slots = self.slots.get_mut(window..window.saturating_add(cap))?;
        let len = self.lens.get_mut(node)?;
        Some((slots, cap, len))
    }

    /// Returns the path living in one slot of one node's beam, or the dead path for a slot
    /// outside the storage.
    fn slot(&self, node: usize, slot: usize) -> PathState {
        let index = node.saturating_mul(self.cap).saturating_add(slot);
        self.slots.get(index).copied().unwrap_or(PathState::DEAD)
    }

    /// Collects the edges of one path, back to front, and answers how many there are.
    ///
    /// The walk follows the path's own back-pointers, one node at a time -- an explicit
    /// loop, never recursion -- so a long input cannot overflow the stack. It cannot spin
    /// either: every edge ends at a later node than it starts from, so the walk reaches
    /// the empty path within a bounded number of steps, and the counter stops it there in
    /// any case.
    fn chain(&self, node: usize, slot: usize, out: &mut [u32; MAX_NODES]) -> usize {
        let mut count = 0usize;
        let mut at = node;
        let mut at_slot = slot;
        while count < MAX_NODES {
            let state = self.slot(at, at_slot);
            if state.edge == NO_EDGE {
                break;
            }
            out[count] = state.edge;
            count += 1;
            at = usize::from(state.from);
            at_slot = usize::from(state.previous);
        }
        count
    }

    /// Reads one path back into the text it spells. A candidate is labelled `UserDict`
    /// when any of its words is the user's own, which is where the window's source label
    /// comes from.
    fn read_path(&self, node: usize, slot: usize, score: i32) -> Option<Draft> {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(node, slot, &mut edges);
        let mut text = String::with_capacity(count.saturating_mul(BYTES_PER_WORD));
        let mut source = CandidateSource::Dict;
        let mut syllables = 0u16;
        for index in edges[..count].iter().rev() {
            let edge = self.sources.lattice.edge_at(*index as usize)?;
            text.push_str(edge.word.text);
            syllables = syllables.saturating_add(u16::from(edge.syllables));
            if edge.source == CandidateSource::UserDict {
                source = CandidateSource::UserDict;
            }
        }
        Some(Draft { text, source, syllables, score })
    }

    /// Reads the word one path starts with: the candidate for a user who typed more than
    /// they meant to commit.
    ///
    /// It carries the score of the path it was cut from rather than the score of the prefix
    /// on its own. An additive score over log probabilities always prefers a shorter path
    /// -- every further word can only lower the total -- so a prefix scored on its own
    /// would outrank the whole sentence it came from, and the first candidate would stop
    /// being the reading of the whole input. Inheriting the parent's score puts the prefix
    /// immediately after the readings it was cut from, where it is reachable without
    /// displacing them.
    fn first_word(&self, node: usize, slot: usize, score: i32) -> Option<Draft> {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(node, slot, &mut edges);
        let head = *edges.get(count.checked_sub(1)?)?;
        let edge = self.sources.lattice.edge_at(head as usize)?;
        Some(Draft {
            text: edge.word.text.to_owned(),
            source: edge.source,
            syllables: u16::from(edge.syllables),
            score,
        })
    }

    /// Reads the paths that reach `terminal` into candidate drafts.
    ///
    /// The whole-sentence candidates come first, best path first, and the first-word
    /// candidate of the best path follows them. Nothing is deduplicated or ordered here --
    /// [`finish`] does that -- so the drafts keep the generation order that breaks the
    /// score ties.
    fn collect(&self, terminal: usize) -> Vec<Draft> {
        let live = self.lens.get(terminal).copied().unwrap_or(0);
        let mut drafts = Vec::with_capacity(live.saturating_add(1));
        for slot in 0..live {
            let score = self.slot(terminal, slot).score;
            if let Some(draft) = self.read_path(terminal, slot, score) {
                drafts.push(draft);
            }
        }
        let best = self.slot(terminal, 0).score;
        if let Some(draft) = self.first_word(terminal, 0, best) {
            drafts.push(draft);
        }
        drafts
    }

    /// Describes the cut of the winning path, one [`Segment`] per word.
    ///
    /// The syllable indices are counted along the path rather than read off the nodes,
    /// because a node index is a byte offset and a segment counts syllables.
    fn segments_of(&self, terminal: usize) -> Vec<Segment> {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(terminal, 0, &mut edges);
        let mut segments = Vec::with_capacity(count);
        let mut start = 0u16;
        for index in edges[..count].iter().rev() {
            let Some(edge) = self.sources.lattice.edge_at(*index as usize) else {
                break;
            };
            let end = start.saturating_add(u16::from(edge.syllables));
            segments.push(Segment {
                start,
                end,
                text: edge.word.text.to_owned(),
                source: edge.source,
            });
            start = end;
        }
        segments
    }
}

/// One candidate before it is numbered: what it spells, what it scored, and the two fields
/// the window reads.
struct Draft {
    /// The text the candidate commits.
    text: String,
    /// Where the candidate came from.
    source: CandidateSource,
    /// Syllables it consumes.
    syllables: u16,
    /// Q16.16 score. The ordering is decided on this integer, never on the `f32` that is
    /// written into the candidate.
    score: i32,
}

/// Merges the drafts that spell the same text, orders them best first, and keeps at most
/// `max_candidates` of them.
fn finish(drafts: &mut Vec<Draft>, max_candidates: u16) {
    dedupe(drafts);
    // A stable sort, so two drafts that score the same keep the order they were generated
    // in, which is the order of the paths they came from.
    drafts.sort_by(|left, right| right.score.cmp(&left.score));
    drafts.truncate(usize::from(max_candidates));
}

/// Merges the drafts that spell the same text into one.
///
/// The scan is a linear walk over the drafts kept so far rather than a map lookup: a decode
/// produces at most `beam_k + 1` drafts, so the walk is a handful of comparisons, and a
/// `HashMap` would put its iteration order -- or at least the burden of showing it cannot
/// matter -- into a result that has to be byte-identical across runs.
fn dedupe(drafts: &mut Vec<Draft>) {
    let mut kept = 0usize;
    for read in 0..drafts.len() {
        let found = drafts[..kept]
            .iter()
            .position(|seen| seen.text == drafts[read].text);
        match found {
            Some(at) => {
                let (left, right) = drafts.split_at_mut(read);
                merge_into(&mut left[at], &right[0]);
            }
            None => {
                drafts.swap(kept, read);
                kept += 1;
            }
        }
    }
    drafts.truncate(kept);
}

/// Folds a second draft that spells the same text into the first.
///
/// The higher score wins. The one exception is a text the base dictionary and the user's own
/// dictionary both hold: that is the strongest evidence there is that the user means it, so
/// the two labels merge into `UserDict` and the two scores add up instead of one being
/// thrown away.
fn merge_into(target: &mut Draft, other: &Draft) {
    let dictionary = |source: CandidateSource| {
        matches!(source, CandidateSource::Dict | CandidateSource::UserDict)
    };
    if dictionary(target.source) && dictionary(other.source) && target.source != other.source {
        target.source = CandidateSource::UserDict;
        target.score = target.score.saturating_add(other.score);
        return;
    }
    if other.score > target.score {
        target.score = other.score;
        target.source = other.source;
        target.syllables = other.syllables;
    }
}

/// The answer to a request that cannot be decoded: one candidate that commits the input
/// unchanged, and a degraded result.
fn passthrough(raw: &str) -> DecodeResult {
    DecodeResult {
        candidates: vec![Candidate {
            index: 1,
            text: raw.to_owned(),
            annotation: None,
            source: CandidateSource::Passthrough,
            // No path was scored, so there is nothing to show; the field is display-only
            // and never decides an order.
            score: 0.0,
            consumed_syllables: 0,
        }],
        // No path, so there is no cut to describe.
        segments: Vec::new(),
        degraded: true,
    }
}

/// Turns the ordered drafts into the frozen result.
fn result_of(drafts: Vec<Draft>, segments: Vec<Segment>, degraded: bool) -> DecodeResult {
    let candidates = drafts
        .into_iter()
        .enumerate()
        .map(|(position, draft)| Candidate {
            // The display number is the position, 1-based: the window draws it next to
            // the number key that selects the candidate.
            index: u16::try_from(position).unwrap_or(u16::MAX).saturating_add(1),
            text: draft.text,
            annotation: None,
            source: draft.source,
            // The one float in the pipeline, written last and read by nobody: a Q16.16
            // score turned into the display score the window may show.
            score: draft.score as f32 / Q16_ONE,
            consumed_syllables: draft.syllables,
        })
        .collect();
    DecodeResult { candidates, segments, degraded }
}
