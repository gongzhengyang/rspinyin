//! The layout validator's error classes, each pinned by its own regression test.
//!
//! Responsibility: hold the per-class tests the task card's DoD asks for -- a
//! conflicting mapping, a missing table, an unreachable final, an invalid key
//! name, a spelling the syllable table does not know, and a declaration that
//! disagrees with the school it names -- asserting for each that the refusal is
//! a `config/invalid` diagnostic under [`LAYOUT_DIAGNOSTIC_KEY`] whose reason
//! names the keys and linguistic spellings involved and never quotes a table
//! entry's value. The fallback behaviour a refusal triggers lives one layer up
//! (the configuration projection) and is deliberately not re-asserted here.
//!
//! These tests sit beside `layout` instead of inside it because that file is
//! close to the 800-line limit; they exercise the public loader surface only
//! (`parse_layout`, `compile_layout`, `LayoutDocument`), never the private
//! helpers, which `layout`'s own test module covers.

use ime_types::{ImeError, SchemeId};

use super::layout::{
    LAYOUT_DIAGNOSTIC_KEY, LayoutDocument, LayoutTables, compile_layout, parse_layout,
};
use super::{SCHEME_KEYS, SEMICOLON_INDEX, map_syllable};
use crate::segment::syllable_at;

/// The one-character key name a table slot is written under in a layout document.
fn key_name(slot: usize) -> String {
    if slot == SEMICOLON_INDEX {
        String::from(";")
    } else {
        char::from(b'a' + slot as u8).to_string()
    }
}

/// The Xiaohe layout as the plain maps a user's inline `[shuangpin.custom]`
/// tables carry: a valid document every mutation below starts from.
///
/// Read back from the compiled published table rather than restated, so the
/// fixture cannot drift from the data it exercises.
fn xiaohe_tables() -> LayoutTables {
    let table = super::xiaohe::table();
    let mut tables = LayoutTables {
        name: Some(table.name.clone()),
        zero_initial_style: Some(String::from("first-letter")),
        ..LayoutTables::default()
    };
    for slot in 0..SCHEME_KEYS {
        let key = key_name(slot);
        if let Some(initial) = &table.initials[slot] {
            tables.initials.insert(key.clone(), initial.to_string());
        }
        if !table.finals[slot].is_empty() {
            let finals = table.finals[slot]
                .iter()
                .map(|final_text| final_text.as_ref())
                .collect::<Vec<&str>>()
                .join(" ");
            tables.finals.insert(key.clone(), finals);
        }
        if !table.standalone[slot].is_empty() {
            tables
                .standalone
                .insert(key, table.standalone[slot].to_string());
        }
    }
    tables
}

/// Compiles `document` and returns every refusal as the `(key, reason)` of its
/// diagnostic.
///
/// A layout refusal that is anything but a `config/invalid` diagnostic is a
/// contract breach, so an error of another shape fails the test on the spot.
fn diagnostics_of(document: &LayoutDocument) -> Vec<(String, String)> {
    match compile_layout(document, SchemeId::FULL) {
        Ok(_) => Vec::new(),
        Err(errors) => errors
            .into_iter()
            .map(|error| match error {
                ImeError::ConfigInvalid { key, reason } => (key, reason),
                other => panic!("a layout refusal must be a config/invalid diagnostic: {other:?}"),
            })
            .collect(),
    }
}

#[test]
fn test_compile_layout_two_keys_claiming_one_initial_report_the_conflict() {
    let mut tables = xiaohe_tables();
    // The `;` key is free in Xiaohe; handing it the initial `c` leaves two keys
    // claiming one sound, the transcription error the loader refuses whole.
    tables.initials.insert(String::from(";"), String::from("c"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "the conflict is the document's only error: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("conflicting mapping") && reason.contains("both claim the initial `c`"),
        "the refusal names the mistake and the claimed sound: {reason}"
    );
    assert!(
        reason.contains(';') && reason.contains("`c`"),
        "both holding keys are named: {reason}"
    );
}

#[test]
fn test_compile_layout_two_keys_sharing_one_final_compiles_without_a_conflict() {
    let mut tables = xiaohe_tables();
    // A final may legitimately sit on two keys -- Microsoft reaches `üe` through
    // both `t` and `v` -- so sharing a final is transcription, not a conflict.
    // Giving `w` a second holder of the `e` final must still load.
    let existing = tables.finals.get("w").cloned().unwrap_or_default();
    tables
        .finals
        .insert(String::from("w"), format!("{existing} e"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert!(
        diagnostics.is_empty(),
        "a shared final is not a conflicting mapping: {diagnostics:?}"
    );
}

#[test]
fn test_compile_layout_finals_table_missing_reports_the_missing_table() {
    let mut tables = xiaohe_tables();
    // A layout whose `[finals]` is gone closes no syllable; the refusal names
    // the table by its own diagnostic beside the per-syllable losses.
    tables.finals.clear();
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert!(
        diagnostics.iter().any(|(key, reason)| {
            key.as_str() == LAYOUT_DIAGNOSTIC_KEY && reason.contains("[finals] is missing or empty")
        }),
        "the missing table is refused by name: {diagnostics:?}"
    );
}

#[test]
fn test_compile_layout_required_final_without_a_key_reports_it_unreachable() {
    let mut tables = xiaohe_tables();
    // Dropping the `h` key's finals leaves every `-ang` syllable, `ang`
    // included, without a keystroke; the refusal reports the loss per lost
    // syllable, naming the initial, the final and a spelling that shows it.
    assert!(
        tables.finals.remove("h").is_some(),
        "the fixture carries `ang` on `h`"
    );
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert!(
        diagnostics.iter().any(|(key, reason)| {
            key.as_str() == LAYOUT_DIAGNOSTIC_KEY
                && reason.contains("with the final `ang`")
                && reason.contains("cannot be spelled")
        }),
        "the unreachable final is named with a syllable that shows the loss: {diagnostics:?}"
    );
}

#[test]
fn test_compile_layout_a_key_outside_the_alphabet_is_refused_by_its_name() {
    let mut tables = xiaohe_tables();
    // A layout is indexed by `a`-`z` and `;`; a digit is not a key. The value is
    // a plain valid final, so the key name is the document's only mistake.
    tables.finals.insert(String::from("1"), String::from("ang"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "the key name is the document's only error: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("[finals] key `1` is not a scheme key: a key is one of a-z or `;`"),
        "the refusal names the table, the key and the alphabet: {reason}"
    );
}

#[test]
fn test_compile_layout_an_initial_the_table_does_not_know_is_refused_by_its_key() {
    let mut tables = xiaohe_tables();
    // The `;` key is free; a spelling outside the closed initial set is a typo
    // the loader reports rather than an initial the syllable table would never
    // match.
    tables
        .initials
        .insert(String::from(";"), String::from("zhh"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "the unknown spelling is the document's only error: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("[initials] key `;` carries a value that is not a syllable initial"),
        "the refusal names the table and the key: {reason}"
    );
}

#[test]
fn test_compile_layout_zero_initial_data_under_a_first_letter_declaration_is_refused() {
    let mut tables = xiaohe_tables();
    // A first-letter layout opens a vowel-initial syllable with its own letter
    // and carries no zero initial; sneaking the empty initial onto a key is
    // o-led data under a first-letter declaration.
    tables.initials.insert(String::from(";"), String::new());
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "the misplaced zero initial is the document's only error: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("an empty initial is o-led data") && reason.contains("key `;`"),
        "the refusal names the school and the offending key: {reason}"
    );
}

#[test]
fn test_compile_layout_an_o_led_declaration_over_first_letter_data_is_refused() {
    let mut tables = xiaohe_tables();
    // The tables are first-letter data (`o` carries itself); declaring the
    // o-led school leaves the declaration without the zero initial it needs on
    // key `o`, and the loader names exactly that.
    tables.zero_initial_style = Some(String::from("o-led"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "only the declaration disagrees with the tables: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("key `o` must carry the empty initial"),
        "the refusal names the school's key: {reason}"
    );
}

#[test]
fn test_compile_layout_an_unknown_school_spelling_is_refused() {
    let mut tables = xiaohe_tables();
    // A declaration outside the two schools cannot be checked for mixing, so it
    // is refused on its own, naming the two spellings the format accepts.
    tables.zero_initial_style = Some(String::from("oled"));
    let document = LayoutDocument::from_tables(tables);

    let diagnostics = diagnostics_of(&document);

    assert_eq!(
        diagnostics.len(),
        1,
        "the spelling is the document's only error: {diagnostics:?}"
    );
    let (key, reason) = &diagnostics[0];
    assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
    assert!(
        reason.contains("unknown zero_initial_style spelling"),
        "the refusal names the declaration: {reason}"
    );
}

#[test]
fn test_parse_layout_a_broken_document_is_reported_without_quoting_it() {
    // A document is the user's data: the parse diagnostic carries the line the
    // mistake sits on and never the offending table or value, so a refusal can
    // be logged without leaking the layout.
    let broken = "[initials]\nx = \"a\"\n[bogus]\nq = 1\n";

    let errors = parse_layout(broken).expect_err("an unknown table is not a layout document");

    assert_eq!(errors.len(), 1, "one diagnostic for the parse: {errors:?}");
    match &errors[0] {
        ImeError::ConfigInvalid { key, reason } => {
            assert_eq!(key.as_str(), LAYOUT_DIAGNOSTIC_KEY);
            assert!(
                reason.contains("not a valid layout document") && reason.contains("line "),
                "the refusal names the mistake and its line: {reason}"
            );
            assert!(
                !reason.contains("bogus") && !reason.contains("\"a\""),
                "the diagnostic never quotes the user's text: {reason}"
            );
        }
        other => panic!("a parse refusal must be a config/invalid diagnostic: {other:?}"),
    }
}

#[test]
fn test_compile_layout_the_published_xiaohe_document_compiles_as_a_custom_layout() {
    // The document a user writes is the data the build ships: the same parse
    // and compile pair must accept it as a custom layout, under the scheme
    // number a custom table travels by, and the compiled table must answer the
    // keys -- Xiaohe spells `zhong` as `v` followed by `s`.
    let document = parse_layout(include_str!("data/xiaohe.toml"))
        .expect("the published document is valid TOML");

    let table = compile_layout(&document, SchemeId::FULL)
        .expect("the published document passes the shared validator");

    assert_eq!(
        table.name, "小鹤双拼",
        "the layout's own name travels with it"
    );
    let mapped = map_syllable(&table, b"vs").ok().flatten();
    assert_eq!(mapped.map(|mapped| mapped.consumed), Some(2));
    assert_eq!(
        mapped.and_then(|mapped| syllable_at(mapped.full)),
        Some("zhong"),
        "the compiled table spells syllables, not just shape"
    );
}
