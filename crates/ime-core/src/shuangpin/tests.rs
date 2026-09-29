//! Cross-scheme tests: the properties every table has to satisfy, the mapping's edge
//! cases, and the rewrite that turns keystrokes into the spelling the segmentation
//! layer reads.

use ime_types::{DecodeFlags, DecodeRequest, ImeError, SchemeId};

use crate::lm::InMemoryLm;
use crate::segment::syllable::{MAX_RAW_LEN, MAX_SYLLABLE_LEN, normalize, syllable_at};
use crate::shuangpin::{
    SCHEME_KEYS, SEMICOLON_INDEX, SchemeMap, SchemeTable, TABLES, is_rewrite_requested, key_index,
    map_syllable, microsoft, rewrite_for_scheme, rewrite_request, sogou, table_for, xiaohe,
    ziguang, ziranma,
};
use crate::viterbi::Decoder;
use crate::viterbi::lattice::testing::{MockLexicon, NoUser};

/// Every scheme table this build ships, with the name a failure message uses.
fn tables() -> [(&'static str, SchemeId, &'static SchemeTable); 5] {
    [
        ("xiaohe", SchemeId::XIAOHE, &xiaohe::XIAOHE),
        ("ziranma", SchemeId::ZIRANMA, &ziranma::ZIRANMA),
        ("microsoft", SchemeId::MICROSOFT, &microsoft::MICROSOFT),
        ("sogou", SchemeId::SOGOU, &sogou::SOGOU),
        ("ziguang", SchemeId::ZIGUANG, &ziguang::ZIGUANG),
    ]
}

/// Every key a scheme table is indexed by: `a`-`z` and `;`.
fn alphabet() -> Vec<u8> {
    let mut keys: Vec<u8> = (b'a'..=b'z').collect();
    keys.push(b';');
    keys
}

/// Every spelling a scheme's key matrix can reach, one entry per key sequence.
fn reachable(table: &SchemeTable) -> Vec<&'static str> {
    let mut out = Vec::new();
    for &first in &alphabet() {
        for &second in &alphabet() {
            let pair = [first, second];
            if let Some(spelling) = map_syllable(table, &pair)
                .expect("letters are scheme keys")
                .and_then(|mapped| syllable_at(mapped.full))
            {
                out.push(spelling);
            }
        }
        if let Some(spelling) = map_syllable(table, &[first])
            .expect("letters are scheme keys")
            .and_then(|mapped| syllable_at(mapped.full))
        {
            out.push(spelling);
        }
    }
    out
}

#[test]
fn test_key_index_covers_the_twenty_seven_key_alphabet() {
    assert_eq!(key_index(b'a'), Some(0));
    assert_eq!(key_index(b'z'), Some(25));
    assert_eq!(key_index(b';'), Some(SEMICOLON_INDEX));
    assert_eq!(SCHEME_KEYS, 27);
    for key in alphabet() {
        assert!(key_index(key).is_some(), "{key:?} must be a scheme key");
    }
    // A byte string rather than an array of byte literals: `A`, `0`, the quote, the space
    // and the tilde are exactly the five bytes this spells.
    for key in *b"A0' ~" {
        assert_eq!(key_index(key), None, "{key:?} must not be a scheme key");
    }
}

#[test]
fn test_map_syllable_empty_input_returns_none() {
    for (name, _, table) in tables() {
        assert_eq!(
            map_syllable(table, b"").expect("an empty key list is legal input"),
            None,
            "{name} must not invent a syllable for no keys"
        );
    }
}

#[test]
fn test_map_syllable_rejects_a_character_outside_the_key_alphabet() {
    let first = map_syllable(&xiaohe::XIAOHE, b"3a");
    assert!(matches!(
        first,
        Err(ImeError::DecodeInvalidChar { ch: '3', at: 0 })
    ));
    let second = map_syllable(&xiaohe::XIAOHE, b"a3");
    assert!(matches!(
        second,
        Err(ImeError::DecodeInvalidChar { ch: '3', at: 1 })
    ));
}

#[test]
fn test_map_syllable_reports_a_scheme_this_build_does_not_implement() {
    // A table naming a scheme a newer build wrote is malformed input to this one, and
    // the contract's `decode/scheme-unsupported` is what it has to raise.
    let mut table = xiaohe::XIAOHE;
    table.id = SchemeId::from_u8(SchemeId::COUNT);
    let mapped = map_syllable(&table, b"vs");
    match mapped {
        Err(ImeError::SchemeUnsupported { scheme }) => {
            assert_eq!(scheme, SchemeId::COUNT);
        }
        other => panic!("expected decode/scheme-unsupported, got {other:?}"),
    }
}

#[test]
fn test_map_syllable_consumes_two_keys_for_a_five_letter_final() {
    // `iong` is five letters and still two keys: the final's key carries the whole
    // spelling, so nothing about the pair is three keys long.
    let mapped = map_syllable(&xiaohe::XIAOHE, b"js")
        .expect("keys inside the alphabet")
        .expect("jiong must map");
    assert_eq!(mapped.consumed, 2);
    assert_eq!(syllable_at(mapped.full), Some("jiong"));
}

#[test]
fn test_map_syllable_reads_only_the_first_two_keys() {
    let pair = map_syllable(&xiaohe::XIAOHE, b"vs").expect("keys inside the alphabet");
    let longer = map_syllable(&xiaohe::XIAOHE, b"vsgo").expect("keys inside the alphabet");
    assert_eq!(pair, longer);
}

#[test]
fn test_map_syllable_returns_none_for_a_pair_the_scheme_cannot_spell() {
    // `b` opens a syllable and `z` stands for `ou`, but `bou` is not a syllable, and
    // neither is any other final `z` carries. The answer is a report, not a guess.
    assert_eq!(
        map_syllable(&xiaohe::XIAOHE, b"bz").expect("keys inside the alphabet"),
        None
    );
}

#[test]
fn test_map_syllable_falls_back_to_a_standalone_letter() {
    let mapped = map_syllable(&xiaohe::XIAOHE, b"a")
        .expect("keys inside the alphabet")
        .expect("a is a syllable on its own");
    assert_eq!(mapped.consumed, 1);
    assert_eq!(syllable_at(mapped.full), Some("a"));
    assert_eq!(
        map_syllable(&xiaohe::XIAOHE, b"b").expect("keys inside the alphabet"),
        None
    );
}

#[test]
fn test_every_pair_in_the_key_matrix_lands_on_a_syllable_of_the_table() {
    // The 27x27 matrix every scheme is defined by. No pair may raise an error, and
    // every syllable one does produce has to be an entry of the syllable table, no
    // longer than the table's longest.
    for (name, _, table) in tables() {
        for &first in &alphabet() {
            for &second in &alphabet() {
                let pair = [first, second];
                let mapped = map_syllable(table, &pair).expect("letters are scheme keys");
                let Some(mapped) = mapped else {
                    continue;
                };
                assert!(
                    (1..=2).contains(&mapped.consumed),
                    "{name}: {first}{second} consumed {} keys",
                    mapped.consumed
                );
                let spelling = syllable_at(mapped.full).expect("a mapped syllable is in the table");
                assert!(
                    !spelling.is_empty(),
                    "{name}: {first}{second} spelled nothing"
                );
                assert!(
                    spelling.len() <= MAX_SYLLABLE_LEN,
                    "{name}: {first}{second} spelled {spelling:?}, longer than the table allows"
                );
            }
        }
    }
}

#[test]
fn test_every_scheme_can_spell_the_common_syllables() {
    // Coverage, computed rather than tabulated: each of these has to be reachable from
    // some key sequence of the scheme, whichever one the layout chose.
    let required = [
        "ni", "hao", "zhong", "guo", "ren", "min", "shi", "jie", "xue", "xi", "huan", "ying", "an",
        "ai", "en", "wo", "de", "le", "zai", "you", "jia", "qing", "hua", "er",
    ];
    for (name, _, table) in tables() {
        let spellings = reachable(table);
        for syllable in required {
            assert!(
                spellings.contains(&syllable),
                "{name} cannot spell {syllable:?} with any key sequence"
            );
        }
    }
}

#[test]
fn test_every_table_names_a_distinct_scheme() {
    let mut seen: Vec<u8> = Vec::new();
    for (name, scheme, table) in tables() {
        assert!(
            scheme.is_known(),
            "{name} must be a scheme this build knows"
        );
        assert_eq!(table.id, scheme, "{name} must identify itself");
        assert!(!table.name.is_empty(), "{name} must carry a display name");
        assert!(
            !seen.contains(&scheme.value()),
            "{name} repeats a scheme number"
        );
        seen.push(scheme.value());
    }
    assert_eq!(seen.len(), usize::from(SchemeId::COUNT) - 1);
}

#[test]
fn test_the_registered_table_list_matches_the_schemes_it_names() {
    // `TABLES` is the one place a scheme is registered, so it has to agree with the
    // tables themselves: same count, and each table identifying itself by the number it
    // is filed under.
    assert_eq!(TABLES.len(), tables().len());
    for (name, scheme, table) in tables() {
        let registered = table_for(scheme)
            .expect("a registered scheme is implemented")
            .expect("a registered scheme has a table");
        assert_eq!(
            registered.id, scheme,
            "{name} is filed under the wrong number"
        );
        assert_eq!(
            registered.name, table.name,
            "{name} is not the table registered"
        );
    }
    assert!(
        !TABLES.iter().any(|(id, _)| *id == SchemeId::FULL),
        "full pinyin is the identity and must not be registered with a table"
    );
}

#[test]
fn test_table_for_full_pinyin_needs_no_table() {
    // Full pinyin is the identity, so there is no table to hand back -- and that is a
    // success, not a failure.
    assert!(
        table_for(SchemeId::FULL)
            .expect("full pinyin is supported")
            .is_none()
    );
}

#[test]
fn test_table_for_rejects_a_scheme_from_a_newer_build() {
    let future = SchemeId::from_u8(SchemeId::COUNT);
    match table_for(future) {
        Err(ImeError::SchemeUnsupported { scheme }) => assert_eq!(scheme, SchemeId::COUNT),
        other => panic!("expected decode/scheme-unsupported, got {other:?}"),
    }
}

#[test]
fn test_rewrite_for_scheme_is_the_identity_for_full_pinyin() {
    // Zero copy, which is the whole reason the hook takes a `Cow`.
    let identity = rewrite_for_scheme("zhongguo", SchemeId::FULL);
    let identity = identity.expect("full pinyin is supported");
    assert!(matches!(identity, std::borrow::Cow::Borrowed("zhongguo")));
}

#[test]
fn test_rewrite_for_scheme_rejects_a_scheme_this_build_does_not_implement() {
    let rewritten = rewrite_for_scheme("nihao", SchemeId::from_u8(SchemeId::COUNT));
    assert!(matches!(
        rewritten,
        Err(ImeError::SchemeUnsupported { scheme }) if scheme == SchemeId::COUNT
    ));
}

#[test]
fn test_rewrite_for_scheme_rejects_input_past_the_length_limit() {
    // The limit is judged on the keystrokes, exactly as the segmentation layer judges
    // it on a full-pinyin input.
    let raw = "a".repeat(MAX_RAW_LEN + 1);
    let rewritten = rewrite_for_scheme(&raw, SchemeId::XIAOHE);
    assert!(matches!(
        rewritten,
        Err(ImeError::DecodeTooLong { len, max }) if len == MAX_RAW_LEN + 1 && max == MAX_RAW_LEN
    ));
    let at_limit = "a".repeat(MAX_RAW_LEN);
    assert!(rewrite_for_scheme(&at_limit, SchemeId::XIAOHE).is_ok());
}

#[test]
fn test_rewrite_carries_an_unmappable_keystroke_through_and_reports_it() {
    let map = SchemeMap::build("bz", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.text(), "bz");
    assert_eq!(map.syllables(), 2);
    assert_eq!(map.unmapped_count(), 2);
    assert!(map.is_unmapped(0));
    assert!(map.is_unmapped(1));
}

#[test]
fn test_rewrite_drops_a_key_the_scheme_does_not_define() {
    // Xiaohe has no use for `;`, so the key is not part of its alphabet: it is dropped
    // rather than mapped to an empty syllable or carried into the spelling.
    let map = SchemeMap::build("a;", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.text(), "a");
    assert_eq!(map.syllables(), 1);
    assert_eq!(map.unmapped_count(), 0);
}

#[test]
fn test_rewrite_keeps_a_forced_boundary_marker() {
    let map = SchemeMap::build("ni'hc", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.text(), "ni'hao");
    assert_eq!(map.syllables(), 2);
    assert_eq!(map.full_span(0), Some((0, 2)));
    // The second syllable's span begins on the marker, the way the segmentation graph
    // reports a pinned boundary too.
    assert_eq!(map.full_span(1), Some((2, 6)));

    // A repeated marker carries the same single boundary; the segmentation layer's
    // normalizer folds the surplus one away, and so does the rewrite.
    let doubled = SchemeMap::build("ni''hc", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(doubled.text(), "ni'hao");
}

#[test]
fn test_rewrite_folds_markers_that_carry_no_information() {
    // A leading marker pins nothing, and a trailing one is folded away because the
    // syllable after it never arrives.
    for raw in ["''nihc", "nihc'", "'nihc'"] {
        let map = SchemeMap::build(raw, SchemeId::XIAOHE)
            .expect("xiaohe is implemented")
            .expect("a scheme rewrites");
        assert_eq!(map.text(), "nihao", "rewriting {raw:?}");
    }
}

#[test]
fn test_rewrite_is_a_fixed_point_of_the_normalizer() {
    // The alignment this map reports is only exact while the text it produces is
    // already in the segmentation layer's canonical form: a normalizer that changed
    // one byte would move every offset after it.
    let cases = [
        "nihc", "vsgo", "a", "bz", "''nihc", "nihc'", "ni'hc", "o;", "vx", "qqqq", ";", "VSHC",
    ];
    for (name, scheme, _) in tables() {
        for raw in cases {
            let map = SchemeMap::build(raw, scheme)
                .expect("the schemes are implemented")
                .expect("a scheme rewrites");
            let normalized = normalize(map.text());
            assert_eq!(
                normalized.text,
                map.text(),
                "{name}: {raw:?} is not canonical"
            );
            assert!(
                normalized.dropped.is_empty(),
                "{name}: {raw:?} drops bytes the normalizer would have removed"
            );
        }
    }
}

#[test]
fn test_rewrite_folds_a_bare_v_because_the_normalizer_would_rewrite_it() {
    // `v` is the one letter the normalizer rewrites, into `ü`. A scheme that cannot
    // spell the keystroke must not carry it through, or the text would grow behind the
    // caller's back.
    let map = SchemeMap::build("vx", SchemeId::MICROSOFT)
        .expect("microsoft is implemented")
        .expect("a scheme rewrites");
    assert!(!map.text().contains('v'), "text is {:?}", map.text());
}

#[test]
fn test_scheme_map_returns_none_for_full_pinyin() {
    assert!(
        SchemeMap::build("nihao", SchemeId::FULL)
            .expect("full pinyin is supported")
            .is_none()
    );
}

#[test]
fn test_scheme_span_maps_a_whole_word_back_to_its_keystrokes() {
    let map = SchemeMap::build("vsgo", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.text(), "zhongguo");
    assert_eq!(map.syllables(), 2);
    // A segment covering both syllables reports the keystrokes of both.
    assert_eq!(map.scheme_span(0, 8), Some((0, 2)));
    // A segment covering one reports one, whichever byte range it is given.
    assert_eq!(map.scheme_span(0, 5), Some((0, 1)));
    assert_eq!(map.scheme_span(5, 8), Some((1, 2)));
    assert_eq!(map.scheme_span(3, 7), Some((0, 2)));
}

#[test]
fn test_scheme_span_rejects_an_empty_or_out_of_range_span() {
    let map = SchemeMap::build("vsgo", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.scheme_span(4, 4), None);
    assert_eq!(map.scheme_span(4, 1), None);
    assert_eq!(map.scheme_span(0, 9), None, "past the end of the text");
    assert_eq!(map.full_span(2), None, "past the last syllable");

    // A keystroke stream that rewrites to nothing has no syllables to align.
    let empty = SchemeMap::build(";", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(empty.syllables(), 0);
    assert_eq!(empty.scheme_span(0, 1), None);
}

#[test]
fn test_is_rewrite_requested_needs_both_the_switch_and_a_scheme() {
    let switched = DecodeRequest::new("nihc")
        .with_scheme(SchemeId::XIAOHE)
        .with_flags(DecodeFlags::USER_DICT | DecodeFlags::SHUANGPIN);
    assert!(is_rewrite_requested(&switched));

    let unswitched = switched.clone().with_flags(DecodeFlags::USER_DICT);
    assert!(!is_rewrite_requested(&unswitched));

    let full_pinyin = switched.clone().with_scheme(SchemeId::FULL);
    assert!(!is_rewrite_requested(&full_pinyin));
}

#[test]
fn test_rewrite_request_borrows_when_no_scheme_is_wanted() {
    let request = DecodeRequest::new("nihao");
    let rewritten = rewrite_request(&request).expect("no rewrite can fail");
    assert!(matches!(rewritten, std::borrow::Cow::Borrowed("nihao")));
}

#[test]
fn test_rewrite_request_rewrites_when_the_switch_is_set() {
    let request = DecodeRequest::new("nihc")
        .with_scheme(SchemeId::XIAOHE)
        .with_flags(DecodeFlags::USER_DICT | DecodeFlags::SHUANGPIN);
    let rewritten = rewrite_request(&request).expect("xiaohe is implemented");
    assert_eq!(rewritten, "nihao");
}

#[test]
fn test_syllable_spans_tile_the_rewritten_text() {
    // The alignment the engine reads back has to be complete: every scheme syllable
    // owns a non-empty byte range, the ranges follow one another without a gap, and
    // together they cover the text exactly.
    for (name, scheme, _) in tables() {
        for raw in ["nihc", "vsgo", "bz", "ni'hc"] {
            let map = SchemeMap::build(raw, scheme)
                .expect("the schemes are implemented")
                .expect("a scheme rewrites");
            let mut expected = 0u16;
            for index in 0..map.syllables() {
                let (start, end) = map.full_span(index).expect("every syllable has a span");
                assert_eq!(
                    start, expected,
                    "{name}: {raw:?} syllable {index} is misplaced"
                );
                assert!(end > start, "{name}: {raw:?} syllable {index} is empty");
                expected = end;
            }
            let covered = usize::from(expected);
            assert_eq!(
                covered,
                map.text().len(),
                "{name}: {raw:?} spans miss the end"
            );
            assert_eq!(map.full_span(map.syllables()), None);
        }
    }
}

#[test]
fn test_xiaohe_reads_two_syllables_out_of_four_keys() {
    // The scheme the whole feature is documented with: `nihc` is `ni` then `hao`.
    let map = SchemeMap::build("nihc", SchemeId::XIAOHE)
        .expect("xiaohe is implemented")
        .expect("a scheme rewrites");
    assert_eq!(map.syllables(), 2);
    assert_eq!(map.unmapped_count(), 0);
    assert_eq!(map.text(), "nihao");
}

#[test]
fn test_scheme_and_full_pinyin_agree_on_candidates() {
    // The whole point of the scheme layer: it is a different encoding of the same
    // syllables, so the candidate list it produces has to be the one full pinyin
    // produces, text for text.
    let lexicon = MockLexicon::phrase();
    let lm = InMemoryLm::new();
    let decoder = Decoder::default();

    let full = decoder.decode(&DecodeRequest::new("zhongguo"), &lexicon, &NoUser, &lm);
    let scheme = SchemeId::XIAOHE;
    let rewritten = rewrite_for_scheme("vsgo", scheme).expect("xiaohe is implemented");
    let via_scheme = decoder.decode(
        &DecodeRequest::new(rewritten.as_ref()),
        &lexicon,
        &NoUser,
        &lm,
    );

    let full_texts: Vec<&str> = full
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    let scheme_texts: Vec<&str> = via_scheme
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(
        !full_texts.is_empty(),
        "the mock dictionary knows this word"
    );
    assert_eq!(scheme_texts, full_texts);
}
