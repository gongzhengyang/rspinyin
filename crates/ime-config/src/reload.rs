//! Reading, writing and reloading the configuration file.
//!
//! Responsibility: everything that touches the filesystem -- reading
//! `$XDG_CONFIG_HOME/rspinyin/config.toml`, writing the documented default when there
//! is none, moving a file that cannot be parsed aside, and re-reading the file when
//! the host asks for it. The model lives in `crate::schema`; this module is the only
//! place that knows a file exists.
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

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ime_types::{CONFIG_SCHEMA_VERSION, ConfigError, ImeError};
use serde::Deserialize;

use crate::schema::{
    Config, KEY_FLIP_KEYS, KEY_HIGHLIGHT_KEYS, KeyName, MAX_DOCUMENT_KEYS, Rgb, Warnings,
};

/// The name of the configuration file inside the configuration directory.
pub const FILE_NAME: &str = "config.toml";

/// The key name reported for a failure that concerns the whole document rather than
/// one of its keys.
const DOCUMENT_KEY: &str = "config";

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

# Format version. Only 1 is understood.
schema_version = 1

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
# Keys that page the candidate list, at most eight, and keys that move the highlight.
flip_keys = ["minus", "equal", "up", "down"]
highlight_keys = ["tab", "shift_tab"]

[data]
# "eventual" batches user-frequency writes; "immediate" flushes each one.
durability = "eventual"

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
    data: Option<PartialData>,
    diagnostics: Option<PartialDiagnostics>,
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
        let document: toml::Table = toml::from_str(text)
            .map_err(|error| document_error(format!("not a TOML document: {error}")))?;
        let partial: PartialConfig = toml::from_str(text).map_err(|error| {
            document_error(format!("a key holds a value of the wrong type: {error}"))
        })?;

        let mut warnings = Warnings::default();
        if let Some(version) = partial.schema_version {
            if version != i64::from(CONFIG_SCHEMA_VERSION) {
                warnings.report(
                    "schema_version",
                    format!("unsupported schema version: {version}"),
                );
                return Ok((Self::default(), warnings.entries));
            }
        }
        if count_keys(&document) > MAX_DOCUMENT_KEYS {
            warnings.report_limit(DOCUMENT_KEY, MAX_DOCUMENT_KEYS);
        }

        let mut config = Self::default();
        merge_engine(partial.engine, &mut config, &mut warnings);
        merge_ui(partial.ui, &mut config);
        merge_theme(partial.theme, &mut config, &mut warnings);
        merge_keys(partial.keys, &mut config, &mut warnings);
        merge_data(partial.data, &mut config, &mut warnings);
        merge_diagnostics(partial.diagnostics, &mut config, &mut warnings);

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

/// The configuration file the plugin reads when nothing else is specified:
/// `$XDG_CONFIG_HOME/rspinyin/config.toml`, or `~/.config/rspinyin/config.toml` when
/// `XDG_CONFIG_HOME` is unset or empty.
///
/// # Returns
///
/// The path, or `None` when neither `XDG_CONFIG_HOME` nor `HOME` is set: there is then
/// no configuration directory to use, and the built-in defaults are all there is.
pub fn default_path() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(directory) => PathBuf::from(directory),
        None => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("rspinyin").join(FILE_NAME))
}

/// What reading the configuration file produced.
enum Document {
    /// The file was read; the text is what it held.
    Text(String),
    /// There is no file at the path.
    Missing,
    /// The file exists but could not be read; the reason is already reported.
    Unusable,
}

/// Reads the configuration document at `path`.
///
/// A file that is not there is reported as [`Document::Missing`] rather than as a
/// failure: it is the state every user starts in, and the caller answers it by writing
/// the documented default. A file that cannot be read is *not* moved aside -- the loader
/// does not touch a file it could not even read.
fn read(path: &Path, warnings: &mut Warnings) -> Document {
    match fs::read_to_string(path) {
        Ok(text) => Document::Text(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Document::Missing,
        Err(error) => {
            warnings.report(
                DOCUMENT_KEY,
                format!("cannot read the configuration file: {error}"),
            );
            Document::Unusable
        }
    }
}

/// The file name of `path`, or the default file name when it has none.
fn file_name_of(path: &Path) -> OsString {
    path.file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from(FILE_NAME))
}

/// Moves a file that cannot be parsed out of the way.
///
/// The file is moved, never deleted: a configuration the user spent time on is theirs to
/// recover by hand, and the loader has no way to know what they meant. A stamp that is
/// already taken -- two corrupt files within the same second -- takes the next `.1`,
/// `.2`, ... suffix, and if every name is taken the file is left where it is rather than
/// overwritten.
fn quarantine(path: &Path, unix_secs: u64, warnings: &mut Warnings) {
    let mut base = file_name_of(path);
    base.push(format!(".bad.{unix_secs}"));
    let base = path.with_file_name(base);
    for attempt in 0..MAX_BACKUP_ATTEMPTS {
        let target = if attempt == 0 {
            base.clone()
        } else {
            let mut name = file_name_of(&base);
            name.push(format!(".{attempt}"));
            base.with_file_name(name)
        };
        if target.exists() {
            continue;
        }
        match fs::rename(path, &target) {
            Ok(()) => return,
            Err(error) => {
                warnings.report(
                    DOCUMENT_KEY,
                    format!("cannot move the unreadable configuration aside: {error}"),
                );
                return;
            }
        }
    }
    warnings.report(
        DOCUMENT_KEY,
        format!("no free backup name beside {}", base.display()),
    );
}

/// Writes the documented default configuration to `path` when nothing is there.
///
/// `create_new` is what makes this safe: the file is written only when the path is free,
/// so a configuration the user has -- including one this loader could not read -- is
/// never replaced by the template.
fn write_template(path: &Path, warnings: &mut Warnings) {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(error) = fs::create_dir_all(parent) {
                warnings.report(
                    DOCUMENT_KEY,
                    format!("cannot create the configuration directory: {error}"),
                );
                return;
            }
        }
    }
    let written = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(DEFAULT_CONFIG_TOML.as_bytes()),
        // The path is taken, which is the one case that is not a failure: whatever is
        // there is the user's and is left alone.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    };
    if let Err(error) = written {
        warnings.report(
            DOCUMENT_KEY,
            format!("cannot write the default configuration: {error}"),
        );
    }
}

/// The current time as seconds since the Unix epoch, or `0` when the clock is set before
/// it.
///
/// `0` is a usable stamp: a backup name only has to be distinct from the ones already in
/// the directory, not correct.
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Config {
    /// Loads the configuration from `path`, stamping a corrupt file's backup name with
    /// the current system time.
    ///
    /// # Errors
    ///
    /// None: the diagnostics are the report.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(path: &Path) -> (Self, Vec<ImeError>) {
        Self::load_at(path, unix_secs())
    }

    /// Loads the configuration from `path`, stamping a corrupt file's backup name with
    /// `unix_secs`.
    ///
    /// # Parameters
    ///
    /// - `path`: the configuration file.
    /// - `unix_secs`: the stamp used when a file that cannot be parsed is moved aside. It
    ///   is a parameter rather than a call to the system clock so that loading is a
    ///   function of its inputs and its tests are deterministic.
    ///
    /// # Returns
    ///
    /// The configuration to run with, and the diagnostics raised while reading it. A
    /// missing file, an unreadable file and a corrupt file all answer the built-in
    /// defaults with a diagnostic, so a configuration problem never keeps the input
    /// method from starting. A missing file is written back out as the documented
    /// default, which is how a user discovers the available keys.
    ///
    /// # Errors
    ///
    /// None: the diagnostics are the report.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load_at(path: &Path, unix_secs: u64) -> (Self, Vec<ImeError>) {
        let mut warnings = Warnings::default();
        match read(path, &mut warnings) {
            Document::Text(text) => match Self::from_document(&text) {
                Ok((config, mut parsed)) => {
                    warnings.entries.append(&mut parsed);
                    (config, warnings.entries)
                }
                Err(error) => {
                    warnings.report_error(error);
                    quarantine(path, unix_secs, &mut warnings);
                    write_template(path, &mut warnings);
                    (Self::default(), warnings.entries)
                }
            },
            Document::Missing => {
                write_template(path, &mut warnings);
                (Self::default(), warnings.entries)
            }
            Document::Unusable => (Self::default(), warnings.entries),
        }
    }
}

/// What one reload produced.
#[derive(Debug)]
pub enum ReloadOutcome {
    /// The file parsed to a configuration different from the one in force, which is now
    /// active.
    Updated {
        /// Diagnostics raised while the new document was read.
        warnings: Vec<ImeError>,
    },
    /// The file parsed to exactly the configuration already in force, so nothing changed.
    /// The diagnostics were reported when that configuration was adopted.
    Unchanged,
    /// The file was missing or could not be parsed. The configuration in force is kept
    /// unchanged, and the diagnostics say why.
    Kept {
        /// Diagnostics explaining why the configuration in force was kept.
        warnings: Vec<ImeError>,
    },
}

/// The configuration in force, and the file it was read from.
///
/// The store owns the `Arc` the rest of the plugin holds, so a reload replaces one
/// pointer: a component that took a copy of the previous `Arc` -- a decoder in the middle
/// of a composition, say -- keeps the configuration it started with and is never
/// rewritten underneath. That is what makes a reload unable to disturb an input session
/// that is in progress (0.4 rule 10).
pub struct ConfigStore {
    /// The file the configuration is read from and reloaded from.
    path: PathBuf,
    /// The configuration in force.
    current: Arc<Config>,
}

impl ConfigStore {
    /// Loads the configuration for the first time.
    pub fn load(path: &Path) -> (Self, Vec<ImeError>) {
        Self::load_at(path, unix_secs())
    }

    /// Loads the configuration for the first time, stamping a corrupt file's backup name
    /// with `unix_secs`.
    pub fn load_at(path: &Path, unix_secs: u64) -> (Self, Vec<ImeError>) {
        let (config, warnings) = Config::load_at(path, unix_secs);
        let store = Self {
            path: path.to_path_buf(),
            current: Arc::new(config),
        };
        (store, warnings)
    }

    /// The configuration in force.
    ///
    /// The `Arc` is what a caller hands to another thread; the value behind it is never
    /// mutated, so a reader always sees one consistent configuration.
    pub fn current(&self) -> &Arc<Config> {
        &self.current
    }

    /// Re-reads the configuration file: the entry point fcitx5's `reloadConfig()` calls.
    ///
    /// It is deliberately not the load path. A reload improves the configuration or
    /// leaves it alone, and never writes to the file or falls back to the defaults.
    ///
    /// # Returns
    ///
    /// [`ReloadOutcome::Updated`] with the diagnostics of the new document when the file
    /// parsed to a different configuration, [`ReloadOutcome::Unchanged`] when it parsed to
    /// the one already in force, and [`ReloadOutcome::Kept`] with the reason when the file
    /// could not be read or parsed.
    ///
    /// # Errors
    ///
    /// None: the outcome carries the diagnostics.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reload(&mut self) -> ReloadOutcome {
        let mut warnings = Vec::new();
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) => {
                // A file that cannot be read -- gone, or unreadable -- leaves the
                // configuration in force alone. A missing file is far likelier to be a
                // save in progress or a mistake than a deliberate reset, and a restart is
                // the unambiguous way back to the built-in defaults.
                warnings.push(ImeError::from(document_error(format!(
                    "cannot read the configuration file: {error}"
                ))));
                return ReloadOutcome::Kept { warnings };
            }
        };
        let (config, mut parsed) = match Config::from_document(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                // The file is left exactly as it is: it may be an edit in progress, and
                // the configuration in force is known to be good.
                warnings.push(ImeError::from(error));
                return ReloadOutcome::Kept { warnings };
            }
        };
        warnings.append(&mut parsed);
        if config == *self.current {
            return ReloadOutcome::Unchanged;
        }
        self.current = Arc::new(config);
        ReloadOutcome::Updated { warnings }
    }
}

#[cfg(test)]
mod tests;
