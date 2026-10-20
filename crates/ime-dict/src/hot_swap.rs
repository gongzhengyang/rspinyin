//! The hot-swap slot: the live container and its model, replaceable in place.
//!
//! Responsibility: hold the dictionary a session decodes against behind one atomic
//! word, so a reload can replace the whole container -- mapping, index and model
//! together -- without rebuilding the readers that hold it. A reader that wants to
//! outlive the call it fetched in takes [`LexiconSlot::load`]'s `Arc` and keeps the
//! bundle it read alive for as long as it needs it; a swap stores a new `Arc` and the
//! old one dies with its last reader.
//!
//! Boundaries: this module decides nothing about *when* a swap happens -- that is the
//! host's reload path -- and validates nothing beyond what [`FstLexicon::load_with`]
//! already validates, which is the whole container: header, section table, lengths,
//! and, under [`Verify::Full`], the file and section checksums. A candidate that fails
//! any of it never reaches the slot, and the live container keeps serving.
//!
//! # Why the model rides with the lexicon
//!
//! [`DictLm`] reads slices of the mapping the lexicon owns. The type system spells
//! those slices `'static` -- the widening the mapping's ownership argument licenses --
//! so a model whose lexicon was dropped would read unmapped memory and no compiler
//! error. The bundle is the argument made structural: the two are one allocation,
//! swapped together and dropped together, and no reader can hold one without the
//! other.
//!
//! # The swap's threading contract
//!
//! The reload path runs on the host thread, and so does every decode; one thread
//! cannot be inside a lookup while it runs the callback that swaps. That is what
//! makes a swap safe for the word references a lookup hands back: they name bytes of
//! the old mapping, and the old mapping stays mapped for as long as anyone holds its
//! `Arc` -- which the swap's own discipline provides. A caller that swapped from
//! another thread while a decode ran would be a new kind of caller, and it would have
//! to hold [`LexiconSlot::load`] guards for its reads first; the frozen
//! `Lexicon::lookup` signature cannot force that, so the contract is written here and
//! enforced by where the calls live.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use ime_types::DictError;

use crate::format::reader::Verify;
use crate::fst_index::{DictLm, FstLexicon};

/// The mapped container and the model built on it, one replaceable unit.
pub struct LexiconBundle {
    lexicon: FstLexicon,
    lm: DictLm,
    /// Where the container was read from, for the diagnostics a swap reports.
    path: PathBuf,
}

impl LexiconBundle {
    /// Maps `path` and builds the bundle over it, verifying as much as `verify` asks
    /// for.
    ///
    /// # Errors
    ///
    /// The errors of [`FstLexicon::load_with`]: a file that cannot be mapped, is not
    /// a dictionary, is of another format version, fails its layout checks, or -- under
    /// [`Verify::Full`] -- fails its checksums.
    pub fn load(path: &Path, verify: Verify) -> Result<Self, DictError> {
        let lexicon = FstLexicon::load_with(path, verify)?;
        // The model reads the lexicon's mapping, and the bundle owns the lexicon for
        // as long as it owns the model: that is the pairing the swap depends on.
        let lm = DictLm::new(&lexicon);
        Ok(Self {
            lexicon,
            lm,
            path: path.to_path_buf(),
        })
    }

    /// The container a decode reads.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lexicon(&self) -> &FstLexicon {
        &self.lexicon
    }

    /// The model a decode scores with.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lm(&self) -> &DictLm {
        &self.lm
    }

    /// The file this bundle was read from.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The live lexicon handle: one atomic word the readers load and a swap replaces.
///
/// The slot itself never blocks a reader. [`LexiconSlot::load`] is one atomic read of
/// an `Arc`, and the decode it feeds holds that `Arc`, which is what keeps the bundle
/// it read mapped for the decode's whole run however many swaps land afterwards.
pub struct LexiconSlot {
    current: ArcSwap<LexiconBundle>,
}

impl LexiconSlot {
    /// Builds the slot over `bundle`, which is the live container until a swap
    /// replaces it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(bundle: LexiconBundle) -> Self {
        Self {
            current: ArcSwap::from_pointee(bundle),
        }
    }

    /// The live bundle, one atomic read.
    ///
    /// Holding the answer keeps its container mapped: a swap that lands afterwards
    /// replaces the slot's word, not the `Arc` in the caller's hand.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(&self) -> Arc<LexiconBundle> {
        self.current.load_full()
    }

    /// Validates `candidate` whole and, if it passes, swaps it in for the live
    /// container.
    ///
    /// The validation is the container load's own -- the full pass under
    /// [`Verify::Full`], which is what "checked before it is swapped" means here.
    /// Anything it rejects leaves the slot untouched: the old container keeps
    /// serving, and the reason travels out as the [`DictError`] the load answered
    /// with, which is what `dict/swap/rejected` is reported against. The measured
    /// cost of the full pass on the shipped container is about a millisecond, page
    /// cache warm, so the card's split into a header-only fast check and a deferred
    /// deep check is dropped as machinery the number does not ask for.
    ///
    /// # Parameters
    ///
    /// * `candidate` — the staged container file to validate and swap in.
    ///
    /// # Returns
    ///
    /// The bundle now live, held by an `Arc` the caller may keep.
    ///
    /// # Errors
    ///
    /// Every [`DictError`] a container load can answer with; the slot is unchanged in
    /// all of them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn try_swap(&self, candidate: &Path) -> Result<Arc<LexiconBundle>, DictError> {
        let bundle = Arc::new(LexiconBundle::load(candidate, Verify::Full)?);
        self.current.store(Arc::clone(&bundle));
        Ok(bundle)
    }
}

/// Tests for the slot, over containers the shared fixture encodes.
#[cfg(test)]
mod tests;
