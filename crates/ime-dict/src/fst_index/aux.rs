//! The aux-column view of the FST lexicon: the [`AuxSource`] the router's
//! refinement gate reads.
//!
//! Responsibility: answer the frozen `AuxSource` contract out of the container the
//! lexicon maps. The tail AUX column is not a section-table section, so it is not
//! among the slices the lexicon keeps; what the lexicon keeps is the parsed reader,
//! whose verified block offset says where the column sits and how far it runs. The
//! implementation is therefore a pure delegation -- one flag read and one word
//! lookup, both of them the format layer's own code paths -- with no row arithmetic
//! of this module's, so a code the lexicon hands out and a code the reader hands out
//! are the same read.
//!
//! # Concurrency
//!
//! `Sync`, as the trait's supertrait requires: every read is an immutable pass over
//! the mapped image the lexicon keeps, the reader holds no interior mutability, and
//! the one mutable thing behind the lexicon -- the malformed-entry log -- is a mutex
//! the aux path never touches. The `&str` a lookup returns borrows the image, which
//! lives as long as the lexicon does.

use ime_core::aux::AuxSource;
use ime_types::DictError;

use super::FstLexicon;

impl AuxSource for FstLexicon {
    /// Returns `true` when the mapped container carries the tail AUX column.
    ///
    /// A container without the column answers `false`, which is the state the
    /// refinement session degrades from (`aux/absent`), not an error.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn has_aux_column(&self) -> bool {
        self.reader.aux_column_present()
    }

    /// Returns the aux code recorded for `word`, borrowed from the mapped image.
    ///
    /// The word resolves through the container's own hash-ordered lookup and the
    /// code is one row slice out of the verified block, so the answer borrows the
    /// mapping and nothing is copied. A word the container does not hold, or one
    /// whose first character carries no decomposition, reads as `None`.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::LengthOutOfRange`] when the column is absent -- the
    /// `aux_absent` field the contract's degradation names -- or when the image
    /// disagrees with the layout the parse step verified, which a container that
    /// passed it cannot be.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn aux_code(&self, word: &str) -> Result<Option<&str>, DictError> {
        self.reader.aux_lookup(word)
    }
}

#[cfg(test)]
mod tests;
