//! What the projection makes of a configuration document.
//!
//! Everything here is in memory: a [`Config`] value is built, projected and compared, so no
//! test touches a configuration file, the environment, the clock or a display server. The
//! two properties worth pinning are the ones the routing layer depends on — that the
//! projection carries every setting the router acts on, and that a default view is exactly
//! what the shipped document projects to, so the two cannot drift apart.

use ime_config::KeyName;
use ime_config::keymap::{FlipSet, HighlightSet};
use ime_types::SchemeId;

use super::*;

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
