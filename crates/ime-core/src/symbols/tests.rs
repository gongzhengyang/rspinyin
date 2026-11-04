//! The symbol table's tests: document reading, lookup shape and degradation.

use ime_types::ImeError;

use super::{MAX_EMOJI_PER_DOMAIN, MAX_SYMBOLS_PER_CATEGORY, SYMBOLS_FILE_KEY, SymbolTable};

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

/// A three-entry emoji document: two homophones and one distinct alias.
const EMOJI_DOCUMENT: &str = "😀\txiao\n😁\txiao\n😊\tlian\n";

#[test]
fn test_add_emoji_domain_installs_the_domain_and_answers_its_code() {
    let mut table = SymbolTable::empty();
    let report = table.add_emoji_domain(EMOJI_DOCUMENT);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.overridden, 0);
    // `ve` answers the whole list in file order; the label is the domain's own.
    let served = table.serve("e").expect("the code names the domain");
    assert_eq!(served.label, "表情");
    assert_eq!(served.symbols.as_slice(), &['😀', '😁', '😊']);
}

#[test]
fn test_serve_filters_the_emoji_domain_by_alias_prefix() {
    // The suffix typed past the domain's code narrows the list to the entries
    // whose alias starts with it; the kept entries stay in file order.
    let mut table = SymbolTable::empty();
    let _ = table.add_emoji_domain(EMOJI_DOCUMENT);
    let served = table.serve("ex").expect("the code names the domain");
    assert_eq!(
        served.symbols.as_slice(),
        &['😀', '😁'],
        "the entries whose alias starts with the suffix, in file order"
    );
    let served = table
        .serve("exiao")
        .expect("the alias names the homophones");
    assert_eq!(
        served.symbols.as_slice(),
        &['😀', '😁'],
        "homophones stay side by side in file order"
    );
    let served = table
        .serve("elian")
        .expect("the alias names the third entry");
    assert_eq!(served.symbols.as_slice(), &['😊']);
}

#[test]
fn test_serve_answers_the_whole_list_for_a_category_without_aliases() {
    // A plain category serves its list whole, and a suffix typed past its
    // prefix is ignored exactly as the table documents -- the emoji filter is
    // the alias-carrying category's behaviour alone.
    let table = SymbolTable::parse("1\t数字\t０１\n").expect("rows are usable");
    let served = table.serve("1x").expect("the longest prefix wins");
    assert_eq!(served.label, "数字");
    assert_eq!(served.symbols.as_slice(), &['０', '１']);
}

#[test]
fn test_serve_answers_an_empty_list_when_no_alias_matches() {
    // A filter that keeps nothing is an empty list on an open panel, not a
    // fall-through: the code still names the domain, and the user narrows it
    // further or backs up.
    let mut table = SymbolTable::empty();
    let _ = table.add_emoji_domain(EMOJI_DOCUMENT);
    let served = table.serve("ezz").expect("the code names the domain");
    assert!(served.symbols.as_slice().is_empty());
}

#[test]
fn test_add_emoji_domain_counts_skipped_rows() {
    // Rows the reader refused are counted and the walk continues: a multi-code-
    // point character, an upper-case alias, an empty alias and a stray fourth
    // column are each one skip, and the usable rows still serve.
    let mut table = SymbolTable::empty();
    let report =
        table.add_emoji_domain("😀\txiao\n👍🏽\tzan\n😁\tZAN\n😊\t\n😂\txiao\textra\n🤣\tpeng\n");
    assert_eq!(report.loaded, 1);
    assert_eq!(report.skipped, 4);
    assert_eq!(
        table.serve("e").expect("served").symbols.as_slice(),
        &['😀', '🤣']
    );
}

#[test]
fn test_add_emoji_domain_with_nothing_usable_installs_nothing() {
    let mut table = SymbolTable::empty();
    let report = table.add_emoji_domain("# only a comment\nbroken row\n");
    assert_eq!(report.loaded, 0);
    assert!(table.serve("e").is_none(), "no domain was installed");
    assert!(table.longest_prefix("e").is_none());
}

#[test]
fn test_add_emoji_domain_overrides_an_existing_category_of_the_code() {
    // The document the plain reader holds may already define the emoji domain's
    // code; the domain replaces it whole, the same last-definition-wins rule a
    // later line of the plain document follows.
    let mut table = SymbolTable::parse("e\t编程\t⌨\n1\t数字\t０\n").expect("rows are usable");
    let report = table.add_emoji_domain("😀\txiao\n");
    assert_eq!(report.overridden, 1);
    assert_eq!(table.len(), 2, "the category kept its position");
    let served = table.serve("e").expect("the code names the domain");
    assert_eq!(served.label, "表情");
    assert_eq!(served.symbols.as_slice(), &['😀']);
}

#[test]
fn test_add_emoji_domain_refuses_rows_past_the_domain_bound() {
    // The bound is what keeps a file that is not an emoji document from being
    // loaded as one; the extra rows are skips, not a truncated domain.
    let rows: String = (0..MAX_EMOJI_PER_DOMAIN + 3)
        .map(|_| "😀\txiao\n")
        .collect();
    let mut table = SymbolTable::empty();
    let report = table.add_emoji_domain(&rows);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.skipped, 3);
    let served = table.serve("e").expect("served");
    assert_eq!(
        served.symbols.as_slice().len(),
        MAX_EMOJI_PER_DOMAIN,
        "the domain holds exactly the bound"
    );
}

#[test]
fn test_domains_lists_codes_and_labels_in_table_order() {
    // The index page is one candidate per domain in exactly this order, which
    // is what makes a selection's position name the domain it selects.
    let mut table = SymbolTable::parse("1\t数字\t０\nx\t箭头\t←\n").expect("rows are usable");
    let _ = table.add_emoji_domain("😀\txiao\n");
    let domains: Vec<(String, String)> = table
        .domains()
        .map(|domain| (String::from(domain.code), String::from(domain.label)))
        .collect();
    assert_eq!(
        domains,
        [
            (String::from("1"), String::from("数字")),
            (String::from("x"), String::from("箭头")),
            (String::from("e"), String::from("表情")),
        ],
        "the document's order, with the emoji domain last"
    );
}

#[test]
fn test_domains_of_an_empty_table_answer_nothing() {
    let table = SymbolTable::empty();
    assert_eq!(table.domains().count(), 0);
}
