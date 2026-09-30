//! The scenario set the harness runs when no directory is given.
//!
//! These are the cases the engine's contracts need today, written as data rather than as
//! test functions so that a case in `tests.md` can name the scenario it runs. They are
//! not a replacement for the engine's own unit tests: a unit test can reach into a
//! private helper, while a scenario can only see what a caller sees. The two are
//! complements, and the harness keeps both.
//!
//! Every scenario here is a real assertion. A step whose expectation is only that the
//! engine answered would be indistinguishable from a passing one, so there are none.
//!
//! # What this set cannot drive
//!
//! A step carries a `KeyAction`, so a scenario here begins *after* the routing table: the
//! keystroke that produced the action is never exercised, and a green run says nothing
//! about whether a key can reach the machine at all. The other side of that gap is
//! `crates/ime-fcitx5/tests/keymap_matrix.rs`, which drives real keysyms through the
//! table, the bus, the session and the host boundary. Closing it *here* would take two
//! changes outside this module: `xtask` would have to link `ime-fcitx5` for
//! `translate_key`, and the step type would need a variant carrying a keysym and a
//! modifier mask instead of an action. Until both exist, the shortcut table's composing
//! rows are covered only where a `KeyAction` can express them -- typing, Backspace, the
//! caret and Escape -- and the rows whose effect is a commit, a selection or a page flip
//! are outside this harness's reach entirely.

use ime_core::segment::MAX_RAW_LEN;
use ime_types::KeyAction;

use crate::testd::engine::scenario::{
    BigramRow, DictionarySpec, Expectation, Scenario, SingleRow, Step, UserWordRow, WordRow,
};

/// The scenarios the harness runs when no directory is given.
///
/// # Panics
///
/// Never panics.
pub fn scenarios() -> Vec<Scenario> {
    let mut all = vec![
        empty_input(),
        input_without_a_path(),
        input_past_the_length_limit(),
        input_outside_the_alphabet(),
        folded_marker(),
        typing_reaches_a_word(),
        backspace_removes_a_syllable(),
        caret_moves_by_syllable(),
        caret_returns_after_moving_back(),
        escape_takes_back_the_composition(),
        ranking_follows_the_unigram(),
        ranking_follows_the_user_frequency(),
        ranking_follows_the_language_model(),
        falls_back_to_single_characters(),
        fills_a_page_and_the_next(),
    ];
    // One scenario per code the mock dictionary can refuse a lookup with, so that the
    // degraded branch of the decode is covered for every one of them and not only for the
    // two a Phase 1 dictionary is expected to raise in practice.
    for code in ["dict/unavailable", "dict/corrupt", "dict/unsupported"] {
        all.push(refused_lookup(code));
    }
    all
}

/// A decode of nothing: the buffer holds no input and the engine degrades.
///
/// The decoder answers a request it cannot read with the pass-through candidate rather
/// than with an error, so the code is what the segmentation layer reports for an empty
/// input.
fn empty_input() -> Scenario {
    Scenario::new(
        "engine-empty-input",
        DictionarySpec::default(),
        vec![decode(degraded("decode/empty-input"))],
    )
}

/// Input no syllable of the table can spell.
fn input_without_a_path() -> Scenario {
    Scenario::new(
        "engine-input-without-a-path",
        DictionarySpec::default(),
        vec![
            key('z', candidates("z", 1, &[])),
            key('z', candidates("zz", 1, &[])),
            key('z', degraded("decode/no-path")),
        ],
    )
}

/// The hard length limit, from one byte to one byte past it.
///
/// An empty dictionary is deliberate: every prefix then has no reading at all, so the
/// whole run is the pass-through path and the only expectation a step needs is the text
/// the engine echoes back.
fn input_past_the_length_limit() -> Scenario {
    let mut steps = Vec::with_capacity(MAX_RAW_LEN + 1);
    for len in 1..=MAX_RAW_LEN {
        let raw = "a".repeat(len);
        steps.push(key('a', candidates(&raw, 1, &[])));
    }
    steps.push(key('a', degraded("decode/too-long")));
    Scenario::new(
        "engine-input-past-the-length-limit",
        DictionarySpec::default(),
        steps,
    )
}

/// A character outside the input alphabet, refused by the buffer.
fn input_outside_the_alphabet() -> Scenario {
    Scenario::new(
        "engine-input-outside-the-alphabet",
        DictionarySpec::default(),
        vec![
            key('n', candidates("n", 1, &[])),
            key('3', degraded("decode/invalid-char")),
        ],
    )
}

/// A boundary marker that leads the input, which normalization folds away.
///
/// The marker is reported as a discarded character, and the input still decodes: the
/// spelling the graph holds is the one the preedit renders.
fn folded_marker() -> Scenario {
    let spec = dictionary(&[("n", &["嗯"][..]), ("ni", &["你"][..])]);
    Scenario::new(
        "engine-folds-a-leading-marker",
        spec,
        vec![
            key('\'', degraded("decode/invalid-char")),
            key('n', preedit("n", 1, 2)),
            key('i', candidates("你", 1, &[])),
        ],
    )
}

/// Five keystrokes, from a bare letter to a word.
fn typing_reaches_a_word() -> Scenario {
    Scenario::new(
        "engine-typing-reaches-a-word",
        two_syllable_dictionary(),
        vec![
            key('n', candidates("n", 1, &[])),
            key('i', preedit("ni", 2, 2)),
            key('h', candidates("nih", 1, &[])),
            key('a', preedit("ni'ha", 5, 4)),
            key('o', candidates("你好", 1, &[])),
        ],
    )
}

/// Backspace at the end of the input removes the trailing syllable.
///
/// The grid it deletes by is the one the previous step's segmentation wrote back, so the
/// scenario fails if the write-back stops happening.
fn backspace_removes_a_syllable() -> Scenario {
    let mut steps = typing_a_two_syllable_word();
    steps.push(backspace(preedit("ni", 2, 2)));
    Scenario::new(
        "engine-backspace-removes-a-syllable",
        two_syllable_dictionary(),
        steps,
    )
}

/// The caret moves between syllable boundaries and the preedit follows it.
///
/// Moving back one syllable puts the caret in front of the second syllable, which is
/// where the cursor span has to land -- after the separator, not before it.
fn caret_moves_by_syllable() -> Scenario {
    let mut steps = typing_a_two_syllable_word();
    steps.push(move_caret(-1, preedit("ni'hao", 3, 4)));
    Scenario::new(
        "engine-caret-moves-by-syllable",
        two_syllable_dictionary(),
        steps,
    )
}

/// The caret moves back to the end of the input, one syllable at a time.
///
/// The forward half of the pair the shortcut table names: `Left` steps the caret towards
/// the start of the input and `Right` steps it back, and the preedit follows both. A grid
/// written back by the previous step is what makes the second move land where it does, so
/// this scenario fails if the write-back stops happening.
fn caret_returns_after_moving_back() -> Scenario {
    let mut steps = typing_a_two_syllable_word();
    steps.push(move_caret(-1, preedit("ni'hao", 3, 4)));
    steps.push(move_caret(1, preedit("ni'hao", 6, 4)));
    Scenario::new(
        "engine-caret-returns-after-moving-back",
        two_syllable_dictionary(),
        steps,
    )
}

/// Escape takes the composition back: the input is dropped and nothing is committed.
///
/// The step after the cancel decodes an empty input, which is the same observation the
/// empty-input scenario makes -- the codes the engine surfaces for an input it cannot
/// read are what tells the two apart from a step that did nothing.
fn escape_takes_back_the_composition() -> Scenario {
    let mut steps = typing_a_two_syllable_word();
    steps.push(escape(degraded("decode/empty-input")));
    Scenario::new(
        "engine-escape-takes-back-the-composition",
        two_syllable_dictionary(),
        steps,
    )
}

/// Two words under one key, ranked by their unigram scores.
fn ranking_follows_the_unigram() -> Scenario {
    let spec = with_unigrams(
        dictionary(&[("de", &["的", "得"][..])]),
        &[("的", -128), ("得", -512)],
    );
    Scenario::new(
        "engine-ranking-follows-the-unigram",
        spec,
        vec![
            key('d', candidates("d", 1, &[])),
            key('e', candidates("的", 2, &["得"])),
        ],
    )
}

/// The same two words, with the user's own count deciding instead of the model.
fn ranking_follows_the_user_frequency() -> Scenario {
    let spec = with_user_freq(
        with_user_words(
            with_unigrams(
                dictionary(&[("de", &["的", "得"][..])]),
                &[("的", -128), ("得", -256)],
            ),
            &["得"],
        ),
        &[("得", 1000)],
    );
    Scenario::new(
        "engine-ranking-follows-the-user-frequency",
        spec,
        vec![
            key('d', candidates("d", 1, &[])),
            key('e', candidates("得", 2, &["的"])),
        ],
    )
}

/// One key that reads both as one word and as two, ranked by the model.
///
/// `xian` reads as `xian` and as `xi'an`, and both readings are in the dictionary. The
/// model prefers the two-word reading, so the whole-word candidate must not come first
/// merely because it is one word; the first word of the winning path is offered beside
/// them, which is what makes the list three long.
fn ranking_follows_the_language_model() -> Scenario {
    let spec = with_bigram(
        with_unigrams(
            dictionary(&[
                ("xian", &["先"][..]),
                ("xi", &["西"][..]),
                ("an", &["安"][..]),
            ]),
            &[("先", -2048), ("西", -128), ("安", -128)],
        ),
        "西",
        "安",
        -64,
    );
    Scenario::new(
        "engine-ranking-follows-the-language-model",
        spec,
        vec![
            key('x', candidates("x", 1, &[])),
            key('i', candidates("西", 1, &[])),
            key('a', candidates("xia", 1, &[])),
            key('n', candidates("西安", 3, &["先", "西"])),
        ],
    )
}

/// One input whose readings fill more than one page of the window.
///
/// `dede` reads as `de|de` and nothing else, so every reading is either one word of the
/// `de'de` key or a pair of words from the `de` key. The beam keeps sixteen paths to the
/// end of the input, and every one of them spells a different text, so the list holds
/// sixteen candidates: four pages of the five a row holds by default, the last of them
/// holding one.
fn fills_a_page_and_the_next() -> Scenario {
    let single = ["的", "得", "德", "地", "底", "低", "滴", "敌"];
    let whole = [
        "得到", "得知", "得意", "得罪", "得体", "得手", "得救", "得逞",
    ];
    Scenario::new(
        "engine-fills-a-page-and-the-next",
        dictionary(&[("de", &single[..]), ("de'de", &whole[..])]),
        vec![
            key('d', candidates("d", 1, &[])),
            key('e', candidates("的", 8, &["敌"])),
            key('d', candidates("ded", 1, &[])),
            key('e', candidates("得到", 16, &["得逞", "的敌"])),
            decode(page(1, 4)),
        ],
    )
}

/// A dictionary that refuses one key's lookup, named by the code it refuses with.
fn refused_lookup(code: &str) -> Scenario {
    let spec = with_failures(
        dictionary(&[("ni", &["你"][..]), ("hao", &["好"][..])]),
        &[("ni'hao", code)],
    );
    Scenario::new(
        format!("engine-refused-lookup-{}", code.replace('/', "-")),
        spec,
        vec![
            key('n', candidates("n", 1, &[])),
            key('i', candidates("你", 1, &[])),
            key('h', candidates("nih", 1, &[])),
            key('a', candidates("niha", 1, &[])),
            key('o', degraded(code)),
        ],
    )
}

/// A dictionary with no word for a syllable, falling back to its single characters.
///
/// The frozen contract promises that every syllable yields at least one candidate, so
/// that any input with a legal segmentation produces something the user can commit. The
/// fallback only fires for a span no word covers, which is why the whole word never
/// appears here.
fn falls_back_to_single_characters() -> Scenario {
    let spec = with_singles(
        DictionarySpec::default(),
        &[("ni", &["伱"][..]), ("hao", &["号"][..])],
    );
    Scenario::new(
        "engine-falls-back-to-single-characters",
        spec,
        vec![
            key('n', candidates("n", 1, &[])),
            key('i', candidates("伱", 1, &[])),
            key('h', candidates("nih", 1, &[])),
            key('a', candidates("niha", 1, &[])),
            key('o', candidates("伱号", 2, &["伱"])),
        ],
    )
}

/// The five keystrokes of `nihao`, as every two-syllable scenario types them.
fn typing_a_two_syllable_word() -> Vec<Step> {
    vec![
        key('n', candidates("n", 1, &[])),
        key('i', candidates("你", 1, &[])),
        key('h', candidates("nih", 1, &[])),
        key('a', candidates("niha", 1, &[])),
        key('o', candidates("你好", 1, &[])),
    ]
}

/// The dictionary the two-syllable scenarios run against.
fn two_syllable_dictionary() -> DictionarySpec {
    dictionary(&[
        ("ni", &["你"][..]),
        ("hao", &["好"][..]),
        ("ni'hao", &["你好"][..]),
    ])
}

/// One `input-char` step.
fn key(ch: char, expect: Expectation) -> Step {
    Step {
        action: KeyAction::InputChar(ch),
        expect,
    }
}

/// One `ignore` step: no key is consumed, and the engine still decodes.
fn decode(expect: Expectation) -> Step {
    Step {
        action: KeyAction::Ignore,
        expect,
    }
}

/// One `backspace` step.
fn backspace(expect: Expectation) -> Step {
    Step {
        action: KeyAction::Backspace,
        expect,
    }
}

/// One `move-caret` step.
fn move_caret(delta: i8, expect: Expectation) -> Step {
    Step {
        action: KeyAction::MoveCaret(delta),
        expect,
    }
}

/// One `escape` step.
fn escape(expect: Expectation) -> Step {
    Step {
        action: KeyAction::Escape,
        expect,
    }
}

/// The candidates expectation.
fn candidates(first: &str, count: usize, contains: &[&str]) -> Expectation {
    Expectation::Candidates {
        first: first.to_owned(),
        count,
        contains: contains.iter().map(|text| (*text).to_owned()).collect(),
    }
}

/// The preedit expectation.
fn preedit(text: &str, caret: u32, spans: usize) -> Expectation {
    Expectation::Preedit {
        text: text.to_owned(),
        caret,
        spans,
    }
}

/// The degraded-code expectation.
fn degraded(code: &str) -> Expectation {
    Expectation::Degraded {
        code: code.to_owned(),
    }
}

/// The page expectation.
fn page(current: u8, total: u8) -> Expectation {
    Expectation::Page { current, total }
}

/// Builds a dictionary spec from `(key, words)` rows.
fn dictionary(rows: &[(&str, &[&str])]) -> DictionarySpec {
    DictionarySpec {
        words: rows
            .iter()
            .map(|(key, texts)| WordRow {
                key: (*key).to_owned(),
                texts: texts.iter().map(|text| (*text).to_owned()).collect(),
            })
            .collect(),
        ..DictionarySpec::default()
    }
}

/// Adds the single-character fallbacks of one syllable to a spec.
fn with_singles(mut spec: DictionarySpec, rows: &[(&str, &[&str])]) -> DictionarySpec {
    spec.singles = rows
        .iter()
        .map(|(syllable, texts)| SingleRow {
            syllable: (*syllable).to_owned(),
            texts: texts.iter().map(|text| (*text).to_owned()).collect(),
        })
        .collect();
    spec
}

/// Adds the keys a dictionary refuses, and the codes they refuse with.
fn with_failures(mut spec: DictionarySpec, rows: &[(&str, &str)]) -> DictionarySpec {
    spec.failures = rows
        .iter()
        .map(|(key, code)| ((*key).to_owned(), (*code).to_owned()))
        .collect();
    spec
}

/// Adds the user's own commit counts to a spec.
fn with_user_freq(mut spec: DictionarySpec, rows: &[(&str, u32)]) -> DictionarySpec {
    spec.user_freq = rows
        .iter()
        .map(|(word, count)| ((*word).to_owned(), *count))
        .collect();
    spec
}

/// Adds the words the user is credited with coining to a spec.
fn with_user_words(mut spec: DictionarySpec, words: &[&str]) -> DictionarySpec {
    spec.user_words = words
        .iter()
        .map(|text| UserWordRow {
            text: (*text).to_owned(),
        })
        .collect();
    spec
}

/// Adds the model's unigram scores to a spec.
fn with_unigrams(mut spec: DictionarySpec, rows: &[(&str, i32)]) -> DictionarySpec {
    spec.unigrams = rows
        .iter()
        .map(|(word, score)| ((*word).to_owned(), *score))
        .collect();
    spec
}

/// Adds one conditional score to a spec.
fn with_bigram(mut spec: DictionarySpec, prev: &str, word: &str, score: i32) -> DictionarySpec {
    spec.bigrams.push(BigramRow {
        prev: prev.to_owned(),
        word: word.to_owned(),
        score,
    });
    spec
}
