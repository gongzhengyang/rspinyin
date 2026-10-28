//! Tests for the mapped language model's Bigram successor interval.

use ime_types::WordId;

use super::DictLm;
use crate::format::{
    BigramRecord, DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer,
};
use crate::fst_index::FstLexicon;

/// Loads a tiny container whose raw pair keys directly exercise interval boundaries.
///
/// Raw keys let a test name predecessor `7` and successor `8` without inverting the word
/// hash. The image is leaked intentionally: a language model borrows its mapping for
/// `'static`, and the handful of small test images live only until the process exits.
fn model_with_bigrams(bigrams: &[(u64, u16)]) -> DictLm {
    let mut index = fst::MapBuilder::memory();
    let packed = pack_fst_value(0, 1).expect("packing the word-list range");
    index
        .insert("ni".as_bytes(), packed)
        .expect("inserting the key");

    let mut unigram = hash_word("你").to_le_bytes().to_vec();
    unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
    unigram.extend_from_slice(&[0; 2]);

    let mut rows = bigrams
        .iter()
        .map(|(key, score)| BigramRecord::new(*key, *score))
        .collect::<Vec<_>>();
    rows.sort_by_key(|record| record.pair_key);
    let bigram = rows
        .iter()
        .flat_map(|record| record.encode())
        .collect::<Vec<_>>();

    let mut container = writer::DictWriter::new();
    for (kind, payload) in [
        (
            SectionKind::Fst,
            index.into_inner().expect("finishing the FST"),
        ),
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
        container
            .add_section(kind, payload)
            .expect("adding a section");
    }

    let image = container.encode().expect("encoding the container");
    let image: &'static [u8] = Box::leak(image.into_boxed_slice());
    let lexicon = FstLexicon::from_image(image).expect("loading the test image");
    DictLm::new(&lexicon)
}

#[test]
fn test_successors_orders_one_segment_by_score_and_provider_id() {
    let model = model_with_bigrams(&[
        (7 << 32 | 3, 1_024),
        (7 << 32 | 1, PROB_Q12_MAX),
        (7 << 32 | 2, PROB_Q12_MAX),
        (8 << 32 | 0, PROB_Q12_MAX),
    ]);

    let successors = model.successors(WordId::new(7), u8::MAX);
    let ids: Vec<_> = successors.iter().map(|(id, _)| id.value()).collect();
    assert_eq!(ids, [2, 1, 3]);
    assert_eq!(
        successors[0].1, 0,
        "the highest Q12 probability maps to score zero"
    );
    assert!(successors[1].1 < successors[0].1);
    assert!(successors[2].1 < successors[1].1);
}

#[test]
fn test_successors_truncates_at_the_predecessor_segment_end() {
    let model = model_with_bigrams(&[
        (7 << 32 | 0, PROB_Q12_MAX),
        (7 << 32 | 1, PROB_Q12_MAX),
        (8 << 32 | 2, PROB_Q12_MAX),
    ]);

    let successors = model.successors(WordId::new(7), u8::MAX);
    let ids: Vec<_> = successors.iter().map(|(id, _)| id.value()).collect();
    assert_eq!(
        ids,
        [0, 1],
        "the later predecessor must not enter the interval"
    );
}

#[test]
fn test_successors_applies_limit_after_score_ordering() {
    let model = model_with_bigrams(&[
        (7 << 32 | 1, 2_048),
        (7 << 32 | 2, PROB_Q12_MAX),
        (7 << 32 | 3, 4_096),
    ]);

    let successors = model.successors(WordId::new(7), 2);
    let ids: Vec<_> = successors.iter().map(|(id, _)| id.value()).collect();
    assert_eq!(ids, [2, 3]);
    assert!(model.successors(WordId::new(7), 0).is_empty());
}

#[test]
fn test_successors_of_absent_predecessor_returns_none() {
    let model = model_with_bigrams(&[(8 << 32 | 1, PROB_Q12_MAX)]);
    assert!(model.successors(WordId::new(7), u8::MAX).is_empty());
}
