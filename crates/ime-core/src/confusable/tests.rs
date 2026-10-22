//! The confusable table's tests: document reading, lookup shape and degradation.

use ime_types::ImeError;

use super::{
    CONFUSABLE_ANNOTATION_PREFIX, CONFUSABLE_FILE_KEY, ConfusableTable, MAX_CONFUSABLE_LEN,
};

#[test]
fn test_parse_reads_pairs_and_answers_the_correct_form() {
    let table =
        ConfusableTable::parse("按奈\t按捺\n迫不急待\t迫不及待\n").expect("rows are usable");
    assert_eq!(table.len(), 2);
    assert_eq!(table.lookup("按奈"), Some("按捺"));
    assert_eq!(table.lookup("迫不急待"), Some("迫不及待"));
}

#[test]
fn test_lookup_matches_the_whole_text_and_no_substring() {
    let table = ConfusableTable::parse("一如继往\t一如既往\n").expect("rows are usable");
    // The wrong form matches in full and only in full: a candidate that merely holds
    // it as a part, or the correct form itself, answers nothing.
    assert_eq!(table.lookup("一如继往"), Some("一如既往"));
    assert_eq!(table.lookup("一如既往"), None);
    assert_eq!(table.lookup("一如继"), None);
    assert_eq!(table.lookup("一如继往了"), None);
    assert_eq!(table.lookup(""), None);
}

#[test]
fn test_parse_skips_blank_lines_and_comments() {
    let table = ConfusableTable::parse("# a comment\n\n按奈\t按捺\n").expect("rows are usable");
    assert_eq!(table.len(), 1);
}

#[test]
fn test_parse_names_the_line_of_the_first_bad_row() {
    // Two columns only: a third field is a row this table cannot carry, and the
    // answer names where reading stopped. The reason never quotes the row.
    let refused = ConfusableTable::parse("按奈\t按捺\n按奈\t按捺\textra\n");
    let error = refused.expect_err("the third column is a fault");
    let ImeError::ConfigInvalid { key, reason } = &error else {
        panic!("a document fault is a ConfigInvalid");
    };
    assert_eq!(key, CONFUSABLE_FILE_KEY);
    assert!(
        reason.contains("line 2"),
        "the fault names its line: {reason}"
    );
    assert!(!reason.contains("按奈"), "the reason never quotes the row");
}

#[test]
fn test_parse_refuses_an_empty_or_oversized_column() {
    assert!(ConfusableTable::parse("\t按捺\n").is_err());
    assert!(ConfusableTable::parse("按奈\t\n").is_err());
    let long = "a".repeat(MAX_CONFUSABLE_LEN + 1);
    assert!(ConfusableTable::parse(&format!("{long}\t按捺\n")).is_err());
    // At the bound exactly the row is usable.
    let bound = "一".repeat(MAX_CONFUSABLE_LEN / 3);
    assert!(ConfusableTable::parse(&format!("{bound}\t按捺\n")).is_ok());
}

#[test]
fn test_load_skips_unusable_rows_and_counts_them() {
    let (table, report) =
        ConfusableTable::load("# built-in\n按奈\t按捺\nbroken\n\n谈笑风声\t谈笑风生\n");
    assert_eq!(report.loaded, 2);
    assert_eq!(report.skipped, 1);
    assert_eq!(table.lookup("谈笑风声"), Some("谈笑风生"));
}

#[test]
fn test_load_lets_a_later_row_override_an_earlier_key() {
    let (table, report) = ConfusableTable::load("按奈\t按捺\n按奈\t按纳\n");
    assert_eq!(report.loaded, 1);
    assert_eq!(report.overridden, 1);
    assert_eq!(table.lookup("按奈"), Some("按纳"));
}

#[test]
fn test_empty_table_matches_nothing_and_holds_nothing() {
    let table = ConfusableTable::empty();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert_eq!(table.lookup("按奈"), None);
}

#[test]
fn test_annotation_prefix_carries_the_arrow_and_the_field_name() {
    // The hint a candidate carries is the prefix followed by the correct form; the
    // spelling is user-facing copy and is pinned here so a rewording is a decision.
    assert_eq!(CONFUSABLE_ANNOTATION_PREFIX, "字:→");
}
