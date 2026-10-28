//! Integration tests for the data-defined double-pinyin layouts.
//!
//! Three jobs, in the order the feature card asks for them:
//!
//! 1. **Migration equivalence.** The five published layouts used to be hard-coded
//!    `const` tables. The tables the compiled data produces must behave
//!    byte-identically to those, so the pre-migration tables are pinned here as
//!    golden data and the whole key space of every layout is enumerated through
//!    both.
//! 2. **Coverage.** Every built-in layout must be able to spell every syllable of
//!    the language's syllable table (minus the documented exceptions), and the
//!    published vowel-initial keystrokes of each layout must reach their
//!    syllables.
//! 3. **The validator.** A wrong layout must refuse to load, one `config/invalid`
//!    diagnostic per error, never quoting a table entry's value; a good user
//!    document must compile to the same behaviour as the built-in it mirrors.

use std::collections::BTreeMap;

use ime_core::segment::syllable::{SYLLABLES, lookup, syllable_at};
use ime_core::shuangpin::layout::{
    LAYOUT_DIAGNOSTIC_KEY, LayoutDocument, LayoutTables, compile_layout, parse_layout,
};
use ime_core::shuangpin::{
    SCHEME_KEYS, SchemeTable, key_index, map_syllable, microsoft, sogou, table_for, tables,
    xiaohe, ziguang, ziranma,
};
use ime_types::{ImeError, SchemeId, SyllableId};

/// The Xiaohe layout document, the fixture a user layout is modelled on.
const XIAOHE_DOCUMENT: &str = include_str!("../src/shuangpin/data/xiaohe.toml");

/// The pre-migration Xiaohe table: the migration's golden data.
const GOLDEN_XIAOHE: Golden = Golden {
    id: SchemeId::XIAOHE,
    name: "xiaohe",
    initials: GOLDEN_SELF_INITIALS,
    finals: [
        &["a"],
        &["in"],
        &["ao"],
        &["ai"],
        &["e"],
        &["en"],
        &["eng", "ng"],
        &["ang"],
        &["i"],
        &["an"],
        &["uai", "ing"],
        &["iang", "uang"],
        &["ian"],
        &["iao"],
        &["uo", "o"],
        &["ie"],
        &["iu"],
        &["uan", "er"],
        &["ong", "iong"],
        &["üe", "ue"],
        &["u"],
        &["ü", "ui"],
        &["ei"],
        &["ia", "ua"],
        &["un"],
        &["ou"],
        &[],
    ],
    standalone: GOLDEN_VOWEL_STANDALONE,
};

/// The pre-migration Ziranma table: the migration's golden data.
const GOLDEN_ZIRANMA: Golden = Golden {
    id: SchemeId::ZIRANMA,
    name: "ziranma",
    initials: GOLDEN_SELF_INITIALS,
    finals: [
        &["a"],
        &["ou"],
        &["iao"],
        &["uang", "iang"],
        &["e"],
        &["en"],
        &["eng", "ng"],
        &["ang"],
        &["i"],
        &["an"],
        &["ao"],
        &["ai"],
        &["ian"],
        &["in"],
        &["uo", "o"],
        &["un"],
        &["iu"],
        &["uan", "er"],
        &["ong", "iong"],
        &["üe", "ue"],
        &["u"],
        &["ü", "ui"],
        &["ia", "ua"],
        &["ie"],
        &["uai", "ing"],
        &["ei"],
        &[],
    ],
    standalone: GOLDEN_VOWEL_STANDALONE,
};

/// The pre-migration Microsoft table: the migration's golden data.
const GOLDEN_MICROSOFT: Golden = Golden {
    id: SchemeId::MICROSOFT,
    name: "microsoft",
    initials: GOLDEN_OLED_INITIALS,
    finals: [
        &["a"],
        &["ou"],
        &["iao"],
        &["uang", "iang"],
        &["e"],
        &["en"],
        &["eng", "ng"],
        &["ang"],
        &["i"],
        &["an"],
        &["ao"],
        &["ai"],
        &["ian"],
        &["in"],
        &["uo", "o"],
        &["un"],
        &["iu"],
        &["uan", "er"],
        &["ong", "iong"],
        &["üe", "ue"],
        &["u"],
        &["ui", "üe", "ue"],
        &["ia", "ua"],
        &["ie"],
        &["uai", "ü"],
        &["ei"],
        &["ing"],
    ],
    standalone: GOLDEN_VOWEL_STANDALONE,
};

/// The pre-migration Sogou table: the migration's golden data.
const GOLDEN_SOGOU: Golden = Golden {
    id: SchemeId::SOGOU,
    name: "sogou",
    initials: GOLDEN_OLED_INITIALS,
    finals: [
        &["a"],
        &["ou"],
        &["iao"],
        &["uang", "iang"],
        &["e"],
        &["en"],
        &["eng", "ng"],
        &["ang"],
        &["i"],
        &["an"],
        &["ao"],
        &["ai"],
        &["ian"],
        &["in"],
        &["uo", "o"],
        &["un"],
        &["iu"],
        &["uan", "er"],
        &["ong", "iong"],
        &["üe", "ue"],
        &["u"],
        &["ui"],
        &["ia", "ua"],
        &["ie"],
        &["uai", "ü"],
        &["ei"],
        &["ing"],
    ],
    standalone: GOLDEN_VOWEL_STANDALONE,
};

/// The pre-migration Ziguang table: the migration's golden data.
const GOLDEN_ZIGUANG: Golden = Golden {
    id: SchemeId::ZIGUANG,
    name: "ziguang",
    initials: GOLDEN_ZIGUANG_INITIALS,
    finals: [
        &["a"],
        &["iao"],
        &[],
        &["ie"],
        &["e"],
        &["ian"],
        &["iang", "uang"],
        &["ong", "iong"],
        &["i"],
        &["er", "iu"],
        &["ei"],
        &["uan"],
        &["un"],
        &["üe", "ue", "ui"],
        &["uo", "o"],
        &["ai"],
        &["ao"],
        &["an"],
        &["ang"],
        &["eng", "ng"],
        &["u"],
        &["ü"],
        &["en"],
        &["ia", "ua"],
        &["in", "uai"],
        &["ou"],
        &["ing"],
    ],
    standalone: GOLDEN_VOWEL_STANDALONE,
};

/// The initials shared by Xiaohe and Ziranma: every letter carries itself except
/// the three retroflexes.
const GOLDEN_SELF_INITIALS: [Option<&str>; SCHEME_KEYS] = [
    Some("a"),
    Some("b"),
    Some("c"),
    Some("d"),
    Some("e"),
    Some("f"),
    Some("g"),
    Some("h"),
    Some("ch"),
    Some("j"),
    Some("k"),
    Some("l"),
    Some("m"),
    Some("n"),
    Some("o"),
    Some("p"),
    Some("q"),
    Some("r"),
    Some("s"),
    Some("t"),
    Some("sh"),
    Some("zh"),
    Some("w"),
    Some("x"),
    Some("y"),
    Some("z"),
    None,
];

/// The initials shared by Microsoft and Sogou: the zero initial sits on `o`.
const GOLDEN_OLED_INITIALS: [Option<&str>; SCHEME_KEYS] = [
    Some("a"),
    Some("b"),
    Some("c"),
    Some("d"),
    Some("e"),
    Some("f"),
    Some("g"),
    Some("h"),
    Some("ch"),
    Some("j"),
    Some("k"),
    Some("l"),
    Some("m"),
    Some("n"),
    Some(""),
    Some("p"),
    Some("q"),
    Some("r"),
    Some("s"),
    Some("t"),
    Some("sh"),
    Some("zh"),
    Some("w"),
    Some("x"),
    Some("y"),
    Some("z"),
    None,
];

/// Ziguang's initials: the retroflexes moved, `e` and `v` opening nothing.
const GOLDEN_ZIGUANG_INITIALS: [Option<&str>; SCHEME_KEYS] = [
    Some("ch"),
    Some("b"),
    Some("c"),
    Some("d"),
    None,
    Some("f"),
    Some("g"),
    Some("h"),
    Some("sh"),
    Some("j"),
    Some("k"),
    Some("l"),
    Some("m"),
    Some("n"),
    Some(""),
    Some("p"),
    Some("q"),
    Some("r"),
    Some("s"),
    Some("t"),
    Some("zh"),
    None,
    Some("w"),
    Some("x"),
    Some("y"),
    Some("z"),
    None,
];

/// The standalone syllables every published layout carries: the vowels alone.
const GOLDEN_VOWEL_STANDALONE: [&str; SCHEME_KEYS] = [
    "a", "", "", "", "e", "", "", "", "", "", "", "", "", "", "o", "", "", "", "", "", "", "", "",
    "", "", "", "",
];

/// The pre-migration shape of a layout table, as the migration's golden data.
struct Golden {
    id: SchemeId,
    name: &'static str,
    initials: [Option<&'static str>; SCHEME_KEYS],
    finals: [&'static [&'static str]; SCHEME_KEYS],
    standalone: [&'static str; SCHEME_KEYS],
}

/// Every golden table, in scheme-number order.
fn goldens() -> [&'static Golden; 5] {
    [
        &GOLDEN_XIAOHE,
        &GOLDEN_ZIRANMA,
        &GOLDEN_MICROSOFT,
        &GOLDEN_SOGOU,
        &GOLDEN_ZIGUANG,
    ]
}

/// Every key a layout is indexed by: `a`-`z` and `;`.
fn alphabet() -> Vec<u8> {
    let mut keys: Vec<u8> = (b'a'..=b'z').collect();
    keys.push(b';');
    keys
}

/// Joins one initial and one final the way the rewrite does: a final that already
/// begins with its initial is used as it stands, and the zero initial contributes
/// nothing.
fn golden_join(initial: &str, final_text: &str) -> String {
    if initial.is_empty() || final_text.starts_with(initial) {
        String::from(final_text)
    } else {
        format!("{initial}{final_text}")
    }
}

/// Maps a key pair through the golden table, replicating the pair-first rule.
fn golden_pair(golden: &Golden, first_slot: usize, second_slot: usize) -> Option<SyllableId> {
    let initial = golden.initials[first_slot]?;
    for final_text in golden.finals[second_slot] {
        let joined = golden_join(initial, final_text);
        if let Some(full) = lookup(&joined) {
            return Some(full);
        }
    }
    None
}

/// Maps a key sequence through the golden table: the pair first, then the key on
/// its own.
fn golden_map(golden: &Golden, keys: &[u8]) -> Option<(SyllableId, u8)> {
    let first = *keys.first()?;
    let first_slot = key_index(first)?;
    if let Some(&second) = keys.get(1)
        && let Some(second_slot) = key_index(second)
        && let Some(full) = golden_pair(golden, first_slot, second_slot)
    {
        return Some((full, 2));
    }
    let standalone = golden.standalone[first_slot];
    if standalone.is_empty() {
        return None;
    }
    Some((lookup(standalone)?, 1))
}

/// Whether the golden table defines `slot`, the rule the rewrite drops undefined
/// keys by.
fn golden_defines(golden: &Golden, slot: usize) -> bool {
    golden.initials[slot].is_some()
        || !golden.finals[slot].is_empty()
        || !golden.standalone[slot].is_empty()
}

/// Whether the compiled table defines `slot`, read off its public fields.
fn compiled_defines(table: &SchemeTable, slot: usize) -> bool {
    table.initials[slot].is_some()
        || !table.finals[slot].is_empty()
        || !table.standalone[slot].is_empty()
}

/// Asserts the compiled mapping is the golden one, naming the layout and input on
/// failure.
fn assert_same_mapping(
    golden: &Golden,
    keys: &[u8],
    from_table: Option<ime_core::shuangpin::MappedSyllable>,
    from_golden: Option<(SyllableId, u8)>,
) {
    let typed = keys.iter().map(|key| *key as char).collect::<String>();
    match (from_table, from_golden) {
        (None, None) => {}
        (Some(mapped), Some((full, consumed))) => {
            assert_eq!(mapped.full, full, "{}: {typed:?}", golden.name);
            assert_eq!(
                mapped.consumed, consumed,
                "{}: {typed:?} consumed the wrong count",
                golden.name
            );
        }
        (from_table, from_golden) => panic!(
            "{}: {typed:?} mapped as {from_table:?} against golden {from_golden:?}",
            golden.name
        ),
    }
}

#[test]
fn test_builtin_tables_behave_identically_to_the_pre_migration_tables() {
    // The migration's whole promise: the same keystroke in, the same syllable out,
    // over the entire key space of every layout, plus the drop rule the rewrite
    // prunes undefined keys by.
    for golden in goldens() {
        let table = table_for(golden.id)
            .expect("a published scheme is implemented")
            .expect("a published scheme has a table");
        let keys = alphabet();
        for &first in &keys {
            for &second in &keys {
                let pair = [first, second];
                assert_same_mapping(
                    golden,
                    &pair,
                    map_syllable(table, &pair).expect("alphabet keys"),
                    golden_map(golden, &pair),
                );
            }
            let single = [first];
            assert_same_mapping(
                golden,
                &single,
                map_syllable(table, &single).expect("alphabet keys"),
                golden_map(golden, &single),
            );
        }
        for slot in 0..SCHEME_KEYS {
            assert_eq!(
                compiled_defines(table, slot),
                golden_defines(golden, slot),
                "{}: slot {slot} defined-ness moved",
                golden.name
            );
        }
    }
}

/// The spellings the whole key space of a table reaches.
fn reachable_through(table: &SchemeTable) -> Vec<&'static str> {
    let keys = alphabet();
    let mut out = Vec::new();
    for &first in &keys {
        for &second in &keys {
            let pair = [first, second];
            if let Some(mapped) = map_syllable(table, &pair).expect("alphabet keys")
                && let Some(spelling) = syllable_at(mapped.full)
            {
                out.push(spelling);
            }
        }
        let single = [first];
        if let Some(mapped) = map_syllable(table, &single).expect("alphabet keys")
            && let Some(spelling) = syllable_at(mapped.full)
        {
            out.push(spelling);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The syllables a valid layout may leave unreachable, and why: the interjections
/// no published layout puts on a key, and `lo`, which the shared `uo`/`o` key of
/// every published layout resolves to `luo` on purpose.
const OPTIONAL_SYLLABLES: [&str; 7] = ["m", "n", "ng", "hm", "hng", "ê", "lo"];

#[test]
fn test_every_builtin_layout_spells_every_required_syllable_of_the_table() {
    for golden in goldens() {
        let table = table_for(golden.id)
            .expect("a published scheme is implemented")
            .expect("a published scheme has a table");
        let reachable = reachable_through(table);
        for syllable in SYLLABLES {
            if OPTIONAL_SYLLABLES.contains(syllable) {
                continue;
            }
            assert!(
                reachable.contains(syllable),
                "{} cannot spell {syllable}",
                golden.name
            );
        }
    }
}

/// Maps `keys` through the named built-in layout and returns the spelling.
fn spelled(table: &SchemeTable, keys: &[u8]) -> &'static str {
    let mapped = map_syllable(table, keys)
        .expect("keys inside the alphabet")
        .expect("the official keystrokes must map");
    assert_eq!(
        mapped.consumed,
        u8::try_from(keys.len()).expect("short"),
        "{keys:?} consumed the wrong key count"
    );
    syllable_at(mapped.full).expect("a mapped syllable is in the table")
}

#[test]
fn test_xiaohe_official_vowel_initial_keystrokes_reach_their_syllables() {
    // The first-letter school: the syllable opens with its own letter.
    let cases: [(&[u8], &str); 11] = [
        (b"aa", "a"),
        (b"ad", "ai"),
        (b"aj", "an"),
        (b"ah", "ang"),
        (b"ac", "ao"),
        (b"ee", "e"),
        (b"ef", "en"),
        (b"eg", "eng"),
        (b"oo", "o"),
        (b"oz", "ou"),
        (b"er", "er"),
    ];
    let table = xiaohe::table();
    for (keys, expected) in cases {
        assert_eq!(spelled(table, keys), expected, "{keys:?}");
    }
}

#[test]
fn test_ziranma_official_vowel_initial_keystrokes_reach_their_syllables() {
    let cases: [(&[u8], &str); 11] = [
        (b"aa", "a"),
        (b"al", "ai"),
        (b"aj", "an"),
        (b"ah", "ang"),
        (b"ak", "ao"),
        (b"ee", "e"),
        (b"ef", "en"),
        (b"eg", "eng"),
        (b"oo", "o"),
        (b"ob", "ou"),
        (b"er", "er"),
    ];
    let table = ziranma::table();
    for (keys, expected) in cases {
        assert_eq!(spelled(table, keys), expected, "{keys:?}");
    }
}

#[test]
fn test_microsoft_official_vowel_initial_keystrokes_reach_their_syllables() {
    // The o-led school; the un-prefixed spelling Rime's derive rule keeps is
    // asserted beside the o-led one. The zero initial answers the vowel-initial
    // syllables only, which is why every case below is one of them.
    let cases: [(&[u8], &str); 13] = [
        (b"oa", "a"),
        (b"ol", "ai"),
        (b"oj", "an"),
        (b"oh", "ang"),
        (b"ok", "ao"),
        (b"oe", "e"),
        (b"oz", "ei"),
        (b"of", "en"),
        (b"og", "eng"),
        (b"oo", "o"),
        (b"ob", "ou"),
        (b"aa", "a"),
        (b"al", "ai"),
    ];
    let table = microsoft::table();
    for (keys, expected) in cases {
        assert_eq!(spelled(table, keys), expected, "{keys:?}");
    }
}

#[test]
fn test_sogou_official_vowel_initial_keystrokes_reach_their_syllables() {
    // Both spellings the publisher's documentation gives: the `o` prefix and the
    // repeated first letter. Every case is one of the vowel-initial syllables the
    // zero initial exists for; `ai` sits on `l` in this layout.
    let cases: [(&[u8], &str); 12] = [
        (b"oa", "a"),
        (b"ol", "ai"),
        (b"or", "er"),
        (b"oj", "an"),
        (b"ok", "ao"),
        (b"oe", "e"),
        (b"oz", "ei"),
        (b"of", "en"),
        (b"og", "eng"),
        (b"oo", "o"),
        (b"ob", "ou"),
        (b"aa", "a"),
    ];
    let table = sogou::table();
    for (keys, expected) in cases {
        assert_eq!(spelled(table, keys), expected, "{keys:?}");
    }
}

#[test]
fn test_ziguang_official_vowel_initial_keystrokes_reach_their_syllables() {
    let cases: [(&[u8], &str); 11] = [
        (b"oa", "a"),
        (b"op", "ai"),
        (b"oq", "ao"),
        (b"or", "an"),
        (b"os", "ang"),
        (b"oe", "e"),
        (b"ow", "en"),
        (b"ot", "eng"),
        (b"oo", "o"),
        (b"oz", "ou"),
        (b"oj", "er"),
    ];
    let table = ziguang::table();
    for (keys, expected) in cases {
        assert_eq!(spelled(table, keys), expected, "{keys:?}");
    }
}

/// Mutates the Xiaohe fixture document for one validator case.
fn mutated(replacement: &str, needle: &str) -> String {
    XIAOHE_DOCUMENT.replacen(needle, replacement, 1)
}

/// Compiles a document, failing the test when the load unexpectedly refuses.
fn compiled(document: &LayoutDocument) -> SchemeTable {
    compile_layout(document, SchemeId::FULL).expect("the document must load")
}

/// Asserts the document refuses, and that some diagnostic names `reason_part`.
fn assert_refused(text: &str, reason_part: &str) {
    let errors = match parse_layout(text) {
        Ok(document) => compile_layout(&document, SchemeId::FULL)
            .expect_err("the document must refuse to load"),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.to_string().contains(reason_part)),
        "no diagnostic names {reason_part:?}: {errors:?}"
    );
    for error in errors {
        let rendered = error.to_string();
        assert!(
            rendered.contains(LAYOUT_DIAGNOSTIC_KEY),
            "the diagnostic must carry the stable code: {rendered}"
        );
    }
}

#[test]
fn test_compile_layout_accepts_a_final_reached_from_two_keys() {
    // Microsoft reaches `üe` through both `t` and `v` -- two ways of typing one
    // sound is the published standard, not a conflict.
    let text = mutated("l = \"iang uang ai\"", "l = \"iang uang\"");
    let document = parse_layout(&text).expect("the text parses");
    let table = compiled(&document);
    let finals: Vec<&str> = table.finals[usize::from(b'l' - b'a')]
        .iter()
        .map(|final_text| final_text.as_ref())
        .collect();
    assert_eq!(finals, ["iang", "uang", "ai"], "trial order is kept");
}

#[test]
fn test_compile_layout_rejects_an_initial_claimed_by_two_keys() {
    let text = mutated("b = \"zh\"", "b = \"b\"");
    assert_refused(&text, "conflicting mapping");
}

#[test]
fn test_compile_layout_rejects_a_key_outside_the_alphabet() {
    let text = mutated("[finals]\n1 = \"ai\"\n", "[finals]\n");
    assert_refused(&text, "is not a scheme key");
}

#[test]
fn test_compile_layout_rejects_an_initial_the_table_does_not_know() {
    let text = mutated("b = \"xh\"", "b = \"b\"");
    assert_refused(&text, "is not a syllable initial");
}

#[test]
fn test_compile_layout_rejects_a_final_the_table_does_not_know() {
    let text = mutated("b = \"inn\"", "b = \"in\"");
    assert_refused(&text, "is not a syllable final");
}

#[test]
fn test_compile_layout_rejects_a_document_missing_a_final() {
    // Drop the `in` final and every -in syllable loses its keystrokes.
    let text = mutated("", "b = \"in\"\n");
    assert_refused(&text, "cannot be spelled");
}

#[test]
fn test_compile_layout_rejects_a_first_letter_document_with_a_zero_initial() {
    let text = mutated("o = \"\"", "o = \"o\"");
    assert_refused(&text, "first-letter layout");
}

#[test]
fn test_compile_layout_rejects_an_o_led_document_without_the_zero_initial() {
    let text = mutated(
        "zero_initial_style = \"o-led\"",
        "zero_initial_style = \"first-letter\"",
    );
    assert_refused(&text, "o-led layout");
}

#[test]
fn test_compile_layout_rejects_a_second_zero_initial() {
    // Microsoft is o-led; a second empty initial on `e` is mixed-school data.
    let text = include_str!("../src/shuangpin/data/microsoft.toml")
        .replacen("e = \"e\"", "e = \"\"", 1);
    assert_refused(&text, "only key `o` may carry the zero initial");
}

#[test]
fn test_compile_layout_rejects_an_unknown_school_spelling() {
    let text = mutated(
        "zero_initial_style = \"oled\"",
        "zero_initial_style = \"first-letter\"",
    );
    assert_refused(&text, "unknown zero_initial_style spelling");
}

#[test]
fn test_compile_layout_rejects_a_document_without_a_name() {
    let text = mutated("", "name = \"小鹤双拼\"\n");
    assert_refused(&text, "must name itself");
}

#[test]
fn test_parse_layout_reports_a_syntax_error_with_a_line_and_no_content() {
    let errors = parse_layout("[layout]\nname = \"x\"\n@@@\n")
        .expect_err("the document is not TOML");
    assert_eq!(errors.len(), 1);
    let rendered = errors[0].to_string();
    assert!(
        rendered.contains("line 3"),
        "the diagnostic names the line: {rendered}"
    );
    assert!(!rendered.contains("@@@"), "the value is not quoted");
}

#[test]
fn test_parse_layout_rejects_a_table_the_format_does_not_define() {
    let errors = parse_layout("[layout]\nname = \"x\"\nzero_initial_style = \"o-led\"\n[bogus]\na = \"a\"\n")
        .expect_err("the table is not part of the format");
    let rendered = errors[0].to_string();
    assert!(
        rendered.contains("unknown table or key"),
        "the diagnostic names the shape of the mistake: {rendered}"
    );
}

#[test]
fn test_compile_layout_never_quotes_a_rejected_entry_value() {
    // `xh` is the typo; the diagnostic names the key it sits on, never the value.
    let text = mutated("b = \"xh\"", "b = \"b\"");
    let document = parse_layout(&text).expect("the text parses");
    let errors = compile_layout(&document, SchemeId::FULL).expect_err("the entry is invalid");
    assert!(!errors.is_empty());
    for error in errors {
        let rendered = error.to_string();
        assert!(
            !rendered.contains("xh"),
            "the value must not be quoted: {rendered}"
        );
        assert!(
            rendered.contains("key `b`"),
            "the key is named: {rendered}"
        );
    }
}

#[test]
fn test_a_user_layout_document_compiles_to_the_builtin_xiaohe_behaviour() {
    // The fixture the card asks for: a user document holding exactly the Xiaohe
    // data must behave exactly like the built-in Xiaohe over the whole key space.
    let document = parse_layout(XIAOHE_DOCUMENT).expect("the fixture parses");
    let compiled = compiled(&document);
    let keys = alphabet();
    for &first in &keys {
        for &second in &keys {
            let pair = [first, second];
            assert_eq!(
                map_syllable(&compiled, &pair).expect("alphabet keys"),
                map_syllable(xiaohe::table(), &pair).expect("alphabet keys"),
                "{:?} diverges from the built-in",
                core::str::from_utf8(&pair)
            );
        }
    }
}

#[test]
fn test_a_rejected_layout_leaves_the_selected_builtin_in_place() {
    // The fallback the card mandates: a wrong layout refuses to load, and the
    // built-in scheme the user selected keeps answering.
    let broken = mutated("o = \"\"", "o = \"o\"");
    let document = parse_layout(&broken).expect("the text parses");
    assert!(
        compile_layout(&document, SchemeId::FULL).is_err(),
        "the mixed-school document must refuse"
    );
    let selected = table_for(SchemeId::SOGOU)
        .expect("the scheme is implemented")
        .expect("the scheme has a table");
    // One syllable through the mapping, and a whole word through the rewrite:
    // the selected built-in keeps answering exactly as before the refused load.
    assert_eq!(spelled(selected, b"vs"), "zhong");
    let rewritten = ime_core::shuangpin::rewrite_for_scheme("vsgo", SchemeId::SOGOU)
        .expect("the scheme is implemented");
    assert_eq!(rewritten, "zhongguo");
    assert_eq!(spelled(selected, b"oj"), "an");
}

/// The Xiaohe initials as the configuration's inline map holds them.
fn xiaohe_inline_initials() -> BTreeMap<String, String> {
    [
        ("a", "a"),
        ("b", "b"),
        ("c", "c"),
        ("d", "d"),
        ("e", "e"),
        ("f", "f"),
        ("g", "g"),
        ("h", "h"),
        ("i", "ch"),
        ("j", "j"),
        ("k", "k"),
        ("l", "l"),
        ("m", "m"),
        ("n", "n"),
        ("o", "o"),
        ("p", "p"),
        ("q", "q"),
        ("r", "r"),
        ("s", "s"),
        ("t", "t"),
        ("u", "sh"),
        ("v", "zh"),
        ("w", "w"),
        ("x", "x"),
        ("y", "y"),
        ("z", "z"),
    ]
    .into_iter()
    .map(|(key, value)| (String::from(key), String::from(value)))
    .collect()
}

/// The Xiaohe finals as the configuration's inline map holds them.
fn xiaohe_inline_finals() -> BTreeMap<String, String> {
    [
        ("a", "a"),
        ("b", "in"),
        ("c", "ao"),
        ("d", "ai"),
        ("e", "e"),
        ("f", "en"),
        ("g", "eng ng"),
        ("h", "ang"),
        ("i", "i"),
        ("j", "an"),
        ("k", "uai ing"),
        ("l", "iang uang"),
        ("m", "ian"),
        ("n", "iao"),
        ("o", "uo o"),
        ("p", "ie"),
        ("q", "iu"),
        ("r", "uan er"),
        ("s", "ong iong"),
        ("t", "üe ue"),
        ("u", "u"),
        ("v", "ü ui"),
        ("w", "ei"),
        ("x", "ia ua"),
        ("y", "un"),
        ("z", "ou"),
    ]
    .into_iter()
    .map(|(key, value)| (String::from(key), String::from(value)))
    .collect()
}

#[test]
fn test_the_config_maps_build_a_document_the_validator_accepts() {
    // The typed path: the inline `[shuangpin.custom]` tables arrive as plain maps
    // and must compile to the same layout the TOML document compiles to.
    let document = LayoutDocument::from_tables(LayoutTables {
        name: Some(String::from("小鹤双拼")),
        zero_initial_style: Some(String::from("first-letter")),
        initials: xiaohe_inline_initials(),
        finals: xiaohe_inline_finals(),
        standalone: BTreeMap::new(),
    });
    let compiled = compiled(&document);
    for keys in [&b"nihc"[..], &b"vsgo"[..], &b"aa"[..], &b"lv"[..], &b"tk"[..]] {
        assert_eq!(
            map_syllable(&compiled, keys).expect("alphabet keys"),
            map_syllable(xiaohe::table(), keys).expect("alphabet keys"),
            "{keys:?} diverges from the built-in",
        );
    }
    assert_eq!(
        compiled.initials[usize::from(b'v' - b'a')].as_deref(),
        Some("zh")
    );
    assert_eq!(compiled.finals[usize::from(b'k' - b'a')].len(), 2);
    // The default standalone syllables arrived with no explicit table.
    assert_eq!(compiled.standalone[0].as_ref(), "a");
    assert_eq!(compiled.standalone[usize::from(b'o' - b'a')].as_ref(), "o");
}

#[test]
fn test_compile_layout_is_deterministic_across_reloads() {
    // A reload recompiles the document from scratch; two compiles of one document
    // must agree value for value.
    let document = parse_layout(XIAOHE_DOCUMENT).expect("the fixture parses");
    assert_eq!(compiled(&document), compiled(&document));
}

#[test]
fn test_the_registered_tables_are_exactly_the_published_schemes() {
    let registered = tables();
    assert_eq!(registered.len(), 5);
    let mut seen: Vec<u8> = Vec::new();
    for (id, table) in registered {
        assert!(id.is_known());
        assert_eq!(table.id, *id, "a table must identify itself");
        assert!(
            !seen.contains(&id.value()),
            "a scheme number is registered twice"
        );
        seen.push(id.value());
        assert!(table_for(*id).is_ok(), "the scheme must resolve");
    }
    assert!(
        table_for(SchemeId::FULL)
            .expect("full pinyin is supported")
            .is_none(),
        "full pinyin needs no table"
    );
    assert!(matches!(
        table_for(SchemeId::from_u8(SchemeId::COUNT)),
        Err(ImeError::SchemeUnsupported { .. })
    ));
}
