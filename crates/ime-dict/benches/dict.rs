//! Criterion benchmarks for the dictionary read path.
//!
//! The budget these exist for is the lookup latency: one `Lexicon::lookup` of a key of at
//! most 48 bytes has to stay inside the interactive budget the decode path is held to.
//! The fixture is built and mapped the way the compiler and the addon do it -- a real
//! container written by the writer, mapped read-only by the loader -- so the benchmark
//! measures the read path and its page-fault behaviour, not just the FST traversal.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use ime_dict::format::{DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer};
use ime_dict::fst_index::FstLexicon;
use ime_types::Lexicon;

/// How many keys the fixture holds.
///
/// Large enough that the index and the entry table do not fit in the first-level cache,
/// which is the case the lookup budget is stated for.
const FIXTURE_KEYS: usize = 2_000;

/// Builds a dictionary of two-syllable keys, maps it and returns the lexicon together
/// with the keys it holds.
///
/// The keys are drawn from the real syllable table, so the lookups walk a dictionary that
/// is shaped like the shipped one. Returns `None` when the fixture cannot be built or
/// mapped; the benchmark then registers nothing, because a benchmark is not a place to
/// abort a run: `unwrap` and `expect` are not available to non-test code in this
/// workspace, and a missing fixture is a reason to skip rather than to fail.
fn loaded_fixture() -> Option<(FstLexicon, Vec<String>)> {
    let syllables = ime_core::segment::SYLLABLES;
    if syllables.len() < 2 {
        return None;
    }
    let mut keys = Vec::with_capacity(FIXTURE_KEYS);
    for index in 0..FIXTURE_KEYS {
        let first = syllables[index % syllables.len()];
        let second = syllables[(index * 7 + 3) % syllables.len()];
        keys.push((format!("{first}'{second}"), format!("{first}{second}")));
    }

    // Word ids follow hash order, as the compiler assigns them.
    let mut words: Vec<&str> = keys.iter().map(|(_, word)| word.as_str()).collect();
    words.sort_by_key(|word| hash_word(word));
    let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
    for (index, word) in words.iter().enumerate() {
        ids.insert(*word, u32::try_from(index).ok()?);
    }

    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for word in &words {
        let offset = u32::try_from(strpool.len()).ok()?;
        let len = u16::try_from(word.len()).ok()?;
        strpool.extend_from_slice(word.as_bytes());
        entries.extend_from_slice(&DictEntry::new(offset, len, 2, 0, 10_000).encode());
        unigram.extend_from_slice(&hash_word(word).to_le_bytes());
        unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }

    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, word) in &keys {
        let id = *ids.get(word.as_str())?;
        let start = u64::try_from(wordlist.len() / 4).ok()?;
        wordlist.extend_from_slice(&id.to_le_bytes());
        let packed = pack_fst_value(start, 1).ok()?;
        fst_builder.insert(key.as_bytes(), packed).ok()?;
    }

    let mut writer = writer::DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst_builder.into_inner().ok()?)
        .ok()?;
    writer.add_section(SectionKind::Entries, entries).ok()?;
    writer.add_section(SectionKind::StrPool, strpool).ok()?;
    writer.add_section(SectionKind::Unigram, unigram).ok()?;
    writer.add_section(SectionKind::WordList, wordlist).ok()?;
    let image = writer.encode().ok()?;

    let dir = std::env::temp_dir().join(format!("rspinyin-dict-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("base.dict");
    std::fs::write(&path, image).ok()?;
    let lexicon = FstLexicon::load(&path).ok()?;
    // The mapping keeps the inode alive after the unlink, so the fixture leaves nothing
    // behind even though the lexicon is still reading it.
    std::fs::remove_dir_all(&dir).ok()?;
    Some((lexicon, keys.into_iter().map(|(key, _)| key).collect()))
}

/// Benchmarks one lookup per key, cycling through the whole key set.
///
/// Cycling rather than repeating one key is what keeps the measurement honest: a single
/// hot key would sit in the first-level cache and report a latency the decode path never
/// sees.
fn lookup_bench(c: &mut Criterion) {
    let Some((lexicon, keys)) = loaded_fixture() else {
        return;
    };
    let mut group = c.benchmark_group("dict");
    group.bench_function("lookup", |bencher| {
        let mut cursor = 0usize;
        bencher.iter(|| {
            let key = &keys[cursor % keys.len()];
            cursor = cursor.wrapping_add(1);
            let served = lexicon.lookup(black_box(key));
            black_box(served.map(|iter| iter.count()))
        });
    });
    group.finish();
}

criterion_group!(benches, lookup_bench);
criterion_main!(benches);
