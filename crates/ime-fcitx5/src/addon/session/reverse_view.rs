//! The reverse-lookup view of the container the start mapped: the anchor the
//! router's `u`-domain reads through, and the reads it answers.
//!
//! Responsibility: hold the anchored `Arc` of the bundle the slot took at start and
//! answer the [`ReverseSource`](ime_core::reverse::ReverseSource) contract through
//! it. The module mirrors the aux view beside it, and the two keep one anchor each:
//! both pin the same install-time container -- the `lexicon` step anchors both right
//! after the slot is installed -- and keeping the anchors apart keeps each view's
//! whole story in one spot, which is the reason the aux view's own module names for
//! being a place of its own. A reference into the slot's *current* bundle would
//! dangle across the swap a staged dictionary makes -- the same reason the symbol
//! table is not a `&` into the slot -- and the frozen contract hands back codes
//! borrowed from the view, so the view cannot re-read the slot per call the way the
//! decode path does.
//!
//! Boundary: this module decides nothing about when the view is handed out -- the
//! session-host step reads it through [`reverse_source`] -- and it touches the slot
//! only through the one read that anchors it.

use std::sync::{Arc, OnceLock};

use ime_core::reverse::ReverseSource;
use ime_dict::fst_index::FstLexicon;
use ime_dict::hot_swap::LexiconBundle;

/// The reverse-lookup view of the container the start mapped, anchored for the
/// process's life.
///
/// The anchor holds one `Arc` of the bundle the slot took at start, and the view
/// answers through it. A decomposition index is build-time materialization of the
/// shipped table, so after a swap the anchored view answers the characters the two
/// containers agree on, and a payload the swapped-in index would not name reads as
/// a miss rather than as a fault -- the same degradation a container without the
/// index shows.
static REVERSE_VIEW: OnceLock<Arc<LexiconBundle>> = OnceLock::new();

/// Anchors the bundle the slot holds, so the view the router reads stays mapped for
/// the process's whole life.
///
/// Called by the `lexicon` step right after the slot is installed, beside the aux
/// view's own anchor: a swap replaces the slot's own reference but not this one,
/// which is what keeps the view from ever dangling.
///
/// # Panics
///
/// Never.
pub(super) fn anchor_once() {
    if let Some(slot) = super::LEXICON_SLOT.get() {
        let _ = REVERSE_VIEW.set(slot.load());
    }
}

/// The reverse-lookup view the router's `u`-domain reads, when the start mapped a
/// container that carries the index.
///
/// `None` for a start that mapped nothing and for a container without the index:
/// both are the degradation the router's once-per-process report names, and the
/// router treats them identically.
///
/// # Panics
///
/// Never.
pub(in crate::addon) fn reverse_source() -> Option<&'static dyn ReverseSource> {
    let lexicon: &'static FstLexicon = REVERSE_VIEW.get()?.lexicon();
    // A container without the index stays `None` rather than handing over a view
    // whose first answer is `false`: the router treats the two identically, and
    // `None` says so without the call.
    lexicon
        .has_reverse_index()
        .then_some(lexicon as &dyn ReverseSource)
}
