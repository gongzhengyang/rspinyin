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
mod merge;
mod store;

use self::merge::{
    merge_assoc, merge_data, merge_diagnostics, merge_engine, merge_fuzzy, merge_keys,
    merge_phrases, merge_quick_input, merge_scheme, merge_theme, merge_ui,
};
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
#
# Every key's comment also ends with how a change to it reaches the running plugin:
#
#   Takes effect on reload -- the host's configuration reload applies it
#   Takes effect at the next fcitx5 restart -- read once, at start-up
#   Not wired yet -- accepted by this build, but nothing reads it

# Format version. Only 2 is understood; a file written by an older build is migrated
# once, with the original kept beside it. Read on every load and every reload: it
# selects the migration, never a behaviour.
schema_version = 2

[engine]
# "chinese" replaces ASCII punctuation with its Chinese mark; "english" leaves it to
# the application. Takes effect on reload: new contexts start where the document says,
# and the chords keep flipping both switches at runtime.
punct_mode = "chinese"
# Widen the committed text of ASCII characters. Takes effect on reload: new contexts
# start where the document says, and the chord keeps flipping the switch at runtime.
full_width = false
# Commit a leading uppercase letter instead of decoding it, so typing English needs no
# mode switch. Takes effect on reload.
auto_english_on_uppercase = true
# Leave the keys to the application while the caret is inside a URL or an email.
# Takes effect on reload.
passthrough_url = true
# Hard limit on the length of the raw input, 1..=64. Takes effect on reload, and it
# applies to the next character pushed, never to the input already on screen.
max_raw_len = 64
# "full" verifies the whole dictionary when it is loaded, "header" only its header.
# Read once, when the dictionary is mapped. Takes effect at the next fcitx5 restart.
verify_dict_on_load = "full"
# Expand initial-letter abbreviations, so that `nh` reaches `ni'hao`. Off by default:
# an abbreviation is ambiguous by nature, and the abbreviation candidate rides a
# penalty so it never outranks the full spelling at equal dictionary weight. Takes
# effect on reload: the switch latches with the scheme, so a change answers the next
# composition and never re-reads one in progress.
abbrev = false
# Correct one-step typing slips -- a keyboard-neighbour substitution or an adjacent
# transposition -- for an input no exact spelling covers, so that `zhegn` still
# reaches the word spelled `zheng`. Off by default: a corrected candidate rides a
# heavy penalty and is marked in its annotation, and an input that spells a word is
# never corrected at all. Takes effect on reload: the switch latches with the scheme,
# so a change answers the next composition and never re-reads one in progress.
correction = false
# Offer the English lexicon's entry for an input no pinyin reading cuts, so that
# `github` reaches the word spelled `GitHub`. Off by default: a user who never asked
# for English candidates must not have their compositions answered from a table they
# did not know was there. Takes effect on reload: the switch latches with the scheme,
# so a change answers the next composition and never re-reads one in progress.
mixed_english = false
# What happens to a live composition when the input method is switched away from
# or reset. "clear" takes the composing text back and commits nothing; "commit_raw"
# commits the raw input first, so a user who switches away mid-word keeps the
# letters they typed. Takes effect on reload from the next switch away.
on_im_switch = "clear"
# Answer a dead-key sequence the host is composing -- an acute accent followed by
# `e`, for one -- before the key table sees the key. Off by default: a pass that
# swallows keys changes what a European keyboard types, and a host that offers no
# compose support leaves the switch inert.
compose = false
# "context" keeps one session per input context; "program" shares one session across
# every input context of the same application, so the Chinese/English mode and the
# other mode bits survive a window switch inside one application. Applications are
# told apart by a hash of their identifier, never by a name. Takes effect on reload:
# the sessions already alive finish under the policy they started with, and the next
# activation pools under the new one.
share_state = "context"

[ui]
# Show the composing text in the application's preedit area instead of the window.
# An application the host reports as unable to render a preedit area is never
# written one, whatever this says: the candidate window's header takes over.
# Takes effect on reload.
client_preedit = false
# Candidates per row, 3..=9. Takes effect on reload.
max_per_row = 5
# Show the word annotation beside each candidate. Takes effect on reload.
show_annotation = true
# Widest candidate window, 220..=1200 dp. Takes effect on reload.
max_width_dp = 720
# Corner radius, 8..=20 dp. The window look is built once, at the user-interface
# addon's start-up. Takes effect at the next fcitx5 restart.
corner_radius_dp = 12
# Background opacity, 0..=255. The default is 0.85. Takes effect at the next fcitx5
# restart.
base_alpha = 217

[ui.animation]
# Animate the window with a spring: angular frequency in rad/s, 4.0..=80.0, and the
# damping ratio, 0.3..=2.0. The durations are 0..=600 ms. The whole section is read
# when the window is built. Takes effect at the next fcitx5 restart.
enabled = true
omega0 = 26.0
zeta = 0.85
appear_ms = 110
disappear_ms = 90

[theme]
# "auto" follows the desktop's colour scheme; "light" and "dark" pin it. Read by the
# user-interface addon once, at start-up. Takes effect at the next fcitx5 restart.
scheme = "auto"
# Accent colour, written as #RRGGBB. Takes effect at the next fcitx5 restart.
accent = "#4C9AFF"

[keys]
# What the "0" key does once the candidate list reaches ten entries: "passthrough"
# sends the digit to the application, "flip" turns the page. Takes effect on reload.
digit_zero = "passthrough"
# Commit the raw input on Enter instead of the highlighted candidate. Takes effect on
# reload.
enter_commit_raw = false
# Keys that page the candidate list, at most six, and keys that move the highlight.
# "home" jumps to the first page and "end" to the last. Takes effect on reload.
flip_keys = ["minus", "equal", "up", "down", "home", "end"]
highlight_keys = ["tab", "shift_tab"]

[scheme]
# The layout the keystrokes follow: "full" is plain pinyin, and "xiaohe", "ziranma",
# "microsoft", "sogou" and "ziguang" are the double-pinyin layouts. Takes effect on
# reload: a new layout answers the next composition and never re-reads one in progress.
scheme = "full"
# Name the active layout in the candidate window's header. Takes effect on reload.
show_hint = true
# Still read a full-pinyin syllable typed while a double-pinyin layout is active.
# Takes effect on reload.
keep_full_pinyin = true

[fuzzy]
# Each key selects one fuzzy-equivalence class the decoder widens with: `zh_z` makes
# `zh` and `z` read as each other, `n_l` makes `n` and `l` do the same, and so on for
# every pair the class table defines. `enabled = true` selects every class at once;
# an individual `true` selects its class even with the master switch off. Takes
# effect on reload: the classes latch with the scheme, so a change answers the next
# composition and never re-reads one in progress.
enabled = false
zh_z = false
ch_c = false
sh_s = false
n_l = false
f_h = false
an_ang = false
en_eng = false
in_ing = false

[assoc]
# Offer next-word association candidates after a successful commit. Off by default:
# a new candidate window after every commit is an explicit opt-in. Takes effect on
# reload.
enabled = false
# How many association candidates one commit may offer, 1..=9. Takes effect on
# reload.
max_candidates = 5
# How many recent commits the in-session history source may retain, 0..=64. This
# memory dies with the session and is never written to disk. Takes effect on reload.
history_depth = 8

[quick_input]
# Recognise whole-buffer quick-input triggers such as the date family and the
# calculator. Off by default: a trigger that turns ordinary spelling into a domain is
# an explicit opt-in. Trigger recognition only; translators are not wired yet.
enabled = false
# Whole-buffer spellings that open the date family. Takes effect on reload.
date_prefixes = ["rq", "dt"]
# Recognise the calculator trigger. Off by default. Takes effect on reload.
calc_enabled = false

[phrases]
# Let the phrase table take part in a decode. With it off the dictionary answers alone.
# The table is read once, at start-up. Takes effect at the next fcitx5 restart.
enabled = true
# The phrase document to read. Empty means the default location under the user's
# configuration directory. Takes effect at the next fcitx5 restart.
file = ""
# How many entries the table may hold, 1..=50000. Takes effect at the next fcitx5
# restart.
max_entries = 5000

[data]
# "eventual" batches user-frequency writes; "immediate" flushes each one. The batch
# window is the store's construction-time policy. Takes effect at the next fcitx5
# restart.
durability = "eventual"
# Copy the user's learned words automatically. On by default: the store cannot be
# rebuilt from anywhere else. Takes effect at the next fcitx5 restart.
backup_enabled = true
# How many backup generations are kept, 1..=32. The oldest is removed once a new one
# has landed. Takes effect at the next fcitx5 restart.
backup_keep = 3
# The dictionary file the next reload swaps in whole: the reload validates the file
# completely, swaps it for the live one without touching the composition in progress,
# and clears this key. A file the validation rejects is reported as
# dict/swap/rejected and the key stands for you to fix. Empty means nothing to swap.
dict_staged_path = ""

[diagnostics]
# "error" | "warn" | "info" | "debug" | "trace". Takes effect on reload.
level = "info"
# When the log file reaches this many MiB it rolls over, and this many rolled files
# are kept. Takes effect on reload.

# Logical-clock half-life of user-word decay, in commits. The default matches the
# librime e-folding constant. Takes effect on reload.
decay_half_life_ticks = 139

# Smallest user-decay accumulator, in Q16.16; a record at or below it stops
# ranking. Takes effect on reload.
decay_floor = 65536
log_rotation_mb = 8
log_keep_files = 3
# Accepted, and deliberately without effect on what is recorded: the plugin never
# writes the characters you type to the log, whether this is false or true.
log_input_content = false
# Switch the diagnostic probes on. Read once, at start-up, with the configuration.
# Takes effect at the next fcitx5 restart.
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
    assoc: Option<PartialAssoc>,
    quick_input: Option<PartialQuickInput>,
    ui: Option<PartialUi>,
    theme: Option<PartialTheme>,
    keys: Option<PartialKeys>,
    scheme: Option<PartialScheme>,
    fuzzy: Option<PartialFuzzy>,
    data: Option<PartialData>,
    diagnostics: Option<PartialDiagnostics>,
    phrases: Option<PartialPhrases>,
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
        merge_ui(partial.ui, &mut config);
        merge_theme(partial.theme, &mut config, &mut warnings);
        merge_keys(partial.keys, &mut config, &mut warnings);
        merge_scheme(partial.scheme, &mut config, &mut warnings);
        merge_fuzzy(partial.fuzzy, &mut config);
        merge_data(partial.data, &mut config, &mut warnings);
        merge_diagnostics(partial.diagnostics, &mut config, &mut warnings);
        merge_phrases(partial.phrases, &mut config, &mut warnings);

        let (config, mut repaired) = config.repaired();
        warnings.entries.append(&mut repaired);
        Ok((config, warnings.entries))
    }
}

#[cfg(test)]
mod tests;
