//! Test doubles shared by the lattice and the decoder tests.
//!
//! They live in a file of their own, beside the lattice rather than in the decoder's own test
//! module, because both suites rank against the same dictionary: one in-memory implementation
//! of the frozen traits keeps the two honest about what a lookup answers. The module is
//! declared `#[cfg(test)]`, so none of this is compiled into a shipped build.

use std::collections::{BTreeMap, BTreeSet};

use ime_types::{ImeError, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef};

use crate::segment::syllable_at;

/// A dictionary with fixed contents: keys to word texts, spelled syllables to
/// their single-character fallbacks, the keys whose lookup fails, and the words
/// the user is credited with.
///
/// Words come back in the order they were listed, which is the order the frozen
/// contract describes for a real lookup: strongest first.
pub(crate) struct MockLexicon {
    words: BTreeMap<&'static str, Vec<&'static str>>,
    singles: BTreeMap<&'static str, Vec<&'static str>>,
    failing: BTreeSet<&'static str>,
    coined: BTreeSet<&'static str>,
}

impl MockLexicon {
    /// Builds a dictionary from `(key, word)` rows, one row per word.
    pub(crate) fn with(rows: &[(&'static str, &'static str)]) -> Self {
        let mut words: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
        for (key, word) in rows {
            words.entry(key).or_default().push(word);
        }
        Self {
            words,
            singles: BTreeMap::new(),
            failing: BTreeSet::new(),
            coined: BTreeSet::new(),
        }
    }

    /// The dictionary the decoder tests rank against: the words the four
    /// determinism inputs need, plus both readings of the ambiguous `xian`.
    pub(crate) fn phrase() -> Self {
        Self::with(&[
            ("ni", "你"),
            ("hao", "好"),
            ("ni'hao", "你好"),
            ("wo", "我"),
            ("ai", "爱"),
            ("wo'ai", "我爱"),
            ("wo'ai'ni", "我爱你"),
            ("zhong", "中"),
            ("guo", "国"),
            ("zhong'guo", "中国"),
            ("bei", "北"),
            ("jing", "京"),
            ("bei'jing", "北京"),
            ("da", "大"),
            ("xue", "学"),
            ("da'xue", "大学"),
            ("bei'jing'da'xue", "北京大学"),
            ("xi", "西"),
            ("an", "安"),
            ("xian", "先"),
        ])
        .single("ni", &["伱"])
        .single("hao", &["号", "浩", "郝"])
        .single("wo", &["窝"])
        .single("ai", &["矮", "唉"])
        .single("zhong", &["种"])
        .single("guo", &["果"])
        .single("bei", &["背"])
        .single("jing", &["惊"])
        .single("da", &["打"])
        .single("xue", &["雪", "穴"])
        .single("xi", &["希"])
        .single("an", &["按"])
    }

    /// Adds the single-character fallbacks of one syllable.
    pub(crate) fn single(mut self, syllable: &'static str, texts: &[&'static str]) -> Self {
        self.singles.insert(syllable, texts.to_vec());
        self
    }

    /// Makes one key fail its lookup.
    pub(crate) fn failing(mut self, key: &'static str) -> Self {
        self.failing.insert(key);
        self
    }

    /// Credits the user with one word.
    pub(crate) fn coined(mut self, word: &'static str) -> Self {
        self.coined.insert(word);
        self
    }
}

impl Lexicon for MockLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        if self.failing.contains(key) {
            return Err(ImeError::Unsupported);
        }
        let syllables = u8::try_from(key.split('\'').count()).unwrap_or(1);
        let words: Vec<WordRef<'_>> = self
            .words
            .get(key)
            .map(|texts| {
                texts
                    .iter()
                    .map(|text| WordRef {
                        text,
                        weight: 1,
                        syl_count: syllables,
                        flags: if self.coined.contains(text) {
                            WordFlags::USER
                        } else {
                            WordFlags::empty()
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(WordIter::from_vec(words))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        let words: Vec<WordRef<'_>> = syllable_at(syl)
            .and_then(|spelling| self.singles.get(spelling))
            .map(|texts| {
                texts
                    .iter()
                    .take(limit)
                    .map(|text| WordRef {
                        text,
                        weight: 1,
                        syl_count: 1,
                        flags: WordFlags::empty(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(WordIter::from_vec(words))
    }
}

/// A user-frequency source that has recorded nothing.
pub(crate) struct NoUser;

impl UserFreqSource for NoUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }
    fn record(&self, _key: &str, _weight_hint: u16) {}
    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// A user-frequency source with a fixed count per key.
pub(crate) struct MockUserFreq {
    counts: BTreeMap<&'static str, u32>,
}

impl MockUserFreq {
    /// Builds a source from `(key, count)` rows.
    pub(crate) fn with(rows: &[(&'static str, u32)]) -> Self {
        Self {
            counts: rows.iter().copied().collect(),
        }
    }
}

impl UserFreqSource for MockUserFreq {
    fn freq(&self, key: &str) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }
    fn record(&self, _key: &str, _weight_hint: u16) {}
    fn is_user_word(&self, key: &str) -> bool {
        self.counts.contains_key(key)
    }
}
