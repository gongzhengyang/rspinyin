use super::*;
use std::path::PathBuf;

use ime_dict::format::SectionKind;
use ime_dict::format::reader::{Reader, Verify};
use ime_dict::format::writer::DictWriter;

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

/// Wraps one word for the compiler, with its readings ready for expansion.
fn word(text: &str, key: &str, weight: u32, l1: &BTreeMap<char, Vec<String>>) -> Word {
    let readings: Vec<Vec<String>> = text
        .chars()
        .map(|character| l1.get(&character).cloned().unwrap_or_default())
        .collect();
    let polyphone = readings.iter().any(|readings| readings.len() > 1);
    Word {
        text: text.to_owned(),
        key: key.to_owned(),
        readings,
        weight,
        flags: 0,
        polyphone,
    }
}

/// Encodes compiled sections into a container image.
fn image_of(compiled: &Compiled) -> Vec<u8> {
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, compiled.fst.clone())
        .expect("fst");
    writer
        .add_section(SectionKind::Entries, compiled.entries.clone())
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, compiled.strpool.clone())
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, compiled.unigram.clone())
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, compiled.wordlist.clone())
        .expect("wordlist");
    writer.encode().expect("encoding")
}

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

#[test]
fn test_compile_round_trips_through_the_reader() {
    let l1 = test_l1();
    let words = vec![
        word("中国", "zhong'guo", 5_000, &l1),
        word("银行", "yin'hang", 4_000, &l1),
        word("行长", "hang'zhang", 300, &l1),
    ];
    let band = Band {
        threshold: 1_000,
        requested: 50_000,
    };
    let compiled = compile(&words, band, 4, &[], NormalizeRules::default()).expect("compiling");
    assert_eq!(compiled.stats.words, 3);
    assert_eq!(compiled.stats.band_words, 2);
    assert_eq!(compiled.stats.polyphone_risk, 2);
    assert_eq!(compiled.stats.expanded_words, 1, "only 银行 has a choice");
    assert_eq!(compiled.stats.expanded_keys, 1);

    let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
    assert_eq!(reader.entry_count(), 3);
    assert_eq!(reader.key_words("zhong'guo").expect("lookup"), vec!["中国"]);
    assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
    assert_eq!(
        reader.key_words("yin'xing").expect("lookup"),
        vec!["银行"],
        "the second reading is reachable"
    );
    assert_eq!(
        reader.key_words("hang'zhang").expect("lookup"),
        vec!["行长"]
    );
    assert!(reader.unigram_lookup("银行").expect("unigram").is_some());
    // Every record carries the character count, which is not the byte length.
    for index in 0..reader.entry_count() {
        let entry = reader.entry(index).expect("the record");
        let text = reader.word(&entry).expect("the text");
        assert_eq!(
            usize::from(entry.char_count),
            text.chars().count(),
            "{text}"
        );
    }
}

#[test]
fn test_compile_is_deterministic() {
    let l1 = test_l1();
    let words = vec![
        word("中国", "zhong'guo", 5_000, &l1),
        word("银行", "yin'hang", 4_000, &l1),
        word("好心", "hao'xin", 200, &l1),
    ];
    let band = Band {
        threshold: 100,
        requested: 50_000,
    };
    let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
    let first = compile(&words, band, 4, &corrections, NormalizeRules::default()).expect("first");
    let second = compile(&words, band, 4, &corrections, NormalizeRules::default()).expect("second");
    assert_eq!(first.fst, second.fst, "the FST must be byte-identical");
    assert_eq!(first.entries, second.entries);
    assert_eq!(first.strpool, second.strpool);
    assert_eq!(first.unigram, second.unigram);
    assert_eq!(first.wordlist, second.wordlist);
    // The container is what a build ships and what a release records the digest of, so
    // the determinism claim is about its bytes rather than about the payloads alone:
    // the header, the section table and the alignment padding are part of what two
    // runs over the same inputs have to agree on.
    let image = image_of(&first);
    assert_eq!(
        image,
        image_of(&second),
        "two runs over one input must produce one container"
    );
    assert_eq!(&image[0..4], b"RSPD", "the image is a container");
}

#[test]
fn test_compile_applies_polyphone_corrections_without_duplicating_a_word() {
    let l1 = test_l1();
    let words = vec![word("银行", "yin'hang", 1_000, &l1)];
    let band = Band {
        threshold: 1_000,
        requested: 50_000,
    };
    let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
    let compiled =
        compile(&words, band, 4, &corrections, NormalizeRules::default()).expect("compiling");
    assert_eq!(compiled.stats.polyphone_keys, 1);
    let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
    assert_eq!(
        reader.key_words("yin'hang").expect("lookup"),
        vec!["银行"],
        "the corrected key holds the word exactly once"
    );
}

#[test]
fn test_compile_adds_a_corrected_key_outside_the_band() {
    let l1 = test_l1();
    let words = vec![word("银行", "yin'xing", 5, &l1)];
    let band = Band {
        threshold: 1_000,
        requested: 50_000,
    };
    let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
    let compiled =
        compile(&words, band, 4, &corrections, NormalizeRules::default()).expect("compiling");
    assert_eq!(
        compiled.stats.expanded_words, 0,
        "the word is outside the band"
    );
    let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
    assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
}

#[test]
fn test_compile_keeps_the_baseline_key_when_the_cap_drops_it() {
    let readings = vec![
        vec!["xing".to_owned(), "hang".to_owned()],
        vec!["ye".to_owned(), "xie".to_owned()],
    ];
    // The product is four combinations; a cap of two keeps the first two, so a
    // word whose stated reading is the third must still be reachable.
    let expanded = expand_keys(&readings, 2);
    assert_eq!(expanded, vec!["xing'ye", "xing'xie"]);
    let l1 = test_l1();
    let words = vec![word("行长", "hang'zhang", 5_000, &l1)];
    let band = Band {
        threshold: 1_000,
        requested: 50_000,
    };
    let compiled = compile(&words, band, 2, &[], NormalizeRules::default()).expect("compiling");
    let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
    assert_eq!(
        reader.key_words("hang'zhang").expect("lookup"),
        vec!["行长"],
        "the word keeps its own key"
    );
}

#[test]
fn test_expand_keys_is_capped_and_ordered_most_likely_first() {
    let readings = vec![
        vec!["xing".to_owned(), "hang".to_owned()],
        vec!["ye".to_owned()],
    ];
    assert_eq!(expand_keys(&readings, 4), vec!["xing'ye", "hang'ye"]);
    assert_eq!(expand_keys(&readings, 1), vec!["xing'ye"]);
    assert!(expand_keys(&readings, 0).is_empty());

    let both = vec![
        vec!["a".to_owned(), "b".to_owned()],
        vec!["c".to_owned(), "d".to_owned()],
    ];
    assert_eq!(expand_keys(&both, 4), vec!["a'c", "a'd", "b'c", "b'd"]);
    assert_eq!(expand_keys(&both, 2), vec!["a'c", "a'd"]);
}

/// The readings of the ten characters the expansion fixture gives two readings to.
///
/// Every one is a syllable of the real table, which the test asserts: an invented
/// reading would make the fixture measure an expansion no source could produce.
const POLYPHONE_READINGS: [(&str, &str); 10] = [
    ("xing", "hang"),
    ("chang", "zhang"),
    ("zhong", "chong"),
    ("huan", "hai"),
    ("dou", "du"),
    ("le", "yue"),
    ("jue", "jiao"),
    ("shuo", "shui"),
    ("shu", "cu"),
    ("qiang", "jiang"),
];

/// The reading of each of the ten characters that only ever start an unexpanded word.
const MONOPHONE_FIRST: [&str; 10] = [
    "tian", "guo", "xin", "hao", "yin", "shan", "feng", "yun", "mao", "lin",
];

/// The reading of each character that ends an expanded word.
const SECOND_IN_BAND: [&str; 2] = ["ban", "cai"];

/// The reading of each character that ends an unexpanded word.
const SECOND_OUT_OF_BAND: [&str; 8] = ["da", "er", "fa", "ge", "he", "ji", "ke", "la"];

/// Returns the `offset`-th character the expansion fixture uses.
///
/// The characters are drawn from the CJK unified block one per fixture slot, so every
/// character the fixture names is distinct and no two of its words can share one by
/// accident.
fn fixture_character(offset: usize) -> char {
    let codepoint = 0x4E00 + u32::try_from(offset).expect("the fixture is small");
    char::from_u32(codepoint).expect("a CJK code point")
}

/// The fixture the two expansion tests share.
///
/// A hundred two-character words with a hundred distinct baseline keys, of which the
/// twenty weighted into the band each carry a first character with two readings, so
/// the expansion has a second key to generate, and the eighty weighted below it carry
/// one reading each. That shape is what turns the key growth into a number rather than
/// an accident -- twenty extra keys over a hundred baseline keys is twenty percent --
/// and it is the shape the release build's word list has, in miniature.
///
/// Returns the words, and for each in-band word its text and both of its keys: the
/// baseline first, then the reading the expansion generates from it.
fn expansion_fixture() -> (Vec<Word>, Vec<(String, String, String)>) {
    let mut l1: BTreeMap<char, Vec<String>> = BTreeMap::new();
    let polyphone: Vec<char> = POLYPHONE_READINGS
        .iter()
        .enumerate()
        .map(|(index, (heavy, light))| {
            let character = fixture_character(index);
            l1.insert(character, vec![heavy.to_string(), light.to_string()]);
            character
        })
        .collect();
    let monophone: Vec<char> = MONOPHONE_FIRST
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            let character = fixture_character(POLYPHONE_READINGS.len() + index);
            l1.insert(character, vec![reading.to_string()]);
            character
        })
        .collect();
    let in_band_tail: Vec<char> = SECOND_IN_BAND
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            let offset = POLYPHONE_READINGS.len() + MONOPHONE_FIRST.len() + index;
            let character = fixture_character(offset);
            l1.insert(character, vec![reading.to_string()]);
            character
        })
        .collect();
    let out_of_band_tail: Vec<char> = SECOND_OUT_OF_BAND
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            let offset =
                POLYPHONE_READINGS.len() + MONOPHONE_FIRST.len() + SECOND_IN_BAND.len() + index;
            let character = fixture_character(offset);
            l1.insert(character, vec![reading.to_string()]);
            character
        })
        .collect();

    let mut words = Vec::new();
    let mut in_band = Vec::new();
    for (index, first) in polyphone.iter().enumerate() {
        let (heavy, light) = POLYPHONE_READINGS[index];
        for (slot, second) in in_band_tail.iter().enumerate() {
            let text = format!("{first}{second}");
            let key = format!("{heavy}'{}", SECOND_IN_BAND[slot]);
            let alternate = format!("{light}'{}", SECOND_IN_BAND[slot]);
            words.push(word(&text, &key, 5_000, &l1));
            in_band.push((text, key, alternate));
        }
    }
    for (index, first) in monophone.iter().enumerate() {
        for (slot, second) in out_of_band_tail.iter().enumerate() {
            let text = format!("{first}{second}");
            let key = format!("{}'{}", MONOPHONE_FIRST[index], SECOND_OUT_OF_BAND[slot]);
            words.push(word(&text, &key, 100, &l1));
        }
    }
    (words, in_band)
}

/// The band the expansion fixture is measured against: the twenty words weighted 5000
/// are inside it, the eighty weighted 100 are not.
fn expansion_band() -> Band {
    Band {
        threshold: 1_000,
        requested: 50_000,
    }
}

#[test]
fn test_compile_expands_every_polyphone_word_of_the_band() {
    let (words, in_band) = expansion_fixture();
    assert_eq!(words.len(), 100);
    assert_eq!(in_band.len(), 20);
    for (text, key, alternate) in &in_band {
        assert_ne!(key, alternate, "{text} must have a second reading");
        for syllable in key.split('\'').chain(alternate.split('\'')) {
            assert!(
                ime_core::segment::lookup(syllable).is_some(),
                "{syllable} is not a syllable of the table"
            );
        }
    }

    let compiled =
        compile(&words, expansion_band(), 4, &[], NormalizeRules::default()).expect("compiling");
    assert_eq!(compiled.stats.words, 100);
    // The twenty heavy words are the band, every one of them carries a character with a
    // second reading, and every one of them therefore generates a second key.
    assert_eq!(compiled.stats.band_words, 20);
    assert_eq!(compiled.stats.polyphone_risk, 20);
    assert_eq!(compiled.stats.expanded_words, 20);
    assert_eq!(compiled.stats.expanded_keys, 20);

    let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
    for (text, key, alternate) in &in_band {
        for candidate in [key.as_str(), alternate.as_str()] {
            let served = reader.key_words(candidate).expect("lookup");
            assert_eq!(served, vec![text.as_str()], "{candidate} serves {text}");
        }
    }
}

#[test]
fn test_compile_keeps_the_key_growth_inside_the_target_band() {
    let (words, _) = expansion_fixture();
    let compiled =
        compile(&words, expansion_band(), 4, &[], NormalizeRules::default()).expect("compiling");
    let baseline = compiled.stats.baseline_keys();
    assert_eq!(baseline, 100, "one baseline key per word");
    // Twenty words gained one key each, over a hundred baseline keys.
    assert_eq!(compiled.stats.keys, 120);
    // The band is the one ADR-0000 measured the expansion at: a growth below it means
    // the expansion is not reaching the words people type, and the ceiling is what
    // `--max-key-growth` refuses a build for.
    let growth = crate::dictc::growth_percent(compiled.stats.keys, baseline);
    assert!(
        (15.0..=30.0).contains(&growth),
        "the expansion grew the key count by {growth:.1}%, outside the 15%..30% band"
    );
}

#[test]
fn test_prob_q12_scales_to_the_heaviest_word() {
    assert_eq!(prob_q12(0, 0), 1);
    assert_eq!(prob_q12(1_000, 1_000), PROB_Q12_MAX);
    assert_eq!(prob_q12(500, 1_000), PROB_Q12_MAX / 2);
    assert_eq!(prob_q12(0, 1_000), 1, "a zero weight still scores");
}

#[test]
fn test_band_threshold_reads_the_frequency_ranking() {
    let dir = scratch("band");
    let path = dir.join("jieba-dict.tsv");
    fs::write(&path, "中\t500\n国\t300\n心\t100\n好\t40\n的\t10\n").expect("writing");

    let band = band_threshold(&path, 2, None).expect("band");
    assert_eq!(
        band.threshold, 300,
        "the second entry's weight is the floor"
    );
    let band = band_threshold(&path, 0, None).expect("band");
    assert_eq!(
        band.threshold,
        u32::MAX,
        "no band when nothing is requested"
    );
    let band = band_threshold(&path, 50_000, None).expect("band");
    assert_eq!(
        band.threshold, 10,
        "a rank past the end clamps to the last entry"
    );
    let band = band_threshold(&path, 50_000, Some(7)).expect("band");
    assert_eq!(band.threshold, 7, "an explicit floor wins");
    let band = band_threshold(&dir.join("missing.tsv"), 50_000, None).expect("band");
    assert_eq!(band.threshold, FALLBACK_BAND_THRESHOLD);

    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_merge_two_char_layer_overrides_fills_and_skips_other_lengths() {
    let l1 = test_l1();
    let mut jieba = vec![
        word("中国", "zhong'guo", 5_000, &l1),
        word("银行", "yin'hang", 4_000, &l1),
        word("好心", "hao'xin", 200, &l1),
    ];
    let huayu = vec![
        // Both sources hold it: the huayu weight and reading win.
        word("银行", "yin'xing", 9_000, &l1),
        word("中国", "zhong'guo", 8_000, &l1),
        // Only huayu holds it: the gap is filled.
        word("大国", "da'guo", 700, &l1),
        // Other lengths pass through untouched.
        word("好", "hao", 50, &l1),
        word("阿尔巴", "a'er'ba", 900, &l1),
    ];
    let stats = merge_two_char_layer(&mut jieba, huayu, 0);
    assert_eq!(stats.overridden, 2);
    assert_eq!(stats.added, 1);
    assert_eq!(jieba.len(), 4, "three originals plus one added word");
    // Shared words keep the jieba weight -- the corpus ranking is the backbone
    // -- and take the huayu reading, which the automatic polyphone check then
    // judges.
    let bank = jieba.iter().find(|w| w.text == "银行").expect("银行");
    assert_eq!(bank.weight, 4_000, "the jieba weight is the backbone");
    assert_eq!(
        bank.key, "yin'xing",
        "the huayu reading travels with the word"
    );
    let china = jieba.iter().find(|w| w.text == "中国").expect("中国");
    assert_eq!(china.weight, 5_000);
    // A word only huayu holds joins at the jieba weight of its rank in the
    // admitted layer (rank 2 of three): jieba's own third weight.
    let added = jieba.iter().find(|w| w.text == "大国").expect("大国");
    assert_eq!(added.weight, 200);
    let heart = jieba.iter().find(|w| w.text == "好心").expect("好心");
    assert_eq!(heart.weight, 200, "a word only jieba holds is untouched");
}

#[test]
fn test_merge_two_char_layer_floor_and_order_free() {
    let l1 = test_l1();
    let make_huayu = || -> Vec<Vec<Word>> {
        // The same entries in two different orders: the merge must not tell
        // them apart.
        vec![
            vec![
                word("银行", "yin'xing", 9_000, &l1),
                word("小国", "xiao'guo", 50, &l1),
                word("大国", "da'guo", 700, &l1),
            ],
            vec![
                word("大国", "da'guo", 700, &l1),
                word("小国", "xiao'guo", 50, &l1),
                word("银行", "yin'xing", 9_000, &l1),
            ],
        ]
    };
    let mut first = vec![word("银行", "yin'hang", 4_000, &l1)];
    let mut second = vec![word("银行", "yin'hang", 4_000, &l1)];
    let mut orders = make_huayu();
    let second_order = orders.pop().expect("the second row order");
    let first_order = orders.pop().expect("the first row order");
    let a = merge_two_char_layer(&mut first, first_order, 100);
    let b = merge_two_char_layer(&mut second, second_order, 100);
    assert_eq!(a, b, "the stats do not depend on the huayu row order");
    let key = |words: &[Word]| -> Vec<(String, u32, String)> {
        let mut rows: Vec<_> = words
            .iter()
            .map(|w| (w.text.clone(), w.weight, w.key.clone()))
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(key(&first), key(&second), "the merged lists agree");
    // The floor keeps the 50-weight entry out entirely.
    assert!(first.iter().all(|w| w.text != "小国"));
    assert_eq!(first.len(), 2, "jieba's 银行 slot plus the added 大国");
}

#[test]
fn test_polyphone_rule_keeps_the_readings_usage_supports() {
    let l1 = test_l1();
    // 行 reads xing 1000 weighted and hang 300 weighted; 长 reads chang 1000
    // and zhang 300. Both divergent readings clear a 5% floor.
    let mut words = vec![
        word("行人", "xing'ren", 1_000, &l1),
        word("行长", "hang'zhang", 300, &l1),
        // Divergent on both characters, each above the floor.
        word("银行", "yin'hang", 300, &l1),
    ];
    let counts = apply_polyphone_rule(
        &mut words,
        &l1,
        &std::collections::BTreeSet::new(),
        PolyphoneRule { freq_ratio_pct: 5 },
    );
    assert_eq!(
        counts,
        PolyphoneAutoCounts {
            assigned: 2,
            flattened: 0,
            manual: 0,
            total: 2
        }
    );
    let bank = words.iter().find(|w| w.text == "银行").expect("银行");
    assert_eq!(bank.key, "yin'hang", "a supported reading is kept");
}

#[test]
fn test_polyphone_rule_flattens_below_the_floor_and_exempts_manual() {
    let l1 = test_l1();
    // The corpus votes: 行 reads xing 1000 weighted and hang 801; 长 reads
    // chang 1000 and zhang once. 银行's hang clears the 5% line, 好长's zhang
    // does not, and 心行 states the same kind of divergence but the L3c table
    // owns it.
    let mut words = vec![
        word("行", "xing", 1_000, &l1),
        word("长", "chang", 1_000, &l1),
        word("银行", "yin'hang", 300, &l1),
        word("心行", "xin'hang", 500, &l1),
        word("好长", "hao'zhang", 1, &l1),
    ];
    let manual = std::collections::BTreeSet::from([String::from("心行")]);
    let counts = apply_polyphone_rule(
        &mut words,
        &l1,
        &manual,
        PolyphoneRule { freq_ratio_pct: 5 },
    );
    assert_eq!(counts.total, 3, "every divergent word is examined");
    assert_eq!(counts.assigned, 1, "hang carries 300 of xing's 1000");
    assert_eq!(counts.flattened, 1, "zhang carries 1 of chang's 1000");
    assert_eq!(counts.manual, 1);
    let pair = words.iter().find(|w| w.text == "好长").expect("好长");
    assert_eq!(pair.key, "hao'chang", "flattened onto the top reading");
    let owned = words.iter().find(|w| w.text == "心行").expect("心行");
    assert_eq!(owned.key, "xin'hang", "a manual row is left to the table");
}

#[test]
fn test_polyphone_rule_zero_top_votes_leaves_the_reading_standing() {
    let l1 = test_l1();
    // Every word of the list states hang for 行: the top reading's vote count is
    // zero, so there is no majority to lose to and the reading stands.
    let mut words = vec![word("银行", "yin'hang", 4_000, &l1)];
    let counts = apply_polyphone_rule(
        &mut words,
        &l1,
        &std::collections::BTreeSet::new(),
        PolyphoneRule { freq_ratio_pct: 5 },
    );
    assert_eq!(counts.total, 1);
    assert_eq!(counts.assigned, 1);
    assert_eq!(words[0].key, "yin'hang");
}
