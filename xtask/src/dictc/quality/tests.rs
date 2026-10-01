//! Tests for the quality closure.
//!
//! Every fixture is written to a scratch directory of its own under the system temp
//! directory, so nothing here reads the real `data/` tree: the lexicon is a double the
//! test states outright, and the single end-to-end case builds its container inside the
//! scratch directory through the compiler's own `compile`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;
use ime_types::{ImeError, SyllableId, WordFlags, WordIter, WordRef};

use crate::dictc::build::{Band, compile};
use crate::dictc::source::Word;

use super::*;

/// One correction row in each of the three column counts the format has had.
const TWO_COLUMNS: &str = "银行\tyin'hang\n";

/// The three-column form: the same row with a weight.
const THREE_COLUMNS: &str = "银行\tyin'hang\t60000\n";

/// The four-column form: the same row with a weight and an origin.
const FOUR_COLUMNS: &str = "银行\tyin'hang\t60000\tunihan\n";

/// The held-out rows the end-to-end test measures: two keys, one per layer.
const HOLDOUT: &str = "zhongguo\t中国\tL1\nyinhang\t银行\tL3c\n";

/// Two annotated correction rows, which is the floor the end-to-end test states.
const CORRECTIONS: &str = "银行\tyin'hang\t60000\tunihan\n银行\tyin'xing\t1000\tmanual\n";

/// One annotated correction row, which is below the floor of two.
const ONE_CORRECTION: &str = "银行\tyin'hang\t60000\tunihan\n";

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-qual-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Writes `rows` into `dir` as a TSV and returns its path.
fn table(dir: &Path, name: &str, rows: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, rows).expect("writing the fixture");
    path
}

/// One held-out case over an already-canonical key.
fn case(key: &str, expected: &str, source: ReadingSource) -> HoldoutCase {
    HoldoutCase {
        key: key.to_owned(),
        expected: expected.to_owned(),
        source,
    }
}

/// One correction row carrying `origin`.
fn correction(origin: Option<Origin>) -> Correction {
    Correction {
        word: "银行".to_owned(),
        reading: "yin'hang".to_owned(),
        weight: None,
        origin,
    }
}

/// The argument set the end-to-end tests run with, over a scratch root.
///
/// Both case and correction floors sit at the two rows the fixtures hold rather than at
/// the production values: what the last tests are about is the tool's refusal behaviour,
/// and each one raises the bound it is testing. The ceiling stays at the production value,
/// so raising a floor past it is a test of the bounds check and not of the table.
fn quality_args(root: &Path) -> QualityArgs {
    QualityArgs {
        root: Some(root.to_path_buf()),
        dict: PathBuf::from("base.dict"),
        holdout: PathBuf::from("holdout.tsv"),
        polyphone: PathBuf::from("polyphone.tsv"),
        min_corrections: 2,
        max_corrections: MAX_CORRECTIONS,
        min_cases: 2,
    }
}

/// Borrows `texts` as ranked word handles: the first word is the heaviest, so the order
/// a test wrote is the order the measurement reads.
fn rank(texts: &[String]) -> Vec<WordRef<'_>> {
    texts
        .iter()
        .enumerate()
        .map(|(rank, text)| WordRef {
            text: text.as_str(),
            weight: 1_000u32.saturating_sub(rank as u32),
            syl_count: 1,
            flags: WordFlags::empty(),
        })
        .collect()
}

/// A lexicon over literal keys, so a test states the candidate order it measures.
struct MockLexicon {
    /// Words per key, in the order the key serves them.
    by_key: BTreeMap<String, Vec<String>>,
}

impl MockLexicon {
    /// Builds a lexicon from `(key, words)` pairs, first word ranked first.
    fn new(rows: Vec<(&str, Vec<&str>)>) -> Self {
        let mut by_key = BTreeMap::new();
        for (key, words) in rows {
            let words = words.into_iter().map(str::to_owned).collect();
            by_key.insert(key.to_owned(), words);
        }
        Self { by_key }
    }
}

impl Lexicon for MockLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        let words = match self.by_key.get(key) {
            Some(texts) => rank(texts),
            None => Vec::new(),
        };
        Ok(WordIter::from_vec(words))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }
}

/// A lexicon whose every query fails, so the error path is exercised.
struct BrokenLexicon;

impl Lexicon for BrokenLexicon {
    fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }
}

/// One compiled word with no readings, so nothing expands.
fn word(text: &str, key: &str, weight: u32) -> Word {
    Word {
        text: text.to_owned(),
        key: key.to_owned(),
        readings: Vec::new(),
        weight,
        flags: 0,
        polyphone: false,
    }
}

/// Writes a two-word container into `dir`, so `run` measures a real artifact.
///
/// The container is built through the compiler's own `compile` rather than assembled by
/// hand: the point of the test is that the measurement reads what `dictc` writes, and a
/// hand-built section would prove only that the reader agrees with the test.
fn write_container(dir: &Path) {
    let words = vec![
        word("中国", "zhong'guo", 5_000),
        word("银行", "yin'hang", 4_000),
    ];
    let band = Band {
        threshold: u32::MAX,
        requested: 0,
    };
    let compiled = compile(&words, band, 0, &[]).expect("compiling the fixture");
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, compiled.fst)
        .expect("fst");
    writer
        .add_section(SectionKind::Entries, compiled.entries)
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, compiled.strpool)
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, compiled.unigram)
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, compiled.wordlist)
        .expect("wordlist");
    writer
        .finish(&dir.join("base.dict"))
        .expect("writing the fixture");
}

#[test]
fn test_key_of_segments_typed_pinyin_into_the_engine_cut() {
    assert_eq!(key_of("nihao").as_deref(), Some("ni'hao"));
    assert_eq!(key_of("yinhang").as_deref(), Some("yin'hang"));
    assert_eq!(key_of("chongqing").as_deref(), Some("chong'qing"));
    assert_eq!(key_of("wo").as_deref(), Some("wo"));
    // A column that states its own boundaries keeps them.
    assert_eq!(key_of("ni'hao").as_deref(), Some("ni'hao"));
}

#[test]
fn test_key_of_refuses_input_with_no_segmentation() {
    assert_eq!(key_of("xyz"), None);
    assert_eq!(key_of(""), None);
}

#[test]
fn test_load_holdout_reads_the_three_columns_and_defaults_the_layer() {
    let dir = scratch("holdout");
    let rows = "# comment\n\nnihao\t你好\tL3b\nyinhang\t银行\tL3c\nwo\t我\n";
    let cases = load_holdout(&table(&dir, "holdout.tsv", rows)).expect("reading the set");
    assert_eq!(cases.len(), 3);
    assert_eq!(
        cases[0],
        HoldoutCase {
            key: "ni'hao".to_owned(),
            expected: "你好".to_owned(),
            source: ReadingSource::L3b,
        }
    );
    assert_eq!(cases[1].source, ReadingSource::L3c);
    // A row with no layer column is a baseline row, which is what the generator writes.
    assert_eq!(cases[2].source, ReadingSource::L1);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_holdout_refuses_a_row_it_cannot_measure() {
    let dir = scratch("holdout-bad");
    let rows = [
        ("missing-column", "nihao\n", "expected-word"),
        ("unknown-layer", "nihao\t你好\tL9\n", "L3b or L3c"),
        ("unsegmentable", "xyz\t我\tL1\n", "segment"),
        ("empty", "# nothing but a comment\n", "no rows"),
    ];
    for (tag, text, needle) in rows {
        let path = table(&dir, &format!("{tag}.tsv"), text);
        let failure = load_holdout(&path).expect_err(tag);
        assert!(failure.to_string().contains(needle), "{tag}: {failure}");
    }
    let absent = load_holdout(&dir.join("absent.tsv")).expect_err("a set never written");
    assert!(absent.to_string().contains("cannot read"));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_measure_counts_the_first_choice_and_the_page() {
    let lexicon = MockLexicon::new(vec![
        ("ni'hao", vec!["你好", "尼号"]),
        ("yin'hang", vec!["银行", "印行"]),
    ]);
    let cases = vec![
        case("ni'hao", "你好", ReadingSource::L1),
        case("yin'hang", "印行", ReadingSource::L3b),
    ];
    let report = measure(&cases, &lexicon).expect("measuring");
    assert_eq!(report.top1_rate, 0.5);
    assert_eq!(report.top9_rate, 1.0);
    assert_eq!(report.unreachable, 0);
    // Every case is served, so the two first-choice rates agree.
    assert_eq!(report.served_top1_rate, report.top1_rate);
}

#[test]
fn test_measure_reports_the_ordering_apart_from_the_coverage() {
    let lexicon = MockLexicon::new(vec![("ni'hao", vec!["你好", "尼号"])]);
    let cases = vec![
        case("ni'hao", "你好", ReadingSource::L1),
        case("mei'you", "没有", ReadingSource::L1),
    ];
    let report = measure(&cases, &lexicon).expect("measuring");
    // The word the dictionary does not hold counts against the rate over every case ...
    assert_eq!(report.top1_rate, 0.5);
    assert_eq!(report.unreachable, 1);
    // ... and not against the rate over the cases the key serves.
    assert_eq!(report.served_top1_rate, 1.0);
}

#[test]
fn test_measure_counts_the_last_slot_of_the_page() {
    let mut words = vec!["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛"];
    words.push("目标");
    let lexicon = MockLexicon::new(vec![("mu'biao", words)]);
    let cases = vec![case("mu'biao", "目标", ReadingSource::L1)];
    let report = measure(&cases, &lexicon).expect("measuring");
    assert_eq!(report.top1_rate, 0.0);
    // The ninth candidate is the last one the reachability metric looks at.
    assert_eq!(report.top9_rate, 1.0);
    assert_eq!(report.unreachable, 0);
}

#[test]
fn test_measure_does_not_count_a_word_ranked_past_the_page_as_missing() {
    let mut words = vec!["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬"];
    words.push("目标");
    let lexicon = MockLexicon::new(vec![("mu'biao", words)]);
    let cases = vec![case("mu'biao", "目标", ReadingSource::L1)];
    let report = measure(&cases, &lexicon).expect("measuring");
    assert_eq!(report.top1_rate, 0.0);
    assert_eq!(report.top9_rate, 0.0);
    // The word is served, it is just ranked past the page, so it is not missing.
    assert_eq!(report.unreachable, 0);
}

#[test]
fn test_measure_of_an_empty_set_reports_zero() {
    let report = measure(&[], &MockLexicon::new(Vec::new())).expect("measuring nothing");
    assert_eq!(report, QualityReport::default());
}

#[test]
fn test_measure_reports_a_lookup_failure() {
    let cases = vec![case("ni'hao", "你好", ReadingSource::L1)];
    let failure = measure(&cases, &BrokenLexicon).expect_err("a failing lookup");
    assert!(failure.to_string().contains("dict/unsupported"));
}

#[test]
fn test_reading_source_parses_its_three_spellings() {
    for source in ReadingSource::ALL {
        assert_eq!(ReadingSource::parse(source.as_str()), Some(source));
    }
    // The column is written by a generator, so a near miss is a defect, not a fold.
    assert_eq!(ReadingSource::parse("l3b"), None);
    assert_eq!(ReadingSource::parse(""), None);
}

#[test]
fn test_origin_parses_its_three_spellings() {
    assert_eq!(Origin::parse("unihan"), Some(Origin::Unihan));
    assert_eq!(Origin::parse("manual"), Some(Origin::Manual));
    assert_eq!(Origin::parse("corpus"), Some(Origin::Corpus));
    // The column is exact, and a commercial source is not one of the three values.
    assert_eq!(Origin::parse("Unihan"), None);
    assert_eq!(Origin::parse("sogou"), None);
    assert_eq!(Origin::parse(""), None);
}

#[test]
fn test_load_corrections_accepts_two_three_and_four_columns() {
    let dir = scratch("corrections");
    let two = table(&dir, "two.tsv", TWO_COLUMNS);
    let three = table(&dir, "three.tsv", THREE_COLUMNS);
    let four = table(&dir, "four.tsv", FOUR_COLUMNS);
    let two = load_corrections(&two).expect("reading the two-column form");
    let three = load_corrections(&three).expect("reading the three-column form");
    let four = load_corrections(&four).expect("reading the four-column form");
    assert_eq!(two.len(), 1);
    assert_eq!(three.len(), 1);
    assert_eq!(four.len(), 1);
    // The word and the reading read the same way whatever the column count, which is
    // what lets the origin column be added without a flag day.
    assert_eq!(two[0].word, three[0].word);
    assert_eq!(three[0].word, four[0].word);
    assert_eq!(two[0].reading, four[0].reading);
    assert_eq!(two[0].weight, None);
    assert_eq!(two[0].origin, None);
    assert_eq!(three[0].weight, Some(60_000));
    assert_eq!(three[0].origin, None);
    assert_eq!(four[0].weight, Some(60_000));
    assert_eq!(four[0].origin, Some(Origin::Unihan));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_corrections_refuses_a_row_it_cannot_trust() {
    let dir = scratch("corrections-bad");
    let rows = [
        (
            "bad-origin",
            "银行\tyin'hang\t60000\tsogou\n",
            "unihan, manual, corpus",
        ),
        (
            "bad-weight",
            "银行\tyin'hang\tmany\tunihan\n",
            "is not a weight",
        ),
        (
            "extra-column",
            "银行\tyin'hang\t60000\tunihan\tnote\n",
            "2 to 4 columns",
        ),
        ("empty-reading", "银行\t\t60000\n", "reading column"),
        ("one-column", "银行\n", "2 to 4 columns"),
    ];
    for (tag, text, needle) in rows {
        let path = table(&dir, &format!("{tag}.tsv"), text);
        let failure = load_corrections(&path).expect_err(tag);
        assert!(failure.to_string().contains(needle), "{tag}: {failure}");
    }
    let absent = load_corrections(&dir.join("absent.tsv")).expect_err("a table never written");
    assert!(absent.to_string().contains("dict/source/missing"));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_origin_mix_counts_each_origin_and_the_unannotated_rows() {
    let rows = vec![
        correction(Some(Origin::Unihan)),
        correction(Some(Origin::Unihan)),
        correction(Some(Origin::Manual)),
        correction(Some(Origin::Corpus)),
        correction(None),
    ];
    let mix = OriginMix::of(&rows);
    assert_eq!(mix.total(), 5);
    assert_eq!(mix.count(Origin::Unihan), 2);
    assert_eq!(mix.count(Origin::Manual), 1);
    assert_eq!(mix.count(Origin::Corpus), 1);
    assert_eq!(mix.unspecified, 1);
    let mut empty = OriginMix::default();
    assert_eq!(empty.total(), 0);
    empty.record(Some(Origin::Corpus));
    assert_eq!(empty.count(Origin::Corpus), 1);
}

/// The bounds the audit tests judge against: the production floor and ceiling.
const BOUNDS: Bounds = Bounds {
    floor: MIN_CORRECTIONS,
    ceiling: MAX_CORRECTIONS,
};

#[test]
fn test_audit_refuses_a_table_below_the_floor_and_one_without_origins() {
    let path = Path::new("data/raw/polyphone.tsv");
    let short = OriginMix {
        unihan: 10,
        ..OriginMix::default()
    };
    let failure = audit(path, &short, 0, BOUNDS).expect_err("a short table");
    assert!(
        failure
            .to_string()
            .contains(&format!("{} are required", MIN_CORRECTIONS))
    );
    let full = OriginMix {
        unihan: MIN_CORRECTIONS as u32,
        ..OriginMix::default()
    };
    audit(path, &full, 0, BOUNDS).expect("a table at the floor, fully annotated");
    let unannotated = OriginMix {
        unihan: MIN_CORRECTIONS as u32 - 1,
        unspecified: 1,
        ..OriginMix::default()
    };
    let failure = audit(path, &unannotated, 0, BOUNDS).expect_err("a bare row");
    assert!(failure.to_string().contains("no `origin` column"));
}

#[test]
fn test_audit_refuses_a_table_past_the_ceiling_and_one_with_repeats() {
    let path = Path::new("data/raw/polyphone.tsv");
    let over = OriginMix {
        unihan: 20_001,
        ..OriginMix::default()
    };
    let failure = audit(path, &over, 0, BOUNDS).expect_err("a table past the ceiling");
    assert!(
        failure.to_string().contains("past the 20000-row ceiling"),
        "{failure}"
    );
    let at = OriginMix {
        unihan: 20_000,
        ..OriginMix::default()
    };
    audit(path, &at, 0, BOUNDS).expect("a table at the ceiling, which is inside it");
    let failure = audit(path, &at, 1, BOUNDS).expect_err("a repeated pair");
    assert!(failure.to_string().contains("repeat"), "{failure}");
}

#[test]
fn test_count_repeats_counts_a_pair_stated_twice_and_not_two_readings() {
    let rows = vec![
        correction(Some(Origin::Unihan)),
        correction(Some(Origin::Unihan)),
        // The same word under a second reading is not a repeat: that is what the table
        // is for, and 银行 has two readings a user can mean.
        Correction {
            reading: "yin'xing".to_owned(),
            ..correction(Some(Origin::Manual))
        },
    ];
    assert_eq!(count_repeats(&rows), 1);
    assert_eq!(count_repeats(&[]), 0);
}

#[test]
fn test_run_measures_a_compiled_container_and_audits_the_correction_table() {
    let dir = scratch("run");
    write_container(&dir);
    table(&dir, "holdout.tsv", HOLDOUT);
    table(&dir, "polyphone.tsv", CORRECTIONS);
    run(quality_args(&dir)).expect("measuring a two-word container");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_run_refuses_a_container_that_was_never_built() {
    let dir = scratch("run-missing");
    table(&dir, "holdout.tsv", HOLDOUT);
    table(&dir, "polyphone.tsv", CORRECTIONS);
    let failure = run(quality_args(&dir)).expect_err("no container");
    assert!(failure.to_string().contains("xtask dictc"));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_run_refuses_a_held_out_set_below_the_case_floor() {
    let dir = scratch("run-cases");
    write_container(&dir);
    table(&dir, "holdout.tsv", HOLDOUT);
    table(&dir, "polyphone.tsv", CORRECTIONS);
    let mut args = quality_args(&dir);
    args.min_cases = MIN_HOLDOUT_CASES;
    let failure = run(args).expect_err("two cases, floor of five thousand");
    assert!(failure.to_string().contains("before a rate"));
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_run_refuses_a_correction_table_below_the_floor() {
    let dir = scratch("run-floor");
    write_container(&dir);
    table(&dir, "holdout.tsv", HOLDOUT);
    table(&dir, "polyphone.tsv", ONE_CORRECTION);
    let mut args = quality_args(&dir);
    args.min_corrections = MIN_CORRECTIONS;
    let failure = run(args).expect_err("one row, floor of five thousand");
    assert!(
        failure
            .to_string()
            .contains("are required before its effect")
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_run_refuses_bounds_no_table_could_satisfy() {
    let dir = scratch("run-bounds");
    write_container(&dir);
    table(&dir, "holdout.tsv", HOLDOUT);
    table(&dir, "polyphone.tsv", CORRECTIONS);
    let mut args = quality_args(&dir);
    args.min_corrections = 10;
    args.max_corrections = 5;
    let failure = run(args).expect_err("a floor past the ceiling");
    assert!(
        failure.to_string().contains("is past --max-corrections"),
        "{failure}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}
