//! Criterion benchmark for the K-best decoder.
//!
//! The case this file exists for is `decode/viterbi`: one full decode of a twelve-syllable
//! input against a dictionary that covers it, which is the shape the decode budget
//! (`BUDGET-LAT-02`, P99 3ms and P999 8ms) is stated for. It measures rather than asserts:
//! turning the measurement into a gate belongs to the task that owns
//! `docs/dev/budgets.json`.
//!
//! Nothing here touches the filesystem, the clock or the network, and both the dictionary
//! and the model are fixed, so a run is reproducible.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ime_core::lm::InMemoryLm;
use ime_core::segment::syllable_at;
use ime_core::viterbi::Decoder;
use ime_types::{
    DecodeRequest, ImeError, Lexicon, SyllableId, UserFreqSource, WordFlags, WordIter, WordRef,
};

/// The input the budget is stated for: twelve syllables in forty bytes.
///
/// The tail of every word can start the next one, so the segmentation graph branches at
/// each seam -- `daxuezhongguo` reads as `da|xue|zhong|guo` and as `da|xue|zho|ng|guo` --
/// which is the expensive shape a decode has to stay inside its budget on.
const INPUT: &str = "zhongguobeijingdaxuezhongguobeijingdaxue";

/// The words the corpus is spelled with.
///
/// Every key holds two readings, so the beam has something to rank: a dictionary with one
/// word per key would leave the merge with nothing to choose between.
const WORDS: &[(&str, &[&str])] = &[
    ("zhong", &["中", "种"]),
    ("guo", &["国", "果"]),
    ("zhong'guo", &["中国"]),
    ("bei", &["北", "背"]),
    ("jing", &["京", "惊"]),
    ("bei'jing", &["北京"]),
    ("da", &["大", "打"]),
    ("xue", &["学", "雪"]),
    ("da'xue", &["大学"]),
    ("bei'jing'da'xue", &["北京大学"]),
    ("zhong'guo'bei'jing", &["中国北京"]),
];

/// An in-memory dictionary: the words of [`WORDS`], plus a single-character candidate for
/// every syllable of the table.
///
/// A lookup materializes its words into a `Vec`, exactly as the frozen contract describes
/// the Phase 1 dictionary doing, so the allocation a lookup costs is on the measured path
/// rather than optimized away.
struct BenchLexicon {
    /// Word text per key, strongest first.
    words: BTreeMap<&'static str, &'static [&'static str]>,
}

impl BenchLexicon {
    /// Builds the dictionary from [`WORDS`].
    fn new() -> Self {
        let words = WORDS.iter().copied().collect();
        Self { words }
    }
}

impl Lexicon for BenchLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let syllables = u8::try_from(key.split('\'').count()).unwrap_or(1);
        let mut words: Vec<WordRef<'_>> = Vec::new();
        if let Some(texts) = self.words.get(key) {
            for text in texts.iter().copied() {
                words.push(WordRef {
                    text,
                    weight: 1,
                    syl_count: syllables,
                    flags: WordFlags::empty(),
                });
            }
        }
        Ok(WordIter::from_vec(words))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
        // Every syllable of the table answers, so no cut of the input dies for want of a
        // single character: the fallback is what keeps the lattice at its widest.
        let words: Vec<WordRef<'_>> = match syllable_at(syl) {
            Some(spelling) if limit > 0 => vec![WordRef {
                text: spelling,
                weight: 1,
                syl_count: 1,
                flags: WordFlags::empty(),
            }],
            _ => Vec::new(),
        };
        Ok(WordIter::from_vec(words))
    }
}

/// A user-frequency source that has recorded nothing.
struct NoUser;

impl UserFreqSource for NoUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// The model the case scores with: one unigram per word of [`WORDS`], so the scorer's
/// lookups are on the measured path, and two bigrams so that the conditional term is not
/// always a miss.
fn model() -> InMemoryLm {
    let mut lm = InMemoryLm::new();
    for (_, texts) in WORDS {
        for text in texts.iter().copied() {
            lm.insert_unigram(text, -256);
        }
    }
    lm.insert_bigram("中", "国", -64);
    lm.insert_bigram("北", "京", -64);
    lm
}

/// Times one decode of [`INPUT`] with the shipped configuration.
fn bench_viterbi(criterion: &mut Criterion) {
    let lexicon = BenchLexicon::new();
    let user = NoUser;
    let lm = model();
    let decoder = Decoder::default();
    let request = DecodeRequest::new(INPUT);
    let mut group = criterion.benchmark_group("decode");
    group.bench_function("viterbi", |bencher| {
        bencher.iter(|| {
            let result = decoder.decode(black_box(&request), &lexicon, &user, &lm);
            // The candidate count is consumed rather than the whole result, which keeps the
            // drop of the result outside what the loop measures.
            black_box(result.candidates.len())
        });
    });
    group.finish();
}

criterion_group!(benches, bench_viterbi);
criterion_main!(benches);
