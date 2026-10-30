//! Unit tests for the `[scheme]` section.
//!
//! Responsibility: pin the spellings the `scheme.scheme` key accepts, the mapping onto
//! the contract's layout numbering, the two values the section projects onto the layers
//! below, and the repair rules for a custom table -- both as a section built by hand and
//! as a document writes it.
//!
//! Boundaries: everything here is in memory. The one document the suite parses is a
//! string literal, so no file is touched; every other input is a value built by hand.
//! The suite depends on no clock, no environment and no dictionary.

use ime_types::{ConfigError, ImeError, SchemeId};

use super::{CUSTOM_TABLE_KEYS, CustomSchemeConfig, SchemeChoice, SchemeConfig};
use crate::schema::Config;

/// Every spelling the `scheme.scheme` key accepts, in schema order.
const CHOICES: [SchemeChoice; 7] = [
    SchemeChoice::Full,
    SchemeChoice::Xiaohe,
    SchemeChoice::Ziranma,
    SchemeChoice::Microsoft,
    SchemeChoice::Sogou,
    SchemeChoice::Ziguang,
    SchemeChoice::Custom,
];

/// Parses a spelling, failing the test when the parser does not accept it.
fn parsed(raw: &str) -> SchemeChoice {
    match SchemeChoice::parse(raw) {
        Ok(choice) => choice,
        Err(error) => panic!("{raw} must parse: {error}"),
    }
}

/// The `config/invalid` key of each diagnostic, in the order they were raised.
fn rejected(warnings: &[ImeError]) -> Vec<String> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            ImeError::ConfigInvalid { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

/// A custom table with one entry per letter of the alphabet, in both lists.
fn custom_table() -> CustomSchemeConfig {
    CustomSchemeConfig {
        initials: vec![String::new(); CUSTOM_TABLE_KEYS],
        finals: vec![String::new(); CUSTOM_TABLE_KEYS],
    }
}

#[test]
fn test_scheme_choice_parse_accepts_every_spelling_it_writes() {
    for choice in CHOICES {
        assert_eq!(SchemeChoice::parse(choice.as_str()), Ok(choice));
        assert_eq!(choice.as_str(), choice.as_str().to_ascii_lowercase());
    }
}

#[test]
fn test_scheme_choice_parse_ignores_the_case_of_a_name() {
    assert_eq!(parsed("XiaoHe"), SchemeChoice::Xiaohe);
    assert_eq!(parsed("FULL"), SchemeChoice::Full);
    assert_eq!(parsed("ZiGuang"), SchemeChoice::Ziguang);
    assert_eq!(parsed("ziranma"), SchemeChoice::Ziranma);
}

#[test]
fn test_scheme_choice_parse_rejects_an_unknown_name() {
    match SchemeChoice::parse("shuangpin") {
        Err(ConfigError::Invalid { key, reason }) => {
            assert_eq!(key, "scheme.scheme");
            assert!(
                reason.contains("shuangpin"),
                "the reason quotes what the document said, and it is {reason}"
            );
        }
        other => panic!("expected config/invalid, got {other:?}"),
    }
}

#[test]
fn test_scheme_choice_parse_rejects_an_empty_name() {
    // The boundary on the other side of the accepted set: an empty value is a name
    // that was never written, not a name this build does not know.
    assert!(SchemeChoice::parse("").is_err());
    assert!(SchemeChoice::parse(" ").is_err());
}

#[test]
fn test_scheme_choice_try_from_matches_parse() {
    // The document loader reads every closed set of spellings through `TryFrom`, so
    // the two must agree on both sides of the boundary.
    assert_eq!(SchemeChoice::try_from("sogou"), Ok(SchemeChoice::Sogou));
    assert!(matches!(
        SchemeChoice::try_from("nope"),
        Err(ConfigError::Invalid { .. })
    ));
}

#[test]
fn test_scheme_choice_default_is_full_pinyin() {
    assert_eq!(SchemeChoice::default(), SchemeChoice::Full);
    assert!(!SchemeChoice::default().is_double_pinyin());
    for choice in CHOICES {
        assert_eq!(choice.is_double_pinyin(), choice != SchemeChoice::Full);
    }
}

#[test]
fn test_scheme_choice_maps_onto_the_contract_numbering() {
    let pairs: [(SchemeChoice, SchemeId); 6] = [
        (SchemeChoice::Full, SchemeId::FULL),
        (SchemeChoice::Xiaohe, SchemeId::XIAOHE),
        (SchemeChoice::Ziranma, SchemeId::ZIRANMA),
        (SchemeChoice::Microsoft, SchemeId::MICROSOFT),
        (SchemeChoice::Sogou, SchemeId::SOGOU),
        (SchemeChoice::Ziguang, SchemeId::ZIGUANG),
    ];
    for (choice, scheme) in pairs {
        assert_eq!(SchemeId::from(choice), scheme);
    }
    // A custom layout has no number of its own: the frozen numbering names the five
    // published layouts, and a table no other build can interpret must not borrow one
    // of their numbers.
    assert_eq!(SchemeId::from(SchemeChoice::Custom), SchemeId::FULL);
    for choice in CHOICES {
        assert!(SchemeId::from(choice).is_known(), "a known scheme");
    }
}

#[test]
fn test_scheme_choice_hint_names_the_layout() {
    assert_eq!(SchemeChoice::Full.hint(), "全拼");
    assert_eq!(SchemeChoice::Xiaohe.hint(), "小鹤");
    for choice in CHOICES {
        assert!(!choice.hint().is_empty(), "a hint is required");
    }
}

#[test]
fn test_scheme_config_default_keeps_full_pinyin_typing() {
    let config = SchemeConfig::default();
    assert_eq!(config.scheme, SchemeChoice::Full);
    assert!(config.show_hint, "the header names the layout");
    assert!(config.keep_full_pinyin, "full pinyin is still read");
    assert!(config.custom.initials.is_empty(), "no table ships");
    assert!(config.custom.finals.is_empty(), "no table ships");
}

#[test]
fn test_scheme_config_repaired_leaves_a_valid_section_alone() {
    let config = SchemeConfig {
        scheme: SchemeChoice::Xiaohe,
        show_hint: false,
        keep_full_pinyin: false,
        custom: CustomSchemeConfig::default(),
    };
    let (repaired, warnings) = config.clone().repaired();
    assert_eq!(repaired, config);
    assert!(warnings.is_empty(), "a valid section raises nothing");
    assert!(config.validate().is_empty());
}

#[test]
fn test_scheme_config_repaired_degrades_a_missing_custom_table() {
    let config = SchemeConfig {
        scheme: SchemeChoice::Custom,
        ..SchemeConfig::default()
    };
    let (repaired, warnings) = config.repaired();
    assert_eq!(repaired.scheme, SchemeChoice::Full);
    // One diagnostic, not three. The actionable problem is that this build has no
    // custom layout; the absent table is the shape that choice has here, and naming its
    // two lists as short would describe a table the user never wrote.
    assert_eq!(
        rejected(&warnings),
        [String::from("scheme.scheme")],
        "the choice is reported and the absent table is not"
    );
}

#[test]
fn test_scheme_config_repaired_degrades_a_usable_custom_table() {
    // A table that is perfect still cannot be honoured, and the repair has to say so
    // rather than leave the document claiming a layout that is not running.
    let config = SchemeConfig {
        scheme: SchemeChoice::Custom,
        custom: custom_table(),
        ..SchemeConfig::default()
    };
    let (repaired, warnings) = config.repaired();
    assert_eq!(repaired.scheme, SchemeChoice::Full);
    assert_eq!(rejected(&warnings), [String::from("scheme.scheme")]);
}

#[test]
fn test_scheme_config_repaired_reports_a_custom_table_that_is_unused() {
    // The lists are checked whether or not the custom layout is the active one, the
    // way every other key of the configuration is: a mistake in the document is worth
    // telling the user about even while the key has no effect on the decode.
    assert!(custom_table().validate().is_empty());

    let broken = SchemeConfig {
        custom: CustomSchemeConfig {
            initials: vec![String::new(); CUSTOM_TABLE_KEYS - 1],
            ..custom_table()
        },
        ..SchemeConfig::default()
    };
    let (repaired, warnings) = broken.repaired();
    assert_eq!(repaired.scheme, SchemeChoice::Full);
    assert_eq!(
        rejected(&warnings),
        [String::from("scheme.custom.initials")]
    );
}

#[test]
fn test_custom_table_validate_checks_both_lists_against_the_key_alphabet() {
    let long = CustomSchemeConfig {
        finals: vec![String::new(); CUSTOM_TABLE_KEYS + 1],
        ..custom_table()
    };
    assert_eq!(
        rejected(&long.validate()),
        [String::from("scheme.custom.finals")]
    );

    // A table the document never wrote is not a table with a mistake in it: the
    // shipped default has no custom layout, and the default configuration has to be
    // clean. A table half written is the case that names a key.
    assert!(
        CustomSchemeConfig::default().validate().is_empty(),
        "an absent table is not a broken one"
    );
    let half = CustomSchemeConfig {
        initials: custom_table().initials,
        finals: Vec::new(),
    };
    assert_eq!(
        rejected(&half.validate()),
        [String::from("scheme.custom.finals")],
        "the list that was left out is the one named"
    );
}

#[test]
fn test_custom_table_validate_is_read_only() {
    let table = custom_table();
    assert!(table.validate().is_empty());
    assert_eq!(
        table,
        custom_table(),
        "validating a table does not change it"
    );
}

/// The configuration a document produces, failing the test when it cannot be read.
///
/// The section is pinned twice: as a value built by hand above, and as a document writes
/// it below. The second form is what a user actually has, and it is the only way to reach
/// the loader's answer to a name that is not a layout.
fn parsed_document(text: &str) -> (Config, Vec<ImeError>) {
    match Config::from_document(text) {
        Ok(parsed) => parsed,
        Err(error) => panic!("the document must be readable: {error}"),
    }
}

#[test]
fn test_scheme_section_degrades_an_unknown_name_to_full_pinyin() {
    let (config, warnings) = parsed_document("[scheme]\nscheme = \"shuangpin\"\n");

    assert_eq!(
        config.scheme.scheme,
        SchemeChoice::Full,
        "an unusable name leaves the default in force"
    );
    assert_eq!(rejected(&warnings), [String::from("scheme.scheme")]);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.to_string().contains("shuangpin")),
        "the reason quotes what the document said: {warnings:?}"
    );

    // The keys beside the broken one are still read: one mistyped layout name does not
    // cost the user the rest of the section.
    let (config, _) = parsed_document(
        "[scheme]\n\
         scheme = \"shuangpin\"\n\
         keep_full_pinyin = false\n",
    );
    assert!(!config.scheme.keep_full_pinyin);
    assert_eq!(config.scheme.decode_settings(), (SchemeId::FULL, false));
}

#[test]
fn test_scheme_section_reads_its_keys_from_a_document() {
    let (config, warnings) = parsed_document(
        "[scheme]\n\
         scheme = \"XiaoHe\"\n\
         show_hint = false\n\
         keep_full_pinyin = false\n",
    );

    assert!(warnings.is_empty(), "every value is usable: {warnings:?}");
    assert_eq!(
        config.scheme.scheme,
        SchemeChoice::Xiaohe,
        "the name is read in any case"
    );
    assert!(!config.scheme.show_hint);
    assert!(!config.scheme.keep_full_pinyin);
    assert_eq!(config.scheme.decode_settings(), (SchemeId::XIAOHE, false));
    assert_eq!(config.scheme.header_hint(), None);
}

#[test]
fn test_scheme_section_repairs_a_custom_table_a_document_writes_short() {
    // A table one entry short of the alphabet, beside one of the right length: the layout
    // cannot be used, and the list that is wrong is named as well as the choice that asked
    // for it. The second list is written out in full so that the report has to pick the
    // short one rather than naming both -- a document that leaves a list out entirely
    // names both, which is the other test's case.
    let short = vec!["\"a\""; CUSTOM_TABLE_KEYS - 1].join(", ");
    let full = vec!["\"a\""; CUSTOM_TABLE_KEYS].join(", ");
    let document = format!(
        "[scheme]\nscheme = \"custom\"\n[scheme.custom]\n\
         initials = [{short}]\n\
         finals = [{full}]\n"
    );
    let (config, warnings) = parsed_document(&document);

    assert_eq!(config.scheme.scheme, SchemeChoice::Full);
    assert_eq!(
        rejected(&warnings),
        [
            String::from("scheme.scheme"),
            String::from("scheme.custom.initials")
        ]
    );

    // A table of the right length is set aside all the same, because no build can carry
    // it: the only diagnostic is the choice, and the table raises nothing of its own.
    let document = format!(
        "[scheme]\nscheme = \"custom\"\n[scheme.custom]\n\
         initials = [{full}]\n\
         finals = [{full}]\n"
    );
    let (config, warnings) = parsed_document(&document);

    assert_eq!(config.scheme.scheme, SchemeChoice::Full);
    assert_eq!(rejected(&warnings), [String::from("scheme.scheme")]);
}

#[test]
fn test_scheme_config_decode_settings_matches_the_section() {
    let pairs: [(SchemeChoice, SchemeId); 6] = [
        (SchemeChoice::Full, SchemeId::FULL),
        (SchemeChoice::Xiaohe, SchemeId::XIAOHE),
        (SchemeChoice::Ziranma, SchemeId::ZIRANMA),
        (SchemeChoice::Microsoft, SchemeId::MICROSOFT),
        (SchemeChoice::Sogou, SchemeId::SOGOU),
        (SchemeChoice::Ziguang, SchemeId::ZIGUANG),
    ];
    for (choice, layout) in pairs {
        let section = SchemeConfig {
            scheme: choice,
            ..SchemeConfig::default()
        };
        assert_eq!(
            section.decode_settings(),
            (layout, true),
            "the projection of {choice:?}"
        );
    }

    // The boundary: a custom layout has no number of its own, so the decode runs full
    // pinyin whatever the mixed-input switch says.
    let custom = SchemeConfig {
        scheme: SchemeChoice::Custom,
        keep_full_pinyin: false,
        ..SchemeConfig::default()
    };
    assert_eq!(custom.decode_settings(), (SchemeId::FULL, false));
}

#[test]
fn test_scheme_config_header_hint_follows_the_show_hint_key() {
    let shown = SchemeConfig {
        scheme: SchemeChoice::Xiaohe,
        ..SchemeConfig::default()
    };
    assert_eq!(shown.header_hint(), Some("小鹤"));
    assert_eq!(
        SchemeConfig::default().header_hint(),
        Some("全拼"),
        "the header names the layout a user who never switched is typing in"
    );

    // The boundary: the hint is the user's to turn off, and `None` is what the engine
    // reads as "show your own label instead". Only the hint changes; the layout the
    // decode runs under is the one the section names either way.
    let hidden = SchemeConfig {
        scheme: SchemeChoice::Xiaohe,
        show_hint: false,
        ..SchemeConfig::default()
    };
    assert_eq!(hidden.header_hint(), None);
    assert_eq!(hidden.decode_settings(), (SchemeId::XIAOHE, true));
}
