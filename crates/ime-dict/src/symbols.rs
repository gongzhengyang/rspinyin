//! The symbol table's hot-swap slot: the table the v-mode reads, replaceable in place.
//!
//! Responsibility: hold the symbol table a session builds its panel candidates from behind
//! one atomic word, so a configuration reload can replace the whole table -- categories,
//! labels and symbol lists together -- without rebuilding the session that reads it. A
//! reader that wants to outlive the call it fetched in takes [`SymbolSlot::load`]'s `Arc`
//! and keeps the table it read alive for as long as it needs it; a reload stores a new
//! value and the old table dies with its last reader.
//!
//! Boundaries: this module decides nothing about *when* a swap happens -- that is the
//! host's reload path -- and owns no bytes of the document: parsing stays in
//! [`SymbolTable`](ime_core::symbols::SymbolTable), and the file the document is read
//! from is the host layer's, exactly as the phrase table's loader is the host's. The slot
//! is pure memory; it touches no file, no clock and no environment.
//!
//! # Absent and empty
//!
//! The slot's value is an `Option`: `None` means no table is installed, which is the state
//! of an install whose data directory carries no `symbols.tsv`, and `None` is what the
//! session reads as "the v-mode is off" -- an input that starts with `v` then decodes
//! exactly as a build before the panel did. `Some` holds a table, possibly one with no
//! categories (a document of comments alone), which serves an open panel over an empty
//! list. Keeping the two apart is what preserves that degradation byte for byte across a
//! reload: a reload that finds no file stores `None`, not an empty table, and the
//! pass-through behavior survives the swap.
//!
//! # The swap's threading contract
//!
//! The same contract the lexicon slot documents: the reload path runs on the host thread,
//! and so does every decode on it, so one thread is never inside a read while the reload
//! swaps. [`SymbolTable`] is immutable once built, which makes the whole table one
//! allocation-sized unit: [`SymbolSlot::store`] is a single pointer store, a reader either
//! sees the old table whole or the new one whole, and no reader ever sees a half-loaded
//! table. A reader on another thread that fetched an `Arc` before the swap keeps its table
//! alive through that `Arc`; the swap cannot tear it.
//!
//! # Why the reader keeps an `Arc`
//!
//! The table the panel serves is read once per rebuild through [`SymbolSlot::load`], and
//! the `Arc` it answers is the reader's proof the bytes live: a frame assembled from a
//! table it holds stays valid however many reloads land afterwards, because the old table
//! lives as long as its last `Arc` does. Handing out a borrowed reference instead would
//! make a swap able to free the bytes under a frame -- the tear the `Arc` exists to rule
//! out.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use ime_core::symbols::SymbolTable;

/// The live symbol table handle: one atomic word the readers load and a reload replaces.
///
/// The slot itself never blocks a reader. [`SymbolSlot::load`] is one atomic read of an
/// `Option<Arc>`, and the rebuild it feeds holds that `Arc`, which is what keeps the table
/// it read alive for the rebuild's whole run however many reloads land afterwards.
pub struct SymbolSlot {
    current: ArcSwapOption<SymbolTable>,
}

impl SymbolSlot {
    /// Builds a slot over `table`, which is the live value until a reload replaces it.
    ///
    /// [`None`](Option) installs no table -- the v-mode is off, the state of an install
    /// whose data directory carries no `symbols.tsv`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(table: Option<SymbolTable>) -> Self {
        Self {
            current: ArcSwapOption::from_pointee(table),
        }
    }

    /// Builds a slot holding no table, as a `static` initializer wants.
    ///
    /// The value a process starts with before its load step installs the document it
    /// found; a load that finds no file leaves the slot exactly here.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn empty() -> Self {
        Self {
            current: ArcSwapOption::const_empty(),
        }
    }

    /// The live table, one atomic read.
    ///
    /// [`None`](Option) means no table is installed and the v-mode is off. Holding the
    /// answer keeps its table alive: a reload that lands afterwards replaces the slot's
    /// word, not the `Arc` in the caller's hand, so the categories and symbols the caller
    /// borrowed stay valid to the end of the rebuild.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(&self) -> Option<Arc<SymbolTable>> {
        self.current.load_full()
    }

    /// Installs `table` as the live value, replacing whatever was there.
    ///
    /// The reload path's entry point: the host reads the document, parses it into a fresh
    /// table, and stores it whole. The store is one atomic pointer write, so a reader is
    /// either on the old table or on the new one -- the half-loaded state the atomicity
    /// requirement rules out is not representable. A reader that already holds the old
    /// table's `Arc` keeps it alive; the old table dies with its last reader.
    ///
    /// Storing [`None`](Option) takes the panel away, which is what a reload whose data
    /// directory no longer carries the document asks for -- the v-mode degrades to the
    /// pass-through it had before the panel existed.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::symbols::SymbolTable;
    /// use ime_dict::symbols::SymbolSlot;
    ///
    /// let slot = SymbolSlot::empty();
    /// assert!(slot.load().is_none());
    /// let (table, _) = SymbolTable::load("1\t数字\t０１\n");
    /// slot.store(Some(table));
    /// let live = slot.load().expect("a table is installed");
    /// assert_eq!(live.len(), 1);
    /// ```
    pub fn store(&self, table: Option<SymbolTable>) {
        self.current.store(table.map(Arc::new));
    }
}

/// Tests for the slot, over tables the parse builds in memory.
#[cfg(test)]
mod tests;
