//! The case suite's own metadata: what each case is called and what it declares it needs.
//!
//! Responsibility: read the case documents and pull out, per case, its identifier, its module
//! and the environment it declares. Boundaries: this file reads the suite and interprets
//! nothing -- whether a declared requirement is one the machine can serve is [`super::classify`]'s
//! answer, and this file never asks it.
//!
//! # An undeclared requirement is not a missing field
//!
//! A case whose metadata declares nothing keeps [`EnvRequirement::Undeclared`] rather than
//! acquiring a default. That is deliberate: the gate withholds an undeclared requirement, so a
//! case that has not been told what it needs runs nowhere, which is the only safe reading of a
//! blank. A reader that guessed from the case's own text would be inventing the contract it is
//! supposed to check.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{EnvRequirement, TestCaseMeta};

/// Path of the case suite's hub document, relative to the repository root.
pub const CASE_DOCUMENT: &str = "docs/dev/tests.md";

/// Path of the directory holding the case suite's shards, relative to the repository root.
pub const CASE_SHARD_DIR: &str = "docs/dev/tests";

/// The metadata field a case declares its environment requirement in.
const REQUIREMENT_FIELD: &str = "环境需求";

/// The metadata field a case names its module in.
const MODULE_FIELD: &str = "模块与类别";

/// Reads the case metadata out of a case document.
///
/// One entry per `### TC-<module>-<number>` heading, in document order, with the module and the
/// environment requirement taken from the case's own attribute block. A case that declares no
/// requirement keeps [`EnvRequirement::Undeclared`], which is what makes an undeclared case run
/// nowhere rather than everywhere.
///
/// A heading whose identifier is not shaped like a case identifier -- the suite's own template
/// placeholder among them -- opens no entry.
///
/// # Panics
///
/// Never.
pub fn parse_cases(text: &str) -> Vec<TestCaseMeta> {
    let mut cases = Vec::new();
    let mut current: Option<TestCaseMeta> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(id) = case_heading(trimmed) {
            if let Some(case) = current.take() {
                cases.push(case);
            }
            current = Some(TestCaseMeta {
                module: module_of(&id),
                requirement: EnvRequirement::Undeclared(String::new()),
                id,
            });
            continue;
        }
        let Some(case) = current.as_mut() else {
            continue;
        };
        if let Some(module) = field_value(trimmed, MODULE_FIELD) {
            case.module = module;
        }
        if let Some(declared) = field_value(trimmed, REQUIREMENT_FIELD) {
            case.requirement = EnvRequirement::parse(&declared);
        }
    }
    if let Some(case) = current {
        cases.push(case);
    }
    cases
}

/// Reads the case suite's metadata out of the repository rooted at `root`.
///
/// The suite is a hub document plus one shard per module, and a case is declared the same way
/// in both, so both are read here: a gate that saw only the hub would classify a third of the
/// suite and report a total that is not the suite's.
///
/// # Errors
///
/// Returns an error when the hub, the shard directory or any shard cannot be read.
///
/// # Panics
///
/// Never.
pub fn read_cases(root: &Path) -> Result<Vec<TestCaseMeta>> {
    let mut cases = parse_cases(&super::read_text(&root.join(CASE_DOCUMENT))?);
    for path in markdown_files(&root.join(CASE_SHARD_DIR))? {
        cases.extend(parse_cases(&super::read_text(&path)?));
    }
    Ok(cases)
}

/// The value of a case metadata field, with its backticks removed.
///
/// A field states its value and may then qualify it, so the value is the first cell of the
/// line and everything after a bar is commentary rather than part of the value.
///
/// The suite bolds some attribute names and leaves others plain -- `- **基本属性**：` opens
/// the block and `- 模块与类别：` sits inside it -- so the emphasis is stripped rather than
/// required. Demanding it would read the module of no case at all, silently, which is the
/// failure a metadata reader is least able to notice.
///
/// # Panics
///
/// Never.
fn field_value(line: &str, name: &str) -> Option<String> {
    let rest = line.strip_prefix("- ")?.trim_start();
    let rest = rest.strip_prefix("**").unwrap_or(rest);
    let rest = rest.strip_prefix(name)?;
    let rest = rest.strip_prefix("**").unwrap_or(rest);
    let rest = rest.strip_prefix('：')?;
    let value = rest.split('|').next()?;
    Some(value.trim().trim_matches('`').trim().to_owned())
}

/// The case identifier a heading line opens, when it opens one.
///
/// A heading whose first token is not shaped like a case identifier -- the suite's own template
/// placeholder among them -- opens none, so a placeholder never enters a batch as a case.
///
/// # Panics
///
/// Never.
fn case_heading(line: &str) -> Option<String> {
    let rest = line.strip_prefix("### ")?;
    let id = rest.split_whitespace().next()?;
    let shaped =
        id.starts_with("TC-") && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
    shaped.then(|| id.to_owned())
}

/// The module a case identifier names, lowercased.
///
/// The suite spells the module twice -- once in the identifier (`TC-UI-16`) and once in the
/// metadata -- and the shards only spell it in the identifier. Reading it off the identifier
/// is what keeps a shard case from arriving with an empty module, which would print as
/// `TC-UI-06 []` in a batch report and hide which part of the suite a withheld case belongs
/// to. An explicit `模块与类别` field still wins: a case whose identifier and metadata
/// disagree is a document defect, and the field is the more specific statement of the two.
///
/// The answer is empty when the identifier carries no module segment at all, which is the
/// shape of the suite's own template placeholder.
fn module_of(id: &str) -> String {
    id.split('-')
        .nth(1)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The Markdown documents of a directory, sorted by path.
///
/// The sort is what makes a batch's case order reproducible: a directory listing has no order of
/// its own, and a report whose counts move between runs is a report nobody trusts.
///
/// # Errors
///
/// Returns an error when the directory or any entry of it cannot be read.
///
/// # Panics
///
/// Never.
fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let listing = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in listing {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        if is_markdown(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Whether a path names a Markdown document.
///
/// # Panics
///
/// Never.
fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md")
}
