//! K-best Viterbi decoding: the word lattice, the bounded merge and the decoder.
//!
//! Responsibility: turn one decode request into the ordered candidate list the window
//! draws. Three pieces, in the order the data flows through them:
//!
//! - [`lattice`] reads the dictionary and builds the graph of every word covering every
//!   span of the input's segmentation graph.
//! - [`kbest`] holds [`TopK`], the bounded merge each node's best paths are kept with.
//! - [`decoder`] walks the lattice in byte order, scores every edge with the
//!   fixed-point [`Scorer`](crate::lm::Scorer), and reads the paths that reach the last
//!   node back into candidates.
//!
//! Boundaries: this module is pure. It reaches the dictionary, the user's frequencies and
//! the language model through the trait objects of the frozen contract crate and touches
//! no file, clock, environment variable or global state, which is what lets a test drive
//! a whole decode from literals. It does not render, does not page and does not decide
//! what the user commits; the session state machine above it does that.
//!
//! # Determinism
//!
//! The same input, the same dictionary and the same configuration produce a
//! byte-identical candidate order, on every run and in every process. Each part of that
//! promise is documented next to what constrains it: [`decoder`] lists the four rules the
//! sweep follows, and [`TopK`] explains the tie-breaking it relies on.
//!
//! # Examples
//!
//! ```
//! use ime_core::viterbi::Decoder;
//! use ime_types::{
//!     CandidateSource, DecodeRequest, ImeError, LanguageModel, Lexicon, SyllableId,
//!     UserFreqSource, WordIter,
//! };
//!
//! /// A dictionary with nothing in it, and a model with nothing to say.
//! struct Nothing;
//!
//! impl Lexicon for Nothing {
//!     fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
//!         Ok(WordIter::from_vec(Vec::new()))
//!     }
//!     fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
//!         Err(ImeError::Unsupported)
//!     }
//!     fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
//!         Ok(WordIter::from_vec(Vec::new()))
//!     }
//! }
//!
//! impl UserFreqSource for Nothing {
//!     fn freq(&self, _key: &str) -> u32 { 0 }
//!     fn record(&self, _key: &str, _weight_hint: u16) {}
//!     fn is_user_word(&self, _key: &str) -> bool { false }
//! }
//!
//! impl LanguageModel for Nothing {
//!     fn unigram(&self, _word: &str) -> i32 { 0 }
//!     fn bigram(&self, _prev: &str, _word: &str) -> i32 { 0 }
//! }
//!
//! let result = Decoder::default().decode(
//!     &DecodeRequest::new("nihao"),
//!     &Nothing,
//!     &Nothing,
//!     &Nothing,
//! );
//! // Nothing can read the input, so the answer is the input itself: a decode never
//! // answers with an empty candidate list.
//! assert_eq!(result.candidates.len(), 1);
//! assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
//! assert_eq!(result.candidates[0].text, "nihao");
//! assert!(result.degraded);
//! ```

pub mod decoder;
pub mod kbest;
pub mod lattice;

pub use crate::viterbi::decoder::{
    DEFAULT_BEAM_K, DEFAULT_MAX_CANDIDATES, DecodeConfig, Decoder, MAX_BEAM_K, MAX_CANDIDATES,
};
pub use crate::viterbi::kbest::TopK;
pub use crate::viterbi::lattice::{
    FALLBACK_SINGLES, Lattice, LatticeEdge, MAX_LATTICE_NODES, MAX_WORD_SYLLABLES, WORDS_PER_KEY,
    build_lattice,
};

/// Tests for the decode pipeline, driven through the one entry point this tree exposes.
///
/// They sit at the root of the tree rather than beside the decoder because they cover the
/// whole flow -- segmentation, lattice, sweep and candidate assembly -- and because the
/// decoder's own file is at its line budget. The two pieces with behaviour of their own
/// keep their unit tests next to their code.
#[cfg(test)]
mod tests {
    use ime_types::{
        CandidateSource, DecodeFlags, DecodeRequest, DecodeResult, ImeError, LanguageModel,
        UserFreqSource,
    };

    use super::{DEFAULT_MAX_CANDIDATES, DecodeConfig, Decoder, MAX_BEAM_K, MAX_CANDIDATES};
    use crate::lm::{InMemoryLm, ScoreWeights};
    use crate::segment::MAX_RAW_LEN;
    use crate::viterbi::lattice::testing::{MockLexicon, MockUserFreq, NoUser};

    /// Decodes `raw` with the shipped configuration and no user history.
    fn decode(raw: &str, lexicon: &MockLexicon, lm: &dyn LanguageModel) -> DecodeResult {
        decode_configured(raw, lexicon, lm, DecodeConfig::default())
    }

    /// Decodes `raw` with an explicit configuration and no user history.
    fn decode_configured(
        raw: &str,
        lexicon: &MockLexicon,
        lm: &dyn LanguageModel,
        cfg: DecodeConfig,
    ) -> DecodeResult {
        let decoder = Decoder::new(cfg).expect("the test configurations are legal");
        decoder.decode(&DecodeRequest::new(raw), lexicon, &NoUser, lm)
    }

    /// Decodes `raw` against a user history, under the shipped configuration.
    fn decode_user(
        raw: &str,
        lexicon: &MockLexicon,
        user: &dyn UserFreqSource,
        lm: &dyn LanguageModel,
    ) -> DecodeResult {
        Decoder::default().decode(&DecodeRequest::new(raw), lexicon, user, lm)
    }

    /// The dictionary the ranking tests use: the four determinism inputs, with a second
    /// reading for most of their syllables so that a decode ranks more than one candidate.
    fn rich_lexicon() -> MockLexicon {
        MockLexicon::with(&[
            ("ni", "你"),
            ("ni", "尼"),
            ("hao", "好"),
            ("hao", "号"),
            ("ni'hao", "你好"),
            ("wo", "我"),
            ("ai", "爱"),
            ("ai", "矮"),
            ("wo'ai", "我爱"),
            ("wo'ai'ni", "我爱你"),
            ("zhong", "中"),
            ("zhong", "种"),
            ("guo", "国"),
            ("guo", "果"),
            ("zhong'guo", "中国"),
            ("bei", "北"),
            ("bei", "背"),
            ("jing", "京"),
            ("jing", "惊"),
            ("bei'jing", "北京"),
            ("da", "大"),
            ("da", "打"),
            ("xue", "学"),
            ("xue", "雪"),
            ("da'xue", "大学"),
            ("bei'jing'da'xue", "北京大学"),
        ])
    }

    /// Every field of every candidate, with the display score reduced to its bits so that
    /// the comparison is byte-for-byte rather than numeric.
    fn fingerprint(result: &DecodeResult) -> Vec<(u16, String, CandidateSource, u16, u32, bool)> {
        result
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.index,
                    candidate.text.clone(),
                    candidate.source,
                    candidate.consumed_syllables,
                    candidate.score.to_bits(),
                    result.degraded,
                )
            })
            .collect()
    }

    #[test]
    fn test_decode_ranks_the_whole_sentence_first() {
        let lexicon = rich_lexicon();
        let lm = InMemoryLm::new();
        let cases = [
            ("nihao", "你好", 2u16),
            ("woaini", "我爱你", 3),
            ("zhongguo", "中国", 2),
            ("beijingdaxue", "北京大学", 4),
        ];
        for (raw, text, syllables) in cases {
            let result = decode(raw, &lexicon, &lm);
            let best = &result.candidates[0];
            assert_eq!(best.text, text, "{raw}");
            assert_eq!(best.index, 1, "{raw}");
            assert_eq!(best.consumed_syllables, syllables, "{raw}");
            assert!(!result.degraded, "{raw}");
        }
    }

    #[test]
    fn test_decode_candidate_order_is_identical_across_100_runs() {
        let lexicon = rich_lexicon();
        let lm = InMemoryLm::new();
        for raw in ["nihao", "woaini", "zhongguo", "beijingdaxue"] {
            let expected = fingerprint(&decode(raw, &lexicon, &lm));
            assert!(expected.len() > 1, "{raw} must rank more than one candidate");
            for run in 0..100 {
                let seen = fingerprint(&decode(raw, &lexicon, &lm));
                assert_eq!(seen, expected, "{raw}, run {run}");
            }
        }
    }

    #[test]
    fn test_decode_returns_a_non_empty_list_on_every_boundary() {
        let lm = InMemoryLm::new();
        let empty = MockLexicon::with(&[]);
        let singles = MockLexicon::with(&[])
            .single("ni", &["伱"])
            .single("hao", &["好"]);
        let phrase = MockLexicon::phrase();
        let long = "nihao".repeat(12) + "niha";
        assert_eq!(long.len(), MAX_RAW_LEN);
        let cases: [(&str, &MockLexicon); 5] = [
            ("", &singles),
            ("zzz", &phrase),
            ("ni", &phrase),
            (long.as_str(), &phrase),
            ("nihao", &empty),
        ];
        for (raw, lexicon) in cases {
            let result = decode(raw, lexicon, &lm);
            assert!(!result.candidates.is_empty(), "{raw:?} must produce a candidate");
            assert!(
                result.candidates.len() <= usize::from(DEFAULT_MAX_CANDIDATES),
                "{raw:?} must stay inside the candidate limit"
            );
        }
    }

    #[test]
    fn test_decode_of_an_input_without_a_reading_degrades_to_passthrough() {
        let result = decode("zzz", &MockLexicon::phrase(), &InMemoryLm::new());
        assert!(result.degraded);
        assert!(result.segments.is_empty(), "there is no cut to describe");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].text, "zzz");
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
        assert_eq!(result.candidates[0].consumed_syllables, 0);
    }

    #[test]
    fn test_decode_without_a_dictionary_falls_back_to_single_characters() {
        let lexicon = MockLexicon::with(&[])
            .single("ni", &["伱"])
            .single("hao", &["好"]);
        let result = decode("nihao", &lexicon, &InMemoryLm::new());
        assert_eq!(result.candidates[0].text, "伱好");
        assert_eq!(result.candidates[0].consumed_syllables, 2);
        assert!(!result.degraded, "a fallback reading is still a reading");
    }

    #[test]
    fn test_decode_without_a_usable_word_degrades_to_passthrough() {
        let lexicon = MockLexicon::with(&[]);
        for fallback_single in [true, false] {
            let cfg = DecodeConfig {
                fallback_single,
                ..DecodeConfig::default()
            };
            let result = decode_configured("nihao", &lexicon, &InMemoryLm::new(), cfg);
            assert_eq!(result.candidates.len(), 1, "fallback {fallback_single}");
            assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
            assert!(result.degraded, "fallback {fallback_single}");
        }
    }

    #[test]
    fn test_decode_keeps_the_list_inside_the_page_budget() {
        let lexicon = rich_lexicon();
        let lm = InMemoryLm::new();
        let result = decode("beijingdaxue", &lexicon, &lm);
        assert!(result.candidates.len() > 1);
        assert!(result.candidates.len() <= usize::from(DEFAULT_MAX_CANDIDATES));
        // Nine candidates per page and five pages is the whole list the window can page
        // through, which is where the shipped limit comes from.
        assert_eq!(DEFAULT_MAX_CANDIDATES, 45);
        let narrowed = decode_configured(
            "beijingdaxue",
            &lexicon,
            &lm,
            DecodeConfig {
                max_candidates: 1,
                ..DecodeConfig::default()
            },
        );
        assert_eq!(narrowed.candidates.len(), 1);
    }

    #[test]
    fn test_decode_merges_candidates_that_spell_the_same_text() {
        // `xian` reads both as one word and as `xi` + `an`, and both readings spell the
        // same text, so the list must hold it once.
        let lexicon = MockLexicon::with(&[("xian", "西安"), ("xi", "西"), ("an", "安")]);
        let result = decode("xian", &lexicon, &InMemoryLm::new());
        assert_eq!(result.candidates[0].text, "西安");
        let spelled = result
            .candidates
            .iter()
            .filter(|candidate| candidate.text == "西安")
            .count();
        assert_eq!(spelled, 1);
    }

    #[test]
    fn test_decode_offers_the_first_word_of_the_best_path() {
        // The best reading of `nihaoma` is 你好|吗, so the opening word 你好 is offered
        // beside it for a user who only meant to type the first two syllables.
        let lexicon = MockLexicon::with(&[
            ("ni", "你"),
            ("hao", "好"),
            ("ni'hao", "你好"),
            ("ma", "吗"),
        ]);
        let result = decode("nihaoma", &lexicon, &InMemoryLm::new());
        assert_eq!(result.candidates[0].text, "你好吗");
        assert_eq!(result.candidates[0].consumed_syllables, 3);
        assert_eq!(result.candidates[1].text, "你好");
        assert_eq!(result.candidates[1].consumed_syllables, 2);
        // The prefix inherits its parent's score, so it never displaces the reading of
        // the whole input.
        assert!(result.candidates[0].score >= result.candidates[1].score);
    }

    #[test]
    fn test_decode_segments_describe_the_winning_cut() {
        // No whole-sentence word, so the winning path is two words and the cut covers the
        // syllables without a gap.
        let lexicon = MockLexicon::with(&[("ni", "你"), ("hao", "号")]);
        let result = decode("nihao", &lexicon, &InMemoryLm::new());
        assert_eq!(result.candidates[0].text, "你号");
        assert_eq!(result.segments.len(), 2);
        assert_eq!((result.segments[0].start, result.segments[0].end), (0, 1));
        assert_eq!((result.segments[1].start, result.segments[1].end), (1, 2));
        assert_eq!(result.segments[1].source, CandidateSource::Dict);
        let joined: String = result
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect();
        assert_eq!(joined, result.candidates[0].text);
    }

    #[test]
    fn test_decode_marks_a_refused_lookup_as_degraded() {
        let lexicon = MockLexicon::phrase().failing("hao");
        let result = decode("nihao", &lexicon, &InMemoryLm::new());
        assert!(result.degraded, "an incomplete lattice is a degraded answer");
        assert!(!result.candidates.is_empty());
    }

    #[test]
    fn test_decode_follows_the_user_frequency_and_the_user_dictionary_switch() {
        let lexicon = MockLexicon::with(&[("de", "的"), ("de", "得")]);
        let user = MockUserFreq::with(&[("得", 1000)]);
        let lm = InMemoryLm::new();
        let ranked = decode_user("de", &lexicon, &user, &lm);
        assert_eq!(ranked.candidates[0].text, "得");
        // Without a history the dictionary's own order decides.
        let plain = decode("de", &lexicon, &lm);
        assert_eq!(plain.candidates[0].text, "的");
        // With the user dictionary switched off the counts take no part either.
        let request = DecodeRequest::new("de").with_flags(DecodeFlags::empty());
        let decoder = Decoder::new(DecodeConfig::default()).expect("the shipped configuration");
        let off = decoder.decode(&request, &lexicon, &user, &lm);
        assert_eq!(off.candidates[0].text, "的");
    }

    #[test]
    fn test_decode_ranks_by_the_language_model_between_two_readings() {
        let lexicon = MockLexicon::with(&[("xian", "先"), ("xi", "西"), ("an", "安")]);
        let mut split = InMemoryLm::new();
        split.insert_unigram("先", -2048);
        split.insert_unigram("西", -128);
        split.insert_unigram("安", -128);
        assert_eq!(decode("xian", &lexicon, &split).candidates[0].text, "西安");

        let mut whole = InMemoryLm::new();
        whole.insert_unigram("先", -128);
        whole.insert_unigram("西", -2048);
        whole.insert_unigram("安", -2048);
        assert_eq!(decode("xian", &lexicon, &whole).candidates[0].text, "先");
    }

    #[test]
    fn test_decode_with_a_zero_width_beam_degrades_to_passthrough() {
        let cfg = DecodeConfig {
            beam_k: 0,
            ..DecodeConfig::default()
        };
        let result = decode_configured("nihao", &MockLexicon::phrase(), &InMemoryLm::new(), cfg);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
        assert!(result.degraded);
    }

    #[test]
    fn test_decoder_new_refuses_a_configuration_outside_the_ceilings() {
        for cfg in [
            DecodeConfig {
                beam_k: MAX_BEAM_K + 1,
                ..DecodeConfig::default()
            },
            DecodeConfig {
                max_candidates: MAX_CANDIDATES + 1,
                ..DecodeConfig::default()
            },
        ] {
            let error = Decoder::new(cfg).expect_err("the ceilings are hard");
            assert!(matches!(error, ImeError::ConfigInvalid { .. }), "{error}");
        }
        // The ceilings themselves are legal, and so is a beam that keeps nothing.
        for cfg in [
            DecodeConfig {
                beam_k: MAX_BEAM_K,
                max_candidates: MAX_CANDIDATES,
                ..DecodeConfig::default()
            },
            DecodeConfig {
                beam_k: 0,
                max_candidates: 0,
                ..DecodeConfig::default()
            },
        ] {
            assert!(Decoder::new(cfg).is_ok());
        }
        // A negative weight is refused by the scorer rather than by the ceiling check.
        let negative = DecodeConfig {
            weights: ScoreWeights {
                seg: -1,
                ..ScoreWeights::default()
            },
            ..DecodeConfig::default()
        };
        assert!(Decoder::new(negative).is_err());
    }

    #[test]
    fn test_decode_config_defaults_are_the_shipped_values() {
        let cfg = DecodeConfig::default();
        assert_eq!(cfg.beam_k, 16);
        assert_eq!(cfg.max_candidates, 45);
        assert!(cfg.fallback_single);
        assert_eq!(cfg.weights, ScoreWeights::default());
        assert!(cfg.validate().is_ok());
        assert_eq!(Decoder::default().config(), &cfg);
        // The derived default pairs the shipped configuration with the scorer built from
        // the same weights, so it must rank exactly like an explicitly built decoder.
        let lexicon = rich_lexicon();
        let lm = InMemoryLm::new();
        let request = DecodeRequest::new("nihao");
        let derived = Decoder::default().decode(&request, &lexicon, &NoUser, &lm);
        let explicit = Decoder::new(cfg)
            .expect("the shipped configuration is legal")
            .decode(&request, &lexicon, &NoUser, &lm);
        assert_eq!(fingerprint(&derived), fingerprint(&explicit));
    }
}
