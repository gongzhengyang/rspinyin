//! Unit tests for the configuration model.
//!
//! Responsibility: pin the repair rules -- the table of scalar keys that must fall back to
//! their built-in default, the two key-binding lists, and the size budget of `Config`
//! itself -- and the whitelist `KeyName` is parsed against.
//!
//! Boundaries: everything here is in memory. No document is read, no file is touched, and
//! the only input is `Config::default` with one field changed, so the suite depends on no
//! clock, no environment and no dictionary.

use super::*;

/// A configuration that breaks exactly one rule, with the key its diagnostic must name.
///
/// A named alias rather than the bare tuple so the table below reads as a list of
/// cases rather than as a signature.
type ScalarBreaker = (&'static str, fn(&mut Config));

/// A configuration with one change applied.
fn tweaked(change: impl FnOnce(&mut Config)) -> Config {
    let mut config = Config::default();
    change(&mut config);
    config
}

/// The `config/invalid` key of each diagnostic.
fn rejected(warnings: &[ImeError]) -> Vec<String> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            ImeError::ConfigInvalid { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

/// Every scalar rule, with a configuration that breaks it and the key its
/// diagnostic must name. `Config::repaired` is the single implementation of all of
/// them, so a rule missing from this table is a rule nothing checks.
fn scalar_breakers() -> Vec<ScalarBreaker> {
    vec![
        ("schema_version", |c: &mut Config| c.schema_version = 2),
        ("engine.max_raw_len", |c: &mut Config| {
            c.engine.max_raw_len = 0
        }),
        ("ui.max_per_row", |c: &mut Config| c.ui.max_per_row = 2),
        ("ui.max_width_dp", |c: &mut Config| c.ui.max_width_dp = 200),
        ("ui.corner_radius_dp", |c: &mut Config| {
            c.ui.corner_radius_dp = 4
        }),
        ("ui.animation.omega0", |c: &mut Config| {
            c.ui.animation.omega0 = 100.0
        }),
        ("ui.animation.zeta", |c: &mut Config| {
            c.ui.animation.zeta = 0.1
        }),
        ("ui.animation.appear_ms", |c: &mut Config| {
            c.ui.animation.appear_ms = 700
        }),
        ("ui.animation.disappear_ms", |c: &mut Config| {
            c.ui.animation.disappear_ms = 700;
        }),
        ("phrases.max_entries", |c: &mut Config| {
            c.phrases.max_entries = 0
        }),
    ]
}

#[test]
fn test_repaired_restores_the_default_of_every_scalar_key() {
    let (defaults, warnings) = Config::default().repaired();
    assert_eq!(defaults, Config::default());
    assert!(warnings.is_empty(), "the defaults are valid");

    let breakers = scalar_breakers();
    assert!(breakers.len() >= 9, "every scalar rule is in the table");
    for (key, break_it) in breakers {
        let config = tweaked(break_it);
        assert!(!config.validate().is_empty(), "breaking {key} is noticed");

        let (repaired, warnings) = config.repaired();
        assert_eq!(
            repaired,
            Config::default(),
            "the default of {key} is restored"
        );
        assert_eq!(
            rejected(&warnings),
            [String::from(key)],
            "diagnostic of {key}"
        );
    }
}

#[test]
fn test_repaired_repairs_the_key_binding_lists() {
    // A repeat is reported and the first position survives.
    let repeated = tweaked(|c| c.keys.flip_keys.push(KeyName::Minus));
    let (repaired, warnings) = repeated.repaired();
    assert_eq!(rejected(&warnings), [String::from(KEY_FLIP_KEYS)]);
    assert_eq!(repaired.keys.flip_keys, Config::default().keys.flip_keys);
    assert!(repaired.validate().is_empty());

    // More entries than the bound allows: the surplus is dropped, as `ASM-19`
    // requires, and what is left is still a usable list.
    let over = tweaked(|c| {
        c.keys.flip_keys = [
            KeyName::Minus,
            KeyName::Equal,
            KeyName::Up,
            KeyName::Down,
            KeyName::Left,
            KeyName::Right,
            KeyName::Tab,
            KeyName::ShiftTab,
            KeyName::PageUp,
        ]
        .to_vec();
    });
    assert_eq!(over.keys.flip_keys.len(), MAX_KEY_BINDINGS + 1);
    let (repaired, warnings) = over.repaired();
    assert_eq!(rejected(&warnings), [String::from(KEY_FLIP_KEYS)]);
    assert_eq!(repaired.keys.flip_keys.len(), MAX_KEY_BINDINGS);
    assert!(repaired.validate().is_empty());

    // The same rule on the other list, so neither can be forgotten.
    let highlight = tweaked(|c| c.keys.highlight_keys.push(KeyName::Tab));
    let (repaired, warnings) = highlight.repaired();
    assert_eq!(rejected(&warnings), [String::from(KEY_HIGHLIGHT_KEYS)]);
    assert_eq!(
        repaired.keys.highlight_keys,
        Config::default().keys.highlight_keys
    );
}

#[test]
fn test_validate_leaves_the_configuration_alone() {
    let config = tweaked(|c| c.ui.max_per_row = 99);
    let before = config.clone();
    assert_eq!(rejected(&config.validate()), ["ui.max_per_row"]);
    assert_eq!(config, before, "validate is the read-only view");
}

#[test]
fn test_key_name_whitelist_round_trips_through_as_str() {
    // The whitelist and `as_str` are two halves of one mapping: pinning them
    // together means neither can be extended without the other.
    let names = [
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
    assert!(
        names.len() > MAX_KEY_BINDINGS,
        "the length bound is reachable only while the whitelist is longer than it"
    );
    for name in names {
        // An empty string is what a rejected name round-trips to, so the
        // assertion below fails rather than silently passing.
        let written = KeyName::parse(name, KEY_FLIP_KEYS)
            .map(KeyName::as_str)
            .unwrap_or_default();
        assert_eq!(written, name);
    }
    assert!(KeyName::parse("", KEY_FLIP_KEYS).is_err());
    assert!(KeyName::parse("空格", KEY_FLIP_KEYS).is_err());
}

#[test]
fn test_logs_input_characters_is_false_whatever_the_key_says() {
    let off = Config::default().diagnostics;
    assert!(!off.log_input_content, "the default is off");
    assert!(!off.logs_input_characters());

    let on = DiagnosticsConfig {
        log_input_content: true,
        ..off
    };
    assert!(on.log_input_content, "the key is carried");
    assert!(
        !on.logs_input_characters(),
        "the key cannot turn logging on"
    );
    assert!(off.probes, "probes default to on");
}

#[test]
fn test_config_stays_inside_the_size_budget() {
    // The design passes the configuration around as an `Arc`, and the budget is
    // 2KB; a field that quietly grows the struct past it is a regression.
    let size = size_of::<Config>();
    assert!(size <= 2048, "Config is {size} bytes");
}

#[test]
fn test_phrases_section_defaults() {
    let phrases = Config::default().phrases;
    assert!(phrases.enabled, "the phrase table is on by default");
    assert!(
        phrases.file.is_empty(),
        "an empty file means the default location"
    );
    assert_eq!(phrases.max_entries, DEFAULT_PHRASE_ENTRIES);
    assert_eq!(DEFAULT_PHRASE_ENTRIES, 5_000);
    // Both values are constants, so the relation between them is checked at compile time
    // rather than by a runtime assertion that could only ever restate the numbers above.
    const _: () = assert!(MAX_PHRASE_ENTRIES > DEFAULT_PHRASE_ENTRIES);

    // The section is usable as it ships: repairing the defaults changes nothing and
    // reports nothing.
    let (repaired, warnings) = Config::default().repaired();
    assert!(warnings.is_empty());
    assert_eq!(repaired.phrases, phrases);
}

#[test]
fn test_phrases_max_entries_bounds() {
    let key = [String::from("phrases.max_entries")];

    // Zero is not a usable limit: a table that may hold nothing is a disabled table,
    // and `enabled` is the key that says so.
    let zero = tweaked(|c| c.phrases.max_entries = 0);
    assert_eq!(rejected(&zero.validate()), key);
    let (repaired, warnings) = zero.repaired();
    assert_eq!(rejected(&warnings), key);
    assert_eq!(repaired.phrases.max_entries, DEFAULT_PHRASE_ENTRIES);

    // The upper bound is the other half: a document cannot ask for a table whose size
    // nothing bounds.
    let over = tweaked(|c| c.phrases.max_entries = MAX_PHRASE_ENTRIES + 1);
    assert_eq!(rejected(&over.validate()), key);
    let (repaired, warnings) = over.repaired();
    assert_eq!(rejected(&warnings), key);
    assert_eq!(repaired.phrases.max_entries, DEFAULT_PHRASE_ENTRIES);

    // Both ends of the range are accepted, so the bound is inclusive and a user who
    // asks for exactly the limit is not silently moved back to the default.
    let edges = [
        tweaked(|c| c.phrases.max_entries = 1),
        tweaked(|c| c.phrases.max_entries = MAX_PHRASE_ENTRIES),
    ];
    for config in edges {
        assert!(config.validate().is_empty(), "the range is inclusive");
        // `repaired` consumes the configuration, so the value the assertion compares
        // against is taken first rather than reached for afterwards.
        let expected = config.phrases.clone();
        let (repaired, warnings) = config.repaired();
        assert!(warnings.is_empty());
        assert_eq!(repaired.phrases, expected);
    }
}
