//! What the projection makes of a configuration document.
//!
//! Everything here is in memory: a [`Config`] value is built, projected and compared, so no
//! test touches a configuration file, the environment, the clock or a display server. The
//! two properties worth pinning are the ones the routing layer depends on — that the
//! projection carries every setting the router acts on, and that a default view is exactly
//! what the shipped document projects to, so the two cannot drift apart.
//!
//! The layout-file tests are the one exception to "no file": the section names a document
//! by path, so reading it back is the behaviour under test. Each test owns its scratch
//! directory under the system temp directory and passes an absolute path, so nothing here
//! reads the configuration environment.

use std::env;
use std::fs;

use ime_config::KeyName;
use ime_config::keymap::{FlipSet, HighlightSet};
use ime_core::viterbi::context::ContextBand;
use ime_types::{ImeError, SchemeId};

use super::*;

/// The Xiaohe layout document, the fixture a user layout is modelled on: a real
/// published layout, taken through the file form the section documents.
const USER_LAYOUT_DOCUMENT: &str =
    include_str!("../../../../../ime-core/src/shuangpin/data/xiaohe.toml");

/// A configuration whose two binding lists are `flip` and `highlight`, everything else at
/// its shipped default.
///
/// The shape the shipped document has, so a test only has to name the list it is about.
fn keys_with(flip: Vec<KeyName>, highlight: Vec<KeyName>) -> Config {
    let mut config = Config::default();
    config.keys.flip_keys = flip;
    config.keys.highlight_keys = highlight;
    config
}

#[test]
fn test_from_config_projects_the_shipped_document_onto_the_default_view() {
    // The drift guard: a caller with no configuration yet must route, fill the preedit area
    // and label the window exactly as a fresh installation does, so the written-out default
    // and the projection of the built-in document have to be one value.
    let (projected, warnings) = RoutingConfig::from_config(&Config::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(projected, RoutingConfig::default());
}

#[test]
fn test_from_config_carries_every_setting_the_routing_layer_acts_on() {
    let mut config = Config::default();
    config.keys.enter_commit_raw = true;
    config.ui.client_preedit = true;
    config.ui.max_per_row = 9;
    config.ui.max_width_dp = 900;
    config.ui.show_annotation = false;
    config.engine.max_raw_len = 32;
    config.engine.auto_english_on_uppercase = false;
    config.engine.passthrough_url = false;
    config.scheme.scheme = SchemeChoice::Xiaohe;
    config.scheme.keep_full_pinyin = false;

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        projected.keys.enter_commit_raw,
        "`[keys]` reached the table"
    );
    assert!(projected.client_preedit, "`[ui]` reached the executor");
    assert_eq!(projected.session.max_per_row, 9);
    assert_eq!(projected.session.max_width_dp, 900);
    assert!(!projected.session.show_annotation);
    assert_eq!(projected.session.max_raw_len, 32);
    assert_eq!(projected.session.scheme, SchemeId::XIAOHE);
    assert!(!projected.session.keep_full_pinyin);
    assert!(
        !projected.auto_english_on_uppercase,
        "`[engine]` reached the passthrough policy"
    );
    assert!(!projected.passthrough_url);
    assert_eq!(
        projected.scheme_hint,
        Some("小鹤"),
        "the header names the layout the document declares"
    );
}

#[test]
fn test_from_config_projects_context_tie_band_onto_the_session() {
    // The shipped Q16.16 width is pinned in both layers so a silent default drift cannot
    // change re-ranking while the configuration still parses.
    let (projected, warnings) = RoutingConfig::from_config(&Config::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(projected.session.context_band, ContextBand::default());
    assert_eq!(projected.session.context_band.q16, 1 << 8);

    // A repaired caller may still hand over a bypassed bad value; the projection falls
    // back to the shipped band rather than making zero a legal width.
    let mut config = Config::default();
    config.data.context_tie_band_q16 = 0;
    let (projected, _) = RoutingConfig::from_config(&config);
    assert_eq!(projected.session.context_band, ContextBand::default());

    config.data.context_tie_band_q16 = 1 << 12;
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        projected.session.context_band,
        ContextBand::new(1 << 12).expect("a positive width builds")
    );
}

#[test]
fn test_from_config_reads_the_binding_lists_as_flag_sets() {
    let config = keys_with(
        vec![KeyName::Minus, KeyName::PageDown],
        vec![KeyName::ShiftTab, KeyName::Right],
    );

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        projected.keys.flip_keys,
        FlipSet::MINUS | FlipSet::PAGE_DOWN
    );
    assert_eq!(
        projected.keys.highlight_keys,
        HighlightSet::SHIFT_TAB | HighlightSet::RIGHT
    );
}

#[test]
fn test_from_config_routes_every_name_the_whitelist_offers() {
    // Every name the two lists accept has to become a binding in the list that can carry
    // it: a name the configuration takes and the routing table then drops is the defect the
    // flag sets exist to close, and the eight pageable and six highlightable names are the
    // whole of what `keys.flip_keys` and `keys.highlight_keys` can hold.
    let (pages, page_warnings) = RoutingConfig::from_config(&keys_with(
        vec![
            KeyName::Minus,
            KeyName::Equal,
            KeyName::Up,
            KeyName::Down,
            KeyName::PageUp,
            KeyName::PageDown,
            KeyName::Home,
            KeyName::End,
        ],
        Vec::new(),
    ));
    assert!(page_warnings.is_empty(), "{page_warnings:?}");
    assert_eq!(
        pages.keys.flip_keys,
        FlipSet::MINUS
            | FlipSet::EQUAL
            | FlipSet::UP
            | FlipSet::DOWN
            | FlipSet::PAGE_UP
            | FlipSet::PAGE_DOWN
            | FlipSet::HOME
            | FlipSet::END
    );

    let (highlights, highlight_warnings) = RoutingConfig::from_config(&keys_with(
        Vec::new(),
        vec![
            KeyName::Tab,
            KeyName::ShiftTab,
            KeyName::Up,
            KeyName::Down,
            KeyName::Left,
            KeyName::Right,
        ],
    ));
    assert!(highlight_warnings.is_empty(), "{highlight_warnings:?}");
    assert_eq!(
        highlights.keys.highlight_keys,
        HighlightSet::TAB
            | HighlightSet::SHIFT_TAB
            | HighlightSet::UP
            | HighlightSet::DOWN
            | HighlightSet::LEFT
            | HighlightSet::RIGHT
    );
}

#[test]
fn test_from_config_keeps_the_highlight_binding_of_a_key_both_lists_name() {
    // One key cannot page the list and move the highlight at once. The projection drops the
    // page binding and reports the collision, which is the precedence the routing table
    // applies when it reads the two sets, so the table and the document answer alike.
    let config = keys_with(vec![KeyName::Up, KeyName::Down], vec![KeyName::Up]);

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert_eq!(
        projected.keys.flip_keys,
        FlipSet::DOWN,
        "the page binding is the one that gives way"
    );
    assert_eq!(projected.keys.highlight_keys, HighlightSet::UP);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let reported = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        reported.contains("keys/binding-conflict"),
        "the collision is reported: {reported}"
    );
}

#[test]
fn test_from_config_reports_a_binding_the_routing_table_cannot_use() {
    // `left` is a name the configuration's whitelist accepts and a page list has no binding
    // for. The projection drops it and says so rather than routing nothing in silence.
    let config = keys_with(vec![KeyName::Left], vec![KeyName::Tab]);

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert_eq!(projected.keys.flip_keys, FlipSet::empty());
    assert_eq!(projected.keys.highlight_keys, HighlightSet::TAB);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let reported = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        reported.contains("keys/unroutable-binding"),
        "the unusable entry is reported: {reported}"
    );
}

#[test]
fn test_from_config_turns_the_scheme_hint_off_when_the_document_asks() {
    let mut config = Config::default();
    config.scheme.scheme = SchemeChoice::Ziranma;
    config.scheme.show_hint = false;

    let (projected, _warnings) = RoutingConfig::from_config(&config);

    assert_eq!(
        projected.scheme_hint, None,
        "a user who finds the label noise gets the engine's own label instead"
    );
    assert_eq!(
        projected.session.scheme,
        SchemeId::ZIRANMA,
        "the layout still governs the decode"
    );
}

#[test]
fn test_from_config_clamps_a_raw_length_limit_the_schema_forbids() {
    // The projection is the last step before the session sees the value, so a zero limit
    // cannot reach it: a session without a length limit at all is not a configuration
    // `ime-config` can produce, and this is what keeps a caller that bypassed the repair
    // rules from handing one over.
    let mut config = Config::default();
    config.engine.max_raw_len = 0;

    let (projected, _warnings) = RoutingConfig::from_config(&config);

    assert_eq!(projected.session.max_raw_len, 1);
}

#[test]
fn test_from_config_projects_the_fuzzy_section_onto_the_session_mask() {
    // One class with the master switch off: the projection sets the master bit
    // beside the class, because the decode gate needs the two to agree.
    let mut config = Config::default();
    config.fuzzy.zh_z = true;
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        projected
            .session
            .fuzzy
            .contains(ime_types::DecodeFlags::FUZZY_ZH_Z)
    );
    assert!(
        projected
            .session
            .fuzzy
            .contains(ime_types::DecodeFlags::FUZZY),
        "one class switches the master on"
    );
    assert!(
        !projected
            .session
            .fuzzy
            .contains(ime_types::DecodeFlags::FUZZY_N_L),
        "a class the section did not name stays off"
    );

    // The master switch alone selects every class.
    let mut config = Config::default();
    config.fuzzy.enabled = true;
    let (projected, _) = RoutingConfig::from_config(&config);
    assert!(
        projected
            .session
            .fuzzy
            .contains(ime_types::DecodeFlags::FUZZY_F_H)
    );

    // The shipped document selects nothing, and the session decodes as a build
    // before the section existed.
    let (projected, _) = RoutingConfig::from_config(&Config::default());
    assert_eq!(projected.session.fuzzy, ime_types::DecodeFlags::empty());
}

#[test]
fn test_from_config_carries_the_abbrev_switch() {
    let mut config = Config::default();
    assert!(
        !RoutingConfig::from_config(&config).0.session.abbrev,
        "the shipped document leaves abbreviations off"
    );
    config.engine.abbrev = true;
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(projected.session.abbrev, "the switch reaches the session");
}

#[test]
fn test_from_config_projects_the_output_bits_a_context_starts_from() {
    // The shipped document starts half width with Chinese marks, which is what a
    // context with no configuration has always started from.
    let (projected, warnings) = RoutingConfig::from_config(&Config::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        projected.output,
        super::super::modes::OutputProjection {
            full_width: false,
            punct_full: true,
        }
    );

    // A document that says otherwise starts new contexts its own way.
    let mut config = Config::default();
    config.engine.full_width = true;
    config.engine.punct_mode = ime_config::PunctMode::English;
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(projected.output.full_width);
    assert!(!projected.output.punct_full);
}

/// The `config/invalid` key of every diagnostic the projection raised.
fn rejected_keys(warnings: &[ImeError]) -> Vec<&str> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            ImeError::ConfigInvalid { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect()
}

/// The one-character key name a table slot is written under in a layout document.
fn scheme_key_name(slot: usize) -> String {
    if slot == ime_core::shuangpin::SEMICOLON_INDEX {
        String::from(";")
    } else {
        char::from(b'a' + slot as u8).to_string()
    }
}

/// A configuration whose `[shuangpin.custom]` section holds the Xiaohe table as the
/// plain maps the inline form carries: the round trip a user's layout takes through
/// the schema, from a table this build ships back to the document shape a user
/// writes.
fn config_with_inline_xiaohe(zero_initial_style: &str) -> Config {
    let table = ime_core::shuangpin::xiaohe::table();
    let mut config = Config::default();
    let section = &mut config.shuangpin.custom;
    section.name = table.name.clone();
    section.zero_initial_style = String::from(zero_initial_style);
    for slot in 0..ime_core::shuangpin::SCHEME_KEYS {
        let key = scheme_key_name(slot);
        if let Some(initial) = &table.initials[slot] {
            section
                .initials
                .insert(key.clone(), String::from(initial.as_ref()));
        }
        if !table.finals[slot].is_empty() {
            let finals = table.finals[slot]
                .iter()
                .map(|final_text| final_text.as_ref())
                .collect::<Vec<&str>>()
                .join(" ");
            section.finals.insert(key.clone(), finals);
        }
        if !table.standalone[slot].is_empty() {
            section
                .standalone
                .insert(key.clone(), String::from(table.standalone[slot].as_ref()));
        }
    }
    config
}

#[test]
fn test_from_config_compiles_the_inline_custom_layout_into_the_session_slot() {
    // The chain the card asks for, at the projection: a user's inline three-table
    // layout compiles once, here, and the session slot carries the compiled table.
    // The selection beside it stays full pinyin, because a user layout has no number
    // in the frozen scheme numbering and travels in its own slot instead.
    let config = config_with_inline_xiaohe("first-letter");

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(warnings.is_empty(), "{warnings:?}");
    let table = projected
        .session
        .custom_layout
        .expect("the user's layout compiled into the slot");
    assert_eq!(table.name, "小鹤双拼");
    // The compiled table answers the keys, and Xiaohe spells `zhong` as `v` `s`.
    let mapped = ime_core::shuangpin::map_syllable(table, b"vs")
        .ok()
        .flatten()
        .map(|mapped| mapped.full);
    assert_eq!(
        mapped.and_then(ime_core::segment::syllable_at),
        Some("zhong"),
        "the slot holds a working layout, not an empty table"
    );
    assert_eq!(
        projected.scheme_hint,
        Some("小鹤双拼"),
        "the header names the user's layout by its own name"
    );
    assert_eq!(projected.session.scheme, SchemeId::FULL);
}

#[test]
fn test_from_config_keeps_the_hint_off_when_the_document_asks() {
    let mut config = config_with_inline_xiaohe("first-letter");
    config.scheme.show_hint = false;

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        projected.session.custom_layout.is_some(),
        "the hint switch is not the loader's"
    );
    assert_eq!(projected.scheme_hint, None);
}

#[test]
fn test_from_config_answers_a_refused_layout_with_the_selected_scheme() {
    // A layout the validator refuses never half-loads: the slot stays empty -- which
    // is the fallback to the scheme the document selected -- and each error is one
    // `config/invalid` diagnostic under the loader's own key.
    let mut config = Config::default();
    config.scheme.scheme = SchemeChoice::Xiaohe;
    let section = &mut config.shuangpin.custom;
    section.name = String::from("残缺布局");
    section.zero_initial_style = String::from("first-letter");
    // No initials and no finals: the two tables a layout cannot live without are
    // both missing, so the refusal carries a diagnostic each.

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(
        projected.session.custom_layout.is_none(),
        "a refused layout is not active"
    );
    assert_eq!(
        projected.session.scheme,
        SchemeId::XIAOHE,
        "the selected scheme is what answers the keys"
    );
    assert_eq!(
        projected.scheme_hint,
        Some("小鹤"),
        "the header names the scheme that is actually answering"
    );
    let keys = rejected_keys(&warnings);
    assert!(
        !keys.is_empty() && keys.iter().all(|key| *key == "shuangpin.custom.layout"),
        "one diagnostic per error, under the loader's key: {keys:?}"
    );
    let rendered = warnings
        .iter()
        .map(|warning| warning.to_string())
        .collect::<Vec<String>>()
        .join("\n");
    assert!(
        rendered.contains("[initials]") && rendered.contains("[finals]"),
        "each missing table is named: {rendered}"
    );
}

#[test]
fn test_from_config_leaves_the_slot_empty_when_the_section_names_nothing() {
    // The shipped state: no file, no inline tables, so no layout exists to load and
    // the loader is never consulted -- which is why the projection raises nothing.
    let (projected, warnings) = RoutingConfig::from_config(&Config::default());

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(projected.session.custom_layout.is_none());
    assert_eq!(projected.scheme_hint, Some("全拼"));
}

#[test]
fn test_from_config_compiles_the_layout_file_the_section_names() {
    let directory = env::temp_dir().join("rspinyin-router-config-layout-file");
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    let path = directory.join("my-layout.toml");
    assert!(
        fs::write(&path, USER_LAYOUT_DOCUMENT).is_ok(),
        "the layout file"
    );

    let mut config = Config::default();
    config.shuangpin.custom.file = path.display().to_string();

    let (projected, warnings) = RoutingConfig::from_config(&config);

    assert!(warnings.is_empty(), "{warnings:?}");
    let table = projected
        .session
        .custom_layout
        .expect("the named document compiled");
    assert_eq!(
        table.name, "小鹤双拼",
        "the file's own name travels with it"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn test_from_config_reports_a_layout_file_it_cannot_load() {
    let directory = env::temp_dir().join("rspinyin-router-config-layout-broken");
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    let path = directory.join("broken.toml");
    assert!(
        fs::write(&path, "not a layout\n").is_ok(),
        "the broken file"
    );

    // A document that is not a layout is the loader's parse diagnostic, under the
    // key every layout error carries; the slot stays empty and the selection stands.
    let mut config = Config::default();
    config.shuangpin.custom.file = path.display().to_string();
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(projected.session.custom_layout.is_none());
    assert_eq!(
        rejected_keys(&warnings),
        ["shuangpin.custom.layout"],
        "{warnings:?}"
    );

    // A file that is not there at all is the section's own diagnostic, naming the
    // key the path was read from.
    let mut config = Config::default();
    config.shuangpin.custom.file = directory.join("absent.toml").display().to_string();
    let (projected, warnings) = RoutingConfig::from_config(&config);
    assert!(projected.session.custom_layout.is_none());
    assert_eq!(
        rejected_keys(&warnings),
        ["shuangpin.custom.file"],
        "{warnings:?}"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn test_from_config_makes_the_file_the_layout_when_both_forms_are_named() {
    // Naming the file and the inline tables at once is an ambiguity worth a line,
    // never a silence: the file is the layout, and the tables the user also wrote
    // are ignored and told so.
    let directory = env::temp_dir().join("rspinyin-router-config-layout-both");
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    let path = directory.join("my-layout.toml");
    assert!(
        fs::write(&path, USER_LAYOUT_DOCUMENT).is_ok(),
        "the layout file"
    );

    let mut config = Config::default();
    let section = &mut config.shuangpin.custom;
    section.file = path.display().to_string();
    section.name = String::from("内联布局");

    let (projected, warnings) = RoutingConfig::from_config(&config);

    let table = projected
        .session
        .custom_layout
        .expect("the file compiled despite the ambiguity");
    assert_eq!(
        table.name, "小鹤双拼",
        "the file is the layout, not the inline name"
    );
    assert_eq!(
        rejected_keys(&warnings),
        ["shuangpin.custom"],
        "the ambiguity is reported: {warnings:?}"
    );
    let _ = fs::remove_dir_all(&directory);
}
