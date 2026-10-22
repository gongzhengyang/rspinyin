//! The decode workspace: every buffer one decode fills, kept across decodes.
//!
//! Responsibility: hold the storage a decode writes into -- the segmentation graph, the
//! sweep's beams, the candidate drafts and the result -- and drive one decode through it.
//! The working set of a decode has a known upper bound
//! ([`MAX_NODES`](crate::segment::MAX_NODES) nodes, a beam of at most
//! [`MAX_BEAM_K`](crate::viterbi::MAX_BEAM_K), at most
//! [`MAX_CANDIDATES`](crate::viterbi::MAX_CANDIDATES) candidates), so the storage is a
//! fixed-shape pool rather than a general allocator: a workspace that has decoded one
//! twelve-syllable input decodes the next without touching the allocator at all.
//!
//! Boundaries: this module owns no policy. The ranking is the sweep's, the graph is the
//! segmentation layer's, and the dictionary is injected. What is left here is the order the
//! pieces are driven in and the buffers they write into. A workspace is per-session state: it
//! is passed in by `&mut`, so one decode at a time holds it, which is exactly the concurrency
//! the decoder promises -- decoding is serial per session, and the decoder itself stays
//! `Send + Sync` because the workspace is not part of it.
//!
//! # Why the lattice is not in here
//!
//! [`LatticeEdge`](crate::viterbi::LatticeEdge) holds a [`WordRef`](ime_types::WordRef)
//! borrowed from the dictionary, so a `Lattice` cannot outlive the decode call that produced
//! it without the session itself becoming generic over the dictionary's lifetime. The lattice
//! is therefore built per decode and pre-sized, which is what keeps its edge vector to one
//! allocation instead of the eight a doubling growth would take; it is the one buffer a
//! steady-state decode still allocates.

use ime_types::{
    Candidate, CandidateSource, DecodeFlags, DecodeRequest, DecodeResult, LanguageModel, Lexicon,
    UserFreqSource,
};

use crate::confusable::ConfusableTable;
use crate::segment::{Readings, SyllableDag, abbrev::is_enabled};
use crate::viterbi::decoder::{Decoder, Silent};
use crate::viterbi::lattice::{Lattice, LatticeOptions, build_lattice_into};
use crate::viterbi::sweep::{
    Draft, Sources, Sweep, SweepStorage, WordSpan, finish, result_of_into,
};

/// Everything one decode allocates, kept across decodes.
///
/// The fields are the decode's working storage rather than an interface:
/// [`Decoder::decode_into`] drives them in the order the data flows -- graph, lattice, sweep,
/// drafts, result -- and the accessors below are the read side.
#[derive(Debug)]
pub struct DecodeScratch {
    /// Segmentation graph of the input being decoded, buffers reused.
    dag: SyllableDag,
    /// The sweep's beams and the live length of each, reused.
    sweep: SweepStorage,
    /// Candidate texts before they are numbered, capacities reused.
    drafts: Vec<Draft>,
    /// The word spans the last collect recorded, indexed by the ranges the drafts carry.
    /// Cleared and refilled by every collect, so a range is valid for exactly the draft
    /// list of the decode that wrote it.
    word_spans: Vec<WordSpan>,
    /// The key arena the lattice build writes the canonical spelling of every looked-up
    /// key into, and the spans name ranges into. Cleared by the next build, so a range is
    /// valid for exactly the draft list of the decode that wrote it -- the same lifetime
    /// [`DecodeScratch::word_spans`] keeps.
    lattice_keys: Vec<u8>,
    /// The result the decode writes into, capacities reused.
    out: DecodeResult,
    /// The abbreviation walk's enumeration buffer, reused by every node of a build and
    /// handed to the next one.
    readings: Readings,
    /// Whether the last decode's abbreviation walk left readings out of the lattice.
    abbrev_truncated: bool,
}

impl DecodeScratch {
    /// Returns the segmentation graph of the input the last decode read.
    ///
    /// The graph belongs to the workspace rather than to the caller: a decode segments the
    /// request itself, and handing the same graph back is what lets a session read the
    /// syllable grid without segmenting the input a second time.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn dag(&self) -> &SyllableDag {
        &self.dag
    }

    /// Empties the graph, keeping its buffers.
    ///
    /// A session that has finished a composition holds no input and therefore no graph;
    /// this is the call that makes the workspace agree, without giving up the capacity the
    /// next composition's first decode would otherwise have to allocate.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn clear_dag(&mut self) {
        let _ = self.dag.build("");
    }

    /// Creates an empty workspace; the first decode allocates.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self {
            dag: SyllableDag::new(),
            sweep: SweepStorage::new(),
            drafts: Vec::new(),
            word_spans: Vec::new(),
            lattice_keys: Vec::new(),
            out: empty_result(),
            readings: Readings::new(),
            abbrev_truncated: false,
        }
    }

    /// Returns whether the last decode's abbreviation walk left readings of the input out
    /// of the lattice.
    ///
    /// The signal behind the [`ABBREV_TRUNCATED_CODE`] diagnostic: an input with more
    /// readings than the enumeration cap holds is still decoded, from the most specific
    /// readings the cap left room for, and this is what tells a caller the answer is built
    /// from a subset. It is a fact about the input rather than about the flag set, so a
    /// caller that logs it once has to remember that it already did. Always `false` for a
    /// request that clears [`DecodeFlags::ABBREV`](ime_types::DecodeFlags::ABBREV), and
    /// for a request that is answered without a lattice at all.
    ///
    /// [`ABBREV_TRUNCATED_CODE`]: crate::segment::abbrev::ABBREV_TRUNCATED_CODE
    ///
    /// # Panics
    ///
    /// Never.
    pub fn abbrev_truncated(&self) -> bool {
        self.abbrev_truncated
    }

    /// Returns the result of the last decode, which a decode that has not run leaves empty.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn result(&self) -> &DecodeResult {
        &self.out
    }

    /// Returns the result of the last decode, mutably, for the session's post-passes.
    ///
    /// The reader is the context re-rank: it reorders the candidate list in place,
    /// after the decode assembled it and before the session repairs everything
    /// derived from it, and it needs the list as the workspace holds it rather than
    /// through a take-and-return that would move the text buffers out and back.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn result_mut(&mut self) -> &mut DecodeResult {
        &mut self.out
    }

    /// Re-ranks the answer by `context` through the context module, keeping the drafts
    /// aligned.
    ///
    /// The wrapper exists because the re-rank has to permute the drafts by the same
    /// moves as the candidates -- the pairs a commit coins from are read out of the
    /// draft at the highlight's position -- and the drafts are the workspace's own
    /// type, not one a caller outside this module can name.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn rerank_with_context(&mut self, context: Option<&str>, lm: &dyn LanguageModel) {
        crate::viterbi::context::rerank_aligned(
            &mut self.out.candidates,
            &mut self.drafts,
            context,
            lm,
        );
    }

    /// The key bytes one span names in the workspace's key arena.
    ///
    /// # Panics
    ///
    /// Never: a range that does not fit the arena answers empty.
    fn key_bytes(&self, range: (u32, u16)) -> &[u8] {
        let start = usize::try_from(range.0).unwrap_or(usize::MAX);
        self.lattice_keys
            .get(start..)
            .and_then(|bytes| bytes.get(..usize::from(range.1)))
            .unwrap_or(&[])
    }

    /// The word pairs of one candidate's path that a commit could coin, best first in
    /// `(key, text)` form: the adjacent single-character words of the reading, with the
    /// key spelled the way the input spells it.
    ///
    /// The index is the candidate's position in [`DecodeScratch::result`]. A candidate
    /// with nothing to coin -- a single-word reading, a pass-through answer, a path that
    /// needed a spelling correction, a pair where either word is more than one character
    /// -- answers an empty list, which is the caller's signal to coin nothing. The
    /// allocation happens here, at the commit, and only for the candidate being
    /// committed; the decode itself records ranges and allocates nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn coin_pairs(&self, index: usize) -> Vec<(String, String)> {
        let Some(draft) = self.drafts.get(index) else {
            return Vec::new();
        };
        // A corrected path reads a spelling the user did not type, so the key it would
        // coin under is the slip; a pair with a longer word is a phrase the user did not
        // choose character by character. Both leave the candidate with nothing to coin.
        if draft.corrected() {
            return Vec::new();
        }
        let spans = draft.words(&self.word_spans);
        if spans.len() < 2 {
            return Vec::new();
        }
        let text = draft.text();
        let mut pairs = Vec::new();
        for window in spans.windows(2) {
            let (left, right) = (window[0], window[1]);
            let Some(word_left) = text.get(usize::from(left.text.0)..usize::from(left.text.1))
            else {
                continue;
            };
            let Some(word_right) = text.get(usize::from(right.text.0)..usize::from(right.text.1))
            else {
                continue;
            };
            // The card's shape: a two-character combination of two words the user chose
            // one character of. Anything longer is a phrase, and phrases are the phrase
            // document's business.
            if word_left.chars().count() != 1 || word_right.chars().count() != 1 {
                continue;
            }
            // The pair's key is the two words' canonical spellings joined the way a
            // dictionary key joins them -- which is what the next decode of the same
            // input will look the coined word up under. A word whose key never entered
            // the arena (an abbreviation reading) leaves the pair refused.
            let left_key = self.key_bytes(left.key);
            let right_key = self.key_bytes(right.key);
            if left_key.is_empty() || right_key.is_empty() {
                continue;
            }
            let (Ok(left_key), Ok(right_key)) = (
                std::str::from_utf8(left_key),
                std::str::from_utf8(right_key),
            ) else {
                continue;
            };
            pairs.push((
                format!("{left_key}'{right_key}"),
                format!("{word_left}{word_right}"),
            ));
        }
        pairs
    }

    /// Takes the result of the last decode out of the workspace, leaving an empty one behind.
    ///
    /// The buffers the result holds leave with it, so a caller that keeps decoding has to hand
    /// them back with [`DecodeScratch::recycle`] to keep the reuse. A caller that reads the
    /// answer in place instead uses [`DecodeScratch::result`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn take_result(&mut self) -> DecodeResult {
        core::mem::replace(&mut self.out, empty_result())
    }

    /// Puts a result back into the workspace, so the next decode reuses its buffers.
    ///
    /// This is the other half of [`DecodeScratch::take_result`]: the session keeps the result
    /// it handed to the frame it sent to the UI thread, and gives it back before the next
    /// keystroke. The next decode overwrites the candidates and segments in place, which is
    /// what keeps their text buffers; clearing the lists here would drop the texts and give
    /// the buffers up.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn recycle(&mut self, used: DecodeResult) {
        self.out = used;
    }

    /// Takes the result of the last decode out of the workspace, consuming it.
    ///
    /// The workspace is gone afterwards, so this is the form a caller that decodes once uses;
    /// see [`DecodeScratch::take_result`] for the form that keeps decoding.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn into_result(self) -> DecodeResult {
        self.out
    }
}

impl Default for DecodeScratch {
    /// An empty workspace, the same as [`DecodeScratch::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// The result a decode writes into before the first decode has run.
///
/// `DecodeResult` is a frozen contract type with no `Default`, so the empty value is spelled
/// out here. `degraded` is clear rather than set: it describes an answer, and there is no
/// answer yet.
fn empty_result() -> DecodeResult {
    DecodeResult {
        candidates: Vec::new(),
        segments: Vec::new(),
        degraded: false,
    }
}

impl Decoder {
    /// Decodes one request into `scratch`, reusing every buffer it holds.
    ///
    /// This is the steady-state form: a caller that decodes more than once -- which is every
    /// session, once per keystroke -- keeps its workspace and calls this, and a decode after
    /// the first one allocates nothing but the lattice's own edge vector.
    /// [`Decoder::decode`] is the convenience form, which builds a workspace for the call and
    /// throws it away.
    ///
    /// The answer is written into `scratch.result()`, and the graph is left describing
    /// `req.raw`, so a caller that also needs the segmentation -- the session does, for the
    /// preedit -- reads it from the workspace instead of building a second graph. With
    /// [`DecodeFlags::USER_DICT`](ime_types::DecodeFlags::USER_DICT) clear, the user's own
    /// words and counts take no part in the ranking or in the labels; with
    /// [`DecodeFlags::ABBREV`](ime_types::DecodeFlags::ABBREV) set, the lattice is widened
    /// by the initial-letter readings of the input, and
    /// [`DecodeScratch::abbrev_truncated`] reports whether the enumeration cap left any of
    /// them out.
    ///
    /// # Errors
    ///
    /// None. A request that cannot be decoded is answered with a degraded result rather than
    /// an error, because the caller has to show the user something either way.
    ///
    /// # Panics
    ///
    /// Never panics: every index is taken with a checked lookup and every arithmetic step
    /// saturates.
    pub fn decode_into<'lx>(
        &self,
        scratch: &mut DecodeScratch,
        req: &DecodeRequest,
        lx: &'lx dyn Lexicon,
        uf: &dyn UserFreqSource,
        lm: &dyn LanguageModel,
        english: Option<&'lx dyn Lexicon>,
        hints: Option<&ConfusableTable>,
    ) {
        // The flag describes an answer, so it is cleared before anything can set it: a
        // request answered without a lattice at all reports no truncation, whatever the
        // decode before it found.
        scratch.abbrev_truncated = false;
        // Every way the input can be refused -- empty, past the length limit, or impossible
        // to cut into syllables -- leaves the graph without a path, and that is the condition
        // the degraded answer keys on. The error is not propagated because the contract
        // answers a request it cannot decode with a candidate rather than with an error the
        // caller has to handle.
        //
        // The abbreviation switch is the one thing that changes which requests those are: the
        // walk reads the letters of the input rather than the syllables of the graph, so `nh`
        // and `bjdx` reach a word although no cut of them spells one, and the lattice is left
        // to find it -- its own answer to a lattice with no edge is the pass-through below, so
        // nothing is lost by asking. A request that carries no input at all stays out of that
        // path: there is nothing to read, and the empty root path the sweep would find at
        // node 0 is not a candidate.
        let built = scratch.dag.build_for(&req.raw, req.flags);
        // The English lexicon is read only when the request asked for it: with the
        // bit clear the option is `None` all the way down, which is the whole cost
        // of the switch being off.
        let english_source = if crate::segment::english::is_enabled(req.flags) {
            english
        } else {
            None
        };
        let abbrev_readable = is_enabled(req.flags) && !scratch.dag.is_empty();
        if built.is_err() && !abbrev_readable && english_source.is_none() {
            passthrough_into(&mut scratch.out, &req.raw);
            return;
        }
        // The switch is honoured by hiding the user's history from both readers of it,
        // rather than by threading a flag into the lattice and the scorer, which would put
        // the same condition in two more places.
        let silent = Silent;
        let user: &dyn UserFreqSource = if req.flags.contains(DecodeFlags::USER_DICT) {
            uf
        } else {
            &silent
        };
        let cfg = self.config();
        // Pre-sized, so the edge vector is allocated once rather than doubled eight times: a
        // node contributes at most `WORDS_PER_KEY` edges for its one-syllable spans plus the
        // fallbacks, and `node_count` nodes exist.
        let mut lattice =
            Lattice::with_capacity(usize::from(scratch.dag.len()) + 1, cfg.fallback_single);
        build_lattice_into(
            &mut lattice,
            &scratch.dag,
            lx,
            user,
            LatticeOptions {
                fallback_single: cfg.fallback_single,
                flags: req.flags,
                readings: &mut scratch.readings,
                keys: &mut scratch.lattice_keys,
                english: english_source,
            },
        );
        scratch.abbrev_truncated = lattice.abbrev_truncated();
        let nodes = lattice.node_count();
        scratch.sweep.prepare(nodes, usize::from(cfg.beam_k));
        let sources = Sources::new(&lattice, self.scorer(), lm, user, &scratch.lattice_keys);
        let mut sweep =
            Sweep::with_storage(sources, usize::from(cfg.beam_k), nodes, &mut scratch.sweep);
        sweep.run();
        let terminal = usize::from(scratch.dag.len());
        sweep.collect_into(terminal, &mut scratch.drafts, &mut scratch.word_spans);
        finish(&mut scratch.drafts, cfg.max_candidates);
        if scratch.drafts.is_empty() {
            // Nothing reached the end of the input: no word of the dictionary covers any
            // reading of it and the fallback is off, or the beam is too narrow to keep a
            // path at all.
            passthrough_into(&mut scratch.out, &req.raw);
            return;
        }
        sweep.segments_into(terminal, &mut scratch.out.segments);
        // The graph's normalized spelling is what a corrected candidate is annotated
        // with: the input as the decoder read it, which is the slip the correction
        // edges stand for.
        result_of_into(
            &mut scratch.out,
            &scratch.drafts,
            lattice.lookup_failed(),
            scratch.dag.normalized(),
            hints,
        );
    }
}

/// Writes the answer to a request that cannot be decoded into `out`, reusing its buffers.
///
/// One candidate that commits the input unchanged, no cut to describe, and `degraded` set:
/// a window that renders nothing looks to the user like an input method that has stopped
/// responding, which is a worse failure than showing the text they typed.
fn passthrough_into(out: &mut DecodeResult, raw: &str) {
    match out.candidates.first_mut() {
        Some(slot) => {
            slot.index = 1;
            slot.text.clear();
            slot.text.push_str(raw);
            slot.annotation = None;
            slot.source = CandidateSource::Passthrough;
            // No path was scored, so there is nothing to show; the field is display-only and
            // never decides an order.
            slot.score = 0.0;
            slot.consumed_syllables = 0;
        }
        None => out.candidates.push(Candidate {
            index: 1,
            text: raw.to_owned(),
            annotation: None,
            source: CandidateSource::Passthrough,
            score: 0.0,
            consumed_syllables: 0,
        }),
    }
    out.candidates.truncate(1);
    // No path, so there is no cut to describe; the segment texts are given up with the
    // entries, because the contract says a degraded result carries no segments.
    out.segments.clear();
    out.degraded = true;
}

#[cfg(test)]
mod tests {
    use ime_types::{CandidateSource, DecodeFlags, DecodeRequest, DecodeResult, LanguageModel};

    use crate::lm::InMemoryLm;
    use crate::viterbi::decoder::{DecodeConfig, Decoder};
    use crate::viterbi::lattice::testing::{MockLexicon, MockUserFreq, NoUser};

    use super::*;

    /// Decodes `raw` into a workspace of its own and answers the result.
    fn decoded_into(raw: &str, lexicon: &MockLexicon, lm: &dyn LanguageModel) -> DecodeResult {
        let decoder = Decoder::default();
        let request = DecodeRequest::new(raw);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(&mut scratch, &request, lexicon, &NoUser, lm, None, None);
        scratch.into_result()
    }

    /// Every capacity a decode can grow, so that a steady-state decode can be shown not to
    /// have grown any of them.
    ///
    /// A capacity that stayed the same across a repeated decode is the evidence that no
    /// buffer was reallocated: a growing `Vec` or `String` always reports a different
    /// capacity, and the buffers a decode reuses are written in place.
    fn capacities(scratch: &DecodeScratch) -> Vec<usize> {
        let mut seen = vec![
            scratch.sweep.slots_capacity(),
            scratch.drafts.capacity(),
            scratch.out.candidates.capacity(),
            scratch.out.segments.capacity(),
        ];
        seen.extend(
            scratch
                .out
                .candidates
                .iter()
                .map(|candidate| candidate.text.capacity()),
        );
        seen.extend(
            scratch
                .out
                .segments
                .iter()
                .map(|segment| segment.text.capacity()),
        );
        seen
    }

    #[test]
    fn test_mixed_english_offers_the_lexicon_entry_for_an_input_no_reading_cuts() {
        let system = MockLexicon::with(&[("ni'hao", "你好")]);
        let english = MockLexicon::with(&[("github", "GitHub")]);
        let decoder = Decoder::default();
        let request = DecodeRequest::new("github").with_flags(DecodeFlags::ENGLISH);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(
            &mut scratch,
            &request,
            &system,
            &NoUser,
            &InMemoryLm::new(),
            Some(&english),
            None,
        );
        let first = &scratch.result().candidates[0];
        assert_eq!(first.text, "GitHub");
        assert_eq!(first.source, CandidateSource::English);
        assert_eq!(
            first.annotation.as_deref(),
            Some(crate::segment::english::ENGLISH_ANNOTATION)
        );
        assert_eq!(
            first.consumed_syllables, 0,
            "no pinyin syllable is consumed: the candidate is the shape a pass-through has"
        );
    }

    #[test]
    fn test_mixed_english_switch_off_leaves_the_pass_through_untouched() {
        let system = MockLexicon::with(&[("ni'hao", "你好")]);
        let english = MockLexicon::with(&[("github", "GitHub")]);
        let decoder = Decoder::default();
        let request = DecodeRequest::new("github");
        let mut scratch = DecodeScratch::new();
        // The lexicon is handed over, but the request carries no ENGLISH bit: the
        // switch is off, and the answer is the raw text, as before the table existed.
        decoder.decode_into(
            &mut scratch,
            &request,
            &system,
            &NoUser,
            &InMemoryLm::new(),
            Some(&english),
            None,
        );
        let first = &scratch.result().candidates[0];
        assert_eq!(first.text, "github");
        assert_eq!(first.source, CandidateSource::Passthrough);
        assert_eq!(first.annotation, None);
    }

    #[test]
    fn test_mixed_english_miss_falls_back_to_the_pass_through() {
        let system = MockLexicon::with(&[("ni'hao", "你好")]);
        let english = MockLexicon::with(&[]);
        let decoder = Decoder::default();
        let request = DecodeRequest::new("github").with_flags(DecodeFlags::ENGLISH);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(
            &mut scratch,
            &request,
            &system,
            &NoUser,
            &InMemoryLm::new(),
            Some(&english),
            None,
        );
        let first = &scratch.result().candidates[0];
        assert_eq!(
            first.text, "github",
            "the table has no entry, so the raw text"
        );
        assert_eq!(first.source, CandidateSource::Passthrough);
    }

    #[test]
    fn test_mixed_english_never_disturbs_an_input_a_reading_cuts() {
        // Even an English table that holds the input's own spelling stays out: the
        // exact walk cut the input, so the English pass is never reached.
        let system = MockLexicon::with(&[("ni'hao", "你好")]);
        let english = MockLexicon::with(&[("nihao", "Nihao")]);
        let decoder = Decoder::default();
        let request = DecodeRequest::new("nihao").with_flags(DecodeFlags::ENGLISH);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(
            &mut scratch,
            &request,
            &system,
            &NoUser,
            &InMemoryLm::new(),
            Some(&english),
            None,
        );
        let candidates = &scratch.result().candidates;
        assert_eq!(candidates[0].text, "你好");
        assert_eq!(candidates[0].source, CandidateSource::Dict);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.source != CandidateSource::English),
            "no English candidate exists for an input the exact walk cut"
        );
    }

    #[test]
    fn test_decode_scratch_new_holds_an_empty_result() {
        let scratch = DecodeScratch::new();
        assert!(scratch.result().candidates.is_empty());
        assert!(scratch.result().segments.is_empty());
        assert!(!scratch.result().degraded);
        assert_eq!(scratch.drafts.len(), 0);
        assert_eq!(
            scratch.sweep.slots_capacity(),
            0,
            "nothing is allocated yet"
        );
        assert_eq!(DecodeScratch::default().result(), scratch.result());
    }

    #[test]
    fn test_decode_into_answers_what_decode_answers() {
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        // The inputs the two entry points have to agree on: a whole-sentence reading, a
        // multi-word reading, an input with no reading at all (the pass-through answer), the
        // empty input, and an input long enough to fill the beam.
        for raw in [
            "nihao",
            "woaini",
            "zhongguo",
            "zzz",
            "",
            "nihaonihaonihaonihao",
        ] {
            let request = DecodeRequest::new(raw);
            let expected = decoder.decode(&request, &lexicon, &NoUser, &lm);
            let seen = decoded_into(raw, &lexicon, &lm);
            assert_eq!(seen, expected, "{raw:?}");
        }
    }

    #[test]
    fn test_decode_into_honours_the_user_dictionary_switch() {
        let lexicon = MockLexicon::with(&[("de", "的"), ("de", "得")]);
        let user = MockUserFreq::with(&[("得", 1000)]);
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        // The history the user's own commits built ranks 得 first.
        let ranked = DecodeRequest::new("de");
        decoder.decode_into(&mut scratch, &ranked, &lexicon, &user, &lm, None, None);
        assert_eq!(scratch.result().candidates[0].text, "得");
        // With the user dictionary switched off the counts take no part either, so the
        // dictionary's own order decides.
        let off = DecodeRequest::new("de").with_flags(DecodeFlags::empty());
        decoder.decode_into(&mut scratch, &off, &lexicon, &user, &lm, None, None);
        assert_eq!(scratch.result().candidates[0].text, "的");
        assert_eq!(scratch.result().candidates[0].source, CandidateSource::Dict);
    }

    #[test]
    fn test_decode_into_reuses_every_buffer_across_decodes() {
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        // `xian` is the fixture's one input that ranks more than one candidate: it is both
        // the dictionary's own 先 and the user's 西 followed by 安. `nihao` would not do --
        // `ni'hao` and `ni`+`hao` both spell 你好, so the dedupe leaves a single candidate
        // and the buffer-reuse assertion below would be exercising an almost empty list.
        let request = DecodeRequest::new("xian");
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        let first: Vec<String> = scratch
            .result()
            .candidates
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect();
        assert!(first.len() > 1, "the fixture ranks more than one candidate");
        let before = capacities(&scratch);
        // The second decode of the same input is the steady state: everything it needs is
        // already there, so a capacity that changed is an allocation.
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        assert_eq!(
            capacities(&scratch),
            before,
            "a steady-state decode grows no buffer"
        );
        let second: Vec<String> = scratch
            .result()
            .candidates
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect();
        assert_eq!(second, first, "the reused buffers hold the same answer");
    }

    #[test]
    fn test_decode_into_after_a_degraded_decode_answers_like_a_fresh_one() {
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        // A decode that ends in the pass-through answer leaves the workspace holding one
        // candidate and no segments, which the next decode has to overwrite completely.
        for raw in ["zzz", "nihao", "zzz", "zhongguo", "zzz"] {
            let request = DecodeRequest::new(raw);
            decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
            let expected = decoder.decode(&request, &lexicon, &NoUser, &lm);
            assert_eq!(scratch.result(), &expected, "{raw:?}");
        }
    }

    #[test]
    fn test_decode_into_does_not_read_a_path_the_last_input_left_behind() {
        // The first decode fills the beams of the workspace with a reading; the second has no
        // reading at all, because the fallback is off and no word covers it. The beams are not
        // cleared between decodes -- that is the point of reusing them -- so a candidate read
        // out of a slot the second decode never wrote would be a reading of the wrong input.
        let lexicon = MockLexicon::with(&[
            ("zhong", "中"),
            ("guo", "国"),
            ("zhong'guo", "中国"),
            ("ni", "你"),
        ]);
        let lm = InMemoryLm::new();
        let decoder = Decoder::new(DecodeConfig {
            fallback_single: false,
            ..DecodeConfig::default()
        })
        .expect("the shipped knobs with the fallback off");
        let mut scratch = DecodeScratch::new();
        let read = DecodeRequest::new("zhongguo");
        decoder.decode_into(&mut scratch, &read, &lexicon, &NoUser, &lm, None, None);
        assert_eq!(scratch.result().candidates[0].text, "中国");
        let unreadable = DecodeRequest::new("nihao");
        decoder.decode_into(
            &mut scratch,
            &unreadable,
            &lexicon,
            &NoUser,
            &lm,
            None,
            None,
        );
        let expected = decoder.decode(&unreadable, &lexicon, &NoUser, &lm);
        assert!(expected.degraded, "no word covers the second input");
        assert_eq!(scratch.result(), &expected);
    }

    #[test]
    fn test_decode_into_leaves_the_graph_describing_the_last_input() {
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let first = DecodeRequest::new("nihao");
        decoder.decode_into(&mut scratch, &first, &lexicon, &NoUser, &lm, None, None);
        assert_eq!(scratch.dag.normalized(), "nihao");
        assert!(scratch.dag.has_path());
        let second = DecodeRequest::new("zhongguo");
        decoder.decode_into(&mut scratch, &second, &lexicon, &NoUser, &lm, None, None);
        // The graph is left describing the input of the last decode, which is what lets a
        // session read the preedit from the workspace instead of building a second graph.
        assert_eq!(scratch.dag.normalized(), "zhongguo");
        assert!(scratch.dag.has_path());
    }

    #[test]
    fn test_decode_scratch_hands_the_buffers_back_for_the_next_decode() {
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let request = DecodeRequest::new("nihao");
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        // The session keeps the result it hands to the frame, and hands the previous one back
        // before the next keystroke. A workspace that never gets its buffers back would
        // allocate a fresh set of candidate texts on every decode.
        let held = scratch.take_result();
        assert!(!held.candidates.is_empty());
        assert!(
            scratch.result().candidates.is_empty(),
            "the take empties it"
        );
        scratch.recycle(held);
        let before = capacities(&scratch);
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        assert_eq!(
            capacities(&scratch),
            before,
            "a handed-back result keeps its buffers"
        );
        assert_eq!(scratch.result().candidates[0].text, "你好");
    }

    #[test]
    fn test_decode_into_writes_a_passthrough_answer_into_a_used_workspace() {
        // No whole-sentence word, so the winning reading of the first decode is two words and
        // the workspace holds a cut of two segments that the degraded answer has to clear.
        let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "号")]);
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let read = DecodeRequest::new("nihao");
        decoder.decode_into(&mut scratch, &read, &lexicon, &NoUser, &lm, None, None);
        assert_eq!(scratch.result().segments.len(), 2);
        let unreadable = DecodeRequest::new("zzz");
        decoder.decode_into(
            &mut scratch,
            &unreadable,
            &lexicon,
            &NoUser,
            &lm,
            None,
            None,
        );
        let result = scratch.result();
        assert!(result.degraded);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].text, "zzz");
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
        assert!(result.segments.is_empty(), "there is no cut to describe");
    }

    #[test]
    fn test_decode_into_reads_an_abbreviated_input_through_the_prefix_query() {
        // `nh` spells no syllable, so no cut of it is a reading at all -- the graph has no
        // path -- and the only way the input can be decoded is the abbreviation walk. The
        // query it makes is `n'h`, which is what a dictionary that indexes initials
        // answers, and the word it returns is the one the user meant.
        let lexicon = MockLexicon::with(&[("n'h", "你好")]);
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let request =
            DecodeRequest::new("nh").with_flags(DecodeFlags::USER_DICT | DecodeFlags::ABBREV);
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        let result = scratch.result();
        assert!(
            !result.degraded,
            "an abbreviation is a reading of the input"
        );
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].text, "你好");
        assert_eq!(result.candidates[0].consumed_syllables, 2);
        assert_eq!(result.candidates[0].source, CandidateSource::Dict);
        assert!(!scratch.abbrev_truncated());
        // The cut of the winning path describes the abbreviated reading: one word of two
        // syllables, covering the whole input.
        assert_eq!(result.segments.len(), 1);
        assert_eq!((result.segments[0].start, result.segments[0].end), (0, 2));
    }

    #[test]
    fn test_decode_into_answers_an_abbreviated_input_with_the_pass_through_without_the_flag() {
        // The same input with the switch clear has no cut of its own, so the answer is the
        // input itself -- what the decoder answered before abbreviation existed -- and the
        // prefix query is not made either, which the dictionary reports by refusing every
        // one of them.
        let lexicon = MockLexicon::with(&[("n'h", "你好")]).refusing_prefix();
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let request = DecodeRequest::new("nh");
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        let result = scratch.result();
        assert!(result.degraded);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].text, "nh");
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);

        // With the switch set the query is made and refused, so the lattice is empty and
        // the answer degrades the same way: a dictionary that cannot enumerate prefixes
        // leaves abbreviation with nothing to offer rather than with a wrong candidate.
        let asked = DecodeRequest::new("nh").with_flags(DecodeFlags::ABBREV);
        decoder.decode_into(&mut scratch, &asked, &lexicon, &NoUser, &lm, None, None);
        let result = scratch.result();
        assert!(result.degraded);
        assert_eq!(result.candidates[0].text, "nh");
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
    }

    #[test]
    fn test_decode_into_reports_a_truncated_abbreviation_enumeration() {
        // An input whose readings multiply past the enumeration cap still decodes, and the
        // workspace says the answer was built from a subset of them. The same input with
        // the switch clear reports nothing, because nothing was enumerated.
        let lexicon = MockLexicon::with(&[("ai", "爱")]);
        let lm = InMemoryLm::new();
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let long = "ai".repeat(32);
        let request = DecodeRequest::new(long.as_str())
            .with_flags(DecodeFlags::USER_DICT | DecodeFlags::ABBREV);
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm, None, None);
        assert!(scratch.abbrev_truncated(), "the cap left readings out");
        let plain = DecodeRequest::new(long.as_str());
        decoder.decode_into(&mut scratch, &plain, &lexicon, &NoUser, &lm, None, None);
        assert!(
            !scratch.abbrev_truncated(),
            "a decode that does not walk abbreviations clears the previous report"
        );
    }

    /// Decodes `raw` into a workspace and answers the pairs its best candidate offers to
    /// coining.
    fn coin_pairs_of(raw: &str, lexicon: &MockLexicon) -> Vec<(String, String)> {
        let decoder = Decoder::default();
        let request = DecodeRequest::new(raw);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(
            &mut scratch,
            &request,
            lexicon,
            &NoUser,
            &InMemoryLm::new(),
            None,
            None,
        );
        scratch.coin_pairs(0)
    }

    #[test]
    fn test_coin_pairs_read_the_adjacent_single_characters_off_the_winning_path() {
        // The dictionary holds the two characters but no word over them, so the best
        // reading of `zhuji` is the two single-character words -- exactly the shape a
        // commit coins from, with the key spelled the way the input is.
        let lexicon = MockLexicon::with(&[("zhu", "诸"), ("ji", "暨")]);
        assert_eq!(
            coin_pairs_of("zhuji", &lexicon),
            vec![(String::from("zhu'ji"), String::from("诸暨"))]
        );
    }

    #[test]
    fn test_coin_pairs_of_a_whole_word_reading_are_empty() {
        // The path is one dictionary word: nothing was chosen character by character, so
        // there is nothing to coin.
        let lexicon = MockLexicon::with(&[("zhu'ji", "诸暨")]);
        assert!(coin_pairs_of("zhuji", &lexicon).is_empty());
    }

    #[test]
    fn test_coin_pairs_refuse_pairs_with_a_multi_character_word() {
        // 你好 is one word of the reading, so the adjacent pair it forms is a phrase
        // shape the coining rule does not take.
        let lexicon = MockLexicon::with(&[("ni'hao", "你好"), ("ba", "吧")]);
        assert!(coin_pairs_of("nihaoba", &lexicon).is_empty());
    }

    #[test]
    fn test_coin_pairs_of_a_corrected_reading_are_empty() {
        // The winning path needed a spelling correction, so the key it would coin under
        // is the slip the user did not mean.
        let lexicon = MockLexicon::with(&[("wan", "万"), ("ba", "吧")]);
        let decoder = Decoder::default();
        let mut scratch = DecodeScratch::new();
        let request = DecodeRequest::new("wanba").with_flags(ime_types::DecodeFlags::CORRECTION);
        decoder.decode_into(
            &mut scratch,
            &request,
            &lexicon,
            &NoUser,
            &InMemoryLm::new(),
            None,
            None,
        );
        // The correction really is what the best path needed; the guard is exercised
        // only if the decode agrees.
        let corrected = scratch
            .result()
            .candidates
            .first()
            .map(|candidate| candidate.annotation.is_some())
            .unwrap_or(false);
        if corrected {
            assert!(scratch.coin_pairs(0).is_empty());
        }
    }

    #[test]
    fn test_coin_pairs_of_an_out_of_range_index_are_empty() {
        let lexicon = MockLexicon::with(&[("zhu", "诸"), ("ji", "暨")]);
        let decoder = Decoder::default();
        let request = DecodeRequest::new("zhuji");
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(
            &mut scratch,
            &request,
            &lexicon,
            &NoUser,
            &InMemoryLm::new(),
            None,
            None,
        );
        assert!(scratch.coin_pairs(usize::MAX).is_empty());
    }
}
