//! The aux-column view of the container the start mapped: the anchor the router's
//! refinement gate reads through, and the reads it answers.
//!
//! Responsibility: hold the anchored `Arc` of the bundle the slot took at start and
//! answer the [`AuxSource`](ime_core::aux::AuxSource) contract through it. The module
//! is a place of its own because the anchor's reason wants the whole story in one
//! spot: a reference into the slot's *current* bundle would dangle across the swap a
//! staged dictionary makes -- the same reason the symbol table is not a `&` into the
//! slot -- and the frozen contract hands back codes borrowed from the view, so the
//! view cannot re-read the slot per call the way the decode path does.
//!
//! Boundary: this module decides nothing about when the view is handed out -- the
//! session-host step reads it through [`aux_source`] -- and it touches the slot only
//! through the one read that anchors it.

use std::sync::{Arc, OnceLock};

use ime_core::aux::AuxSource;
use ime_dict::fst_index::FstLexicon;
use ime_dict::hot_swap::LexiconBundle;

/// The aux-column view of the container the start mapped, anchored for the
/// process's life.
///
/// The anchor holds one `Arc` of the bundle the slot took at start, and the view
/// answers through it. Codes are a pure function of the word and the compiler's
/// decomposition table, so after a swap the anchored view answers the codes the two
/// containers agree on, and a word it does not hold reads as unrecorded rather than
/// as a fault.
static AUX_VIEW: OnceLock<Arc<LexiconBundle>> = OnceLock::new();

/// Anchors the bundle the slot holds, so the view the router reads stays mapped for
/// the process's whole life.
///
/// Called by the `lexicon` step right after the slot is installed: a swap replaces
/// the slot's own reference but not this one, which is what keeps the view from ever
/// dangling.
///
/// # Panics
///
/// Never.
pub(super) fn anchor_once() {
    if let Some(slot) = super::LEXICON_SLOT.get() {
        let _ = AUX_VIEW.set(slot.load());
    }
}

/// The aux-column view the router's refinement gate reads, when the start mapped a
/// container that carries the column.
///
/// `None` for a start that mapped nothing and for a container without the column:
/// both are the degradation the router's once-per-process report names, and the
/// router treats them identically.
///
/// # Panics
///
/// Never.
pub(in crate::addon) fn aux_source() -> Option<&'static dyn AuxSource> {
    let lexicon: &'static FstLexicon = AUX_VIEW.get()?.lexicon();
    // A container without the column stays `None` rather than handing over a view
    // whose first answer is `false`: the router treats the two identically, and
    // `None` says so without the call.
    lexicon
        .has_aux_column()
        .then_some(lexicon as &dyn AuxSource)
}
