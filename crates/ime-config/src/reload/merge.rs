//! The section merge: what one document's keys overwrite, per section.
//!
//! Split out of `reload.rs` to keep that file inside the line limit. Every function here
//! copies one section's set keys over the defaults and reports what it had to reject;
//! what a key means when it is absent is the partial struct's `Option`, so a merge is
//! total and a reload of a partial document can never reset a section the document
//! did not mention.

use super::*;

/// Copies the `[engine]` keys the document sets over the defaults.
pub(super) fn merge_engine(
    partial: Option<PartialEngine>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    let engine = &mut config.engine;
    take_enum(partial.punct_mode, &mut engine.punct_mode, warnings);
    engine.full_width = partial.full_width.unwrap_or(engine.full_width);
    engine.auto_english_on_uppercase = partial
        .auto_english_on_uppercase
        .unwrap_or(engine.auto_english_on_uppercase);
    engine.passthrough_url = partial.passthrough_url.unwrap_or(engine.passthrough_url);
    engine.max_raw_len = partial.max_raw_len.unwrap_or(engine.max_raw_len);
    take_enum(
        partial.verify_dict_on_load,
        &mut engine.verify_dict_on_load,
        warnings,
    );
    engine.abbrev = partial.abbrev.unwrap_or(engine.abbrev);
    engine.correction = partial.correction.unwrap_or(engine.correction);
    engine.mixed_english = partial.mixed_english.unwrap_or(engine.mixed_english);
    take_enum(partial.on_im_switch, &mut engine.on_im_switch, warnings);
    engine.compose = partial.compose.unwrap_or(engine.compose);
    take_enum(partial.share_state, &mut engine.share_state, warnings);
}

/// Copies the `[fuzzy]` keys the document sets over the defaults.
///
/// The section is nine bools and nothing else, so there is no value to repair: a key
/// the document sets is a key that is usable, and the rule the section carries -- how
/// the master switch and the individual switches combine -- is the projection's, not
/// the load's.
pub(super) fn merge_fuzzy(partial: Option<PartialFuzzy>, config: &mut Config) {
    let Some(partial) = partial else { return };
    let fuzzy = &mut config.fuzzy;
    fuzzy.enabled = partial.enabled.unwrap_or(fuzzy.enabled);
    fuzzy.zh_z = partial.zh_z.unwrap_or(fuzzy.zh_z);
    fuzzy.ch_c = partial.ch_c.unwrap_or(fuzzy.ch_c);
    fuzzy.sh_s = partial.sh_s.unwrap_or(fuzzy.sh_s);
    fuzzy.n_l = partial.n_l.unwrap_or(fuzzy.n_l);
    fuzzy.f_h = partial.f_h.unwrap_or(fuzzy.f_h);
    fuzzy.an_ang = partial.an_ang.unwrap_or(fuzzy.an_ang);
    fuzzy.en_eng = partial.en_eng.unwrap_or(fuzzy.en_eng);
    fuzzy.in_ing = partial.in_ing.unwrap_or(fuzzy.in_ing);
}

/// Copies the `[phrases]` keys the document sets over the defaults.
///
/// `max_entries` is narrowed with `try_into` rather than `as`: a document may state a
/// number wider than the field, and a silent truncation would turn a too-large limit into
/// a small one. A value the field cannot hold is left at its default and reported.
pub(super) fn merge_phrases(
    partial: Option<PartialPhrases>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    let phrases = &mut config.phrases;
    phrases.enabled = partial.enabled.unwrap_or(phrases.enabled);
    if let Some(file) = partial.file {
        phrases.file = file;
    }
    if let Some(entries) = partial.max_entries {
        match u32::try_from(entries) {
            Ok(entries) => phrases.max_entries = entries,
            Err(_) => warnings.report_limit(PHRASE_ENTRIES_KEY, MAX_PHRASE_ENTRIES as usize),
        }
    }
}

/// Copies the `[scheme]` keys the document sets over the defaults.
///
/// The custom table is replaced whole rather than merged key by key: a table is a unit,
/// and taking one list from the document and the other from the default would build a
/// table neither side wrote.
pub(super) fn merge_scheme(
    partial: Option<PartialScheme>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    let scheme = &mut config.scheme;
    take_enum(partial.scheme, &mut scheme.scheme, warnings);
    scheme.show_hint = partial.show_hint.unwrap_or(scheme.show_hint);
    scheme.keep_full_pinyin = partial.keep_full_pinyin.unwrap_or(scheme.keep_full_pinyin);
    if let Some(custom) = partial.custom {
        if let Some(initials) = custom.initials {
            scheme.custom.initials = initials;
        }
        if let Some(finals) = custom.finals {
            scheme.custom.finals = finals;
        }
    }
}

/// Copies the `[ui]` keys the document sets over the defaults.
///
/// No key of this section can be rejected: every one of them is a number or a flag, and
/// a value the field cannot hold fails the parse rather than reaching this function.
pub(super) fn merge_ui(partial: Option<PartialUi>, config: &mut Config) {
    let Some(partial) = partial else { return };
    let ui = &mut config.ui;
    ui.client_preedit = partial.client_preedit.unwrap_or(ui.client_preedit);
    ui.max_per_row = partial.max_per_row.unwrap_or(ui.max_per_row);
    ui.show_annotation = partial.show_annotation.unwrap_or(ui.show_annotation);
    ui.max_width_dp = partial.max_width_dp.unwrap_or(ui.max_width_dp);
    ui.corner_radius_dp = partial.corner_radius_dp.unwrap_or(ui.corner_radius_dp);
    ui.base_alpha = partial.base_alpha.unwrap_or(ui.base_alpha);
    let Some(animation) = partial.animation else {
        return;
    };
    let animation_config = &mut ui.animation;
    animation_config.enabled = animation.enabled.unwrap_or(animation_config.enabled);
    // A TOML float is an `f64`; the cast saturates, and a saturated value lies outside
    // the key's documented range, so the repair reports the key.
    if let Some(value) = animation.omega0 {
        animation_config.omega0 = value as f32;
    }
    if let Some(value) = animation.zeta {
        animation_config.zeta = value as f32;
    }
    animation_config.appear_ms = animation.appear_ms.unwrap_or(animation_config.appear_ms);
    animation_config.disappear_ms = animation
        .disappear_ms
        .unwrap_or(animation_config.disappear_ms);
}

/// Copies the `[theme]` keys the document sets over the defaults.
pub(super) fn merge_theme(
    partial: Option<PartialTheme>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    take_enum(partial.scheme, &mut config.theme.scheme, warnings);
    if let Some(accent) = partial.accent {
        match Rgb::try_from(accent.as_str()) {
            Ok(colour) => config.theme.accent = colour,
            Err(error) => warnings.report_error(error),
        }
    }
}

/// Copies the `[keys]` keys the document sets over the defaults.
pub(super) fn merge_keys(
    partial: Option<PartialKeys>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    let keys = &mut config.keys;
    take_enum(partial.digit_zero, &mut keys.digit_zero, warnings);
    keys.enter_commit_raw = partial.enter_commit_raw.unwrap_or(keys.enter_commit_raw);
    if let Some(names) = partial.flip_keys {
        keys.flip_keys = key_names(names, KEY_FLIP_KEYS, warnings);
    }
    if let Some(names) = partial.highlight_keys {
        keys.highlight_keys = key_names(names, KEY_HIGHLIGHT_KEYS, warnings);
    }
}

/// Copies the `[data]` keys the document sets over the defaults.
pub(super) fn merge_data(
    partial: Option<PartialData>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    take_enum(partial.durability, &mut config.data.durability, warnings);
    // `backup_keep` is copied as written rather than narrowed here: the range check is
    // `Config::repaired`'s, which owns every scalar rule and reports the key name, and a
    // second check here would be a second place the bound is written down.
    config.data.backup_enabled = partial.backup_enabled.unwrap_or(config.data.backup_enabled);
    config.data.backup_keep = partial.backup_keep.unwrap_or(config.data.backup_keep);
    if let Some(written) = partial.decay_half_life_ticks {
        match u32::try_from(written) {
            Ok(half_life) if half_life > 0 => config.data.decay_half_life_ticks = half_life,
            _ => warnings.report_error(ConfigError::Invalid {
                key: String::from("data.decay_half_life_ticks"),
                reason: format!("{written} is not a usable commit count"),
            }),
        }
    }
    if let Some(floor) = partial.decay_floor {
        config.data.decay_floor = floor;
    }
    // The staged path is copied as written: it names a file, and judging the file is
    // the swap's business, not the reader's. An empty value is the shipped state, not
    // an error.
    if let Some(staged) = partial.dict_staged_path {
        config.data.dict_staged_path = staged;
    }
}

/// Copies the `[diagnostics]` keys the document sets over the defaults.
pub(super) fn merge_diagnostics(
    partial: Option<PartialDiagnostics>,
    config: &mut Config,
    warnings: &mut Warnings,
) {
    let Some(partial) = partial else { return };
    let diagnostics = &mut config.diagnostics;
    take_enum(partial.level, &mut diagnostics.level, warnings);
    diagnostics.log_rotation_mb = partial
        .log_rotation_mb
        .unwrap_or(diagnostics.log_rotation_mb);
    diagnostics.log_keep_files = partial.log_keep_files.unwrap_or(diagnostics.log_keep_files);
    diagnostics.log_input_content = partial
        .log_input_content
        .unwrap_or(diagnostics.log_input_content);
    diagnostics.probes = partial.probes.unwrap_or(diagnostics.probes);
}
