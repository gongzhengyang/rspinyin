//! The mixed-English pass: the whole-input edge an uncuttable input is offered.
//!
//! Responsibility: after the exact, fuzzy, correction and abbreviation walks have run,
//! offer the input's own spelling to the English lexicon and add one edge per word it
//! holds, priced [`MIXED_ENGLISH_PENALTY_Q8`], when no pinyin path reaches the input's
//! end.
//!
//! Boundaries: the pass reads the English lexicon and pushes edges; it scores nothing
//! and never touches a request whose input the pinyin walks could cut. It is the only
//! place the English lexicon is read, which is what keeps a request that reads as
//! pinyin at the cost it had before the switch existed.

use ime_types::CandidateSource;
use smallvec::SmallVec;

use crate::segment::SyllableDag;
use crate::segment::english::MIXED_ENGLISH_PENALTY_Q8;

use super::Builder;
use super::LatticeEdge;
use super::WORDS_PER_KEY;
use super::chars::count_characters;

impl Builder<'_, '_, '_> {
    /// Offers the whole input as one English span, when nothing covers it.
    ///
    /// The rule the mixed-English switch asks for: the exact, fuzzy and correction
    /// walks have all run by the time this is reached, so an input whose end no path
    /// reaches is one no pinyin reading cuts. The English lexicon is asked for the
    /// input's own spelling -- latin letters are their own key -- and every entry it
    /// holds becomes one edge covering the whole span, priced
    /// [`MIXED_ENGLISH_PENALTY_Q8`]. The pass is the only place the English lexicon
    /// is read, so a request that reads as pinyin never queries it and costs what it
    /// cost before the switch existed.
    pub(super) fn english_span(&mut self, dag: &SyllableDag) {
        // Taken out of `self` so the words it hands back carry the builder's own
        // lexicon lifetime, which is the one the lattice's edges are built over.
        let Some(english) = self.english else {
            return;
        };
        let end = usize::from(dag.len());
        // An input the walks cut (or an empty one) has its answer already: the
        // question is never asked, which is what keeps an English candidate from
        // ever displacing a pinyin reading.
        if end == 0 || self.live.get(end).copied().unwrap_or(false) {
            return;
        }
        let text = dag.normalized();
        let Ok(words) = english.lookup(text) else {
            self.lattice.lookup_failed = true;
            return;
        };
        // The span's spelling is its own key: latin letters are what the English
        // container stores, and the arena entry is what a later decode of the same
        // input spells again.
        let key_start = self.keys.len();
        self.keys.extend_from_slice(text.as_bytes());
        let key_range = (
            u32::try_from(key_start).unwrap_or(u32::MAX),
            u16::try_from(text.len()).unwrap_or(u16::MAX),
        );
        let Ok(end) = u16::try_from(end) else {
            return;
        };
        // The edges leave node 0, and the lattice groups a node's edges contiguously:
        // they are collected first and spliced in ahead of everything, so node 0's
        // range covers them, and every later node's start shifts by their count. The
        // count is bounded by the same per-key cap every lookup answers to.
        let mut spliced: SmallVec<[LatticeEdge<'_>; WORDS_PER_KEY]> = SmallVec::new();
        for word in words.take(WORDS_PER_KEY) {
            spliced.push(LatticeEdge {
                end,
                // No pinyin syllable is consumed: the candidate's own count stays at
                // zero, the same shape a pass-through candidate has.
                syllables: 0,
                characters: count_characters(word.text),
                source: CandidateSource::English,
                word,
                key: key_range,
                penalty_q8: MIXED_ENGLISH_PENALTY_Q8,
            });
            // The edge leaves the root, which is live by definition, so the marking
            // is made directly rather than through `reach`, whose guard reads the
            // node the walk was on.
            if let Some(slot) = self.live.get_mut(usize::from(end)) {
                *slot = true;
            }
        }
        let added = spliced.len();
        let head = spliced.into_vec();
        self.lattice.edges.splice(0..0, head);
        for (index, start) in self.lattice.starts.iter_mut().enumerate() {
            if index > 0 {
                *start = start.saturating_add(u32::try_from(added).unwrap_or(u32::MAX));
            }
        }
    }
}
