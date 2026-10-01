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

use crate::schema::{Config, DigitZero, KeyName, KeysConfig};

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
    let span = locate_keys_table(text);
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

/// The byte range of the `[keys]` table in a document: from the header line through the
/// last non-blank line before the next header.
#[derive(Clone, Copy)]
struct KeysSpan {
    start: usize,
    end: usize,
}

/// Finds the `[keys]` table in `text`.
///
/// The span starts at the `[keys]` header line and runs through the last non-blank line
/// before the next header of any kind, or through the end of the file: the blank lines
/// that separate one section from the next belong to the spacing and are kept out of
/// the span, which is what keeps a rewrite from tightening a user's layout.
fn locate_keys_table(text: &str) -> Option<KeysSpan> {
    let mut span: Option<KeysSpan> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        let end = start + line.len();
        offset = end;
        match span.as_mut() {
            None => {
                if is_keys_table_header(line) {
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

/// Whether the line opens the `[keys]` table.
///
/// Only the plain spelling is recognised: a quoted, escaped or array-of-tables header
/// names the table in a form the shipped document never uses, and refusing it costs a
/// write while misreading it could cost a table.
fn is_keys_table_header(line: &str) -> bool {
    let Some((name, is_array)) = header_of(line) else {
        return false;
    };
    !is_array && name == "keys"
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
mod tests {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process;

    use ime_types::ImeError;

    use super::*;
    use crate::reload::{DEFAULT_CONFIG_TOML, FILE_NAME};

    /// A scratch directory for one test, named after the test and the process id.
    fn scratch(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "rspinyin-ime-config-writeback-{}-{name}",
            process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("creating the scratch directory");
        directory
    }

    /// Writes `text` to `path`.
    fn write_text(path: &Path, text: &str) {
        fs::write(path, text).expect("writing the test file");
    }

    /// The text of `path`.
    fn read_text(path: &Path) -> String {
        fs::read_to_string(path).expect("reading the test file")
    }

    /// A `[keys]` section from its parts.
    fn bindings(
        digit_zero: DigitZero,
        enter_commit_raw: bool,
        flip: &[KeyName],
        highlight: &[KeyName],
    ) -> KeysConfig {
        KeysConfig {
            digit_zero,
            enter_commit_raw,
            flip_keys: flip.to_vec(),
            highlight_keys: highlight.to_vec(),
        }
    }

    /// The configuration `text` parses to, requiring that it parses with no diagnostic.
    fn parsed(text: &str) -> Config {
        let (config, warnings) = Config::from_document(text).expect("the document parses");
        assert!(warnings.is_empty(), "no diagnostics: {warnings:?}");
        config
    }

    /// A document with comments, blank lines and two sections around the `[keys]` one.
    const PRESERVED_BEFORE: &str = concat!(
        "# header comment\n",
        "schema_version = 2\n",
        "\n",
        "[engine]\n",
        "# engine comment\n",
        "max_raw_len = 32\n",
        "\n",
    );

    /// The text the rewrite must keep behind the `[keys]` region.
    const PRESERVED_AFTER: &str = "\n[theme]\n# theme comment\nscheme = \"dark\"\n";

    /// The `[keys]` block the rendered bindings must come out as, spelled here rather
    /// than taken from the module so the format is pinned, not assumed.
    const PRESERVED_BLOCK: &str = concat!(
        "[keys]\n",
        "digit_zero = \"flip\"\n",
        "enter_commit_raw = true\n",
        "flip_keys = [\"minus\", \"equal\"]\n",
        "highlight_keys = [\"tab\", \"shift_tab\"]\n",
    );

    #[test]
    fn test_write_keys_rewrites_only_the_keys_table_and_preserves_the_rest() {
        let path = scratch("preserve").join(FILE_NAME);
        let new = bindings(
            DigitZero::Flip,
            true,
            &[KeyName::Minus, KeyName::Equal],
            &[KeyName::Tab, KeyName::ShiftTab],
        );
        // What the caller loaded: the document with its own `[keys]` region intact.
        let original =
            format!("{PRESERVED_BEFORE}[keys]\ndigit_zero = \"passthrough\"\n{PRESERVED_AFTER}");
        write_text(&path, &original);
        let previous = parsed(&original).keys;

        write_keys(&path, &previous, &new).expect("the write succeeds");

        let written = read_text(&path);
        assert_eq!(
            written,
            format!("{PRESERVED_BEFORE}{PRESERVED_BLOCK}{PRESERVED_AFTER}")
        );
        assert_eq!(parsed(&written).keys, new);
    }

    #[test]
    fn test_write_keys_without_a_keys_table_appends_one_the_parser_accepts() {
        let path = scratch("append").join(FILE_NAME);
        let original = "# my settings\n\n[engine]\nmax_raw_len = 32\n";
        write_text(&path, original);
        let new = bindings(
            DigitZero::Flip,
            false,
            &[KeyName::PageUp, KeyName::PageDown],
            &[KeyName::Up, KeyName::Down],
        );

        // The file declares no `[keys]` table, so the values in force were the defaults.
        write_keys(&path, &Config::default().keys, &new).expect("the write succeeds");

        let written = read_text(&path);
        assert!(written.starts_with(original), "the file is kept verbatim");
        assert!(
            written[original.len()..].starts_with("\n[keys]"),
            "behind a blank line"
        );
        assert_eq!(parsed(&written).keys, new);
    }

    #[test]
    fn test_write_keys_region_changed_after_the_load_reports_a_race_and_writes_nothing() {
        let path = scratch("race").join(FILE_NAME);
        let loaded = "[keys]\ndigit_zero = \"passthrough\"\nflip_keys = [\"minus\"]\n";
        write_text(&path, loaded);
        let previous = parsed(loaded).keys;
        let new = bindings(DigitZero::Flip, true, &[KeyName::Equal], &[KeyName::Tab]);
        // Another writer replaces the section between the load and the write.
        let edited = "[keys]\ndigit_zero = \"flip\"\nflip_keys = [\"equal\"]\n";
        write_text(&path, edited);

        let result = write_keys(&path, &previous, &new);

        match result {
            Err(WritebackError::Race { reason }) => assert!(!reason.is_empty()),
            other => panic!("expected a race, got {other:?}"),
        }
        assert_eq!(read_text(&path), edited, "the file is untouched");
    }

    #[test]
    fn test_write_keys_same_bindings_written_twice_leaves_the_file_byte_identical() {
        let path = scratch("idempotent").join(FILE_NAME);
        write_text(&path, DEFAULT_CONFIG_TOML);
        let previous = parsed(DEFAULT_CONFIG_TOML).keys;
        let new = bindings(
            DigitZero::Flip,
            true,
            &[KeyName::Up, KeyName::Down],
            &[KeyName::Left, KeyName::Right],
        );

        write_keys(&path, &previous, &new).expect("the first write succeeds");
        let once = read_text(&path);
        // The caller's loaded values after the first write are the ones it wrote.
        write_keys(&path, &new, &new).expect("the second write succeeds");

        assert_eq!(once, read_text(&path), "the second write changes nothing");
        assert_eq!(parsed(&read_text(&path)).keys, new);
    }

    #[test]
    fn test_write_keys_unwritable_directory_returns_the_typed_error_without_panicking() {
        let directory = scratch("unwritable");
        let path = directory.join(FILE_NAME);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
            .expect("narrowing the scratch directory");
        // A process that owns the directory -- root, or a supervisor -- sees the mode
        // but not the restriction; the probe skips honestly where it cannot be tested.
        if fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .is_ok()
        {
            restore(&directory);
            return;
        }
        let new = bindings(DigitZero::Flip, false, &[KeyName::Minus], &[KeyName::Tab]);

        let result = write_keys(&path, &Config::default().keys, &new);

        restore(&directory);
        match result {
            Err(WritebackError::Io(_)) => {}
            other => panic!("expected an io error, got {other:?}"),
        }
        assert!(!path.exists(), "nothing is created where nothing can be");
    }

    /// Hands a narrowed scratch directory its modes back, so its cleanup can remove it.
    fn restore(directory: &Path) {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .expect("restoring the scratch directory");
    }

    #[test]
    fn test_write_keys_created_file_and_directory_carry_the_private_modes() {
        let directory = scratch("modes");
        let path = directory.join("nested").join(FILE_NAME);
        let new = bindings(DigitZero::Passthrough, true, &[KeyName::Tab], &[]);

        write_keys(&path, &Config::default().keys, &new).expect("the write succeeds");

        let file_mode = fs::metadata(&path)
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);
        let parent = path.parent().expect("the file has a parent");
        let dir_mode = fs::metadata(parent)
            .expect("the directory exists")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(parsed(&read_text(&path)).keys, new);
    }

    #[test]
    fn test_write_errors_convert_to_ime_error_with_the_stable_codes() {
        let path = scratch("codes").join(FILE_NAME);
        let loaded = "[keys]\ndigit_zero = \"passthrough\"\n";
        write_text(&path, loaded);
        let previous = parsed(loaded).keys;
        let new = bindings(DigitZero::Flip, false, &[], &[]);
        write_text(&path, "[keys]\ndigit_zero = \"flip\"\n");

        let raced = ImeError::from(write_keys(&path, &previous, &new).expect_err("races"));
        assert!(
            raced
                .to_string()
                .starts_with("config/invalid: config/writeback-race"),
            "{raced}"
        );

        // A file sitting where the configuration directory belongs cannot hold the file.
        let blocked = scratch("codes-io").join("blocker");
        write_text(&blocked, "not a directory");
        let failed = write_keys(&blocked.join(FILE_NAME), &Config::default().keys, &new)
            .expect_err("cannot be written");
        let converted = ImeError::from(failed);
        assert!(
            converted
                .to_string()
                .starts_with("config/invalid: config ("),
            "{converted}"
        );

        let refused = WritebackError::Refused {
            reason: String::from("would not parse"),
        };
        assert!(
            ImeError::from(refused)
                .to_string()
                .starts_with("config/invalid: config ("),
            "refused"
        );
    }

    #[test]
    fn test_write_keys_string_value_shaped_like_a_header_races_and_writes_nothing() {
        let path = scratch("string-value").join(FILE_NAME);
        // The `[keys]` line is the *value* of `phrases.file`; the scanner reads it as one.
        let original = "[phrases]\nfile = \"\"\"\n[keys]\n\"\"\"\n";
        write_text(&path, original);
        let previous = parsed(original).keys;
        let new = bindings(DigitZero::Flip, false, &[], &[]);

        let result = write_keys(&path, &previous, &new);

        match result {
            Err(WritebackError::Race { .. }) => {}
            other => panic!("expected a race, got {other:?}"),
        }
        assert_eq!(read_text(&path), original, "the file is untouched");
    }

    #[test]
    fn test_digit_zero_spelling_round_trips_through_the_closed_set() {
        for value in [DigitZero::Passthrough, DigitZero::Flip] {
            let spelling = digit_zero_spelling(value);
            assert_eq!(DigitZero::try_from(spelling).expect("in the set"), value);
        }
    }
}
