//! Unit tests for the configuration model.
//!
//! Responsibility: pin the repair rules -- the table of scalar keys that must fall back to
//! their built-in default, the two key-binding lists, the defaults of the sections that
//! ship a switch beside a value, and the size budget of `Config` itself -- and the
//! whitelist `KeyName` is parsed against.
//!
//! Boundaries: everything here is in memory. The documents the suite parses are string
//! literals, so no file is touched; every other input is `Config::default` with one field
//! changed. The suite depends on no clock, no environment and no dictionary.

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

/// A configuration whose two key-binding lists are the given ones.
fn with_lists(flip: Vec<KeyName>, highlight: Vec<KeyName>) -> Config {
    tweaked(|c| {
        c.keys.flip_keys = flip;
        c.keys.highlight_keys = highlight;
    })
}

/// The key-name whitelist, spelled out by hand.
///
/// Deliberately not derived from `KeyName`: a variant nobody adds here is exactly the drift
/// these walks exist to catch, so deriving the list would defeat them.
const WHITELIST: [&str; 12] = [
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
    "home",
    "end",
];

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
        ("schema_version", |c: &mut Config| {
            c.schema_version = CONFIG_SCHEMA_VERSION + 1;
        }),
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
        ("data.backup_keep", |c: &mut Config| c.data.backup_keep = 0),
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

    // More entries than the bound allows: the surplus is dropped and what is left is still
    // a usable list. The bound is the number of keys the list can route, so the entry that
    // goes is one that could never have done anything.
    let over = tweaked(|c| {
        c.keys.flip_keys = [
            KeyName::Minus,
            KeyName::Equal,
            KeyName::Up,
            KeyName::Down,
            KeyName::Left,
            KeyName::Right,
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
fn test_repaired_settles_a_key_both_lists_can_claim() {
    // `up` and `down` are the two names the two whitelists share, so a document that writes
    // one of them into both lists is the whole of this conflict. The page entry is the one
    // that gives way, which is the direction the routing table applies as well.
    for name in [KeyName::Up, KeyName::Down] {
        let document = format!(
            "[keys]\nflip_keys = [\"{}\"]\nhighlight_keys = [\"{}\"]\n",
            name.as_str(),
            name.as_str()
        );
        let (config, warnings) = parsed_document(&document);

        assert_eq!(
            rejected(&warnings),
            [String::from(BINDING_CONFLICT_CODE)],
            "one conflict, one diagnostic"
        );
        assert!(
            config.keys.flip_keys.is_empty(),
            "the page entry is the one that gives way"
        );
        assert_eq!(
            config.keys.highlight_keys,
            [name],
            "the highlight entry stays"
        );
        assert!(
            config.validate().is_empty(),
            "the repair is idempotent: nothing is left to report"
        );
    }
}

#[test]
fn test_repaired_reports_a_cross_list_conflict_only_when_both_lists_can_use_the_key() {
    // The rule has a boundary: a name one of the two lists cannot route is that list's own
    // mistake, which the projection reports as `keys/unroutable-binding`, and calling it a
    // conflict would describe a clash that never happened. The walk covers the whole
    // whitelist, so the boundary is pinned for every name rather than for the two that
    // conflict today.
    for spelling in WHITELIST {
        let name = KeyName::parse(spelling, KEY_FLIP_KEYS).expect("the whitelist parses");
        let claimed_twice = with_lists(vec![name], vec![name]);
        let routable_in_both = both_lists_can_route(name, &claimed_twice.keys);

        let (repaired, warnings) = claimed_twice.repaired();
        let reported = rejected(&warnings) == [String::from(BINDING_CONFLICT_CODE)];

        assert_eq!(
            reported, routable_in_both,
            "{spelling}: a conflict is reported exactly when both lists can route the key"
        );
        if reported {
            assert!(
                repaired.keys.flip_keys.is_empty(),
                "{spelling} leaves the page list"
            );
            assert_eq!(
                repaired.keys.highlight_keys,
                [name],
                "{spelling} stays a highlight key"
            );
        } else {
            assert_eq!(
                repaired.keys.flip_keys,
                [name],
                "{spelling} keeps its page entry"
            );
        }
    }
}

#[test]
fn test_repaired_names_the_page_list_and_keeps_the_highlight_entry() {
    let conflict = with_lists(vec![KeyName::Minus, KeyName::Up], vec![KeyName::Up]);

    let (repaired, warnings) = conflict.repaired();

    assert_eq!(rejected(&warnings), [String::from(BINDING_CONFLICT_CODE)]);
    assert_eq!(
        repaired.keys.flip_keys,
        [KeyName::Minus],
        "only the shared key leaves the page list"
    );
    assert_eq!(repaired.keys.highlight_keys, [KeyName::Up]);

    // The rendering is what a user reads, so it is pinned: the code under `config/invalid`,
    // and a reason naming both lists and the key that was named twice.
    let rendered = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        rendered.starts_with("config/invalid: keys/binding-conflict"),
        "{rendered}"
    );
    assert!(rendered.contains(KEY_FLIP_KEYS), "{rendered}");
    assert!(rendered.contains(KEY_HIGHLIGHT_KEYS), "{rendered}");
    assert!(rendered.contains("\"up\""), "{rendered}");
}

#[test]
fn test_repaired_leaves_disjoint_binding_lists_alone() {
    // The shipped defaults are disjoint, and so is a document that never names one key
    // twice. A repair that reported anything here would put a diagnostic on a configuration
    // nobody got wrong.
    let (defaults, warnings) = Config::default().repaired();
    assert!(
        warnings.is_empty(),
        "the defaults are disjoint: {warnings:?}"
    );
    assert_eq!(defaults, Config::default());

    let disjoint = with_lists(
        vec![KeyName::Minus, KeyName::PageUp],
        vec![KeyName::Tab, KeyName::Right],
    );
    let (repaired, warnings) = disjoint.repaired();

    assert!(
        warnings.is_empty(),
        "no overlap, no diagnostic: {warnings:?}"
    );
    assert_eq!(repaired.keys.flip_keys, [KeyName::Minus, KeyName::PageUp]);
    assert_eq!(repaired.keys.highlight_keys, [KeyName::Tab, KeyName::Right]);
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
    assert!(
        WHITELIST.len() > MAX_KEY_BINDINGS,
        "the length bound is reachable only while the whitelist is longer than it"
    );
    for name in WHITELIST {
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

/// The configuration a document produces, failing the test when it cannot be read.
///
/// The model is pinned twice: as values built by hand above, and as documents below. The
/// second form is what a user actually has, and it is the only way to reach the loader's
/// answer to a key.
fn parsed_document(text: &str) -> (Config, Vec<ImeError>) {
    match Config::from_document(text) {
        Ok(parsed) => parsed,
        Err(error) => panic!("the document must be readable: {error}"),
    }
}

#[test]
fn test_engine_abbrev_default_is_off() {
    // An abbreviation is ambiguous by nature -- `nh` reads as `ni'hao` as readily as
    // `na'he` -- so a user who never asked for one must not have it answered ahead of
    // the full spelling.
    assert!(!Config::default().engine.abbrev, "the default is off");
    assert!(
        Config::default().validate().is_empty(),
        "a flag needs no repair rule"
    );
}

#[test]
fn test_engine_abbrev_leaves_the_other_keys_alone() {
    // The switch is one flag of one section. Turning it on, and leaving it off, must both
    // leave every other key exactly as it was: a flag that moved anything beside itself
    // would be a decode change nobody asked for.
    let (repaired, warnings) = tweaked(|c| c.engine.abbrev = true).repaired();
    assert!(warnings.is_empty(), "a flag cannot hold an unusable value");
    assert!(repaired.engine.abbrev, "the value is carried");
    assert_eq!(
        EngineConfig {
            abbrev: false,
            ..repaired.engine
        },
        Config::default().engine,
        "only the switch moved"
    );

    // Off is the default, so a configuration that spells it out is the built-in one.
    assert_eq!(tweaked(|c| c.engine.abbrev = false), Config::default());
}

#[test]
fn test_data_section_defaults() {
    let data = Config::default().data;
    assert_eq!(data.durability, Durability::Eventual);
    assert!(
        data.backup_enabled,
        "the user's words are copied by default"
    );
    assert_eq!(data.backup_keep, DEFAULT_BACKUP_KEEP);
    assert_eq!(DEFAULT_BACKUP_KEEP, 3);
    // Both values are constants, so the relation between them is checked at compile time
    // rather than by a runtime assertion that could only ever restate the numbers above.
    const _: () = assert!(MAX_BACKUP_KEEP > DEFAULT_BACKUP_KEEP);

    // The section has a `Default` of its own because two of its three keys are not
    // zero-valued, so the two spellings of the same defaults are pinned together.
    assert_eq!(DataConfig::default(), data);

    // The section is usable as it ships: repairing the defaults changes nothing and
    // reports nothing.
    let (repaired, warnings) = Config::default().repaired();
    assert!(warnings.is_empty());
    assert_eq!(repaired.data, data);
}

#[test]
fn test_data_backup_keep_bounds() {
    let key = [String::from("data.backup_keep")];

    // Zero is not a usable count: the rotation removes the oldest generations past the
    // count once a new one has landed, so it would remove the copy it had just written.
    let zero = tweaked(|c| c.data.backup_keep = 0);
    assert_eq!(rejected(&zero.validate()), key);
    let (repaired, warnings) = zero.repaired();
    assert_eq!(rejected(&warnings), key);
    assert_eq!(repaired.data.backup_keep, DEFAULT_BACKUP_KEEP);

    // The ceiling is the other half: one generation is at most the store's export ceiling
    // on disk, so a document cannot ask for a backup directory nothing bounds.
    let over = tweaked(|c| c.data.backup_keep = MAX_BACKUP_KEEP + 1);
    assert_eq!(rejected(&over.validate()), key);
    let (repaired, warnings) = over.repaired();
    assert_eq!(rejected(&warnings), key);
    assert_eq!(repaired.data.backup_keep, DEFAULT_BACKUP_KEEP);

    // Both ends of the range are accepted, so the bound is inclusive and a user who asks
    // for exactly the limit is not silently moved back to the default.
    for keep in [1, MAX_BACKUP_KEEP] {
        let config = tweaked(|c| c.data.backup_keep = keep);
        assert!(config.validate().is_empty(), "the range is inclusive");
        let expected = config.data;
        let (repaired, warnings) = config.repaired();
        assert!(warnings.is_empty());
        assert_eq!(repaired.data, expected);
    }
}

#[test]
fn test_data_backup_enabled_is_a_value_not_a_mistake() {
    // A flag has no value outside its set, so there is nothing for a rule to report: both
    // spellings are accepted, and the one that was written survives the repair.
    for enabled in [true, false] {
        let config = tweaked(|c| c.data.backup_enabled = enabled);
        assert!(config.validate().is_empty(), "a flag cannot be unusable");
        let (repaired, warnings) = config.repaired();
        assert!(warnings.is_empty());
        assert_eq!(
            repaired.data.backup_enabled, enabled,
            "the value is carried"
        );
    }
}

#[test]
fn test_engine_section_reads_abbrev_from_a_document() {
    let (config, warnings) = parsed_document("[engine]\nabbrev = true\n");
    assert!(warnings.is_empty(), "every value is usable: {warnings:?}");
    assert!(config.engine.abbrev, "the key is carried");
    assert_eq!(
        EngineConfig {
            abbrev: false,
            ..config.engine
        },
        Config::default().engine,
        "the keys beside it are still the built-in ones"
    );

    // Off is what a document that never mentions the key gets, and a document that says so
    // explicitly is the built-in configuration.
    assert_eq!(
        parsed_document("[engine]\nabbrev = false\n").0,
        Config::default()
    );
}

#[test]
fn test_data_section_reads_the_backup_keys_from_a_document() {
    let (config, warnings) = parsed_document(
        "[data]\n\
         backup_enabled = false\n\
         backup_keep = 7\n",
    );
    assert!(warnings.is_empty(), "every value is in range: {warnings:?}");
    assert!(!config.data.backup_enabled);
    assert_eq!(config.data.backup_keep, 7);
    // The key beside them is still read: one setting does not cost the user the rest of
    // the section.
    assert_eq!(config.data.durability, Durability::Eventual);
}

#[test]
fn test_data_section_repairs_a_backup_keep_a_document_writes_out_of_range() {
    let out_of_range = [String::from("0"), (MAX_BACKUP_KEEP + 1).to_string()];
    for written in out_of_range {
        let document = format!("[data]\nbackup_keep = {written}\n");
        let (config, warnings) = parsed_document(&document);

        assert_eq!(
            config.data.backup_keep, DEFAULT_BACKUP_KEEP,
            "the default is restored"
        );
        assert_eq!(rejected(&warnings), [String::from("data.backup_keep")]);
        assert!(
            warnings.iter().any(|warning| warning
                .to_string()
                .contains(&format!("{written} is outside"))),
            "the reason quotes what the document said: {warnings:?}"
        );
    }
}
