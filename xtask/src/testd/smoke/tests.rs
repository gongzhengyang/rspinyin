//! Unit tests for the smoke baseline: the parser, the expectation forms, and the
//! evaluation over a real compiled container.
//!
//! The container is built through the compiler's own `compile` rather than assembled by
//! hand, so an evaluation that passes here passes against what `dictc` writes. The
//! parser and the expectation forms are tested over text, where the failure modes live.
//! Every clock a test hands a run is a [`Clock::Fixed`] reading: no test reads the wall
//! clock, and a `@today` row answers the instant the test names.

use std::fs;

use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;
use ime_types::{Candidate, CandidateSource};

use super::{
    CORE_QUOTA, Clock, ExpectKind, SmokeCase, derive, evaluate, expectation, mount, parse_cases,
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

/// Writes a small English container into `dir` and answers its path.
///
/// Two words whose key and text are the same ASCII spelling, which is what the
/// English entries look like in production.
fn write_english_container(dir: &std::path::Path) -> std::path::PathBuf {
    let words = vec![word("github", "github", 713), word("hello", "hello", 700)];
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
    .expect("compiling the English fixture");
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
    let path = dir.join("eng.dict");
    writer.finish(&path).expect("writing the English fixture");
    path
}

/// The clock every evaluation in this file pins: 2026-10-28 12:34:56 UTC, the instant
/// the quick module's own tests render against.
fn pinned_clock() -> Clock {
    Clock::Fixed {
        epoch_secs: 1_793_190_896,
    }
}

/// A case with one tag, the shape every assertion below starts from.
fn case(id: &str, keys: &str, expect: ExpectKind) -> SmokeCase {
    SmokeCase {
        id: id.to_owned(),
        keys: keys.to_owned(),
        expect: expectation_for(expect),
        flags: super::v2::RowFlags::default(),
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
        ExpectKind::Top3(items) => {
            expectation(&format!("@top3:{}", items.join("|"))).expect("a top3 expectation")
        }
        ExpectKind::Today(fmt) => {
            expectation(&format!("@today:{fmt}")).expect("a today expectation")
        }
        ExpectKind::Empty => expectation("@empty").expect("an empty expectation"),
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
fn test_parse_cases_keeps_every_v1_row_shape_and_refuses_a_malformed_one() {
    // The v1 shapes, exactly as the first baseline wrote them: bare, prefixed and
    // tagged. The v2 column is additive, so all of these must parse to the same
    // cases they always did.
    let set = parse_cases(
        "a\tni\tni\nb\tni'hao\t@first:你好\tcore\nc\txian\t@contains:西安\tambiguity\nd\tzhegn\t@regex:^占\n",
    )
    .expect("the v1 shapes parse");
    assert_eq!(set.len(), 4);
    assert_eq!(set[0].expect.raw, "ni");
    assert_eq!(set[1].tags, ["core"]);
    assert_eq!(set[2].tags, ["ambiguity"]);
    assert_eq!(set[3].tags.len(), 0, "the tag column stays optional in v1");
    for row in &set {
        assert_eq!(
            row.flags,
            super::v2::RowFlags::default(),
            "a v1 row carries no flags"
        );
    }

    let error = parse_cases("base_001\tnihao\n").expect_err("two columns are too few");
    assert!(error.to_string().contains("3"), "{error}");
    let error = parse_cases("a\tni\tni\na\two\two\n").expect_err("the id repeats");
    assert!(error.to_string().contains("twice"), "{error}");
    let error = parse_cases("a\tNi\t你\n").expect_err("uppercase is not a pinyin key");
    // The line context wraps the cause, so the assertion reads the whole chain.
    assert!(format!("{error:#}").contains("pinyin key"), "{error:#}");
}

#[test]
fn test_parse_cases_reads_the_v2_flags_column() {
    let set = parse_cases(
        "q\trq\t@today:%Y-%m-%d\tflags:quick\tquick-matrix\n\
         a\tnihao\t@contains:你好\tflags:ABBREV\tabbrev-matrix\n\
         s\tnihao <commit> ni <commit>\t@first:你好\tflags:assoc\tassoc-matrix\n\
         t\tnihao <commit> ni <commit>\t@empty\tflags:assoc-off\tassoc-matrix\n",
    )
    .expect("the v2 rows parse");
    assert!(set[0].flags.quick, "the quick row carries its switch");
    assert_eq!(
        set[0].tags,
        ["quick-matrix"],
        "tags follow the flags column"
    );
    assert!(set[1].flags.decode.contains(ime_types::DecodeFlags::ABBREV));
    assert!(set[2].flags.assoc && set[2].flags.assoc_enabled);
    assert_eq!(set[2].keys, "nihao <commit> ni <commit>");
    assert!(set[3].flags.assoc && !set[3].flags.assoc_enabled);
}

#[test]
fn test_parse_cases_refuses_v2_rows_that_cannot_mean_what_they_say() {
    let error = parse_cases("a\trq\t@today:%Y-%m-%d\n")
        .expect_err("@today without the quick clock is unanswerable");
    assert!(format!("{error:#}").contains("flags:quick"), "{error:#}");
    let error = parse_cases("a\tnihao <commit>\t@empty\n")
        .expect_err("@empty without an association row is unanswerable");
    assert!(
        format!("{error:#}").contains("association row"),
        "{error:#}"
    );
    let error = parse_cases("a\tsj\t@first:sj\tflags:quick\n")
        .expect_err("an unshipped trigger is refused");
    assert!(format!("{error:#}").contains("prefix table"), "{error:#}");
    let error = parse_cases("a\tzhongguo <commit>\t@first:中\tflags:assoc,ABBREV\n")
        .expect_err("an association row reads no decode flags");
    assert!(
        format!("{error:#}").contains("shipped configuration"),
        "{error:#}"
    );
    let error = parse_cases("a\tnihao\t@first:你好\tflags:NO_SUCH_BIT\n")
        .expect_err("an unknown flag is refused");
    assert!(format!("{error:#}").contains("unknown flag"), "{error:#}");
}

#[test]
fn test_expectation_reads_the_six_forms_and_refuses_an_unknown_one() {
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
    assert!(matches!(
        expectation("@top3:你好|拟好").expect("top3").kind,
        ExpectKind::Top3(_)
    ));
    assert!(matches!(
        expectation("@today:%Y-%m-%d").expect("today").kind,
        ExpectKind::Today(_)
    ));
    assert!(matches!(
        expectation("@empty").expect("empty").kind,
        ExpectKind::Empty
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
    let error = expectation("@top3:你好||拟好").expect_err("an empty item pins nothing");
    assert!(error.to_string().contains("empty item"), "{error}");
    let error = expectation("@today:%Q").expect_err("an unknown directive is refused");
    assert!(error.to_string().contains("%Q"), "{error}");
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
fn test_satisfied_orders_the_top3_window_and_pins_emptiness() {
    let page = texts(&["建国", "国家", "国", "人际"]);
    // In order, non-contiguous: the relative order is the pin, the gap is not.
    assert!(satisfied(
        &ExpectKind::Top3(vec![String::from("建国"), String::from("国家")]),
        &page
    ));
    assert!(satisfied(
        &ExpectKind::Top3(vec![String::from("建国"), String::from("国")]),
        &page
    ));
    // Order-sensitive: the same texts reversed are a different ranking.
    assert!(!satisfied(
        &ExpectKind::Top3(vec![String::from("国家"), String::from("建国")]),
        &page
    ));
    // The window is the top three: a fourth candidate answers nothing.
    assert!(!satisfied(
        &ExpectKind::Top3(vec![String::from("人际")]),
        &page
    ));
    assert!(satisfied(&ExpectKind::Empty, &[]));
    assert!(!satisfied(&ExpectKind::Empty, &page));
    // A shorter page than the window still answers what it holds.
    let two = texts(&["建国", "国家"]);
    assert!(satisfied(
        &ExpectKind::Top3(vec![String::from("建国"), String::from("国家")]),
        &two
    ));
}

#[test]
fn test_evaluate_pins_the_first_commit_and_reports_the_got() {
    let dir = scratch("evaluate");
    let dict = write_container(&dir);
    let mounted = mount(&dict, None).expect("the fixture mounts");
    let set = vec![
        case("hit", "ni'hao", ExpectKind::First(String::from("你好"))),
        case("miss", "ni", ExpectKind::First(String::from("你吗"))),
        case("page", "ni'hao", ExpectKind::Contains(String::from("拟好"))),
    ];
    let report = evaluate(&set, &mounted, pinned_clock());
    assert_eq!(report.total, 3);
    assert_eq!(report.passed, 2);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].id, "miss");
    assert_eq!(
        report.failures[0].got, "你",
        "the failure names what the engine answered"
    );
    assert_eq!(
        report.failures[0].page, "你",
        "a single-candidate page is the page"
    );
}

#[test]
fn test_evaluate_reports_the_page_for_the_membership_forms() {
    let dir = scratch("membership");
    let dict = write_container(&dir);
    let mounted = mount(&dict, None).expect("the fixture mounts");
    let set = vec![
        case(
            "rank",
            "ni'hao",
            ExpectKind::Top3(vec![String::from("拟好"), String::from("你好")]),
        ),
        case("void", "ni'hao", ExpectKind::Empty),
    ];
    let report = evaluate(&set, &mounted, pinned_clock());
    assert_eq!(
        report.failures.len(),
        2,
        "both forms fail against this page"
    );
    assert_eq!(
        report.failures[0].got, "你好/拟好",
        "the got column names the page, where the expected order went"
    );
    assert_eq!(
        report.failures[1].got, "你好/拟好",
        "an @empty failure names the page that was not empty"
    );
}

#[test]
fn test_evaluate_today_follows_the_injected_clock_and_a_skew_turns_a_pin_red() {
    let dir = scratch("today");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    // The relative form renders the run's own clock, so it holds under any injected
    // reading; the literal pin beside it holds only for the instant it names, which
    // is what makes a clock skew visible: shift the clock one day and the pin turns
    // red while the relative form keeps answering.
    fs::write(
        &cases,
        "rel\trq\t@today:%Y-%m-%d\tflags:quick\n\
         pin\trq\t@first:2026-10-28\tflags:quick\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict, None, pinned_clock()).expect("the set runs");
    assert_eq!(
        (report.passed, report.failures.len()),
        (2, 0),
        "the pinned clock answers both rows: {report:?}"
    );
    let day = 86_400;
    let shifted = Clock::Fixed {
        epoch_secs: 1_793_190_896 + day,
    };
    let report = run_cases(&cases, &dict, None, shifted).expect("the set runs");
    assert_eq!(report.failures.len(), 1, "the skewed clock breaks the pin");
    assert_eq!(report.failures[0].id, "pin");
    assert_eq!(
        report.failures[0].got, "2026-10-29",
        "the render followed the injected clock, not the wall clock"
    );
}

#[test]
fn test_run_cases_drives_the_assoc_script_over_a_real_container() {
    let dir = scratch("assoc");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    // The second commit opens the history view, whose single hit is the first
    // commit; with the feature off the same script answers the empty idle state.
    fs::write(
        &cases,
        "on\tnihao <commit> ni <commit>\t@first:你好\tflags:assoc\n\
         off\tnihao <commit> ni <commit>\t@empty\tflags:assoc-off\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict, None, pinned_clock()).expect("the set runs");
    assert_eq!(
        (report.passed, report.failures.len()),
        (2, 0),
        "both sides of the controlled comparison hold: {report:?}"
    );
}

#[test]
fn test_run_cases_drives_assoc_commits_and_pins_the_history_order() {
    let dir = scratch("assoc-order");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    // Three commits: after the third, the view's candidate set holds the earlier
    // history beside the newest commit, and the pin is the relative order of the
    // pair -- 你好 before 你 -- whatever the absolute positions turn out to be.
    fs::write(
        &cases,
        "chain\tni <commit> ni'hao <commit> ni <commit>\t@top3:你好|你\tflags:assoc\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict, None, pinned_clock()).expect("the set runs");
    assert_eq!(
        (report.passed, report.failures.len()),
        (1, 0),
        "the newest-first order holds: {report:?}"
    );
}

#[test]
fn test_run_cases_assoc_selection_lands_and_the_session_stays_usable() {
    let dir = scratch("assoc-select");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    // After a digit selection the selection's commit has landed and the machine is
    // back in a state a next composition can use: typing after the selection
    // composes and decodes exactly as before.
    fs::write(
        &cases,
        "chain\tni <commit> ni'hao <commit> <assoc-select-1> ni\t@first:你\tflags:assoc\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict, None, pinned_clock()).expect("the set runs");
    assert_eq!(
        (report.passed, report.failures.len()),
        (1, 0),
        "the selection lands and the machine keeps answering: {report:?}"
    );
}

#[test]
fn test_run_cases_refuses_an_empty_set_and_a_broken_case() {
    let dir = scratch("empty");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    fs::write(&cases, "# nothing but a header\n").expect("writing the empty set");
    assert!(
        run_cases(&cases, &dict, None, pinned_clock()).is_err(),
        "a baseline of zero cases pins nothing"
    );

    fs::write(&cases, "a\tni\n").expect("writing the broken set");
    assert!(
        run_cases(&cases, &dict, None, pinned_clock()).is_err(),
        "a set that does not parse is refused"
    );

    assert!(
        run_cases(&dir.join("absent.tsv"), &dict, None, pinned_clock()).is_err(),
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
    let report = run_cases(&cases, &dict, None, pinned_clock()).expect("the set runs");
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
fn test_derive_carries_v2_rows_through_the_rewrite_unchanged() {
    let dir = scratch("derive-v2");
    let dict = write_container(&dir);
    let cases = dir.join("cases.tsv");
    let v2_row = "mx_a\tnihao\t@contains:你好\tflags:ABBREV\tabbrev-matrix\n\
                  mx_b\tnihao <commit> ni <commit>\t@empty\tflags:assoc-off\tassoc-matrix\n";
    fs::write(&cases, v2_row).expect("handwritten v2 band");
    let holdout = dir.join("holdout.tsv");
    fs::write(&holdout, "ni\t你\nni'hao\t你好\n").expect("writing the holdout");
    derive(&cases, &holdout, &dict).expect("the derivation runs");
    let written = fs::read_to_string(&cases).expect("reading the rewritten set");
    assert!(
        written.contains("mx_a\tnihao\t@contains:你好\tflags:ABBREV\tabbrev-matrix"),
        "the v2 row's flags column survives the rewrite: {written}"
    );
    assert!(
        written.contains("mx_b\tnihao <commit> ni <commit>\t@empty\tflags:assoc-off\tassoc-matrix"),
        "the association script survives the rewrite: {written}"
    );
    let parsed = parse_cases(&written).expect("the rewritten set parses");
    assert_eq!(parsed.iter().filter(|c| c.id.starts_with("mx_")).count(), 2);
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

#[test]
fn test_english_rows_mount_the_english_lexicon_and_leave_pinyin_intact() {
    let dir = scratch("english");
    let dict = write_container(&dir);
    let english = write_english_container(&dir);
    let cases = dir.join("cases.tsv");
    // The English rows answer the word itself; the pinyin row beside them proves a
    // mounted English lexicon does not disturb the ordinary decode. The fixture
    // words are the same spellings the real English dictionary holds, so a green
    // run here is the contract the matrix band pins against the real container.
    fs::write(
        &cases,
        "en_on\tgithub\t@first:github\tflags:ENGLISH\n\
         en_off\tgithub\t@first:github\n\
         en_hello\thello\t@first:hello\tflags:ENGLISH\n\
         py\tni'hao\t@first:你好\tflags:ENGLISH\n",
    )
    .expect("writing the set");
    let report = run_cases(&cases, &dict, Some(&english), pinned_clock()).expect("the set runs");
    assert_eq!(
        (report.passed, report.failures.len()),
        (4, 0),
        "the English mount answers its rows and leaves pinyin alone: {report:?}"
    );
}

#[test]
fn test_parse_cases_refuses_an_association_script_with_a_bad_marker() {
    // The script is validated at load, not at observation: a malformed marker must
    // refuse the set, or an `@empty` row could silently pass on the emptiness of a
    // script that never ran.
    let error = parse_cases("a\tnihao <assoc-select-0>\t@empty\tflags:assoc\n")
        .expect_err("digit zero is not a marker");
    assert!(format!("{error:#}").contains("1..=9"), "{error:#}");
    let error = parse_cases("a\tnihao <grab>\t@first:你\tflags:assoc\n")
        .expect_err("an unknown marker is refused");
    assert!(
        format!("{error:#}").contains("unknown association marker"),
        "{error:#}"
    );
}
