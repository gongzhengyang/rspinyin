//! Unit tests for the key-binding projection.
//!
//! Responsibility: pin what the `[keys]` section becomes -- the bit each whitelisted name
//! carries, the diagnostics of an entry that cannot be recorded, and the whole-value
//! replacement the reload path performs.
//!
//! Boundaries: everything here is in memory and depends on no clock, no file, no dictionary
//! and no host. The session half of the configuration-reload rule -- that a composition in
//! progress keeps its input, its candidates and its highlight -- is asserted where the
//! session lives, in the routing layer's own reload test; what this module can prove is that
//! a re-projection touches nothing but the binding table.

use super::*;

use crate::Config;

/// The whitelist, spelled out by hand.
///
/// Deliberately not derived from `KeyName`: a variant nobody adds here is exactly the drift
/// these tests exist to catch.
const WHITELIST: [&str; 10] = [
    "minus",
    "equal",
    "up",
    "down",
    "left",
    "right",
    "tab",
    "shift_tab",
    "page_up",
    "page_down",
];

/// A configuration whose two binding lists are the given ones.
fn with_lists(flip: Vec<KeyName>, highlight: Vec<KeyName>) -> Config {
    let mut config = Config::default();
    config.keys.flip_keys = flip;
    config.keys.highlight_keys = highlight;
    config
}

/// The codes of the diagnostics, in the order they were raised.
fn codes(warnings: &[ImeError]) -> Vec<String> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            ImeError::ConfigInvalid { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

/// The reason of the first diagnostic, or an empty string when there is none.
///
/// Total on purpose: the caller's assertion on the code is what fails the test when the
/// diagnostic is missing, and this never indexes a vector to say so.
fn reason_of(warnings: &[ImeError]) -> String {
    warnings
        .iter()
        .find_map(|warning| match warning {
            ImeError::ConfigInvalid { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn test_project_keys_defaults_match_the_shipped_table() {
    let (bindings, warnings) = project_keys(&Config::default().keys);

    assert!(warnings.is_empty(), "the defaults are routable");
    assert_eq!(bindings, KeyBindings::default());
    assert_eq!(
        bindings.flip_keys,
        FlipSet::MINUS | FlipSet::EQUAL | FlipSet::UP | FlipSet::DOWN
    );
    assert_eq!(
        bindings.highlight_keys,
        HighlightSet::TAB | HighlightSet::SHIFT_TAB
    );
    assert_eq!(bindings.digit_zero, DigitZero::Passthrough);
    assert!(!bindings.enter_commit_raw);
}

#[test]
fn test_project_keys_copies_the_scalar_keys() {
    let mut keys = with_lists(Vec::new(), Vec::new()).keys;
    keys.digit_zero = DigitZero::Flip;
    keys.enter_commit_raw = true;

    let (bindings, warnings) = project_keys(&keys);

    assert!(warnings.is_empty(), "a scalar key cannot be refused");
    assert_eq!(bindings.digit_zero, DigitZero::Flip);
    assert!(bindings.enter_commit_raw);
}

#[test]
fn test_project_keys_projects_the_page_keys() {
    // `page_up` and `page_down` are the two names the four-field table this replaced
    // could not carry: accepted by the configuration, then dropped in silence.
    let config = with_lists(vec![KeyName::PageUp, KeyName::PageDown], Vec::new());

    let (bindings, warnings) = project_keys(&config.keys);

    assert!(warnings.is_empty(), "both names are pageable");
    assert_eq!(bindings.flip_keys, FlipSet::PAGE_UP | FlipSet::PAGE_DOWN);
    assert!(
        !bindings.flip_keys.contains(FlipSet::MINUS),
        "the list replaces the table rather than adding to it"
    );
}

#[test]
fn test_project_keys_projects_the_highlight_keys() {
    // The setting that had no reader: with only `shift_tab` listed, `Tab` no longer
    // moves the highlight.
    let config = with_lists(Vec::new(), vec![KeyName::ShiftTab]);

    let (bindings, warnings) = project_keys(&config.keys);

    assert!(warnings.is_empty());
    assert_eq!(bindings.highlight_keys, HighlightSet::SHIFT_TAB);
    assert!(!bindings.highlight_keys.contains(HighlightSet::TAB));
}

#[test]
fn test_project_keys_accepts_an_empty_list() {
    // A boundary rather than an error: a user who wants no page key at all writes an
    // empty list, and nothing about it is worth a diagnostic.
    let config = with_lists(Vec::new(), Vec::new());

    let (bindings, warnings) = project_keys(&config.keys);

    assert!(warnings.is_empty(), "an empty list is a choice");
    assert_eq!(bindings.flip_keys, FlipSet::empty());
    assert_eq!(bindings.highlight_keys, HighlightSet::empty());
}

#[test]
fn test_project_keys_reports_a_name_the_page_list_cannot_use() {
    let config = with_lists(vec![KeyName::Left], Vec::new());

    let (bindings, warnings) = project_keys(&config.keys);

    let expected = [String::from(UNROUTABLE_BINDING_CODE)];
    assert_eq!(codes(&warnings), expected);
    let reason = reason_of(&warnings);
    assert!(reason.contains("keys.flip_keys"), "{reason}");
    assert!(reason.contains("\"left\""), "{reason}");
    assert!(reason.contains("page_down"), "{reason}");
    assert_eq!(bindings.flip_keys, FlipSet::empty());
}

#[test]
fn test_project_keys_reports_a_name_the_highlight_list_cannot_use() {
    let config = with_lists(Vec::new(), vec![KeyName::PageUp]);

    let (bindings, warnings) = project_keys(&config.keys);

    let expected = [String::from(UNROUTABLE_BINDING_CODE)];
    assert_eq!(codes(&warnings), expected);
    let reason = reason_of(&warnings);
    assert!(reason.contains("keys.highlight_keys"), "{reason}");
    assert!(reason.contains("\"page_up\""), "{reason}");
    assert!(reason.contains("shift_tab"), "{reason}");
    assert_eq!(bindings.highlight_keys, HighlightSet::empty());
}

#[test]
fn test_project_keys_every_whitelisted_name_is_routable_in_one_list() {
    // Every name the configuration accepts has to reach a binding somewhere, and each
    // list's own whitelist has to agree with the bits it sets -- a name listed as accepted
    // but refused by the table would make the diagnostic text a lie.
    for spelling in WHITELIST {
        // A rejected name is replaced by an arbitrary one so that the round-trip assertion
        // below fails and names the spelling, rather than panicking here.
        let name = KeyName::parse(spelling, KEY_FLIP_KEYS).unwrap_or(KeyName::Minus);
        assert_eq!(name.as_str(), spelling, "the spelling round-trips");
        assert!(
            page_bit(name).is_some() || highlight_bit(name).is_some(),
            "{spelling} is routed by nothing"
        );
        assert_eq!(
            PAGEABLE.contains(&name),
            page_bit(name).is_some(),
            "{spelling} and the page list disagree"
        );
        assert_eq!(
            HIGHLIGHTABLE.contains(&name),
            highlight_bit(name).is_some(),
            "{spelling} and the highlight list disagree"
        );
    }
}

#[test]
fn test_project_keys_bit_sets_and_lists_describe_the_same_keys() {
    // One bit per accepted name, no sharing: a name that reused another's bit would make
    // the two keys indistinguishable, which is a silent way for a binding to do the wrong
    // thing. The union of the bits has to be the whole set, so no bit is left unroutable.
    let mut pageable = FlipSet::empty();
    for name in PAGEABLE {
        let Some(bit) = page_bit(*name) else {
            continue;
        };
        assert!(
            !pageable.contains(bit),
            "{} shares a bit with another name",
            name.as_str()
        );
        pageable |= bit;
    }
    assert_eq!(pageable, FlipSet::all());

    let mut highlightable = HighlightSet::empty();
    for name in HIGHLIGHTABLE {
        let Some(bit) = highlight_bit(*name) else {
            continue;
        };
        assert!(
            !highlightable.contains(bit),
            "{} shares a bit with another name",
            name.as_str()
        );
        highlightable |= bit;
    }
    assert_eq!(highlightable, HighlightSet::all());
}

#[test]
fn test_project_keys_reports_a_key_both_lists_bound() {
    let config = with_lists(vec![KeyName::Up], vec![KeyName::Up]);

    let (bindings, warnings) = project_keys(&config.keys);

    assert_eq!(codes(&warnings), [String::from(BINDING_CONFLICT_CODE)]);
    let reason = reason_of(&warnings);
    assert!(reason.contains("keys.flip_keys"), "{reason}");
    assert!(reason.contains("keys.highlight_keys"), "{reason}");
    assert!(reason.contains("\"up\""), "{reason}");
    assert!(
        !bindings.flip_keys.contains(FlipSet::UP),
        "the page binding is the one that gives way"
    );
    assert!(bindings.highlight_keys.contains(HighlightSet::UP));
}

#[test]
fn test_project_keys_reports_a_repeated_name() {
    let config = with_lists(vec![KeyName::Minus, KeyName::Minus], Vec::new());

    let (bindings, warnings) = project_keys(&config.keys);

    assert_eq!(codes(&warnings), [String::from(BINDING_CONFLICT_CODE)]);
    let reason = reason_of(&warnings);
    assert!(reason.contains("\"minus\""), "{reason}");
    assert!(bindings.flip_keys.contains(FlipSet::MINUS));
}

#[test]
fn test_project_keys_leaves_the_configuration_untouched() {
    // The projection is read-only over the configuration it is handed. A reload re-projects
    // the document in force, so a projection that edited it would change what the next
    // component reads -- which is the configuration-layer half of the reload rule.
    let config = with_lists(vec![KeyName::Left, KeyName::Up], vec![KeyName::Up]);
    let before = config.clone();

    let _ = project_keys(&config.keys);

    assert_eq!(config, before, "the configuration is only read");
}

#[test]
fn test_project_keys_codes_render_under_config_invalid() {
    // The codes are contractual: a diagnostic and a test match on the rendered string, so
    // the rendering is pinned here rather than left to the caller to discover.
    let page_left = with_lists(vec![KeyName::Left], Vec::new());
    let up_in_both = with_lists(vec![KeyName::Up], vec![KeyName::Up]);

    let (_, warnings) = project_keys(&page_left.keys);
    let rendered = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        rendered.starts_with("config/invalid: keys/unroutable-binding"),
        "{rendered}"
    );

    let (_, warnings) = project_keys(&up_in_both.keys);
    let rendered = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        rendered.starts_with("config/invalid: keys/binding-conflict"),
        "{rendered}"
    );
}

#[test]
fn test_reproject_replaces_the_table_wholesale() {
    let mut bindings = KeyBindings {
        flip_keys: FlipSet::all(),
        highlight_keys: HighlightSet::all(),
        ..KeyBindings::default()
    };
    let config = with_lists(vec![KeyName::PageDown], vec![KeyName::Left]);

    let warnings = bindings.reproject(&config.keys);

    assert!(warnings.is_empty());
    assert_eq!(
        bindings.flip_keys,
        FlipSet::PAGE_DOWN,
        "the previous table is replaced, not merged"
    );
    assert_eq!(bindings.highlight_keys, HighlightSet::LEFT);
    assert_eq!(bindings, project_keys(&config.keys).0);
}

#[test]
fn test_reproject_reports_what_it_cannot_record() {
    let mut bindings = KeyBindings::default();
    // `tab` pages nothing, so that entry is dropped and reported while the rest of the
    // table is still built.
    let config = with_lists(vec![KeyName::Minus, KeyName::Tab], vec![KeyName::Tab]);

    let warnings = bindings.reproject(&config.keys);

    let expected = [String::from(UNROUTABLE_BINDING_CODE)];
    assert_eq!(codes(&warnings), expected);
    assert_eq!(bindings.flip_keys, FlipSet::MINUS);
    assert_eq!(bindings.highlight_keys, HighlightSet::TAB);
}

#[test]
fn test_reproject_is_idempotent_for_an_unchanged_configuration() {
    // Reloading a document that did not change leaves the table exactly as it was: the
    // reload path is a re-projection, not a merge, and the diagnostics are recomputed
    // rather than accumulated.
    let config = with_lists(vec![KeyName::PageUp], vec![KeyName::ShiftTab]);
    let mut bindings = KeyBindings::default();

    let first = bindings.reproject(&config.keys);
    let after_first = bindings;
    let second = bindings.reproject(&config.keys);

    assert!(first.is_empty());
    assert_eq!(codes(&first), codes(&second));
    assert_eq!(bindings, after_first);
}

#[test]
fn test_reproject_depends_only_on_the_configuration() {
    // No hidden state: the same document projects to the same table whatever was in force
    // before it. This is the half of the reload rule this layer can prove -- nothing of the
    // table that was replaced survives into the one that follows it.
    let first = with_lists(vec![KeyName::Minus], vec![KeyName::Tab]);
    let other = with_lists(vec![KeyName::PageUp], vec![KeyName::Left]);
    let mut bindings = KeyBindings::default();

    let _ = bindings.reproject(&first.keys);
    let expected = bindings;
    let _ = bindings.reproject(&other.keys);
    let _ = bindings.reproject(&first.keys);

    assert_eq!(bindings, expected, "the table depends on the document");
}
