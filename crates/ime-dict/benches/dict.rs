//! Criterion benchmarks for the dictionary read path.
//!
//! Three claims are measured here: one `Lexicon::lookup` of a key of at most 48 bytes has to
//! stay inside the interactive budget the decode path is held to, one record-to-word
//! conversion -- which the decoder runs once per candidate -- has to stay in the tens of
//! nanoseconds, and a lookup whose word list fits the contract's inline capacity has to be
//! served without the heap. The fixture is built and mapped the way the compiler and the
//! addon do it -- a real container written by the writer, mapped read-only by the loader --
//! so the benchmarks measure the read path and its page-fault behaviour, not just the FST
//! traversal.
//!
//! The lookup cases are a set of three because the read path has two shapes: a key whose list
//! fits [`WORD_ITER_INLINE`] is served out of the inline slots and a longer one spills to the
//! heap, so `lookup_inline` and `lookup_spill` measure the two shapes apart while `lookup`
//! cycles both, which is the mix a decode sees.
//!
//! What this file cannot do is *count* allocations: a counting allocator needs
//! `#[global_allocator]` and an `unsafe impl GlobalAlloc`, and `unsafe` is confined to the
//! two FFI directories and `ime-dict/src/mmap.rs`. The zero-allocation claim therefore rests
//! on the buffer's own unit tests (`src/fst_index/word_buf_tests.rs`), which pin the spill
//! threshold and the order of a spilled list, and on [`check_fixture`] below, which pins the
//! input side: the two key families really do hold the word counts their cases assume.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};

use ime_dict::format::{DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer};
use ime_dict::fst_index::FstLexicon;
use ime_types::Lexicon;
use ime_types::lexicon::WORD_ITER_INLINE;

/// How many keys the fixture holds whose list fits the inline capacity.
///
/// Large enough that the index and the entry table do not fit in the first-level cache,
/// which is the case the lookup budget is stated for.
const FIXTURE_KEYS: usize = 2_000;

/// How many keys the fixture holds whose list is longer than the inline capacity.
const FIXTURE_SPILL_KEYS: usize = 128;

/// How many words a spill key holds: enough to be past the inline capacity, not more.
const FIXTURE_SPILL_WORDS: usize = WORD_ITER_INLINE + 4;

/// The mapped fixture, and the key families the cases cycle through.
struct Fixture {
    /// The dictionary, loaded through the same path the addon uses.
    lexicon: FstLexicon,
    /// Keys holding one word each: a lookup of one stays inline and allocates nothing.
    inline_keys: Vec<String>,
    /// Keys holding [`FIXTURE_SPILL_WORDS`] words each: a lookup of one spills to the heap.
    spill_keys: Vec<String>,
    /// The records the fixture serves, for the record-to-word case.
    records: Vec<DictEntry>,
}

/// Builds a dictionary of two- and three-syllable keys, maps it and returns it with the keys
/// it holds.
///
/// The keys are drawn from the real syllable table, so the lookups walk a dictionary that is
/// shaped like the shipped one. Returns `None` when the fixture cannot be built or mapped;
/// the benchmark then registers nothing, because a benchmark is not a place to abort a run:
/// `unwrap` and `expect` are not available to non-test code in this workspace, and a missing
/// fixture is a reason to skip rather than to fail.
fn loaded_fixture() -> Option<Fixture> {
    let syllables = ime_core::segment::SYLLABLES;
    if syllables.len() < 2 {
        return None;
    }
    let mut keys: Vec<(String, Vec<String>)> =
        Vec::with_capacity(FIXTURE_KEYS + FIXTURE_SPILL_KEYS);
    for serial in 0..FIXTURE_KEYS {
        // The serial enters the key through the quotient as well as through the two syllable
        // positions. Drawn from the positions alone the keys would repeat every
        // `SYLLABLES.len()` serials -- 411 of them -- and the FST builder refuses a key it has
        // already seen, so the whole fixture would come up empty.
        let first = syllables[serial % syllables.len()];
        let second =
            syllables[(serial * 7 + 3 + (serial / syllables.len()) * 29) % syllables.len()];
        keys.push((
            format!("{first}'{second}"),
            vec![format!("{first}{second}")],
        ));
    }
    for serial in 0..FIXTURE_SPILL_KEYS {
        // Three syllables, so these keys cannot collide with a two-syllable key whatever the
        // serials are; the positions alone keep them distinct from each other while there are
        // fewer of them than the table has entries.
        let first = syllables[serial % syllables.len()];
        let second = syllables[(serial * 5 + 2) % syllables.len()];
        let third = syllables[(serial * 11 + 7) % syllables.len()];
        let readings = (0..FIXTURE_SPILL_WORDS)
            .map(|reading| format!("{first}{second}{third}{reading}"))
            .collect();
        keys.push((format!("{first}'{second}'{third}"), readings));
    }
    // The FST builder takes its keys in byte order, and it is the order of the packed values
    // that carries each key's word-list range, so sorting here changes nothing but the order
    // the sections are written in.
    keys.sort_by(|(left, _), (right, _)| left.cmp(right));

    // Word ids follow hash order, as the compiler assigns them.
    let mut words: Vec<(String, u8)> = Vec::new();
    for (key, readings) in &keys {
        let separators = key.matches('\'').count();
        let span = u8::try_from(separators + 1).ok()?;
        for reading in readings {
            words.push((reading.clone(), span));
        }
    }
    words.sort_by_key(|(word, _)| hash_word(word));
    let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
    for (index, (word, _)) in words.iter().enumerate() {
        ids.insert(word.as_str(), u32::try_from(index).ok()?);
    }

    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut records = Vec::with_capacity(words.len());
    let mut unigram = Vec::new();
    for (word, span) in &words {
        let offset = u32::try_from(strpool.len()).ok()?;
        let len = u16::try_from(word.len()).ok()?;
        strpool.extend_from_slice(word.as_bytes());
        let record = DictEntry::new(offset, len, *span, 0, 10_000);
        entries.extend_from_slice(&record.encode());
        records.push(record);
        unigram.extend_from_slice(&hash_word(word).to_le_bytes());
        unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }

    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, readings) in &keys {
        let start = u64::try_from(wordlist.len() / 4).ok()?;
        for reading in readings {
            let id = *ids.get(reading.as_str())?;
            wordlist.extend_from_slice(&id.to_le_bytes());
        }
        let count = u32::try_from(readings.len()).ok()?;
        let packed = pack_fst_value(start, count).ok()?;
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

    let mut inline_keys = Vec::with_capacity(FIXTURE_KEYS);
    let mut spill_keys = Vec::with_capacity(FIXTURE_SPILL_KEYS);
    for (key, readings) in keys {
        if readings.len() > WORD_ITER_INLINE {
            spill_keys.push(key);
        } else {
            inline_keys.push(key);
        }
    }
    Some(Fixture {
        lexicon,
        inline_keys,
        spill_keys,
        records,
    })
}

/// Checks the fixture is the one the cases describe, and stops the run when it is not.
///
/// A benchmark over a mis-shaped fixture reports the cost of a path no caller ever takes,
/// which is worse than no number at all: the two families are checked to hold the word counts
/// their cases assume, and a spill key is checked to be served whole -- the read path must not
/// cut a list short to stay inline.
///
/// # Panics
///
/// When the fixture does not hold the two families the cases cycle through, or when a lookup
/// does not serve the words the container holds.
fn check_fixture(fixture: &Fixture) {
    assert!(
        !fixture.inline_keys.is_empty() && !fixture.spill_keys.is_empty(),
        "the fixture must hold both key families"
    );
    // The spill family has to outrun the inline capacity, or the fixture never reaches the
    // heap path it exists to exercise. Checked at compile time rather than at run time:
    // both values are constants, so a runtime assertion would only be a slower way to say
    // the same thing.
    const _: () = assert!(FIXTURE_SPILL_WORDS > WORD_ITER_INLINE);
    // A failed lookup is reported as no words at all, so the two counts below fail the run
    // rather than silently measuring an empty dictionary.
    let inline = fixture
        .lexicon
        .lookup(&fixture.inline_keys[0])
        .map(|iter| iter.count());
    assert_eq!(inline.ok(), Some(1), "an inline key holds exactly one word");
    let spill = fixture
        .lexicon
        .lookup(&fixture.spill_keys[0])
        .map(|iter| iter.count());
    assert_eq!(
        spill.ok(),
        Some(FIXTURE_SPILL_WORDS),
        "a list past the inline capacity is served whole, not truncated"
    );
}

/// Registers one lookup case cycling through `keys`, one key per iteration.
///
/// Cycling the whole family rather than repeating one key is what keeps the measurement
/// honest: a single hot key would sit in the first-level cache and report a latency the
/// decode path never sees.
fn bench_lookup(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    lexicon: &FstLexicon,
    keys: &[&str],
) {
    if keys.is_empty() {
        return;
    }
    group.bench_function(name, |bencher| {
        let mut cursor = 0usize;
        bencher.iter(|| {
            let key = keys[cursor % keys.len()];
            cursor = cursor.wrapping_add(1);
            let served = lexicon.lookup(black_box(key));
            black_box(served.map(|iter| iter.count()))
        });
    });
}

/// Benchmarks the steps of the dictionary read path: resolving a key to its words -- inline
/// and spilled apart -- and turning one record into the word it names.
fn dict_bench(c: &mut Criterion) {
    let Some(fixture) = loaded_fixture() else {
        return;
    };
    check_fixture(&fixture);
    let inline: Vec<&str> = fixture.inline_keys.iter().map(String::as_str).collect();
    let spill: Vec<&str> = fixture.spill_keys.iter().map(String::as_str).collect();
    let mixed: Vec<&str> = inline
        .iter()
        .copied()
        .chain(spill.iter().copied())
        .collect();

    let mut group = c.benchmark_group("dict");
    bench_lookup(&mut group, "lookup", &fixture.lexicon, &mixed);
    bench_lookup(&mut group, "lookup_inline", &fixture.lexicon, &inline);
    bench_lookup(&mut group, "lookup_spill", &fixture.lexicon, &spill);
    // One conversion per call, which is what the decoder runs once per candidate: the record
    // is checked against the string pool on every access, so this measures the four bounds
    // checks and the borrow rather than a lookup.
    group.bench_function("entry_to_ref", |bencher| {
        let mut cursor = 0usize;
        bencher.iter(|| {
            let record = &fixture.records[cursor % fixture.records.len()];
            cursor = cursor.wrapping_add(1);
            let served = fixture.lexicon.entry_to_ref(black_box(record));
            black_box(served.map(|word| word.text.len()))
        });
    });
    group.finish();
}

criterion_group!(benches, dict_bench);
criterion_main!(benches);
