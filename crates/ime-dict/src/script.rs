//! The variant table the dictionary layer serves to the commit path.
//!
//! Responsibility: hold the simplified/traditional word mapping behind the
//! [`VariantSource`] trait the rewrite in `ime_core::script` is written against, and name
//! the value the plugin holds when that mapping is not there.
//!
//! Boundaries: this module answers lookups and decides nothing else. It never walks a
//! string -- longest-match rewriting is the engine-side module's job -- and it does not
//! know when a conversion runs, which is the commit path's business. It reads no file,
//! holds no clock and keeps no global state, and it allocates nothing on the read path:
//! every answer borrows the pair that was found.
//!
//! # Where the table comes from
//!
//! The design compiles the mapping into `script.dict`, a second container read through the
//! same zero-copy path as `base.dict`. The container cannot carry it as it stands: version
//! 1 defines six sections and none of them holds a word-to-word table, since the `FST`
//! section is keyed by syllable string and its payload is a word-list range. A variant
//! mapping also needs two key spaces -- a simplified word is the key of one direction and
//! a traditional word the key of the other -- so one index cannot serve both directions.
//!
//! Until a compiled table exists, the table this build serves is the in-memory seed from
//! `ime_core::script`, and [`ScriptIndex::unavailable`] is what a build without a table
//! degrades to. Both are a [`ScriptIndex`], so the engine's call site does not change when
//! the compiled index lands.
//!
//! # Concurrency
//!
//! `Send + Sync`. The table is built once and never written to, so the thread that decodes
//! and the thread that draws share one index without a lock. A lookup borrows the pair it
//! found, allocates nothing and returns in time bounded by the length of the word it is
//! asked about, which is what the commit path inside a host callback requires.

use ime_core::script::{VariantSource, VariantTable};
use ime_types::ui::Script;

/// The simplified/traditional word mapping a commit converts through.
///
/// This is the [`VariantSource`] the engine holds: the rewrite asks it what a word looks
/// like in the other script, and it answers from the table the dictionary layer provides.
/// A word the table does not hold is an answer rather than an error -- the rewrite copies
/// it through -- which is what makes a gap in the table lossless.
///
/// # Concurrency
///
/// `Send + Sync`: the table is built once and never written to, so the thread that decodes
/// and the thread that draws share one index without a lock. A lookup borrows the pair it
/// found, allocates nothing and is reentrant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptIndex {
    /// The pairs, indexed in both directions.
    table: VariantTable,
}

impl ScriptIndex {
    /// Builds an index from `pairs` of `(simplified, traditional)`.
    ///
    /// The pairs may arrive in any order and may repeat a key. The first pair for a key
    /// wins and a pair whose key or value is empty is dropped, which is the rule
    /// [`VariantTable::from_pairs`] applies; a generated table therefore does not depend on
    /// the order its generator emitted in.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A pair the table cannot use is
    /// dropped rather than reported.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::script::to_traditional;
    /// use ime_dict::script::ScriptIndex;
    ///
    /// let index = ScriptIndex::from_pairs(&[("干净", "乾淨")]);
    /// assert_eq!(to_traditional("干净", &index), "乾淨");
    /// ```
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            table: VariantTable::from_pairs(pairs),
        }
    }

    /// The table this build ships.
    ///
    /// A placeholder for the compiled mapping, and the table the plugin converts with until
    /// that file exists. It holds the word-level entries that disambiguate the characters
    /// with more than one traditional form, so the feature is usable as soon as it is
    /// switched on; see the module documentation for what it is not.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::script::to_traditional;
    /// use ime_dict::script::ScriptIndex;
    ///
    /// let index = ScriptIndex::seed();
    /// assert_eq!(to_traditional("银行", &index), "銀行");
    /// ```
    pub fn seed() -> Self {
        Self {
            table: VariantTable::from_seed(),
        }
    }

    /// The index that answers nothing.
    ///
    /// What the plugin holds when the mapping is not there: a build without a table, or a
    /// table that could not be read. Every lookup misses, so every conversion returns its
    /// input unchanged and the input path stays fully usable. The caller reports the
    /// degradation once, with the code `ui/script/unavailable`, which the user never sees.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::script::to_traditional;
    /// use ime_dict::script::ScriptIndex;
    ///
    /// let index = ScriptIndex::unavailable();
    /// assert_eq!(to_traditional("干净", &index), "干净");
    /// ```
    pub fn unavailable() -> Self {
        Self {
            table: VariantTable::default(),
        }
    }
}

impl VariantSource for ScriptIndex {
    /// Answers from the table's index for the direction `target` names.
    ///
    /// Reentrant, allocation-free and non-blocking: the answer borrows the pair that was
    /// found, and the table is never written to. A word the table does not hold yields
    /// `None` rather than a guess, which the rewrite turns into a copy-through.
    fn lookup(&self, word: &str, target: Script) -> Option<&str> {
        self.table.lookup(word, target)
    }
}

#[cfg(test)]
mod tests;
