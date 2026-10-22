//! Unit tests for the smoke baseline: the parser, the expectation forms, and the
//! evaluation over a real compiled container.
//!
//! The container is built through the compiler's own `compile` rather than assembled by
//! hand, so an evaluation that passes here passes against what `dictc` writes. The
//! parser and the expectation forms are tested over text, where the failure modes live.

use std::fs;

use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;
use ime_types::{Candidate, CandidateSource};

use super::{
    CORE_QUOTA, ExpectKind, SmokeCase, derive, evaluate, expectation, mount, parse_cases,
    run_cases, satisfied,
};
use crate::dictc::build::{Band, compile};
use crate::dictc::source::Word;

/// A scratch directory for one test, named after the test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-testd-smoke-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
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

/// Writes a small container into `dir` and answers its path.
///
/// Three words: one single, one two-syllable, and a second candidate under the same
/// two-syllable key, so a `@contains` assertion has a page to look at.
fn write_container(dir: &std::path::Path) -> std::path::PathBuf {
    let words = vec![
        word("你", "ni", 5_000),
        word("你好", "ni'hao", 5_000),
        word("拟好", "ni'hao", 4_000),
    ];
    let band = Band {
        threshold: u32::MAX,
        requested: 0,
    };
    let compiled = compile(
        &words,
        band,
        0,
        &[],
        crate::dictc::normalize::NormalizeRules::default(),
    )
    .expect("compiling the fixture");
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
    let path = dir.join("base.dict");
    writer.finish(&path).expect("writing the fixture");
    path
}

/// A case with one tag, the shape every assertion below starts from.
fn case(id: &str, keys: &str, expect: ExpectKind) -> SmokeCase {
    SmokeCase {
        id: id.to_owned(),
        keys: keys.to_owned(),
        expect: expectation_for(expect),
        tags: vec![String::from("test")],
    }
}

/// Wraps a parsed kind into the expectation the case struct holds.
fn expectation_for(kind: ExpectKind) -> super::Expectation {
    match kind {
        ExpectKind::First(text) => expectation(&text).expect("a first expectation"),
        ExpectKind::Contains(text) => {
            expectation(&format!("@contains:{text}")).expect("a contains expectation")
        }
        ExpectKind::Regex(_) => expectation("@regex:^你").expect("a regex expectation"),
    }
}

/// The texts `satisfied` reads, built from plain strings.
fn texts(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn test_parse_cases_reads_comments_blank_lines_and_tag_columns() {
    let set = parse_cases(
        "# header\n\namb_001\txian\t@contains:西安\tambiguity extra\nbase_001\tnihao\t你好\n",
    )
    .expect("a legal set");
    assert_eq!(set.len(), 2);
    assert_eq!(set[0].id, "amb_001");
    assert_eq!(set[0].keys, "xian");
    assert_eq!(set[0].tags, ["ambiguity", "extra"]);
    assert_eq!(set[1].tags.len(), 0, "the tag column is optional");
    assert_eq!(set[1].expect.raw, "你好");
}

#[test]
fn test_parse_cases_refuses_a_malformed_row_naming_the_line() {
    let error = parse_cases("base_001\tnihao\n").expect_err("two columns are too few");
    assert!(error.to_string().contains("3"), "{error}");
}

#[test]
fn test_parse_cases_refuses_a_repeated_id_and_a_foreign_key() {
    let error = parse_cases("a\tni\tni\na\two\two\n").expect_err("the id repeats");
    assert!(error.to_string().contains("twice"), "{error}");
    let error = parse_cases("a\tNi\t你\n").expect_err("uppercase is not a pinyin key");
    // The line context wraps the cause, so the assertion reads the whole chain.
    assert!(format!("{error:#}").contains("pinyin key"), "{error:#}");
}

#[test]
fn test_expectation_reads_the_three_forms_and_refuses_an_unknown_one() {
    assert!(matches!(
        expectation("@first:你好").expect("first").kind,
        ExpectKind::First(_)
    ));
    assert!(matches!(
        expectation("@contains:西安").expect("contains").kind,
        ExpectKind::Contains(_)
    ));
    assert!(matches!(
        expectation("@regex:^你").expect("regex").kind,
        ExpectKind::Regex(_)
    ));
    // A bare column is the literal spelling of @first.
    assert!(matches!(
        expectation("你好").expect("literal").kind,
        ExpectKind::First(_)
    ));
    let error = expectation("@fist:你好").expect_err("a misspelled prefix is refused");
    assert!(
        error.to_string().contains("unknown expectation form"),
        "{error}"
    );
    let error = expectation("@regex:(unclosed").expect_err("the pattern does not compile");
    assert!(error.to_string().contains("does not compile"), "{error}");
}

#[test]
fn test_satisfied_reads_first_contains_and_regex_against_the_page() {
    let page = texts(&["先生", "西安", "先", "县"]);
    assert!(satisfied(&ExpectKind::First(String::from("先生")), &page));
    assert!(!satisfied(&ExpectKind::First(String::from("西安")), &page));
    assert!(satisfied(
        &ExpectKind::Contains(String::from("西安")),
        &page
    ));
    assert!(!satisfied(
        &ExpectKind::Contains(String::from("现实")),
        &page
    ));
    assert!(satisfied(
        &expectation("@regex:^先").expect("regex").kind,
        &page
    ));
    // The contains scan stops at the first page: a candidate past it cannot be
    // taken without a page turn.
    let mut deep = vec![String::from("占位"); 12];
    deep[10] = String::from("含西安的答案");
    assert!(!satisfied(
        &ExpectKind::Contains(String::from("西安")),
        &deep
    ));
    assert!(satisfied(&ExpectKind::First(String::new()), &[]).eq(&false));
}

#[test]
fn test_evaluate_pins_the_first_commit_and_reports_the_got() {
    let dir = scratch("evaluate");
    let dict = write_container(&dir);
    let mounted = mount(&dict).expect("the fixture mounts");
    let set = vec![
        case("hit", "ni'hao", ExpectKind::First(String::from("你好"))),
        case("miss", "ni", ExpectKind::First(String::from("你吗"))),
        case("page", "ni'hao", ExpectKind::Contains(String::from("拟好"))),
    ];
    let report = evaluate(&set, &mounted);
    assert_eq!(report.total, 3);
    assert_eq!(report.passed, 2);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].id, "miss");
    assert_eq!(
        report.failures[0].got, "你",
        "the failure names what the engine answered"
    );
}

#[test]
fn test_run_cases_refuses_an_empty_set_and_a_broken_case() {
    let dir = scratch("empty");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    fs::write(&cases, "# nothing but a header\n").expect("writing the empty set");
    assert!(
        run_cases(&cases, &dict).is_err(),
        "a baseline of zero cases pins nothing"
    );

    fs::write(&cases, "a\tni\n").expect("writing the broken set");
    assert!(
        run_cases(&cases, &dict).is_err(),
        "a set that does not parse is refused"
    );

    assert!(
        run_cases(&dir.join("absent.tsv"), &dict).is_err(),
        "a missing case file is an exit, not a green run"
    );
}

#[test]
fn test_run_cases_answers_a_report_over_a_real_container() {
    let dir = scratch("run");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    fs::write(
        &cases,
        "hit\tni'hao\t@first:你好\npage\tni'hao\t@contains:拟好\tcore\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict).expect("the set runs");
    assert_eq!(report.total, 2);
    assert_eq!(report.passed, 2);
    assert!(report.failures.is_empty());
}

#[test]
fn test_derive_keeps_the_handwritten_bands_and_fills_the_core_quota() {
    let dir = scratch("derive");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    fs::write(&cases, "amb_001\tni'hao\t@contains:拟好\tambiguity\n").expect("handwritten band");
    let holdout = dir.join("holdout.tsv");
    // The first two rows the engine answers correctly; the third names a text the
    // fixture cannot produce, and the fourth repeats the first key.
    fs::write(
        &holdout,
        "# holdout\nni\t你\nni'hao\t你好\nni\t你吗\nni\t你\n",
    )
    .expect("writing the holdout");
    let summary = derive(&cases, &holdout, &dict).expect("the derivation runs");
    assert_eq!(summary.scanned, 4);
    assert_eq!(
        summary.derived, 2,
        "the third row is not answered, the fourth is a repeat"
    );
    assert_eq!(summary.kept, 1, "the handwritten case survives the rewrite");

    let written = fs::read_to_string(&cases).expect("reading the rewritten set");
    let parsed = parse_cases(&written).expect("the rewritten set parses");
    assert!(
        parsed.iter().any(|c| c.id == "amb_001"),
        "the handwritten id is still there"
    );
    let core: Vec<&SmokeCase> = parsed
        .iter()
        .filter(|c| c.tags.iter().any(|t| t == "core"))
        .collect();
    assert_eq!(core.len(), 2);
    assert_eq!(core[0].id, "core_001");
    assert_eq!(core[0].expect.raw, "@first:你");
}

#[test]
fn test_derive_stops_at_the_quota_the_card_states() {
    let dir = scratch("quota");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    let holdout = dir.join("holdout.tsv");
    // One answerable row, repeated far past the quota: deduplication makes it one
    // derived case, which is also what the assert reads.
    let mut rows = String::from("# holdout\n");
    for _ in 0..(CORE_QUOTA + 10) {
        rows.push_str("ni\t你\n");
    }
    fs::write(&holdout, rows).expect("writing the holdout");
    let summary = derive(&cases, &holdout, &dict).expect("the derivation runs");
    assert_eq!(summary.derived, 1, "repeats deduplicate to one case");
}

/// The candidate type the engine hands back, built once so a future field addition
/// surfaces here as a compile error rather than a silent default.
#[test]
fn test_candidate_shape_matches_what_the_expectations_read() {
    let candidate = Candidate {
        index: 1,
        text: String::from("你"),
        annotation: None,
        source: CandidateSource::Dict,
        score: 0.0,
        consumed_syllables: 1,
    };
    assert_eq!(candidate.text, "你");
}
