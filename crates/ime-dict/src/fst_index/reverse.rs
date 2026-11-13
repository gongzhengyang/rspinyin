//! The reverse-lookup view of the FST lexicon: the [`ReverseSource`] the router's
//! `u`-domain gate and query read.
//!
//! Responsibility: answer the frozen `ReverseSource` contract out of the container
//! the lexicon maps. Like the AUX column, the reverse block is not a section-table
//! section, so it is not among the slices the lexicon keeps; what the lexicon keeps
//! is the parsed reader, whose parse accepted the block and fixed where it sits. The
//! implementation is therefore a pure delegation -- one flag read and one fragment
//! query, both of them the format layer's own code paths -- with no row arithmetic
//! of this module's, so a payload the lexicon answers and a payload the reader
//! answers are the same read.
//!
//! # Concurrency
//!
//! `Sync`, as the trait's supertrait requires: every read is an immutable pass over
//! the mapped image the lexicon keeps, and the reader holds no interior mutability.
//! The code a hit's annotation borrows the image, which lives as long as the lexicon
//! does.

use ime_core::reverse::{ReverseHit, ReverseSource};
use ime_types::DictError;

use super::FstLexicon;

impl ReverseSource for FstLexicon {
    /// Returns `true` when the mapped container carries the tail reverse index.
    ///
    /// A container without the index answers `false`, which is the state the
    /// `u`-domain degrades from (`reverse/absent`), not an error.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn has_reverse_index(&self) -> bool {
        self.reader.reverse_index_present()
    }

    /// Returns the characters whose decomposition code contains `payload` as a
    /// subsequence, borrowed from the mapped image.
    ///
    /// The fragment resolves through the reader's own binary search and each hit
    /// is a row slice out of the verified block, so the answer borrows the mapping
    /// and nothing is copied. A payload no key names, or one the domain would not
    /// accept, reads as an empty list.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the index is absent -- the
    /// `reverse_absent` field the contract's degradation names -- or when the image
    /// disagrees with the layout the parse step verified, which a container that
    /// passed it cannot be.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError> {
        // The reader's inherent method wins method resolution anyway; the plain
        // call is the delegation, in the same spirit as the reader's own trait
        // impl, which spells its delegation out for the same reason.
        self.reader.reverse_lookup(payload)
    }
}

#[cfg(test)]
mod tests;
