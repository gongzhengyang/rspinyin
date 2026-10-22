//! The symbol table's tests: document reading, lookup shape and degradation.

use ime_types::ImeError;

use super::{MAX_SYMBOLS_PER_CATEGORY, SYMBOLS_FILE_KEY, SymbolTable};

#[test]
fn test_parse_reads_categories_and_answers_the_code() {
    let table = SymbolTable::parse("1\t数字\t０１２\nx\t箭头\t←↑→\n").expect("rows are usable");
    assert_eq!(table.len(), 2);
    let digits = table.longest_prefix("1").expect("the digit code answers");
    assert_eq!(digits.label, "数字");
    assert_eq!(digits.symbols, &['０', '１', '２']);
    let arrows = table.longest_prefix("x").expect("the arrow code answers");
    assert_eq!(arrows.label, "箭头");
    assert_eq!(arrows.symbols, &['←', '↑', '→']);
}

#[test]
fn test_lookup_matches_the_longest_prefix_and_ignores_the_rest() {
    // A code longer than the prefix is served by the prefix that names it: `1x`
    // asks for the digits and the `x` is not the panel's business.
    let table = SymbolTable::parse("1\t数字\t０１\nym\t声调\tāá\n").expect("rows are usable");
    let hit = table.longest_prefix("1x").expect("the longest prefix wins");
    assert_eq!(hit.label, "数字");
    assert!(table.longest_prefix("zhishi").is_none());
    assert!(table.longest_prefix("").is_none());
}

#[test]
fn test_parse_skips_blank_lines_and_comments() {
    let table = SymbolTable::parse("# the panel\n\n1\t数字\t０１\n").expect("rows are usable");
    assert_eq!(table.len(), 1);
}

#[test]
fn test_parse_names_the_line_of_the_first_bad_row() {
    let refused = SymbolTable::parse("1\t数字\t０１\n1\t数字\t０１\textra\n");
    let error = refused.expect_err("the fourth column is a fault");
    let ImeError::ConfigInvalid { key, reason } = &error else {
        panic!("a document fault is a ConfigInvalid");
    };
    assert_eq!(key, SYMBOLS_FILE_KEY);
    assert!(
        reason.contains("line 2"),
        "the fault names its line: {reason}"
    );
    assert!(!reason.contains('０'), "the reason never quotes the row");
}

#[test]
fn test_parse_refuses_an_out_of_alphabet_or_oversized_field() {
    // The code alphabet is a-z and 0-9; a hanzi code is not a keystroke after `v`.
    assert!(SymbolTable::parse("数\t数字\t０１\n").is_err());
    assert!(SymbolTable::parse("\t数字\t０１\n").is_err());
    assert!(SymbolTable::parse("1\t\t０１\n").is_err());
    assert!(SymbolTable::parse("1\t数字\t\n").is_err());
    let long = "a".repeat(5);
    assert!(SymbolTable::parse(&format!("{long}\t数字\t０１\n")).is_err());
    let many = "０".repeat(MAX_SYMBOLS_PER_CATEGORY + 1);
    assert!(SymbolTable::parse(&format!("1\t数字\t{many}\n")).is_err());
}

#[test]
fn test_load_skips_unusable_rows_and_counts_them() {
    let (table, report) = SymbolTable::load("# built-in\n1\t数字\t０１\nbroken\n\nx\t箭头\t←\n");
    assert_eq!(report.loaded, 2);
    assert_eq!(report.skipped, 1);
    assert!(table.longest_prefix("x").is_some());
}

#[test]
fn test_load_lets_a_later_row_override_an_earlier_code() {
    let (table, report) = SymbolTable::load("1\t数字\t０１\n1\t数目\t０\n");
    assert_eq!(report.loaded, 1);
    assert_eq!(report.overridden, 1);
    let hit = table
        .longest_prefix("1")
        .expect("the code survived the override");
    assert_eq!(hit.label, "数目");
    assert_eq!(hit.symbols, &['０']);
}

#[test]
fn test_code_is_folded_to_lower_case_before_it_is_keyed() {
    // A document a hand edited may spell the code in upper case; the canonical
    // form is the lower-case one the match folds the input to.
    let (table, _) = SymbolTable::load("X\t箭头\t←\n");
    assert!(table.longest_prefix("x").is_some());
}

#[test]
fn test_empty_table_names_nothing() {
    let table = SymbolTable::empty();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert!(table.longest_prefix("1").is_none());
}
