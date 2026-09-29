//! Unit tests for the phrase table and for the writer that produces its documents.
//!
//! Everything here is in memory: a document is a string literal and a table is built
//! from it, so no file, no clock, no environment and no dictionary take part. The
//! candidate lists the injection is tested against are built from literals for the
//! same reason -- what the injection does with a list is a property of the list, not
//! of where the list came from.
//!
//! The writer's tests hold to the same rule and add one: what the writer produces is
//! handed straight to the reader, because a document the writer builds is only correct if
//! the reader accepts it and reads back the entry that was written.

use ime_types::{Candidate, CandidateSource, ImeError};

use super::{
    MAX_PHRASE_KEY_LEN, MAX_PHRASE_TEXT_LEN, PHRASE_ANNOTATION, PhraseTable, append_phrase,
    inject_phrase_candidates, phrase_row, replace_phrase,
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
    assert_eq!(
        inject_phrase_candidates(&table, "nihao", &mut candidates),
        1
    );
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
        assert!(
            error.to_string().contains("the key is longer than"),
            "{error}"
        );
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

// ── The writer ───────────────────────────────────────────────────────────────────
//
// Everything below is still in memory: a document is a string literal, and what the
// writer produces is handed straight to the reader. That round trip is what these tests
// exist to assert -- a document the writer produces is one the reader accepts, and reads
// back as the entry that was written -- so the assertions name the reader's answer and
// never the writer's own bytes alone.

/// Asserts that a document the writer produced reads back as the entries it was built
/// from, and that it holds nothing else.
///
/// The reader is asked through [`PhraseTable::parse`], the strict entry point, so a
/// document with a single unusable row fails the assertion instead of quietly losing it.
fn assert_round_trip(document: &str, entries: &[(&str, &str)]) {
    let parsed = PhraseTable::parse(document, 1000);
    assert!(
        parsed.is_ok(),
        "the writer must produce a document the reader accepts"
    );
    let table = parsed.unwrap_or_default();
    for (key, text) in entries {
        let hit = table.longest_match(key, 0);
        assert_eq!(
            hit.map(|hit| table.text(hit)),
            Some(*text),
            "the reader must read {key:?} back as the body it was written with"
        );
    }
    assert_eq!(
        table.len(),
        u64::try_from(entries.len()).unwrap_or(u64::MAX),
        "the document must hold exactly the entries it was built from"
    );
}

#[test]
fn test_phrase_row_renders_a_row_the_reader_reads_back() {
    let row = phrase_row("rq", "2026-09-29").unwrap_or_default();
    assert_eq!(row, "rq\t2026-09-29\n");
    assert_round_trip(&row, &[("rq", "2026-09-29")]);
}

#[test]
fn test_phrase_row_folds_the_key_the_reader_folds() {
    // The row the writer renders and the input the reader matches meet in one spelling:
    // the key is folded on the way in, exactly as it is folded while a document is read.
    let row = phrase_row("RQ", "2026-09-29").unwrap_or_default();
    assert_eq!(row, "rq\t2026-09-29\n");
    assert_round_trip(&row, &[("rq", "2026-09-29")]);
}

#[test]
fn test_phrase_row_accepts_an_entry_exactly_at_the_limits() {
    // Both bounds are inclusive on the read side, so the writer must accept them too:
    // a key of exactly the limit and a body of exactly the limit are usable entries.
    let key = "a".repeat(MAX_PHRASE_KEY_LEN);
    let text = "x".repeat(MAX_PHRASE_TEXT_LEN);
    let row = phrase_row(&key, &text).unwrap_or_default();
    assert_round_trip(&row, &[(key.as_str(), text.as_str())]);
}

#[test]
fn test_phrase_row_refuses_a_key_the_reader_would_refuse() {
    // A digit, a dash, a space, a tab, an empty key and a non-ASCII byte are each outside
    // the key alphabet, which is what a shortcut is typed in.
    let cases = ["", "r q", "rq-1", "rq1", "rq\tx", "rq\nx", "\u{e9}rq"];
    for key in cases {
        let refused = phrase_row(key, "2026-09-29");
        assert!(refused.is_err(), "{key:?} must be refused");
        if let Err(error) = refused {
            let rendered = error.to_string();
            let expected = "config/invalid: phrases.file (";
            assert!(rendered.starts_with(expected), "{rendered}");
            let quotes_the_entry = rendered.contains("2026-09-29");
            assert!(!quotes_the_entry, "a diagnostic never quotes it");
            assert!(matches!(error, ImeError::ConfigInvalid { .. }));
        }
    }
}

#[test]
fn test_phrase_row_refuses_a_key_past_the_limit() {
    let key = "a".repeat(MAX_PHRASE_KEY_LEN + 1);
    let refused = phrase_row(&key, "text");
    assert!(refused.is_err(), "a key past the limit is refused");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.contains("the key is longer than"), "{rendered}");
    }
}

#[test]
fn test_phrase_row_refuses_a_body_the_reader_would_refuse() {
    let long = "x".repeat(MAX_PHRASE_TEXT_LEN + 1);
    assert!(phrase_row("rq", "").is_err(), "an empty phrase is refused");
    let refused = phrase_row("rq", &long);
    assert!(refused.is_err(), "a phrase past the limit is refused");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.contains("the phrase is longer than"), "{rendered}");
    }
}

#[test]
fn test_phrase_row_refuses_a_body_that_is_not_one_line() {
    // A body holding a tab would be read as a third field and one holding a line break as
    // a second row, so neither is an entry this writer may write; a carriage return would
    // be eaten as part of a line ending and the phrase would come back short.
    for text in ["a\tb", "a\nb", "a\rb", "a\r"] {
        let refused = phrase_row("rq", text);
        assert!(refused.is_err(), "{text:?} must be refused");
        if let Err(error) = refused {
            let rendered = error.to_string();
            let reason = "a tab or a line break";
            assert!(rendered.contains(reason), "{rendered}");
        }
    }
}

#[test]
fn test_append_phrase_round_trips_through_the_reader() {
    let document = append_phrase("", "rq", "2026-09-29").unwrap_or_default();
    assert_eq!(document, "rq\t2026-09-29\n");
    let document = append_phrase(&document, "dz", "user@example.com").unwrap_or_default();
    assert_eq!(document, "rq\t2026-09-29\ndz\tuser@example.com\n");
    let entries = [("rq", "2026-09-29"), ("dz", "user@example.com")];
    assert_round_trip(&document, &entries);
}

#[test]
fn test_append_phrase_keeps_what_the_document_already_held() {
    // The comments and the blank lines a user wrote are not the writer's to remove: the
    // row is added to the document, not written over it.
    let original = "# my shortcuts\n\nrq\t2026-09-29\n";
    let document = append_phrase(original, "dz", "user@example.com").unwrap_or_default();
    let expected = "# my shortcuts\n\nrq\t2026-09-29\ndz\tuser@example.com\n";
    assert_eq!(document, expected);
    let entries = [("rq", "2026-09-29"), ("dz", "user@example.com")];
    assert_round_trip(&document, &entries);
}

#[test]
fn test_append_phrase_terminates_a_last_line_that_has_no_line_ending() {
    // A document a tool wrote may end without a newline. Appending straight onto it would
    // splice two rows into one, which is a row neither the reader nor the caller knows.
    let document = append_phrase("rq\t2026-09-29", "dz", "user@example.com").unwrap_or_default();
    assert_eq!(document, "rq\t2026-09-29\ndz\tuser@example.com\n");
    let entries = [("rq", "2026-09-29"), ("dz", "user@example.com")];
    assert_round_trip(&document, &entries);
}

#[test]
fn test_append_phrase_of_a_key_the_document_holds_makes_the_new_row_win() {
    // The reader keeps the last definition of a key, which is what makes an append the
    // right way to override one -- and what lets a user's document override a built-in
    // one read before it.
    let document = append_phrase("rq\t2026-09-29\n", "rq", "2027-01-01").unwrap_or_default();
    assert_eq!(document, "rq\t2026-09-29\nrq\t2027-01-01\n");
    assert_round_trip(&document, &[("rq", "2027-01-01")]);
}

#[test]
fn test_append_phrase_refuses_an_entry_and_leaves_the_document_alone() {
    let document = "rq\t2026-09-29\n";
    assert!(append_phrase(document, "r q", "text").is_err());
    // The caller's document is untouched: the function returns a new one, so a refused
    // entry cannot leave a half-written document behind.
    assert_eq!(document, "rq\t2026-09-29\n");
}

#[test]
fn test_replace_phrase_rewrites_the_row_where_it_stood() {
    let original = "# a comment\nrq\told\ndz\tkept\n";
    let document = replace_phrase(original, "rq", "new").unwrap_or_default();
    assert_eq!(document, "# a comment\nrq\tnew\ndz\tkept\n");
    assert_round_trip(&document, &[("rq", "new"), ("dz", "kept")]);
}

#[test]
fn test_replace_phrase_drops_a_later_definition_of_the_same_key() {
    // Leaving both rows would leave the document saying two things about one key, and the
    // later one would shadow the entry that was just written.
    let original = "rq\tfirst\nother\tkept\nrq\tsecond\n";
    let document = replace_phrase(original, "rq", "third").unwrap_or_default();
    assert_eq!(document, "rq\tthird\nother\tkept\n");
    assert_round_trip(&document, &[("rq", "third"), ("other", "kept")]);
}

#[test]
fn test_replace_phrase_appends_a_key_the_document_does_not_hold() {
    let document = replace_phrase("# a comment\n", "rq", "2026-09-29").unwrap_or_default();
    assert_eq!(document, "# a comment\nrq\t2026-09-29\n");
    assert_round_trip(&document, &[("rq", "2026-09-29")]);
}

#[test]
fn test_replace_phrase_leaves_a_row_it_cannot_parse_alone() {
    // The writer rewrites the entry it was asked about and repairs nothing else: a broken
    // row may be a mistake the user is in the middle of fixing, and dropping it would
    // lose text they typed. The reader keeps skipping exactly the two rows it skipped
    // before the rewrite.
    let original = "broken row\nrq\told\nhas-dash\ttext\n";
    let document = replace_phrase(original, "rq", "new").unwrap_or_default();
    assert_eq!(document, "broken row\nrq\tnew\nhas-dash\ttext\n");
    let (table, report) = PhraseTable::load(&document, 100);
    assert_eq!(report.loaded, 1);
    assert_eq!(report.skipped, 2);
    assert_eq!(table.len(), 1);
}

#[test]
fn test_replace_phrase_matches_a_row_the_reader_folds() {
    // The row is found the way the reader reads it: a hand-typed row in upper case, or
    // one whose key is padded by spaces, is the row of that key.
    let document = replace_phrase("RQ\told\n", "rq", "new").unwrap_or_default();
    assert_eq!(document, "rq\tnew\n");
    let document = replace_phrase(" rq \told\n", "rq", "new").unwrap_or_default();
    assert_eq!(document, "rq\tnew\n");
}

#[test]
fn test_replace_phrase_keeps_a_document_that_ends_without_a_line_ending() {
    // A document whose last line has no line ending is left as it is when its key is not
    // the one being replaced, and terminated when the replacement is appended to it.
    let document = replace_phrase("other\tkept", "rq", "new").unwrap_or_default();
    assert_eq!(document, "other\tkept\nrq\tnew\n");
    assert_round_trip(&document, &[("other", "kept"), ("rq", "new")]);
}
