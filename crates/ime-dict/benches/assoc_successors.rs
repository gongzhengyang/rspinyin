//! Criterion benchmark for interval-bound Bigram successor queries.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fst::MapBuilder;
use ime_dict::format::{
    BigramRecord, DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer,
};
use ime_dict::fst_index::{DictLm, FstLexicon};
use ime_types::WordId;

/// Number of successors attached to the benchmarked predecessor.
const SUCCESSOR_COUNT: u32 = 64;

/// Builds the smallest valid container whose predecessor interval has many rows.
///
/// The image is leaked for `'static` and loaded through `from_image`, matching the other
/// dictionary benchmarks: model construction is outside the timed region and the query
/// reads no filesystem. Scores vary so ranking work is included with interval selection.
fn successor_model() -> Option<DictLm> {
    let mut index = MapBuilder::memory();
    let packed = pack_fst_value(0, 1).ok()?;
    index.insert("ni".as_bytes(), packed).ok()?;

    let mut container = writer::DictWriter::new();
    let mut unigram = hash_word("你").to_le_bytes().to_vec();
    unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
    unigram.extend_from_slice(&[0; 2]);

    let mut bigram = (0..SUCCESSOR_COUNT)
        .map(|next| {
            let offset = u16::try_from(next).unwrap_or(0);
            let score = PROB_Q12_MAX.saturating_sub(offset);
            BigramRecord::new(7 << 32 | u64::from(next), score)
        })
        .collect::<Vec<_>>();
    bigram.sort_by_key(|record| record.pair_key);
    let bigram = bigram
        .iter()
        .flat_map(|record| record.encode())
        .collect::<Vec<_>>();

    for (kind, payload) in [
        (SectionKind::Fst, index.into_inner().ok()?),
        (
            SectionKind::Entries,
            DictEntry::new(0, 3, 1, 0, 900)
                .with_characters(1)
                .encode()
                .to_vec(),
        ),
        (SectionKind::StrPool, "你".as_bytes().to_vec()),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, 0u32.to_le_bytes().to_vec()),
        (SectionKind::Bigram, bigram),
    ] {
        container.add_section(kind, payload).ok()?;
    }

    let image = container.encode().ok()?;
    let image: &'static [u8] = Box::leak(image.into_boxed_slice());
    let lexicon = Box::leak(Box::new(FstLexicon::from_image(image).ok()?));
    Some(DictLm::new(lexicon))
}

/// Measures locating a predecessor segment, reading it and ranking its first eight hits.
fn assoc_successors(criterion: &mut Criterion) {
    let Some(model) = successor_model() else {
        return;
    };
    criterion.bench_function("assoc_successors_limit_8", |bencher| {
        bencher.iter(|| black_box(&model).successors(black_box(WordId::new(7)), black_box(8)));
    });
}

criterion_group!(benches, assoc_successors);
criterion_main!(benches);
