//! The frozen error codes, read out of the documents that declare them.
//!
//! Responsibility: build the set `assert_error_codes_known` checks a log against, by extracting
//! the codes from their declaring sources -- never by copying them into a list. A hand-written
//! whitelist is a second statement of the contract that nothing compares against the first, and
//! it drifts silently: a code renamed in the contract would simply stop matching, and every case
//! would keep passing because the whitelist still held the old spelling.
//!
//! # The three sources
//!
//! | Source | What it declares | Why it is read |
//! |---|---|---|
//! | `crates/ime-types/src/error.rs` | the `#[error("...")]` attributes of the six error enums | the codes a cross-boundary failure renders as |
//! | `docs/dev/features.md` 2.2.4 | a readable copy of those enums, plus three tables of codes subsystems raise directly | the codes the contract registers outside the enums, and the copy the document promises matches the source verbatim |
//! | `crates/**/*.rs` | `const <NAME>_CODE: &str = "<code>";` | the codes a subsystem states as a constant instead of as an enum variant |
//!
//! The second source is read for two different things and both matter. Its fenced copy of the
//! enums is the cell `features.md` 2.2.4 says "必须与 `crates/ime-types/src/error.rs` 逐字一致",
//! so [`EnumCodes`] can compare the two documents instead of trusting them. Its tables carry the
//! codes that have no `ImeError` variant at all -- `ui/select/timeout`, the `ffi/*` family, the
//! `lifecycle/*` family -- which a plugin logs just the same and which an enums-only whitelist
//! would reject.
//!
//! The third source is what makes the set usable. A subsystem that degrades on its own -- the
//! path layer's `data/perms/fixed`, the crash handler's `crash/panic` -- states its code as a
//! constant rather than as an enum variant, and none of them appear in either of the first two
//! sources. Without this one, `assert_error_codes_known` would fail a session for logging
//! exactly the degradations it is supposed to report.
//!
//! # What a code looks like
//!
//! The shape is the contract's `domain/action/reason`: at least two `/`-separated segments, each
//! made of lower-case ASCII letters, digits and hyphens -- `dict/corrupt`, `ui/stale-select`,
//! `platform/x11/no-compositor`. The predicate is written out rather than pulled from a
//! pattern-matching crate: it is a test over one short string, and the workspace keeps its
//! dependency set small on purpose.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::error::LogError;

/// The Rust source that declares the cross-boundary error enums.
pub const ERROR_SOURCE: &str = "crates/ime-types/src/error.rs";

/// The document that carries the readable copy of those enums and the diagnostic-code tables.
pub const SPEC_SOURCE: &str = "docs/dev/features.md";

/// The heading that opens the error-code section of [`SPEC_SOURCE`].
pub const SPEC_SECTION: &str = "#### 2.2.4";

/// The directory whose `*_CODE` constants are read.
pub const CRATE_SOURCES: &str = "crates";

/// The suffix a diagnostic-code constant's name carries.
const CODE_SUFFIX: &str = "CODE";

/// A directory name never descended into.
const BUILD_DIRECTORY: &str = "target";

/// Which document a code was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorCodeSource {
    /// An `#[error("...")]` attribute of [`ERROR_SOURCE`].
    ErrorEnum,
    /// The copy of those enums inside [`SPEC_SOURCE`] 2.2.4.
    SpecEnums,
    /// A row of one of the diagnostic-code tables of [`SPEC_SOURCE`] 2.2.4.
    SpecTables,
    /// A `*_CODE` string constant under [`CRATE_SOURCES`].
    Constant,
}

impl ErrorCodeSource {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::ErrorEnum => "the error enums",
            Self::SpecEnums => "the specification's copy of the enums",
            Self::SpecTables => "the specification's diagnostic tables",
            Self::Constant => "the code constants",
        }
    }
}

/// The codes one Rust source declares, grouped by the enum that declares them.
///
/// A code the source declares outside any enum is not recorded: the only Rust source this is
/// used on is an enum file, and a declaration outside an enum would be a `*_CODE` constant,
/// which [`ErrorCodeIndex::add_constants`] reads instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnumCodes {
    /// One entry per enum, keyed by the enum's name, holding the codes it declares.
    by_enum: BTreeMap<String, BTreeSet<String>>,
}

impl EnumCodes {
    /// Reads the `#[error("...")]` attributes of every enum in `source`.
    ///
    /// # Return value
    ///
    /// One entry per enum the source declares, whether or not it declares a code: an enum whose
    /// attributes are all prose -- `DictError`, whose variants render `magic mismatch` and
    /// `crc mismatch: ...` rather than a `domain/action/reason` code -- is recorded with an
    /// empty set, which is what lets a comparison notice that the document's copy of it gained
    /// or lost one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(source: &str) -> Self {
        let mut by_enum: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut current: Option<String> = None;
        let mut pending: Option<String> = None;
        for line in source.lines() {
            if pending.is_some() {
                let mut open = pending.take().unwrap_or_default();
                open.push('\n');
                open.push_str(line);
                if open.contains(")]") {
                    record(&mut by_enum, current.as_deref(), &open);
                } else {
                    pending = Some(open);
                }
                continue;
            }
            let trimmed = line.trim();
            if let Some(name) = enum_name(trimmed) {
                by_enum.entry(name.clone()).or_default();
                current = Some(name);
                continue;
            }
            if trimmed == "}" && !line.starts_with(' ') && !line.starts_with('\t') {
                current = None;
                continue;
            }
            if trimmed.starts_with("#[error(") {
                // The attribute may be written on one line or spread over several; either way
                // it is collected until its closing bracket.
                let open = String::from(trimmed);
                if open.contains(")]") {
                    record(&mut by_enum, current.as_deref(), &open);
                } else {
                    pending = Some(open);
                }
            }
        }
        Self { by_enum }
    }

    /// Every enum the source declares, in name order.
    pub fn enums(&self) -> Vec<&str> {
        self.by_enum.keys().map(String::as_str).collect()
    }

    /// The codes one enum declares, or `None` when the source does not declare that enum.
    pub fn codes(&self, enum_name: &str) -> Option<&BTreeSet<String>> {
        self.by_enum.get(enum_name)
    }

    /// Every code the source declares, from every enum.
    pub fn all(&self) -> BTreeSet<&str> {
        self.by_enum
            .values()
            .flat_map(|codes| codes.iter().map(String::as_str))
            .collect()
    }

    /// How many enums the source declares.
    pub fn len(&self) -> usize {
        self.by_enum.len()
    }

    /// Whether the source declares no enum at all.
    pub fn is_empty(&self) -> bool {
        self.by_enum.is_empty()
    }
}

/// The frozen error-code set: every code the declaring sources name, and where each was found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorCodeIndex {
    /// One entry per code, holding the sources that declare it.
    sources: BTreeMap<String, BTreeSet<ErrorCodeSource>>,
}

impl ErrorCodeIndex {
    /// Builds the index from the text of the two documents that declare codes in prose.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::CodeSource`] when the source declares no enum at all, when the
    /// specification has no 2.2.4 section, or when that section declares no code. An index that
    /// silently came out empty would make every code look unknown; one that silently lost a
    /// document would make a whole domain look unknown.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_sources(errors_rs: &str, spec: &str) -> Result<Self, LogError> {
        let enums = EnumCodes::parse(errors_rs);
        if enums.is_empty() {
            return Err(LogError::CodeSource {
                document: ERROR_SOURCE,
                detail: String::from("it declares no enum this reader recognises"),
            });
        }
        let copied = spec_enums(spec)?;
        let mut index = Self::default();
        for code in enums.all() {
            index.insert(code, ErrorCodeSource::ErrorEnum);
        }
        for code in copied.all() {
            index.insert(code, ErrorCodeSource::SpecEnums);
        }
        let mut tabulated = 0usize;
        for code in table_codes(&spec_section(spec)?) {
            index.insert(&code, ErrorCodeSource::SpecTables);
            tabulated += 1;
        }
        if tabulated == 0 {
            return Err(LogError::CodeSource {
                document: SPEC_SOURCE,
                detail: format!("the section {SPEC_SECTION} has no diagnostic-code table rows"),
            });
        }
        Ok(index)
    }

    /// Reads every declaring source out of the repository rooted at `root`.
    ///
    /// # Errors
    ///
    /// As [`ErrorCodeIndex::from_sources`], plus [`LogError::Io`] when a source cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_repo(root: &Path) -> Result<Self, LogError> {
        let errors = read_source(&root.join(ERROR_SOURCE))?;
        let spec = read_source(&root.join(SPEC_SOURCE))?;
        let mut index = Self::from_sources(&errors, &spec)?;
        index.add_constants(&root.join(CRATE_SOURCES))?;
        Ok(index)
    }

    /// Adds every `*_CODE` string constant found under `root`.
    ///
    /// The `CODE` suffix is what tells a diagnostic code from any other string constant in the
    /// tree: a name that ends in it and whose value has the `domain/action/reason` shape is a
    /// code, and nothing else is. The suffix rather than the shape alone because the shape
    /// cannot tell the two apart -- `crates/ime-ui/ui` is a directory and reads exactly like a
    /// code -- so the name is what a declaration opts in with.
    ///
    /// That makes the convention load-bearing, and it is a *convention*: this reader supplements
    /// the two documents rather than replacing them. A constant named without the suffix is not
    /// read here, and it is still in the set when a diagnostic table of the specification
    /// registers it -- `PHRASE_TABLE_UNAVAILABLE` is such a constant -- so what a missing suffix
    /// costs is that constant's own declaration and never the whitelist as a whole.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when a source file or a directory under `root` cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn add_constants(&mut self, root: &Path) -> Result<usize, LogError> {
        let mut added = 0usize;
        for path in rust_sources(root)? {
            let source = read_source(&path)?;
            for line in source.lines() {
                if let Some(code) = constant_code(line) {
                    self.insert(&code, ErrorCodeSource::Constant);
                    added += 1;
                }
            }
        }
        Ok(added)
    }

    /// Whether the index declares `code`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn contains(&self, code: &str) -> bool {
        self.sources.contains_key(code)
    }

    /// Every code the index declares, in name order.
    pub fn codes(&self) -> Vec<&str> {
        self.sources.keys().map(String::as_str).collect()
    }

    /// The sources that declare `code`.
    pub fn sources_of(&self, code: &str) -> Option<&BTreeSet<ErrorCodeSource>> {
        self.sources.get(code)
    }

    /// How many codes the index declares.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether the index declares no code at all.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Records one declaration of `code`.
    ///
    /// # Panics
    ///
    /// Never.
    fn insert(&mut self, code: &str, source: ErrorCodeSource) {
        self.sources
            .entry(code.to_owned())
            .or_default()
            .insert(source);
    }
}

/// Records the code one `#[error(...)]` attribute declares, under the enum it belongs to.
///
/// An attribute that declares no code, and one that stands outside any enum, both record
/// nothing: the enums of an error source are what this reader is about, and a `#[error(...)]`
/// somewhere else is a fixture rather than a contract.
///
/// # Panics
///
/// Never.
fn record(
    by_enum: &mut BTreeMap<String, BTreeSet<String>>,
    enum_name: Option<&str>,
    attribute: &str,
) {
    let Some(enum_name) = enum_name else {
        return;
    };
    let payload = payload_of(attribute);
    let Some(code) = code_of(&payload) else {
        return;
    };
    by_enum
        .entry(enum_name.to_owned())
        .or_default()
        .insert(code.to_owned());
}

/// The name of the enum a `pub enum <Name> {` line opens, if it opens one.
///
/// # Panics
///
/// Never.
fn enum_name(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("pub enum ")
        .or_else(|| line.strip_prefix("enum "))?;
    let name = rest.split([' ', '{', '<']).next().unwrap_or_default();
    if name.is_empty() || !name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
        return None;
    }
    Some(name.to_owned())
}

/// The message text of an `#[error(...)]` attribute: its string literals, joined.
///
/// A literal that ends with a backslash continues on the next line, and Rust drops the backslash,
/// the newline and the following indentation; joining the literals without the backslash is
/// therefore what the compiler would have produced.
///
/// # Panics
///
/// Never.
fn payload_of(attribute: &str) -> String {
    let mut payload = String::new();
    for literal in string_literals(attribute) {
        match literal.strip_suffix('\\') {
            Some(head) => payload.push_str(head),
            None => payload.push_str(&literal),
        }
    }
    payload
}

/// The string literals of `text`, with their quotes removed and their escapes left as they are.
///
/// # Panics
///
/// Never.
fn string_literals(text: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = text.char_indices();
    while let Some((start, ch)) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut end = text.len();
        let mut escaped = false;
        for (at, inner) in chars.by_ref() {
            if escaped {
                escaped = false;
                continue;
            }
            if inner == '\\' {
                // An escape covers the character after it, so a `\"` does not close the
                // literal. The flag rather than a nested `next()`: the `for` already holds
                // the one mutable borrow of this iterator.
                escaped = true;
                continue;
            }
            if inner == '"' {
                end = at;
                break;
            }
        }
        literals.push(text[start + 1..end].to_owned());
    }
    literals
}

/// The code an `#[error("...")]` payload renders, if it renders one.
///
/// The code is what precedes the first colon: a payload of the shape `code: detail` renders the
/// code and then a description, and one that carries no colon is the code alone.
///
/// # Panics
///
/// Never.
fn code_of(payload: &str) -> Option<&str> {
    let head = match payload.split_once(':') {
        Some((head, _)) => head,
        None => payload,
    };
    let head = head.trim();
    is_code_shape(head).then_some(head)
}

/// Whether `text` has the `domain/action/reason` shape the contract fixes.
///
/// # Panics
///
/// Never.
pub fn is_code_shape(text: &str) -> bool {
    let mut segments = 0usize;
    for segment in text.split('/') {
        segments += 1;
        if segment.is_empty() {
            return false;
        }
        let shaped = segment
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
        if !shaped {
            return false;
        }
    }
    segments >= 2
}

/// The body of the section `opening` introduces, up to the next heading.
///
/// # Panics
///
/// Never.
fn section_of(document: &str, opening: &str) -> Option<String> {
    let mut body = String::new();
    let mut inside = false;
    for line in document.lines() {
        if !inside {
            if line.trim_start().starts_with(opening) {
                inside = true;
            }
            continue;
        }
        // A heading of the same level or above ends the section.
        if is_heading(line) {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    inside.then_some(body)
}

/// The body of the error-code section of `spec`.
///
/// # Errors
///
/// Returns [`LogError::CodeSource`] when the document has no such section.
///
/// # Panics
///
/// Never.
fn spec_section(spec: &str) -> Result<String, LogError> {
    section_of(spec, SPEC_SECTION).ok_or_else(|| LogError::CodeSource {
        document: SPEC_SOURCE,
        detail: format!("it no longer has the section {SPEC_SECTION}"),
    })
}

/// The codes the specification's own copy of the error enums declares, grouped by enum.
///
/// This is the half of the specification that exists to be *compared*: 2.2.4 states that its
/// fenced copy of `crates/ime-types/src/error.rs` must be verbatim, and a caller that holds both
/// can check that promise per enum instead of trusting it.
///
/// # Errors
///
/// Returns [`LogError::CodeSource`] when the document has no error-code section, or when that
/// section copies no enum at all -- an empty result would make a comparison against the source
/// pass for the wrong reason.
///
/// # Panics
///
/// Never.
pub fn spec_enums(spec: &str) -> Result<EnumCodes, LogError> {
    let section = spec_section(spec)?;
    let mut copied = EnumCodes::default();
    for block in fenced_rust(&section) {
        let declared = EnumCodes::parse(&block);
        for name in declared.enums() {
            let codes = declared.codes(name).cloned().unwrap_or_default();
            copied.by_enum.insert(name.to_owned(), codes);
        }
    }
    if copied.is_empty() {
        return Err(LogError::CodeSource {
            document: SPEC_SOURCE,
            detail: format!("the section {SPEC_SECTION} copies no error enum"),
        });
    }
    Ok(copied)
}

/// Whether `line` opens a Markdown heading of the depth this reader treats as a boundary.
///
/// # Panics
///
/// Never.
fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (2..=4).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// Every ```rust block inside `section`, with its fences removed.
///
/// # Panics
///
/// Never.
fn fenced_rust(section: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut open: Option<Vec<&str>> = None;
    for line in section.lines() {
        match line.trim().strip_prefix("```") {
            Some(language) => match open.take() {
                Some(block) => blocks.push(block.join("\n")),
                // Only a fence that names Rust opens a block this reader reads; a fence that
                // names another language is skipped until its own closing fence.
                None => {
                    if language.trim() == "rust" {
                        open = Some(Vec::new());
                    }
                }
            },
            None => {
                if let Some(block) = open.as_mut() {
                    block.push(line);
                }
            }
        }
    }
    blocks
}

/// Every code a diagnostic-code table of `section` lists in its first column.
///
/// # Panics
///
/// Never.
fn table_codes(section: &str) -> Vec<String> {
    let rows: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .collect();
    let mut codes = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let cells = cells_of(row);
        if is_separator(&cells) {
            continue;
        }
        // The header row is the one the separator line follows.
        let followed_by_separator = rows
            .get(index + 1)
            .is_some_and(|next| is_separator(&cells_of(next)));
        if followed_by_separator {
            continue;
        }
        let Some(first) = cells.first() else {
            continue;
        };
        let name = first.trim().trim_matches('`').trim();
        if is_code_shape(name) {
            codes.push(name.to_owned());
        }
    }
    codes
}

/// The cells of a Markdown table row, with the outer pipes removed.
///
/// # Panics
///
/// Never.
fn cells_of(row: &str) -> Vec<String> {
    row.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// Whether a row is the `|---|---|` line that separates a table's header from its body.
///
/// # Panics
///
/// Never.
fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim_matches(':');
            trimmed.len() >= 2 && trimmed.chars().all(|ch| ch == '-')
        })
}

/// The code a `const <NAME>_CODE: ... = "<code>";` line declares.
///
/// # Panics
///
/// Never.
fn constant_code(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("pub const ")
        .or_else(|| trimmed.strip_prefix("const "))?;
    let (name, value) = rest.split_once('=')?;
    let name = name.split(':').next()?.trim();
    let name = name.strip_suffix(CODE_SUFFIX)?;
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return None;
    }
    let value = value.trim().strip_suffix(';')?.trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    if is_code_shape(value) {
        Some(value.to_owned())
    } else {
        None
    }
}

/// Every `.rs` file under `root`, in a deterministic order.
///
/// # Errors
///
/// Returns [`LogError::Io`] when a directory under `root` cannot be listed.
///
/// # Panics
///
/// Never.
fn rust_sources(root: &Path) -> Result<Vec<PathBuf>, LogError> {
    let mut found = Vec::new();
    collect(root, &mut found)?;
    found.sort();
    Ok(found)
}

/// Adds every `.rs` file under `directory` to `found`.
///
/// # Errors
///
/// Returns [`LogError::Io`] when a directory cannot be listed.
///
/// # Panics
///
/// Never.
fn collect(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), LogError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LogError::Io {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| LogError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| LogError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) != Some(BUILD_DIRECTORY) {
                children.push(path);
            }
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            found.push(path);
        }
    }
    children.sort();
    for child in children {
        collect(&child, found)?;
    }
    Ok(())
}

/// Reads one source file as text.
///
/// # Errors
///
/// Returns [`LogError::Io`] when the file cannot be read as UTF-8.
///
/// # Panics
///
/// Never.
fn read_source(path: &Path) -> Result<String, LogError> {
    fs::read_to_string(path).map_err(|source| LogError::Io {
        path: path.to_path_buf(),
        source,
    })
}
