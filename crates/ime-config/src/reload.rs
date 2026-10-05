//! Reading, writing and reloading the configuration file.
//!
//! Responsibility: everything that touches the filesystem -- reading
//! `$XDG_CONFIG_HOME/rspinyin/config.toml`, writing the documented default when there
//! is none, moving a file that cannot be parsed aside, and re-reading the file when
//! the host asks for it. The model lives in `crate::schema`; this module is the only
//! place that knows a file exists.
//!
//! It is also where the two halves of a reload meet: the `[keys]` section of the
//! document that is adopted is projected into the routing layer's binding table
//! (`crate::keymap`) at that same moment, and both are handed out by
//! [`ConfigStore`]. A table in force and the configuration in force therefore always
//! come from one document, and neither can be observed half-replaced.
//!
//! # How a reload is triggered
//!
//! fcitx5 calls `reloadConfig()` on an addon whose configuration changed, and
//! [`ConfigStore::reload`] is what that callback calls. There is deliberately no
//! filesystem watcher: the host already owns the trigger, and a watcher would add a
//! dependency and a background thread for a callback that is a few hundred
//! microseconds of parsing.
//!
//! # What a reload may not do
//!
//! A reload may improve the configuration and nothing else. It never writes to the
//! file, never moves a file aside, and never falls back to the defaults: when the file
//! cannot be read or parsed, the configuration in force is kept and the reason is
//! reported. The other half of 0.4 rule 10 is that the configuration is handed out as
//! an `Arc` that is replaced rather than mutated, so a component in the middle of a
//! composition keeps the settings it started with.
//!
//! # How the module is laid out
//!
//! This file holds the document model: the sections as a user writes them, the merge
//! of one document over the built-in defaults, and the text of the template a fresh
//! install is given. The two halves that act on that model sit beside it. The `load`
//! submodule owns every touch of the filesystem -- where the file is, how it is read,
//! and what is written when it is not there. The `store` submodule owns the
//! configuration in force and the reload path. Both submodules are private, and what
//! they hand out is re-exported below, so the public path of every item is the one it
//! has always had.

mod load;
mod store;

pub use load::default_path;
pub use store::{ConfigStore, ReloadOutcome};

use std::path::Path;

use ime_types::{CONFIG_SCHEMA_VERSION, ConfigError, ImeError};
use serde::Deserialize;

use crate::migrate;
use crate::schema::{
    Config, KEY_FLIP_KEYS, KEY_HIGHLIGHT_KEYS, KeyName, MAX_DOCUMENT_KEYS, MAX_PHRASE_ENTRIES, Rgb,
    Warnings,
};

/// The name of the configuration file inside the configuration directory.
pub const FILE_NAME: &str = "config.toml";

/// The key name reported for a failure that concerns the whole document rather than
/// one of its keys.
const DOCUMENT_KEY: &str = "config";

/// The key a document declares its schema version under.
const SCHEMA_VERSION_KEY: &str = "schema_version";

/// The key name reported when `[phrases] max_entries` states a number the field cannot
/// hold. A silent truncation would turn a too-large limit into a small one.
const PHRASE_ENTRIES_KEY: &str = "phrases.max_entries";

/// How many `.1`, `.2`, ... suffixes are tried before a backup name is given up on.
const MAX_BACKUP_ATTEMPTS: u32 = 100;

/// The configuration a user who has none is given, and the documentation of every key.
///
/// This is what the loader writes when there is no `config.toml` yet: a user finds the
/// available keys by reading the file rather than by reading the crate. It is also the
/// statement of the built-in defaults -- a test parses it and requires the result to
/// equal `Config::default`, so the document and the defaults cannot drift apart.
///
/// The design places this document in `config/default.toml` and pulls it in with
/// `include_str!`; it is inlined here so that the crate compiles from
/// `crates/ime-config` alone, with no build-time dependency on a repository path.
pub const DEFAULT_CONFIG_TOML: &str = r##"# rspinyin configuration.
#
# Every key is optional: an absent key keeps its built-in default, which is the value
# written here. The file is read at startup and again whenever the host asks the addon
# to reload. A file that cannot be read or parsed never stops the input method.

# Format version. Only 2 is understood; a file written by an older build is migrated
# once, with the original kept beside it.
schema_version = 2

[engine]
# "chinese" replaces ASCII punctuation with its Chinese mark; "english" leaves it to
# the application.
punct_mode = "chinese"
# Widen the committed text of ASCII characters.
full_width = false
# Commit a leading uppercase letter instead of decoding it, so typing English needs no
# mode switch.
auto_english_on_uppercase = true
# Leave the keys to the application while the caret is inside a URL or an email.
passthrough_url = true
# Hard limit on the length of the raw input, 1..=64.
max_raw_len = 64
# "full" verifies the whole dictionary when it is loaded, "header" only its header.
verify_dict_on_load = "full"
# Expand initial-letter abbreviations, so that `nh` reaches `你好`. Off by default: an
# abbreviation is ambiguous by nature.
abbrev = false

[ui]
# Show the composing text in the application's preedit area instead of the window.
client_preedit = false
# Candidates per row, 3..=9.
max_per_row = 5
# Show the word annotation beside each candidate.
show_annotation = true
# Widest candidate window, 220..=1200 dp; corner radius, 8..=20 dp.
max_width_dp = 720
corner_radius_dp = 12
# Background opacity, 0..=255. The default is 0.85.
base_alpha = 217

[ui.animation]
# Animate the window with a spring: angular frequency in rad/s, 4.0..=80.0, and the
# damping ratio, 0.3..=2.0. The durations are 0..=600 ms.
enabled = true
omega0 = 26.0
zeta = 0.85
appear_ms = 110
disappear_ms = 90

[theme]
# "auto" follows the desktop's colour scheme; "light" and "dark" pin it.
scheme = "auto"
# Accent colour, written as #RRGGBB.
accent = "#4C9AFF"

[keys]
# What the "0" key does once the candidate list reaches ten entries: "passthrough"
# sends the digit to the application, "flip" turns the page.
digit_zero = "passthrough"
# Commit the raw input on Enter instead of the highlighted candidate.
enter_commit_raw = false
# Keys that page the candidate list, at most six, and keys that move the highlight.
# "home" jumps to the first page and "end" to the last.
flip_keys = ["minus", "equal", "up", "down", "home", "end"]
highlight_keys = ["tab", "shift_tab"]

[scheme]
# The layout the keystrokes follow: "full" is plain pinyin, and "xiaohe", "ziranma",
# "microsoft", "sogou" and "ziguang" are the double-pinyin layouts.
scheme = "full"
# Name the active layout in the candidate window's header.
show_hint = true
# Still read a full-pinyin syllable typed while a double-pinyin layout is active.
keep_full_pinyin = true

[phrases]
# Let the phrase table take part in a decode. With it off the dictionary answers alone.
enabled = true
# The phrase document to read. Empty means the default location under the user's
# configuration directory.
file = ""
# How many entries the table may hold, 1..=50000.
max_entries = 5000

[data]
# "eventual" batches user-frequency writes; "immediate" flushes each one.
durability = "eventual"
# Copy the user's learned words automatically. On by default: the store cannot be
# rebuilt from anywhere else.
backup_enabled = true
# How many backup generations are kept, 1..=32. The oldest is removed once a new one
# has landed.
backup_keep = 3

[diagnostics]
# "error" | "warn" | "info" | "debug" | "trace".
level = "info"
log_rotation_mb = 8
log_keep_files = 3
# Accepted, and deliberately without effect on what is recorded: the plugin never
# writes the characters you type to the log, whether this is false or true.
log_input_content = false
# Switch the diagnostic probes on.
probes = true
"##;

/// The document as the user wrote it: every key optional, every value as the file
/// spells it.
///
/// A separate type from [`Config`] on purpose. The document carries *what the user
/// asked for*, including values that turn out to be unusable, while `Config` carries
/// only values that passed; keeping them apart is what lets the loader report a bad key
/// and carry on rather than refusing the file.
#[derive(Deserialize)]
struct PartialConfig {
    schema_version: Option<i64>,
    engine: Option<PartialEngine>,
    ui: Option<PartialUi>,
    theme: Option<PartialTheme>,
    keys: Option<PartialKeys>,
    scheme: Option<PartialScheme>,
    data: Option<PartialData>,
    diagnostics: Option<PartialDiagnostics>,
    phrases: Option<PartialPhrases>,
}

/// The `[engine]` table of a document.
#[derive(Deserialize)]
struct PartialEngine {
    punct_mode: Option<String>,
    full_width: Option<bool>,
    auto_english_on_uppercase: Option<bool>,
    passthrough_url: Option<bool>,
    max_raw_len: Option<u8>,
    verify_dict_on_load: Option<String>,
    abbrev: Option<bool>,
}

/// The `[scheme]` table of a document.
#[derive(Deserialize)]
struct PartialScheme {
    scheme: Option<String>,
    show_hint: Option<bool>,
    keep_full_pinyin: Option<bool>,
    custom: Option<PartialCustomScheme>,
}

/// The `[scheme.custom]` table of a document.
#[derive(Deserialize)]
struct PartialCustomScheme {
    initials: Option<Vec<String>>,
    finals: Option<Vec<String>>,
}

/// The `[phrases]` table of a document.
#[derive(Deserialize)]
struct PartialPhrases {
    enabled: Option<bool>,
    file: Option<String>,
    max_entries: Option<u64>,
}

/// The `[ui]` table of a document.
#[derive(Deserialize)]
struct PartialUi {
    client_preedit: Option<bool>,
    max_per_row: Option<u8>,
    show_annotation: Option<bool>,
    max_width_dp: Option<u16>,
    corner_radius_dp: Option<u8>,
    base_alpha: Option<u8>,
    animation: Option<PartialAnimation>,
}

/// The `[ui.animation]` table of a document.
#[derive(Deserialize)]
struct PartialAnimation {
    enabled: Option<bool>,
    omega0: Option<f64>,
    zeta: Option<f64>,
    appear_ms: Option<u16>,
    disappear_ms: Option<u16>,
}

/// The `[theme]` table of a document.
#[derive(Deserialize)]
struct PartialTheme {
    scheme: Option<String>,
    accent: Option<String>,
}

/// The `[keys]` table of a document.
#[derive(Deserialize)]
struct PartialKeys {
    digit_zero: Option<String>,
    enter_commit_raw: Option<bool>,
    flip_keys: Option<Vec<String>>,
    highlight_keys: Option<Vec<String>>,
}

/// The `[data]` table of a document.
#[derive(Deserialize)]
struct PartialData {
    durability: Option<String>,
    backup_enabled: Option<bool>,
    backup_keep: Option<u8>,
}

/// The `[diagnostics]` table of a document.
#[derive(Deserialize)]
struct PartialDiagnostics {
    level: Option<String>,
    log_rotation_mb: Option<u32>,
    log_keep_files: Option<u8>,
    log_input_content: Option<bool>,
    probes: Option<bool>,
}

/// The `config/invalid` diagnostic for a document that cannot be read at all.
///
/// The key is the document itself rather than one of its keys: a file that is not TOML
/// has no key to point at.
fn document_error(reason: String) -> ConfigError {
    ConfigError::Invalid {
        key: String::from(DOCUMENT_KEY),
        reason,
    }
}

/// Copies a value from a closed set of spellings, reporting one outside the set.
///
/// The key a rejection names comes from the type's own `TryFrom`, so a value is always
/// reported against the key it was read from.
fn take_enum<T>(raw: Option<String>, slot: &mut T, warnings: &mut Warnings)
where
    T: for<'a> TryFrom<&'a str, Error = ConfigError>,
{
    if let Some(raw) = raw {
        match T::try_from(raw.as_str()) {
            Ok(value) => *slot = value,
            Err(error) => warnings.report_error(error),
        }
    }
}

/// Parses one configured key-binding list.
///
/// An entry outside the whitelist is dropped and reported rather than costing the user
/// the whole list, so one typo leaves the bindings that were right in place. Repeats and
/// the length bound are handled by `Config::repaired`, which sees the list once it is a
/// list of [`KeyName`]s.
fn key_names(raw: Vec<String>, key: &str, warnings: &mut Warnings) -> Vec<KeyName> {
    let mut names = Vec::with_capacity(raw.len());
    for entry in raw {
        match KeyName::parse(&entry, key) {
            Ok(name) => names.push(name),
            Err(error) => warnings.report_error(error),
        }
    }
    names
}

/// Counts the keys of a document.
///
/// A table contributes nothing and a list contributes one, so the entries of
/// `keys.flip_keys` count as the single key the list is.
fn count_keys(document: &toml::Table) -> usize {
    document
        .values()
        .map(|value| match value {
            toml::Value::Table(table) => count_keys(table),
            _ => 1,
        })
        .sum()
}

impl Config {
    /// Builds a configuration from the text of a configuration document.
    ///
    /// The document is merged over the built-in defaults key by key, so a document that
    /// sets three keys keeps the built-in default for every other one, and the result is
    /// then validated and repaired.
    ///
    /// # Parameters
    ///
    /// - `text`: the contents of a `config.toml`.
    ///
    /// # Returns
    ///
    /// The configuration to use, and every diagnostic raised while reading it: keys
    /// whose value was not one of the documented spellings, keys that were repaired, and
    /// a document carrying more than [`MAX_DOCUMENT_KEYS`] keys. A diagnostic never
    /// prevents the configuration from being returned.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming the whole document when `text` is not TOML, or
    /// when a key holds a value of the wrong type: `max_per_row = "five"` is a mistake in
    /// the file rather than in a value, and the caller answers it by moving the file
    /// aside. A document that merely declares a `schema_version` this build does not know
    /// is *not* an error -- it is reported and answered with the defaults, because the
    /// file may belong to a newer version and must not be moved aside.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_document(text: &str) -> Result<(Self, Vec<ImeError>), ConfigError> {
        Self::from_document_at(text, None)
    }

    /// Builds a configuration from a document that was read from `path`.
    ///
    /// This is [`Config::from_document`] plus the one thing the string form cannot do: a
    /// document written by an older build is migrated forward, and the file the migration
    /// displaced is kept beside `path`. A caller holding only text passes `None`, and the
    /// migration then happens in memory alone -- the next start migrates again, which is
    /// what makes a migration idempotent.
    ///
    /// # Errors
    ///
    /// As [`Config::from_document`].
    ///
    /// # Panics
    ///
    /// Never.
    fn from_document_at(
        text: &str,
        path: Option<&Path>,
    ) -> Result<(Self, Vec<ImeError>), ConfigError> {
        let mut document: toml::Value = toml::from_str(text)
            .map_err(|error| document_error(format!("not a TOML document: {error}")))?;

        let mut warnings = Warnings::default();
        // The migration runs before anything reads the document. A version check that
        // came first would answer a version 1 file with the defaults, which is precisely
        // the silent loss of every setting the migration exists to prevent.
        //
        // The declared version is read before the migration so that a refusal can name
        // what it was asked to migrate from.
        let declared = document
            .get(SCHEMA_VERSION_KEY)
            .and_then(toml::Value::as_integer)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(CONFIG_SCHEMA_VERSION);
        match migrate::migrate(&mut document, path.unwrap_or_else(|| Path::new(""))) {
            Ok(Some(report)) => warnings.report_error(ConfigError::Migrated {
                from: report.from,
                to: report.to,
                backup: report
                    .backup
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            }),
            // Already at this build's version: the common case, and the one that has to
            // stay silent.
            Ok(None) => {}
            Err(error) => {
                // A document this build cannot bring forward -- one written by a newer
                // build, or a step that failed. It is never fatal and never moved aside:
                // the defaults are used, the file is left exactly as it is, and the
                // diagnostic names what went wrong. Moving a user's configuration out of
                // the way because this build is older than the one that wrote it would
                // destroy settings it merely does not understand.
                warnings.report_error(ConfigError::MigrationFailed {
                    from: declared,
                    to: CONFIG_SCHEMA_VERSION,
                    reason: error.to_string(),
                });
                return Ok((Self::default(), warnings.entries));
            }
        }

        let partial: PartialConfig = document.clone().try_into().map_err(|error| {
            document_error(format!("a key holds a value of the wrong type: {error}"))
        })?;

        if let Some(version) = partial.schema_version {
            if version != i64::from(CONFIG_SCHEMA_VERSION) {
                warnings.report(
                    SCHEMA_VERSION_KEY,
                    format!("unsupported schema version: {version}"),
                );
                return Ok((Self::default(), warnings.entries));
            }
        }
        if document
            .as_table()
            .is_some_and(|table| count_keys(table) > MAX_DOCUMENT_KEYS)
        {
            warnings.report_limit(DOCUMENT_KEY, MAX_DOCUMENT_KEYS);
        }

        let mut config = Self::default();
        merge_engine(partial.engine, &mut config, &mut warnings);
        merge_ui(partial.ui, &mut config);
        merge_theme(partial.theme, &mut config, &mut warnings);
        merge_keys(partial.keys, &mut config, &mut warnings);
        merge_scheme(partial.scheme, &mut config, &mut warnings);
        merge_data(partial.data, &mut config, &mut warnings);
        merge_diagnostics(partial.diagnostics, &mut config, &mut warnings);
        merge_phrases(partial.phrases, &mut config, &mut warnings);

        let (config, mut repaired) = config.repaired();
        warnings.entries.append(&mut repaired);
        Ok((config, warnings.entries))
    }
}

/// Copies the `[engine]` keys the document sets over the defaults.
fn merge_engine(partial: Option<PartialEngine>, config: &mut Config, warnings: &mut Warnings) {
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
}

/// Copies the `[phrases]` keys the document sets over the defaults.
///
/// `max_entries` is narrowed with `try_into` rather than `as`: a document may state a
/// number wider than the field, and a silent truncation would turn a too-large limit into
/// a small one. A value the field cannot hold is left at its default and reported.
fn merge_phrases(partial: Option<PartialPhrases>, config: &mut Config, warnings: &mut Warnings) {
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
fn merge_scheme(partial: Option<PartialScheme>, config: &mut Config, warnings: &mut Warnings) {
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
fn merge_ui(partial: Option<PartialUi>, config: &mut Config) {
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
fn merge_theme(partial: Option<PartialTheme>, config: &mut Config, warnings: &mut Warnings) {
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
fn merge_keys(partial: Option<PartialKeys>, config: &mut Config, warnings: &mut Warnings) {
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
fn merge_data(partial: Option<PartialData>, config: &mut Config, warnings: &mut Warnings) {
    let Some(partial) = partial else { return };
    take_enum(partial.durability, &mut config.data.durability, warnings);
    // `backup_keep` is copied as written rather than narrowed here: the range check is
    // `Config::repaired`'s, which owns every scalar rule and reports the key name, and a
    // second check here would be a second place the bound is written down.
    config.data.backup_enabled = partial.backup_enabled.unwrap_or(config.data.backup_enabled);
    config.data.backup_keep = partial.backup_keep.unwrap_or(config.data.backup_keep);
}

/// Copies the `[diagnostics]` keys the document sets over the defaults.
fn merge_diagnostics(
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

#[cfg(test)]
mod tests;
