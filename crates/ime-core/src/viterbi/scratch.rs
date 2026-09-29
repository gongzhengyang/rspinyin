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

use crate::segment::SyllableDag;
use crate::viterbi::decoder::{Decoder, Silent};
use crate::viterbi::lattice::{Lattice, build_lattice_into};
use crate::viterbi::sweep::{Draft, Sources, Sweep, SweepStorage, finish, result_of_into};

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
    /// The result the decode writes into, capacities reused.
    out: DecodeResult,
}

impl DecodeScratch {
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
            out: empty_result(),
        }
    }

    /// Returns the result of the last decode, which a decode that has not run leaves empty.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn result(&self) -> &DecodeResult {
        &self.out
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
    /// words and counts take no part in the ranking or in the labels.
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
    pub fn decode_into(
        &self,
        scratch: &mut DecodeScratch,
        req: &DecodeRequest,
        lx: &dyn Lexicon,
        uf: &dyn UserFreqSource,
        lm: &dyn LanguageModel,
    ) {
        // Every way the input can be refused -- empty, past the length limit, or impossible
        // to cut into syllables -- leaves the graph without a path, and that is the condition
        // the degraded answer keys on. The error is not propagated because the contract
        // answers a request it cannot decode with a candidate rather than with an error the
        // caller has to handle.
        if scratch.dag.build(&req.raw).is_err() || !scratch.dag.has_path() {
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
        build_lattice_into(&mut lattice, &scratch.dag, lx, user, cfg.fallback_single);
        let nodes = lattice.node_count();
        scratch.sweep.prepare(nodes, usize::from(cfg.beam_k));
        let sources = Sources::new(&lattice, self.scorer(), lm, user);
        let mut sweep =
            Sweep::with_storage(sources, usize::from(cfg.beam_k), nodes, &mut scratch.sweep);
        sweep.run();
        let terminal = usize::from(scratch.dag.len());
        sweep.collect_into(terminal, &mut scratch.drafts);
        finish(&mut scratch.drafts, cfg.max_candidates);
        if scratch.drafts.is_empty() {
            // Nothing reached the end of the input: no word of the dictionary covers any
            // reading of it and the fallback is off, or the beam is too narrow to keep a
            // path at all.
            passthrough_into(&mut scratch.out, &req.raw);
            return;
        }
        sweep.segments_into(terminal, &mut scratch.out.segments);
        result_of_into(&mut scratch.out, &scratch.drafts, lattice.lookup_failed());
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
    use ime_types::{DecodeFlags, DecodeRequest, DecodeResult, LanguageModel};

    use crate::lm::InMemoryLm;
    use crate::viterbi::decoder::{DecodeConfig, Decoder};
    use crate::viterbi::lattice::testing::{MockLexicon, MockUserFreq, NoUser};

    use super::*;

    /// Decodes `raw` into a workspace of its own and answers the result.
    fn decoded_into(raw: &str, lexicon: &MockLexicon, lm: &dyn LanguageModel) -> DecodeResult {
        let decoder = Decoder::default();
        let request = DecodeRequest::new(raw);
        let mut scratch = DecodeScratch::new();
        decoder.decode_into(&mut scratch, &request, lexicon, &NoUser, lm);
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
        decoder.decode_into(&mut scratch, &ranked, &lexicon, &user, &lm);
        assert_eq!(scratch.result().candidates[0].text, "得");
        // With the user dictionary switched off the counts take no part either, so the
        // dictionary's own order decides.
        let off = DecodeRequest::new("de").with_flags(DecodeFlags::empty());
        decoder.decode_into(&mut scratch, &off, &lexicon, &user, &lm);
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
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm);
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
            decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &read, &lexicon, &NoUser, &lm);
        assert_eq!(scratch.result().candidates[0].text, "中国");
        let unreadable = DecodeRequest::new("nihao");
        decoder.decode_into(&mut scratch, &unreadable, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &first, &lexicon, &NoUser, &lm);
        assert_eq!(scratch.dag.normalized(), "nihao");
        assert!(scratch.dag.has_path());
        let second = DecodeRequest::new("zhongguo");
        decoder.decode_into(&mut scratch, &second, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &request, &lexicon, &NoUser, &lm);
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
        decoder.decode_into(&mut scratch, &read, &lexicon, &NoUser, &lm);
        assert_eq!(scratch.result().segments.len(), 2);
        let unreadable = DecodeRequest::new("zzz");
        decoder.decode_into(&mut scratch, &unreadable, &lexicon, &NoUser, &lm);
        let result = scratch.result();
        assert!(result.degraded);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].text, "zzz");
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
        assert!(result.segments.is_empty(), "there is no cut to describe");
    }
}
