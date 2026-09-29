//! The variant table held in memory, and the seed it is built from.
//!
//! Responsibility: [`VariantTable`], the table-driven [`VariantSource`] the rewrite
//! is tested against, and [`SEED`], the small hand-written table it is built from.
//!
//! Boundaries: data and a lookup, no algorithm. This module never walks a string and
//! never decides what a match means; it answers one question -- what is this word's
//! spelling in that script -- and the module beside it does the rest. It is as pure
//! as that module: no file, no clock, no environment, no global state.
//!
//! # The seed is a placeholder for a generated table
//!
//! The production table is generated from Unihan's `kTraditionalVariant` and
//! `kSimplifiedVariant` fields by the dictionary compiler and shipped as
//! `script.dict`, which implements the same trait over its memory map. [`SEED`] is
//! what exists before that pipeline does: the five one-to-many groups the design
//! calls out, each disambiguated at word level, plus a handful of unambiguous single
//! characters. It is small on purpose -- a fixture with a real contract, not a
//! dictionary.
//!
//! # Why the ambiguous single characters are absent
//!
//! `发` is `發` in `出发` and `髮` in `头发`; `干` is `乾` in `干净`, `幹` in
//! `干活` and `干` in `干涉`. No single-character entry can be right for all of
//! them, and picking one would be a guess the user cannot see or undo. The table
//! therefore holds the *words* and leaves the bare character to the rewrite's
//! pass-through rule, which copies it through unchanged.
//!
//! # Direction
//!
//! A pair is stored once and indexed twice, by simplified form and by traditional
//! form, which is what serves both directions from one table. A word spelled the
//! same in both scripts (`干涉`, `公里`) is stored as a pair whose two sides are
//! equal: it is not a no-op entry but the record of a disambiguation decision, and
//! it is what keeps a longer match from reaching across it.
//!
//! # Duplicates and unusable pairs
//!
//! The first pair for a key wins. A pair with an empty key or an empty value is
//! dropped: the first could never be matched, and the second would delete the word it
//! matched, which the rewrite never does. The rules are what make a generated table
//! deterministic when its generator emits a word twice.

use ime_types::ui::Script;

use super::VariantSource;

/// One variant pair: a word in simplified Chinese and its traditional spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariantEntry {
    /// The simplified spelling. It is the key a lookup reads when the target is
    /// [`Script::Traditional`].
    pub simplified: &'static str,
    /// The traditional spelling, the key for the other direction.
    pub traditional: &'static str,
}

/// The hand-written seed table.
///
/// The word-level entries are the five one-to-many groups of the design, and each
/// one is a decision the bare character cannot make. The single-character entries
/// are unambiguous: every one of them maps to exactly one traditional character
/// wherever it appears.
///
/// A production table replaces this one; see the module documentation for what the
/// seed is and what it is not.
pub const SEED: &[VariantEntry] = &[
    // 发: 髮 is hair, 發 is everything else, so only the word can decide.
    VariantEntry {
        simplified: "头发",
        traditional: "頭髮",
    },
    VariantEntry {
        simplified: "出发",
        traditional: "出發",
    },
    VariantEntry {
        simplified: "发现",
        traditional: "發現",
    },
    // 干: 乾 is dry or clean, 幹 is to do, and 干涉 stays 干.
    VariantEntry {
        simplified: "干净",
        traditional: "乾淨",
    },
    VariantEntry {
        simplified: "干活",
        traditional: "幹活",
    },
    VariantEntry {
        simplified: "干涉",
        traditional: "干涉",
    },
    // 后: 後 is behind, and the empress is 后 in both scripts.
    VariantEntry {
        simplified: "后面",
        traditional: "後面",
    },
    VariantEntry {
        simplified: "皇后",
        traditional: "皇后",
    },
    // 里: 裡 is inside, and a unit of distance is 里 in both scripts.
    VariantEntry {
        simplified: "里面",
        traditional: "裡面",
    },
    VariantEntry {
        simplified: "公里",
        traditional: "公里",
    },
    // 台: 臺 is the island and the platform, 颱 is the typhoon, 檯 is furniture.
    VariantEntry {
        simplified: "台湾",
        traditional: "臺灣",
    },
    VariantEntry {
        simplified: "台风",
        traditional: "颱風",
    },
    VariantEntry {
        simplified: "写字台",
        traditional: "寫字檯",
    },
    // Unambiguous single characters, the commonest ones a commit runs into.
    VariantEntry {
        simplified: "银",
        traditional: "銀",
    },
    VariantEntry {
        simplified: "汉",
        traditional: "漢",
    },
    VariantEntry {
        simplified: "国",
        traditional: "國",
    },
    VariantEntry {
        simplified: "语",
        traditional: "語",
    },
    VariantEntry {
        simplified: "说",
        traditional: "說",
    },
    VariantEntry {
        simplified: "门",
        traditional: "門",
    },
    VariantEntry {
        simplified: "车",
        traditional: "車",
    },
    VariantEntry {
        simplified: "书",
        traditional: "書",
    },
    VariantEntry {
        simplified: "电",
        traditional: "電",
    },
    VariantEntry {
        simplified: "话",
        traditional: "話",
    },
    VariantEntry {
        simplified: "见",
        traditional: "見",
    },
    VariantEntry {
        simplified: "学",
        traditional: "學",
    },
    VariantEntry {
        simplified: "时",
        traditional: "時",
    },
    VariantEntry {
        simplified: "会",
        traditional: "會",
    },
    VariantEntry {
        simplified: "个",
        traditional: "個",
    },
    VariantEntry {
        simplified: "们",
        traditional: "們",
    },
    VariantEntry {
        simplified: "来",
        traditional: "來",
    },
    VariantEntry {
        simplified: "对",
        traditional: "對",
    },
    VariantEntry {
        simplified: "开",
        traditional: "開",
    },
    VariantEntry {
        simplified: "关",
        traditional: "關",
    },
    VariantEntry {
        simplified: "头",
        traditional: "頭",
    },
    VariantEntry {
        simplified: "风",
        traditional: "風",
    },
    VariantEntry {
        simplified: "马",
        traditional: "馬",
    },
    VariantEntry {
        simplified: "鱼",
        traditional: "魚",
    },
    VariantEntry {
        simplified: "龙",
        traditional: "龍",
    },
    VariantEntry {
        simplified: "万",
        traditional: "萬",
    },
];

/// A variant table held in memory and indexed in both directions.
///
/// This is the [`VariantSource`] the rewrite is tested against, and the one a caller
/// holding a handful of pairs can use without a dictionary. The compiled
/// `script.dict` index in the dictionary layer implements the same trait over its
/// memory map, which is what keeps a large table out of the heap.
///
/// A table is immutable once built, which is what makes it `Send + Sync` and lets
/// the decoding thread and the drawing thread share one. A pair is held twice, once
/// per direction, so the memory a table costs is roughly twice the pairs it holds --
/// which is why the large table is the memory-mapped one and not this.
///
/// [`Default`] is the empty table: every lookup misses and every string converts to
/// itself, which is what a missing table degrades to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VariantTable {
    /// `(key, value)` pairs sorted by key, where the key is the simplified spelling.
    to_traditional: Vec<(String, String)>,
    /// The same pairs sorted by key, where the key is the traditional spelling.
    to_simplified: Vec<(String, String)>,
}

impl VariantTable {
    /// Builds a table from `pairs` of `(simplified, traditional)`.
    ///
    /// The pairs may arrive in any order and may repeat a key. The table sorts them,
    /// keeps the first pair for a key, and drops a pair whose key or value is empty;
    /// see the module documentation for why.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`. A table is built from
    /// whatever pairs it is given, and one it cannot use is dropped rather than
    /// reported.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::script::{VariantSource, VariantTable};
    /// use ime_types::ui::Script;
    ///
    /// let table = VariantTable::from_pairs(&[("干净", "乾淨")]);
    /// assert_eq!(table.lookup("干净", Script::Traditional), Some("乾淨"));
    /// assert_eq!(table.lookup("乾淨", Script::Simplified), Some("干净"));
    /// assert_eq!(table.lookup("干", Script::Traditional), None);
    /// ```
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        let mut to_traditional = Vec::with_capacity(pairs.len());
        for &(simplified, traditional) in pairs {
            to_traditional.push((String::from(simplified), String::from(traditional)));
        }
        Self::build(to_traditional)
    }

    /// Builds the table from [`SEED`].
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    pub fn from_seed() -> Self {
        let mut to_traditional = Vec::with_capacity(SEED.len());
        for entry in SEED {
            let simplified = String::from(entry.simplified);
            let traditional = String::from(entry.traditional);
            to_traditional.push((simplified, traditional));
        }
        Self::build(to_traditional)
    }

    /// Sorts `to_traditional` by key, drops the pairs a later duplicate repeats, and
    /// derives the reverse index from what is left.
    fn build(mut to_traditional: Vec<(String, String)>) -> Self {
        // A pair the rewrite cannot use as a replacement is dropped rather than left
        // unreachable: no window the rewrite builds is empty, so an empty key can
        // never be matched, and an empty value would delete the word it matched --
        // which the rewrite never does.
        to_traditional.retain(|pair| !pair.0.is_empty() && !pair.1.is_empty());
        // A stable sort keeps the pairs of equal keys in the order they arrived, so
        // "the first pair for a key wins" is the rule the dedup below applies.
        to_traditional.sort_by(|left, right| left.0.cmp(&right.0));
        to_traditional.dedup_by(|left, right| left.0 == right.0);

        let mut to_simplified: Vec<(String, String)> = to_traditional
            .iter()
            .map(|pair| (pair.1.clone(), pair.0.clone()))
            .collect();
        to_simplified.sort_by(|left, right| left.0.cmp(&right.0));
        to_simplified.dedup_by(|left, right| left.0 == right.0);

        Self {
            to_traditional,
            to_simplified,
        }
    }
}

impl VariantSource for VariantTable {
    /// Binary-searches the index for the direction `target` names.
    ///
    /// Reentrant and allocation-free: the answer is a borrow of the pair that was
    /// found, and the table is never written to.
    fn lookup(&self, word: &str, target: Script) -> Option<&str> {
        let pairs = match target {
            Script::Traditional => &self.to_traditional,
            Script::Simplified => &self.to_simplified,
        };
        search(pairs, word)
    }
}

/// Binary-searches `(key, value)` pairs sorted by key.
///
/// The answer borrows from the table and never from the word being looked up: a caller
/// holds the table for as long as it holds the answer, while the word is often a
/// temporary it is about to drop.
fn search<'table>(pairs: &'table [(String, String)], word: &str) -> Option<&'table str> {
    pairs
        .binary_search_by(|pair| pair.0.as_str().cmp(word))
        .ok()
        .and_then(|index| pairs.get(index))
        .map(|pair| pair.1.as_str())
}
