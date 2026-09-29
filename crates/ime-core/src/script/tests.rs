//! Unit tests for the script conversion layer.
//!
//! They live beside the module rather than inside it because the suite is longer
//! than the code it drives; see `script.rs` for the responsibility and the
//! boundaries of what is under test.
//!
//! Every test here runs against [`VariantTable`], an in-memory table: no dictionary
//! file, no clock and no environment is read, so the suite is deterministic by
//! construction.

use ime_types::ui::Script;

use super::*;

/// A table of one pair, for a test that is about the algorithm and not the data.
fn pair(simplified: &str, traditional: &str) -> VariantTable {
    VariantTable::from_pairs(&[(simplified, traditional)])
}

#[test]
fn test_to_traditional_disambiguates_fa_by_word() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("头发", &table), "頭髮");
    assert_eq!(to_traditional("出发", &table), "出發");
    assert_eq!(to_traditional("发现", &table), "發現");
    // The bare character has two traditional forms, so the table refuses to guess.
    assert_eq!(to_traditional("发", &table), "发");
}

#[test]
fn test_to_traditional_disambiguates_gan_by_word() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("干净", &table), "乾淨");
    assert_eq!(to_traditional("干活", &table), "幹活");
    assert_eq!(to_traditional("干", &table), "干");
    // This word is spelled the same in both scripts, and the entry records that.
    assert_eq!(to_traditional("干涉", &table), "干涉");
}

#[test]
fn test_to_traditional_disambiguates_hou_by_word() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("后面", &table), "後面");
    assert_eq!(to_traditional("皇后", &table), "皇后");
    assert_eq!(to_traditional("后", &table), "后");
}

#[test]
fn test_to_traditional_disambiguates_li_by_word() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("里面", &table), "裡面");
    assert_eq!(to_traditional("公里", &table), "公里");
    assert_eq!(to_traditional("里", &table), "里");
}

#[test]
fn test_to_traditional_disambiguates_tai_by_word() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("台湾", &table), "臺灣");
    assert_eq!(to_traditional("台风", &table), "颱風");
    assert_eq!(to_traditional("写字台", &table), "寫字檯");
    assert_eq!(to_traditional("台", &table), "台");
}

#[test]
fn test_to_traditional_is_idempotent() {
    let table = VariantTable::from_seed();
    let cases = [
        "头发",
        "干活",
        "皇后",
        "台湾",
        "银行",
        "去银行 (ICBC) 取钱",
        "简体 mixed with 繁體",
        "",
    ];
    for text in cases {
        let once = to_traditional(text, &table);
        assert_eq!(
            to_traditional(&once, &table),
            once,
            "{text:?} converts to itself the second time"
        );
    }
}

#[test]
fn test_to_traditional_prefers_the_longest_entry_that_matches() {
    let table = VariantTable::from_pairs(&[("台", "臺"), ("台湾", "臺灣")]);
    assert_eq!(to_traditional("台湾", &table), "臺灣");
    assert_eq!(to_traditional("台", &table), "臺");
    // A shorter entry still applies where the longer one does not.
    assert_eq!(to_traditional("台风", &table), "臺风");
}

#[test]
fn test_to_traditional_stops_at_the_match_window() {
    // The window is eight characters, so a longer word is never looked up whole.
    let nine = "一二三四五六七八九";
    let table = pair(nine, "九十");
    assert_eq!(to_traditional(nine, &table), nine);

    let eight = "一二三四五六七八";
    let table = pair(eight, "八十");
    assert_eq!(to_traditional(eight, &table), "八十");
}

#[test]
fn test_to_traditional_matches_a_full_window_word_inside_a_longer_text() {
    // The window is measured from each position rather than over the whole input, so a word
    // of exactly `MAX_MATCH_CHARS` characters is found in the middle of a longer commit and
    // everything around it is copied through.
    let word = "一二三四五六七八";
    let table = pair(word, "八十");
    let text = format!("前{word}后");
    assert_eq!(to_traditional(&text, &table), "前八十后");
}

#[test]
fn test_to_traditional_does_not_match_a_word_truncated_at_the_end_of_the_text() {
    // The window never runs past the end of the input: a text that ends in the first
    // characters of a word is not that word, and a character in front of it does not move
    // the match to a position where it would fit either.
    let table = pair("一二三四五六七八", "八十");
    let truncated = "一二三四五六七";
    assert_eq!(to_traditional(truncated, &table), truncated);
    assert_eq!(
        to_traditional("九一二三四五六七", &table),
        "九一二三四五六七"
    );
    // The whole word, ending exactly where the text does, is still matched.
    assert_eq!(to_traditional("一二三四五六七八", &table), "八十");
}

#[test]
fn test_to_traditional_windows_by_character_and_not_by_byte() {
    // The window counts characters, so a word that starts with a four-byte character is
    // found; an implementation that measured the window in bytes would miss it or cut a
    // character in half.
    let table = pair("🙂干净", "🙂乾淨");
    assert_eq!(to_traditional("🙂干净", &table), "🙂乾淨");
    // Four characters and eleven bytes: the word still fits the eight-character window
    // even though its byte length does not.
    assert_eq!(to_traditional("🙂🙂干净", &table), "🙂🙂乾淨");
}

#[test]
fn test_to_traditional_leaves_the_unmapped_runs_alone() {
    let table = VariantTable::from_seed();
    // The mapped word changes and every other character is copied through.
    let text = "去银行 (ICBC) 取钱";
    let expected = "去銀行 (ICBC) 取钱";
    assert_eq!(to_traditional(text, &table), expected);
    // A match is found wherever it starts, not only at the beginning.
    assert_eq!(to_traditional("的头发", &table), "的頭髮");
}

#[test]
fn test_to_traditional_leaves_ascii_and_punctuation_alone() {
    let table = VariantTable::from_seed();
    let text = "hello, world! 123 -- 🙂";
    assert_eq!(to_traditional(text, &table), text);
}

#[test]
fn test_to_traditional_maps_a_single_unambiguous_character() {
    let table = VariantTable::from_seed();
    // One character is the whole input rather than a prefix of a longer word, so the window
    // shrinks to one and the single-character entry is what answers.
    assert_eq!(to_traditional("银", &table), "銀");
    assert_eq!(to_traditional("国", &table), "國");
    assert_eq!(to_simplified("銀", &table), "银");
}

#[test]
fn test_to_traditional_copies_a_single_unmapped_character_through() {
    let table = VariantTable::from_seed();
    // A single character the table holds nothing for is neither a failure nor a deletion,
    // whatever its width: a Han character, an ASCII letter and a four-byte emoji all come
    // back as they went in.
    assert_eq!(to_traditional("文", &table), "文");
    assert_eq!(to_traditional("a", &table), "a");
    assert_eq!(to_traditional("🙂", &table), "🙂");
    assert_eq!(to_simplified("文", &table), "文");
}

#[test]
fn test_to_traditional_of_text_the_table_holds_nothing_for_is_unchanged() {
    // The whole input is outside the table, so nothing matches at any position and every
    // character is copied through -- which is what makes a conversion on a mixed sentence
    // safe rather than a rewrite of the parts it recognizes.
    let table = pair("干净", "乾淨");
    let text = "mixed 火星文 mixed";
    assert_eq!(to_traditional(text, &table), text);
    assert_eq!(to_simplified(text, &table), text);
}

#[test]
fn test_to_traditional_ignores_an_empty_replacement() {
    // A source that answers every question with the empty string.
    struct Empty;

    impl VariantSource for Empty {
        fn lookup(&self, _word: &str, _target: Script) -> Option<&str> {
            Some("")
        }
    }

    // An empty answer is not a licence to delete: the character is copied through.
    let text = "干净";
    assert_eq!(to_traditional(text, &Empty), text);
    assert_eq!(to_simplified(text, &Empty), text);
}

#[test]
fn test_to_traditional_of_empty_input_is_empty() {
    let table = VariantTable::from_seed();
    assert_eq!(to_traditional("", &table), "");
    assert_eq!(to_simplified("", &table), "");
}

#[test]
fn test_to_traditional_with_no_table_returns_the_input_unchanged() {
    // What a missing `script.dict` degrades to: the feature answers nothing, and
    // every string converts to itself rather than failing.
    let missing = VariantTable::default();
    let simplified = "头发干净台湾银行";
    let traditional = "頭髮乾淨臺灣銀行";
    assert_eq!(to_traditional(simplified, &missing), simplified);
    assert_eq!(to_simplified(traditional, &missing), traditional);
}

#[test]
fn test_to_traditional_depends_only_on_the_text_and_the_table() {
    let first = pair("台", "臺");
    let second = pair("台", "檯");
    assert_eq!(to_traditional("台", &first), "臺");
    assert_eq!(to_traditional("台", &second), "檯");
    // The same text and the same table answer the same way every time.
    assert_eq!(to_traditional("台", &first), "臺");
}

#[test]
fn test_to_simplified_reverses_the_seed_table() {
    let table = VariantTable::from_seed();
    assert_eq!(to_simplified("頭髮", &table), "头发");
    assert_eq!(to_simplified("乾淨", &table), "干净");
    assert_eq!(to_simplified("臺灣", &table), "台湾");
    assert_eq!(to_simplified("銀行", &table), "银行");
    assert_eq!(to_simplified("頭髮乾淨", &table), "头发干净");
}

#[test]
fn test_to_simplified_of_a_word_the_table_does_not_hold_is_unchanged() {
    let table = VariantTable::from_seed();
    assert_eq!(to_simplified("银行", &table), "银行");
    assert_eq!(to_simplified("头发", &table), "头发");
    assert_eq!(to_simplified("ICBC", &table), "ICBC");
}

#[test]
fn test_convert_answers_both_directions_from_one_table() {
    let table = VariantTable::from_seed();
    assert_eq!(convert("干净", &table, Script::Traditional), "乾淨");
    assert_eq!(convert("乾淨", &table, Script::Simplified), "干净");
    // A word already in the target script is left alone.
    assert_eq!(convert("干净", &table, Script::Simplified), "干净");
}

#[test]
fn test_convert_with_offsets_reports_where_each_boundary_landed() {
    let table = VariantTable::from_seed();
    let converted = convert_with_offsets("去银行", &table, Script::Traditional);
    assert_eq!(converted.text(), "去銀行");
    assert_eq!(converted.map_offset(0), Some(0));
    assert_eq!(converted.map_offset(3), Some(3));
    assert_eq!(converted.map_offset(6), Some(6));
    assert_eq!(converted.map_offset(9), Some(9));
}

#[test]
fn test_convert_with_offsets_shifts_every_boundary_after_a_longer_word() {
    // The case the design calls out as the easy one to get wrong: a word that grows
    // moves every offset behind it, so a caret cannot be reused unchanged.
    let table = pair("甲", "甲乙");
    let converted = convert_with_offsets("文甲文", &table, Script::Traditional);
    assert_eq!(converted.text(), "文甲乙文");
    assert_eq!(converted.map_offset(0), Some(0));
    // The character in front did not move, and the one behind it did.
    assert_eq!(converted.map_offset(3), Some(3));
    assert_eq!(converted.map_offset(6), Some(9));
    assert_eq!(converted.map_offset(9), Some(12));
}

#[test]
fn test_map_offset_rejects_an_offset_inside_a_character() {
    let table = pair("甲", "甲乙");
    let converted = convert_with_offsets("甲", &table, Script::Traditional);
    assert_eq!(converted.text(), "甲乙");
    assert_eq!(converted.map_offset(0), Some(0));
    assert_eq!(converted.map_offset(1), None);
    assert_eq!(converted.map_offset(2), None);
}

#[test]
fn test_map_offset_rejects_an_offset_inside_a_replaced_word() {
    // The rewrite replaces a word as one opaque string, so a position inside the
    // word it consumed has nothing to point at -- not even when the two spellings
    // happen to be the same length, which is the case here.
    let table = VariantTable::from_seed();
    let converted = convert_with_offsets("干净", &table, Script::Traditional);
    assert_eq!(converted.text(), "乾淨");
    assert_eq!(converted.map_offset(0), Some(0));
    assert_eq!(converted.map_offset(3), None);
    // The end of the word is a boundary again.
    assert_eq!(converted.map_offset(6), Some(6));
}

#[test]
fn test_map_offset_rejects_an_offset_past_the_end() {
    let table = VariantTable::from_seed();
    let converted = convert_with_offsets("去银行", &table, Script::Traditional);
    assert_eq!(converted.map_offset(10), None);
    assert_eq!(converted.map_offset(usize::MAX), None);
}

#[test]
fn test_map_offset_of_empty_input_has_only_the_end() {
    let table = VariantTable::from_seed();
    let converted = convert_with_offsets("", &table, Script::Traditional);
    assert_eq!(converted.text(), "");
    // The end of an empty input is its start.
    assert_eq!(converted.map_offset(0), Some(0));
    assert_eq!(converted.map_offset(1), None);
}

#[test]
fn test_converted_text_into_text_returns_what_text_borrowed() {
    let table = VariantTable::from_seed();
    let converted = convert_with_offsets("头发", &table, Script::Traditional);
    let borrowed = String::from(converted.text());
    assert_eq!(converted.into_text(), borrowed);
    assert_eq!(borrowed, "頭髮");
}

#[test]
fn test_variant_table_keeps_the_first_pair_for_a_repeated_key() {
    let table = VariantTable::from_pairs(&[("台", "臺"), ("台", "檯")]);
    assert_eq!(table.lookup("台", Script::Traditional), Some("臺"));
}

#[test]
fn test_variant_table_drops_a_pair_with_an_empty_key() {
    let table = VariantTable::from_pairs(&[("", "臺"), ("台", "臺")]);
    assert_eq!(table.lookup("", Script::Traditional), None);
    assert_eq!(table.lookup("台", Script::Traditional), Some("臺"));
    assert_eq!(table, VariantTable::from_pairs(&[("台", "臺")]));
}

#[test]
fn test_variant_table_drops_a_pair_with_an_empty_value() {
    // A pair that would replace a word with nothing is dropped: the rewrite is
    // lossless, so a word the table cannot spell is left exactly as it is.
    let table = VariantTable::from_pairs(&[("台", ""), ("台湾", "臺灣")]);
    assert_eq!(table.lookup("台", Script::Traditional), None);
    assert_eq!(to_traditional("台", &table), "台");
    assert_eq!(to_traditional("台湾", &table), "臺灣");
}

#[test]
fn test_variant_table_lookup_returns_none_for_an_unknown_word() {
    let table = pair("干净", "乾淨");
    assert_eq!(table.lookup("干净", Script::Traditional), Some("乾淨"));
    assert_eq!(table.lookup("乾淨", Script::Simplified), Some("干净"));
    assert_eq!(table.lookup("干", Script::Traditional), None);
    assert_eq!(table.lookup("", Script::Traditional), None);
    // The pair is one-way: its simplified side is not a key of the other index.
    assert_eq!(table.lookup("干净", Script::Simplified), None);
}

#[test]
fn test_variant_table_default_answers_nothing() {
    let empty = VariantTable::default();
    assert_eq!(empty.lookup("干净", Script::Traditional), None);
    assert_eq!(empty.lookup("乾淨", Script::Simplified), None);
    assert_eq!(to_traditional("干净", &empty), "干净");
    assert_eq!(to_simplified("乾淨", &empty), "乾淨");
}

#[test]
fn test_seed_table_is_free_of_duplicate_keys() {
    for (index, entry) in SEED.iter().enumerate() {
        for other in &SEED[index + 1..] {
            assert_ne!(
                entry.simplified, other.simplified,
                "two pairs share a simplified key, so one of them is unreachable"
            );
        }
    }
}

#[test]
fn test_seed_table_holds_a_word_entry_for_every_ambiguous_group() {
    let table = VariantTable::from_seed();
    // One word per one-to-many character: 发, 干, 后, 里, 台.
    for word in ["头发", "干净", "后面", "里面", "台湾"] {
        assert!(
            table.lookup(word, Script::Traditional).is_some(),
            "{word} disambiguates a one-to-many character"
        );
    }
}

#[test]
fn test_seed_table_round_trips_every_entry() {
    let table = VariantTable::from_seed();
    for entry in SEED {
        assert_eq!(
            to_traditional(entry.simplified, &table),
            entry.traditional,
            "{} converts to its stored traditional spelling",
            entry.simplified
        );
        assert_eq!(
            to_simplified(entry.traditional, &table),
            entry.simplified,
            "{} converts back",
            entry.simplified
        );
    }
}

#[test]
fn test_seed_table_values_are_never_converted_again() {
    // The property the seed needs for the rewrite to be idempotent: no traditional
    // form contains a simplified key the rewrite would match. Asserted over the
    // whole table because adding an entry is exactly what could break it.
    let table = VariantTable::from_seed();
    for entry in SEED {
        assert_eq!(
            to_traditional(entry.traditional, &table),
            entry.traditional,
            "{} is already traditional",
            entry.traditional
        );
        assert_eq!(
            to_simplified(entry.simplified, &table),
            entry.simplified,
            "{} is already simplified",
            entry.simplified
        );
    }
}
