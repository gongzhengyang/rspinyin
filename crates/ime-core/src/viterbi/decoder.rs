//! The decoder's configuration, and the two entry points that decode a request.
//!
//! Responsibility: hold the tunable knobs and the scorer built from them, and run one decode
//! from a request to the frozen [`DecodeResult`]. The work itself lives next to the data it
//! walks: the sweep ranks the paths and builds the candidates,
//! [`build_lattice`](crate::viterbi::build_lattice) reads the dictionary, and
//! [`DecodeScratch`] holds the buffers a decode fills, so what is left here is the order the
//! pieces are driven in.
//!
//! [`Decoder::decode`] is the convenience form, which builds a workspace for the call and
//! throws it away; [`Decoder::decode_into`] is the steady-state form, which a caller that
//! decodes once per keystroke uses to keep its buffers.
//!
//! Boundaries: a decode is a pure function of its arguments -- the request, the dictionary, the
//! user's frequencies and the language model, and nothing else. No file, no clock, no
//! environment, no global state, which is what makes the same input rank the same way on every
//! run and in every test.
//!
//! # The degraded answer
//!
//! A decode never answers with an empty candidate list. When there is no reading at all -- the
//! input has no segmentation, or no word of the dictionary covers it and the single-character
//! fallback is off, or the abbreviation walk that reads the input's letters instead finds
//! nothing either -- the answer is one pass-through candidate holding the raw input, with
//! `degraded` set. A window that renders nothing looks to the user like an input method that
//! has stopped responding, which is a worse failure than showing the text they typed.

use ime_types::{DecodeRequest, DecodeResult, ImeError, LanguageModel, Lexicon, UserFreqSource};

use crate::lm::{ScoreWeights, Scorer};
use crate::viterbi::scratch::{DecodeScratch, DecodeSources};

/// Widest beam a configuration may ask for.
pub const MAX_BEAM_K: u16 = 32;

/// Longest candidate list a configuration may ask for.
pub const MAX_CANDIDATES: u16 = 64;

/// Beam width the shipped configuration uses.
pub const DEFAULT_BEAM_K: u16 = 16;

/// Candidate limit the shipped configuration uses: five pages of nine.
pub const DEFAULT_MAX_CANDIDATES: u16 = 45;

/// First candidate-materialization quota: enough for the usual first screens.
pub const INITIAL_CANDIDATE_QUOTA: u16 = 24;

/// Second candidate-materialization quota, used when paging asks past the first.
pub const EXPANDED_CANDIDATE_QUOTA: u16 = INITIAL_CANDIDATE_QUOTA * 2;

/// Highest candidate-materialization quota.
pub const SUPPLY_CANDIDATE_CAP: u16 = MAX_CANDIDATES;

/// Returns the quota that can satisfy a paging request past `materialized`.
///
/// The ladder is deliberately not configurable: `24 -> 48 -> 64` keeps the common
/// keystroke cheap, doubles only when the user actually pages, and makes the last step
/// at the hard decoder cap. A list already shorter than the first quota still asks for
/// that quota again rather than inventing a fourth rung.
///
/// # Panics
///
/// Never panics.
pub const fn candidate_supply_quota(materialized: u16) -> u16 {
    if materialized < INITIAL_CANDIDATE_QUOTA {
        INITIAL_CANDIDATE_QUOTA
    } else if materialized < EXPANDED_CANDIDATE_QUOTA {
        EXPANDED_CANDIDATE_QUOTA
    } else {
        SUPPLY_CANDIDATE_CAP
    }
}

/// Reports whether a page request beyond `materialized` may expand the decode.
///
/// A request already covered by the materialized prefix needs no re-decode. The cap
/// always answers `false`: it is a supply ceiling, not a claim that exactly
/// [`SUPPLY_CANDIDATE_CAP`] candidates exist.
///
/// # Panics
///
/// Never panics.
pub const fn has_more_candidate_supply(materialized: u16, requested: u16) -> bool {
    requested > materialized && materialized < SUPPLY_CANDIDATE_CAP
}

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
/// `Send + Sync` and reentrant: a decode borrows the decoder immutably and its working
/// storage comes from the caller, so two threads could decode at once, each with a
/// workspace of its own. Nothing in the decoder is written by a decode.
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

    /// Returns the scorer built from the configuration's weights.
    ///
    /// The decode workspace's entry point needs it to build the sweep's sources. It is not
    /// part of the decoder's public surface, because a scorer read outside the decoder could
    /// be paired with weights the decoder was not built from.
    pub(super) fn scorer(&self) -> &Scorer {
        &self.scorer
    }

    /// Decodes one request into the ordered candidate list the window draws.
    ///
    /// The segmentation graph is built here, from `req.raw`. With
    /// [`DecodeFlags::USER_DICT`](ime_types::DecodeFlags::USER_DICT) clear, the user's own
    /// words and counts take no part in the ranking or in the labels; with
    /// [`DecodeFlags::ABBREV`](ime_types::DecodeFlags::ABBREV) set, the lattice is widened
    /// by the initial-letter readings of the input, so that `nh` reaches the word keyed
    /// `ni'hao` and `bjdx` the one keyed `bei'jing'da'xue`.
    ///
    /// This is the convenience form: it builds a workspace for the call and throws it away. A
    /// caller that decodes more than once -- which is every session, once per keystroke --
    /// keeps a [`DecodeScratch`] and calls [`Decoder::decode_into`] instead, which is what
    /// makes a steady-state decode allocate nothing but the lattice.
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
        let mut scratch = DecodeScratch::new();
        self.decode_into(
            &mut scratch,
            req,
            DecodeSources {
                lexicon: lx,
                user_freq: uf,
                lm,
                english: None,
                hints: None,
            },
        );
        scratch.into_result()
    }

    /// Decodes with one explicit candidate-materialization quota.
    ///
    /// This is the expansion hook for the supply ladder. It runs the same deterministic
    /// decode as [`Decoder::decode`] with only `max_candidates` replaced, so each wider
    /// quota contains the narrower result as a bit-identical prefix and introduces no
    /// duplicates. It is intended for the page-request path, not for every keystroke.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] when the base configuration or the requested quota is
    /// invalid, using the same rules as [`DecodeConfig::validate`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn decode_with_candidate_quota(
        &self,
        req: &DecodeRequest,
        lx: &dyn Lexicon,
        uf: &dyn UserFreqSource,
        lm: &dyn LanguageModel,
        quota: u16,
    ) -> Result<DecodeResult, ImeError> {
        let mut cfg = self.cfg;
        cfg.max_candidates = quota;
        Ok(Decoder::new(cfg)?.decode(req, lx, uf, lm))
    }
}

/// A user-frequency source that has recorded nothing.
///
/// Stands in for the real one when a request clears
/// [`DecodeFlags::USER_DICT`](ime_types::DecodeFlags::USER_DICT), so that neither the lattice
/// nor the scorer has to know the switch exists.
pub(super) struct Silent;

impl UserFreqSource for Silent {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXPANDED_CANDIDATE_QUOTA, INITIAL_CANDIDATE_QUOTA, MAX_CANDIDATES, DecodeRequest, Decoder,
        candidate_supply_quota, has_more_candidate_supply,
    };
    use crate::lm::InMemoryLm;
    use crate::viterbi::lattice::testing::{MockLexicon, NoUser};
    use crate::state::SessionConfig;

    #[test]
    fn test_candidate_supply_quota_has_the_documented_ladder() {
        assert_eq!(INITIAL_CANDIDATE_QUOTA, 24);
        assert_eq!(EXPANDED_CANDIDATE_QUOTA, 48);
        assert_eq!(MAX_CANDIDATES, 64);
        assert_eq!(candidate_supply_quota(0), 24);
        assert_eq!(candidate_supply_quota(24), 48);
        assert_eq!(candidate_supply_quota(48), 64);
        assert_eq!(candidate_supply_quota(64), 64);
    }

    #[test]
    fn test_candidate_supply_projection_matches_the_engine_ladder() {
        assert_eq!(
            SessionConfig::candidate_supply_quota(24),
            candidate_supply_quota(24)
        );
        assert_eq!(
            SessionConfig::has_more_candidate_supply(48, 49),
            has_more_candidate_supply(48, 49)
        );
    }

    #[test]
    fn test_candidate_quota_expansion_preserves_the_narrow_prefix_bit_for_bit() {
        let decoder = Decoder::default();
        let lexicon = MockLexicon::phrase();
        let lm = InMemoryLm::new();
        let request = DecodeRequest::new("wo'ai'ni");
        let decode = |quota| {
            decoder
                .decode_with_candidate_quota(&request, &lexicon, &NoUser, &lm, quota)
                .unwrap_or_else(|error| panic!("quota {quota} is valid: {error}"))
        };
        let initial = decode(24);
        let expanded = decode(48);
        assert!(!initial.candidates.is_empty());
        assert!(initial.candidates.len() <= expanded.candidates.len());
        assert_eq!(
            initial.candidates,
            expanded.candidates[..initial.candidates.len()],
            "the wider quota must retain the narrower list bit-for-bit"
        );
        assert_eq!(
            initial.candidates.first().map(|candidate| candidate.text.as_str()),
            expanded
                .candidates
                .first()
                .map(|candidate| candidate.text.as_str())
        );
    }
}
