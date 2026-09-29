//! Tests for the tuner: folding, ranking, measuring and the holdout generator.
//!
//! The folding, ranking and measuring tests run against a lexicon built from a literal
//! word list in a scratch directory, so nothing here depends on a compiled dictionary.
//! The holdout test reads the repository's own corpus and golden set through the same
//! root resolver `run` uses, because the property it checks -- that the two sets never
//! overlap -- is a property of the files the project ships.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ime_core::lm::{InMemoryLm, Scorer};
use ime_core::segment::lookup;

use super::eval::{evaluate, percent, rank_first_words};
use super::grid::grid_search;
use super::io::{Row, baseline_key, fold_reading, load_rows};
use super::lexicon::{TsvLexicon, build_lexicon, build_lm};
use super::{DEFAULT_CORPUS, DEFAULT_EVAL, DEFAULT_L1, TuneArgs, resolve_root, run};

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-tune-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// An L1 table covering the characters the small fixtures use.
fn test_l1() -> BTreeMap<char, String> {
    [('中', "zhong"), ('国', "guo"), ('你', "ni"), ('好', "hao")]
        .into_iter()
        .map(|(character, reading)| (character, reading.to_owned()))
        .collect()
}

/// A tiny lexicon over the small fixture characters, plus its model.
fn small_lexicon(dir: &Path) -> (TsvLexicon, InMemoryLm) {
    fs::write(
        dir.join("words.tsv"),
        "你好\t300000\n你\t250000\n好\t150000\n",
    )
    .expect("writing the word list");
    let lexicon = build_lexicon(&dir.join("words.tsv"), &test_l1()).expect("building");
    let lm = build_lm(&lexicon);
    (lexicon, lm)
}

/// A holdout run over the repository's own corpus and golden set, with the
/// output redirected into `holdout`.
fn holdout_args(holdout: PathBuf, corpus: PathBuf) -> TuneArgs {
    TuneArgs {
        root: None,
        l1: PathBuf::from(DEFAULT_L1),
        corpus,
        eval: PathBuf::from(DEFAULT_EVAL),
        holdout,
        gen_holdout: true,
        holdout_rows: 5_200,
        grid: false,
    }
}

#[test]
fn test_fold_reading_strips_tones_digits_and_umlauts() {
    let cases = [
        ("nǐ", "ni"),
        ("hǎo", "hao"),
        ("ni3", "ni"),
        ("lǚ", "lü"),
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
fn test_rank_and_evaluate_count_the_first_choice_and_the_reachable_rows() {
    let dir = scratch("evaluate");
    let (lexicon, lm) = small_lexicon(&dir);
    let scorer = Scorer::default();
    assert_eq!(baseline_key("你好", &test_l1()).as_deref(), Some("ni'hao"));
    assert_eq!(
        baseline_key("你龘", &test_l1()),
        None,
        "an uncovered character"
    );

    let candidates = rank_first_words("ni'hao", &lexicon, &lm, &scorer).expect("ranking");
    let words: Vec<&str> = candidates.iter().map(|c| c.word.as_str()).collect();
    assert_eq!(
        words.first(),
        Some(&"你好"),
        "the whole word wins: {words:?}"
    );
    assert!(words.contains(&"你"), "the split path is still a candidate");
    assert!(
        candidates[0].score > candidates[1].score,
        "strictly ordered"
    );
    assert!(
        rank_first_words("zzz", &lexicon, &lm, &scorer)
            .expect("ranking")
            .is_empty(),
        "a key nothing covers produces no candidate"
    );

    let row = |key: &str, word: &str, weight: u32| Row {
        key: key.to_owned(),
        word: word.to_owned(),
        weight,
    };
    let rows = vec![
        row("ni'hao", "你好", 3),
        row("ni'hao", "你", 1),
        row("mei'you", "没有", 1),
    ];
    let metrics = evaluate(&rows, &lexicon, &lm, &scorer).expect("scoring");
    assert_eq!(metrics.rows, 3);
    assert_eq!(
        metrics.first_choice, 1,
        "only the first row is the first choice"
    );
    assert_eq!(metrics.weight_hit, 3);
    assert_eq!(metrics.weight_total, 5);
    assert_eq!(metrics.in_top_nine, 2, "你 is reachable, 没有 is not");
    assert_eq!(metrics.empty, 1, "a key nothing covers yields no candidate");
    assert_eq!(percent(metrics.first_choice, metrics.rows), 100.0 / 3.0);
    assert_eq!(percent(0, 0), 0.0);
    grid_search(&rows, &lexicon, &lm).expect("the grid search runs");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_generate_holdout_writes_rows_that_avoid_the_evaluation_set() {
    let dir = scratch("holdout");
    let holdout = dir.join("lm_holdout.tsv");
    let corpus = PathBuf::from(DEFAULT_CORPUS);
    run(holdout_args(holdout.clone(), corpus)).expect("generating the holdout set");

    let written = fs::read_to_string(&holdout).expect("reading the holdout");
    let rows: Vec<&str> = written
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(rows.len(), 5_200, "the requested number of rows is written");
    // `run` resolves a relative default against the repository root, while a test binary
    // runs with the working directory set to the crate, so the golden set is read through
    // the same resolver instead of through the raw relative constant.
    let root = resolve_root(None).expect("resolving the repository root");
    let golden = load_rows(&root.join(DEFAULT_EVAL)).expect("reading the golden set");
    let golden_words: BTreeSet<&str> = golden.iter().map(|row| row.word.as_str()).collect();
    let golden_keys: BTreeSet<&str> = golden.iter().map(|row| row.key.as_str()).collect();
    for row in &rows {
        let mut columns = row.split('\t');
        let key = columns.next().unwrap_or_default();
        let word = columns.next().unwrap_or_default();
        assert_eq!(
            columns.next(),
            None,
            "a holdout row has exactly two columns"
        );
        assert!(!key.is_empty() && !word.is_empty());
        assert!(!golden_words.contains(word), "{word} is in the golden set");
        assert!(!golden_keys.contains(key), "{key} is in the golden set");
    }

    // A corpus the floor cannot be met from must fail and write nothing.
    fs::remove_file(&holdout).expect("clearing the previous run");
    let tiny = dir.join("tiny.tsv");
    fs::write(&tiny, "你好\t5\n中国\t4\n").expect("writing the tiny corpus");
    let failure = run(holdout_args(holdout.clone(), tiny)).expect_err("must fail");
    assert!(failure.to_string().contains("tiny.tsv"), "{failure}");
    assert!(!holdout.exists(), "a failed run must write nothing");
    fs::remove_dir_all(&dir).expect("cleaning up");
}
