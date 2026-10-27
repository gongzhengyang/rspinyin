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

/// The sources one decode reads, handed to [`Decoder::decode_into`] as one bundle.
///
/// One struct rather than five trailing parameters because the five travel together -- every
/// caller hands the same bundle, and a call whose tail was bare `None`s left a reader to the
/// signature to learn what had been left out. The lexicons share the lifetime the lattice
/// borrows the dictionary under, so they carry `'lx`; the frequency source, the language
/// model and the hint table are only read within the call and carry `'a`.
pub struct DecodeSources<'a, 'lx> {
    /// The syllable lexicon the segmentation graph is cut against.
    pub lexicon: &'lx dyn Lexicon,
    /// The user's own frequencies, consulted only when the request carries the bit.
    pub user_freq: &'a dyn UserFreqSource,
    /// The language model the lattice scores with.
    pub lm: &'a dyn LanguageModel,
    /// The English lexicon, consulted only when the request carries the bit.
    pub english: Option<&'lx dyn Lexicon>,
    /// The confusable table, consulted for known wrong forms when one is loaded.
    pub hints: Option<&'a ConfusableTable>,
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
    pub fn decode_into<'a, 'lx>(
        &self,
        scratch: &mut DecodeScratch,
        req: &DecodeRequest,
        sources: DecodeSources<'a, 'lx>,
    ) {
        let DecodeSources {
            lexicon: lx,
            user_freq: uf,
            lm,
            english,
            hints,
        } = sources;
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
mod tests;
