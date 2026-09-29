//! The K-best sweep over the lattice, and the candidates it produces.
//!
//! Responsibility: rank the readings of one input and hand back the ordered candidates the
//! window draws. The sweep walks the lattice's nodes in byte order, keeps the best few paths
//! at every node, and reads the paths that reach the last node back into candidate text.
//! Keeping the best `K` at every node -- not only the single best -- is what makes the second
//! and third readings available at all: the second-best reading of the whole input can share a
//! prefix with the best one and diverge only at the last word, and a node that kept one path
//! would have thrown it away before the divergence was reached.
//!
//! Boundaries: the sweep looks nothing up on its own -- the lattice it walks was built by
//! [`build_lattice`](crate::viterbi::build_lattice) -- and it never decides how a candidate
//! looks on screen. It fills in the frozen [`DecodeResult`] and stops there. It is a pure
//! function of its arguments: the lattice, the user's frequencies and the language model, and
//! nothing else. No file, no clock, no environment, no global state, which is what makes the
//! same input rank the same way on every run and in every test.
//!
//! # Storage
//!
//! Nothing here owns a buffer it could be lent. The beams and their live lengths come from a
//! [`SweepStorage`] the caller keeps across decodes, and the drafts and the frozen result come
//! from buffers it keeps as well, so a sweep that has ranked one input ranks the next without
//! touching the allocator.
//!
//! # Determinism
//!
//! The candidate order is part of the contract, so every step that could make it depend on
//! anything but the input is pinned down:
//!
//! - Scores are Q8.8 integers ([`Scorer`]) and the ordering is decided on those integers; the
//!   `f32` on a [`Candidate`] is written last and read by nobody.
//! - Every path carries a monotone generation counter, so two paths that score the same are
//!   ordered by when they were generated rather than by which one a heap happened to compare
//!   first.
//! - A node's beam is sorted as the sweep reaches it, so the order its paths are expanded in
//!   follows from the scores, not from the heap's internal layout.
//! - No `HashMap` is on the path; drafts are deduplicated by a linear scan.

use core::cmp::Ordering;

use ime_types::{Candidate, CandidateSource, DecodeResult, LanguageModel, Segment, UserFreqSource};

use crate::lm::{Q, Scorer};
use crate::segment::MAX_NODES;
use crate::viterbi::decoder::MAX_BEAM_K;
use crate::viterbi::kbest::TopK;
use crate::viterbi::lattice::Lattice;

/// Marks a path that no edge reached: the empty path every other one starts from.
const NO_EDGE: u32 = u32::MAX;

/// Marks a path with no predecessor.
const NO_SLOT: u16 = u16::MAX;

/// Bytes one word adds to a candidate, used to size its text before it is built.
///
/// A word is at least one CJK character, three bytes in UTF-8, so this is a lower bound and a
/// good enough hint: sizing exactly would need a second pass over the path for the sake of a
/// few bytes.
const BYTES_PER_WORD: usize = 3;

/// The Q16.16 unit as a float, for the display-only candidate score.
const Q16_ONE: f32 = (Q * Q) as f32;

/// The inputs one sweep ranks against, grouped so the sweep's methods stay inside the
/// parameter budget.
pub(super) struct Sources<'a, 'dict> {
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
    /// Groups the four readers one sweep ranks against.
    pub(super) fn new(
        lattice: &'a Lattice<'dict>,
        scorer: &'a Scorer,
        lm: &'a dyn LanguageModel,
        uf: &'a dyn UserFreqSource,
    ) -> Self {
        Self {
            lattice,
            scorer,
            lm,
            uf,
        }
    }

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

/// The sweep's working storage, kept across decodes.
///
/// Every node owns a window of `cap` slots in one flat buffer, and the live length of each
/// window sits beside it. That is what lets one node's beam be merged into from several
/// predecessors in turn: the window is handed to [`TopK`] again and again, and each merge
/// continues where the previous one stopped. The storage is lent to a sweep rather than owned
/// by it, which is what lets the allocation survive the call.
#[derive(Debug)]
pub(super) struct SweepStorage {
    /// `slots[node * cap .. (node + 1) * cap]` is that node's beam.
    slots: Vec<PathState>,
    /// Live length of every node's beam.
    lens: [usize; MAX_NODES],
}

impl SweepStorage {
    /// Creates empty storage; the first sweep that uses it allocates.
    pub(super) fn new() -> Self {
        Self {
            slots: Vec::new(),
            lens: [0usize; MAX_NODES],
        }
    }

    /// Grows the storage to hold a lattice of `nodes` nodes and a beam of `cap`, keeping the
    /// allocation it already holds whenever that is enough.
    ///
    /// A beam wider than [`MAX_BEAM_K`] and a node count past [`MAX_NODES`] are cut to their
    /// ceilings, exactly as the sweep cuts them, so a hand-built configuration cannot make
    /// this allocate without bound. Emptying the beams is the sweep's job rather than this
    /// one's: a slot the previous decode left behind sits below the live length of every beam
    /// and is never read, so it does not have to be written.
    pub(super) fn prepare(&mut self, nodes: usize, cap: usize) {
        let cap = cap.min(usize::from(MAX_BEAM_K));
        let needed = nodes.min(MAX_NODES).saturating_mul(cap);
        if self.slots.len() < needed {
            self.slots.resize(needed, PathState::DEAD);
        }
    }
}

#[cfg(test)]
impl SweepStorage {
    /// Returns how many beam slots the storage holds, for the tests that pin the reuse.
    ///
    /// The allocation a reused workspace is worth is the one this reports, and the tests that
    /// show a decode does not grow it need to read it.
    pub(super) fn slots_capacity(&self) -> usize {
        self.slots.capacity()
    }
}

/// The sweep itself: the lattice being walked and the storage it walks it with.
pub(super) struct Sweep<'a, 'dict, 'store> {
    /// Everything the sweep reads: the lattice and the three injected sources.
    sources: Sources<'a, 'dict>,
    /// Slots per node.
    cap: usize,
    /// Nodes the lattice describes.
    nodes: usize,
    /// Borrowed from the caller's storage: `slots[node * cap .. (node + 1) * cap]` is that
    /// node's beam. Borrowed rather than owned so the buffer survives the call, which is the
    /// whole point of the workspace it is lent by.
    slots: &'store mut [PathState],
    /// Live length of every node's beam, borrowed for the same reason.
    lens: &'store mut [usize; MAX_NODES],
    /// Generation counter to hand out next.
    sequence: u32,
}

impl<'a, 'dict, 'store> Sweep<'a, 'dict, 'store> {
    /// Prepares a sweep over a lattice of `nodes` nodes and a beam of `cap`, using `storage`
    /// for its beams.
    ///
    /// A beam wider than [`MAX_BEAM_K`] is lowered to it, a node count past [`MAX_NODES`] is
    /// cut to it, and the node count is cut again to what the storage can hold, so a
    /// hand-built configuration cannot index past the buffer it was lent. The beams start
    /// empty: the live lengths are cleared here, which is the only reset the reused slots
    /// need.
    pub(super) fn with_storage(
        sources: Sources<'a, 'dict>,
        cap: usize,
        nodes: usize,
        storage: &'store mut SweepStorage,
    ) -> Self {
        let cap = cap.min(usize::from(MAX_BEAM_K));
        let nodes = nodes.min(MAX_NODES).min(storage.slots.len() / cap.max(1));
        let SweepStorage { slots, lens } = storage;
        *lens = [0usize; MAX_NODES];
        Self {
            sources,
            cap,
            nodes,
            slots: &mut slots[..],
            lens,
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
    pub(super) fn run(&mut self) {
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
                let edge_score = self
                    .sources
                    .scorer
                    .edge_score(
                        self.sources.lm,
                        self.sources.uf,
                        prev_word,
                        edge.word.text,
                        edge.characters,
                    )
                    // The edge's own penalty, subtracted after the score rather than folded
                    // into it: an abbreviated edge and a full-spelling edge for the same word
                    // are scored by the same language model, and what separates them is that
                    // one of them guessed at the reading. Subtracting here is what makes the
                    // ordering a property of the scores rather than of the order the edges
                    // were pushed in.
                    .saturating_sub(edge.penalty_q8);
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

    /// Reads the paths that reach `terminal` into candidate drafts, reusing the text buffers
    /// `out` already holds.
    ///
    /// The whole-sentence candidates come first, best path first, and the first-word
    /// candidate of the best path follows them. Nothing is deduplicated or ordered here --
    /// [`finish`] does that -- so the drafts keep the generation order that breaks the
    /// score ties.
    pub(super) fn collect_into(&self, terminal: usize, out: &mut Vec<Draft>) {
        let live = self.lens.get(terminal).copied().unwrap_or(0);
        let mut written = 0usize;
        for slot in 0..live {
            let score = self.slot(terminal, slot).score;
            if self.write_path(terminal, slot, score, out, written) {
                written = written.saturating_add(1);
            }
        }
        // Only a beam with a path in it can offer a first word: with nothing at the terminal
        // node the slot below is one the previous decode left behind, and reading it would
        // turn a stale path into a candidate for an input that has no reading at all.
        if live > 0 {
            let best = self.slot(terminal, 0).score;
            if self.write_first_word(terminal, 0, best, out, written) {
                written = written.saturating_add(1);
            }
        }
        // A decode that produced fewer drafts than the last one gives the surplus slots up;
        // a longer one is served by the slots the walk grew.
        out.truncate(written);
    }

    /// Writes the draft of the path ending at `(node, slot)` into `out[position]`, reusing the
    /// text buffer that slot already holds, and answers whether the path was complete.
    ///
    /// A path whose chain names an edge the lattice does not hold is not a candidate: the
    /// answer is `false` and the caller leaves that slot out of the list, which is why the
    /// caller passes the position rather than the sweep appending for itself.
    fn write_path(
        &self,
        node: usize,
        slot: usize,
        score: i32,
        out: &mut Vec<Draft>,
        position: usize,
    ) -> bool {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(node, slot, &mut edges);
        // The list grows to hold this decode's candidates: a fresh slot holds no allocation,
        // because an empty `String` has no backing store until a word is pushed into it.
        if position >= out.len() {
            out.push(Draft::empty());
        }
        let Some(draft) = out.get_mut(position) else {
            return false;
        };
        draft.text.clear();
        draft.score = score;
        draft.source = CandidateSource::Dict;
        draft.syllables = 0;
        // A capacity hint, not a requirement: a buffer that already holds enough is left
        // alone, so a steady-state decode reserves nothing.
        draft.text.reserve(count.saturating_mul(BYTES_PER_WORD));
        for index in edges[..count].iter().rev() {
            let Some(edge) = self.sources.lattice.edge_at(*index as usize) else {
                return false;
            };
            draft.text.push_str(edge.word.text);
            draft.syllables = draft.syllables.saturating_add(u16::from(edge.syllables));
            if edge.source == CandidateSource::UserDict {
                draft.source = CandidateSource::UserDict;
            }
        }
        true
    }

    /// Writes the word one path starts with into `out[position]`: the candidate for a user
    /// who typed more than they meant to commit.
    ///
    /// It carries the score of the path it was cut from rather than the score of the prefix
    /// on its own. An additive score over log probabilities always prefers a shorter path
    /// -- every further word can only lower the total -- so a prefix scored on its own
    /// would outrank the whole sentence it came from, and the first candidate would stop
    /// being the reading of the whole input. Inheriting the parent's score puts the prefix
    /// immediately after the readings it was cut from, where it is reachable without
    /// displacing them.
    fn write_first_word(
        &self,
        node: usize,
        slot: usize,
        score: i32,
        out: &mut Vec<Draft>,
        position: usize,
    ) -> bool {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(node, slot, &mut edges);
        let Some(head) = count.checked_sub(1).and_then(|at| edges.get(at).copied()) else {
            return false;
        };
        let Some(edge) = self.sources.lattice.edge_at(head as usize) else {
            return false;
        };
        if position >= out.len() {
            out.push(Draft::empty());
        }
        let Some(draft) = out.get_mut(position) else {
            return false;
        };
        draft.text.clear();
        draft.text.push_str(edge.word.text);
        draft.source = edge.source;
        draft.syllables = u16::from(edge.syllables);
        draft.score = score;
        true
    }

    /// Describes the cut of the winning path, one [`Segment`] per word, reusing the text
    /// buffers `out` already holds.
    ///
    /// The syllable indices are counted along the path rather than read off the nodes,
    /// because a node index is a byte offset and a segment counts syllables.
    pub(super) fn segments_into(&self, terminal: usize, out: &mut Vec<Segment>) {
        let mut edges = [0u32; MAX_NODES];
        let count = self.chain(terminal, 0, &mut edges);
        let mut start = 0u16;
        let mut written = 0usize;
        for (position, index) in edges[..count].iter().rev().enumerate() {
            let Some(edge) = self.sources.lattice.edge_at(*index as usize) else {
                break;
            };
            let end = start.saturating_add(u16::from(edge.syllables));
            match out.get_mut(position) {
                Some(segment) => {
                    segment.start = start;
                    segment.end = end;
                    segment.text.clear();
                    segment.text.push_str(edge.word.text);
                    segment.source = edge.source;
                }
                None => out.push(Segment {
                    start,
                    end,
                    text: edge.word.text.to_owned(),
                    source: edge.source,
                }),
            }
            start = end;
            written = position.saturating_add(1);
        }
        // A cut shorter than the previous one gives the surplus slots up rather than leaving
        // a stale segment behind the end of the list.
        out.truncate(written);
    }
}

/// One candidate before it is numbered: what it spells, what it scored, and the two fields
/// the window reads.
#[derive(Debug)]
pub(super) struct Draft {
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

impl Draft {
    /// An empty draft: no text, no score, the dictionary label.
    ///
    /// Growing a list to a longer decode pushes these, which allocates nothing.
    fn empty() -> Self {
        Self {
            text: String::new(),
            source: CandidateSource::Dict,
            syllables: 0,
            score: 0,
        }
    }
}

/// Merges the drafts that spell the same text, orders them best first, and keeps at most
/// `max_candidates` of them.
pub(super) fn finish(drafts: &mut Vec<Draft>, max_candidates: u16) {
    dedupe(drafts);
    // A stable sort, so two drafts that score the same keep the order they were generated
    // in, which is the order of the paths they came from. `sort_by_key` is stable too, so
    // the tie-break is preserved by the `Reverse` rather than by the comparator's shape.
    drafts.sort_by_key(|draft| std::cmp::Reverse(draft.score));
    // Truncating keeps the capacity of the list itself; the text buffers of the drafts that
    // are dropped go with them, which is the price of a shorter candidate list.
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

/// Turns the ordered drafts into the frozen result, reusing the buffers `out` already holds.
///
/// Every candidate is written through the slot it already occupies, which is what keeps the
/// text buffers: a decode that produced the same number of candidates as the last one
/// allocates nothing here, because each text is overwritten in place rather than replaced.
pub(super) fn result_of_into(out: &mut DecodeResult, drafts: &[Draft], degraded: bool) {
    for (position, draft) in drafts.iter().enumerate() {
        // The display number is the position, 1-based: the window draws it next to the
        // number key that selects the candidate.
        let index = u16::try_from(position)
            .unwrap_or(u16::MAX)
            .saturating_add(1);
        // The one float in the pipeline, written last and read by nobody: a Q16.16
        // score turned into the display score the window may show.
        let score = draft.score as f32 / Q16_ONE;
        match out.candidates.get_mut(position) {
            Some(slot) => {
                slot.index = index;
                slot.text.clear();
                slot.text.push_str(&draft.text);
                slot.annotation = None;
                slot.source = draft.source;
                slot.score = score;
                slot.consumed_syllables = draft.syllables;
            }
            None => out.candidates.push(Candidate {
                index,
                text: draft.text.clone(),
                annotation: None,
                source: draft.source,
                score,
                consumed_syllables: draft.syllables,
            }),
        }
    }
    // The list is cut to what this decode produced. A longer previous list gives its surplus
    // candidates up, text buffers included: the contract says the list is exactly what the
    // decode found.
    out.candidates.truncate(drafts.len());
    out.degraded = degraded;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sweep_storage_prepare_grows_to_the_sweep_it_will_hold() {
        let mut storage = SweepStorage::new();
        assert_eq!(storage.slots.len(), 0, "an empty storage holds nothing");
        storage.prepare(4, 16);
        assert_eq!(storage.slots.len(), 64);
        // A wider sweep than the storage holds grows it; a smaller one reuses it.
        storage.prepare(8, 16);
        assert_eq!(storage.slots.len(), 128);
        storage.prepare(2, 16);
        assert_eq!(
            storage.slots.len(),
            128,
            "a smaller sweep keeps the allocation"
        );
    }

    #[test]
    fn test_sweep_storage_prepare_cuts_the_beam_and_the_nodes_to_their_ceilings() {
        let mut storage = SweepStorage::new();
        // A configuration past either ceiling cannot make the storage allocate without bound:
        // the ceilings are what the sweep clamps to as well.
        storage.prepare(usize::MAX, usize::from(MAX_BEAM_K) + 8);
        assert_eq!(storage.slots.len(), MAX_NODES * usize::from(MAX_BEAM_K));
        let mut zero = SweepStorage::new();
        zero.prepare(4, 0);
        assert_eq!(zero.slots.len(), 0, "a beam of no slots needs no storage");
    }
}
