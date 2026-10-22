//! The switches one lattice build reads and the buffer it reuses.
//!
//! Responsibility: bundle the per-request knobs and the caller's reusable buffers into
//! one value so the build entry points take a single options argument. The bundle is
//! plain data: it decides nothing, and every field is read by the build exactly as it
//! stands.
//!
//! Boundaries: the type owns no behaviour beyond its [`fmt::Debug`] shape, which
//! prints the lexicons as their presence -- a `dyn Lexicon` has no readable shape,
//! and the build's behaviour is decided by whether one is there.

use core::fmt;

use ime_types::{DecodeFlags, Lexicon};

use crate::segment::Readings;

/// The switches one lattice build reads and the buffer it reuses.
///
/// Grouped rather than passed one by one because the three travel together: a build is
/// asked for the same thing every time and differs only in these, and the buffer is the
/// caller's, so that a decode that runs once per keystroke hands the same allocation
/// back instead of building one per node.
pub struct LatticeOptions<'a, 'e> {
    /// Whether a one-syllable span the dictionary has no word for is filled in with
    /// single-character candidates. Turning it off leaves the lattice without a path
    /// for such an input, which the decoder answers with a pass-through candidate.
    pub fallback_single: bool,
    /// The switches of the request being decoded.
    ///
    /// [`DecodeFlags::ABBREV`] is the whole abbreviation gate: with the bit clear the
    /// abbreviation walk is never entered, no reading is enumerated and no prefix query
    /// is made, so a build costs what it cost before abbreviation existed.
    pub flags: DecodeFlags,
    /// The buffer the abbreviation enumeration reuses.
    ///
    /// It is cleared and filled once per node the walk reaches, and the syllable vectors
    /// it holds are handed back to the next call, so a decode that keeps one of these
    /// stops allocating after the first input of a given shape.
    pub readings: &'a mut Readings,
    /// The key arena the build writes the canonical spelling of every looked-up key into,
    /// and the edges name ranges into.
    ///
    /// Cleared by the build, so a range names this build's bytes and nothing earlier.
    /// The arena belongs to the caller's workspace rather than to the lattice on purpose:
    /// a lattice is built per decode and dropped with the call, and a buffer of its own
    /// here would put a second allocation back on the key path the workspace had already
    /// paid once for.
    pub keys: &'a mut Vec<u8>,
    /// The English lexicon the mixed-English switch reads, or `None` for a request
    /// that did not ask for it.
    ///
    /// `Some` does not by itself make a query happen: the English pass runs only when
    /// the exact, fuzzy and correction walks have all left the input uncut, so a
    /// request that reads as pinyin never reaches the English lexicon and costs what
    /// it cost before the switch existed.
    pub english: Option<&'e dyn Lexicon>,
}

impl fmt::Debug for LatticeOptions<'_, '_> {
    /// Prints the switches and the buffers; the lexicons print as their presence,
    /// because a `dyn Lexicon` has no readable shape and the build's behaviour is
    /// decided by whether one is there.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LatticeOptions")
            .field("fallback_single", &self.fallback_single)
            .field("flags", &self.flags)
            .field("english", &self.english.is_some())
            .finish_non_exhaustive()
    }
}
