//! Putting back the Fcitx5 user interface the plugin took over.
//!
//! Responsibility: read the record the plugin writes when it makes itself the active
//! user interface, and write the value it displaced back into Fcitx5's configuration.
//!
//! # Why an uninstall has to do this
//!
//! Fcitx5 picks its active user interface from the `UI` addons that report themselves
//! available, and the plugin pins that choice to itself while it is installed. Remove
//! the plugin without putting the previous choice back and Fcitx5 is left pointing at
//! an addon that no longer exists: no candidate window at all, on a system where
//! nothing else looks broken enough to explain why.
//!
//! The record is self-describing -- it names the configuration file, the section and
//! the key it changed -- so this module needs no knowledge of Fcitx5's configuration
//! layout beyond the defaults it falls back to when the record omits them. It belongs
//! to the user running the command, not to the system, so it is never `DESTDIR`-
//! prefixed and never needs elevation.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

/// File name of the record, inside the user's plugin data directory.
pub const RECORD_FILE: &str = "ui_takeover.json";

/// Directory Fcitx5 keeps its own configuration in, under `$XDG_CONFIG_HOME`.
const FCITX5_DIR: &str = "fcitx5";

/// Fcitx5's global configuration file, which holds the active user interface.
const FCITX5_CONFIG: &str = "config";

/// Section of Fcitx5's global configuration that holds the active user interface.
const DEFAULT_SECTION: &str = "Behavior";

/// Key of Fcitx5's global configuration that names the active user interface.
const DEFAULT_KEY: &str = "ActiveUserInterface";

/// Field names the record may use for the configuration file it edited.
const CONFIG_FILE_KEYS: &[&str] = &["config_file", "file", "path"];

/// Field names the record may use for the section the key lives in.
const SECTION_KEYS: &[&str] = &["section", "group"];

/// Field names the record may use for the key that was changed.
const KEY_KEYS: &[&str] = &["key", "option"];

/// Field names the record may use for the value the takeover displaced.
///
/// Deliberately not `value`: a record that carries both a previous and a current value
/// would have the two confused, and writing the plugin's own name back into the
/// configuration is exactly the failure this module exists to prevent.
const PREVIOUS_KEYS: &[&str] = &["previous", "original", "previous_value"];

/// The record the plugin writes when it makes itself the active user interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Configuration file the takeover edited.
    pub config_file: PathBuf,
    /// Section the key lives in.
    pub section: String,
    /// Key that was changed.
    pub key: String,
    /// The value that was there before; `None` means the key did not exist.
    pub previous: Option<String>,
}

impl Record {
    /// Parses a record, filling in the defaults for the fields it omits.
    ///
    /// Read as a JSON object rather than through a derived struct because of the one
    /// distinction that matters: a field that is present and `null` means the key did
    /// not exist before the takeover and has to be removed again, while a field that is
    /// absent means this is not a record the uninstall understands and nothing may be
    /// touched. An `Option<T>` field loses exactly that distinction.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not a JSON object, when it carries none of the
    /// field names this installer knows, or when a field it does carry has the wrong
    /// type.
    pub fn parse(text: &str, config_home: &Path) -> Result<Self> {
        let value: Value = serde_json::from_str(text).context("the takeover record is not JSON")?;
        let object = value
            .as_object()
            .context("the takeover record is not a JSON object")?;
        Ok(Self {
            config_file: string_field(object, CONFIG_FILE_KEYS)?.map_or_else(
                || config_home.join(FCITX5_DIR).join(FCITX5_CONFIG),
                PathBuf::from,
            ),
            section: string_field(object, SECTION_KEYS)?
                .unwrap_or_else(|| DEFAULT_SECTION.to_owned()),
            key: string_field(object, KEY_KEYS)?.unwrap_or_else(|| DEFAULT_KEY.to_owned()),
            previous: previous_field(object, PREVIOUS_KEYS)?,
        })
    }
}

/// What an uninstall did about the takeover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No record exists, so the plugin never took the user interface over.
    NoRecord,
    /// The record exists and would be applied; nothing was written.
    Planned {
        /// Configuration file that would be edited.
        file: PathBuf,
        /// Value that would be written back; `None` means the key would be removed.
        value: Option<String>,
    },
    /// The recorded value was written back, or was already in place.
    Restored {
        /// Configuration file that was edited.
        file: PathBuf,
        /// Value that was written back; `None` means the key was removed because it did
        /// not exist before the takeover.
        value: Option<String>,
    },
}

/// Puts the recorded value back into Fcitx5's configuration.
///
/// `record_path` is the takeover record; `config_home` is `$XDG_CONFIG_HOME`, used when
/// the record does not name a file. With `dry_run` the record is read and validated but
/// nothing is written.
///
/// # Errors
///
/// Returns an error when the record exists but cannot be read or parsed, when the
/// configuration file cannot be read, and when the rewritten configuration cannot be
/// written back.
pub fn restore(record_path: &Path, config_home: &Path, dry_run: bool) -> Result<Outcome> {
    let text = match fs::read_to_string(record_path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Outcome::NoRecord),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", record_path.display()));
        }
    };
    let record = Record::parse(&text, config_home)
        .with_context(|| format!("{}: unusable takeover record", record_path.display()))?;
    if dry_run {
        return Ok(Outcome::Planned {
            file: record.config_file,
            value: record.previous,
        });
    }
    let current = match fs::read_to_string(&record.config_file) {
        Ok(text) => text,
        // Fcitx5 has not written a configuration file yet, which is the state of a
        // fresh account. The takeover cannot have changed a key that was never saved,
        // so there is nothing to put back and nothing to create.
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Outcome::Restored {
                file: record.config_file,
                value: record.previous,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", record.config_file.display()));
        }
    };
    let updated = set_ini_value(
        &current,
        &record.section,
        &record.key,
        record.previous.as_deref(),
    );
    if updated != current {
        write_atomically(&record.config_file, &updated)?;
    }
    Ok(Outcome::Restored {
        file: record.config_file,
        value: record.previous,
    })
}

/// Sets `key` in `section` to `value`, or removes it when `value` is `None`.
///
/// Hand-rolled rather than routed through a configuration crate because the file
/// belongs to Fcitx5: everything outside the one line this changes has to come back out
/// byte for byte, comments, key order and blank lines included, which is the opposite
/// of what a parse-and-reserialize round trip does.
pub fn set_ini_value(text: &str, section: &str, key: &str, value: Option<&str>) -> String {
    let header = format!("[{section}]");
    let mut lines: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut written = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if is_section_header(trimmed) {
            if in_section && !written {
                write_assignment(&mut lines, key, value, &mut written);
            }
            in_section = trimmed == header;
            seen_section |= in_section;
            lines.push(line.to_owned());
            continue;
        }
        if in_section && assigns(trimmed, key) {
            write_assignment(&mut lines, key, value, &mut written);
            continue;
        }
        lines.push(line.to_owned());
    }
    if in_section && !written {
        write_assignment(&mut lines, key, value, &mut written);
    }
    if !seen_section && value.is_some() {
        if !lines.is_empty() && !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        lines.push(header);
        write_assignment(&mut lines, key, value, &mut written);
    }
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

/// Appends the assignment, or nothing at all when the key is being removed.
fn write_assignment(lines: &mut Vec<String>, key: &str, value: Option<&str>, written: &mut bool) {
    *written = true;
    if let Some(value) = value {
        lines.push(format!("{key}={value}"));
    }
}

/// Whether a trimmed line opens an INI section.
fn is_section_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2
}

/// Whether a trimmed line assigns `key`.
///
/// A commented-out assignment is not an assignment: Fcitx5's own configuration files
/// carry commented examples of the keys they document, and treating one as the live key
/// would write the value into a line the user has deliberately disabled.
fn assigns(trimmed: &str, key: &str) -> bool {
    if trimmed.starts_with('#') || trimmed.starts_with(';') {
        return false;
    }
    trimmed
        .split_once('=')
        .is_some_and(|(name, _)| name.trim() == key)
}

/// Writes `text` over `path`, keeping the mode the file already had.
///
/// Through a temporary sibling, so that a failure part way through cannot leave a
/// truncated configuration behind: Fcitx5 reads this file at startup, and half a file
/// is worse than the value this uninstall was trying to correct.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("rspinyin-tmp");
    fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temporary, metadata.permissions())
            .with_context(|| format!("setting the mode of {}", temporary.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("moving {} to {}", temporary.display(), path.display()))
}

/// Reads one of `names` from `object` as a string.
///
/// # Errors
///
/// Returns an error when one of the names is present with a value that is not a string.
fn string_field(object: &Map<String, Value>, names: &[&str]) -> Result<Option<String>> {
    for name in names {
        let Some(value) = object.get(*name) else {
            continue;
        };
        let text = value
            .as_str()
            .with_context(|| format!("the takeover record's `{name}` is not a string"))?;
        return Ok(Some(text.to_owned()));
    }
    Ok(None)
}

/// Reads the value the takeover displaced.
///
/// # Errors
///
/// Returns an error when none of `names` is present -- the record is not one this
/// installer understands, and guessing would mean deleting a setting the user chose --
/// or when the name that is present is neither a string nor `null`.
fn previous_field(object: &Map<String, Value>, names: &[&str]) -> Result<Option<String>> {
    for name in names {
        let Some(value) = object.get(*name) else {
            continue;
        };
        return match value {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            other => {
                anyhow::bail!(
                    "the takeover record's `{name}` is neither a string nor null: {other}"
                )
            }
        };
    }
    anyhow::bail!(
        "the takeover record carries none of `{}`, so this uninstall cannot tell which \
         Fcitx5 setting it changed; put the previous `{}` back by hand",
        names.join("`, `"),
        DEFAULT_KEY
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rspinyin-takeover-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// The shape of a real Fcitx5 global configuration, with the two sections a
    /// takeover touches and a comment that has to survive the round trip.
    const CONFIG: &str = "\
[Hotkey]\n\
TriggerKey=Control+space\n\
\n\
[Behavior]\n\
# the UI that draws candidates\n\
ActiveUserInterface=classic\n\
ShareInputState=No\n\
\n\
[Appearance]\n\
Theme=default\n";

    #[test]
    fn test_set_ini_value_replaces_a_key_in_the_named_section() {
        let updated = set_ini_value(CONFIG, "Behavior", "ActiveUserInterface", Some("rspinyin"));
        assert!(
            updated.contains("ActiveUserInterface=rspinyin"),
            "{updated}"
        );
        assert!(!updated.contains("=classic"), "{updated}");
    }

    #[test]
    fn test_set_ini_value_leaves_every_other_line_untouched() {
        let updated = set_ini_value(CONFIG, "Behavior", "ActiveUserInterface", Some("rspinyin"));
        let expected: Vec<&str> = CONFIG
            .lines()
            .map(|line| {
                if line.trim() == "ActiveUserInterface=classic" {
                    "ActiveUserInterface=rspinyin"
                } else {
                    line
                }
            })
            .collect();
        assert_eq!(updated, format!("{}\n", expected.join("\n")));
    }

    #[test]
    fn test_set_ini_value_appends_a_missing_key_to_an_existing_section() {
        let updated = set_ini_value(CONFIG, "Behavior", "NewKey", Some("yes"));
        let lines: Vec<&str> = updated.lines().collect();
        let key = lines
            .iter()
            .position(|line| *line == "NewKey=yes")
            .expect("the key was appended");
        let section = lines
            .iter()
            .position(|line| *line == "[Behavior]")
            .expect("the section header");
        let next = lines
            .iter()
            .position(|line| *line == "[Appearance]")
            .expect("the next section header");
        assert!(
            key > section && key < next,
            "the key lands inside the section it belongs to: {lines:?}"
        );
    }

    #[test]
    fn test_set_ini_value_creates_the_section_when_it_is_absent() {
        let updated = set_ini_value(
            "[Hotkey]\nTriggerKey=Control+space\n",
            "Behavior",
            "ActiveUserInterface",
            Some("classic"),
        );
        assert_eq!(
            updated,
            "[Hotkey]\nTriggerKey=Control+space\n\n[Behavior]\nActiveUserInterface=classic\n"
        );
    }

    #[test]
    fn test_set_ini_value_removes_the_key_when_the_value_is_none() {
        let updated = set_ini_value(CONFIG, "Behavior", "ActiveUserInterface", None);
        assert!(!updated.contains("ActiveUserInterface"), "{updated}");
        assert!(updated.contains("ShareInputState=No"), "{updated}");
        assert!(!updated.contains("[Behavior]\n\n"), "no blank hole is left");
    }

    #[test]
    fn test_set_ini_value_ignores_a_commented_out_assignment() {
        // Fcitx5's configuration files document their keys with commented examples;
        // writing into one of those would edit a line the user disabled on purpose.
        let text = "[Behavior]\n# ActiveUserInterface=classic\n";
        let updated = set_ini_value(text, "Behavior", "ActiveUserInterface", Some("rspinyin"));
        assert!(
            updated.contains("# ActiveUserInterface=classic"),
            "{updated}"
        );
        assert!(
            updated.contains("\nActiveUserInterface=rspinyin"),
            "{updated}"
        );
    }

    #[test]
    fn test_set_ini_value_does_not_touch_a_key_in_another_section() {
        let other = "[Appearance]\nActiveUserInterface=keep-me\n";
        let updated = set_ini_value(other, "Behavior", "ActiveUserInterface", Some("classic"));
        assert!(
            updated.contains("[Appearance]\nActiveUserInterface=keep-me"),
            "{updated}"
        );
        assert!(
            updated.contains("[Behavior]\nActiveUserInterface=classic"),
            "{updated}"
        );
    }

    #[test]
    fn test_record_parse_reads_a_complete_record() {
        let record = Record::parse(
            r#"{"config_file":"/tmp/fcitx5-config","section":"Behavior",
                "key":"ActiveUserInterface","previous":"classic"}"#,
            Path::new("/home/user/.config"),
        )
        .expect("a complete record");
        assert_eq!(record.config_file, PathBuf::from("/tmp/fcitx5-config"));
        assert_eq!(record.section, "Behavior");
        assert_eq!(record.key, "ActiveUserInterface");
        assert_eq!(record.previous.as_deref(), Some("classic"));
    }

    #[test]
    fn test_record_parse_falls_back_to_fcitx5s_own_configuration_file() {
        let record = Record::parse(r#"{"previous":"classic"}"#, Path::new("/home/user/.config"))
            .expect("a minimal record");
        assert_eq!(
            record.config_file,
            PathBuf::from("/home/user/.config/fcitx5/config")
        );
        assert_eq!(record.section, DEFAULT_SECTION);
        assert_eq!(record.key, DEFAULT_KEY);
    }

    #[test]
    fn test_record_parse_reads_null_as_the_key_never_existing() {
        let record = Record::parse(r#"{"previous":null}"#, Path::new("/cfg"))
            .expect("a record with a null value");
        assert_eq!(record.previous, None);
    }

    #[test]
    fn test_record_parse_refuses_a_record_it_does_not_understand() {
        // No recognisable field for the displaced value: removing a key the user chose
        // on a guess is the one outcome this must never produce.
        let failure = Record::parse(r#"{"something_else":"classic"}"#, Path::new("/cfg"))
            .expect_err("an unrecognised record is refused");
        assert!(failure.to_string().contains("previous"), "{failure}");
        assert!(Record::parse("not json", Path::new("/cfg")).is_err());
        assert!(Record::parse("[]", Path::new("/cfg")).is_err());
        assert!(Record::parse(r#"{"previous":42}"#, Path::new("/cfg")).is_err());
        assert!(Record::parse(r#"{"previous":"a","section":42}"#, Path::new("/cfg")).is_err());
    }

    #[test]
    fn test_restore_reports_no_record_when_the_plugin_never_took_over() {
        let dir = scratch("absent");
        let outcome =
            restore(&dir.join(RECORD_FILE), &dir, false).expect("absence is not an error");
        assert_eq!(outcome, Outcome::NoRecord);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_restore_puts_the_previous_value_back() {
        let dir = scratch("restore");
        let config = dir.join("fcitx5/config");
        fs::create_dir_all(config.parent().expect("the fixture has a parent"))
            .expect("creating the fixture directory");
        fs::write(&config, CONFIG.replace("=classic", "=rspinyin")).expect("writing the fixture");
        let record = dir.join(RECORD_FILE);
        fs::write(
            &record,
            format!(
                r#"{{"config_file":"{}","previous":"classic"}}"#,
                config.display()
            ),
        )
        .expect("writing the fixture");

        let outcome = restore(&record, &dir, false).expect("restoring the value");
        assert!(matches!(outcome, Outcome::Restored { value: Some(_), .. }));
        let restored = fs::read_to_string(&config).expect("reading back");
        assert!(
            restored.contains("ActiveUserInterface=classic"),
            "{restored}"
        );
        assert!(!restored.contains("rspinyin"), "{restored}");
        assert!(
            !config.with_extension("rspinyin-tmp").exists(),
            "the temporary file is renamed away"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_restore_removes_the_key_when_the_record_says_it_never_existed() {
        let dir = scratch("null");
        let config = dir.join("fcitx5/config");
        fs::create_dir_all(config.parent().expect("the fixture has a parent"))
            .expect("creating the fixture directory");
        fs::write(&config, CONFIG.replace("=classic", "=rspinyin")).expect("writing the fixture");
        let record = dir.join(RECORD_FILE);
        fs::write(
            &record,
            format!(
                r#"{{"config_file":"{}","previous":null}}"#,
                config.display()
            ),
        )
        .expect("writing the fixture");

        restore(&record, &dir, false).expect("restoring the value");
        let restored = fs::read_to_string(&config).expect("reading back");
        assert!(!restored.contains("ActiveUserInterface"), "{restored}");
        assert!(restored.contains("ShareInputState=No"), "{restored}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_restore_dry_run_changes_nothing() {
        let dir = scratch("dry-run");
        let config = dir.join("fcitx5/config");
        fs::create_dir_all(config.parent().expect("the fixture has a parent"))
            .expect("creating the fixture directory");
        fs::write(&config, CONFIG).expect("writing the fixture");
        let record = dir.join(RECORD_FILE);
        fs::write(
            &record,
            format!(
                r#"{{"config_file":"{}","previous":"something-else"}}"#,
                config.display()
            ),
        )
        .expect("writing the fixture");

        let outcome = restore(&record, &dir, true).expect("a dry run reads but does not write");
        assert!(matches!(outcome, Outcome::Planned { .. }));
        assert_eq!(
            fs::read_to_string(&config).expect("reading back"),
            CONFIG,
            "the configuration file is untouched"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_restore_tolerates_a_configuration_file_fcitx5_never_wrote() {
        let dir = scratch("no-config");
        let record = dir.join(RECORD_FILE);
        fs::write(&record, r#"{"previous":"classic"}"#).expect("writing the fixture");

        let outcome = restore(&record, &dir, false).expect("a fresh account has no config file");
        assert!(matches!(outcome, Outcome::Restored { value: Some(_), .. }));
        assert!(
            !dir.join("fcitx5/config").exists(),
            "nothing is created for a value that was never saved"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
