//! The abbreviation walk: the extra edges an input reaches when a syllable may be
//! typed as its initial alone.
//!
//! Responsibility: for one node of the graph, enumerate the readings of the input from
//! that node, turn every reading that leaves a syllable as a bare initial into a
//! dictionary prefix query, and add one edge per word the query answers with -- a word
//! of exactly the length the reading spells, carrying [`ABBREV_PENALTY_Q8`] so that a
//! full spelling outranks an abbreviation at equal dictionary weight.
//!
//! Boundaries: the walk reads the input and the dictionary and pushes edges; it scores
//! nothing, ranks nothing and decides nothing about candidates. It is pure -- no file,
//! no clock, no environment, no global state -- and the buffers it writes into belong to
//! the caller. The algebra of what a reading is lives in
//! [`crate::segment::abbrev`]; this module only wires it to the dictionary.
//!
//! # The dictionary side
//!
//! A reading made of initials spells a query of initials joined by `'`: `nh` is asked
//! for as `n'h`, and `nih` as `ni'h`. The second is a literal prefix of the key
//! `ni'hao`, so any dictionary answers it; the first is the abbreviation key itself, and
//! only a dictionary that indexed those keys answers it. Whether the shipped dictionary
//! does is the dictionary compiler's business rather than the lattice's: a query no key
//! answers contributes no edge, and the input falls back to its full spellings.
//!
//! # Why the walk is bounded on four axes
//!
//! Abbreviation multiplies a lattice: one node has up to [`MAX_READINGS`] readings, one
//! reading up to [`MAX_SPELLINGS`] queries, one query up to [`PREFIX_LIMIT`] words, and
//! the edges of every extension walk together are held to [`MAX_TOTAL_EDGES`]. Each of
//! the four is what makes the walk's cost independent of how ambiguous the input is, and
//! the enumeration's order is what makes them harmless: readings come
//! most-specific-first, so a cap drops the vaguest readings, the ones a reader is least
//! likely to have meant.
//!
//! [`MAX_READINGS`]: crate::segment::abbrev::MAX_READINGS
//! [`MAX_SPELLINGS`]: crate::segment::abbrev::MAX_SPELLINGS
//! [`PREFIX_LIMIT`]: crate::segment::abbrev::PREFIX_LIMIT

use crate::segment::SyllableDag;
use crate::segment::abbrev::{
    ABBREV_PENALTY_Q8, AbbrevReading, MAX_READINGS, PREFIX_LIMIT, is_enabled, readings_into,
    spell_into, spelling_count,
};

use super::MAX_WORD_SYLLABLES;

use super::{Builder, LatticeEdge, MAX_KEYS_PER_NODE, MAX_TOTAL_EDGES, WORDS_PER_KEY};

/// Prefix queries one node makes before the abbreviation walk stops.
///
/// The bound the exact walk puts on the keys of one node, applied to the queries of the
/// abbreviation walk beside it, so that a node costs at most twice what it cost before
/// abbreviation existed. It is far above what a real input needs -- `bjdx` spells one
/// query per reading and has four readings -- and it is what stops a reading of six
/// initials, each of which stands for two spellings, from spelling sixty-four queries on
/// its own.
const MAX_PREFIX_QUERIES_PER_NODE: usize = MAX_KEYS_PER_NODE;

impl Builder<'_, '_, '_> {
    /// Adds the abbreviation edges leaving `node`.
    ///
    /// The walk is entered only for a node a path reaches, only while
    /// [`DecodeFlags::ABBREV`](ime_types::DecodeFlags::ABBREV) is set, and only while the
    /// extension walks have not reached their ceiling; with any of the three against it,
    /// nothing of the abbreviation layer is called at all.
    pub(super) fn abbrev(&mut self, dag: &SyllableDag, node: u16) {
        if !self.live_node || self.extensions_full || !is_enabled(self.flags) {
            return;
        }
        // The span walk of this node is done, so its key buffer is free and can carry
        // the query instead: a query has the same shape as a key -- spellings joined by
        // `'`, at most `MAX_WORD_SYLLABLES` of them -- so the buffer the build already
        // owns is wide enough and no second one is needed.
        let mut query = core::mem::take(&mut self.key);
        self.abbrev_node(dag, node, &mut query);
        self.key = query;
    }

    /// Enumerates the readings of the input from `node` and adds an edge per hit.
    fn abbrev_node(&mut self, dag: &SyllableDag, node: u16, query: &mut String) {
        let Some(rest) = dag.normalized().get(usize::from(node)..) else {
            return;
        };
        // The enumeration buffer is taken out of the build so that the walk can push
        // edges into the lattice while it reads the readings; it goes back on the single
        // exit below, so the caller keeps the allocation it lent.
        let mut readings = core::mem::take(&mut *self.readings);
        if readings_into(&mut readings, rest) {
            self.abbrev_truncated = true;
        }
        let mut queries = 0usize;
        // The walk visits the readings shortest first, and that is not the order the
        // enumeration hands them over in. `readings_into` is most-specific-first, which
        // puts the input's ordinary cut at the front and the single initial at the back;
        // for this walk the two are the other way round, because the reading a reader
        // types is the short one -- one initial stands for a whole syllable -- while the
        // long ones are what the exact walk has already looked up. Visiting them in the
        // enumeration's order spends the query cap on the readings least likely to
        // answer and never reaches the initial that would have found the word.
        //
        // Sorted here rather than in `readings_into` because that enumeration's own order
        // is a documented contract with its other readers, and this buffer is shared.
        let mut visited = [false; MAX_READINGS];
        'readings: for _ in 0..readings.len() {
            let Some(index) = shortest_unvisited(readings.as_slice(), &mut visited) else {
                break;
            };
            let reading = &readings.as_slice()[index];
            let units = reading.syllables.len();
            // A reading with no bare initial is the input's ordinary full-pinyin cut,
            // which the span walk already looked up; a reading longer than the longest
            // word can never be matched by one, so neither is worth a query.
            if units > usize::from(MAX_WORD_SYLLABLES) || !reading.has_initial() {
                continue;
            }
            let Ok(syllables) = u8::try_from(units) else {
                continue;
            };
            let end = node.saturating_add(reading.consumed);
            for combination in 0..spelling_count(reading) {
                if queries >= MAX_PREFIX_QUERIES_PER_NODE || self.extensions_full {
                    break 'readings;
                }
                // A reading that spells more combinations than the cap holds refuses
                // every one of them, so this is the branch that drops it.
                if !spell_into(query, reading, combination) {
                    continue;
                }
                queries = queries.saturating_add(1);
                self.prefix_edges(end, syllables, query.as_str());
            }
        }
        *self.readings = readings;
    }

    /// Adds one edge per word the prefix query `query` answers with.
    fn prefix_edges(&mut self, end: u16, syllables: u8, query: &str) {
        // The reference is copied out first: the dictionary outlives this builder, so the
        // words it hands back stay valid while the lattice is filled.
        let lexicon = self.lexicon;
        let Ok(words) = lexicon.prefix(query, PREFIX_LIMIT) else {
            // A dictionary that cannot serve a prefix query leaves the lattice missing
            // whatever the abbreviation would have found, exactly as a refused exact
            // lookup leaves it missing a word.
            self.lattice.lookup_failed = true;
            return;
        };
        for word in words.take(WORDS_PER_KEY) {
            // A prefix query answers every word whose key starts with the query, and a
            // key can carry more syllables than the reading spells: `ni'h` is a prefix
            // of `ni'hao` and of `ni'hao'ma`. Only a word of exactly the reading's
            // length covers the span the reading covers, so the entry's own claim is the
            // filter -- and the edge's syllable count stays the reading's own, so that a
            // candidate's accounting adds up to the path it came from.
            if usize::from(word.syl_count) != usize::from(syllables) {
                continue;
            }
            if self.extension_edges >= MAX_TOTAL_EDGES {
                self.extensions_full = true;
                return;
            }
            let source = self.source_of(word);
            self.lattice.edges.push(LatticeEdge {
                end,
                syllables,
                characters: u16::try_from(word.text.chars().count()).unwrap_or(u16::MAX),
                source,
                word,
                penalty_q8: ABBREV_PENALTY_Q8,
            });
            self.extension_edges = self.extension_edges.saturating_add(1);
            self.reach(end);
        }
    }
}

/// The index of the reading that consumes the fewest letters and has not been visited yet,
/// marked as visited.
///
/// A selection rather than a sort, because the answer is wanted one reading at a time and
/// the buffer is bounded by [`MAX_READINGS`]: the walk is O(n²) over at most sixty-four
/// readings, which is cheaper than the allocation a sort of borrowed readings would need.
/// It is a total function: `None` only when every reading has been visited.
fn shortest_unvisited(
    readings: &[AbbrevReading],
    visited: &mut [bool; MAX_READINGS],
) -> Option<usize> {
    let mut best: Option<(usize, u16)> = None;
    for (index, reading) in readings.iter().enumerate() {
        if visited.get(index).copied().unwrap_or(true) {
            continue;
        }
        if best.is_none_or(|(_, consumed)| reading.consumed < consumed) {
            best = Some((index, reading.consumed));
        }
    }
    let (index, _) = best?;
    if let Some(slot) = visited.get_mut(index) {
        *slot = true;
    }
    Some(index)
}

#[cfg(test)]
mod tests;
