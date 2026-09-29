//! The filesystem half of the configuration module.
//!
//! Responsibility: where the configuration file is ([`default_path`]), what reading it
//! produced (the `Document` this module keeps to itself), the loader that turns a path
//! into a configuration ([`Config::load_at`]), the template written when there is no
//! file yet, and the backup a file that cannot be parsed is moved to.
//!
//! Boundaries: the document model and the merge over the built-in defaults live in the
//! parent module, and the configuration in force lives in the `store` sibling. Nothing
//! here decides what a document means, only what is on the disk. Nothing here is on the
//! host thread's key path either: a load happens once, at startup.
//!
//! Every failure is a diagnostic rather than a failure to start. A missing file, a file
//! that cannot be read and a file that is not TOML all answer the built-in defaults and
//! say why, because an input method that will not start is worse than one that starts
//! with the shipped settings.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ime_types::ImeError;

use super::{DEFAULT_CONFIG_TOML, DOCUMENT_KEY, FILE_NAME, MAX_BACKUP_ATTEMPTS};
use crate::schema::{Config, Warnings};

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
pub(super) fn unix_secs() -> u64 {
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
            Document::Text(text) => match Self::from_document_at(&text, Some(path)) {
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
