//! Tests for the buffer one lookup writes its word list into.
//!
//! The buffer is what keeps a lookup off the allocator while the list fits the contract's
//! inline capacity, so these tests pin the two properties that promise rests on: where the
//! spill happens -- on the first word past the inline capacity and not before -- and that a
//! spilled list is served whole and in the order the container stores it. An allocation
//! counter would need `unsafe`, which this workspace confines to the FFI directories and
//! `mmap.rs`, so the no-allocation claim is pinned through the storage itself: the inline
//! path never calls `Vec::with_capacity`.

use ime_types::lexicon::WORD_ITER_INLINE;
use ime_types::{WordFlags, WordRef};

use super::WordBuf;

/// One word carrying `text`, with the other fields at a value no assertion depends on.
fn word(text: &str) -> WordRef<'_> {
    WordRef {
        text,
        weight: 1,
        syl_count: 1,
        flags: WordFlags::empty(),
    }
}

/// Twelve distinct words, which is more than the inline capacity holds.
const MANY: [&str; 12] = [
    "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二",
];

#[test]
fn test_word_buf_new_holds_nothing() {
    let buf = WordBuf::new();
    assert_eq!(buf.len, 0);
    assert!(buf.spill.is_empty(), "an empty buffer holds no allocation");
    assert_eq!(buf.into_word_iter().count(), 0);
}

#[test]
fn test_word_buf_holds_the_inline_capacity_without_spilling() {
    let mut buf = WordBuf::new();
    for (index, entry) in MANY.iter().take(WORD_ITER_INLINE).enumerate() {
        buf.push(word(entry));
        assert_eq!(buf.len, index + 1);
        assert!(buf.spill.is_empty(), "slot {index} stays inline");
    }
    let words: Vec<&str> = buf.into_word_iter().map(|kept| kept.text).collect();
    assert_eq!(words.as_slice(), &MANY[..WORD_ITER_INLINE]);
}

#[test]
fn test_word_buf_spills_on_the_first_word_past_the_inline_capacity() {
    let mut buf = WordBuf::new();
    for text in &MANY[..WORD_ITER_INLINE] {
        buf.push(word(text));
    }
    assert!(buf.spill.is_empty(), "the last inline slot is not a spill");
    buf.push(word(MANY[WORD_ITER_INLINE]));
    assert_eq!(
        buf.spill.len(),
        WORD_ITER_INLINE + 1,
        "the spill carries the inline head with it"
    );
    assert_eq!(buf.into_word_iter().count(), WORD_ITER_INLINE + 1);
}

#[test]
fn test_word_buf_serves_a_spilled_list_whole_and_in_order() {
    let mut buf = WordBuf::new();
    for text in MANY {
        buf.push(word(text));
    }
    assert!(!buf.spill.is_empty(), "a longer list moves to the heap");
    let words: Vec<&str> = buf.into_word_iter().map(|kept| kept.text).collect();
    assert_eq!(
        words.as_slice(),
        MANY.as_slice(),
        "a long list is served whole, not truncated"
    );
}

#[test]
fn test_word_buf_keeps_the_weight_of_every_word_it_spills() {
    let mut buf = WordBuf::new();
    for (index, text) in MANY.iter().enumerate() {
        buf.push(WordRef {
            text,
            weight: u32::try_from(index).expect("a small fixture"),
            syl_count: 1,
            flags: WordFlags::empty(),
        });
    }
    let weights: Vec<u32> = buf.into_word_iter().map(|kept| kept.weight).collect();
    let expected: Vec<u32> = (0..MANY.len())
        .map(|index| u32::try_from(index).expect("a small fixture"))
        .collect();
    assert_eq!(weights, expected, "the ranking order survives the spill");
}
