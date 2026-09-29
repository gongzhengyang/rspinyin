//! Unit tests for the phrase table.
//!
//! Everything here is in memory: a document is a string literal and a table is built
//! from it, so no file, no clock, no environment and no dictionary take part. The
//! candidate lists the injection is tested against are built from literals for the
//! same reason -- what the injection does with a list is a property of the list, not
//! of where the list came from.

use ime_types::{Candidate, CandidateSource, ImeError};

use super::{
    MAX_PHRASE_KEY_LEN, MAX_PHRASE_TEXT_LEN, PHRASE_ANNOTATION, PhraseTable,
    inject_phrase_candidates,
};

/// Builds a table from a document whose every row must be usable.
fn table_of(text: &str) -> PhraseTable {
    let parsed = PhraseTable::parse(text, 100);
    assert!(parsed.is_ok(), "the document must parse");
    parsed.unwrap_or_default()
}

/// One dictionary candidate, for the lists the injection is tested against.
///
/// `score` is the same for every one of them, so a phrase that ends up ahead of the
/// list cannot have got there by scoring higher.
fn dictionary(text: &str, index: u16, syllables: u16) -> Candidate {
    Candidate {
        index,
        text: String::from(text),
        annotation: None,
        source: CandidateSource::Dict,
        score: 0.5,
        consumed_syllables: syllables,
    }
}

/// The text of the first candidate of a list, or an empty string when there is none.
fn first_text(candidates: &[Candidate]) -> &str {
    candidates.first().map_or("", |held| held.text.as_str())
}

#[test]
fn test_empty_table_matches_nothing() {
    let table = PhraseTable::empty();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert!(table.longest_match("rq", 0).is_none());
    assert!(table.longest_match("", 0).is_none());
    assert_eq!(PhraseTable::default().len(), table.len());
}

#[test]
fn test_longest_match_prefers_the_longer_key() {
    let table = table_of("rq\t2026-09-29\nrqq\t2026-09-29 12:00\n");
    let long = table.longest_match("rqq", 0);
    assert_eq!(long.map(|hit| table.text(hit)), Some("2026-09-29 12:00"));
    // A key that is not defined is not a match, even though a shorter prefix of the
    // input is: the walk falls back to the longest key that is really there.
    let short = table.longest_match("rqx", 0);
    assert_eq!(short.map(|hit| table.text(hit)), Some("2026-09-29"));
    assert_eq!(short.map(|hit| hit.key_end), Some(2));
}

#[test]
fn test_longest_match_answers_none_for_an_undefined_key() {
    let table = table_of("rq\t2026-09-29\n");
    assert!(table.longest_match("dz", 0).is_none());
    assert!(table.longest_match("", 0).is_none());
    assert!(
        table.longest_match("r", 0).is_none(),
        "a prefix of a key is not a key"
    );
}

#[test]
fn test_longest_match_matches_at_the_offset_it_is_asked_about() {
    let table = table_of("rq\t2026-09-29\n");
    let hit = table.longest_match("xxrq", 2);
    assert!(hit.is_some());
    assert_eq!(hit.map(|hit| hit.key_start), Some(2));
    assert_eq!(hit.map(|hit| hit.key_end), Some(4));
    // An offset past the end of the input is answered rather than panicking, and an
    // offset whose key is not in the table answers `None`.
    assert!(table.longest_match("xxrq", 5).is_none());
    assert!(table.longest_match("xq", 1).is_none());
}

#[test]
fn test_longest_match_folds_the_document_key_and_matches_a_lower_case_input() {
    // The key is folded while the document is read; the input is matched as the
    // decoder spells it, which is lower case, so the two meet in one spelling.
    let table = table_of("RQ\t2026-09-29\n");
    assert!(table.longest_match("rq", 0).is_some());
    assert!(table.longest_match("RQ", 0).is_none());
}

#[test]
fn test_longest_match_accepts_a_key_with_a_pinned_boundary() {
    let table = table_of("ni'hao\t你好\n");
    let hit = table.longest_match("ni'hao", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("你好"));
    assert_eq!(hit.map(|hit| hit.key_end), Some(6));
}

#[test]
fn test_longest_match_finds_every_key_of_a_table_that_is_not_trivially_small() {
    // A table wide enough that the lookup has to be a real search rather than a scan
    // of one or two entries.
    let keys: Vec<String> = (b'a'..=b'z')
        .map(|letter| format!("a{}", char::from(letter)))
        .collect();
    let mut document = String::new();
    for key in &keys {
        document.push_str(key);
        document.push('\t');
        document.push_str("body-");
        document.push_str(key);
        document.push('\n');
    }
    let table = table_of(&document);
    assert_eq!(table.len(), 26);
    for key in &keys {
        let hit = table.longest_match(key, 0);
        let expected = format!("body-{key}");
        assert_eq!(
            hit.map(|hit| table.text(hit)),
            Some(expected.as_str()),
            "{key}"
        );
    }
}

#[test]
fn test_text_of_a_hit_that_names_no_entry_is_empty() {
    let first = table_of("rq\t2026-09-29\n");
    let second = table_of("ab\tx\n");
    let hit = second.longest_match("ab", 0);
    assert!(hit.is_some());
    assert_eq!(hit.map(|hit| first.text(hit)), Some(""));
}

#[test]
fn test_inject_phrase_candidates_puts_the_phrase_ahead_of_the_dictionary() {
    let table = table_of("rq\t2026-09-29\n");
    let mut candidates = vec![dictionary("人气", 1, 2), dictionary("人妻", 2, 2)];
    assert_eq!(inject_phrase_candidates(&table, "rq", &mut candidates), 1);
    assert_eq!(candidates.len(), 3);
    assert_eq!(first_text(&candidates), "2026-09-29");
    assert_eq!(candidates[0].index, 1);
    assert_eq!(candidates[0].source, CandidateSource::Phrase);
    assert_eq!(candidates[0].annotation.as_deref(), Some(PHRASE_ANNOTATION));
    // The phrase outranks the dictionary without having outscored it: the list the
    // caller handed in was already ordered, and the phrase was put in front of it.
    assert!(candidates[0].score > candidates[1].score);
    assert_eq!(candidates[1].text, "人气");
    assert_eq!(candidates[1].index, 2);
    assert_eq!(candidates[2].text, "人妻");
    assert_eq!(candidates[2].index, 3);
}

#[test]
fn test_inject_phrase_candidates_leaves_a_list_it_does_not_match_alone() {
    let table = table_of("rq\t2026-09-29\n");
    let mut candidates = vec![dictionary("人气", 1, 2)];
    assert_eq!(inject_phrase_candidates(&table, "dz", &mut candidates), 0);
    assert_eq!(candidates.len(), 1);
    assert_eq!(first_text(&candidates), "人气");
    assert_eq!(candidates[0].source, CandidateSource::Dict);
}

#[test]
fn test_inject_phrase_candidates_into_an_empty_table_injects_nothing() {
    // The degradation a missing or unreadable document leaves behind: the input
    // method keeps working on the dictionary alone.
    let mut candidates = vec![dictionary("人气", 1, 2)];
    assert_eq!(
        inject_phrase_candidates(&PhraseTable::empty(), "rq", &mut candidates),
        0
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].index, 1);
    assert_eq!(candidates[0].source, CandidateSource::Dict);
}

#[test]
fn test_inject_phrase_candidates_into_an_empty_list_still_injects() {
    // The case a shorthand exists for: `rq` has no reading, so the decode that
    // reaches this point holds nothing but the pass-through candidate or nothing at
    // all, and the phrase has to be able to stand on its own.
    let table = table_of("rq\t2026-09-29\n");
    let mut candidates = Vec::new();
    assert_eq!(inject_phrase_candidates(&table, "rq", &mut candidates), 1);
    assert_eq!(candidates.len(), 1);
    assert_eq!(first_text(&candidates), "2026-09-29");
    assert_eq!(candidates[0].consumed_syllables, 2);
}

#[test]
fn test_inject_phrase_candidates_replaces_a_candidate_that_spells_the_same_text() {
    let table = table_of("dz\tuser@example.com\n");
    let mut candidates = vec![
        dictionary("user@example.com", 1, 2),
        dictionary("肚子", 2, 2),
    ];
    assert_eq!(inject_phrase_candidates(&table, "dz", &mut candidates), 1);
    assert_eq!(candidates.len(), 2, "the text is shown once");
    assert_eq!(candidates[0].source, CandidateSource::Phrase);
    assert_eq!(candidates[1].text, "肚子");
    assert_eq!(candidates[1].index, 2);
}

#[test]
fn test_inject_phrase_candidates_counts_the_syllables_of_a_key_that_is_a_reading() {
    let table = table_of("nihao\t你好呀\n");
    let mut candidates = vec![dictionary("你好", 1, 2)];
    assert_eq!(inject_phrase_candidates(&table, "nihao", &mut candidates), 1);
    assert_eq!(candidates[0].consumed_syllables, 2);
}

#[test]
fn test_inject_phrase_candidates_counts_the_characters_of_a_shorthand() {
    // `rq` is not a sequence the syllable alphabet spells, so it consumes one unit per
    // character -- the number of units the preedit shows for an input the decoder
    // cannot segment.
    let table = table_of("rq\t2026-09-29\nni'hao\t你好\n");
    let mut shorthand = Vec::new();
    assert_eq!(inject_phrase_candidates(&table, "rq", &mut shorthand), 1);
    assert_eq!(shorthand[0].consumed_syllables, 2);

    let mut reading = Vec::new();
    assert_eq!(inject_phrase_candidates(&table, "ni'hao", &mut reading), 1);
    assert_eq!(reading[0].consumed_syllables, 2);
}

#[test]
fn test_load_skips_malformed_rows_and_counts_them() {
    // A comment and a blank line carry no entry and are not counted as skipped; the
    // three rows between the two good ones are each refused for a different reason:
    // one field instead of two, a key outside the alphabet, and a key with a dash in
    // it.
    let document = "# a comment and a blank line carry no entry\n\nrq\t2026-09-29\nbroken row\nk\u{e9}y\ttext\nhas-dash\ttext\nrqq\t2026-09-29 12:00\n";
    let (table, report) = PhraseTable::load(document, 100);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.skipped, 3);
    assert_eq!(report.overridden, 0);
    assert_eq!(report.over_limit, 0);
    assert_eq!(table.len(), 2);
    let hit = table.longest_match("rqq", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29 12:00"));
}

#[test]
fn test_parse_refuses_a_malformed_row_with_a_stable_code() {
    let cases = [
        "rq 2026-09-29\n",
        "rq\t\n",
        "rq\ttext\tmore\n",
        "\t2026-09-29\n",
        "rq-1\ttext\n",
        "\u{2026}rq\ttext\n",
    ];
    for document in cases {
        let refused = PhraseTable::parse(document, 100);
        assert!(refused.is_err(), "{document:?} must be refused");
        if let Err(error) = refused {
            let rendered = error.to_string();
            assert!(
                rendered.starts_with("config/invalid: phrases.file (line 1:"),
                "{rendered}"
            );
            assert!(
                !rendered.contains("2026-09-29"),
                "a diagnostic never quotes the user's own text: {rendered}"
            );
            assert!(matches!(error, ImeError::ConfigInvalid { .. }));
        }
    }
}

#[test]
fn test_parse_names_the_line_of_the_row_it_refuses() {
    let document = "# a comment\nrq\t2026-09-29\n\nbroken\n";
    let refused = PhraseTable::parse(document, 100);
    assert!(refused.is_err());
    if let Err(error) = refused {
        assert!(error.to_string().contains("(line 4:"), "{error}");
    }
}

#[test]
fn test_parse_refuses_a_key_longer_than_the_limit() {
    let long = "a".repeat(MAX_PHRASE_KEY_LEN + 1);
    let document = format!("{long}\t2026-09-29\n");
    let refused = PhraseTable::parse(&document, 100);
    assert!(refused.is_err(), "a key past the limit is refused");
    if let Err(error) = refused {
        assert!(error.to_string().contains("the key is longer than"), "{error}");
    }
    // The row is skipped rather than fatal on the degradation path, so the entries
    // around it survive.
    let document = format!("rq\t2026-09-29\n{long}\ttext\n");
    let (table, report) = PhraseTable::load(&document, 100);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(table.len(), 1);
}

#[test]
fn test_parse_refuses_a_phrase_longer_than_the_limit() {
    let long = "x".repeat(MAX_PHRASE_TEXT_LEN + 1);
    let document = format!("rq\t{long}\n");
    assert!(PhraseTable::parse(&document, 100).is_err());
    let (table, report) = PhraseTable::load(&document, 100);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 1);
    assert!(table.is_empty());
}

#[test]
fn test_parse_refuses_a_row_past_the_entry_limit() {
    let document = "aa\tone\nbb\ttwo\ncc\tthree\n";
    let refused = PhraseTable::parse(document, 2);
    assert!(refused.is_err());
    if let Err(error) = refused {
        assert!(error.to_string().contains("limit of 2 entries"), "{error}");
    }
    // The loader bounds the same document instead of failing it, and says how much of
    // it was dropped.
    let (table, report) = PhraseTable::load(document, 2);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.over_limit, 1);
    assert_eq!(report.skipped, 0);
    assert_eq!(table.len(), 2);
}

#[test]
fn test_load_keeps_the_last_definition_of_a_key() {
    let (table, report) = PhraseTable::load("rq\tfirst\nrq\tsecond\n", 100);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.overridden, 1);
    assert_eq!(table.len(), 1);
    let hit = table.longest_match("rq", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("second"));
}

#[test]
fn test_load_reads_a_built_in_document_before_the_user_document() {
    // The two documents are read in the order they are handed over, so the later
    // definition of a key wins: that is the whole merge rule between a built-in table
    // and the user's own.
    let built_in = "rq\tbuilt-in\ndz\tbuilt-in@example.com\n";
    let user = "rq\tuser@example.com\n";
    let (table, report) = PhraseTable::load(&format!("{built_in}{user}"), 100);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.overridden, 1);
    let rq = table.longest_match("rq", 0);
    assert_eq!(rq.map(|hit| table.text(hit)), Some("user@example.com"));
    let dz = table.longest_match("dz", 0);
    assert_eq!(dz.map(|hit| table.text(hit)), Some("built-in@example.com"));
}

#[test]
fn test_load_accepts_a_document_that_holds_no_entry() {
    let (table, report) = PhraseTable::load("", 100);
    assert_eq!(report.loaded, 0);
    assert!(table.is_empty());
    let (table, report) = PhraseTable::load("# only a comment\n", 100);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 0);
    assert!(table.is_empty());
}

#[test]
fn test_a_rebuilt_table_leaves_the_previous_snapshot_alone() {
    // A reload builds a new table and swaps the pointer the decode path holds; a
    // decode that is already running keeps the snapshot it started with, which is why
    // a rebuild cannot disturb an input session in progress.
    let first = table_of("rq\t2026-09-29\n");
    let second = table_of("rq\t2027-01-01\ndz\tuser@example.com\n");
    let held = first.longest_match("rq", 0);
    assert_eq!(held.map(|hit| first.text(hit)), Some("2026-09-29"));
    assert_eq!(first.len(), 1);
    let rebuilt = second.longest_match("rq", 0);
    assert_eq!(rebuilt.map(|hit| second.text(hit)), Some("2027-01-01"));
    assert_eq!(second.len(), 2);
}

#[test]
fn test_load_reads_a_document_written_with_carriage_returns() {
    // A document written on a machine that ends its lines with CRLF is read the same
    // way: the carriage return belongs to the line ending and not to the phrase.
    let (table, report) = PhraseTable::load("rq\t2026-09-29\r\n", 100);
    assert_eq!(report.loaded, 1);
    let hit = table.longest_match("rq", 0);
    assert_eq!(hit.map(|hit| table.text(hit)), Some("2026-09-29"));
}
