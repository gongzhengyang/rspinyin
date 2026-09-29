//! Tests for the tuner: folding, ranking, measuring, the holdout generator and the
//! weight grid.
//!
//! The folding, ranking and measuring tests run against a lexicon built from a literal
//! word list in a scratch directory, so nothing here depends on a compiled dictionary.
//! The generator tests read the repository's own L1 table through the same root resolver
//! `run` uses, because what they check -- that a word's reading is attributed to the layer
//! that has to serve it -- is a property of the tables the project ships. The corpus those
//! tests draw from is synthetic: a set large enough to fill four frequency bands is not
//! something a test should read out of `data/`, and the bands are what they check.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ime_core::lm::{InMemoryLm, Scorer};
use ime_core::segment::lookup;

use crate::dictc::quality::ReadingSource;

use super::eval::{evaluate, percent, rank_first_words};
use super::grid::grid_search;
use super::holdout::{Candidate, STRATA, Stratum, attribute, is_han, select_stratified};
use super::io::{
    Row, baseline_key, fold_reading, has_polyphone, load_l1, load_l1_readings, load_rows,
};
use super::lexicon::{TsvLexicon, build_lexicon, build_lm};
use super::{
    DEFAULT_CORPUS, DEFAULT_EVAL, DEFAULT_L1, MIN_HOLDOUT_ROWS, TuneArgs, resolve_root, run,
};

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-tune-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Writes `text` into `dir` as `name` and returns its path.
fn table(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).expect("writing the fixture");
    path
}

/// The repository root, which is where the real L1 table lives.
fn repo_root() -> PathBuf {
    resolve_root(None).expect("resolving the repository root")
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

/// A holdout run over the repository's own corpus, tables and tuning set, with the
/// output redirected into `holdout`.
///
/// The requested size is the production default, which asks for more rows than the floor
/// so that the top band's cap still leaves the set above it.
fn holdout_args(holdout: PathBuf, corpus: PathBuf) -> TuneArgs {
    TuneArgs {
        root: None,
        l1: PathBuf::from(DEFAULT_L1),
        corpus,
        eval: PathBuf::from(DEFAULT_EVAL),
        holdout,
        polyphone: PathBuf::from(crate::dictc::DEFAULT_POLYPHONE),
        gen_holdout: true,
        holdout_rows: 8_000,
        grid: false,
    }
}

/// A holdout run over a scratch root, with the real L1 table and everything else local.
fn scratch_args(dir: &Path, corpus: &str) -> TuneArgs {
    TuneArgs {
        root: Some(dir.to_path_buf()),
        // Absolute, so `root.join` leaves it alone: the L1 table is the repository's.
        l1: repo_root().join(DEFAULT_L1),
        corpus: PathBuf::from(corpus),
        eval: PathBuf::from("golden.tsv"),
        holdout: PathBuf::from("holdout.tsv"),
        polyphone: PathBuf::from("polyphone.tsv"),
        gen_holdout: true,
        holdout_rows: 6_000,
        grid: false,
    }
}

/// The Han characters the synthetic corpora are built from, in table order.
///
/// A character carrying several readings is moved to the front, so a word built from the
/// first two entries of the pool exercises the expansion's attribution whatever order the
/// shipped L1 table happens to list its characters in.
fn han_pool(readings: &BTreeMap<char, Vec<String>>) -> Vec<char> {
    let mut pool: Vec<char> = readings
        .keys()
        .copied()
        .filter(|character| is_han(*character))
        .take(300)
        .collect();
    let polyphone = pool
        .iter()
        .position(|character| has_polyphone(&character.to_string(), readings))
        .expect("the L1 table holds a polyphone among its first three hundred characters");
    pool.swap(0, polyphone);
    pool
}

/// Writes a synthetic corpus of `count` two-character words with strictly descending
/// counts, optionally with its lines in reverse order.
///
/// Reversing the lines is the whole point of the flag: the words and their counts are the
/// same either way, so a generator that ranks from the counts has to produce the same set
/// from both files, and one that ranks from the file order cannot.
fn synthetic_corpus(dir: &Path, name: &str, pool: &[char], count: usize, reversed: bool) {
    let mut words: Vec<String> = Vec::with_capacity(count);
    'outer: for left in pool {
        for right in pool {
            if left == right {
                continue;
            }
            words.push(format!("{left}{right}"));
            if words.len() == count {
                break 'outer;
            }
        }
    }
    assert_eq!(
        words.len(),
        count,
        "the pool is too small for {count} words"
    );
    let mut lines: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(index, word)| format!("{word}\t{}", count - index))
        .collect();
    if reversed {
        lines.reverse();
    }
    fs::write(dir.join(name), format!("{}\n", lines.join("\n")))
        .expect("writing the synthetic corpus");
}

/// Candidates spread evenly across `bands`, `per_band` of them in each.
fn banded(bands: &[Stratum], per_band: u32) -> Vec<Candidate<'static>> {
    let mut candidates = Vec::new();
    for band in bands {
        for offset in 0..per_band {
            let rank = band.first.saturating_add(offset);
            if rank > band.last {
                break;
            }
            candidates.push(Candidate {
                word: "汉",
                rank,
                key: "han".to_owned(),
            });
        }
    }
    candidates
}

/// The data lines of `text`, split into their columns.
fn columns(text: &str) -> Vec<Vec<&str>> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').collect())
        .collect()
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
fn test_load_l1_readings_keeps_every_reading_and_drops_a_repeat() {
    let dir = scratch("l1");
    let path = table(
        &dir,
        "l1.tsv",
        "行\txíng,háng,háng\n人\trén\n\nunusable-line\n",
    );
    let readings = load_l1_readings(&path).expect("reading the table");
    // The second reading survives, and a source that lists one reading twice does not
    // turn a single-reading character into a polyphone.
    assert_eq!(
        readings.get(&'行'),
        Some(&vec!["xing".to_owned(), "hang".to_owned()])
    );
    assert_eq!(readings.get(&'人'), Some(&vec!["ren".to_owned()]));
    // The line with no reading column is counted, not invented.
    assert_eq!(readings.len(), 2);
    let first = load_l1(&path).expect("reading the table");
    assert_eq!(first.get(&'行').map(String::as_str), Some("xing"));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_has_polyphone_separates_a_forced_reading_from_a_guessed_one() {
    let readings = BTreeMap::from([
        ('行', vec!["xing".to_owned(), "hang".to_owned()]),
        ('人', vec!["ren".to_owned()]),
    ]);
    assert!(has_polyphone("行人", &readings), "one polyphone is enough");
    assert!(has_polyphone("行", &readings));
    assert!(!has_polyphone("人", &readings));
    assert!(!has_polyphone("", &readings));
    // A character the table does not cover contributes no reading at all.
    assert!(!has_polyphone("龘", &readings));
}

#[test]
fn test_load_rows_reads_a_weight_or_a_layer_in_the_third_column() {
    let dir = scratch("rows");
    let weighted = table(&dir, "weighted.tsv", "ni'hao\t你好\t7\n");
    let rows = load_rows(&weighted).expect("a weight column");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].weight, 7);
    // A held-out row writes the layer there, and the ranking does not weight by layer.
    let layered = table(&dir, "layered.tsv", "ni'hao\t你好\tL3b\n");
    let rows = load_rows(&layered).expect("a layer column");
    assert_eq!(rows[0].weight, 1);
    // A column that is neither is refused rather than silently dropped.
    let bad = table(&dir, "bad.tsv", "ni'hao\t你好\tL9\n");
    let failure = load_rows(&bad).expect_err("not a weight and not a layer");
    assert!(
        failure
            .to_string()
            .contains("neither a weight nor a reading layer"),
        "{failure}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
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
    assert_eq!(metrics.first_rate(), 100.0 / 3.0);
    assert_eq!(metrics.reach_rate(), 200.0 / 3.0);
    assert_eq!(metrics.weighted_rate(), 60.0);
    assert_eq!(percent(0, 0), 0.0);
    grid_search(&rows, &lexicon, &lm).expect("the grid search runs");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_select_stratified_keeps_an_equal_quota_from_every_band() {
    let candidates = banded(&STRATA, 1_200);
    let kept = select_stratified(&candidates, 1_000).expect("every band is full");
    assert_eq!(kept.len(), 4_000, "a thousand from each band");
    for band in STRATA {
        let in_band = kept
            .iter()
            .filter(|candidate| band.contains(candidate.rank))
            .count();
        assert_eq!(in_band, 1_000, "band {}", band.label);
    }
    // A band keeps its most frequent words, and the whole selection stays in rank order.
    assert!(
        kept.windows(2).all(|pair| pair[0].rank < pair[1].rank),
        "rank order"
    );
    // The top band is a thousand words wide, so a quota past that takes every word it has
    // and no more, while the bands below it fill the larger quota.
    let kept = select_stratified(&candidates, 5_000).expect("a quota past the top band");
    assert_eq!(
        kept.len(),
        1_000 + 1_200 * 3,
        "the top band caps the selection"
    );
}

#[test]
fn test_select_stratified_refuses_a_corpus_with_a_band_missing() {
    // A corpus that stops after the second band has no third band to measure.
    let head = banded(&STRATA[..2], 1_200);
    let short = select_stratified(&head, 1_000).expect_err("no tail");
    assert_eq!(short.band, "10k-50k");
    // An empty corpus is a shortfall, not a panic.
    let short = select_stratified(&[], 1_000).expect_err("nothing at all");
    assert_eq!(short.band, "top 1k");
}

#[test]
fn test_attribute_reads_the_layer_from_the_tables() {
    let corrections = BTreeMap::from([("银行".to_owned(), "yin'hang".to_owned())]);
    // The table states a reading the baseline does not: the corrected key is what serves
    // the word, and the row is measured under it.
    assert_eq!(
        attribute("银行", "yin'xing".to_owned(), false, &corrections),
        ("yin'hang".to_owned(), ReadingSource::L3c)
    );
    // A row that restates the baseline is a confirmation, not a correction.
    assert_eq!(
        attribute("银行", "yin'hang".to_owned(), true, &corrections),
        ("yin'hang".to_owned(), ReadingSource::L1)
    );
    // A word the table does not state, whose characters force one reading.
    assert_eq!(
        attribute("中国", "zhong'guo".to_owned(), false, &corrections),
        ("zhong'guo".to_owned(), ReadingSource::L1)
    );
    // A word the table does not state, with a character that carries several readings.
    assert_eq!(
        attribute("行人", "xing'ren".to_owned(), true, &corrections),
        ("xing'ren".to_owned(), ReadingSource::L3b)
    );
}

#[test]
fn test_generate_holdout_writes_rows_that_avoid_the_evaluation_set() {
    let dir = scratch("holdout");
    let holdout = dir.join("lm_holdout.tsv");
    let corpus = PathBuf::from(DEFAULT_CORPUS);
    run(holdout_args(holdout.clone(), corpus)).expect("generating the holdout set");

    let written = fs::read_to_string(&holdout).expect("reading the holdout");
    let rows = columns(&written);
    // The requested eight thousand rows divide into four quotas of two thousand, and the
    // top band contributes every word of the head the tuning set does not already cover,
    // so the set lands between the floor and the requested size.
    assert!(
        rows.len() >= MIN_HOLDOUT_ROWS,
        "the set reaches the floor: {}",
        rows.len()
    );
    assert!(
        rows.len() <= 2_000 * 4,
        "no band can pass its quota: {}",
        rows.len()
    );
    // `run` resolves a relative default against the repository root, while a test binary
    // runs with the working directory set to the crate, so the tuning set is read through
    // the same resolver instead of through the raw relative constant.
    let golden = load_rows(&repo_root().join(DEFAULT_EVAL)).expect("reading the tuning set");
    let golden_words: BTreeSet<&str> = golden.iter().map(|row| row.word.as_str()).collect();
    let golden_keys: BTreeSet<&str> = golden.iter().map(|row| row.key.as_str()).collect();
    for row in &rows {
        assert_eq!(row.len(), 3, "a holdout row has three columns: {row:?}");
        let (key, word, source) = (row[0], row[1], row[2]);
        assert!(!key.is_empty() && !word.is_empty());
        assert!(
            ReadingSource::parse(source).is_some(),
            "{source} is a reading layer"
        );
        assert!(!golden_words.contains(word), "{word} is in the tuning set");
        assert!(!golden_keys.contains(key), "{key} is in the tuning set");
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

#[test]
fn test_generate_holdout_is_stratified_attributed_and_reproducible() {
    let dir = scratch("holdout-synthetic");
    let readings = load_l1_readings(&repo_root().join(DEFAULT_L1)).expect("the L1 table");
    let pool = han_pool(&readings);
    assert!(
        pool.len() >= 300,
        "the L1 table covers the pool: {}",
        pool.len()
    );

    // The pool starts with a character that carries several readings, so the first word
    // built from it is an expansion case and the second is the corrected one.
    let corrected = format!("{}{}", pool[0], pool[1]);
    table(&dir, "golden.tsv", "# an empty tuning set\n");
    table(&dir, "polyphone.tsv", &format!("{corrected}\tqiu'tie\n"));
    synthetic_corpus(&dir, "corpus.tsv", &pool, 60_000, false);
    run(scratch_args(&dir, "corpus.tsv")).expect("generating the holdout set");

    let written = fs::read_to_string(dir.join("holdout.tsv")).expect("reading the holdout");
    let rows = columns(&written);
    // The top band is a thousand words wide, so it contributes all of them; the other
    // three fill the fifteen-hundred-row quota the requested size divides into.
    assert_eq!(rows.len(), 1_000 + 1_500 * 3, "the stratified selection");
    for row in &rows {
        assert_eq!(row.len(), 3);
        assert!(ReadingSource::parse(row[2]).is_some(), "{}", row[2]);
    }
    // The corrected word is served under the corrected key and not under its baseline.
    let keys: Vec<&str> = rows
        .iter()
        .filter(|row| row[1] == corrected)
        .map(|row| row[0])
        .collect();
    assert_eq!(keys, vec!["qiu'tie"], "the correction decides the key");
    // Every layer is represented, which is what makes the report's breakdown readable.
    for source in ReadingSource::ALL {
        assert!(
            rows.iter().any(|row| row[2] == source.as_str()),
            "{source:?} is represented in the set"
        );
    }

    // The same corpus with its lines reversed produces the same file: the ranking comes
    // from the counts, not from the order the corpus happens to be written in.
    synthetic_corpus(&dir, "reversed.tsv", &pool, 60_000, true);
    run(scratch_args(&dir, "reversed.tsv")).expect("generating the holdout set again");
    let again = fs::read_to_string(dir.join("holdout.tsv")).expect("reading the holdout");
    assert_eq!(again, written, "the same corpus produces the same set");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_generate_holdout_refuses_to_measure_the_set_it_tunes_on() {
    let dir = scratch("holdout-same");
    let mut args = scratch_args(&dir, "corpus.tsv");
    args.eval = args.holdout.clone();
    let failure = run(args).expect_err("the same file for both sets");
    assert!(
        failure
            .to_string()
            .contains("tuned on the set it is measured on"),
        "{failure}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}
