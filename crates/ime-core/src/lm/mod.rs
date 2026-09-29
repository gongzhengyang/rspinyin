//! The language-model scoring layer: an integer logarithm, the weighted edge and
//! path score the decoder ranks with, and the in-memory model the engine is
//! tested against.
//!
//! Responsibility: everything that turns a word, its predecessor and the user's
//! own history into one comparable integer. The layer is pure by construction --
//! no file, no clock, no global state -- which is what lets the decoder be driven
//! from literals in a test and still produce the same candidate order every run.
//!
//! Boundaries: this layer never looks a word up, never builds a lattice and never
//! decides how many candidates survive. It receives the language model and the
//! user-frequency source as trait objects from the frozen contract crate, and it
//! returns a number; everything above it is the decoder's business.
//!
//! # Layout
//!
//! - [`score`] owns the fixed-point arithmetic, the weight tuple and [`Scorer`].
//! - [`ngram`] owns [`InMemoryLm`], the in-memory `LanguageModel`.
//!
//! # Where the real model comes from
//!
//! The dictionary-backed `UnigramBigram` is built by `ime-dict` from the compiled
//! dictionary's unigram and word-list sections. It implements the same frozen
//! `LanguageModel` trait as [`InMemoryLm`], so the decoder never learns which of
//! the two it is holding, and a test written against the double keeps its meaning
//! when the real model is swapped in.

pub mod ngram;
pub mod score;

pub use crate::lm::ngram::{BIGRAM_MISS_PENALTY, InMemoryLm, UNIGRAM_MISS};
pub use crate::lm::score::{LOG2_FLOOR, Q, ScoreWeights, Scorer, USER_TERM_CAP, log2_q8};
