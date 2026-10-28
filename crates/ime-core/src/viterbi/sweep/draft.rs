//! The draft: one candidate before it is numbered, and the finishing pass that turns
//! the sweep's paths into the draft list the result is written from.
//!
//! Responsibility: hold what one candidate is made of -- its text, its score, the
//! word spans a commit coins from, and the standard readings a wrong-sound hint is
//! built from -- and merge the drafts that spell the same text into one list, best
//! first.
//!
//! Boundaries: nothing here walks the lattice or scores a path; the ranges and
//! scores arrive filled by the sweep, and the only decision this module makes is
//! which of two drafts that spell the same text survives.

use ime_types::CandidateSource;

/// Where one word of a path sits: the byte range its characters occupy in the draft's
/// text, and the byte range its key occupies in the normalized input.
///
/// The pair list a commit coins from is read off these ranges after the decode, which is
/// why they are recorded here rather than resolved to strings during the sweep: the ranges
/// cost two `u16` pairs per word, are cleared and refilled in buffers the draft already
/// owns, and name bytes of the normalized input the workspace holds for exactly as long as
/// the candidate list it produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WordSpan {
    /// Byte range in [`Draft::text`] the word's characters occupy.
    pub(crate) text: (u16, u16),
    /// Byte range of the word's key in the workspace's key arena: the canonical
    /// spelling the lattice walk built -- syllable texts joined with the separator --
    /// which is what a later lookup of the same word spells. Empty for an edge whose
    /// reading is not a key of its own, which the pair reader refuses.
    pub(crate) key: (u32, u16),
}

/// Where a draft's word spans sit in the workspace's span arena.
///
/// A plain Copy range rather than a `Vec` per draft, because the drafts beyond the merge's
/// keep-list are dropped and refilled on every decode: a buffer of their own would be
/// re-allocated on every steady-state decode, and the allocation budget counts them. The
/// arena the ranges name is cleared at the start of every collect and refilled by the same
/// walk that writes the drafts, so a range is valid for exactly as long as the draft list
/// it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WordSpans {
    /// Arena index the draft's first span sits at.
    pub(super) start: u32,
    /// How many spans the draft's path recorded.
    pub(super) len: u16,
}

impl WordSpans {
    /// The range of a draft with no spans: the empty path, and the value every fresh
    /// slot starts with.
    pub(crate) const EMPTY: Self = Self { start: 0, len: 0 };
}

/// One candidate before it is numbered: what it spells, what it scored, and the two fields
/// the window reads.
#[derive(Debug)]
pub(crate) struct Draft {
    /// The text the candidate commits.
    pub(crate) text: String,
    /// Where the candidate came from.
    pub(crate) source: CandidateSource,
    /// Syllables it consumes.
    pub(crate) syllables: u16,
    /// Q16.16 score. The ordering is decided on this integer, never on the `f32` that is
    /// written into the candidate.
    pub(crate) score: i32,
    /// Whether the path behind the draft used a spelling-correction edge.
    ///
    /// The edge is recognized by its penalty, the one mark a correction edge
    /// carries through the lattice -- the same mark an abbreviation and a fuzzy
    /// variant are told apart by. A candidate whose path needed no correction is
    /// annotated with nothing.
    pub(crate) corrected: bool,
    /// Whether this draft covers the request's final syllable.
    ///
    /// Rank decay applies only to complete sentence paths. Recording the fact at
    /// collection time avoids inferring it from syllable counts after the list has
    /// been sorted and merged.
    pub(crate) whole_sentence: bool,
    /// The standard readings of the widened edges the path used, joined with the
    /// key separator.
    ///
    /// A widened edge is one the exact walk did not produce -- a fuzzy variant or a
    /// spelling correction -- and its key is how the word is read in standard pinyin,
    /// which is what the `音:` hint shows. Empty for a path the exact walk covered,
    /// which is the steady state: the buffer is cleared per decode and only ever
    /// filled for the paths that carry a widened edge, so a hint-free decode
    /// allocates nothing here.
    pub(crate) readings: String,
    /// Where each word of the path sits, as a range into the sweep's span arena.
    ///
    /// Refilled per decode by the same walk that writes the text; a single-word path
    /// records its one word, which is what makes "fewer than two" the coining guard, and a
    /// corrected path's spans are recorded too and ignored by the only reader, which
    /// refuses to coin from a spelling the user did not type.
    pub(crate) words: WordSpans,
}

impl Draft {
    /// An empty draft: no text, no score, the dictionary label.
    ///
    /// Growing a list to a longer decode pushes these, which allocates nothing.
    pub(super) fn empty() -> Self {
        Self {
            text: String::new(),
            source: CandidateSource::Dict,
            syllables: 0,
            score: 0,
            corrected: false,
            whole_sentence: false,
            readings: String::new(),
            words: WordSpans::EMPTY,
        }
    }

    /// The word spans of the path this draft came from, in path order, read out of the
    /// arena they were recorded in.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn words<'spans>(&self, spans: &'spans [WordSpan]) -> &'spans [WordSpan] {
        let start = usize::try_from(self.words.start).unwrap_or(usize::MAX);
        spans
            .get(start..)
            .and_then(|spans| spans.get(..usize::from(self.words.len)))
            .unwrap_or(&[])
    }

    /// Whether the path used a spelling-correction edge.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn corrected(&self) -> bool {
        self.corrected
    }

    /// The text the candidate commits.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// Merges the drafts that spell the same text, orders them best first, and keeps at most
/// `max_candidates` of them.
pub(crate) fn finish(drafts: &mut Vec<Draft>, max_candidates: u16) {
    dedupe(drafts);
    // A stable sort, so two drafts that score the same keep the order they were generated
    // in, which is the order of the paths they came from. `sort_by_key` is stable too, so
    // the tie-break is preserved by the `Reverse` rather than by the comparator's shape.
    drafts.sort_by_key(|draft| std::cmp::Reverse(draft.score));
    // Truncating keeps the capacity of the list itself; the text buffers of the drafts that
    // are dropped go with them, which is the price of a shorter candidate list.
    drafts.truncate(usize::from(max_candidates));
    // Scores are Q16.16 while the decay contract is Q8.8; shifting first keeps the
    // published step meaningful without changing candidate ordering.
    let dropped = crate::viterbi::kbest::retain_rank_decayed_sentences(
        drafts,
        |draft| draft.whole_sentence,
        |draft| draft.score >> 8,
    );
    if dropped > 0 {
        tracing::trace!(
            dropped_sentences = dropped,
            "rank decay truncated candidates"
        );
    }
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
    target.corrected |= other.corrected;
    if dictionary(target.source) && dictionary(other.source) && target.source != other.source {
        target.source = CandidateSource::UserDict;
        target.score = target.score.saturating_add(other.score);
        return;
    }
    if other.score > target.score {
        target.score = other.score;
        target.source = other.source;
        target.syllables = other.syllables;
        // The winning reading's own cut travels with it: the two drafts spell the same
        // text, but not necessarily out of the same words, and the pairs a commit coins
        // from are the winner's pairs. The range is a `Copy`, so the winner's spans are
        // adopted by value out of the shared arena.
        target.words = other.words;
        // The hint travels with the winning reading, for the same reason the spans
        // do: the standard readings are a fact about the path, and the surviving
        // candidate is the winning path's. The clone costs only on a merge that
        // changes the winner, which a steady-state decode has none of.
        target.readings = other.readings.clone();
    }
}
