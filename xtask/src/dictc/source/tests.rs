//! Tests for the raw-source layer.
//!
//! Every fixture is written to a scratch directory of its own under the system temp
//! directory and removed afterwards, so nothing here depends on the committed sources
//! under `data/raw/` or on the state another test left behind.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use ime_core::segment::lookup;
use ime_dict::format;

use super::reading::{fold_reading, parse_reading};
use super::words::{DEFAULT_WEIGHT_SINGLE, parse_flags};

use super::*;

/// A tiny L1 table covering the characters the tests use.
fn test_l1() -> BTreeMap<char, Vec<String>> {
    let rows = [
        ('中', vec!["zhong"]),
        ('国', vec!["guo"]),
        ('心', vec!["xin"]),
        ('行', vec!["xing", "hang"]),
        ('银', vec!["yin"]),
        ('好', vec!["hao"]),
        ('长', vec!["chang", "zhang"]),
    ];
    rows.into_iter()
        .map(|(character, readings)| {
            (
                character,
                readings
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<String>>(),
            )
        })
        .collect()
}

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

#[test]
fn test_fold_reading_strips_tones_digits_and_umlauts() {
    let cases = [
        ("nǐ", "ni"),
        ("hǎo", "hao"),
        ("ni3", "ni"),
        ("lǚ", "lü"),
        ("lv", "lü"),
        ("lüè", "lüe"),
        ("lve", "lüe"),
        ("jü", "ju"),
        ("zhōng", "zhong"),
        ("ê", "ê"),
    ];
    for (raw, expected) in cases {
        assert_eq!(
            fold_reading(raw).as_deref(),
            Some(expected),
            "folding {raw:?}"
        );
        assert!(
            lookup(expected).is_some(),
            "{expected:?} must be a syllable"
        );
    }
    assert_eq!(fold_reading(""), None);
    assert_eq!(fold_reading("你好"), None);
}

#[test]
fn test_parse_reading_requires_one_syllable_per_character() {
    let expected = Some(vec!["zhong".to_owned(), "guo".to_owned()]);
    assert_eq!(parse_reading("zhong'guo", 2), expected);
    assert_eq!(parse_reading("zhōngguó", 2), expected);
    assert_eq!(
        parse_reading("yinhang", 2),
        Some(vec!["yin".to_owned(), "hang".to_owned()])
    );
    assert_eq!(
        parse_reading("anan", 2),
        Some(vec!["an".to_owned(), "an".to_owned()]),
        "the longest syllable wins the tie"
    );
    // Longest syllable first, so `xian` splits as `xia|n` rather than `xi|an`. The
    // split is genuinely ambiguous without knowing the word, and no single rule
    // picks the linguistically likely reading for every case — `xi'an` and `xia'n`
    // are both real. Separated input is how a caller says which one it means, and
    // that path is covered by the `zhong'guo` case above; unseparated input falls
    // back to the documented longest-first rule.
    assert_eq!(
        parse_reading("xian", 2),
        Some(vec!["xia".to_owned(), "n".to_owned()])
    );
    assert_eq!(parse_reading("xian", 1), Some(vec!["xian".to_owned()]));
    // Three syllables is a legal split of `zhongguo`; the parser is not told which
    // characters the word has, so it cannot know the caller meant two.
    assert_eq!(
        parse_reading("zhongguo", 3),
        Some(vec!["zhong".to_owned(), "gu".to_owned(), "o".to_owned()])
    );
    assert_eq!(
        parse_reading("zhongguo", 9),
        None,
        "more syllables than letters"
    );
    assert_eq!(parse_reading("zzz", 1), None);
    assert_eq!(parse_reading("", 0), None);
}

#[test]
fn test_parse_flags_accepts_names_and_masks() {
    assert_eq!(parse_flags(""), Some(0));
    assert_eq!(parse_flags("3"), Some(3));
    assert_eq!(
        parse_flags("surname,place"),
        Some(format::FLAG_SURNAME | format::FLAG_PLACE)
    );
    assert_eq!(parse_flags("nonsense"), None);
    assert_eq!(parse_flags("200"), None, "an undefined flag bit is refused");
}

#[test]
fn test_load_words_reads_columns_weights_and_skips() {
    let dir = scratch("words");
    let path = dir.join("base.tsv");
    let text = "# comment\n\
                中国\tzhong'guo\t500\n\
                中国\t\t900\n\
                行\thang\n\
                好\n\
                心\n\
                银行\tyin'hang\t10\n\
                银行\tzzz\t10\n\
                hello\t\t10\n\
                这个字很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长\t\t1\n";
    fs::write(&path, text).expect("writing the fixture");

    let (words, skips) = load_words(&path, &test_l1(), 100).expect("parsing");
    let by_text: BTreeMap<&str, &Word> = words
        .iter()
        .map(|word| (word.text.as_str(), word))
        .collect();
    assert_eq!(words.len(), 5, "five distinct words survive");
    let zhong_guo = by_text.get("中国").expect("中国");
    assert_eq!(zhong_guo.weight, 900, "the heavier row wins");
    assert_eq!(zhong_guo.key, "zhong'guo");
    // Weight 900 clears the band threshold of 100, so the word carries every
    // character's readings for the multi-key expansion.
    assert_eq!(
        zhong_guo.readings,
        vec![vec!["zhong".to_owned()], vec!["guo".to_owned()]],
        "inside the band"
    );
    let xing = by_text.get("行").expect("行");
    assert_eq!(xing.key, "hang", "an explicit reading wins");
    assert_eq!(xing.readings.len(), 1, "inside the band");
    assert!(xing.polyphone);
    assert_eq!(by_text.get("好").expect("好").key, "hao");
    assert_eq!(by_text.get("好").expect("好").weight, DEFAULT_WEIGHT_SINGLE);
    assert_eq!(by_text.get("银行").expect("银行").key, "yin'hang");
    assert_eq!(skips.duplicate, 1);
    assert_eq!(skips.bad_reading, 1);
    assert_eq!(skips.non_han, 1);
    assert_eq!(skips.too_long, 1);
    assert_eq!(skips.bad_weight, 0);
    assert!(skips.total() >= 4);
    assert!(!skips.examples.is_empty());

    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_words_skips_rows_with_an_unknown_character() {
    let dir = scratch("unknown");
    let path = dir.join("base.tsv");
    fs::write(&path, "银龘\t\t10\n").expect("writing the fixture");
    let (words, skips) = load_words(&path, &test_l1(), 0).expect("parsing");
    assert!(
        words.is_empty(),
        "a word with an uncovered character is skipped"
    );
    assert_eq!(skips.unknown_char, 1);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_polyphone_refuses_an_illegal_row() {
    let dir = scratch("poly");
    let path = dir.join("polyphone.tsv");
    fs::write(&path, "银行\tyin'hang\n行长\tzhang'zhang\n").expect("writing the fixture");
    let rows = load_polyphone(&path).expect("legal rows parse");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("银行".to_owned(), "yin'hang".to_owned()));

    fs::write(&path, "银行\tyin'hang\n行长\tzhangzhang'x\n").expect("writing the fixture");
    let failure = load_polyphone(&path).expect_err("an illegal row must fail");
    assert!(
        failure.to_string().contains("polyphone.tsv:2"),
        "the error names the row: {failure}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_apply_polyphone_counts_rows_outside_the_subset() {
    let l1 = test_l1();
    let (words, _) = {
        let dir = scratch("apply");
        let path = dir.join("base.tsv");
        fs::write(&path, "中国\t\t100\n").expect("writing the fixture");
        let parsed = load_words(&path, &l1, 0).expect("parsing");
        fs::remove_dir_all(&dir).expect("cleaning up");
        parsed
    };
    let rows = vec![
        ("中国".to_owned(), "zhong'guo".to_owned()),
        ("银行".to_owned(), "yin'hang".to_owned()),
    ];
    let (corrections, unmatched) = apply_polyphone(&words, &rows);
    assert!(corrections.is_empty(), "a confirmation is not a correction");
    assert_eq!(unmatched, 1);
}

#[test]
fn test_synth_words_are_deterministic_and_keep_the_band_a_minority() {
    let l1 = wide_l1();
    let first = synth_words(400, &l1, 100);
    let second = synth_words(400, &l1, 100);
    assert_eq!(
        first.len(),
        400,
        "the generator produces the requested count"
    );
    for (left, right) in first.iter().zip(second.iter()) {
        assert_eq!(left.text, right.text);
        assert_eq!(left.weight, right.weight);
    }
    let in_band = first.iter().filter(|word| word.weight >= 100).count();
    assert!(
        in_band * 4 < first.len(),
        "the synthetic band stays a minority: {in_band} of {}",
        first.len()
    );
}

/// An L1 table covering the synthetic generator's character range.
///
/// Every third character carries a second reading, so the generator sees both
/// monophonic and polyphonic words.
fn wide_l1() -> BTreeMap<char, Vec<String>> {
    let syllables = [
        "zhong", "guo", "xin", "hang", "xing", "hao", "chang", "zhang",
    ];
    let mut table = BTreeMap::new();
    for offset in 0..3_000u32 {
        let Some(character) = char::from_u32(0x4E00 + offset) else {
            continue;
        };
        let index = offset as usize % syllables.len();
        let mut readings = vec![syllables[index].to_owned()];
        if offset % 3 == 0 {
            readings.push(syllables[(index + 3) % syllables.len()].to_owned());
        }
        table.insert(character, readings);
    }
    table
}
