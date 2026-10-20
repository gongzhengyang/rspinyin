//! Writing user keymap changes back into `config.toml`.
//!
//! Responsibility: replace the `[keys]` table of an existing configuration file with a
//! freshly rendered one, append the table when the file has none, or create a minimal
//! file when there is none -- and touch nothing else. Comments, blank lines, other
//! sections and their key order travel through the write byte for byte, because a user
//! who documented their configuration inside the file must not find that documentation
//! rewritten by the plugin. The document model lives in `crate::schema` and the rest of
//! the file's life cycle in `crate::reload`; this module is the text work between the
//! two -- it locates the one table, renders it, and reports through [`WritebackError`]
//! without deciding any policy: a refusal is a value the caller interprets, and a disk
//! it cannot write to is an error return, never a panic.
//!
//! # Why callers pass the values they loaded
//!
//! [`write_keys`] takes the `[keys]` values the caller loaded alongside the ones it
//! wants to write. Between a load and a write sits everything a user can do -- including
//! editing the file themselves. Writing the caller's bindings over a section the user
//! has since changed would answer their edit with silence: the file would look saved
//! and hold the wrong settings. Before any write, the module re-reads the file, parses
//! the region it is about to replace, and requires it still to resolve to the loaded
//! values; on a mismatch it writes nothing and reports the stable code
//! `config/writeback-race`. The check narrows the window between load and write; it
//! cannot close it -- there is no lock on a file the user owns -- so the guarantee is
//! "refuse rather than clobber", not "cannot happen".
//!
//! # What the replaced region is
//!
//! The region runs from the `[keys]` header line to the last non-blank line before the
//! next table header, or to the end of the file. Blank lines between the region and the
//! next header are spacing and are kept; blank lines and comments inside the region
//! belong to the table and are replaced by the rendered block. A header the scanner
//! does not recognise -- quoted, escaped, or a `[keys]`-shaped line inside a multi-line
//! string value -- makes the write fail safe rather than fail wrong: the document the
//! write would produce would not parse as TOML, and is refused, never written.
//!
//! # Files the module creates
//!
//! A file this module creates carries mode `0600` and a directory it creates `0700`,
//! the same private modes the user-data store keeps. A file that already exists keeps
//! its own mode: the write goes through a temporary file beside the target that a
//! rename puts in place, so the path holds one whole configuration at every instant.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use ime_types::ImeError;

use crate::schema::{Config, DataConfig, DigitZero, Durability, KeyName, KeysConfig};

/// The stable code a refused write carries when the file no longer holds the values the
/// caller loaded.
///
/// It travels in the `key` field of [`ImeError::ConfigInvalid`] -- where every code
/// without a variant of its own in the frozen error model travels -- so it renders as
/// `config/invalid: config/writeback-race (...)` and stays matchable by diagnostics and
/// tests, exactly as the routing layer's own codes do.
pub const WRITEBACK_RACE_CODE: &str = "config/writeback-race";

/// Mode of a configuration file this module creates.
const FILE_MODE: u32 = 0o600;
/// Mode of a configuration directory this module creates.
const DIR_MODE: u32 = 0o700;
/// The suffix the document under construction is built under beside the file it
/// replaces.
const TEMP_SUFFIX: &str = ".writing";
/// How many `.1`, `.2`, ... suffixes are tried before a temporary name is given up on.
const MAX_TEMP_ATTEMPTS: u32 = 100;
/// The key name reported for a failure that concerns the whole document rather than one
/// of its keys, matching the loader's own spelling.
const DOCUMENT_KEY: &str = "config";

/// Why a keymap write did not happen.
///
/// Every variant is a refusal to write, not a partial write: the file on disk is either
/// untouched or, on success, exactly the composed document. The conversion into the
/// frozen error type folds each variant onto [`ImeError::ConfigInvalid`], keeping the
/// stable codes matchable in the rendered message.
#[derive(Debug)]
pub enum WritebackError {
    /// The file could not be read, created, or written: a directory that cannot be
    /// entered or written, a path that is not a file, a full disk. The caller decides
    /// whether this degrades into read-only mode; nothing was written.
    Io(io::Error),
    /// The `[keys]` region no longer holds the values the caller loaded, so writing
    /// would clobber an edit this layer cannot see. Nothing was written.
    Race {
        /// What the re-read found instead of the loaded values.
        reason: String,
    },
    /// The document the write would produce does not parse, so the region this module
    /// located cannot be the one the file declares. Nothing was written. Defensive depth
    /// for a region misidentified under a document shape the scanner does not model.
    Refused {
        /// Why the composed document is not a document.
        reason: String,
    },
}

impl From<io::Error> for WritebackError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WritebackError> for ImeError {
    // A writeback failure has no variant of its own in the frozen error model, so it
    // folds onto `ConfigInvalid`; the stable writeback code travels in the `key` field,
    // where the routing layer keeps its own codes, so diagnostics match one spelling.
    fn from(error: WritebackError) -> Self {
        match error {
            WritebackError::Io(error) => ImeError::ConfigInvalid {
                key: String::from(DOCUMENT_KEY),
                reason: format!("cannot write the key bindings: {error}"),
            },
            WritebackError::Race { reason } => ImeError::ConfigInvalid {
                key: String::from(WRITEBACK_RACE_CODE),
                reason,
            },
            WritebackError::Refused { reason } => ImeError::ConfigInvalid {
                key: String::from(DOCUMENT_KEY),
                reason,
            },
        }
    }
}

/// Replaces the `[keys]` table of the configuration file at `path` with one rendered
/// from `new`, preserving every other byte of the file.
///
/// # Parameters
///
/// - `path`: the configuration file. A missing file is created, holding only the
///   rendered `[keys]` table.
/// - `previous`: the `[keys]` values the caller loaded from `path` -- the repaired
///   section in force, as [`Config::from_document`] or the loader produced it. It is
///   what the race check compares the current region against; no values loaded means
///   the built-in defaults.
/// - `new`: the bindings to write. The caller owns validating them; this module renders
///   whatever it is given.
///
/// # Errors
///
/// [`WritebackError::Io`] when the file or its directory cannot be read or written;
/// [`WritebackError::Race`] when the file's `[keys]` region no longer resolves to
/// `previous`; [`WritebackError::Refused`] when the composed document would not parse.
/// Nothing is written in any of those cases.
///
/// # Panics
///
/// Never: every failure is a returned error.
pub fn write_keys(
    path: &Path,
    previous: &KeysConfig,
    new: &KeysConfig,
) -> Result<(), WritebackError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(WritebackError::Io(error)),
    };
    let rendered = render_keys(new);
    match text {
        Some(text) => rewrite(path, &text, previous, &rendered),
        // A file that is not there has nothing to race with: the caller's loaded values
        // cannot be clobbered by creating what was missing.
        None => create(path, &rendered),
    }
}

/// Rewrites the `[keys]` region of `text` and puts the result back at `path`.
fn rewrite(
    path: &Path,
    text: &str,
    previous: &KeysConfig,
    rendered: &str,
) -> Result<(), WritebackError> {
    let span = locate_table(text, "keys");
    if let Err(reason) = region_matches_previous(text, span, previous) {
        return Err(WritebackError::Race { reason });
    }
    let composed = match span {
        Some(span) => compose_replacement(text, span, rendered),
        None => compose_append(text, rendered),
    };
    // The composed document is what the file will hold, so it must be a document. A
    // failure here says the region this module located cannot be the one the file
    // declares -- the fail-safe for a header the scanner mistook -- and the write is
    // refused rather than a file broken.
    if let Err(error) = Config::from_document(&composed) {
        return Err(WritebackError::Refused {
            reason: format!(
                "the document this write would produce does not parse, so the [keys] \
                 region was not where this module expected it and nothing was written: \
                 {error}"
            ),
        });
    }
    replace_file(path, &composed).map_err(WritebackError::from)
}

/// Creates the configuration file at `path` holding only the rendered `[keys]` table.
fn create(path: &Path, rendered: &str) -> Result<(), WritebackError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return create_file(path, rendered);
    };
    // The directory is created private: a configuration names no one but its owner.
    // Components that already exist are left exactly as they are.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(parent)?;
    create_file(path, rendered)
}

/// Creates the file itself, private to its owner.
///
/// # Errors
///
/// A [`WritebackError::Io`] when the path cannot be claimed or written. The path is
/// created, never overwritten: a file that appeared in the meantime is left alone.
fn create_file(path: &Path, rendered: &str) -> Result<(), WritebackError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)?;
    file.write_all(rendered.as_bytes())?;
    // `open` applies the mode through the process umask, which can only clear bits, so
    // the mode is set again through the descriptor just opened: the file never exists
    // with a wider mode than the one asked for.
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
        .map_err(WritebackError::from)
}

/// Builds `text` in a temporary file beside `path` and renames it over `path`, keeping
/// the mode the file already had.
///
/// # Errors
///
/// The underlying [`io::Error`] when the mode cannot be read, when no temporary name
/// is free, or when the write or the rename fails; `path` is left untouched either way.
fn replace_file(path: &Path, text: &str) -> io::Result<()> {
    let permissions = fs::metadata(path)?.permissions();
    let (temp_path, mut temp) = claim_free_file(&suffixed(path, TEMP_SUFFIX), FILE_MODE)?;
    // Same publish discipline as the migration's replace: the bytes reach the device
    // before the rename, so the name never leads the contents.
    let written = temp
        .write_all(text.as_bytes())
        .and_then(|()| fs::set_permissions(&temp_path, permissions))
        .and_then(|()| temp.sync_all());
    drop(temp);
    if let Err(error) = written {
        discard(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        discard(&temp_path);
        return Err(error);
    }
    Ok(())
}

/// Claims the first free `base`, `.base.1`, ... name and opens it for writing at `mode`.
///
/// Nothing is ever overwritten, so a name that is already taken -- a temporary a
/// crashed run left behind -- costs a suffix rather than a file.
///
/// # Errors
///
/// The underlying [`io::Error`] when no candidate name is free or the directory cannot
/// be written to.
fn claim_free_file(base: &Path, mode: u32) -> io::Result<(PathBuf, fs::File)> {
    for attempt in 0..MAX_TEMP_ATTEMPTS {
        let candidate = if attempt == 0 {
            base.to_path_buf()
        } else {
            suffixed(base, &format!(".{attempt}"))
        };
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&candidate)
        {
            Ok(file) => {
                // `open` applies the mode through the process umask, which can only
                // clear bits, so the mode is set again through the descriptor just
                // opened: the file never exists with a wider mode than requested.
                if let Err(error) = file.set_permissions(fs::Permissions::from_mode(mode)) {
                    discard(&candidate);
                    return Err(error);
                }
                return Ok((candidate, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        String::from("every temporary name beside the configuration file is taken"),
    ))
}

/// Removes a file this module created; best effort on purpose, because every caller is
/// already on a path that reports a failure.
fn discard(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Appends `suffix` to `path`, which carries no separator of its own.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Replaces the `[data]` table of the configuration file at `path` with one rendered
/// from `new`, preserving every other byte of the file.
///
/// The hot swap's write-back: clearing `dict_staged_path` once a staged dictionary has
/// been swapped in, so the next reload does not swap the same file again. The race
/// discipline is [`write_keys`]' own -- the region must still resolve to `previous`
/// before anything is written.
///
/// # Parameters
///
/// - `path`: the configuration file. A missing file is created, holding only the
///   rendered `[data]` table.
/// - `previous`: the `[data]` values the caller loaded from `path`, as the loader
///   produced them.
/// - `new`: the section to write; the swap clears `dict_staged_path` on a clone of
///   `previous` and passes that.
///
/// # Errors
///
/// As [`write_keys`].
///
/// # Panics
///
/// Never: every failure is a returned error.
pub fn write_data(path: &Path, previous: &DataConfig, new: &DataConfig) -> Result<(), WritebackError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(WritebackError::Io(error)),
    };
    let rendered = render_data(new);
    match text {
        Some(text) => rewrite_data(path, &text, previous, &rendered),
        None => create(path, &rendered),
    }
}

/// Rewrites the `[data]` region of `text` and puts the result back at `path`.
fn rewrite_data(
    path: &Path,
    text: &str,
    previous: &DataConfig,
    rendered: &str,
) -> Result<(), WritebackError> {
    let span = locate_table(text, "data");
    if let Err(reason) = region_matches_previous_data(text, span, previous) {
        return Err(WritebackError::Race { reason });
    }
    let composed = match span {
        Some(span) => compose_replacement(text, span, rendered),
        None => compose_append(text, rendered),
    };
    if let Err(error) = Config::from_document(&composed) {
        return Err(WritebackError::Refused {
            reason: format!(
                "the document this write would produce does not parse, so the [data] \
                 region was not where this module expected it and nothing was written: \
                 {error}"
            ),
        });
    }
    replace_file(path, &composed).map_err(WritebackError::from)
}

/// Verifies the `[data]` region about to be replaced still resolves to the values the
/// caller loaded, as [`region_matches_previous`] does for `[keys]`.
fn region_matches_previous_data(
    text: &str,
    span: Option<KeysSpan>,
    previous: &DataConfig,
) -> Result<(), String> {
    let Some(span) = span else {
        if *previous == Config::default().data {
            return Ok(());
        }
        return Err(String::from(
            "the file has no [data] table, so it no longer holds the values this session \
             loaded",
        ));
    };
    let fragment = &text[span.start..span.end];
    let (parsed, _) = Config::from_document(fragment)
        .map_err(|error| format!("the [data] table no longer parses: {error}"))?;
    if parsed.data != *previous {
        return Err(String::from(
            "the [data] table no longer holds the values this session loaded",
        ));
    }
    Ok(())
}

/// Verifies the region about to be replaced still resolves to the values the caller
/// loaded, which is what makes a write refuse rather than clobber a user's edit.
///
/// A region that no longer parses and a region that parses to other values are the
/// same refusal: the file is not what the caller read. A file with no `[keys]` table
/// at all matches a caller that loaded no `[keys]` values -- the defaults -- alone.
fn region_matches_previous(
    text: &str,
    span: Option<KeysSpan>,
    previous: &KeysConfig,
) -> Result<(), String> {
    let Some(span) = span else {
        if *previous == Config::default().keys {
            return Ok(());
        }
        return Err(String::from(
            "the file has no [keys] table, so it no longer holds the values this session \
             loaded",
        ));
    };
    let fragment = &text[span.start..span.end];
    // The region is compared through the crate's own reader rather than a second
    // parser: `previous` is what that reader answers for the file, so it is the reader
    // the file is held against. A table the reader repairs -- a repeated entry, a key
    // both lists claim -- resolves to the same repaired values it resolved to at load,
    // and a table with a value of the wrong type does not parse at all, which is a
    // refusal.
    let (parsed, _) = Config::from_document(fragment)
        .map_err(|error| format!("the [keys] table no longer parses: {error}"))?;
    if parsed.keys != *previous {
        return Err(String::from(
            "the [keys] table no longer holds the values this session loaded",
        ));
    }
    Ok(())
}

/// The byte range of one table in a document: from the header line through the last
/// non-blank line before the next header.
///
/// The name is the `[keys]` scan's, because that is the table it was written for; the
/// `[data]` rewrite reuses the scan and names nothing of its own.
#[derive(Clone, Copy)]
struct KeysSpan {
    start: usize,
    end: usize,
}

/// Finds the table `name` names in `text`.
///
/// The span starts at the table's header line and runs through the last non-blank line
/// before the next header of any kind, or through the end of the file: the blank lines
/// that separate one section from the next belong to the spacing and are kept out of
/// the span, which is what keeps a rewrite from tightening a user's layout.
fn locate_table(text: &str, name: &str) -> Option<KeysSpan> {
    let mut span: Option<KeysSpan> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        let end = start + line.len();
        offset = end;
        match span.as_mut() {
            None => {
                if is_table_header_of(line, name) {
                    span = Some(KeysSpan { start, end });
                }
            }
            Some(current) => {
                if is_table_header(line) {
                    break;
                }
                if !line.trim().is_empty() {
                    current.end = end;
                }
            }
        }
    }
    span
}

/// Whether the line opens any TOML table, which is where one table's region ends.
fn is_table_header(line: &str) -> bool {
    header_of(line).is_some()
}

/// Whether the line opens the table `name` names.
///
/// Only the plain spelling is recognised: a quoted, escaped or array-of-tables header
/// names the table in a form the shipped document never uses, and refusing it costs a
/// write while misreading it could cost a table.
fn is_table_header_of(line: &str, name: &str) -> bool {
    let Some((found, is_array)) = header_of(line) else {
        return false;
    };
    !is_array && found == name
}

/// Parses the table header a line opens, answering with the key path as written, quotes
/// and all, and whether it opens an array of tables (`[[name]]`).
///
/// A header starts with `[`, closes on the same line, and carries nothing but a comment
/// after its closing bracket; a line that merely looks like one -- an element of a
/// multi-line array, a stray bracket -- fails the remainder check and answers `None`.
fn header_of(line: &str) -> Option<(&str, bool)> {
    let trimmed = line.trim_start();
    let is_array = trimmed.starts_with("[[");
    let rest = trimmed.strip_prefix('[')?;
    let rest = if is_array {
        rest.strip_prefix('[')?
    } else {
        rest
    };
    let close = closing_bracket(rest)?;
    let name = rest[..close].trim();
    let mut remainder = rest[close + 1..].trim_start();
    if is_array {
        remainder = remainder.strip_prefix(']')?.trim_start();
    }
    if remainder.is_empty() || remainder.starts_with('#') {
        return Some((name, is_array));
    }
    None
}

/// The offset of the first `]` in `text` that no string opens across.
fn closing_bracket(text: &str) -> Option<usize> {
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;
    for (offset, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_basic => escaped = true,
            '"' if in_basic => in_basic = false,
            '\'' if in_literal => in_literal = false,
            '"' if !in_literal => in_basic = true,
            '\'' if !in_basic => in_literal = true,
            ']' if !in_basic && !in_literal => return Some(offset),
            _ => {}
        }
    }
    None
}

/// Renders the `[keys]` table for `keys`, in the order the documented template states
/// them, in the spellings the loader accepts, and always ending with a newline so
/// whatever follows it starts on a fresh line.
fn render_keys(keys: &KeysConfig) -> String {
    format!(
        "[keys]\n\
         digit_zero = \"{}\"\n\
         enter_commit_raw = {}\n\
         flip_keys = [{}]\n\
         highlight_keys = [{}]\n",
        digit_zero_spelling(keys.digit_zero),
        keys.enter_commit_raw,
        render_names(&keys.flip_keys),
        render_names(&keys.highlight_keys),
    )
}

/// The spelling `keys.digit_zero` writes a value with.
///
/// The closed set in `crate::schema` owns the spellings and its `TryFrom` accepts them;
/// this is the reverse direction it does not generate, and a test holds the two
/// together so a changed spelling cannot drift past silently.
fn digit_zero_spelling(value: DigitZero) -> &'static str {
    match value {
        DigitZero::Passthrough => "passthrough",
        DigitZero::Flip => "flip",
    }
}

/// Renders one key-binding list as a TOML array of quoted names.
fn render_names(names: &[KeyName]) -> String {
    names
        .iter()
        .map(|name| format!("\"{}\"", name.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders the `[data]` table for `data`, in the order the shipped template states
/// them, in the spellings the loader accepts, and always ending with a newline so
/// whatever follows it starts on a fresh line.
fn render_data(data: &DataConfig) -> String {
    format!(
        "[data]\n\
         durability = \"{}\"\n\
         backup_enabled = {}\n\
         backup_keep = {}\n\
         decay_half_life_ticks = {}\n\
         decay_floor = {}\n\
         dict_staged_path = \"{}\"\n",
        durability_spelling(data.durability),
        data.backup_enabled,
        data.backup_keep,
        data.decay_half_life_ticks,
        data.decay_floor,
        data.dict_staged_path,
    )
}

/// The spelling `data.durability` writes a value with.
///
/// The closed set in `crate::schema` owns the spellings and its `TryFrom` accepts them;
/// this is the reverse direction it does not generate, the same shape
/// [`digit_zero_spelling`] is.
fn durability_spelling(value: Durability) -> &'static str {
    match value {
        Durability::Eventual => "eventual",
        Durability::Immediate => "immediate",
    }
}

/// Replaces the span with `rendered`, keeping the text before and after it verbatim.
fn compose_replacement(text: &str, span: KeysSpan, rendered: &str) -> String {
    let mut composed = String::with_capacity(text.len() + rendered.len());
    composed.push_str(&text[..span.start]);
    composed.push_str(rendered);
    composed.push_str(&text[span.end..]);
    composed
}

/// Appends `rendered` after `text`, behind one blank line.
fn compose_append(text: &str, rendered: &str) -> String {
    let mut composed = String::with_capacity(text.len() + rendered.len() + 2);
    composed.push_str(text);
    if !text.is_empty() {
        if !text.ends_with('\n') {
            composed.push('\n');
        }
        composed.push('\n');
    }
    composed.push_str(rendered);
    composed
}

#[cfg(test)]
mod tests;
