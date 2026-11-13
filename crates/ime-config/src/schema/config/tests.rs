//! Tests for the typed `[engine]`, `[ui]`, `[theme]`, `[keys]`, `[assoc]`,
//! `[quick_input]` and `[english]` sections: the shipped defaults, pinned so a
//! default that moves is a conscious edit, and the document round-trips of the
//! keys this file's section types carry.

use super::*;

#[test]
fn test_config_default_quick_input_section_is_disabled_with_bounded_prefixes() {
    let quick = Config::default().quick_input;
    assert!(!quick.enabled);
    assert_eq!(quick.date_prefixes, ["rq", "dt"]);
    assert!(!quick.calc_enabled);
}

#[test]
fn test_config_default_english_section_is_auto() {
    // The shipped behaviour pinned here so a default that moves is a
    // conscious edit: Auto is the boundary rule the benchmark's spacer
    // translates, and a drift to Off would silently strip the one space
    // every mixed sentence after it expects.
    assert_eq!(Config::default().english.spacing, EnglishSpacing::Auto);
}

#[test]
fn test_config_default_assoc_section_is_disabled_with_shipped_bounds() {
    // The values the section ships with, pinned here so a default that moves
    // is a conscious edit: the section is an opt-in feature, and a default
    // that drifted to enabled would put candidates after every commit for
    // users who never asked for them. The document-level drift test in
    // `reload` covers the same defaults from the template side; this one
    // holds the type side on its own.
    let assoc = Config::default().assoc;
    assert!(!assoc.enabled);
    assert_eq!(assoc.max_candidates, 5);
    assert_eq!(assoc.history_depth, 8);
}

#[test]
fn test_config_default_assoc_section_needs_no_repair() {
    // The shipped section sits inside every range the repair rules hold, so
    // validating the defaults reports nothing: a fresh install never opens
    // with a repaired-value notice it did nothing to earn.
    assert!(Config::default().validate().is_empty());
}

#[test]
fn test_config_default_shuangpin_section_names_no_layout() {
    // The section is an explicit opt-in: a fresh installation names no file
    // and writes no inline tables, so no custom layout exists to load and the
    // double-pinyin layouts are exactly the built-in ones.
    let shuangpin = Config::default().shuangpin;
    assert!(shuangpin.custom.is_unset());
    assert_eq!(shuangpin.custom.file, "");
    assert!(shuangpin.custom.initials.is_empty());
    assert!(shuangpin.custom.finals.is_empty());
    assert!(shuangpin.custom.standalone.is_empty());
}

#[test]
fn test_config_default_select_char_keys_are_the_brackets() {
    // The shipped keys pinned here so a default that moves is a conscious edit:
    // the brackets are what the benchmark's gesture is written with, and a drift
    // would silently take a user's punctuation away or hand the gesture to a key
    // they did not expect.
    let keys = Config::default().keys;
    assert_eq!(keys.select_first_char, SelectCharKey::BracketLeft);
    assert_eq!(keys.select_last_char, SelectCharKey::BracketRight);
    assert_eq!(keys.select_first_char.mark(), Some('['));
    assert_eq!(keys.select_last_char.mark(), Some(']'));
}

#[test]
fn test_document_select_char_keys_are_parsed_and_repaired_by_key() {
    let document = concat!(
        "[keys]\n",
        "select_first_char = \"comma\"\n",
        "select_last_char = \"tilde\"\n",
    );
    let (config, warnings) = Config::from_document(document).expect("the document parses");

    assert_eq!(config.keys.select_first_char, SelectCharKey::Comma);
    assert_eq!(
        config.keys.select_last_char,
        SelectCharKey::BracketRight,
        "the refused value keeps its default"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    for (warning, key) in warnings.iter().zip(["keys.select_last_char"]) {
        assert!(warning.to_string().contains(key), "{warning}");
    }
}

#[test]
fn test_document_select_char_key_off_is_accepted_without_a_warning() {
    let document = "[keys]\nselect_first_char = \"off\"\nselect_last_char = \"off\"\n";
    let (config, warnings) = Config::from_document(document).expect("the document parses");

    assert_eq!(config.keys.select_first_char, SelectCharKey::Off);
    assert_eq!(config.keys.select_last_char, SelectCharKey::Off);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn test_document_assoc_section_is_parsed_and_repaired_by_key() {
    let document = concat!(
        "[assoc]\n",
        "enabled = true\n",
        "max_candidates = 0\n",
        "history_depth = 65\n",
    );
    let (config, warnings) = Config::from_document(document).expect("the document parses");

    assert!(config.assoc.enabled);
    assert_eq!(config.assoc.max_candidates, 5);
    assert_eq!(config.assoc.history_depth, 8);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for (warning, key) in warnings
        .iter()
        .zip(["assoc.max_candidates", "assoc.history_depth"])
    {
        assert!(warning.to_string().contains(key), "{warning}");
    }
}

#[test]
fn test_config_default_aux_refinement_is_off() {
    // The refinement session is an opt-in: a lead key that starts filtering
    // changes what the window shows, and a user who never asked for it must
    // not have their compositions answered by one. Pinned here so a default
    // that moves is a conscious edit; the document-side drift test in
    // `reload` holds the template to the same value.
    assert!(
        !Config::default().engine.aux_refinement,
        "the default is off"
    );
    assert!(
        Config::default().validate().is_empty(),
        "a flag needs no repair rule"
    );
}

#[test]
fn test_document_aux_refinement_is_carried_as_written() {
    // A flag has no value outside its set, so both spellings parse and
    // survive the repair untouched: the document and the typed section can
    // only disagree about a key this file forgot to declare, which the
    // carried value would catch.
    let (on, warnings) =
        Config::from_document("[engine]\naux_refinement = true\n").expect("the document parses");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(on.engine.aux_refinement, "the value is carried");

    let (off, warnings) =
        Config::from_document("[engine]\naux_refinement = false\n").expect("the document parses");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!off.engine.aux_refinement, "the value is carried");

    // Absent keeps the default, which is the off state: a document from a
    // build before the key existed behaves exactly as it always did.
    assert!(!Config::default().engine.aux_refinement);
}
