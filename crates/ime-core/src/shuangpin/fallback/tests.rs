//! Unit tests for the mixed-input fallback ladder.
//!
//! Responsibility: pin the three readings of one position -- the scheme's own, the
//! full-pinyin fallback and the literal keystroke -- the order they are tried in, and
//! the rule that decides which keystrokes may be carried into the rewritten text.
//!
//! Boundaries: everything here is in memory. Every table is either one this build
//! ships or one built by hand in this file, and no test touches a file, the clock, the
//! environment or a display server.

use ime_types::{ImeError, SchemeId};

use crate::segment::syllable::{MAX_SYLLABLE_LEN, syllable_at};
use crate::shuangpin::{SCHEME_KEYS, SchemeTable, TABLES, xiaohe, ziguang};

use super::{FallbackPolicy, ResolvedSyllable, SyllableOrigin, is_carriable, resolve};

/// No finals at all: the entry every key of a table that maps nothing carries.
const NO_FINALS: &[&str] = &[];

/// A table that maps nothing.
///
/// This is the shape a custom table the user never filled in has, so the full-pinyin
/// reading is the only one it leaves -- which is what makes it a clean fixture for the
/// fallback ladder on its own.
fn unmapped_table() -> SchemeTable {
    SchemeTable {
        id: SchemeId::XIAOHE,
        name: "unmapped",
        initials: [None; SCHEME_KEYS],
        finals: [NO_FINALS; SCHEME_KEYS],
        standalone: [""; SCHEME_KEYS],
    }
}

/// A table whose `a` key opens a syllable on its own and pairs with nothing.
///
/// The pair is tried before the key on its own, so a table whose initials are all empty
/// is what reaches that second reading while a second key is still in the stream.
fn standalone_only_table() -> SchemeTable {
    let mut standalone = [""; SCHEME_KEYS];
    standalone[0] = "a";
    SchemeTable {
        id: SchemeId::XIAOHE,
        name: "standalone-only",
        initials: [None; SCHEME_KEYS],
        finals: [NO_FINALS; SCHEME_KEYS],
        standalone,
    }
}

/// Resolves one position, failing the test when the resolution itself fails.
fn resolved(table: &SchemeTable, keys: &[u8], policy: FallbackPolicy) -> ResolvedSyllable {
    let mut scratch = String::new();
    let resolution = resolve(table, keys, policy, &mut scratch);
    match resolution {
        Ok(Some(resolution)) => resolution,
        other => panic!("expected a resolution, got {other:?}"),
    }
}

/// The spelling a resolution produced, or `None` for a literal keystroke.
fn spelled(resolution: ResolvedSyllable) -> Option<&'static str> {
    resolution.full().and_then(syllable_at)
}

/// Walks a whole keystroke stream the way the rewrite does, and returns the spelling it
/// produced together with the origin of every position.
fn walk(table: &SchemeTable, keys: &[u8], policy: FallbackPolicy) -> (String, Vec<SyllableOrigin>) {
    let mut scratch = String::new();
    let mut text = String::new();
    let mut origins = Vec::new();
    let mut at = 0usize;
    while at < keys.len() {
        let resolution = resolve(table, &keys[at..], policy, &mut scratch)
            .expect("a table of this build is implemented")
            .expect("a non-empty stream has a position");
        origins.push(resolution.origin());
        match resolution.full() {
            Some(id) => text.push_str(syllable_at(id).unwrap_or("")),
            None => text.push(char::from(keys[at])),
        }
        at += usize::from(resolution.consumed());
    }
    assert_eq!(at, keys.len(), "the walk consumes the whole stream");
    (text, origins)
}

#[test]
fn test_resolve_empty_keys_returns_none() {
    let mut scratch = String::new();
    for &(_, table) in TABLES.iter() {
        let resolution = resolve(table, b"", FallbackPolicy::KEEP_FULL_PINYIN, &mut scratch);
        assert!(matches!(resolution, Ok(None)), "nothing to resolve");
    }
}

#[test]
fn test_resolve_scheme_reading_wins_over_full_pinyin() {
    // `ai` is chi in Ziguang and a legal full-pinyin syllable on its own: the layout
    // the user chose is the layout they expect, so the scheme's reading is taken and
    // the fallback is never consulted.
    let resolution = resolved(&ziguang::ZIGUANG, b"ai", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Scheme);
    assert_eq!(spelled(resolution), Some("chi"));
    assert_eq!(resolution.consumed(), 2);
}

#[test]
fn test_resolve_scheme_standalone_consumes_one_key() {
    let resolution = resolved(&xiaohe::XIAOHE, b"a", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Scheme);
    assert_eq!(spelled(resolution), Some("a"));
    assert_eq!(resolution.consumed(), 1);
}

#[test]
fn test_resolve_pair_that_cannot_compose_falls_back_to_the_key_on_its_own() {
    // The pair is tried before the key on its own, and a pair that composes nothing
    // leaves the position to that second reading: one key is consumed and the other is
    // left for the caller's next position, rather than the two being swallowed by a
    // pair that did not resolve.
    let table = standalone_only_table();
    let resolution = resolved(&table, b"ab", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Scheme);
    assert_eq!(spelled(resolution), Some("a"));
    assert_eq!(resolution.consumed(), 1, "the second key is not consumed");

    // What the cursor lands on next is that second key, which this table maps nothing
    // for and which opens no full-pinyin syllable either.
    let rest = resolved(&table, b"b", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(rest.origin(), SyllableOrigin::Literal);
    assert_eq!(rest.full(), None);
    assert_eq!(rest.consumed(), 1);
}

#[test]
fn test_full_pinyin_fallback_inside_scheme_session() {
    // Xiaohe gives `m` no standalone syllable, but `m` is a full-pinyin one.
    let resolution = resolved(&xiaohe::XIAOHE, b"m", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::FullPinyinFallback);
    assert_eq!(spelled(resolution), Some("m"));
    assert_eq!(resolution.consumed(), 1);
}

#[test]
fn test_resolve_full_pinyin_fallback_spans_a_longer_syllable() {
    // Three keys the scheme cannot pair into anything, and a three-letter syllable.
    let resolution = resolved(&xiaohe::XIAOHE, b"hng", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::FullPinyinFallback);
    assert_eq!(spelled(resolution), Some("hng"));
    assert_eq!(resolution.consumed(), 3);
}

#[test]
fn test_literal_fallback_keeps_input_typeable() {
    // `b` is neither a Xiaohe syllable nor a full-pinyin one, so the keystroke itself
    // is the resolution -- and it can be typed, because the normalizer keeps it.
    let resolution = resolved(&xiaohe::XIAOHE, b"b", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Literal);
    assert_eq!(resolution.full(), None);
    assert_eq!(resolution.consumed(), 1);
    assert!(is_carriable('b'), "the keystroke survives the normalizer");
}

#[test]
fn test_resolve_scheme_only_policy_turns_the_fallback_off() {
    let fallback = resolved(&xiaohe::XIAOHE, b"m", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(fallback.origin(), SyllableOrigin::FullPinyinFallback);

    let literal = resolved(&xiaohe::XIAOHE, b"m", FallbackPolicy::SCHEME_ONLY);
    assert_eq!(literal.origin(), SyllableOrigin::Literal);
    assert_eq!(literal.full(), None);
}

#[test]
fn test_fallback_policy_from_keep_full_pinyin_matches_the_constants() {
    assert!(FallbackPolicy::from_keep_full_pinyin(true).keeps_full_pinyin());
    assert!(!FallbackPolicy::from_keep_full_pinyin(false).keeps_full_pinyin());
    let off = FallbackPolicy::from_keep_full_pinyin(false);
    assert_eq!(off, FallbackPolicy::SCHEME_ONLY);
}

#[test]
fn test_resolve_reports_a_keystroke_the_scheme_does_not_define() {
    // The semicolon is part of the scheme key alphabet but not a key of Xiaohe's
    // table. It must not vanish: it comes back as a literal, and the caller is told
    // that it cannot be carried into the text.
    let resolution = resolved(&xiaohe::XIAOHE, b";", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Literal);
    assert_eq!(resolution.consumed(), 1);
    assert!(!is_carriable(';'), "the normalizer discards it");
}

#[test]
fn test_resolve_reports_a_byte_outside_the_key_alphabet() {
    let resolution = resolved(&xiaohe::XIAOHE, b"3", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::Literal);
    assert_eq!(resolution.full(), None);
    assert!(!is_carriable('3'), "the normalizer discards it");
}

#[test]
fn test_resolve_reports_a_scheme_this_build_does_not_implement() {
    let mut table = xiaohe::XIAOHE;
    table.id = SchemeId::from_u8(SchemeId::COUNT);
    let mut scratch = String::new();
    let resolution = resolve(
        &table,
        b"vs",
        FallbackPolicy::KEEP_FULL_PINYIN,
        &mut scratch,
    );
    match resolution {
        Err(ImeError::SchemeUnsupported { scheme }) => assert_eq!(scheme, SchemeId::COUNT),
        other => panic!("expected decode/scheme-unsupported, got {other:?}"),
    }
}

#[test]
fn test_resolve_folds_the_umlaut_key_in_a_full_pinyin_fallback() {
    let table = unmapped_table();
    let resolution = resolved(&table, b"lv", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(resolution.origin(), SyllableOrigin::FullPinyinFallback);
    assert_eq!(spelled(resolution), Some("lü"));
    assert_eq!(resolution.consumed(), 2, "two keystrokes");

    // After `j` the umlaut is written as a plain `u`, exactly as the segmentation
    // layer reads it.
    let resolution = resolved(&table, b"jv", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(spelled(resolution), Some("ju"));
}

#[test]
fn test_resolve_never_consumes_more_keys_than_the_stream_holds() {
    // The boundary of the full-pinyin reading: the longest syllable the table holds is
    // six keys, and a stream shorter than that must not be read past its end.
    for len in 1..=MAX_SYLLABLE_LEN {
        let keys = vec![b'a'; len];
        let resolution = resolved(&xiaohe::XIAOHE, &keys, FallbackPolicy::KEEP_FULL_PINYIN);
        assert!(usize::from(resolution.consumed()) <= len);
    }
}

#[test]
fn test_is_carriable_accepts_letters_and_refuses_what_the_normalizer_changes() {
    for key in "abcdefghijklmnopqrstuwxyz".chars() {
        assert!(is_carriable(key), "{key:?} is kept");
        assert!(is_carriable(key.to_ascii_uppercase()), "{key:?} is kept");
    }
    for key in ['v', 'V', ';', '\'', '3', ' ', 'ü'] {
        assert!(!is_carriable(key), "{key:?} is changed");
    }
}

#[test]
fn test_resolve_walks_a_full_pinyin_word_typed_in_a_scheme_session() {
    // The acceptance case: with Xiaohe active, `nihao` still spells ni'hao.
    let (text, origins) = walk(&xiaohe::XIAOHE, b"nihao", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(text, "nihao");
    assert_eq!(origins, [SyllableOrigin::Scheme; 3]);
}

#[test]
fn test_resolve_walks_a_stream_that_needs_the_fallback() {
    // `hng` is three keys the scheme cannot map and `m` is a fourth; the walk steps
    // over all of them and produces the full-pinyin reading of each.
    let (text, origins) = walk(&xiaohe::XIAOHE, b"hngm", FallbackPolicy::KEEP_FULL_PINYIN);
    assert_eq!(text, "hngm");
    assert_eq!(origins, [SyllableOrigin::FullPinyinFallback; 2]);
}
