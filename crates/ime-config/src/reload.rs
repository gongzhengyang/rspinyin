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
//! This file holds the document model: the sections as a user writes them and the
//! merge of one document over the built-in defaults. The two halves that act on
//! that model sit beside it. The `template` submodule holds the text a fresh
//! install is given. The `load`
//! submodule owns every touch of the filesystem -- where the file is, how it is read,
//! and what is written when it is not there. The `store` submodule owns the
//! configuration in force and the reload path. The `retired` submodule holds the keys
//! this build no longer reads and the prune that deletes them from a document, run by
//! the reading above once the migration has been persisted. The submodules are private,
//! and what they hand out is re-exported below, so the public path of every item is the
//! one it has always had.

mod load;
mod merge;
mod retired;
mod store;
mod template;

use self::merge::{
    merge_app_profiles, merge_assoc, merge_data, merge_diagnostics, merge_engine, merge_english,
    merge_fuzzy, merge_keys, merge_phrases, merge_quick_input, merge_scheme, merge_shuangpin,
    merge_theme, merge_ui,
};
pub use load::default_path;
pub use store::{ConfigStore, ReloadOutcome};
pub use template::DEFAULT_CONFIG_TOML;

use std::collections::BTreeMap;
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
    assoc: Option<PartialAssoc>,
    quick_input: Option<PartialQuickInput>,
    english: Option<PartialEnglish>,
    ui: Option<PartialUi>,
    theme: Option<PartialTheme>,
    keys: Option<PartialKeys>,
    scheme: Option<PartialScheme>,
    fuzzy: Option<PartialFuzzy>,
    data: Option<PartialData>,
    diagnostics: Option<PartialDiagnostics>,
    phrases: Option<PartialPhrases>,
    shuangpin: Option<PartialShuangpin>,
    app_profiles: Option<PartialAppProfiles>,
}

/// The `[app_profiles]` table of a document.
#[derive(Deserialize)]
struct PartialAppProfiles {
    profiles: Option<BTreeMap<String, PartialAppProfile>>,
}

/// One `[app_profiles.profiles]` entry of a document.
///
/// Every key is optional, and an absent key is "no opinion": the session starts where
/// the shipped projection starts. Whether the entry's own key is a hashed application
/// identifier, and how many entries the table may hold, is the schema's rule.
#[derive(Deserialize)]
struct PartialAppProfile {
    ascii: Option<bool>,
    script: Option<String>,
    full_width: Option<bool>,
    punct_full: Option<bool>,
}

/// The `[shuangpin]` table of a document.
#[derive(Deserialize)]
struct PartialShuangpin {
    custom: Option<PartialShuangpinCustom>,
}

/// The `[shuangpin.custom]` table of a document.
///
/// The whole table is one layout, so a key the document leaves out keeps its default --
/// which is "not written", the state the layout loader refuses like a document that
/// left it out. Validation is not this layer's business: the tables travel as plain
/// key/value maps, and the layout loader in the layer above judges them when the
/// configuration is adopted.
#[derive(Deserialize)]
struct PartialShuangpinCustom {
    file: Option<String>,
    name: Option<String>,
    zero_initial_style: Option<String>,
    initials: Option<BTreeMap<String, String>>,
    finals: Option<BTreeMap<String, String>>,
    standalone: Option<BTreeMap<String, String>>,
}

/// The `[assoc]` table of a document.
#[derive(Deserialize)]
struct PartialAssoc {
    enabled: Option<bool>,
    max_candidates: Option<u8>,
    history_depth: Option<u8>,
}

/// The `[quick_input]` table of a document.
#[derive(Deserialize)]
struct PartialQuickInput {
    enabled: Option<bool>,
    date_prefixes: Option<Vec<String>>,
    calc_enabled: Option<bool>,
}

/// The `[english]` table of a document.
#[derive(Deserialize)]
struct PartialEnglish {
    spacing: Option<String>,
}

/// The `[fuzzy]` table of a document.
#[derive(Deserialize)]
struct PartialFuzzy {
    enabled: Option<bool>,
    zh_z: Option<bool>,
    ch_c: Option<bool>,
    sh_s: Option<bool>,
    n_l: Option<bool>,
    f_h: Option<bool>,
    an_ang: Option<bool>,
    en_eng: Option<bool>,
    in_ing: Option<bool>,
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
    correction: Option<bool>,
    mixed_english: Option<bool>,
    on_im_switch: Option<String>,
    compose: Option<bool>,
    share_state: Option<String>,
    aux_refinement: Option<bool>,
    reverse_lookup: Option<bool>,
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
    select_first_char: Option<String>,
    select_last_char: Option<String>,
}

/// The `[data]` table of a document.
#[derive(Deserialize)]
struct PartialData {
    durability: Option<String>,
    backup_enabled: Option<bool>,
    backup_keep: Option<u8>,
    decay_half_life_ticks: Option<i64>,
    decay_floor: Option<i64>,
    context_tie_band_q16: Option<i64>,
    dict_staged_path: Option<String>,
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

        // The retired-key prune is the last rewrite the document takes, and it waits
        // for the migration on purpose. By the time the migration has returned, every
        // value it carried is in the document and, when the document came from a file,
        // on the disk as well -- so pruning a retired key here can never take a value
        // with it that a later run would not find again. Pruning before the migration
        // is the one order that could lose a setting, which is the iron rule the list
        // in `retired` is written under: migrate-and-persist first, prune after. The
        // count is not reported: a key this build no longer reads disappearing is not a
        // mistake of the user's to fix.
        retired::prune_retired(&mut document);

        let partial: PartialConfig = document.clone().try_into().map_err(|error| {
            document_error(format!("a key holds a value of the wrong type: {error}"))
        })?;

        if let Some(version) = partial.schema_version
            && version != i64::from(CONFIG_SCHEMA_VERSION)
        {
            warnings.report(
                SCHEMA_VERSION_KEY,
                format!("unsupported schema version: {version}"),
            );
            return Ok((Self::default(), warnings.entries));
        }
        if document
            .as_table()
            .is_some_and(|table| count_keys(table) > MAX_DOCUMENT_KEYS)
        {
            warnings.report_limit(DOCUMENT_KEY, MAX_DOCUMENT_KEYS);
        }

        let mut config = Self::default();
        merge_engine(partial.engine, &mut config, &mut warnings);
        merge_assoc(partial.assoc, &mut config);

        merge_quick_input(partial.quick_input, &mut config);
        merge_english(partial.english, &mut config, &mut warnings);
        merge_ui(partial.ui, &mut config);
        merge_theme(partial.theme, &mut config, &mut warnings);
        merge_keys(partial.keys, &mut config, &mut warnings);
        merge_scheme(partial.scheme, &mut config, &mut warnings);
        merge_fuzzy(partial.fuzzy, &mut config);
        merge_data(partial.data, &mut config, &mut warnings);
        merge_diagnostics(partial.diagnostics, &mut config, &mut warnings);
        merge_phrases(partial.phrases, &mut config, &mut warnings);
        merge_shuangpin(partial.shuangpin, &mut config);
        merge_app_profiles(partial.app_profiles, &mut config, &mut warnings);

        let (config, mut repaired) = config.repaired();
        warnings.entries.append(&mut repaired);
        Ok((config, warnings.entries))
    }
}

#[cfg(test)]
mod retired_tests;
#[cfg(test)]
mod tests;
