//! rspinyin core decoding engine.
//!
//! Pipeline: syllable segmentation, then word-lattice construction, then Viterbi
//! k-best decoding, then preedit generation.
//!
//! This crate holds no file handles. It reaches the dictionary and user-frequency
//! store only through the trait objects defined in `ime-types`, which is what allows
//! it to be tested deterministically against in-memory mocks with no filesystem,
//! clock, or display server involved.

pub mod input;
pub mod lm;
pub mod passthrough;
pub mod preedit;
pub mod privacy;
pub mod segment;
pub mod shuangpin;
pub mod state;
pub mod viterbi;
